#!/usr/bin/env python3
"""6.3 native/API release CLI, Pal and PTY fixtures; no live models or credentials."""
import hashlib
import hmac
import http.server
import json
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import urllib.parse
from pals_smoke import environment
from tui_smoke import Terminal

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


def env():
    value = environment()
    value.pop('NAMZU_NATIVE_MISSING_KEY', None)
    value.update(AWS_ACCESS_KEY_ID='FIXTUREACCESS', AWS_SECRET_ACCESS_KEY='fixture-secret',
        AWS_SESSION_TOKEN='fixture-token')
    return value


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    size = int(self.headers['Content-Length'])
                    assert 0 < size <= 8 * 1024 * 1024
                    raw = self.rfile.read(size)
                    body = json.loads(raw)
                    result = callback(self.path, self.headers, raw, body, len(self.server.requests))
                    self.server.requests.append(body)
                    payload = json.dumps(result).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append((type(error).__name__, str(error)))
                    self.send_error(500)
            def log_message(self, *_): pass
        super().__init__(('127.0.0.1', 0), Handler)
        self.requests, self.errors, self.thread = [], [], None
    def start(self):
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()
    def close(self):
        if self.thread:
            self.shutdown()
            self.thread.join()
        self.server_close()
        assert not self.errors, self.errors


def opts(root, server, provider, model='fixture'):
    return ['--state-dir', str(root / 'state'), '--provider', provider, '--model', model,
        '--base-url', f'http://127.0.0.1:{server.server_port}', '--api-key-env',
        'NAMZU_NATIVE_MISSING_KEY' if provider in ('ollama', 'lmstudio') else 'NAMZU_TUI_FIXTURE_KEY',
        '--max-output-tokens', '128', '--timeout', '20']


def command(kind, options, *args, ok=True, custom_env=None):
    result = subprocess.run([str(BINARY), kind, *options, *args], capture_output=True,
        env=custom_env or env(), timeout=30)
    assert (result.returncode == 0) == ok, (result.stdout.decode(), result.stderr.decode())
    return json.loads(result.stdout)


def answer(provider, call=False):
    args = {'path': 'answer', 'content': '42\n'}
    if provider in ('deepseek', 'lmstudio'):
        message = {'role': 'assistant', 'content': None if call else 'NATIVE_OK'}
        if call:
            message.update(reasoning_content='PRIVATE_REASONING', tool_calls=[{'id': 'call',
                'type': 'function', 'function': {'name': 'write_file', 'arguments': json.dumps(args)}}])
        return {'choices': [{'finish_reason': 'tool_calls' if call else 'stop', 'message': message}],
            'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}
    if provider == 'anthropic':
        content = [{'type': 'thinking', 'thinking': 'PRIVATE_REASONING', 'signature': 'fixture'},
            {'type': 'tool_use', 'id': 'call', 'name': 'write_file', 'input': args}] if call else [
            {'type': 'text', 'text': 'NATIVE_OK'}]
        return {'type': 'message', 'role': 'assistant', 'stop_reason': 'tool_use' if call else 'end_turn',
            'content': content, 'usage': {'input_tokens': 3, 'output_tokens': 2}}
    if provider == 'google':
        parts = [{'functionCall': {'name': 'write_file', 'args': args}, 'thoughtSignature': 'PRIVATE_SIGNATURE'}] if call else [{'text': 'NATIVE_OK'}]
        return {'candidates': [{'finishReason': 'STOP', 'content': {'role': 'model', 'parts': parts}}],
            'usageMetadata': {'promptTokenCount': 3, 'candidatesTokenCount': 2, 'totalTokenCount': 5}}
    if provider == 'bedrock':
        content = [{'reasoningContent': {'reasoningText': {'text': 'PRIVATE_REASONING', 'signature': 'fixture'}}},
            {'toolUse': {'toolUseId': 'call', 'name': 'write_file', 'input': args}}] if call else [{'text': 'NATIVE_OK'}]
        return {'stopReason': 'tool_use' if call else 'end_turn', 'output': {'message': {
            'role': 'assistant', 'content': content}}, 'usage': {'inputTokens': 3, 'outputTokens': 2, 'totalTokens': 5}}
    assert provider == 'ollama'
    return {'done': True, 'done_reason': 'stop', 'message': {'role': 'assistant', 'content': '' if call else 'NATIVE_OK',
        'tool_calls': [{'function': {'name': 'write_file', 'arguments': args}}] if call else []},
        'prompt_eval_count': 3, 'eval_count': 2}


def expected_path(provider):
    return {'anthropic': '/messages', 'google': '/models/fixture:generateContent',
        'bedrock': '/model/fixture/converse', 'ollama': '/api/chat',
        'deepseek': '/chat/completions', 'lmstudio': '/chat/completions'}[provider]


def verify_request(provider, path, headers, body, index):
    assert path == expected_path(provider)
    key = 'fixture-only-not-a-real-key'
    if provider == 'anthropic':
        assert headers['x-api-key'] == key and headers['anthropic-version'] == '2023-06-01'
        assert 'Authorization' not in headers
        assert body['max_tokens'] == 128
    elif provider == 'google':
        assert headers['x-goog-api-key'] == key and '?' not in path
        assert 'Authorization' not in headers
        assert body['generationConfig']['maxOutputTokens'] == 128
    elif provider == 'bedrock':
        assert headers['Authorization'] == 'Bearer ' + key
        assert body['inferenceConfig']['maxTokens'] == 128
    elif provider == 'deepseek':
        assert body['thinking'] == {'type': 'disabled'} and 'reasoning_effort' not in body
        assert headers['Authorization'] == 'Bearer ' + key
    else: assert 'Authorization' not in headers
    if index and provider != 'ollama':
        assert 'PRIVATE_REASONING' in json.dumps(body) or 'PRIVATE_SIGNATURE' in json.dumps(body)
        assert 'credential_tag' not in json.dumps(body)
    if index and provider == 'google':
        replies = [p['functionResponse'] for m in body['contents'] for p in m['parts'] if 'functionResponse' in p]
        assert replies[-1]['name'] == 'write_file' and 'id' not in replies[-1]


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-native-') as directory:
        root = Path(directory)
        for provider in ('anthropic', 'google', 'deepseek', 'bedrock', 'lmstudio', 'ollama'):
            case = root / provider
            case.mkdir()
            def callback(path, headers, raw, body, index):
                verify_request(provider, path, headers, body, index)
                return answer(provider, index == 0)
            server = Fixture(callback)
            server.start()
            try:
                options = [*opts(case, server, provider), '--cwd', str(case), '--allow-write']
                if provider == 'deepseek': options += ['--thinking', 'disabled']
                result = command('exec', options, 'fixture', '--json')
                assert result['output'] == 'NATIVE_OK' and (case / 'answer').read_text() == '42\n'
                usage = result['stats']['usage']
                assert usage['input_tokens'] == {'sum': 6, 'samples': 2}
                assert usage['output_tokens'] == {'sum': 4, 'samples': 2}
                assert usage['total_tokens'] == {'sum': 10, 'samples': 2}
                assert usage['reported_cost_nano_usd'] is None
                saved = json.loads(next((case / 'state').glob('s-*.json')).read_text())
                assert saved['stats']['usage'] == usage
                assert 'PRIVATE_' not in json.dumps(result)
                changed = list(options)
                changed[changed.index('--max-output-tokens') + 1] = '256'
                command('exec', [*changed, '--resume', result['session_id']], 'must reject drift', '--json', ok=False)
                assert len(server.requests) == 2
                continued = command('exec', [*options, '--resume', result['session_id']], 'continue', '--json')
                assert continued['output'] == 'NATIVE_OK' and len(server.requests) == 3
                if provider not in ('ollama', 'lmstudio'):
                    changed_env = env()
                    changed_env['NAMZU_TUI_FIXTURE_KEY'] = 'rotated-fixture-key'
                    command('exec', [*options, '--resume', result['session_id']], 'must reject key drift', '--json', ok=False, custom_env=changed_env)
                    assert len(server.requests) == 3
                checks.append(provider + '_real_cli_tool_file_oracle_private_resume_usage_and_route_pin')
            finally: server.close()

        for provider in ('anthropic', 'google', 'bedrock', 'ollama'):
            case = root / ('incomplete-' + provider)
            case.mkdir()
            def incomplete(path, headers, raw, body, index):
                value = answer(provider, True)
                if provider == 'anthropic': value['stop_reason'] = 'max_tokens'
                elif provider == 'google': value['candidates'][0]['finishReason'] = 'MAX_TOKENS'
                elif provider == 'bedrock': value['stopReason'] = 'max_tokens'
                else: value['done'] = False
                return value
            server = Fixture(incomplete)
            server.start()
            try:
                options = [*opts(case, server, provider), '--cwd', str(case), '--allow-write']
                command('exec', options, 'fixture', '--json', ok=False)
                assert not (case / 'answer').exists() and len(server.requests) == 1
                for extra in (['--api-stream', 'true'], ['--effort', 'low']):
                    command('exec', [*options, *extra], 'must reject unsupported option', '--json', ok=False)
                assert len(server.requests) == 1
                checks.append(provider + '_incomplete_has_no_tool_effect_and_unhonored_options_send_nothing')
            finally: server.close()

        case = root / 'sigv4'
        case.mkdir()
        def signed(path, headers, raw, body, index):
            assert path == '/model/arn:aws:bedrock:us-east-1:000000000000:inference-profile%2Ffixture/converse'
            digest = lambda data: hashlib.sha256(data).hexdigest()
            timestamp = headers['x-amz-date']
            assert re.fullmatch(r'\d{8}T\d{6}Z', timestamp)
            assert headers['x-amz-content-sha256'] == digest(raw)
            auth = headers['Authorization']
            access_scope, names, signature = re.fullmatch(r'AWS4-HMAC-SHA256 Credential=(.*), SignedHeaders=(.*), Signature=([0-9a-f]{64})', auth).groups()
            access, scope = access_scope.split('/', 1)
            assert access == 'FIXTUREACCESS' and scope == timestamp[:8] + '/us-east-1/bedrock/aws4_request'
            canonical_headers = ''.join(name + ':' + headers[name].strip() + '\n' for name in names.split(';'))
            canonical = 'POST\n' + urllib.parse.quote(path, safe='/-_.~') + '\n\n' + canonical_headers + '\n' + names + '\n' + digest(raw)
            string = 'AWS4-HMAC-SHA256\n' + timestamp + '\n' + scope + '\n' + digest(canonical.encode())
            key = b'AWS4fixture-secret'
            for item in (timestamp[:8], 'us-east-1', 'bedrock', 'aws4_request'):
                key = hmac.new(key, item.encode(), hashlib.sha256).digest()
            assert hmac.compare_digest(signature, hmac.new(key, string.encode(), hashlib.sha256).hexdigest())
            assert headers['x-amz-security-token'] == 'fixture-token'
            return answer('bedrock')
        server = Fixture(signed)
        server.start()
        try:
            options = opts(case, server, 'bedrock', 'arn:aws:bedrock:us-east-1:000000000000:inference-profile/fixture')
            options[options.index('--api-key-env') + 1] = 'AWS_ACCESS_KEY_ID'
            result = command('exec', [*options, '--bedrock-auth', 'sigv4', '--aws-region', 'us-east-1', '--cwd', str(case)], 'fixture', '--json')
            assert result['output'] == 'NATIVE_OK'
            checks.append('bedrock_actual_sigv4_payload_path_headers_scope_and_session_token_verified_independently')
        finally: server.close()

        for pal_mode in (False, True):
            for allow in (False, True):
                provider = 'google' if pal_mode else 'anthropic'
                case = root / f'pty-{pal_mode}-{allow}'
                case.mkdir()
                server = Fixture(lambda path, headers, raw, body, index: answer(provider, index == 0))
                options = [*opts(case, server, provider), '--allow-write']
                workspace = case
                if pal_mode:
                    pal = command('pal', options, 'create', 'NativePal')['result']
                    workspace = Path(pal['workspace'])
                    terminal = Terminal(BINARY, [*options, 'tui', pal['id']], case, command=['pal'])
                else: terminal = Terminal(BINARY, [*options, '--cwd', str(case)], case)
                server.start()
                try:
                    terminal.until(lambda: terminal.contains('Enter a message'))
                    terminal.send(b'fixture\r')
                    terminal.until(lambda: terminal.contains('Allow once?'))
                    assert not (workspace / 'answer').exists()
                    terminal.send(b'y' if allow else b'n')
                    terminal.until(lambda: terminal.contains('NATIVE_OK') and 'Ready' in terminal.screen.text())
                    assert (workspace / 'answer').exists() is allow
                    assert terminal.contains('tokens 6/4 in/out')
                    terminal.quit()
                    checks.append(f'{provider}_{"pal" if pal_mode else "normal"}_pty_native_tool_{"allow" if allow else "deny"}_and_token_display')
                finally:
                    terminal.close()
                    server.close()
    report = {'passed': len(checks), 'checks': checks, 'live_model_calls': 0}
    (ROOT / 'artifacts/native-providers-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
