#!/usr/bin/env python3
"""6.2 release CLI/Pal/PTY acceptance. Loopback HTTP, fake keys, no model calls."""
import http.server
import json
from pathlib import Path
import subprocess
import tempfile
import threading
from pals_smoke import environment
from tui_smoke import Terminal

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


def event(value):
    return 'data: ' + json.dumps(value) + '\n\n'


def chat(text='', call=None, details=None, cost='0.00000000100000000000000001'):
    delta = {'content': text}
    if details is not None: delta['reasoning_details'] = details
    if call:
        name, args = call
        args = json.dumps(args)
        delta['tool_calls'] = [{'index': 0, 'id': 'call-1', 'type': 'function',
            'function': {'name': name, 'arguments': args[:3]}}]
    result = event({'choices': [{'index': 0, 'delta': delta, 'finish_reason': None}]})
    if call:
        result += event({'choices': [{'index': 0, 'delta': {'tool_calls': [
            {'index': 0, 'function': {'arguments': args[3:]}}]}, 'finish_reason': None}]})
    result += event({'choices': [{'index': 0, 'delta': {}, 'finish_reason': 'tool_calls' if call else 'stop'}]})
    result += ('data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,'
        '"total_tokens":5,"cost":' + cost + '}}\n\n')
    return result + 'data: [DONE]\n\n'


def response(items, stream):
    value = {'status': 'completed', 'output': items,
        'usage': {'input_tokens': 3, 'output_tokens': 2, 'total_tokens': 5}}
    return event({'type': 'response.completed', 'response': value}) if stream else value


def text_item(text):
    return {'type': 'message', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': text}]}


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    size = int(self.headers['Content-Length'])
                    assert 0 < size <= 8 * 1024 * 1024
                    body = json.loads(self.rfile.read(size))
                    reply = callback(self.path, self.headers, body, len(self.server.requests))
                    self.server.requests.append(body)
                    gated = isinstance(reply, tuple)
                    if gated:
                        prefix, gate, suffix = reply
                        payload = (prefix + suffix).encode()
                    else:
                        payload = (reply if isinstance(reply, str) else json.dumps(reply)).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream' if body['stream'] else 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    if gated:
                        self.wfile.write(prefix.encode())
                        self.wfile.flush()
                        assert gate.wait(12), 'stream gate was never released'
                        self.wfile.write(suffix.encode())
                    else: self.wfile.write(payload)
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


def options(root, server, provider='openrouter', model='fixture', stream=True):
    return ['--state-dir', str(root / 'state'), '--provider', provider, '--model', model,
        '--base-url', f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env',
        'NAMZU_TUI_FIXTURE_KEY', '--api-stream', str(stream).lower(), '--timeout', '20']


def command(kind, opts, *args, ok=True):
    result = subprocess.run([str(BINARY), kind, *opts, *args], capture_output=True,
        env=environment(), timeout=30)
    assert (result.returncode == 0) == ok, (result.stdout.decode(), result.stderr.decode())
    return json.loads(result.stdout)


def totals(root):
    return [json.loads(p.read_text()) for p in (root / 'state').glob('s-*.json')]


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-services-') as directory:
        root = Path(directory)
        case = root / 'openrouter'
        case.mkdir()
        private = [{'type': 'reasoning.encrypted', 'data': 'PRIVATE_REASONING', 'index': 0}]
        def router(path, headers, body, index):
            assert path == '/v1/chat/completions' and body['stream'] is True
            assert headers['Authorization'] == 'Bearer fixture-only-not-a-real-key'
            assert body['provider'] == {'allow_fallbacks': False, 'require_parameters': True}
            assert body['model'] == 'fixture' and 'models' not in body
            assert body['stream_options']['include_usage'] is True
            if index == 0:
                return chat(call=('write_file', {'path': 'answer', 'content': '42\n'}), details=private)
            assert any(m.get('reasoning_details') == private for m in body['messages'])
            assert any(m.get('role') == 'tool' for m in body['messages'])
            return chat('ROUTER_OK')
        server = Fixture(router)
        server.start()
        try:
            opts = options(case, server)
            result = command('exec', [*opts, '--cwd', str(case), '--allow-write'], 'fixture', '--json')
            assert result['output'] == 'ROUTER_OK' and (case / 'answer').read_text() == '42\n'
            usage = result['stats']['usage']
            assert usage['reported_cost_nano_usd'] == {'sum': 4, 'samples': 2}
            assert usage['estimated_cost_nano_usd'] is None
            assert totals(case)[0]['stats']['usage'] == usage
            assert 'PRIVATE_REASONING' not in json.dumps(result)
            checks.append('router_stream_fragmented_tools_private_replay_exact_cost_and_durable_stats')
            for field, value in [('--api-stream', 'false'), ('--api-key-env', 'OTHER_FIXTURE_KEY')]:
                changed = list(opts)
                changed[changed.index(field) + 1] = value
                command('exec', [*changed, '--cwd', str(case), '--allow-write', '--resume', result['session_id']],
                    'must reject', '--json', ok=False)
            assert len(server.requests) == 2
            resumed = command('exec', [*opts, '--cwd', str(case), '--allow-write', '--resume', result['session_id']],
                'continue', '--json')
            assert resumed['output'] == 'ROUTER_OK' and len(server.requests) == 3
            checks.append('session_pins_service_options_key_selector_and_restores_private_reasoning')
        finally: server.close()

        for provider, streaming in [('zen', True), ('openai-compatible', False)]:
            case = root / provider
            case.mkdir()
            def responses(path, headers, body, index):
                assert path == '/v1/responses' and body['stream'] is streaming
                assert body['store'] is False and body['max_output_tokens'] == 128
                assert body['include'] == ['reasoning.encrypted_content']
                assert 'messages' not in body and 'chatgpt-account-id' not in headers
                if index == 0:
                    return response([{'type': 'reasoning', 'encrypted_content': 'PRIVATE_RESPONSE'},
                        {'type': 'function_call', 'call_id': 'responses-call', 'name': 'write_file',
                        'arguments': json.dumps({'path': 'answer', 'content': '64'})}], streaming)
                assert any(i.get('encrypted_content') == 'PRIVATE_RESPONSE' for i in body['input'])
                assert any(i.get('type') == 'function_call_output' and i['call_id'] == 'responses-call'
                    for i in body['input'])
                return response([text_item('RESPONSES_OK')], streaming)
            server = Fixture(responses)
            server.start()
            try:
                opts = options(case, server, provider, 'gpt-fixture', streaming)
                result = command('exec', [*opts, '--api-format', 'responses', '--max-output-tokens', '128',
                    '--cwd', str(case), '--allow-write'], 'fixture', '--json')
                assert result['output'] == 'RESPONSES_OK' and (case / 'answer').read_text() == '64'
                assert 'PRIVATE_RESPONSE' not in json.dumps(result)
                assert result['stats']['usage']['total_tokens'] == {'sum': 10, 'samples': 2}
                checks.append(f'{provider}_keyed_responses_{"sse" if streaming else "buffered"}_tool_and_opaque_replay')
            finally: server.close()

        case = root / 'zen-go'
        case.mkdir()
        def deepseek(path, headers, body, index):
            assert path == '/v1/chat/completions' and body['model'] == 'deepseek-fixture'
            if index:
                assert any(m.get('reasoning_content') == 'PRIVATE_GO' for m in body['messages'])
            return {'choices': [{'finish_reason': 'stop', 'message': {'role': 'assistant',
                'content': 'GO_OK', 'reasoning_content': 'PRIVATE_GO'}}]}
        server = Fixture(deepseek)
        server.start()
        try:
            opts = [*options(case, server, 'zen-go', 'go/deepseek-fixture', False), '--cwd', str(case)]
            first = command('exec', opts, 'fixture', '--json')
            second = command('exec', [*opts, '--resume', first['session_id']], 'continue', '--json')
            assert second['output'] == 'GO_OK' and 'PRIVATE_GO' not in json.dumps(second)
            checks.append('zen_go_prefix_normalization_chat_and_private_resume')
        finally: server.close()

        for mode in ('missing-done', 'invalid-cost', 'http200-error', 'incomplete', 'refusal'):
            case = root / mode
            case.mkdir()
            def bad(path, headers, body, index):
                reply = chat(call=('write_file', {'path': 'forbidden', 'content': 'bad'}))
                if mode == 'missing-done': return reply.removesuffix('data: [DONE]\n\n')
                if mode == 'invalid-cost': return reply.replace('0.00000000100000000000000001', '-1')
                if mode == 'http200-error': return event({'error': {'code': 429, 'message': 'PRIVATE_ERROR'}})
                if mode == 'incomplete': return reply.replace('"finish_reason": "tool_calls"', '"finish_reason": "length"')
                return event({'choices': [{'delta': {'refusal': 'PRIVATE_REFUSAL'}, 'finish_reason': 'stop'}]}) + 'data: [DONE]\n\n'
            server = Fixture(bad)
            server.start()
            try:
                result = command('exec', [*options(case, server), '--cwd', str(case), '--allow-write'],
                    'fixture', '--json', ok=False)
                assert not (case / 'forbidden').exists() and len(server.requests) == 1
                assert 'usage' not in totals(case)[0]['stats']
                assert 'PRIVATE_ERROR' not in json.dumps(result) and 'PRIVATE_REFUSAL' not in json.dumps(result)
                if mode == 'http200-error': assert result['provider_error']['kind'] == 'rate_limited'
            finally: server.close()
        checks.append('truncated_bad_cost_embedded_error_incomplete_refused_streams_have_no_effect_or_retry')

        case = root / 'pal'
        case.mkdir()
        server = Fixture(lambda path, headers, body, index: chat('PAL_ROUTER_OK'))
        server.start()
        try:
            opts = options(case, server)
            pal = command('pal', opts, 'create', 'ServicesPal')['result']
            result = command('pal', opts, 'ask', pal['id'], 'fixture')['result']
            assert result['output'] == 'PAL_ROUTER_OK'
            assert result['stats']['usage']['reported_cost_nano_usd'] == {'sum': 2, 'samples': 1}
            changed = list(opts)
            changed[changed.index('--api-stream') + 1] = 'false'
            command('pal', changed, 'ask', pal['id'], 'must reject', ok=False)
            assert len(server.requests) == 1
            checks.append('pal_uses_selected_stream_and_reported_cost_and_rejects_profile_drift')
        finally: server.close()

        for pal_mode in (False, True):
            for allow in (False, True):
                case = root / f'pty-{pal_mode}-{allow}'
                case.mkdir()
                gate = threading.Event()
                def pty_reply(path, headers, body, index):
                    if pal_mode: assert body['reasoning']['effort'] == 'low'
                    if index: return chat('PTY_SERVICE_OK')
                    prefix = event({'choices': [{'delta': {'content': 'PROVISIONAL_SERVICE'}, 'finish_reason': None}]})
                    return (prefix, gate, chat(call=('write_file', {'path': 'approved', 'content': 'yes'})))
                server = Fixture(pty_reply)
                opts = [*options(case, server), '--allow-write']
                workspace = case
                if pal_mode:
                    pal = command('pal', [*opts, '--effort', 'low'], 'create', 'ServicesPTY')['result']
                    workspace = Path(pal['workspace'])
                    terminal = Terminal(BINARY, [*opts, 'tui', pal['id']], case, command=['pal'])
                else:
                    terminal = Terminal(BINARY, [*opts, '--cwd', str(case)], case)
                # Fork before starting fixture threads.
                server.start()
                try:
                    terminal.until(lambda: terminal.contains('Enter a message'))
                    terminal.send(b'fixture\r')
                    terminal.until(lambda: terminal.contains('PROVISIONAL_SERVICE'))
                    assert not (workspace / 'approved').exists() and not gate.is_set()
                    gate.set()
                    terminal.until(lambda: terminal.contains('Allow once?'))
                    terminal.send(b'y' if allow else b'n')
                    terminal.until(lambda: terminal.contains('PTY_SERVICE_OK') and 'Ready' in terminal.screen.text())
                    assert (workspace / 'approved').exists() is allow
                    assert terminal.contains('tokens 6/4 in/out')
                    if pal_mode:
                        terminal.send(b'\x1b[17~')
                        terminal.until(lambda: 'Model and turn budgets' in terminal.screen.text())
                        assert 'low' in terminal.screen.text()
                        terminal.send(b'\x1b')
                        terminal.until(lambda: 'Model and turn budgets' not in terminal.screen.text())
                    terminal.quit()
                    checks.append(f'{"pal" if pal_mode else "normal"}_pty_incremental_text_then_{"allow" if allow else "deny"}_tool')
                finally:
                    gate.set()
                    terminal.close()
                    server.close()
    report = {'passed': len(checks), 'checks': checks, 'live_model_calls': 0}
    (ROOT / 'artifacts/services-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
