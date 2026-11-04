#!/usr/bin/env python3
"""Compare Go and Rust CLI behavior using a local HTTPS CAS/ePortal mock.

python3 tests/compatibility.py --go /path/to/go-binary --rust /path/to/rust-binary
--rust-runner accepts an optional emulator command (quoted as a single argument).
Without --go, check the Rust executable against the same expected contracts.
The proxy never forwards requests to real services. All credentials are fixtures.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import shlex
import socketserver
import ssl
import subprocess
import tempfile
import threading
from urllib.parse import parse_qs, urlencode, urlsplit

UUID = 'abcdef1234567890abcdef1234567890'
SERVICES = {'campus': '校园网', 'telecom': '中国电信', 'mobile': '中国移动', 'unicom': '中国联通'}


class Portal(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    @property
    def state(self):
        return self.server.state

    def reply(self, data=None, *, text=None, location=None, cookie=None, status=200):
        body = (text if text is not None else json.dumps({'code': 200, 'message': 'ok', 'data': data})).encode()
        self.send_response(302 if location else status)
        if location:
            self.send_header('Location', location)
        if cookie:
            self.send_header('Set-Cookie', cookie)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == '/authserver/index.do':
            if 'TGC=fixture' in self.headers.get('Cookie', ''):
                return self.reply(text='authenticated')
            return self.reply(location='/authserver/login')
        if path == '/authserver/login':
            if 'TGC=fixture' in self.headers.get('Cookie', ''):
                service = parse_qs(urlsplit(self.path).query)['service'][0]
                return self.reply(location=service + '?ticket=ST-fixture')
            return self.reply(text="<form><input name='cllt' value='userNameLogin'>"
                             "<input type='hidden' name='execution' value='e1s1'>"
                             "<input type='hidden' id='pwdEncryptSalt' value='1234567890123456'></form>")
        if path == '/eportal/redirect.jsp':
            return self.reply(location='/portal-main?sessionId=fixture-session')
        if path in ('/portal-main', '/personalInfo/personCenter/index.html'):
            return self.reply(text='ok')
        if path == '/cas-sso/login':
            assert parse_qs(urlsplit(self.path).query)['flowSessionId'] == ['fixture-session']
            return self.reply(text='ok', cookie='SESSION=portal-fixture; Path=/')
        if path == '/cas-sso/clientredirect':
            return self.reply(location='https://cer.ysu.edu.cn/authserver/login?' + urlencode({'service': 'https://auth1.ysu.edu.cn/callback'}))
        if path == '/callback':
            assert 'SESSION=portal-fixture' in self.headers.get('Cookie', '')
            assert parse_qs(urlsplit(self.path).query)['ticket'] == ['ST-fixture']
            self.state['admitted_cas'] = True
            return self.reply(text='ok')
        if path == '/eportal/adaptor/getOnlineUserInfo':
            assert parse_qs(urlsplit(self.path).query)['sessionId'] == ['fixture-session']
            info = {'result': 'fail'}
            if self.state['online']:
                info = {'result': 'success', 'userName': self.state['username'],
                        'userIp': '10.0.0.2', 'realServiceName': self.state['service']}
            return self.reply({'portalOnlineUserInfo': info})
        if path == '/generate_204':
            return self.reply(text='', status=204)
        raise AssertionError(self.path)

    def do_POST(self):
        body = self.rfile.read(int(self.headers['Content-Length']))
        path = urlsplit(self.path).path
        if path == '/authserver/login':
            form = parse_qs(body.decode(), keep_blank_values=True)
            assert form['execution'] == ['e1s1'] and len(form['password'][0]) > 80
            self.state['username'] = form['username'][0]
            self.state['events'].append(['cas-login', form['username'][0]])
            return self.reply(location='https://cer.ysu.edu.cn/personalInfo/personCenter/index.html?ticket=ST-login', cookie='TGC=fixture; Path=/authserver; Secure')
        value = json.loads(body)
        assert value['sessionId'] == 'fixture-session'
        if path.endswith('/getAccountInfo'):
            return self.reply({'name': 'Mock User', 'accountInfo': [{'title': '剩余流量', 'content': '28.1GB'}]})
        if path.endswith('/userOnline'):
            return self.reply({'online': self.state['online']})
        if path.endswith('/serviceSelection'):
            return self.reply([{'key': service, 'value': service} for service in SERVICES.values()])
        if path.endswith('/serviceLogin'):
            assert self.state['admitted_cas']
            self.state['online'] = True
            self.state['service'] = value['service']
            self.state['events'].append(['login', value['service']])
            return self.reply({'authResult': 'success'})
        if path.endswith('/setService'):
            assert value['rememberService'] is False and value['browserFgr'] is None
            self.state['service'] = value['serviceName']
            self.state['events'].append(['switch', value['serviceName']])
            return self.reply({'switchResult': 'success'})
        if path.endswith('/offline'):
            self.state['online'] = False
            self.state['events'].append(['logout'])
            return self.reply()
        if path.endswith('/findDevice'):
            return self.reply({'onlineDevices': [{'onlineUserUuid': UUID, 'deviceName': '', 'userIpv4': '10.0.0.2'}]})
        if path.endswith('/batch'):
            assert value['onlineUserUuids'] == [UUID]
            self.state['events'].append(['kick', UUID])
            return self.reply()
        raise AssertionError(self.path)


class Proxy(socketserver.BaseRequestHandler):
    def handle(self):
        reader = self.request.makefile('rb')
        line = reader.readline().decode().split()
        assert line[0] == 'CONNECT' and line[1] in ('auth1.ysu.edu.cn:443', 'cer.ysu.edu.cn:443'), line
        while reader.readline() not in (b'\r\n', b'\n', b''):
            pass
        self.request.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
        with self.server.context.wrap_socket(self.request, server_side=True) as tls:
            Portal(tls, self.client_address, self.server)
        reader.close()


def cases():
    for args in [[], ['-v'], ['-verbose'], ['--verbose'], ['--json'], ['--json', '-v'], ['-json=false'], ['-test'], ['--json', '-test'], ['--json=true', '--json=false'], ['--json=1', '-test=0']]:
        yield (['status'] + args, True, 0, None)
    yield (['-v', 'status', '-v=false', '--json'], True, 0, None)
    for service in SERVICES:
        for flags in [('-u', '-p', '-s'), ('--u', '--p', '--s'), ('-username', '-password', '-service'), ('--username', '--password', '--service')]:
            yield (['login', flags[0], 'student', flags[1], '-json', flags[2], service, '--json'], False, 0, [['cas-login', 'student'], ['login', SERVICES[service]]])
    yield (['login', '-u=student', '-p=-password', '-s=unicom', '-json=TRUE'], False, 0, [['cas-login', 'student'], ['login', '中国联通']])
    yield (['login', '-u', 'old', '--username', 'student', '-s=mobile', '-s=telecom', '--json'], False, 0, [['cas-login', 'student'], ['login', '中国电信']])
    yield (['login', '-u=', '-p=', '-s=', '--json'], False, 0, [['cas-login', 'configured-user'], ['login', '校园网']])
    yield (['login', '-s=unicom', '--json'], True, 0, [['cas-login', 'configured-user']])
    for name in ['unicom', '3', '联通', '中国联通']:
        yield (['switch', '--json', name], True, 0, [['switch', '中国联通']])
    yield (['switch', '--json', 'campus'], True, 0, [])
    for command in ['devices', 'logout']:
        for flag in [[], ['--json'], ['-json=false']]:
            yield ([command] + flag, True, 0, None)
    yield (['logout', '--json'], False, 0, [])
    for args in [['--json', '1'], ['1'], ['--json', '1', UUID.upper()]]:
        yield (['kick'] + args, True, 0, [['kick', UUID]])
    yield (['kick', '1', '--json'], True, 1, [])
    yield (['kick', '--json', '1', '99'], True, 1, [])
    for command in [['devices', '--json'], ['switch', '--json', 'unicom'], ['kick', '1']]:
        yield (command, False, 1, [])
    for command in [['switch'], ['switch', ' '], ['kick'], ['status', '--json=invalid']]:
        yield (command, True, 2, [])
    yield (['tui'], True, 1, [])  # Both clients require a terminal, never silently mutate from a pipe.
    yield (['daemon'], True, 78, [])  # Empty config supplied below; no HTTP is allowed.
    yield (['version'], True, 0, [])
    yield (['--version'], True, 0, [])
    yield (['-version=false', 'status', '--json'], True, 0, None)
    yield (['--', 'status', '--json'], True, 0, None)
    yield (['@missing-config', 'status', '--json'], True, 0, None)


def normalized_output(text):
    try:
        value, end = json.JSONDecoder().raw_decode(text)
        return value, text[end:].strip()
    except ValueError:
        return text.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go')
    parser.add_argument('--rust', required=True)
    parser.add_argument('--rust-runner', default='')
    args = parser.parse_args()
    binaries = [('Rust', shlex.split(args.rust_runner) + [str(Path(args.rust).resolve())])]
    if args.go:
        binaries.insert(0, ('Go', [str(Path(args.go).resolve())]))
    with tempfile.TemporaryDirectory(prefix='ysunethelper-compat-') as directory:
        tmp = Path(directory)
        cert, key = tmp / 'cert.pem', tmp / 'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(key), '-out', str(cert), '-days', '1', '-subj', '/CN=auth1.ysu.edu.cn', '-addext', 'subjectAltName=DNS:auth1.ysu.edu.cn,DNS:cer.ysu.edu.cn', '-addext', 'basicConstraints=critical,CA:TRUE'], check=True, capture_output=True)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        with socketserver.ThreadingTCPServer(('127.0.0.1', 0), Proxy) as server:
            server.context = context
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            proxy = f'http://127.0.0.1:{server.server_address[1]}'
            env = dict(os.environ, SSL_CERT_FILE=str(cert), HOME=str(tmp), USERPROFILE=str(tmp), NO_PROXY='', no_proxy='')
            for name in ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY']:
                env[name] = env[name.lower()] = proxy
            count = 0
            try:
                for command, online, code, expected_events in cases():
                    observations = []
                    for label, binary in binaries:
                        server.state = {'online': online, 'username': 'configured-user', 'service': '校园网', 'admitted_cas': False, 'events': []}
                        credential = tmp / 'cas.json'
                        credential.unlink(missing_ok=True)
                        config = tmp / 'config.json'
                        config.write_text(json.dumps({} if command == ['daemon'] else {
                            'username': 'configured-user', 'password': 'fixture-password', 'service': 'campus',
                            'credential_path': str(credential), 'http_timeout': '3s',
                            'daemon': {'probe_urls': ['https://auth1.ysu.edu.cn/generate_204']},
                        }))
                        before = config.read_bytes()
                        actual_command = command[1:] if command[0] == '@missing-config' else command
                        actual_config = tmp / 'absent.json' if command[0] == '@missing-config' else config
                        result = subprocess.run(binary + ['-config', str(actual_config)] + actual_command, env=env, cwd=tmp, capture_output=True, text=True, timeout=20)
                        assert result.returncode == code, (label, command, result.returncode, result.stderr)
                        assert config.read_bytes() == before, (label, 'configuration was changed')
                        events = server.state['events']
                        if expected_events is not None:
                            assert events == expected_events, (label, command, events, expected_events)
                        if code == 0:
                            if 'version' in command or '--version' in command:
                                assert result.stdout.startswith('ysunethelper ')
                                value = 'version'
                            else:
                                value = normalized_output(result.stdout)
                            if isinstance(value, tuple):
                                payload = value[0]
                                operation = next((word for word in actual_command if word in ('status', 'login', 'logout', 'switch', 'devices', 'kick')), '')
                                device = {'uuid': UUID, 'name': '', 'ip': '10.0.0.2', 'current': True}
                                if operation == 'status':
                                    assert payload['online'] is True and payload['username'] == 'configured-user'
                                    assert payload['service'] == '校园网' and payload['remaining_traffic'] == '28.1GB'
                                    assert payload['name'] == 'Mock User' and 'user_mac' not in payload
                                elif operation == 'login':
                                    assert payload == {'ok': True, 'username': server.state['username'], 'service': server.state['service'], 'user_ip': '10.0.0.2'}
                                elif operation == 'logout':
                                    assert payload == {'ok': True}
                                elif operation == 'switch':
                                    assert len(result.stdout.splitlines()) == 1, 'switch JSON must stay on one line'
                                    assert payload == {'ok': True, 'online': True, 'username': 'configured-user', 'service': server.state['service'], 'user_ip': '10.0.0.2'}
                                elif operation == 'devices':
                                    assert payload['online'] == [device] and 'raw' in payload
                                elif operation == 'kick':
                                    assert payload == {'ok': True, 'kicked': [device]}
                            observations.append((value, events))
                    if len(observations) == 2:
                        assert observations[0] == observations[1], (command, observations)
                    count += 1
                print(f'{count} CLI compatibility scenarios passed ({" / ".join(label for label, _ in binaries)}); no real campus requests')
            finally:
                server.shutdown()
                thread.join()


if __name__ == '__main__':
    main()
