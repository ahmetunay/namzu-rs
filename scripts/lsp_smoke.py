#!/usr/bin/env python3
"""5.3 release CLI + independent stdio LSP oracle; zero model calls."""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'
FIXTURE = ROOT / 'crates/lsp/tests/fixtures/server.py'
SOURCE = 'let value = "🌍";\nvalue;\n'


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-lsp-acceptance-') as directory:
        root = Path(directory)
        source = root / 'code.rs'
        source.write_text(SOURCE)
        marker = root / 'marker'
        env = dict(os.environ)
        env['NAMZU_LSP_ENV_SECRET_FIXTURE'] = 'fixture-value-must-not-be-inherited'
        def args(mode, timeout=20):
            return [str(BINARY), 'lsp', '--root', str(root), '--timeout', str(timeout), '--program', '/usr/bin/python3',
                    '--server-arg', str(FIXTURE), '--server-arg=--mode', f'--server-arg={mode}', '--server-arg=--marker', f'--server-arg={marker}']
        def run(mode, action, success=True):
            p = subprocess.run([*args(mode), *action], env=env, capture_output=True, timeout=25)
            assert (p.returncode == 0) == success, (p.stdout.decode(), p.stderr.decode())
            value = json.loads(p.stdout)
            if success:
                assert value['output']['close']['exit_code'] == 0
                assert not value['output']['close']['forced']
                return value['output']['result']
            return value
        at = ['code.rs', '--line', '1', '--character', '0']
        for mode in ('normal', 'fragment', 'pull', 'stale', 'server_requests'):
            result = run(mode, ['diagnostics', 'code.rs'])
            assert result['diagnostics'][0]['message'] == 'Untrusted fixture diagnostic'
        assert run('link', ['definition', *at])['locations'][0]['range']['start'] == {'line': 0, 'character': 4}
        assert len(run('normal', ['references', *at])['locations']) == 2
        assert source.read_text() == SOURCE
        checks.append('push_pull_fragmented_diagnostics_definition_references_and_rejected_server_edit')
        preview = run('normal', ['rename', *at, '--new-name', 'renamed'])
        assert preview['preview'] == SOURCE.replace('value', 'renamed') and preview['applied'] is False
        assert source.read_text() == SOURCE
        (root / 'plan.json').write_text(json.dumps(preview['plan']))
        checks.append('rename_preview_has_no_write')
        offline = [str(BINARY), 'lsp', '--root', str(root), 'apply', 'plan.json']
        source.chmod(0o750)
        for flag, success in [(False, False), (True, True), (True, False)]:
            p = subprocess.run([*offline, *(['--allow-write'] if flag else [])], env=env, capture_output=True, timeout=5)
            assert (p.returncode == 0) == success, p.stdout.decode()
        assert source.read_text() == SOURCE.replace('value', 'renamed')
        assert source.stat().st_mode & 0o777 == 0o750
        assert not list(root.glob('.namzu-lsp-*'))
        checks.append('offline_apply_permission_hash_and_no_replay')
        source.write_text(SOURCE)
        applied = run('normal', ['rename', *at, '--new-name', 'changed', '--apply'])
        assert applied['applied']['previous_sha256'] == applied['plan']['expected_sha256']
        assert source.read_text() == SOURCE.replace('value', 'changed')
        checks.append('explicit_live_apply_updates_document_and_preserves_unicode')
        source.write_text(SOURCE)
        run('outside', ['definition', *at], success=False)
        for mode in ('multi', 'version'):
            run(mode, ['rename', *at, '--new-name', 'bad', '--apply'], success=False)
            assert source.read_text() == SOURCE
        checks.append('external_locations_multi_file_and_stale_version_are_rejected')
        marker.unlink(missing_ok=True)
        run('descendant', ['definition', 'code.rs', '--line', '0', '--character', '14'], success=False)
        assert not marker.exists(), 'invalid UTF-16 position must be rejected before launch'
        checks.append('surrogate_preflight_prevents_process_spawn')
        for cancel in (True, False):
            marker.unlink(missing_ok=True)
            p = subprocess.Popen([*args('stall', 20 if cancel else 1), 'definition', *at], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            end = time.monotonic() + 5
            while not marker.exists():
                assert p.poll() is None, p.communicate()
                assert time.monotonic() < end, 'LSP request handshake did not arrive'
                time.sleep(0.005)
            pid = json.loads(marker.read_text())['pid']
            if cancel: p.send_signal(signal.SIGINT)
            stdout, stderr = p.communicate(timeout=5)
            assert p.returncode != 0, (stdout, stderr)
            assert ('cancelled' if cancel else 'deadline') in json.loads(stdout)['error']
            assert not Path(f'/proc/{pid}').exists(), 'LSP leader was not reaped'
            assert source.read_text() == SOURCE
            checks.append('cli_sigint_kills_and_reaps' if cancel else 'cli_deadline_kills_and_reaps')
    report = {'checks': checks, 'live_model_calls': 0, 'language_server': 'independent local toy LSP; not rust-analyzer'}
    (ROOT / 'artifacts/lsp53-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
