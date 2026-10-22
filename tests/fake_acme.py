"""A small ACME certificate authority for tests. Standard library plus the `openssl` command.

It follows RFC 8555 closely enough for real clients, but skips what tests do not need: request
signatures and nonces are not verified, and every order is validated and issued synchronously.
HTTP-01 proofs are fetched from the Passage proxy; DNS-01 proofs are read from the log written by
the DNS hook script (`dns_hook`). Certificates are signed by a throwaway CA (`ca_cert`).
"""
import base64
import hashlib
import http.server
import json
import secrets
import ssl
import subprocess
import tempfile
import threading
import urllib.request
from pathlib import Path

from harness import serve


def b64e(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b'=').decode()


def b64d(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + '=' * (-len(text) % 4))


def openssl(*args):
    subprocess.run(['openssl', *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


class FakeAcme:
    def __init__(self, directory: Path):
        self.dir = directory / 'acme'
        self.dir.mkdir()
        self.ca_cert, self.ca_key = self.dir / 'ca.pem', self.dir / 'ca.key'
        openssl('ecparam', '-genkey', '-name', 'prime256v1', '-noout', '-out', str(self.ca_key))
        openssl('req', '-x509', '-new', '-key', str(self.ca_key), '-days', '3650', '-out', str(self.ca_cert),
                '-subj', '/CN=Passage test CA', '-addext', 'basicConstraints=critical,CA:TRUE',
                '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
        server_cert, server_key = self.dir / 'server.pem', self.dir / 'server.key'
        self.sign_request(server_key, server_cert, ['IP:127.0.0.1'])
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(server_cert, server_key)

        self.proxy_port = None  # where HTTP-01 proofs are fetched; set once Passage is configured
        self.valid_days = 60
        self.failure = None  # when set, every validation fails with this message
        self.thumbprint = None
        self.orders = {}
        self.issued = []  # one {'names': [...], 'serial': int} per certificate signed
        self.lock = threading.Lock()
        self.dns_log = self.dir / 'dns.log'
        self.dns_log.touch()
        self.hook = self.dir / 'dns-hook.sh'
        self.hook.write_text(f'#!/bin/sh\necho "$1 $2 $3" >> "{self.dns_log}"\n')
        self.hook.chmod(0o755)
        handler = type('Handler', (AcmeHandler,), {'acme': self})
        self.server = serve(handler, context)

    @property
    def directory_url(self):
        return f'https://127.0.0.1:{self.server.server_port}/directory'

    def close(self):
        self.server.shutdown()

    def sign_request(self, key: Path, cert: Path, alt_names: list, days=30):
        """Creates a key and a certificate signed by the CA for `alt_names` (e.g. `DNS:a.test`)."""
        request = self.dir / (cert.name + '.csr')
        openssl('req', '-new', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes',
                '-keyout', str(key), '-out', str(request), '-subj', '/CN=test')
        self.sign_csr(request, cert, alt_names, days)

    def sign_csr(self, request: Path, cert: Path, alt_names: list, days: int) -> int:
        extensions = self.dir / (cert.name + '.ext')
        # Like real CAs: the subject is empty, so the SAN extension must be critical.
        extensions.write_text('subjectAltName=critical,' + ','.join(alt_names) + '\n'
                              'basicConstraints=CA:FALSE\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid\n')
        serial = secrets.randbits(63) | 1
        openssl('x509', '-req', '-in', str(request), '-CA', str(self.ca_cert), '-CAkey', str(self.ca_key),
                '-set_serial', str(serial), '-days', str(days), '-extfile', str(extensions), '-out', str(cert))
        return serial

    def dns_records(self) -> set:
        """TXT records currently published by the DNS hook, as (name, value)."""
        records = set()
        for line in self.dns_log.read_text().splitlines():
            action, name, value = line.split(' ')
            (records.add if action == 'present' else records.discard)((name, value))
        return records

    def check(self, name: str, kind: str, token: str):
        """Validates a challenge; returns None on success or the reason for failure."""
        if self.failure:
            return self.failure
        proof = f'{token}.{self.thumbprint}'
        if kind == 'http-01':
            request = urllib.request.Request(
                f'http://127.0.0.1:{self.proxy_port}/.well-known/acme-challenge/{token}', headers={'Host': name})
            try:
                with urllib.request.urlopen(request, timeout=5) as response:
                    body = response.read().decode()
            except Exception as error:
                return f'could not fetch http://{name}/.well-known/acme-challenge/{token}: {error}'
            return None if body == proof else f'{name} served the wrong proof'
        expected = b64e(hashlib.sha256(proof.encode()).digest())
        if (f'_acme-challenge.{name}', expected) in self.dns_records():
            return None
        return f'no TXT record _acme-challenge.{name} with the expected value'

    def status(self, order: dict) -> str:
        if order['status'] != 'pending':
            return order['status']
        states = [authz['status'] for authz in order['authz']]
        if 'invalid' in states:
            return 'invalid'
        return 'ready' if all(state == 'valid' for state in states) else 'pending'


class AcmeHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    acme = None

    def base(self) -> str:
        return f'https://127.0.0.1:{self.server.server_port}'

    def send_body(self, status, body, content_type='application/json', headers=None):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Replay-Nonce', secrets.token_urlsafe(16))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(data)

    def do_HEAD(self):
        self.send_body(200, b'')

    def do_GET(self):
        if self.path != '/directory':
            return self.send_body(200, b'')
        base = self.base()
        self.send_body(200, {
            'newNonce': base + '/new-nonce', 'newAccount': base + '/new-account', 'newOrder': base + '/new-order',
            'revokeCert': base + '/revoke', 'keyChange': base + '/key-change',
        })

    def do_POST(self):
        jws = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        protected = json.loads(b64d(jws['protected']))
        payload = json.loads(b64d(jws['payload'])) if jws['payload'] else None
        parts = self.path.strip('/').split('/')
        with self.acme.lock:
            getattr(self, 'post_' + parts[0].replace('-', '_'))(protected, payload, parts[1:])

    def post_new_account(self, protected, payload, _):
        jwk = {key: protected['jwk'][key] for key in ('crv', 'kty', 'x', 'y')}
        self.acme.thumbprint = b64e(hashlib.sha256(json.dumps(jwk, sort_keys=True, separators=(',', ':')).encode()).digest())
        self.send_body(201, {'status': 'valid'}, headers={'Location': self.base() + '/acct/1'})

    def post_new_order(self, protected, payload, _):
        number = str(len(self.acme.orders) + 1)
        authz = []
        for name in (identifier['value'] for identifier in payload['identifiers']):
            wildcard = name.startswith('*.')
            authz.append({'name': name.removeprefix('*.'), 'wildcard': wildcard, 'status': 'pending', 'error': None,
                          'tokens': {'dns-01': secrets.token_urlsafe(24), 'http-01': secrets.token_urlsafe(24)}})
        self.acme.orders[number] = {'names': [i['value'] for i in payload['identifiers']], 'authz': authz,
                                    'status': 'pending', 'certificate': None}
        self.send_body(201, self.order_json(number), headers={'Location': f'{self.base()}/order/{number}'})

    def order_json(self, number):
        order = self.acme.orders[number]
        body = {
            'status': self.acme.status(order),
            'identifiers': [{'type': 'dns', 'value': name} for name in order['names']],
            'authorizations': [f'{self.base()}/authz/{number}/{i}' for i in range(len(order['authz']))],
            'finalize': f'{self.base()}/finalize/{number}',
        }
        if order['certificate']:
            body['certificate'] = f'{self.base()}/cert/{number}'
        return body

    def post_order(self, protected, payload, parts):
        self.send_body(200, self.order_json(parts[0]))

    def challenge_json(self, number, index, kind):
        authz = self.acme.orders[number]['authz'][index]
        body = {'type': kind, 'url': f'{self.base()}/chall/{number}/{index}/{kind}', 'token': authz['tokens'][kind],
                'status': {'pending': 'pending', 'valid': 'valid', 'invalid': 'invalid'}[authz['status']]}
        if authz['error']:
            body['error'] = {'type': 'urn:ietf:params:acme:error:unauthorized', 'detail': authz['error'], 'status': 403}
        return body

    def post_authz(self, protected, payload, parts):
        number, index = parts[0], int(parts[1])
        authz = self.acme.orders[number]['authz'][index]
        kinds = ['dns-01'] if authz['wildcard'] else ['http-01', 'dns-01']
        self.send_body(200, {
            'identifier': {'type': 'dns', 'value': authz['name']}, 'status': authz['status'],
            'wildcard': authz['wildcard'], 'challenges': [self.challenge_json(number, index, k) for k in kinds],
        })

    def post_chall(self, protected, payload, parts):
        number, index, kind = parts[0], int(parts[1]), parts[2]
        authz = self.acme.orders[number]['authz'][index]
        reason = self.acme.check(authz['name'], kind, authz['tokens'][kind])
        authz['status'], authz['error'] = ('invalid', reason) if reason else ('valid', None)
        body = self.challenge_json(number, index, kind)
        body['status'] = 'processing'
        body.pop('error', None)  # the reason is revealed when the authorization is read
        self.send_body(200, body)

    def post_finalize(self, protected, payload, parts):
        number = parts[0]
        order = self.acme.orders[number]
        names = ['DNS:' + name for name in order['names']]
        with tempfile.TemporaryDirectory() as work:
            request, leaf = Path(work, 'request.pem'), Path(work, 'leaf.pem')
            der = Path(work, 'request.der')
            der.write_bytes(b64d(payload['csr']))
            openssl('req', '-inform', 'DER', '-in', str(der), '-out', str(request))
            serial = self.acme.sign_csr(request, leaf, names, self.acme.valid_days)
            order['certificate'] = leaf.read_text() + self.acme.ca_cert.read_text()
        order['status'] = 'valid'
        self.acme.issued.append({'names': order['names'], 'serial': serial})
        self.send_body(200, self.order_json(number))

    def post_cert(self, protected, payload, parts):
        self.send_body(200, self.acme.orders[parts[0]]['certificate'].encode(),
                       content_type='application/pem-certificate-chain')

    def log_message(self, *_):
        pass
