#!/usr/bin/env python3
"""Exercise SIGINT during blocked login, TLS, HTTP and terminal input.

python3 tests/interruptions.py --binary target/debug/ysunethelper
Uses only local HTTPS fixtures and PTYs, never real credentials or endpoints.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import signal
import socket
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from urllib.parse import urlsplit

from compatibility import Portal as NormalPortal

BINARY = None


class DelayedPortal(NormalPortal):
    def stall(self, body=False):
        if body:
            self.send_response(200)
            self.send_header('Content-Length', '1000000')
            self.end_headers()
            self.wfile.write(b'{')
            self.wfile.flush()
        self.server.blocked.set()
        self.server.release.wait(10)
        self.close_connection = True

    def do_GET(self):
        path = urlsplit(self.path).path
        mode = self.server.mode
        if path == '/authserver/index.do' and mode in ('headers', 'body'):
            return self.stall(body=mode == 'body')
        if path == '/personalInfo/personCenter/index.html' and mode == 'landing':
            return self.stall()
        if path == '/generate_204' and mode == 'probe':
            return self.stall()
        return super().do_GET()

    def do_POST(self):
        if urlsplit(self.path).path == '/authserver/login' and self.server.mode == 'post':
            self.rfile.read(int(self.headers['Content-Length']))
            self.server.state['events'].append(['login-submitted'])
            return self.stall()
        return super().do_POST()


class Proxy(socketserver.BaseRequestHandler):
    def wait_for_disconnect(self):
        self.server.blocked.set()
        self.request.settimeout(0.1)
        while not self.server.release.is_set():
            try:
                if not self.request.recv(8192):
                    self.server.disconnected.set()
                    break
            except socket.timeout:
                continue
            except ConnectionResetError:
                self.server.disconnected.set()
                break

    def handle(self):
        reader = self.request.makefile('rb')
        try:
            line = reader.readline().decode().split()
            assert line[0] == 'CONNECT' and line[1] in ('auth1.ysu.edu.cn:443', 'cer.ysu.edu.cn:443'), line
            while reader.readline() not in (b'\r\n', b'\n', b''):
                pass
            if self.server.mode == 'connect':
                return self.wait_for_disconnect()
            self.request.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
            if self.server.mode == 'handshake':
                return self.wait_for_disconnect()
            with self.server.context.wrap_socket(self.request, server_side=True) as tls:
                DelayedPortal(tls, self.client_address, self.server)
        except (BrokenPipeError, ConnectionResetError, ssl.SSLEOFError):
            pass
        finally:
            reader.close()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


@contextlib.contextmanager
def fixture(mode, online=False):
    with tempfile.TemporaryDirectory(prefix='ysunethelper-interrupt-') as directory:
        tmp = Path(directory)
        cert, key = tmp / 'cert.pem', tmp / 'key.pem'
        subprocess.run([
            'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
            '-keyout', str(key), '-out', str(cert), '-days', '1',
            '-subj', '/CN=auth1.ysu.edu.cn', '-addext',
            'subjectAltName=DNS:auth1.ysu.edu.cn,DNS:cer.ysu.edu.cn',
            '-addext', 'basicConstraints=critical,CA:TRUE',
        ], check=True, capture_output=True)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        with Server(('127.0.0.1', 0), Proxy) as server:
            server.mode = mode
            server.context = context
            server.blocked, server.release, server.disconnected = (threading.Event() for _ in range(3))
            server.state = {'online': online, 'username': 'fixture-user', 'service': '校园网', 'admitted_cas': False, 'events': []}
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            proxy = f'http://127.0.0.1:{server.server_address[1]}'
            env = dict(os.environ, SSL_CERT_FILE=str(cert), HOME=str(tmp), USERPROFILE=str(tmp), NO_PROXY='', no_proxy='')
            for name in ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY']:
                env[name] = env[name.lower()] = proxy
            credential = tmp / 'cas.json'
            config = tmp / 'config.json'
            config.write_text(json.dumps({
                'username': 'fixture-user', 'password': 'fixture-password',
                'credential_path': str(credential), 'http_timeout': '60s',
                'daemon': {'probe_urls': ['https://auth1.ysu.edu.cn/generate_204'], 'probe_timeout': '60s'},
            }))
            try:
                yield server, tmp, config, credential, env
            finally:
                server.release.set()
                server.shutdown()
                thread.join()


class Interruptions(unittest.TestCase):
    def interrupt(self, command, mode, online=False):
        with fixture(mode, online=online) as (server, tmp, config, credential, env):
            child = subprocess.Popen([BINARY, '--config', str(config)] + command,
                                     env=env, cwd=tmp, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                self.assertTrue(server.blocked.wait(8), 'client did not reach the blocked request')
                start = time.monotonic()
                child.send_signal(signal.SIGINT)
                stdout, stderr = child.communicate(timeout=1.5)
                latency = time.monotonic() - start
                self.assertEqual(child.returncode, 130, stderr.decode())
                self.assertNotIn(b'login success', stdout.lower())
                self.assertFalse(credential.exists(), 'cancelled login persisted credentials')
                if mode in ('connect', 'handshake'):
                    self.assertTrue(server.disconnected.wait(1), 'cancelled request left the connection open')
                if mode == 'post':
                    self.assertEqual(server.state['events'], [['login-submitted']])
                elif mode == 'landing':
                    self.assertEqual(server.state['events'], [['cas-login', 'fixture-user']])
                else:
                    self.assertEqual(server.state['events'], [])
                self.assertNotIn(b'status: ONLINE', stdout)
                print(f'  {mode}: exit 130 in {latency * 1000:.0f} ms', flush=True)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.communicate()

    def test_proxy_connect(self):
        self.interrupt(['login', '--json'], 'connect')

    def test_tls_handshake(self):
        self.interrupt(['login', '--json'], 'handshake')

    def test_response_headers(self):
        self.interrupt(['login', '--json'], 'headers')

    def test_response_body(self):
        self.interrupt(['login', '--json'], 'body')

    def test_credential_submission(self):
        self.interrupt(['login', '--json'], 'post')

    def test_best_effort_cas_landing_does_not_swallow_cancellation(self):
        self.interrupt(['login', '--json'], 'landing')

    def test_status_probe(self):
        self.interrupt(['status', '--test', '--json'], 'probe', online=True)

    def test_daemon_probe(self):
        self.interrupt(['daemon'], 'probe', online=True)

    @unittest.skipUnless(os.name == 'posix', 'PTY terminal tests require Unix')
    def test_terminal_input_restores_echo(self):
        import fcntl
        import pty
        import termios
        for stage in ('username', 'password', 'menu'):
            with self.subTest(stage=stage), fixture('normal', online=stage == 'menu') as (_, tmp, config, _, env):
                data = json.loads(config.read_text())
                data.update(username='', password='')
                config.write_text(json.dumps(data))
                master, slave = pty.openpty()
                original = termios.tcgetattr(slave)

                def controlling_terminal():
                    os.setsid()
                    fcntl.ioctl(0, termios.TIOCSCTTY, 0)

                output = bytearray()
                changed = threading.Condition()

                def collect(pipe):
                    while chunk := pipe.read(1):
                        with changed:
                            output.extend(chunk)
                            changed.notify_all()

                def wait_for(text):
                    deadline = time.monotonic() + 8
                    with changed:
                        while text not in output:
                            remaining = deadline - time.monotonic()
                            if remaining <= 0:
                                self.fail(f'{stage}: missing prompt {text!r}, output={bytes(output)!r}')
                            changed.wait(remaining)

                child = subprocess.Popen([BINARY, '--config', str(config), 'tui' if stage == 'menu' else 'login'],
                                         stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         env=env, cwd=tmp, preexec_fn=controlling_terminal)
                readers = [threading.Thread(target=collect, args=(pipe,), daemon=True) for pipe in (child.stdout, child.stderr)]
                for reader in readers:
                    reader.start()
                try:
                    if stage == 'menu':
                        wait_for('q 退出: '.encode())
                    else:
                        wait_for('用户名: '.encode())
                        if stage == 'password':
                            os.write(master, b'student\n')
                            wait_for('密码: '.encode())
                            self.assertFalse(termios.tcgetattr(slave)[3] & termios.ECHO)
                            os.write(master, b'unsubmitted-secret')
                    start = time.monotonic()
                    os.write(master, b'\x03')  # Actual terminal Ctrl+C, not just kill(SIGINT).
                    self.assertEqual(child.wait(timeout=1.5), 130)
                    self.assertEqual(termios.tcgetattr(slave), original, 'terminal settings were not restored')
                    self.assertNotIn(b'unsubmitted-secret', output)
                    print(f'  {stage}: terminal Ctrl+C in {(time.monotonic() - start) * 1000:.0f} ms', flush=True)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait()
                    for reader in readers:
                        reader.join(timeout=1)
                    child.stdout.close()
                    child.stderr.close()
                    os.close(master)
                    os.close(slave)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    args, test_args = parser.parse_known_args()
    BINARY = str(Path(args.binary).resolve(strict=True))
    unittest.main(argv=[sys.argv[0]] + test_args, verbosity=2)
