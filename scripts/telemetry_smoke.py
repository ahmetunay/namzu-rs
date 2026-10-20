#!/usr/bin/env python3
"""7.1 actual release CLI and PTY telemetry acceptance. Loopback/fake keys only."""
import json
from pathlib import Path
import subprocess
import tempfile
import time
from provider_contract_smoke import Fixture
from tui_smoke import Terminal, Fixture as StallingFixture, answer, tools
from pals_smoke import environment

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


def options(root, server):
    return ['--state-dir', str(root / 'state'), '--model', 'SECRET_MODEL', '--base-url',
            f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY',
            '--timeout', '20']


def command(kind, opts, *args, ok=True):
    p = subprocess.run([str(BINARY), kind, *opts, *args], env=environment(), capture_output=True, timeout=30)
    assert (p.returncode == 0) == ok, (p.stdout.decode(), p.stderr.decode())
    return json.loads(p.stdout)


def traces(root):
    return [json.loads(p.read_text()) for p in (root / 'state/telemetry').glob('t-*.json') if p.is_file()]


def clean(trace):
    raw = json.dumps(trace)
    for text in ('SECRET_', 'fixture-only-not-a-real-key', 'credential_tag', 'https://private', 'png_base64'):
        assert text not in raw, text
    assert trace['schema_version'] == 1
    assert len(trace['records']) <= 1024
    assert [r['sequence'] for r in trace['records']] == list(range(len(trace['records'])))
    assert all(a['at_ms'] <= b['at_ms'] for a, b in zip(trace['records'], trace['records'][1:]))
    assert all(r['parent'] == 1 for r in trace['records'] if r['span'] != 1)
    assert len(raw.encode()) <= 1024 * 1024


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-telemetry-') as directory:
        root = Path(directory)
        for enabled in (False, True):
            case = root / ('enabled' if enabled else 'disabled')
            case.mkdir()
            def callback(body, index):
                if enabled and index < 2:
                    pending = traces(case)
                    assert pending and pending[-1]['outcome'] == {'status': 'running'}
                if index == 0:
                    v = tools([('SECRET_CALL_ID', 'write_file', {'path': 'SECRET_PATH', 'content': 'SECRET_ARGUMENT'})])
                    v['choices'][0]['message']['reasoning_content'] = 'SECRET_PRIVATE_REASONING'
                    v['usage'] = {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}
                    return v
                return answer('SECRET_OUTPUT')
            server = Fixture(callback)
            server.start()
            try:
                opts = [*options(case, server), '--cwd', str(case), '--allow-write']
                if enabled: opts += ['--telemetry']
                result = command('exec', opts, 'SECRET_PROMPT', '--json')
                assert (case / 'SECRET_PATH').read_text() == 'SECRET_ARGUMENT'
                assert result['output'] == 'SECRET_OUTPUT'
                if not enabled:
                    assert result['telemetry_id'] is None and not (case / 'state/telemetry').exists()
                    empty = command('telemetry', ['--state-dir', str(case / 'state')], 'list')
                    assert not empty['traces'] and len(server.requests) == 2
                    checks.append('default_disabled_no_trace_directory_or_extra_http')
                else:
                    trace = traces(case)[0]
                    clean(trace)
                    assert trace['trace_id'] == result['telemetry_id']
                    assert trace['outcome'] == {'status': 'ok'} and trace['source'] == 'exec'
                    m = trace['metrics']
                    assert m['provider_started'] == m['provider_completed'] == 2
                    assert m['tools_completed'] == 1 and m['tool_errors'] == 0
                    assert m['usage']['input_tokens'] == {'sum': 3, 'samples': 1}
                    assert m['usage']['responses'] == 2 and m['usage']['reported_cost_nano_usd'] is None
                    path = case / 'state/telemetry' / (result['telemetry_id'] + '.json')
                    assert path.stat().st_mode & 0o777 == 0o600
                    assert path.parent.stat().st_mode & 0o777 == 0o700
                    checks.append('enabled_pending_before_http_numeric_trace_private_file_and_independent_write_oracle')
                    listed = command('telemetry', ['--state-dir', str(case / 'state')], 'list')
                    shown = command('telemetry', ['--state-dir', str(case / 'state')], 'show', result['telemetry_id'])
                    assert shown['trace'] == trace and listed['traces'][0]['trace_id'] == result['telemetry_id']
                    assert shown['offline'] and len(server.requests) == 2
                    checks.append('offline_list_and_show_do_not_discover_models_or_read_auth')
                    before = len(traces(case))
                    resumed = command('exec', [*[v for v in opts if v != '--telemetry'], '--telemetry=false', '--resume', result['session_id']], 'SECRET_CONTINUE', '--json')
                    assert resumed['telemetry_id'] is None and len(traces(case)) == before
                    resumed = command('exec', [*opts, '--resume', result['session_id']], 'SECRET_CONTINUE', '--json')
                    assert resumed['telemetry_id'] != result['telemetry_id'] and len(traces(case)) == before + 1
                    checks.append('telemetry_toggling_does_not_change_session_route_and_resume_gets_fresh_trace')
            finally: server.close()

        case = root / 'config'
        case.mkdir()
        server = Fixture(lambda body, index: answer('SECRET_OUTPUT'))
        server.start()
        try:
            config = case / 'config.json'
            config.write_text(json.dumps({'telemetry': True}))
            opts = [*options(case, server), '--cwd', str(case), '--config', str(config)]
            assert command('exec', opts, 'SECRET_PROMPT', '--json')['telemetry_id']
            assert command('exec', [*opts, '--telemetry=false'], 'SECRET_PROMPT', '--json')['telemetry_id'] is None
            assert len(traces(case)) == 1
            checks.append('config_opt_in_and_explicit_false_override')
        finally: server.close()

        case = root / 'failed'
        case.mkdir()
        server = Fixture(lambda body, index: (429, {'error': 'SECRET_ERROR https://private'}, {'Retry-After': '7'}))
        server.start()
        try:
            result = command('exec', [*options(case, server), '--cwd', str(case), '--telemetry'], 'SECRET_PROMPT', '--json', ok=False)
            trace = traces(case)[0]
            clean(trace)
            assert trace['outcome'] == {'status': 'failed', 'failure': {'kind': 'remote', 'class': 'rate_limited', 'http_status': 429}}
            assert trace['metrics']['provider_started'] == 1 and trace['metrics']['provider_completed'] == 0
            assert trace['trace_id'] == result['telemetry_id'] and len(server.requests) == 1
            checks.append('typed_http_failure_is_redacted_and_has_no_retry_or_success_metadata')
        finally: server.close()

        case = root / 'unsafe'
        case.mkdir()
        (case / 'state').mkdir(mode=0o700)
        outside = case / 'outside'
        outside.mkdir(mode=0o700)
        (case / 'state/telemetry').symlink_to(outside, target_is_directory=True)
        server = Fixture(lambda body, index: answer('unexpected'))
        server.start()
        try:
            command('exec', [*options(case, server), '--cwd', str(case), '--telemetry'], 'SECRET_PROMPT', '--json', ok=False)
            assert not server.requests and not list(outside.iterdir())
            checks.append('unsafe_trace_directory_fails_before_provider_or_tool_effects')
        finally: server.close()

        case = root / 'crash'
        case.mkdir()
        server = StallingFixture(lambda body, index: None)
        server.start()
        process = subprocess.Popen([str(BINARY), 'exec', *options(case, server), '--cwd', str(case), '--telemetry', 'SECRET_PROMPT', '--json'], env=environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 10
            while not server.stalled and time.monotonic() < deadline: time.sleep(0.01)
            assert server.stalled
            process.kill()
            process.communicate(timeout=10)
            assert traces(case)[0]['outcome'] == {'status': 'running'}
            clean(traces(case)[0])
            checks.append('process_death_retains_pending_trace_without_fake_completion')
        finally:
            if process.poll() is None: process.kill(); process.communicate()
            server.close()

        for pal in (False, True):
            for decision in ('allow', 'deny'):
                case = root / f'pty-{pal}-{decision}'
                case.mkdir()
                def callback(body, index):
                    if index == 0:
                        return tools([('SECRET_CALL_ID', 'write_file', {'path': 'SECRET_PATH', 'content': 'SECRET_ARGUMENT'})])
                    return answer('TELEMETRY_DONE')
                server = StallingFixture(callback)
                opts = [*options(case, server), '--allow-write', '--telemetry']
                work = case
                if pal:
                    created = command('pal', opts, 'create', 'SECRET_PAL')
                    work = Path(created['result']['workspace'])
                    term = Terminal(BINARY, [*opts, 'tui', created['result']['id']], case, command=['pal'])
                else:
                    term = Terminal(BINARY, [*opts, '--cwd', str(case)], case)
                server.start()
                try:
                    term.until(lambda: term.contains('Enter a message'))
                    term.send(b'SECRET_PROMPT\r')
                    term.until(lambda: term.contains('Allow once?'))
                    term.send(b'y' if decision == 'allow' else b'n')
                    term.until(lambda: term.contains('TELEMETRY_DONE') and 'Ready' in term.screen.text())
                    trace = traces(case)[0]
                    clean(trace)
                    assert trace['outcome'] == {'status': 'ok'}
                    assert trace['source'] == ('pal' if pal else 'tui')
                    m = trace['metrics']
                    assert m['approvals_allowed'] == (decision == 'allow')
                    assert m['approvals_denied'] == (decision == 'deny')
                    assert m['tool_errors'] == (decision == 'deny')
                    assert (work / 'SECRET_PATH').exists() == (decision == 'allow')
                    term.quit()
                    checks.append(f'{"pal" if pal else "normal"}_pty_{decision}_forwards_approval_records_safe_metrics_and_preserves_terminal')
                finally: term.close(); server.close()

        case = root / 'pty-cancel'
        case.mkdir()
        server = StallingFixture(lambda body, index: None)
        term = Terminal(BINARY, [*options(case, server), '--cwd', str(case), '--telemetry'], case)
        server.start()
        try:
            term.until(lambda: term.contains('Enter a message'))
            term.send(b'SECRET_PROMPT\r')
            term.until(lambda: server.stalled)
            term.send(b'\x03')
            term.until(lambda: traces(case) and traces(case)[0]['outcome'] == {'status': 'failed', 'failure': {'kind': 'cancelled'}})
            assert server.client_closed.wait(3)
            clean(traces(case)[0])
            term.quit()
            checks.append('pty_cancel_interrupts_http_and_publishes_typed_cancelled_trace')
        finally: term.close(); server.close()
    print(json.dumps({'passed': len(checks), 'checks': checks, 'live_model_calls': 0}))


if __name__ == '__main__': main()
