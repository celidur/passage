"""Process and HTTP helpers for the smoke tests. Standard library only."""
import http.client
import http.server
import json
import os
import socket
import ssl
import subprocess
import threading
import time
from pathlib import Path


def free_port() -> int:
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def request(port, path, method='GET', body=None, headers=None, context=None):
    """Sends one request and returns (status, lowercase headers, body)."""
    if context:
        connection = http.client.HTTPSConnection('127.0.0.1', port, timeout=5, context=context)
    else:
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
    headers = dict(headers or {})
    if body is not None:
        body = json.dumps(body)
        headers['Content-Type'] = 'application/json'
    try:
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        return response.status, {k.lower(): v for k, v in response.getheaders()}, response.read()
    finally:
        connection.close()


class EchoUpstream(http.server.BaseHTTPRequestHandler):
    """Replies with the received path and headers as JSON."""

    def do_GET(self):
        payload = json.dumps({'path': self.path, 'headers': dict(self.headers)}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


class FakeAuth(http.server.BaseHTTPRequestHandler):
    """Forward-auth endpoint: `/redirect` sends to /login; `Cookie: session=valid` passes; else 401."""

    hits = 0
    last_headers: dict = {}

    def do_GET(self):
        FakeAuth.hits += 1
        FakeAuth.last_headers = {k.lower(): v for k, v in self.headers.items()}
        if self.path == '/redirect':
            self.send_response(302)
            self.send_header('Location', '/login')
        elif self.headers.get('Cookie') == 'session=valid':
            self.send_response(200)
            self.send_header('Remote-User', 'verified-user')
        else:
            self.send_response(401)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def log_message(self, *_):
        pass


def serve(handler, tls_context=None) -> http.server.ThreadingHTTPServer:
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    if tls_context:
        server.socket = tls_context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def self_signed_certificate(directory: Path):
    cert, key = directory / 'cert.pem', directory / 'key.pem'
    subprocess.run(
        ['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(key),
         '-out', str(cert), '-days', '1', '-subj', '/CN=127.0.0.1',
         '-addext', 'subjectAltName=IP:127.0.0.1'],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return cert, key


class Passage:
    """A Passage process on ephemeral ports with its own data directory."""

    def __init__(self, binary: str, directory: Path, cert: Path, key: Path):
        self.binary = binary
        self.admin_port, self.proxy_port, self.tls_port = free_port(), free_port(), free_port()
        self.data_dir = directory / 'data'
        self.config = directory / 'passage.toml'
        self.config.write_text(
            f'proxy_listen = "127.0.0.1:{self.proxy_port}"\n'
            f'admin_listen = "127.0.0.1:{self.admin_port}"\n'
            f'data_dir = "{self.data_dir}"\n'
            f'[tls]\nlisten = "127.0.0.1:{self.tls_port}"\n'
            f'certificate = "{cert}"\nprivate_key = "{key}"\n'
        )
        self.log = open(directory / 'server.log', 'w+')
        self.process = None
        self.token = None

    def command(self, *args) -> int:
        return subprocess.run(
            [self.binary, '-c', str(self.config), *args],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        ).returncode

    def init(self):
        assert self.command('init') == 0
        self.token = (self.data_dir / 'admin.token').read_text().strip()

    def start(self):
        self.process = subprocess.Popen(
            [self.binary, '-c', str(self.config), 'serve'],
            stdout=self.log, stderr=self.log, env={**os.environ, 'RUST_LOG': 'info'},
        )
        for _ in range(100):
            if self.process.poll() is not None:
                raise AssertionError('Passage exited during startup')
            try:
                if request(self.admin_port, '/healthz')[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise AssertionError('Passage did not become ready')

    def stop(self):
        if self.process is None:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process = None

    def output(self) -> str:
        self.log.flush()
        self.log.seek(0)
        return self.log.read()

    def api(self, path, method='GET', body=None):
        return request(self.admin_port, '/api' + path, method, body,
                       {'Authorization': 'Bearer ' + self.token})

    def create_route(self, **fields) -> str:
        status, _, data = self.api('/routes', 'POST', fields)
        assert status == 201, (status, data)
        return json.loads(data)['id']

    def update_route(self, route_id, **fields):
        status, _, data = self.api('/routes/' + route_id, 'PUT', fields)
        assert status == 200, (status, data)

    def delete_route(self, route_id):
        assert self.api('/routes/' + route_id, 'DELETE')[0] == 204

    def get(self, host, path='/', headers=None, tls_context=None):
        port = self.tls_port if tls_context else self.proxy_port
        return request(port, path, headers={'Host': host, **(headers or {})}, context=tls_context)

    def close(self):
        self.stop()
        self.log.close()
