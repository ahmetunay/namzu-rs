#!/usr/bin/env python3
"""6.1 real release CLI/Pal/PTY provider contract acceptance; loopback only."""
import http.server
import json
from pathlib import Path
import subprocess
import tempfile
import threading
from tui_smoke import Terminal, tools, answer
from pals_smoke import environment
ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    assert self.path == '/v1/chat/completions'
                    size = int(self.headers['Content-Length'])
                    assert 0 < size <= 4 * 1024 * 1024
                    body = json.loads(self.rfile.read(size))
                    assert body['stream'] is False
                    reply = callback(body, len(self.server.requests))
                    self.server.requests.append(body)
                    status, headers = 200, {}
                    if isinstance(reply, tuple): status, reply, headers = reply
                    payload = json.dumps(reply).encode()
                    self.send_response(status)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    for name, value in headers.items(): self.send_header(name, value)
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


def options(root, server):
    return ['--state-dir', str(root / 'state'), '--model', 'fixture', '--api-key-env',
        'NAMZU_TUI_FIXTURE_KEY', '--base-url', f'http://127.0.0.1:{server.server_port}/v1', '--timeout', '20']


def command(kind, opts, *args, ok=True):
    result = subprocess.run([str(BINARY), kind, *opts, *args], capture_output=True, env=environment(), timeout=30)
    data = json.loads(result.stdout)
    assert (result.returncode == 0) == ok, (data, result.stderr.decode())
    return data


def totals(root):
    return [json.loads(p.read_text()) for p in (root / 'state').glob('s-*.json')]


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-provider-contract-') as directory:
        root = Path(directory)
        work = root / 'roundtrip'
        work.mkdir()
        def roundtrip(body, index):
            if index == 0:
                reply = tools([('write', 'write_file', {'path': 'answer', 'content': '12'})])
                reply['usage'] = {'prompt_tokens': 10, 'completion_tokens': 2, 'total_tokens': 12,
                    'prompt_tokens_details': {'cached_tokens': 2}}
                return reply
            if index == 1: return tools([('read', 'read_file', {'path': 'answer'})])
            if index == 2:
                reply = answer('12')
                reply['usage'] = {'prompt_tokens': 20, 'completion_tokens': 4, 'total_tokens': 24,
                    'completion_tokens_details': {'reasoning_tokens': 1}}
                return reply
            return answer('continued without usage')
        server = Fixture(roundtrip)
        server.start()
        try:
            opts = options(work, server)
            result = command('exec', [*opts, '--cwd', str(work), '--allow-write'], 'known answer', '--json')
            usage = result['stats']['usage']
            assert result['output'] == '12' and (work / 'answer').read_text() == '12'
            assert usage['responses'] == 3
            assert usage['input_tokens'] == {'sum': 30, 'samples': 2}
            assert usage['output_tokens'] == {'sum': 6, 'samples': 2}
            assert usage['total_tokens'] == {'sum': 36, 'samples': 2}
            assert usage['cached_input_tokens'] == {'sum': 2, 'samples': 1}
            assert usage['reasoning_tokens'] == {'sum': 1, 'samples': 1}
            assert usage['reported_cost_nano_usd'] is None and usage['estimated_cost_nano_usd'] is None
            assert totals(work)[0]['stats']['usage'] == usage
            checks.append('cli_wire_usage_partial_coverage_and_durable_stats')
            resumed = command('exec', [*opts, '--cwd', str(work), '--resume', result['session_id']], 'continue', '--json')
            assert 'usage' not in resumed['stats'] and resumed['stats']['provider_calls'] == 1
            checks.append('missing_usage_not_zero_and_resume_stats_are_per_turn')
        finally: server.close()
        for status, kind in [(401, 'authentication'), (429, 'rate_limited'), (503, 'unavailable')]:
            case = root / f'error-{status}'
            case.mkdir()
            server = Fixture(lambda body, index: (status, {'error': 'PRIVATE_SERVER_PAYLOAD'}, {'Retry-After': '9'}))
            server.start()
            try:
                result = command('exec', [*options(case, server), '--cwd', str(case)], 'fixture', '--json', ok=False)
                assert result['provider_error'] == {'kind': kind, 'http_status': status, 'retry_after_seconds': 9}
                assert 'PRIVATE_SERVER_PAYLOAD' not in json.dumps(result)
                assert len(server.requests) == 1 and totals(case)[0]['status'] == 'failed'
            finally: server.close()
        checks.append('redacted_typed_http_errors_retry_hint_without_retry')
        for mode in ('malformed-usage', 'incomplete'):
            case = root / mode
            case.mkdir()
            def bad(body, index):
                reply = tools([('forbidden', 'write_file', {'path': 'must-not-exist', 'content': 'bad'})])
                if mode == 'malformed-usage': reply['usage'] = {'prompt_tokens': -1}
                else: reply['choices'][0]['finish_reason'] = 'length'
                return reply
            server = Fixture(bad)
            server.start()
            try:
                command('exec', [*options(case, server), '--cwd', str(case), '--allow-write'], 'fixture', '--json', ok=False)
                assert not (case / 'must-not-exist').exists() and len(server.requests) == 1
                assert 'usage' not in totals(case)[0]['stats']
            finally: server.close()
        checks.append('invalid_usage_or_incomplete_response_has_no_tool_effect')
        case = root / 'pal'
        case.mkdir()
        server = Fixture(lambda body, index: (429, {'error': 'PRIVATE_SERVER_PAYLOAD'}, {}))
        server.start()
        try:
            opts = options(case, server)
            pal = command('pal', opts, 'create', 'ContractPal')['result']
            result = command('pal', opts, 'ask', pal['id'], 'fixture', ok=False)
            assert result['result']['provider_error']['kind'] == 'rate_limited'
            assert 'PRIVATE_SERVER_PAYLOAD' not in json.dumps(result)
            assert len(server.requests) == 1
            checks.append('pal_exposes_same_typed_provider_failure')
        finally: server.close()
        case = root / 'pty'
        case.mkdir()
        def metered(body, index):
            reply = answer('CONTRACT_PTY_OK')
            reply['usage'] = {'prompt_tokens': 12, 'completion_tokens': 3, 'total_tokens': 15}
            return reply
        server = Fixture(metered)
        terminal = Terminal(BINARY, [*options(case, server), '--cwd', str(case)], case)
        server.start()
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'fixture\r')
            terminal.until(lambda: terminal.contains('CONTRACT_PTY_OK') and 'Ready' in terminal.screen.text())
            assert terminal.contains('tokens 12/3 in/out')
            assert totals(case)[0]['stats']['usage']['total_tokens']['sum'] == 15
            terminal.quit()
            checks.append('real_pty_shows_reported_tokens_and_restores_terminal')
        finally:
            terminal.close()
            server.close()
    report = {'passed': len(checks), 'checks': checks, 'live_model_calls': 0}
    (ROOT / 'artifacts/provider-contract-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
