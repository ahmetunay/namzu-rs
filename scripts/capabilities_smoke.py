#!/usr/bin/env python3
"""5.5 release exec/TUI/Pal capabilities + optional real Docker. No live model calls."""
import argparse
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from audio_smoke import wav
from tui_smoke import Terminal, answer, tools
from pals_smoke import environment, tasks, docker_command

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    body = self.rfile.read(int(self.headers['Content-Length']))
                    if self.path == '/v1/chat/completions':
                        data = json.loads(body)
                        reply = callback(data, len(self.server.requests))
                        self.server.requests.append(data)
                        payload = json.dumps(reply).encode()
                    elif self.path == '/v1/audio/transcriptions':
                        assert b'stt-fixture' in body and wav() in body
                        self.server.speech.append(self.path)
                        payload = json.dumps({'text': 'fixture transcript', 'language': 'tr'}).encode()
                    else:
                        assert self.path == '/v1/audio/speech'
                        assert json.loads(body)['model'] == 'tts-fixture'
                        self.server.speech.append(self.path)
                        payload = wav()
                    self.send_response(200)
                    self.send_header('Content-Length', str(len(payload)))
                    self.send_header('Content-Type', 'audio/wav' if self.path.endswith('/speech') else 'application/json')
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append((type(error).__name__, str(error)))
                    self.send_error(500)

            def log_message(self, *_): pass
        super().__init__(('127.0.0.1', 0), Handler)
        self.requests, self.speech, self.errors = [], [], []
        self.thread = None

    def start(self):
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        if self.thread:
            self.shutdown()
            self.thread.join()
        self.server_close()
        assert not self.errors, self.errors


def command(kind, opts, *args, ok=True):
    result = subprocess.run([str(BINARY), kind, *opts, *args], capture_output=True, env=environment(), timeout=45)
    data = json.loads(result.stdout)
    assert (result.returncode == 0) == ok, (kind, data, result.stderr.decode())
    if kind == 'exec': return data
    return data.get('result', data.get('output', data)) if ok else data


def setup(root, server):
    (root / 'input.wav').write_bytes(wav())
    (root / 'code.rs').write_text('let value = 1;\nvalue;\n')
    marker = root / 'lsp-started'
    wrapper = root / 'lsp-wrapper.py'
    wrapper.write_text("import pathlib, runpy, sys\npathlib.Path(sys.argv[1]).write_text('started')\nscript=sys.argv[2]\nsys.argv=[script]\nrunpy.run_path(script,run_name='__main__')\n")
    config = {'capabilities': {'audio': {'base_url': f'http://127.0.0.1:{server.server_port}/v1',
        'api_key_env': 'NAMZU_TUI_FIXTURE_KEY', 'transcription_model': 'stt-fixture', 'speech_model': 'tts-fixture'},
        'lsp': {'program': '/usr/bin/python3', 'args': [str(wrapper), str(marker), str(ROOT / 'crates/lsp/tests/fixtures/server.py')]}}}
    config_path = root / 'config.json'
    config_path.write_text(json.dumps(config))
    opts = ['--state-dir', str(root / 'state'), '--config', str(config_path), '--model', 'fixture',
        '--base-url', f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY', '--timeout', '30']
    return opts, config, marker


def value(body, ident):
    return json.loads(next(m['content'] for m in reversed(body['messages']) if m.get('tool_call_id') == ident))


def names(body):
    return {t['function']['name'] for t in body['tools']}


def host_accept(root):
    phase = {'name': 'roundtrip'}
    saved = {}
    def callback(body, index):
        mode = phase['name']
        if mode == 'roundtrip':
            if index == 0:
                assert {'audio_inspect', 'audio_transcribe', 'audio_synthesize', 'lsp_diagnostics', 'lsp_apply'} <= names(body)
                return tools([('inspect', 'audio_inspect', {'path': 'input.wav'}),
                    ('transcribe', 'audio_transcribe', {'path': 'input.wav', 'language': 'tr'}),
                    ('speech', 'audio_synthesize', {'text': 'fixture', 'voice': 'alloy', 'output': 'out.wav'}),
                    ('diagnostics', 'lsp_diagnostics', {'path': 'code.rs'})])
            if index == 1:
                assert 'fixture transcript' in value(body, 'transcribe')['content']
                assert 'toy-lsp' in value(body, 'diagnostics')['content']
                return tools([('preview', 'lsp_rename_preview', {'path': 'code.rs', 'line': 1, 'character': 0, 'new_name': 'next'})])
            if index == 2:
                saved['plan'] = json.loads(value(body, 'preview')['content'])['plan']
                return tools([('apply', 'lsp_apply', {'plan': saved['plan']})])
            assert 'applied' in value(body, 'apply')['content']
            return answer('CAPABILITIES_OK')
        if mode == 'batch':
            return tools([('first', 'write_file', {'path': 'must-not-exist', 'content': 'bad'}),
                          ('invalid', 'audio_transcribe', {'path': 'missing.wav'})])
        if mode == 'resume':
            assert 'audio_transcribe' not in names(body) and 'lsp_diagnostics' not in names(body)
            return answer('RESUME_NARROWED_OK')
        if mode == 'pal':
            if 'pal-diagnostics' not in str(body['messages']):
                return tools([('pal-diagnostics', 'lsp_diagnostics', {'path': 'code.rs'}),
                              ('pal-audio', 'audio_transcribe', {'path': 'input.wav'})])
            return answer('PAL_CAPABILITIES_OK')
        assert mode == 'pal-ceiling'
        assert 'audio_inspect' in names(body) and 'audio_transcribe' not in names(body)
        return answer('PAL_CEILING_OK')
    server = Fixture(callback)
    opts, config, marker = setup(root, server)
    caps = ['--capability', 'audio,lsp', '--allow-audio', '--allow-lsp', '--allow-write']
    base = ['--state-dir', str(root / 'state'), '--config', str(root / 'config.json'), '--cwd', str(root)]
    checks = []
    try:
        inventory = command('capabilities', base)
        assert all(not n.startswith(('audio_', 'lsp_')) for n in inventory['tools'])
        inventory = command('capabilities', [*base, '--capability', 'audio,lsp'])
        assert 'audio_inspect' in inventory['tools'] and 'audio_transcribe' not in inventory['tools']
        assert not marker.exists() and not server.requests and not server.speech
        checks.append('offline_catalogue_no_ambient_activation_or_launch')
        server.start()
        result = command('exec', [*opts, '--cwd', str(root), *caps], 'fixture', '--json')
        assert result['output'] == 'CAPABILITIES_OK'
        assert (root / 'code.rs').read_text() == 'let next = 1;\nnext;\n'
        assert (root / 'out.wav').read_bytes() == wav() and len(server.speech) == 2
        assert marker.exists()
        session = next(json.loads(p.read_text()) for p in (root / 'state').glob('s-*.json') if json.loads(p.read_text())['status'] == 'complete')
        checks.append('real_exec_speech_lsp_preview_hash_bound_apply')
        phase['name'] = 'batch'
        before = len(server.speech)
        command('exec', [*opts, '--cwd', str(root), *caps], 'bad batch', '--json', ok=False)
        assert not (root / 'must-not-exist').exists() and len(server.speech) == before
        checks.append('batch_preflight_blocks_prior_write_and_speech')
        phase['name'] = 'resume'
        command('exec', [*opts, '--cwd', str(root), '--capability', 'audio,lsp', '--resume', session['id']], 'continue', '--json')
        before = len(server.requests)
        changed = json.loads(json.dumps(config))
        changed['capabilities']['audio']['speech_model'] = 'changed'
        (root / 'config.json').write_text(json.dumps(changed))
        command('exec', [*opts, '--cwd', str(root), '--capability', 'audio,lsp', '--resume', session['id']], 'changed', '--json', ok=False)
        assert len(server.requests) == before
        (root / 'config.json').write_text(json.dumps(config))
        checks.append('resume_pins_settings_and_never_restores_grants')
        phase['name'] = 'pal'
        pal = command('pal', [*opts, *caps], 'create', 'CapabilityPal')
        workspace = Path(pal['workspace'])
        (workspace / 'input.wav').write_bytes(wav())
        (workspace / 'code.rs').write_text('let value = 1;\nvalue;\n')
        command('pal', [*opts, *caps], 'ask', pal['id'], 'fixture')
        checks.append('real_pal_speech_lsp_private_workspace')
        phase['name'] = 'pal-ceiling'
        pal = command('pal', [*opts, '--capability', 'audio'], 'create', 'InspectOnlyPal')
        assert 'audio_transcribe' not in pal['tools']
        before = len(server.speech)
        command('pal', [*opts, '--capability', 'audio', '--allow-audio'], 'ask', pal['id'], 'fixture')
        assert len(server.speech) == before
        before = len(server.requests)
        command('pal', [*opts, '--capability', 'audio,lsp'], 'ask', pal['id'], 'changed', ok=False)
        assert len(server.requests) == before
        checks.append('pal_profile_ceiling_and_selection_pin')
        denied = json.loads(json.dumps(config))
        denied['capabilities']['deny'] = ['audio']
        (root / 'config.json').write_text(json.dumps(denied))
        command('capabilities', [*base, '--capability', 'audio'], ok=False)
        assert len(server.requests) == before
        checks.append('config_package_deny')
    finally:
        server.close()
    return checks


def pty_accept(root, pal_mode, allow):
    def callback(body, index):
        if index == 0:
            return tools([('lsp-pty', 'lsp_diagnostics', {'path': 'code.rs'})])
        assert value(body, 'lsp-pty')['is_error'] == (not allow)
        return answer('CAPABILITY_PTY_DONE')
    server = Fixture(callback)
    opts, _, marker = setup(root, server)
    opts += ['--capability', 'lsp', '--allow-lsp']
    if pal_mode:
        pal = command('pal', opts, 'create', 'PtyPal')
        (Path(pal['workspace']) / 'code.rs').write_text('let value = 1;\n')
        terminal = Terminal(BINARY, [*opts, 'tui', pal['id']], root, command=['pal'])
    else:
        terminal = Terminal(BINARY, [*opts, '--cwd', str(root)], root)
    server.start()
    try:
        terminal.until(lambda: terminal.contains('Enter a message'))
        terminal.send(b'fixture\r')
        terminal.until(lambda: terminal.contains('Allow once?'))
        assert terminal.contains('HOST LSP') and not marker.exists()
        terminal.send(b'y' if allow else b'n')
        terminal.until(lambda: terminal.contains('CAPABILITY_PTY_DONE') and 'Ready' in terminal.screen.text())
        assert marker.exists() == allow
        terminal.quit()
    finally:
        terminal.close()
        server.close()
    return f'{"pal" if pal_mode else "normal"}_pty_lsp_{"allow" if allow else "deny"}'


def sandbox_accept(root, image):
    phase = {'mode': 'write'}
    def callback(body, index):
        assert 'audio_transcribe' not in names(body) and 'lsp_diagnostics' not in names(body)
        ident = 'guest-write' if phase['mode'] == 'write' else 'guest-read'
        if not any(m.get('tool_call_id') == ident for m in body['messages']):
            if phase['mode'] == 'write':
                return tools([(ident, 'write_file', {'path': 'retained.txt', 'content': 'guest only'})])
            return tools([(ident, 'read_file', {'path': 'retained.txt'})])
        assert not value(body, ident)['is_error']
        if phase['mode'] == 'read': assert value(body, ident)['content'] == 'guest only'
        return answer('SANDBOX_AGENT_OK')
    server = Fixture(callback)
    opts, _, _ = setup(root, server)
    policy = command('sandbox', [], 'policy', '--profile', 'build')['policy']
    policy_file = root / 'sandbox.json'
    policy_file.write_text(json.dumps(policy))
    selection = ['--computer-image', image, '--sandbox-policy', str(policy_file), '--allow-write']
    checks = []
    exec_id = pal_id = None
    try:
        server.start()
        command('exec', [*opts, '--cwd', str(root), *selection], 'guest write', '--json')
        session = next(json.loads(p.read_text()) for p in (root / 'state').glob('s-*.json'))
        exec_id = session['id']
        record = command('capabilities', ['--state-dir', str(root / 'state')], 'show', exec_id)['computer']
        assert record['phase'] == 'stopped' and record['spec']['sandbox'] == policy
        assert not (root / 'retained.txt').exists()
        phase['mode'] = 'read'
        command('exec', [*opts, '--cwd', str(root), *selection, '--resume', exec_id], 'guest read', '--json')
        record2 = command('capabilities', ['--state-dir', str(root / 'state')], 'show', exec_id)['computer']
        assert record2['container'] == record['container']
        checks.append('exec_sandbox_guest_only_retained_resume')
        policy['workspace'] = 'read_only'
        policy_file.write_text(json.dumps(policy))
        before = len(server.requests)
        command('exec', [*opts, '--cwd', str(root), *selection, '--resume', exec_id], 'changed', '--json', ok=False)
        assert len(server.requests) == before
        retained = command('capabilities', ['--state-dir', str(root / 'state')], 'show', exec_id)['computer']
        assert retained['container'] == record['container'] and retained['phase'] == 'stopped'
        policy['workspace'] = 'read_write'
        policy_file.write_text(json.dumps(policy))
        command('exec', [*opts, '--cwd', str(root), *selection, '--capability', 'audio'], 'escape', '--json', ok=False)
        assert len(server.requests) == before
        checks.append('sandbox_policy_pin_and_no_host_capability_fallback')
        phase['mode'] = 'write'
        pal = command('pal', [*opts, *selection], 'create', 'SandboxPal')
        pal_id = pal['id']
        command('pal', [*opts, *selection], 'ask', pal_id, 'write')
        phase['mode'] = 'read'
        command('pal', [*opts, *selection], 'ask', pal_id, 'read')
        assert not (Path(pal['workspace']) / 'retained.txt').exists()
        checks.append('pal_sandbox_retained_private_guest_workspace')
    finally:
        if exec_id:
            command('capabilities', ['--state-dir', str(root / 'state')], 'cleanup', exec_id)
        if pal_id:
            command('pal', opts, 'computer', pal_id, 'cleanup')
        server.close()
    checks.append('owned_exec_and_pal_sandbox_cleanup')
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--image')
    args = parser.parse_args()
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-capabilities-accept-') as directory:
        root = Path(directory)
        host = root / 'host'
        host.mkdir()
        checks += host_accept(host)
        for pal in (False, True):
            for allow in (False, True):
                case = root / f'pty-{pal}-{allow}'
                case.mkdir()
                checks.append(pty_accept(case, pal, allow))
        if args.image:
            sandbox = root / 'sandbox'
            sandbox.mkdir()
            checks += sandbox_accept(sandbox, args.image)
    report = {'passed': len(checks), 'checks': checks, 'docker_tested': bool(args.image), 'live_model_calls': 0}
    (ROOT / 'artifacts/capabilities-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
