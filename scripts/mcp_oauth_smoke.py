#!/usr/bin/env python3
"""Independent local OAuth + MCP + model fixture; no accounts or model quota."""
import argparse
import base64
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import threading
import time
from urllib.parse import parse_qs, urlencode, urlsplit


class Fixture(http.server.HTTPServer):
    def __init__(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.dispatch()
            def do_POST(self):
                self.dispatch()
            def do_DELETE(self):
                self.dispatch()
            def dispatch(self):
                try:
                    body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
                    self.server.requests.append((self.command, self.path, dict(self.headers), body))
                    status, headers, value = self.server.reply(self.command, self.path, self.headers, body)
                    payload = b'' if value is None else json.dumps(value).encode()
                    self.send_response(status)
                    for key, val in headers.items(): self.send_header(key, val)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append((type(error).__name__, str(error)))
                    self.send_error(500)
            def log_message(self, *_): pass
        super().__init__(('127.0.0.1', 0), Handler)
        self.origin = f'http://127.0.0.1:{self.server_port}'
        self.requests = []
        self.errors = []
        self.challenge = None
        self.redirect = None
        self.mode = 'normal'
        self.model_calls = 0
        self.worker = threading.Thread(target=self.serve_forever, daemon=True)
        self.worker.start()
    def reply(self, method, path, headers, body):
        origin = self.origin
        if method == 'GET':
            assert 'Authorization' not in headers, 'discovery/browser must not carry tokens'
            if path == '/mcp':
                return 401, {'WWW-Authenticate': f'Bearer realm="local, fixture", resource_metadata="{origin}/protected", scope="files:read"'}, {}
            if path == '/protected':
                return 200, {}, {'resource': origin + '/mcp', 'authorization_servers': [origin]}
            if path == '/.well-known/oauth-authorization-server':
                return 200, {}, {'issuer': origin if self.mode != 'wrong_issuer' else origin + '/wrong', 'authorization_endpoint': origin + '/authorize', 'token_endpoint': origin + '/token', 'code_challenge_methods_supported': ['S256'], 'response_types_supported': ['code'], 'token_endpoint_auth_methods_supported': ['client_secret_post'] if self.mode == 'confidential' else ['none'], 'authorization_response_iss_parameter_supported': True}
            if path.startswith('/authorize?'):
                q = parse_qs(urlsplit(path).query, strict_parsing=True)
                assert q['client_id'] == ['cli-fixture']
                assert q['resource'] == [origin + '/mcp']
                assert q['scope'] == ['files:read']
                assert q['code_challenge_method'] == ['S256']
                assert len(q['state'][0]) == len(q['code_challenge'][0]) == 43
                self.challenge = q['code_challenge'][0]
                self.redirect = q['redirect_uri'][0]
                callback = self.redirect + '?' + urlencode({'code': 'fixture-code', 'state': q['state'][0], 'iss': origin})
                return 302, {'Location': callback}, None
        if method == 'POST' and path == '/token':
            assert 'Authorization' not in headers
            q = parse_qs(body.decode(), strict_parsing=True)
            assert q['client_id'] == ['cli-fixture']
            assert q['resource'] == [origin + '/mcp']
            if self.mode == 'confidential': assert q['client_secret'] == ['fixture-client-secret']
            else: assert 'client_secret' not in q
            if q['grant_type'] == ['authorization_code']:
                assert q['code'] == ['fixture-code'] and q['redirect_uri'] == [self.redirect]
                actual = base64.urlsafe_b64encode(hashlib.sha256(q['code_verifier'][0].encode()).digest()).decode().rstrip('=')
                assert actual == self.challenge
                return 200, {}, {'access_token': 'fixture-access-one', 'refresh_token': 'fixture-refresh-one', 'token_type': 'Bearer', 'expires_in': 1, 'scope': 'files:read'}
            assert q['grant_type'] == ['refresh_token'] and q['refresh_token'] == ['fixture-refresh-one']
            if self.mode == 'refresh_error':
                return 400, {}, {'error': 'invalid_grant', 'error_description': 'DO_NOT_PRINT_FIXTURE_SECRET'}
            return 200, {}, {'access_token': 'fixture-access-two', 'refresh_token': 'fixture-refresh-two', 'token_type': 'Bearer', 'expires_in': 3600, 'scope': 'files:read'}
        if path == '/mcp':
            assert headers.get('Authorization') == 'Bearer fixture-access-two'
            if method == 'DELETE': return 200, {}, None
            message = json.loads(body)
            op = message['method']
            if op == 'initialize':
                result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'independent-oauth-peer', 'version': '1'}}
            elif op == 'notifications/initialized': return 202, {}, None
            elif op == 'ping': result = {}
            elif op == 'tools/list':
                result = {'tools': [{'name': 'read', 'inputSchema': {'type': 'object', 'properties': {}, 'additionalProperties': False}, 'outputSchema': {'type': 'object', 'properties': {'answer': {'type': 'integer'}}, 'required': ['answer']}}]}
            elif op == 'tools/call':
                assert message['params'] == {'name': 'read', 'arguments': {}}
                result = {'content': [{'type': 'text', 'text': '42'}], 'structuredContent': {'answer': 42}}
            else: raise AssertionError(op)
            return 200, {}, {'jsonrpc': '2.0', 'id': message['id'], 'result': result}
        if path == '/v1/chat/completions':
            assert headers.get('Authorization') == 'Bearer fixture-model-key'
            request = json.loads(body)
            if self.model_calls == 0:
                name = next(t['function']['name'] for t in request['tools'] if t['function']['name'].startswith('mcp_'))
                message = {'role': 'assistant', 'content': '', 'tool_calls': [{'id': 'oauth-tool', 'type': 'function', 'function': {'name': name, 'arguments': '{}'}}]}
                finish = 'tool_calls'
            else:
                tool = next(m for m in request['messages'] if m['role'] == 'tool')
                assert json.loads(json.loads(tool['content'])['content'])['structuredContent']['answer'] == 42
                message = {'role': 'assistant', 'content': 'OAUTH_AGENT_DONE'}
                finish = 'stop'
            self.model_calls += 1
            return 200, {}, {'choices': [{'finish_reason': finish, 'message': message}]}
        return 404, {}, {}
    def close(self):
        self.shutdown()
        self.worker.join()
        self.server_close()
        assert not self.errors, self.errors


def fetch(url, host=None):
    u = urlsplit(url)
    conn = http.client.HTTPConnection(u.hostname, u.port, timeout=5)
    conn.putrequest('GET', u.path + ('?' + u.query if u.query else ''), skip_host=host is not None)
    if host: conn.putheader('Host', host)
    conn.endheaders()
    reply = conn.getresponse()
    result = reply.status, dict(reply.getheaders()), reply.read()
    conn.close()
    return result


def run(binary):
    with tempfile.TemporaryDirectory(prefix='namzu-oauth-acceptance-') as directory:
        root = Path(directory)
        state = root / 'state'
        peer = Fixture()
        active = None
        checks = []
        env = {**os.environ, 'NAMZU_OAUTH_MODEL_FIXTURE_KEY': 'fixture-model-key', 'NAMZU_OAUTH_CLIENT_SECRET_FIXTURE': 'fixture-client-secret'}
        def command(*args, success=True):
            p = subprocess.run([str(binary), 'mcp', '--state-dir', str(state), *args], capture_output=True, env=env, timeout=20)
            assert (p.returncode == 0) == success, (p.stdout.decode(), p.stderr.decode())
            text = p.stdout.decode() + p.stderr.decode()
            assert all(s not in text for s in ('fixture-access-', 'fixture-refresh-', 'DO_NOT_PRINT_FIXTURE_SECRET', 'fixture-code', 'fixture-client-secret'))
            return json.loads(p.stdout)
        def login_start():
            nonlocal active
            active = subprocess.Popen([str(binary), 'mcp', '--state-dir', str(state), 'auth', 'login', 'fixture', '--approve', '--timeout', '15'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, text=True)
            # Read one byte at a time to avoid buffered readline + select races.
            until = time.monotonic() + 10
            lines = ''
            while time.monotonic() < until:
                if select.select([active.stderr], [], [], .1)[0]:
                    char = os.read(active.stderr.fileno(), 1).decode()
                    if not char: break
                    lines += char
                    if lines.endswith('\n') and lines.splitlines()[-1].startswith('http'):
                        return lines.splitlines()[-1]
            raise AssertionError('login URL not emitted')
        def finish_login():
            nonlocal active
            url = login_start()
            status, headers, _ = fetch(url)
            assert status == 302
            callback = headers['Location']
            bad = callback.replace('state=', 'state=WRONG')
            assert fetch(bad)[0] == 400
            assert fetch(callback, host='evil.invalid')[0] == 400
            assert fetch(callback + '&state=duplicate')[0] == 400
            assert fetch(callback)[0] == 200
            stdout, stderr = active.communicate(timeout=10)
            assert active.returncode == 0, (stdout, stderr)
            assert json.loads(stdout)['output']['logged_in']
            assert 'fixture-access-' not in stdout + stderr
            active = None
        try:
            spec = root / 'server.json'
            definition = {'transport': {'type': 'http', 'url': peer.origin + '/mcp'}, 'oauth': {'issuer': peer.origin, 'client_id': 'cli-fixture', 'scopes': ['files:read']}, 'policy': {'require_approval': True}}
            spec.write_text(json.dumps(definition))
            command('add', 'fixture', '--spec', str(spec))
            assert not command('auth', 'status', 'fixture')['output']['logged_in']
            command('check', 'fixture', '--approve', success=False)
            assert not peer.requests
            checks.append('no_implicit_login')
            command('auth', 'login', 'fixture', success=False)
            command('--deny', 'oauth/login', 'auth', 'login', 'fixture', '--approve', success=False)
            assert not peer.requests
            checks.append('explicit_policy_admission')
            finish_login()
            checks.append('real_cli_pkce_state_host_duplicate_callback')
            status = command('auth', 'status', 'fixture')['output']
            assert status['refresh_available'] and not status['uncertain']
            command('check', 'fixture', '--approve')
            assert command('auth', 'status', 'fixture')['output']['logged_in']
            command('check', 'fixture', '--approve')
            assert sum(path == '/token' for _, path, _, _ in peer.requests) == 2
            checks.append('refresh_rotation_and_authenticated_mcp')
            p = subprocess.run([str(binary), 'exec', 'test OAuth MCP', '--state-dir', str(state), '--cwd', str(root), '--model', 'fixture', '--base-url', peer.origin + '/v1', '--api-key-env', 'NAMZU_OAUTH_MODEL_FIXTURE_KEY', '--mcp-server', 'fixture', '--mcp-approve', '--timeout', '20', '--json'], env=env, capture_output=True, timeout=25)
            assert p.returncode == 0, (p.stdout.decode(), p.stderr.decode())
            assert json.loads(p.stdout)['output'] == 'OAUTH_AGENT_DONE'
            assert peer.model_calls == 2
            checks.append('normal_agent_oauth_mcp_tool_result')
            definition['oauth']['client_id'] = 'changed-client'
            spec.write_text(json.dumps(definition))
            command('update', 'fixture', '--spec', str(spec))
            assert not command('auth', 'status', 'fixture')['output']['logged_in']
            definition['oauth']['client_id'] = 'cli-fixture'
            spec.write_text(json.dumps(definition))
            command('update', 'fixture', '--spec', str(spec))
            assert command('auth', 'status', 'fixture')['output']['logged_in']
            checks.append('credential_client_resource_binding')
            command('auth', 'logout', 'fixture')
            assert not command('auth', 'status', 'fixture')['output']['logged_in']
            checks.append('redacted_status_local_logout')
            peer.mode = 'confidential'
            definition['oauth']['client_secret_env'] = 'NAMZU_OAUTH_CLIENT_SECRET_FIXTURE'
            spec.write_text(json.dumps(definition))
            command('update', 'fixture', '--spec', str(spec))
            finish_login()
            command('check', 'fixture', '--approve')
            for file in (state / 'mcp/oauth').glob('*.json'):
                assert 'fixture-client-secret' not in file.read_text()
            command('auth', 'logout', 'fixture')
            del definition['oauth']['client_secret_env']
            spec.write_text(json.dumps(definition))
            command('update', 'fixture', '--spec', str(spec))
            peer.mode = 'normal'
            checks.append('confidential_client_secret_post_env_only')
            login_start()
            active.send_signal(signal.SIGINT)
            stdout, stderr = active.communicate(timeout=5)
            assert active.returncode != 0 and 'cancelled' in stdout.lower()
            active = None
            assert not command('auth', 'status', 'fixture')['output']['logged_in']
            checks.append('signal_cancels_loopback_listener')
            peer.mode = 'refresh_error'
            finish_login()
            command('check', 'fixture', '--approve', success=False)
            before = len(peer.requests)
            command('check', 'fixture', '--approve', success=False)
            assert len(peer.requests) == before
            assert command('auth', 'status', 'fixture')['output']['uncertain']
            command('auth', 'logout', 'fixture')
            checks.append('uncertain_refresh_no_replay_or_secret_logging')
            peer.mode = 'wrong_issuer'
            command('auth', 'login', 'fixture', '--approve', '--timeout', '10', success=False)
            assert not command('auth', 'status', 'fixture')['output']['logged_in']
            checks.append('issuer_mismatch_before_browser')
            assert all('Authorization' not in headers for method, path, headers, _ in peer.requests if method == 'GET' or path == '/token')
            return {'checks': checks, 'live_model_calls': 0, 'fixture_model_requests': peer.model_calls}
        finally:
            if active is not None:
                active.kill()
                active.communicate(timeout=5)
            peer.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=Path('target/release/namzu-rs'))
    args = parser.parse_args()
    print(json.dumps(run(args.binary.resolve())))
