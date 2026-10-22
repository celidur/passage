#!/usr/bin/env python3
"""End-to-end checks against a real Passage process: API, proxying, forward-auth, TLS,
persistence, load balancing, failover, health checks, timeouts, limits, WebSocket, streaming
and HTTP/2.

Usage: python3 tests/smoke.py [path-to-passage-binary]
Each scenario creates the routes it needs on its own hostname and removes them afterwards.
gRPC and upstream HTTP/2 are covered by `cargo test --test protocols`.
"""
import http.client
import json
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from harness import (ClosingServer, EchoUpstream, FakeAuth, Passage, WebSocket, free_port, request,
                     self_signed_certificate, serve, stop, wait_for)
from fake_acme import FakeAcme


class Environment:
    """Passage plus fake upstreams and a fake forward-auth service."""

    def __init__(self, binary: str, directory: Path):
        self.cert, key = self_signed_certificate(directory)
        tls_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls_context.load_cert_chain(self.cert, key)
        self.upstream = serve(EchoUpstream)
        self.tls_upstream = serve(EchoUpstream, tls_context)
        self.auth = serve(FakeAuth)
        self.acme = FakeAcme(directory)
        self.passage = Passage(binary, directory, self.cert, key, self.acme)

    @staticmethod
    def url(server, scheme='http'):
        return f'{scheme}://127.0.0.1:{server.server_port}'

    def route(self, host, **overrides):
        return {
            'name': 'Demo',
            'host': host,
            'path_prefix': '/',
            'upstreams': [self.url(self.upstream)],
            'enabled': True,
            'forward_auth_url': None,
            **overrides,
        }

    def auth_url(self, path):
        return f'http://127.0.0.1:{self.auth.server_port}{path}'

    def close(self):
        self.passage.close()
        self.acme.close()
        for server in (self.upstream, self.tls_upstream, self.auth):
            server.shutdown()


def echoed_headers(body: bytes) -> dict:
    return {k.lower(): v for k, v in json.loads(body)['headers'].items()}


def served_by(p, host, path='/hello') -> int:
    """The port of the upstream that answered."""
    status, _, body = p.get(host, path)
    assert status == 200, status
    return json.loads(body)['port']


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
    assert p.api('/routes/' + route_id, 'PUT', {**body, 'upstreams': ['file:///etc/passwd']})[0] == 400
    assert p.api('/routes/' + route_id, 'PUT', {**body, 'upstreams': []})[0] == 400
    for policy in ({'timeouts': {'read_secs': 0}}, {'retries': 99}, {'unknown': 1},
                   {'health': {'check': {'path': 'no-slash'}}}):
        status, _, data = p.api('/routes/' + route_id, 'PUT', {**body, 'policy': policy})
        assert status in (400, 422), (policy, status, data)
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
    untrusted = env.url(env.tls_upstream, 'https')
    p.update_route(route_id, **env.route('tls.local', upstreams=[untrusted]))
    assert p.get('tls.local')[0] == 502
    p.delete_route(route_id)


def persistence_across_restart(env):
    """routes and their policies survive a restart, `check` validates them, deletion applies live"""
    p = env.passage
    policy = {'timeouts': {'read_secs': 7}, 'balancing': 'least_connections'}
    route_id = p.create_route(**env.route('persist.local', policy=policy))
    p.stop()
    assert p.command('check') == 0
    p.start()
    stored = {r['id']: r for r in json.loads(p.api('/routes')[2])}
    assert stored[route_id]['policy']['timeouts']['read_secs'] == 7
    assert stored[route_id]['policy']['balancing'] == 'least_connections'
    assert p.get('persist.local')[0] == 200
    p.delete_route(route_id)
    assert p.get('persist.local')[0] == 404


def traffic_is_shared_across_upstreams(env):
    """round-robin splits traffic evenly; least-connections still uses every upstream"""
    p = env.passage
    first, second = serve(EchoUpstream), serve(EchoUpstream)
    try:
        urls = [env.url(first), env.url(second)]
        route_id = p.create_route(**env.route('share.local', upstreams=urls))
        served = Counter(served_by(p, 'share.local') for _ in range(20))
        assert served == {first.server_port: 10, second.server_port: 10}, served

        policy = {'balancing': 'least_connections'}
        p.update_route(route_id, **env.route('share.local', upstreams=urls, policy=policy))
        served = Counter(served_by(p, 'share.local') for _ in range(20))
        assert set(served) == {first.server_port, second.server_port}, served
        p.delete_route(route_id)
    finally:
        stop(first)
        stop(second)


def dead_upstream_is_skipped_then_rejoins(env):
    """a dead upstream costs no failed request, is set aside, and rejoins once it is back"""
    p = env.passage
    dead_port = free_port()
    urls = [env.url(env.upstream), f'http://127.0.0.1:{dead_port}']
    policy = {'health': {'max_failures': 2, 'cooldown_secs': 2}}
    route_id = p.create_route(**env.route('failover.local', upstreams=urls, policy=policy))
    assert [p.get('failover.local')[0] for _ in range(10)] == [200] * 10
    states = {b['address']: b['healthy'] for b in p.upstreams(route_id)['backends']}
    assert states[f'127.0.0.1:{dead_port}'] is False and states[
        f'127.0.0.1:{env.upstream.server_port}'] is True, states

    revived = serve(EchoUpstream, port=dead_port)
    try:
        wait_for(lambda: served_by(p, 'failover.local') == dead_port,
                 message='the recovered upstream never rejoined')
        assert all(b['healthy'] for b in p.upstreams(route_id)['backends'])
    finally:
        stop(revived)
    p.delete_route(route_id)


def unavailable_upstreams_fail_fast_and_recover(env):
    """with every upstream down requests get a prompt 502, and traffic resumes when one returns"""
    p = env.passage
    ports = [free_port(), free_port()]
    urls = [f'http://127.0.0.1:{port}' for port in ports]
    route_id = p.create_route(**env.route('down.local', upstreams=urls))
    started = time.monotonic()
    assert {p.get('down.local')[0] for _ in range(8)} == {502}
    assert time.monotonic() - started < 3

    revived = serve(EchoUpstream, port=ports[0])
    try:
        wait_for(lambda: p.get('down.local')[0] == 200, message='traffic did not resume')
    finally:
        stop(revived)
    p.delete_route(route_id)


def hostname_upstreams_survive_a_dead_address(env):
    """`localhost` may resolve to ::1 and 127.0.0.1; the unreachable one is skipped"""
    p = env.passage
    url = f'http://localhost:{env.upstream.server_port}'
    route_id = p.create_route(**env.route('names.local', upstreams=[url]))
    assert [p.get('names.local')[0] for _ in range(10)] == [200] * 10
    assert 1 <= len(p.upstreams(route_id)['backends']) <= 2
    p.delete_route(route_id)


def active_health_check_gates_traffic(env):
    """an upstream failing its health check receives no traffic until it passes again"""
    p = env.passage
    steady, flaky = serve(EchoUpstream), serve(EchoUpstream)
    check = {'path': '/health', 'interval_secs': 1, 'timeout_secs': 1,
             'healthy_threshold': 1, 'unhealthy_threshold': 1}
    try:
        urls = [env.url(steady), env.url(flaky)]
        route_id = p.create_route(**env.route(
            'check.local', upstreams=urls, policy={'health': {'check': check}}))
        flaky.RequestHandlerClass.healthy = False
        wait_for(lambda: {served_by(p, 'check.local') for _ in range(6)} == {steady.server_port},
                 message='failing upstream kept receiving traffic')
        assert [b['healthy'] for b in p.upstreams(route_id)['backends']].count(False) == 1

        flaky.RequestHandlerClass.healthy = True
        wait_for(lambda: {served_by(p, 'check.local') for _ in range(6)}
                 == {steady.server_port, flaky.server_port},
                 message='recovered upstream never rejoined')
        p.delete_route(route_id)
    finally:
        stop(steady)
        stop(flaky)


def tcp_health_check_flags_a_dead_upstream_without_traffic(env):
    """a TCP check sets a dead upstream aside before any request has reached it"""
    p = env.passage
    urls = [env.url(env.upstream), f'http://127.0.0.1:{free_port()}']
    check = {'kind': 'tcp', 'interval_secs': 1, 'timeout_secs': 1, 'unhealthy_threshold': 1}
    route_id = p.create_route(**env.route(
        'tcp.local', upstreams=urls, policy={'health': {'check': check}}))
    wait_for(lambda: [b['healthy'] for b in p.upstreams(route_id)['backends']].count(False) == 1,
             message='the dead upstream was never flagged')
    p.delete_route(route_id)


def crashing_upstream_is_bypassed_for_idempotent_requests_only(env):
    """a backend dropping connections is bypassed for GET, but a POST is never replayed"""
    p = env.passage
    crasher = ClosingServer()
    urls = [env.url(crasher), env.url(env.upstream)]
    policy = {'health': {'max_failures': 100}}  # keep the crasher in rotation
    route_id = p.create_route(**env.route('crash.local', upstreams=urls, policy=policy))
    assert [p.get('crash.local')[0] for _ in range(10)] == [200] * 10
    posts = [request(p.proxy_port, '/', 'POST', b'x', {'Host': 'crash.local'})[0] for _ in range(10)]
    assert sorted(set(posts)) == [200, 502], posts
    p.delete_route(route_id)
    crasher.socket.close()


def slow_upstream_times_out(env):
    """an upstream slower than read_secs yields 504 on time and does not affect other requests"""
    p = env.passage
    route_id = p.create_route(**env.route('slow.local', policy={'timeouts': {'read_secs': 1}}))
    started = time.monotonic()
    assert p.get('slow.local', '/slow?delay=4', timeout=10)[0] == 504
    assert 0.9 < time.monotonic() - started < 2.5
    assert p.get('slow.local', '/fast')[0] == 200
    p.delete_route(route_id)


def connection_limit_sheds_load(env):
    """requests beyond max_connections get 503 and capacity returns when they finish"""
    p = env.passage
    route_id = p.create_route(**env.route('limit.local', policy={'limits': {'max_connections': 2}}))
    with ThreadPoolExecutor(2) as pool:
        busy = [pool.submit(p.get, 'limit.local', '/slow?delay=2', timeout=10) for _ in range(2)]
        wait_for(lambda: p.upstreams(route_id)['active'] == 2, message='requests not admitted')
        assert p.get('limit.local')[0] == 503
        assert [f.result()[0] for f in busy] == [200, 200]
    assert p.get('limit.local')[0] == 200
    p.delete_route(route_id)


def request_body_limit(env):
    """oversized request bodies get 413, whether declared or sent chunked"""
    p = env.passage
    route_id = p.create_route(
        **env.route('body.local', policy={'limits': {'max_request_body_bytes': 100}}))
    host = {'Host': 'body.local'}
    status, _, body = request(p.proxy_port, '/', 'POST', b'x' * 100, host)
    assert status == 200 and json.loads(body)['received'] == 100
    assert request(p.proxy_port, '/', 'POST', b'x' * 101, host)[0] == 413
    chunked = {**host, 'Transfer-Encoding': 'chunked'}
    status = request(p.proxy_port, '/', 'POST', iter([b'x' * 60, b'x' * 60]), chunked, chunked=True)[0]
    assert status == 413
    p.delete_route(route_id)


def websocket_is_proxied_both_ways(env):
    """a WebSocket upgrade is relayed, messages flow both ways, and the slot is released on close"""
    p = env.passage
    route_id = p.create_route(**env.route('ws.local'))
    socket = WebSocket(p.proxy_port, 'ws.local')
    try:
        assert socket.status == 101 and socket.accepted
        assert socket.exchange('hello') == 'echo:hello'
        assert socket.exchange('x' * 70000) == 'echo:' + 'x' * 70000
        assert p.upstreams(route_id)['active'] == 1
    finally:
        socket.close()
    wait_for(lambda: p.upstreams(route_id)['active'] == 0, message='slot not released')
    p.delete_route(route_id)


def streaming_is_not_buffered(env):
    """a response is relayed as it is produced, not after it completes"""
    p = env.passage
    route_id = p.create_route(**env.route('stream.local'))
    connection = http.client.HTTPConnection('127.0.0.1', p.proxy_port, timeout=10)
    started = time.monotonic()
    connection.request('GET', '/stream', headers={'Host': 'stream.local'})
    response = connection.getresponse()
    assert response.read(6) == b'first\n'
    first_at = time.monotonic() - started
    assert response.read() == b'second\n'
    assert first_at < 1.0 < 1.4 < time.monotonic() - started, first_at
    connection.close()
    p.delete_route(route_id)


def silent_streams_and_websockets_follow_read_timeout(env):
    """read_secs cuts streams and WebSockets that stay silent longer than it, and only those"""
    p = env.passage
    host = 'silence.local'

    def stream_completes():
        connection = http.client.HTTPConnection('127.0.0.1', p.proxy_port, timeout=10)
        connection.request('GET', '/stream', headers={'Host': host})  # pauses 1.5 s mid-body
        try:
            return connection.getresponse().read() == b'first\nsecond\n'
        except http.client.IncompleteRead:
            return False
        finally:
            connection.close()

    def websocket_survives_idle():
        socket = WebSocket(p.proxy_port, host)
        try:
            time.sleep(2.5)
            return socket.exchange('late') == 'echo:late'
        except (OSError, TypeError):  # reset, or end of stream
            return False
        finally:
            socket.close()

    route_id = p.create_route(**env.route(host, policy={'timeouts': {'read_secs': 1}}))
    assert not stream_completes() and not websocket_survives_idle()
    p.update_route(route_id, **env.route(host, policy={'timeouts': {'read_secs': 5}}))
    assert stream_completes() and websocket_survives_idle()
    p.delete_route(route_id)


def http2_clients_are_accepted(env):
    """HTTP/2 works over TLS (ALPN) and in cleartext (prior knowledge); HTTP/1.1 is unchanged"""
    if shutil.which('curl') is None:
        print('SKIP curl is not installed; HTTP/2 downstream not checked')
        return
    p = env.passage
    route_id = p.create_route(**env.route('h2.local'))

    def fetch(*args):
        result = subprocess.run(
            ['curl', '-sS', '-o', '/dev/null', '-w', '%{http_version} %{http_code}',
             '-H', 'Host: h2.local', *args],
            capture_output=True, text=True, timeout=10)
        return result.stdout

    assert fetch('--http2', '--cacert', str(env.cert), f'https://127.0.0.1:{p.tls_port}/') == '2 200'
    assert fetch('--http2-prior-knowledge', f'http://127.0.0.1:{p.proxy_port}/') == '2 200'
    http1 = fetch(f'http://127.0.0.1:{p.proxy_port}/')
    assert http1 in ('1 200', '1.1 200'), http1  # curl versions differ in how they print 1.1
    p.delete_route(route_id)


def wait_certificate(p, domain, status='valid', timeout=40):
    return wait_for(lambda: (c := p.certificate_for(domain)) and c['status'] == status and c,
                    timeout=timeout, message=f'certificate for {domain} never became {status}: '
                    f'{p.certificate_for(domain)}')


def presented_serial(p, name, ca):
    return p.peer_certificate(name, ca)['serialNumber']


def remove_certificates(p):
    for certificate in p.certificates():
        assert p.api('/certificates/' + certificate['id'], 'DELETE')[0] == 204


def https_is_enabled_from_a_route_and_served_by_sni(env):
    """enabling HTTPS on a host orders its certificate; each name is served its own, live and after restart"""
    p, ca = env.passage, env.acme.ca_cert
    hosts = ['shop.acme.test', 'blog.acme.test']
    route_ids = [p.create_route(**env.route(host, https={'enabled': True})) for host in hosts]
    for host in hosts:
        certificate = wait_certificate(p, host)
        assert certificate['domains'] == [host] and certificate['challenge'] == 'http-01'
        assert certificate['not_after'] > time.time() and certificate['last_error'] is None
    shop, blog = (p.peer_certificate(host, ca) for host in hosts)
    assert ('DNS', 'shop.acme.test') in shop['subjectAltName']
    assert ('DNS', 'shop.acme.test') not in blog['subjectAltName']
    assert shop['serialNumber'] != blog['serialNumber']

    status, body = p.https_get('shop.acme.test', ca, '/hello')
    assert status == 200 and json.loads(body)['path'] == '/hello'
    fallback = p.peer_certificate('unknown.acme.test')['pem']
    assert fallback.strip() == env.cert.read_text().strip(), 'unknown names get the fallback certificate'

    issued = len(env.acme.issued)
    p.stop()
    p.start()
    assert wait_for(lambda: presented_serial_or_none(p, 'shop.acme.test', ca)) == shop['serialNumber']
    assert len(env.acme.issued) == issued, 'a restart must not order certificates again'
    in_use = p.certificate_for('shop.acme.test')
    status, _, body = p.api('/certificates/' + in_use['id'], 'DELETE')
    assert status == 400 and 'shop.acme.test' in json.loads(body)['error'], (status, body)
    for route_id in route_ids:
        p.delete_route(route_id)
    remove_certificates(p)


def presented_serial_or_none(p, name, ca):
    try:
        return presented_serial(p, name, ca)
    except OSError:  # the listener is not up yet
        return None


def plain_http_redirects_to_https_when_asked(env):
    """redirect sends HTTP to HTTPS with path and query, yet the CA's proof is still served over HTTP"""
    p = env.passage
    redirect = p.create_route(**env.route('redirect.acme.test', https={'enabled': True, 'redirect': True}))
    plain = p.create_route(**env.route('plain.acme.test'))
    status, headers, _ = p.get('redirect.acme.test', '/a/b?x=1')
    assert status == 308
    assert headers['location'] == f'https://redirect.acme.test:{p.tls_port}/a/b?x=1', headers
    assert p.get('plain.acme.test')[0] == 200, 'hosts without redirect stay on HTTP'
    # Validation fetched the proof over HTTP from a host that redirects everything else.
    wait_certificate(p, 'redirect.acme.test')
    for route_id in (redirect, plain):
        p.delete_route(route_id)
    remove_certificates(p)


def renewal_replaces_the_certificate_without_dropping_traffic(env):
    """a renewed certificate is served from the next handshake on, while requests keep succeeding"""
    p, ca, host = env.passage, env.acme.ca_cert, 'renew.acme.test'
    route_id = p.create_route(**env.route(host, https={'enabled': True}))
    certificate = wait_certificate(p, host)
    before = presented_serial(p, host, ca)

    results, running = [], threading.Event()
    running.set()

    def hammer():
        while running.is_set():
            try:
                results.append(p.https_get(host, ca)[0])
            except Exception as error:  # a failed handshake is a dropped request
                results.append(repr(error))

    thread = threading.Thread(target=hammer)
    thread.start()
    try:
        status, _, body = p.api('/certificates/' + certificate['id'] + '/renew', 'POST')
        assert status == 200, (status, body)
        wait_for(lambda: presented_serial(p, host, ca) != before, timeout=40,
                 message='the renewed certificate was never served')
        time.sleep(0.5)
    finally:
        running.clear()
        thread.join()
    assert len(results) > 20 and set(results) == {200}, Counter(results)
    assert p.certificate_for(host)['last_error'] is None
    p.delete_route(route_id)
    remove_certificates(p)


def wildcard_certificates_use_the_dns_hook(env):
    """a wildcard certificate proves control through the DNS hook, then covers new hosts without orders"""
    p, ca = env.passage, env.acme.ca_cert
    body = {'domains': ['*.wild.acme.test', 'wild.acme.test'], 'challenge': 'dns-01'}
    assert p.api('/certificates', 'POST', {**body, 'challenge': 'http-01'})[0] == 400, 'wildcards need dns-01'
    status, _, data = p.api('/certificates', 'POST', body)
    assert status == 201, (status, data)
    certificate = wait_certificate(p, 'wild.acme.test')
    assert certificate['challenge'] == 'dns-01'
    assert sorted(env.acme.issued[-1]['names']) == ['*.wild.acme.test', 'wild.acme.test']
    assert env.acme.dns_records() == set(), 'the hook must remove its records afterwards'
    assert p.api('/certificates', 'POST', body)[0] == 400, 'the same domains cannot be ordered twice'

    orders = len(env.acme.issued)
    route_id = p.create_route(**env.route('www.wild.acme.test', https={'enabled': True}))
    assert len(p.certificates()) == 1 and len(env.acme.issued) == orders
    assert ('DNS', '*.wild.acme.test') in p.peer_certificate('www.wild.acme.test', ca)['subjectAltName']
    assert p.https_get('www.wild.acme.test', ca)[0] == 200
    p.delete_route(route_id)
    remove_certificates(p)


def certificate_failures_are_reported_and_can_be_retried(env):
    """a refused order shows the CA's reason and a retry time, and a manual renewal recovers"""
    p, host = env.passage, 'broken.acme.test'
    env.acme.failure = 'the CA could not reach this domain'
    try:
        route_id = p.create_route(**env.route(host, https={'enabled': True}))
        failed = wait_certificate(p, host, 'failed')
    finally:
        env.acme.failure = None
    assert 'the CA could not reach this domain' in failed['last_error'], failed
    assert failed['failures'] == 1 and failed['next_attempt'] > time.time() + 60, failed
    assert failed['not_after'] is None

    status, _, data = p.api('/certificates/' + failed['id'] + '/renew', 'POST')
    assert status == 200, (status, data)
    recovered = wait_certificate(p, host)
    assert recovered['last_error'] is None and recovered['failures'] == 0
    p.delete_route(route_id)
    remove_certificates(p)


def https_settings_are_validated(env):
    """HTTPS needs a public name, redirects need HTTPS, and certificate requests are checked"""
    p = env.passage
    body = env.route('valid.acme.test')
    assert p.api('/routes', 'POST', {**body, 'https': {'redirect': True}})[0] == 400
    assert p.api('/routes', 'POST', {**env.route('intranet', https={'enabled': True})})[0] == 400
    assert p.get('valid.acme.test')[0] == 404, 'a rejected route must not exist'
    assert p.certificates() == [], 'a rejected route must not order a certificate'
    for request_body in ({'domains': []}, {'domains': ['localhost']}, {'domains': ['a.test'], 'extra': 1}):
        assert p.api('/certificates', 'POST', request_body)[0] in (400, 422), request_body
    assert p.api('/certificates/missing', 'DELETE')[0] == 404
    assert p.api('/certificates/missing/renew', 'POST')[0] == 404


def load_burst_and_upstream_crash(env):
    """a burst is served completely; an upstream dying mid-burst costs at most a few requests"""
    p = env.passage
    first, second = serve(EchoUpstream), serve(EchoUpstream)
    route_id = p.create_route(**env.route(
        'burst.local', upstreams=[env.url(first), env.url(second)]))

    def fetch(_):
        try:
            status, _, body = p.get('burst.local', '/hello', timeout=10)
            return status, json.loads(body)['port'] if status == 200 else 0
        except OSError:
            return 0, 0

    with ThreadPoolExecutor(32) as pool:
        results = list(pool.map(fetch, range(400)))
    assert [status for status, _ in results].count(200) == 400, Counter(s for s, _ in results)
    shares = Counter(port for _, port in results)
    assert min(shares.values()) > 120, shares

    with ThreadPoolExecutor(32) as pool:
        futures = [pool.submit(fetch, i) for i in range(400)]
        time.sleep(0.05)
        stop(second)
        statuses = [f.result()[0] for f in futures]
    assert statuses.count(200) >= 380, Counter(statuses)
    assert [fetch(i)[0] for i in range(50)] == [200] * 50
    assert [b['healthy'] for b in p.upstreams(route_id)['backends']].count(False) == 1
    p.delete_route(route_id)
    stop(first)


SCENARIOS = [
    administration_requires_token,
    proxies_and_strips_spoofed_identity,
    invalid_changes_keep_live_configuration,
    enable_disable_applies_live,
    forward_auth,
    ambiguous_paths_cannot_bypass_auth,
    tls,
    persistence_across_restart,
    traffic_is_shared_across_upstreams,
    dead_upstream_is_skipped_then_rejoins,
    unavailable_upstreams_fail_fast_and_recover,
    hostname_upstreams_survive_a_dead_address,
    active_health_check_gates_traffic,
    tcp_health_check_flags_a_dead_upstream_without_traffic,
    crashing_upstream_is_bypassed_for_idempotent_requests_only,
    slow_upstream_times_out,
    connection_limit_sheds_load,
    request_body_limit,
    websocket_is_proxied_both_ways,
    streaming_is_not_buffered,
    silent_streams_and_websockets_follow_read_timeout,
    http2_clients_are_accepted,
    load_burst_and_upstream_crash,
    https_settings_are_validated,
    https_is_enabled_from_a_route_and_served_by_sni,
    plain_http_redirects_to_https_when_asked,
    renewal_replaces_the_certificate_without_dropping_traffic,
    wildcard_certificates_use_the_dns_hook,
    certificate_failures_are_reported_and_can_be_retried,
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
