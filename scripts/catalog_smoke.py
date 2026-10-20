#!/usr/bin/env python3
"""6.4 explicit discovery, private snapshots and real CLI/PTY model guards. Loopback only."""
import hashlib
import hmac
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import urllib.parse
import re
from native_providers_smoke import answer, env
from tui_smoke import Terminal

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def respond(self):
                try:
                    raw = self.rfile.read(int(self.headers.get('Content-Length', '0')))
                    body = json.loads(raw) if raw else None
                    reply = callback(self.command, self.path, self.headers, body,
                        sum(r['method'] == self.command for r in self.server.requests))
                    self.server.requests.append({'method': self.command, 'path': self.path, 'body': body})
                    status, reply = reply if isinstance(reply, tuple) else (200, reply)
                    payload = json.dumps(reply).encode()
                    self.send_response(status)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append((type(error).__name__, str(error)))
                    self.send_error(500)
            do_GET = do_POST = respond
            def log_message(self, *_): pass
        super().__init__(('127.0.0.1', 0), Handler)
        self.requests, self.errors, self.thread = [], [], None
    def start(self):
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()
    def pause(self):
        if self.thread:
            self.shutdown()
            self.thread.join()
            self.thread = None
    def close(self):
        self.pause()
        self.server_close()
        assert not self.errors, self.errors


def options(root, server, provider='openrouter'):
    return ['--state-dir', str(root / 'state'), '--provider', provider, '--base-url',
        f'http://127.0.0.1:{server.server_port}', '--api-key-env',
        'NAMZU_NATIVE_MISSING_KEY' if provider in ('lmstudio', 'ollama') else 'NAMZU_TUI_FIXTURE_KEY']


def command(kind, opts, *args, ok=True, environment=None):
    result = subprocess.run([str(BINARY), kind, *opts, *args], capture_output=True,
        env=environment or env(), timeout=30)
    assert (result.returncode == 0) == ok, (result.stdout.decode(), result.stderr.decode())
    return json.loads(result.stdout)


def row():
    return {'id': 'fixture', 'context_length': 1000, 'max_output_tokens': 100,
        'supported_parameters': ['tools'], 'architecture': {'input_modalities': ['text']}}


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-catalogue-') as directory:
        root = Path(directory)
        case = root / 'roundtrip'
        case.mkdir()
        mode = 'valid'
        def callback(method, path, headers, body, index):
            if method == 'GET':
                assert path == '/models'
                assert headers['Authorization'] == 'Bearer fixture-only-not-a-real-key'
                if mode == 'error': return 503, {'error': 'PRIVATE_ERROR'}
                if mode == 'duplicate': return {'data': [row(), row()]}
                return {'data': [row()]}
            assert path == '/chat/completions' and body['model'] == 'fixture'
            assert body['max_tokens'] == 100 and 'models' not in body
            return answer('deepseek', index == 0)
        server = Fixture(callback)
        server.start()
        try:
            opts = options(case, server)
            listing = command('models', opts, '--save')
            catalog_id = listing['catalog_id']
            record = listing['catalog']['models'][0]
            assert record['context_window'] == 1000 and record['output_token_limit'] == 100
            assert record['input_token_limit'] is None and record['images'] is False
            assert 'credential_tag' not in json.dumps(listing)
            cache = case / 'state/catalogs' / (catalog_id + '.json')
            assert cache.stat().st_mode & 0o777 == 0o600
            offline_env = env()
            offline_env.pop('NAMZU_TUI_FIXTURE_KEY', None)
            offline = command('models', ['--state-dir', str(case / 'state'), '--cached', catalog_id], environment=offline_env)
            assert offline['offline'] is True and offline['catalog']['models'] == listing['catalog']['models']
            checks.append('discovery_exact_metadata_private_cache_and_offline_inspection_without_key')
            run_opts = [*opts, '--model', 'fixture', '--model-catalog', catalog_id, '--cwd', str(case), '--allow-write']
            result = command('exec', run_opts, 'fixture', '--json')
            assert result['output'] == 'NATIVE_OK' and (case / 'answer').read_text() == '42\n'
            resumed = command('exec', [*run_opts, '--resume', result['session_id']], 'continue', '--json')
            assert resumed['output'] == 'NATIVE_OK'
            assert sum(r['method'] == 'GET' for r in server.requests) == 1
            checks.append('selected_catalogue_caps_real_wire_output_and_tool_oracle_and_resumes_without_discovery')
            count = len(server.requests)
            for field, value in [('--model', 'other-paid-model'), ('--api-key-env', 'OTHER_SELECTOR'), ('--provider', 'zen')]:
                changed = list(run_opts)
                changed[changed.index(field) + 1] = value
                command('exec', changed, 'must reject', '--json', ok=False)
            command('exec', [*run_opts, '--max-output-tokens', '101'], 'must reject limit', '--json', ok=False)
            rotated = env()
            rotated['NAMZU_TUI_FIXTURE_KEY'] = 'changed-fixture-key'
            command('exec', run_opts, 'must reject credential', '--json', ok=False, environment=rotated)
            without = list(run_opts)
            i = without.index('--model-catalog')
            del without[i:i+2]
            command('exec', [*without, '--resume', result['session_id']], 'must reject route drift', '--json', ok=False)
            assert len(server.requests) == count
            checks.append('absent_model_key_selector_provider_output_and_resume_policy_drift_send_nothing_no_fallback')
            original = cache.read_bytes()
            tampered = json.loads(original)
            tampered['catalog']['models'][0]['output_token_limit'] = 99
            cache.write_text(json.dumps(tampered))
            command('exec', run_opts, 'must reject corruption', '--json', ok=False)
            cache.write_bytes(original)
            cache.chmod(0o644)
            command('models', ['--state-dir', str(case / 'state'), '--cached', catalog_id], ok=False)
            cache.chmod(0o600)
            assert len(server.requests) == count
            checks.append('digest_tampering_and_nonprivate_snapshot_are_rejected_before_effects')
            for mode in ('error', 'duplicate'):
                failed = command('models', opts, '--save', ok=False)
                assert 'PRIVATE_ERROR' not in json.dumps(failed)
                assert list((case / 'state/catalogs').glob('*.json')) == [cache]
            checks.append('failed_refresh_and_duplicate_models_never_publish_or_use_stale_fallback')
        finally: server.close()

        case = root / 'google'
        case.mkdir()
        def google(method, path, headers, body, index):
            if method == 'GET':
                assert headers['x-goog-api-key'] == 'fixture-only-not-a-real-key'
                if index == 0:
                    assert path == '/models?pageSize=1000'
                    return {'models': [{'name': 'models/fixture', 'inputTokenLimit': 900,
                        'outputTokenLimit': 100, 'supportedGenerationMethods': ['generateContent']},
                        {'name': 'models/embed', 'supportedGenerationMethods': ['embedContent']}],
                        'nextPageToken': 'opaque/&?cursor'}
                assert path == '/models?pageSize=1000&pageToken=opaque%2F%26%3Fcursor'
                return {'models': [{'name': 'models/second', 'supportedGenerationMethods': ['generateContent']}]}
            assert path == '/models/fixture:generateContent'
            assert body['generationConfig']['maxOutputTokens'] == 100
            return answer('google', index == 0)
        server = Fixture(google)
        server.start()
        try:
            opts = options(case, server, 'google')
            result = command('models', opts, '--save')
            models = result['catalog']['models']
            assert [m['id'] for m in models] == ['fixture', 'second']
            assert models[0]['context_window'] is None and models[0]['input_token_limit'] == 900
            done = command('exec', [*opts, '--model', 'models/fixture', '--model-catalog', result['catalog_id'],
                '--cwd', str(case), '--allow-write'], 'fixture', '--json')
            assert done['output'] == 'NATIVE_OK' and (case / 'answer').read_text() == '42\n'
            checks.append('google_atomic_paging_input_output_units_generate_filter_and_native_selected_model')
        finally: server.close()

        for provider in ('openai-compatible', 'deepseek', 'zen', 'zen-go', 'lmstudio', 'ollama', 'bedrock', 'anthropic'):
            case = root / provider
            case.mkdir()
            def listing(method, path, headers, body, index):
                assert method == 'GET'
                if provider == 'ollama':
                    assert path == '/api/tags' and 'Authorization' not in headers
                    return {'models': [{'name': 'fixture'}]}
                if provider == 'bedrock':
                    assert path == '/foundation-models'
                    return {'modelSummaries': [{'modelId': 'fixture', 'modelArn': 'arn:fixture', 'inputModalities': ['TEXT', 'IMAGE']}]}
                if provider == 'anthropic':
                    assert headers['x-api-key'] == 'fixture-only-not-a-real-key'
                    if index == 0:
                        assert path == '/models?limit=1000'
                        return {'data': [{'id': 'fixture'}], 'has_more': True, 'last_id': 'fixture'}
                    assert path == '/models?limit=1000&after_id=fixture'
                    return {'data': [{'id': 'second'}], 'has_more': False}
                assert path == '/models'
                if provider == 'lmstudio': assert 'Authorization' not in headers
                else: assert headers['Authorization'] == 'Bearer fixture-only-not-a-real-key'
                return {'data': [{'id': 'fixture'}]}
            server = Fixture(listing)
            server.start()
            try:
                result = command('models', options(case, server, provider), '--save')
                assert result['catalog']['provider'] == provider
                assert result['catalog']['models'][0]['context_window'] is None
                assert all(r['method'] == 'GET' for r in server.requests)
                checks.append(provider + '_explicit_catalogue_path_auth_and_unknown_limits_without_inference')
            finally: server.close()

        case = root / 'failed-pages'
        case.mkdir()
        def bad_page(method, path, headers, body, index):
            if index == 0: return {'models': [{'name': 'models/fixture'}], 'nextPageToken': 'second'}
            return 429, {'error': 'PRIVATE_SERVER_MESSAGE'}
        server = Fixture(bad_page)
        server.start()
        try:
            failed = command('models', options(case, server, 'google'), '--save', ok=False)
            assert failed['provider_error']['kind'] == 'rate_limited'
            assert 'PRIVATE_SERVER_MESSAGE' not in json.dumps(failed)
            assert not (case / 'state/catalogs').exists()
            checks.append('later_page_failure_has_no_partial_saved_catalogue_or_retry')
        finally: server.close()

        case = root / 'sigv4'
        case.mkdir()
        def sigv4(method, path, headers, body, index):
            assert method == 'GET' and path == '/foundation-models'
            auth = headers['Authorization']
            access_scope, names, signature = re.fullmatch(r'AWS4-HMAC-SHA256 Credential=(.*), SignedHeaders=(.*), Signature=([0-9a-f]{64})', auth).groups()
            assert access_scope.startswith('FIXTUREACCESS/')
            scope = access_scope.split('/', 1)[1]
            timestamp = headers['x-amz-date']
            digest = lambda data: hashlib.sha256(data).hexdigest()
            assert headers['x-amz-content-sha256'] == digest(b'')
            lines = ''.join(name + ':' + headers[name].strip() + '\n' for name in names.split(';'))
            canonical = 'GET\n' + path + '\n\n' + lines + '\n' + names + '\n' + digest(b'')
            string = 'AWS4-HMAC-SHA256\n' + timestamp + '\n' + scope + '\n' + digest(canonical.encode())
            key = b'AWS4fixture-secret'
            for text in (timestamp[:8], 'us-east-1', 'bedrock', 'aws4_request'):
                key = hmac.new(key, text.encode(), hashlib.sha256).digest()
            assert hmac.compare_digest(signature, hmac.new(key, string.encode(), hashlib.sha256).hexdigest())
            assert headers['x-amz-security-token'] == 'fixture-token'
            return {'modelSummaries': [{'modelId': 'fixture'}]}
        server = Fixture(sigv4)
        server.start()
        try:
            opts = options(case, server, 'bedrock')
            opts[opts.index('--api-key-env') + 1] = 'AWS_ACCESS_KEY_ID'
            result = command('models', [*opts, '--bedrock-auth', 'sigv4', '--aws-region', 'us-east-1'], '--save')
            assert result['catalog']['models'][0]['id'] == 'fixture'
            checks.append('actual_bedrock_get_sigv4_empty_payload_headers_scope_and_session_token_verified_independently')
        finally: server.close()

        for pal_mode in (False, True):
            case = root / ('pal-pty' if pal_mode else 'normal-pty')
            case.mkdir()
            def pty_reply(method, path, headers, body, index):
                if method == 'GET': return {'data': [row()]}
                assert body['max_tokens'] == 100
                return answer('deepseek', index == 0)
            server = Fixture(pty_reply)
            server.start()
            opts = options(case, server)
            listing = command('models', opts, '--save')
            server.pause()  # Fork only after fixture threads have joined; retain the same bound endpoint.
            run_opts = [*opts, '--model', 'fixture', '--model-catalog', listing['catalog_id'], '--allow-write']
            workspace = case
            if pal_mode:
                pal = command('pal', run_opts, 'create', 'CataloguePal')['result']
                workspace = Path(pal['workspace'])
                terminal = Terminal(BINARY, [*run_opts, 'tui', pal['id']], case, command=['pal'])
            else: terminal = Terminal(BINARY, [*run_opts, '--cwd', str(case)], case)
            server.start()
            try:
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'fixture\r')
                terminal.until(lambda: terminal.contains('Allow once?'))
                assert not (workspace / 'answer').exists()
                terminal.send(b'n' if pal_mode else b'y')
                terminal.until(lambda: terminal.contains('NATIVE_OK') and 'Ready' in terminal.screen.text())
                assert (workspace / 'answer').exists() is (not pal_mode)
                if not pal_mode:
                    terminal.send(b'\x1b[17~')
                    terminal.until(lambda: 'Model and turn budgets' in terminal.screen.text())
                    terminal.send(b'\x15other-paid-model\r')
                    terminal.until(lambda: 'absent' in terminal.screen.text())
                    terminal.send(b'\x1b')
                    terminal.until(lambda: 'Model and turn budgets' not in terminal.screen.text())
                assert sum(r['method'] == 'GET' for r in server.requests) == 1
                assert sum(r['method'] == 'POST' for r in server.requests) == 2
                terminal.quit()
                checks.append(('pal' if pal_mode else 'normal') + '_pty_saved_catalogue_tool_approval_and_no_implicit_refresh_or_model_fallback')
            finally:
                terminal.close()
                server.close()
    report = {'passed': len(checks), 'checks': checks, 'live_model_calls': 0}
    (ROOT / 'artifacts/catalog-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
