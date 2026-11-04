#!/usr/bin/env python3
"""Exercise real TLS, proxy handling, HTTP, JSON and CLI through a local mock.

Usage: python3 tests/embedded_smoke.py [runner arguments ...] /path/to/ysunethelper
For an emulator, pass its sysroot/CPU arguments before the executable.
No campus requests or account credentials are used.
"""
import http.server
import json
import os
from pathlib import Path
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading


class Portal(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.startswith('/eportal/redirect.jsp'):
            self.send_response(302)
            self.send_header('Location', '/portal-main?sessionId=embedded-test')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        if self.path.startswith('/portal-main'):
            self.respond(b'portal', 'text/html')
            return
        assert self.path.startswith('/eportal/adaptor/getOnlineUserInfo?'), self.path
        assert 'sessionId=embedded-test' in self.path
        self.envelope({'portalOnlineUserInfo': {
            'result': 'success', 'userName': 'mock-student',
            'userIp': '10.0.0.2', 'realServiceName': '校园网',
        }})

    def do_POST(self):
        assert self.path == '/eportal/operator/getAccountInfo', self.path
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert body == {'sessionId': 'embedded-test'}
        self.envelope({'name': 'Mock User', 'accountInfo': [
            {'title': '剩余流量', 'content': '28.1GB'},
        ]})

    def envelope(self, data):
        self.respond(json.dumps({'code': 200, 'data': data}).encode(), 'application/json')

    def respond(self, body, content_type):
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Content-Type', content_type)
        self.end_headers()
        self.wfile.write(body)


class LocalProxy(socketserver.BaseRequestHandler):
    context = None

    def handle(self):
        reader = self.request.makefile('rb')
        request = reader.readline().decode('ascii').split()
        assert request[:2] == ['CONNECT', 'auth1.ysu.edu.cn:443'], request
        while reader.readline() not in (b'\r\n', b'\n', b''):
            pass
        self.request.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
        with self.context.wrap_socket(self.request, server_side=True) as tls:
            Portal(tls, self.client_address, self.server)
        reader.close()


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    with tempfile.TemporaryDirectory(prefix='ysunethelper-tls-') as tmp:
        cert, key = Path(tmp) / 'cert.pem', Path(tmp) / 'key.pem'
        subprocess.run([
            'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
            '-keyout', str(key), '-out', str(cert), '-days', '1',
            '-subj', '/CN=auth1.ysu.edu.cn',
            '-addext', 'subjectAltName=DNS:auth1.ysu.edu.cn',
            '-addext', 'basicConstraints=critical,CA:TRUE',
        ], check=True, capture_output=True)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        LocalProxy.context = context
        with socketserver.ThreadingTCPServer(('127.0.0.1', 0), LocalProxy) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            proxy = f'http://127.0.0.1:{server.server_address[1]}'
            env = dict(os.environ)
            for name in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY'):
                env[name] = env[name.lower()] = proxy
            env['NO_PROXY'] = env['no_proxy'] = ''
            env['SSL_CERT_FILE'] = str(cert)
            config = Path(tmp) / 'config.json'
            config.write_text('{"http_timeout":"5s"}')
            try:
                result = subprocess.run(
                    sys.argv[1:] + ['--config', str(config), 'status', '--json'],
                    env=env, capture_output=True, text=True, timeout=30,
                )
                assert result.returncode == 0, result.stderr
                data = json.loads(result.stdout)
                assert data['online'] and data['name'] == 'Mock User', data
                assert data['service'] == '校园网' and data['remaining_traffic'] == '28.1GB', data
                assert data['username'] == 'mock-student', data
                print('TLS proxy + portal session + status JSON smoke test passed')
            finally:
                server.shutdown()
                thread.join()


if __name__ == '__main__':
    main()
