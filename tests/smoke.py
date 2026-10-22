#!/usr/bin/env python3
"""End-to-end checks against a real Passage process: API, proxying, forward-auth, TLS, persistence.

Usage: python3 tests/smoke.py [path-to-passage-binary]
Each scenario creates the routes it needs on its own hostname and removes them afterwards.
"""
import json
import ssl
import sys
import tempfile
from pathlib import Path

from harness import EchoUpstream, FakeAuth, Passage, free_port, request, self_signed_certificate, serve


class Environment:
    """Passage plus fake upstreams and a fake forward-auth service."""

    def __init__(self, binary: str, directory: Path):
        self.cert, key = self_signed_certificate(directory)
        tls_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls_context.load_cert_chain(self.cert, key)
        self.upstream = serve(EchoUpstream)
        self.tls_upstream = serve(EchoUpstream, tls_context)
        self.auth = serve(FakeAuth)
        self.passage = Passage(binary, directory, self.cert, key)

    def route(self, host, **overrides):
        return {
            'name': 'Demo',
            'host': host,
            'path_prefix': '/',
            'upstream': f'http://127.0.0.1:{self.upstream.server_port}',
            'enabled': True,
            'forward_auth_url': None,
            **overrides,
        }

    def auth_url(self, path):
        return f'http://127.0.0.1:{self.auth.server_port}{path}'

    def close(self):
        self.passage.close()
        for server in (self.upstream, self.tls_upstream, self.auth):
            server.shutdown()


def echoed_headers(body: bytes) -> dict:
    return {k.lower(): v for k, v in json.loads(body)['headers'].items()}


def administration_requires_token(env):
    """administration requires the token; dashboard is embedded"""
    p = env.passage
    assert request(p.admin_port, '/api/routes')[0] == 401
    assert request(p.admin_port, '/api/routes', 'POST', {})[0] == 401
    status, _, body = request(p.admin_port, '/')
    assert status == 200 and b'Passage' in body
    assert p.api('/routes/missing', 'DELETE')[0] == 404


def proxies_and_strips_spoofed_identity(env):
    """upstream receives path/query intact, regenerated forwarding headers, no spoofed identity"""
    p = env.passage
    route_id = p.create_route(**env.route('proxy.local'))
    status, _, body = p.get('proxy.local', '/hello?q=yes', {
        'Remote-User': 'attacker',
        'X-Forwarded-For': '1.2.3.4',
        'X-Authentik-Username': 'attacker',
    })
    assert status == 200
    assert json.loads(body)['path'] == '/hello?q=yes'
    received = echoed_headers(body)
    assert received['host'] == f'127.0.0.1:{env.upstream.server_port}'
    assert received['x-forwarded-for'] == '127.0.0.1'
    assert received['x-forwarded-host'] == 'proxy.local'
    assert 'remote-user' not in received and 'x-authentik-username' not in received
    assert p.get('other.local')[0] == 404
    p.delete_route(route_id)
    assert p.get('proxy.local')[0] == 404


def invalid_changes_keep_live_configuration(env):
    """duplicate, invalid and unknown-route changes are rejected without affecting traffic"""
    p = env.passage
    body = env.route('invalid.local')
    route_id = p.create_route(**body)
    assert p.api('/routes', 'POST', body)[0] == 400
    assert p.api('/routes/' + route_id, 'PUT', {**body, 'upstream': 'file:///etc/passwd'})[0] == 400
    assert p.api('/routes/missing', 'PUT', body)[0] == 404
    assert p.get('invalid.local')[0] == 200
    p.delete_route(route_id)


def enable_disable_applies_live(env):
    """disabling and re-enabling a route takes effect without a restart"""
    p = env.passage
    route_id = p.create_route(**env.route('toggle.local'))
    p.update_route(route_id, **env.route('toggle.local', enabled=False))
    assert p.get('toggle.local')[0] == 404
    p.update_route(route_id, **env.route('toggle.local'))
    assert p.get('toggle.local')[0] == 200
    p.delete_route(route_id)


def forward_auth(env):
    """forward-auth denies, propagates verified identity, relays redirects and fails closed"""
    p = env.passage
    host = 'sso.local'
    route_id = p.create_route(**env.route(host, forward_auth_url=env.auth_url('/verify')))
    assert p.get(host)[0] == 401

    status, _, body = p.get(host, '/private', {'Cookie': 'session=valid', 'Remote-User': 'attacker'})
    assert status == 200
    assert echoed_headers(body)['remote-user'] == 'verified-user'
    assert FakeAuth.last_headers['x-forwarded-host'] == host
    assert FakeAuth.last_headers['x-forwarded-uri'] == '/private'
    assert 'remote-user' not in FakeAuth.last_headers

    p.update_route(route_id, **env.route(host, forward_auth_url=env.auth_url('/redirect')))
    before = FakeAuth.hits
    status, headers, _ = p.get(host)
    assert status == 302 and headers['location'] == '/login' and FakeAuth.hits == before + 1

    unreachable = f'http://127.0.0.1:{free_port()}/verify'
    p.update_route(route_id, **env.route(host, forward_auth_url=unreachable))
    assert p.get(host)[0] == 503
    p.delete_route(route_id)


def ambiguous_paths_cannot_bypass_auth(env):
    """encoded, duplicated and dot-segment paths cannot bypass a protected prefix"""
    p = env.passage
    host = 'paths.local'
    public = p.create_route(**env.route(host))
    guarded = p.create_route(**env.route(
        host, name='Guarded', path_prefix='/admin', forward_auth_url=env.auth_url('/verify')))
    for path in ['/admin', '//admin/x', '/%61dmin/x']:
        assert p.get(host, path)[0] == 401, path
    for path in ['/x/../admin', '/%2e%2e/admin', '/admin\\x']:
        assert p.get(host, path)[0] == 400, path
    assert p.get(host, '/public')[0] == 200
    p.delete_route(guarded)
    p.delete_route(public)


def tls(env):
    """inbound TLS terminates; untrusted upstream certificates are rejected"""
    p = env.passage
    route_id = p.create_route(**env.route('tls.local'))
    client = ssl.create_default_context(cafile=str(env.cert))
    assert p.get('tls.local', tls_context=client)[0] == 200
    untrusted = f'https://127.0.0.1:{env.tls_upstream.server_port}'
    p.update_route(route_id, **env.route('tls.local', upstream=untrusted))
    assert p.get('tls.local')[0] == 502
    p.delete_route(route_id)


def persistence_across_restart(env):
    """routes survive a restart, `check` validates them, deletion applies live"""
    p = env.passage
    route_id = p.create_route(**env.route('persist.local'))
    p.stop()
    assert p.command('check') == 0
    p.start()
    assert route_id in [r['id'] for r in json.loads(p.api('/routes')[2])]
    assert p.get('persist.local')[0] == 200
    p.delete_route(route_id)
    assert p.get('persist.local')[0] == 404


SCENARIOS = [
    administration_requires_token,
    proxies_and_strips_spoofed_identity,
    invalid_changes_keep_live_configuration,
    enable_disable_applies_live,
    forward_auth,
    ambiguous_paths_cannot_bypass_auth,
    tls,
    persistence_across_restart,
]


def main():
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/passage').resolve())
    with tempfile.TemporaryDirectory(prefix='passage-smoke-') as directory:
        env = Environment(binary, Path(directory))
        try:
            env.passage.init()
            assert env.passage.command('init') != 0, 'init must never overwrite the token'
            env.passage.start()
            for scenario in SCENARIOS:
                scenario(env)
                print('PASS', scenario.__doc__)
            print('All smoke checks passed.')
        except Exception:
            print(env.passage.output(), file=sys.stderr)
            raise
        finally:
            env.close()


if __name__ == '__main__':
    main()
