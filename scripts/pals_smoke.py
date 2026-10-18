#!/usr/bin/env python3
"""Pals 2.6 acceptance: real PTYs and optional owned Docker via CLI; no model quota."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
from tui_smoke import Terminal, Fixture, answer, tools


def environment():
    env = dict(os.environ)
    env['NAMZU_TUI_FIXTURE_KEY'] = 'fixture-only-not-a-real-key'
    for key in ('NO_PROXY', 'no_proxy'):
        env[key] = ','.join(filter(None, [env.get(key), '127.0.0.1', 'localhost']))
    return env


def options(root, server):
    return ['--state-dir', str(root / 'state'), '--model', 'fixture', '--base-url',
            f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY', '--timeout', '20']


def command(binary, opts, *args, success=True):
    p = subprocess.run([str(binary), 'pal', *opts, *args], capture_output=True, env=environment(), timeout=40)
    assert (p.returncode == 0) == success, (p.stdout.decode(), p.stderr.decode())
    response = json.loads(p.stdout)
    return response['result'] if success else response


def tasks(root, pal):
    return json.loads((root / 'state' / 'pals' / f'{pal}.json').read_text())['tasks']


def pty_case(binary, kind):
    with tempfile.TemporaryDirectory(prefix='namzu-pal-pty-') as directory:
        root = Path(directory)
        def callback(body, count):
            if kind == 'cancel':
                return None
            if kind.startswith('approval') and count == 0:
                assert any(t['function']['name'] == 'write_file' for t in body['tools'])
                return tools([('pal-write', 'write_file', {'path': 'answer', 'content': '42\n'})])
            if kind == 'follow' and count == 1:
                assert any(m.get('content') == 'PAL_FIRST' for m in body['messages'])
                assert body['messages'][-1]['content'] == 'continue saved'
            return answer('PAL_FIRST' if kind == 'follow' and count == 0 else 'PAL_FINAL')
        server = Fixture(callback)
        opts = options(root, server)
        pal = command(binary, opts, '--allow-write', 'create', 'PTY Pal')
        pal_id = pal['id']
        terminal = Terminal(binary, [*opts, '--allow-write', 'tui', pal_id], root, command=['pal'])
        server.start()
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'first request\r')
            if kind == 'cancel':
                terminal.until(lambda: server.stalled)
                terminal.send(b'\x03')
                terminal.until(lambda: tasks(root, pal_id) and tasks(root, pal_id)[0]['status'] == 'cancelled')
                assert server.client_closed.wait(3)
            elif kind.startswith('approval'):
                terminal.until(lambda: terminal.contains('Allow once?'))
                terminal.send(b'y' if kind == 'approval_allow' else b'n')
                terminal.until(lambda: tasks(root, pal_id) and tasks(root, pal_id)[0]['status'] == 'complete' and terminal.contains('PAL_FINAL') and 'Ready' in terminal.screen.text())
                path = Path(pal['workspace']) / 'answer'
                assert path.exists() == (kind == 'approval_allow')
                if path.exists(): assert path.read_text() == '42\n'
            else:
                terminal.until(lambda: tasks(root, pal_id) and tasks(root, pal_id)[0]['status'] == 'complete' and terminal.contains('PAL_FIRST') and 'Ready' in terminal.screen.text())
                first = tasks(root, pal_id)[0]
                saved = command(binary, opts, 'inspect', pal_id, first['id'])
                terminal.send(b'\x0e\x1b[15~')
                terminal.until(lambda: 'Sessions' in terminal.screen.text())
                terminal.send(b'\r')
                terminal.until(lambda: 'PAL_FIRST' in terminal.screen.text() and 'Sessions · private' not in terminal.screen.text())
                terminal.send(b'continue saved\r')
                terminal.until(lambda: len(tasks(root, pal_id)) == 2 and all(t['status'] == 'complete' for t in tasks(root, pal_id)) and terminal.contains('PAL_FINAL') and 'Ready' in terminal.screen.text())
                assert tasks(root, pal_id)[1]['follows'] == first['id']
                assert command(binary, opts, 'inspect', pal_id, first['id']) == saved
                terminal.send(b'\x1b[17~')
                terminal.until(lambda: 'Model and turn budgets' in terminal.screen.text())
                terminal.send(b'\x15other-model\r')
                terminal.until(lambda: 'pinned' in terminal.screen.text())
                terminal.send(b'\x1b')
                terminal.until(lambda: 'Model and turn budgets' not in terminal.screen.text())
                assert len(server.requests) == 2
            terminal.quit()
        finally:
            terminal.close()
            server.close()
        return {'case': kind, 'pty': True, 'model_calls': 0, 'fixture_requests': len(server.requests)}


def docker_command(docker, *args, check=True):
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'):
        env.pop(key, None)
    return subprocess.run([str(docker), '--host=unix:///var/run/docker.sock', *args], env=env, capture_output=True, check=check, timeout=20)


def docker_case(binary, docker, image):
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-pal-cli-docker-') as directory:
        root = Path(directory)
        def callback(body, _count):
            messages = body['messages']
            last = messages[-1]
            if last['role'] == 'user':
                prompt = last['content']
                if prompt == 'cancel guest':
                    return tools([('long-shell', 'shell', {'command': 'printf once > ready; sleep 120', 'timeout_seconds': 120})])
                if prompt == 'read retained':
                    return tools([('retained-read', 'read_file', {'path': 'answer'})])
                return tools([('guest-write', 'write_file', {'path': 'answer', 'content': '42\n'})])
            result = json.loads(last['content'])
            assert not result['is_error'], result
            if last['tool_call_id'] == 'guest-write':
                return tools([('guest-read', 'read_file', {'path': 'answer'})])
            assert result['content'] == '42\n', result
            return answer('PAL_GUEST_42')
        server = Fixture(callback)
        opts = [*options(root, server), '--docker', str(docker)]
        owned = []
        try:
            pal = command(binary, opts, '--guest', '--allow-write', '--allow-shell', 'create', 'persistent')
            ident = pal['id']; owned.append(ident)
            created = command(binary, opts, '--image', image, 'computer', ident, 'create')
            assert created['phase'] == 'stopped'
            command(binary, opts, 'computer', ident, 'start')
            assert command(binary, opts, 'computer', ident, 'show')['phase'] == 'running'
            command(binary, opts, 'computer', ident, 'restart')
            command(binary, opts, 'computer', ident, 'stop')
            checks.append('cli_create_start_restart_stop')
            payload = root / 'upload'; payload.write_bytes(bytes([0, 255, 10]))
            command(binary, opts, '--allow-write', 'computer', ident, 'upload', 'binary', str(payload))
            target = root / 'download'
            command(binary, opts, 'computer', ident, 'download', 'binary', str(target))
            assert target.read_bytes() == payload.read_bytes()
            checks.append('explicit_binary_transfer')
            server.start()
            command(binary, opts, 'computer', ident, 'start')
            result = command(binary, opts, '--allow-write', 'ask', ident, 'write answer')
            assert result['output'] == 'PAL_GUEST_42'
            assert not (Path(pal['workspace']) / 'answer').exists()
            snapshot = command(binary, opts, 'computer', ident, 'show')
            assert snapshot['phase'] == 'stopped' and snapshot['pending'] is None
            checks.append('pal_sdk_guest_tools_and_persistent_stop')
            command(binary, opts, 'send', ident, 'retained-mail', 'read retained')
            mail = command(binary, opts, 'work', ident, '--max-messages', '1')
            assert mail['deliveries'][0]['result']['output'] == 'PAL_GUEST_42'
            assert mail['deliveries'][0]['delivery']['state'] == 'acknowledged'
            assert command(binary, opts, 'work', ident, '--max-messages', '1')['deliveries'] == []
            checks.append('mailbox_scoped_guest_execution_ack_and_no_replay')
            terminal = Terminal(binary, [*opts, '--allow-write', 'tui', ident], root, command=['pal'])
            try:
                before = len(tasks(root, ident))
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'write answer\r')
                terminal.until(lambda: terminal.contains('Allow once?'))
                terminal.send(b'y')
                terminal.until(lambda: len(tasks(root, ident)) > before and tasks(root, ident)[-1]['status'] == 'complete' and terminal.contains('PAL_GUEST_42') and 'Ready' in terminal.screen.text())
                terminal.quit()
            finally:
                terminal.close()
            assert command(binary, opts, 'computer', ident, 'show')['phase'] == 'stopped'
            checks.append('real_guest_tui_approval_result_and_joined_cleanup')
            team = []
            for name in ('ephemeral-a', 'ephemeral-b'):
                p = command(binary, opts, '--guest', '--allow-write', 'create', name)
                team.append(p['id']); owned.append(p['id'])
            batch = command(binary, opts, '--image', image, '--allow-write', 'team', *team, '--prompt', 'write answer', '--concurrency', '2')
            assert all(x['output'] == 'PAL_GUEST_42' for x in batch['results'])
            for member in team:
                assert command(binary, opts, 'computer', member, 'show')['phase'] == 'closed'
            checks.append('parallel_guest_team_and_ephemeral_cleanup')
            process = subprocess.Popen([str(binary), 'pal', *opts, '--allow-shell', 'ask', ident, 'cancel guest'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment())
            try:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    record = json.loads((root / 'state' / 'pals' / ident / 'computers' / 'computer.json').read_text())
                    if record['pending'] and record['pending']['kind'] == 'shell':
                        ready = docker_command(docker, 'exec', record['container'], '/bin/cat', '/workspace/ready', check=False)
                        if ready.returncode == 0 and ready.stdout == b'once': break
                    time.sleep(.02)
                else: raise AssertionError('guest effect was never observed')
                process.send_signal(signal.SIGTERM)
                stdout, stderr = process.communicate(timeout=25)
                assert process.returncode == 1, (stdout, stderr)
                snapshot = command(binary, opts, 'computer', ident, 'show')
                assert snapshot['phase'] == 'stopped' and snapshot['pending']['kind'] == 'shell'
                assert tasks(root, ident)[-1]['status'] == 'cancelled'
                command(binary, opts, '--allow-write', 'ask', ident, 'refuse uncertain', success=False)
                checks.append('cli_cancellation_stops_remote_effect_and_refuses_replay')
                output = root / 'effect'
                command(binary, opts, 'computer', ident, 'download', 'ready', str(output))
                assert output.read_bytes() == b'once'
                command(binary, opts, 'computer', ident, 'acknowledge')
                assert command(binary, opts, 'computer', ident, 'show')['pending'] is None
                checks.append('explicit_inspection_and_acknowledgement_preserve_evidence')
            finally:
                if process.poll() is None:
                    process.kill(); process.communicate(timeout=5)
        finally:
            records = []
            for ident in owned:
                path = root / 'state' / 'pals' / ident / 'computers' / 'computer.json'
                if path.exists(): records.append(json.loads(path.read_text()))
                command(binary, opts, 'computer', ident, 'cleanup')
            for record in records:
                assert docker_command(docker, 'container', 'inspect', record['container'], check=False).returncode != 0
                assert docker_command(docker, 'volume', 'inspect', record['volume'], check=False).returncode != 0
            server.close()
        checks.append('all_exact_owned_containers_and_volumes_removed')
    return {'checks': checks, 'image': image, 'model_calls': 0}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/namzu-rs'))
    parser.add_argument('--image')
    parser.add_argument('--docker', type=Path, default=Path('/usr/local/bin/docker'))
    parser.add_argument('--out', type=Path, default=Path('artifacts/pals26-acceptance.json'))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    cases = [pty_case(binary, kind) for kind in ('follow', 'approval_allow', 'approval_deny', 'cancel')]
    report = {'format_version': 1, 'pty_cases': cases, 'live_model_calls': 0}
    if args.image:
        report['docker'] = docker_case(binary, args.docker.resolve(strict=True), args.image)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
