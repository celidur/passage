"""Process and HTTP helpers for the smoke tests. Standard library only."""
import base64
import hashlib
import http.client
import http.server
import json
import os
import socket
import ssl
import struct
import subprocess
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

WEBSOCKET_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'


def free_port() -> int:
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def wait_for(condition, timeout=10.0, message='condition not met'):
    """Polls `condition` until it returns a truthy value and returns it."""
    deadline = time.monotonic() + timeout
    while True:
        value = condition()
        if value:
            return value
        if time.monotonic() > deadline:
            raise AssertionError(message)
        time.sleep(0.1)


def request(port, path, method='GET', body=None, headers=None, context=None, timeout=5, chunked=False):
    """Sends one request and returns (status, lowercase headers, body)."""
    if context:
        connection = http.client.HTTPSConnection('127.0.0.1', port, timeout=timeout, context=context)
    else:
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
    headers = dict(headers or {})
    if isinstance(body, (dict, list)):
        body = json.dumps(body)
        headers['Content-Type'] = 'application/json'
    try:
        connection.request(method, path, body=body, headers=headers, encode_chunked=chunked)
        response = connection.getresponse()
        return response.status, {k.lower(): v for k, v in response.getheaders()}, response.read()
    finally:
        connection.close()


class SniConnection(http.client.HTTPSConnection):
    """Connects to 127.0.0.1 but announces `server_name` in the TLS handshake and verifies it."""

    def __init__(self, port, server_name, context, timeout=5):
        super().__init__('127.0.0.1', port, context=context, timeout=timeout)
        self.server_name = server_name

    def connect(self):
        http.client.HTTPConnection.connect(self)
        self.sock = self._context.wrap_socket(self.sock, server_hostname=self.server_name)


def read_frame(stream):
    """Reads one WebSocket frame and returns (opcode, payload), or None at end of stream."""
    head = stream.read(2)
    if len(head) < 2:
        return None
    opcode, masked, length = head[0] & 0x0F, head[1] & 0x80, head[1] & 0x7F
    if length == 126:
        length = struct.unpack('>H', stream.read(2))[0]
    elif length == 127:
        length = struct.unpack('>Q', stream.read(8))[0]
    mask = stream.read(4) if masked else b''
    data = stream.read(length)
    if masked:
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    return opcode, data


def frame(opcode, data, mask=b''):
    """Encodes a final WebSocket frame; clients must pass a 4-byte mask."""
    flag = 0x80 if mask else 0
    head = bytes([0x80 | opcode])
    if len(data) < 126:
        head += bytes([flag | len(data)])
    elif len(data) < 65536:
        head += bytes([flag | 126]) + struct.pack('>H', len(data))
    else:
        head += bytes([flag | 127]) + struct.pack('>Q', len(data))
    if mask:
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    return head + mask + data


class EchoUpstream(http.server.BaseHTTPRequestHandler):
    """Replies with the received path and headers as JSON.

    Special paths: `/health` (status follows `healthy`), `/slow?delay=N`, `/stream`
    (close-delimited body in two parts) and WebSocket upgrades (echoes `echo:<text>`).
    """

    healthy = True

    def reply(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urlsplit(self.path)
        if self.headers.get('Upgrade', '').lower() == 'websocket':
            return self.websocket()
        if url.path == '/health':
            return self.reply(200 if self.healthy else 503, {'healthy': self.healthy})
        if url.path == '/stream':
            return self.stream()
        if url.path == '/slow':
            time.sleep(float(parse_qs(url.query)['delay'][0]))
        self.reply(200, {'path': self.path, 'port': self.server.server_port, 'headers': dict(self.headers)})

    def do_POST(self):
        self.reply(200, {'received': self.read_body()})

    def read_body(self) -> int:
        if self.headers.get('Transfer-Encoding', '').lower() == 'chunked':
            total = 0
            while True:
                size = int(self.rfile.readline().split(b';')[0], 16)
                if size == 0:
                    self.rfile.readline()
                    return total
                total += len(self.rfile.read(size))
                self.rfile.readline()
        return len(self.rfile.read(int(self.headers.get('Content-Length', 0))))

    def stream(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        self.wfile.write(b'first\n')
        time.sleep(1.5)
        self.wfile.write(b'second\n')

    def websocket(self):
        key = self.headers['Sec-WebSocket-Key'] + WEBSOCKET_GUID
        accept = base64.b64encode(hashlib.sha1(key.encode()).digest()).decode()
        self.wfile.write(
            b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
            + f'Sec-WebSocket-Accept: {accept}\r\n\r\n'.encode()
        )
        while (message := read_frame(self.rfile)) is not None:
            opcode, data = message
            if opcode == 0x8:
                self.wfile.write(frame(0x8, b''))
                return
            self.wfile.write(frame(opcode, b'echo:' + data))

    def log_message(self, *_):
        pass


class WebSocket:
    """A minimal client: performs the handshake, then exchanges masked text frames."""

    def __init__(self, port, host, path='/ws'):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.socket.sendall(
            f'GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
            f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode()
        )
        self.stream = self.socket.makefile('rb')
        self.status = int(self.stream.readline().split()[1])
        self.headers = {}
        while (line := self.stream.readline()) not in (b'\r\n', b''):
            name, _, value = line.decode().partition(':')
            self.headers[name.strip().lower()] = value.strip()
        expected = base64.b64encode(hashlib.sha1((key + WEBSOCKET_GUID).encode()).digest()).decode()
        self.accepted = self.headers.get('sec-websocket-accept') == expected

    def exchange(self, text: str) -> str:
        self.socket.sendall(frame(0x1, text.encode(), os.urandom(4)))
        opcode, data = read_frame(self.stream)
        assert opcode == 0x1, opcode
        return data.decode()

    def close(self):
        try:
            self.socket.sendall(frame(0x8, b'', os.urandom(4)))
            read_frame(self.stream)
        except OSError:
            pass
        self.socket.close()


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


class Server(http.server.ThreadingHTTPServer):
    request_queue_size = 256
    allow_reuse_address = True

    def handle_error(self, request, client_address):
        pass  # clients that give up on a slow response are expected here


def serve(handler, tls_context=None, port=0) -> http.server.ThreadingHTTPServer:
    """Starts a server on its own copy of `handler`, so class-level state is per server."""
    server = Server(('127.0.0.1', port), type(handler.__name__, (handler,), {}))
    if tls_context:
        server.socket = tls_context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def stop(server):
    """Stops serving and closes the listening socket, so new connections are refused."""
    server.shutdown()
    server.server_close()


class ClosingServer:
    """Accepts connections and drops them without answering, like a crashing backend."""

    def __init__(self):
        self.socket = socket.create_server(('127.0.0.1', 0))
        self.server_port = self.socket.getsockname()[1]
        threading.Thread(target=self.run, daemon=True).start()

    def run(self):
        while True:
            try:
                connection, _ = self.socket.accept()
            except OSError:
                return
            connection.close()


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

    def __init__(self, binary: str, directory: Path, cert: Path, key: Path, acme=None):
        self.binary = binary
        self.admin_port, self.proxy_port, self.tls_port = free_port(), free_port(), free_port()
        self.data_dir = directory / 'data'
        self.config = directory / 'passage.toml'
        text = (
            f'[general]\ndata_dir = "{self.data_dir}"\n'
            f'[admin]\nlisten = "127.0.0.1:{self.admin_port}"\n'
            f'[http]\nlisten = "127.0.0.1:{self.proxy_port}"\n'
            f'[https]\nlisten = "127.0.0.1:{self.tls_port}"\n'
            f'certificate = "{cert}"\nprivate_key = "{key}"\n'
        )
        if acme:
            acme.proxy_port = self.proxy_port
            text += (
                f'[acme]\nemail = "ops@example.test"\nterms_agreed = true\n'
                f'directory_url = "{acme.directory_url}"\nca_certificate = "{acme.ca_cert}"\n'
                f'dns_hook = "{acme.hook}"\ndns_propagation_secs = 0\n'
            )
        self.config.write_text(text)
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

    def upstreams(self, route_id) -> dict:
        """Live state of a route (`active` requests and `backends`), from /api/upstreams."""
        routes = json.loads(self.api('/upstreams')[2])
        return next(r for r in routes if r['route_id'] == route_id)

    def certificates(self) -> list:
        return json.loads(self.api('/certificates')[2])

    def certificate_for(self, domain):
        """The managed certificate listing `domain` among its names, if any."""
        return next((c for c in self.certificates() if domain in c['domains']), None)

    def peer_certificate(self, server_name, cafile=None) -> dict:
        """The certificate the HTTPS listener presents for an SNI name, verified against `cafile`.

        Without `cafile` nothing is verified, and the certificate is returned as `{'pem': ...}`.
        """
        context = ssl.create_default_context(cafile=str(cafile)) if cafile else ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        if not cafile:
            context.check_hostname, context.verify_mode = False, ssl.CERT_NONE
        with socket.create_connection(('127.0.0.1', self.tls_port), timeout=5) as connection:
            with context.wrap_socket(connection, server_hostname=server_name) as tls:
                if cafile:
                    return tls.getpeercert()
                return {'pem': ssl.DER_cert_to_PEM_cert(tls.getpeercert(binary_form=True))}

    def https_get(self, host, cafile, path='/', timeout=5):
        """A GET over the HTTPS listener with SNI `host`, verified against `cafile`."""
        context = ssl.create_default_context(cafile=str(cafile))
        connection = SniConnection(self.tls_port, host, context, timeout)
        try:
            connection.request('GET', path, headers={'Host': host})
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def get(self, host, path='/', headers=None, tls_context=None, timeout=5):
        port = self.tls_port if tls_context else self.proxy_port
        return request(port, path, headers={'Host': host, **(headers or {})},
                       context=tls_context, timeout=timeout)

    def close(self):
        self.stop()
        self.log.close()
