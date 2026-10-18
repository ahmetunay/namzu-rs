#!/usr/bin/env python3
"""Real CLI recovery + watch, with local peers and model fixture only."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from tui_smoke import Fixture, answer, tools


def run(binary):
    root_source = Path(__file__).resolve().parents[1]
    peer = root_source / 'crates/mcp/tests/fixtures/recovery_server.py'
    with tempfile.TemporaryDirectory(prefix='namzu-recovery-cli-') as directory:
        root = Path(directory)
        state = root / 'state'
        env = {**os.environ, 'NAMZU_RECOVERY_MODEL_FIXTURE_KEY': 'fixture-only'}
        spec = root / 'server.json'
        spec.write_text(json.dumps({'transport': {'type': 'stdio', 'program': '/usr/bin/python3', 'args': [str(peer)], 'cwd': str(root)}, 'policy': {'require_approval': True}}))
        base = [str(binary), 'mcp', '--state-dir', str(state)]
        def command(*args, success=True):
            p = subprocess.run([*base, *args], capture_output=True, env=env, timeout=15)
            assert (p.returncode == 0) == success, (p.stdout.decode(), p.stderr.decode())
            assert 'PRIVATE_FIXTURE_PAYLOAD' not in p.stdout.decode() + p.stderr.decode()
            return json.loads(p.stdout)
        checks = []
        command('add', 'fixture', '--spec', str(spec))
        command('reconnect', 'fixture', success=False)
        command('--deny', 'connection/reconnect', 'reconnect', 'fixture', '--approve', success=False)
        data = command('reconnect', 'fixture', '--approve')['output']
        assert data['generation'] > data['previous_generation'] and not data['replayed']
        checks.append('explicit_reconnect_admission_and_generation')
        (root / 'mode').write_text('normal')
        watch = command('watch', 'fixture', 'mem://fixture/data', '--approve', '--count', '1', '--timeout', '5')['output']
        assert watch['updates'] == ['mem://fixture/data'] and watch['subscriptions'] == 0
        requests = [json.loads(line) for line in (root / 'requests.jsonl').read_text().splitlines()]
        assert sum(r['method'] == 'resources/unsubscribe' for r in requests) == 1
        checks.append('watch_subscribe_update_unsubscribe_close')
        (root / 'mode').write_text('stall_events')
        command('watch', 'fixture', 'mem://fixture/data', '--approve', '--timeout', '1', success=False)
        pid = int((root / 'peer-pid').read_text())
        try: os.kill(pid, 0)
        except ProcessLookupError: pass
        else: raise AssertionError('timed out watch left its peer alive')
        checks.append('watch_deadline_cleanup')
        (root / 'mode').write_text('drop_tool')
        def callback(body, count):
            assert count == 0, 'uncertain tool must not cause a second provider call'
            name = next(t['function']['name'] for t in body['tools'] if '_effect_' in t['function']['name'])
            return tools([('effect-one', name, {'secret': 'PRIVATE_FIXTURE_PAYLOAD'})])
        model = Fixture(callback)
        model.start()
        args = [str(binary), 'exec', 'fixture effect', '--cwd', str(root), '--state-dir', str(state), '--model', 'fixture', '--base-url', f'http://127.0.0.1:{model.server_port}/v1', '--api-key-env', 'NAMZU_RECOVERY_MODEL_FIXTURE_KEY', '--mcp-server', 'fixture', '--mcp-approve', '--timeout', '10', '--json']
        try:
            first = subprocess.run(args, capture_output=True, env=env, timeout=15)
            assert first.returncode != 0
            assert (root / 'effects').read_text().splitlines() == ['once']
            records = command('recovery', 'list')['output']['records']
            assert len(records) == 1 and records[0]['uncertain'] and not records[0]['active']
            blob = next((state / 'mcp/recovery/records').glob('*.json')).read_text()
            assert 'PRIVATE_FIXTURE_PAYLOAD' not in blob
            second = subprocess.run(args, capture_output=True, env=env, timeout=15)
            assert second.returncode != 0 and 'mcp recovery list' in second.stdout.decode()
            assert len(model.requests) == 1
            checks.append('durable_fence_across_real_cli_processes')
            p = subprocess.run([*base, 'recovery', 'acknowledge', records[0]['id']], capture_output=True, timeout=5)
            assert p.returncode != 0
            command('recovery', 'acknowledge', records[0]['id'], '--inspected')
            assert not command('recovery', 'list')['output']['records']
            assert (root / 'effects').read_text().splitlines() == ['once']
            (root / 'mode').write_text('normal')
            command('reconnect', 'fixture', '--approve')
            assert (root / 'effects').read_text().splitlines() == ['once']
            checks.append('explicit_inspection_acknowledges_without_replay')
        finally:
            model.close()
        return {'checks': checks, 'live_model_calls': 0, 'fixture_model_requests': 1}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=Path('target/release/namzu-rs'))
    args = parser.parse_args()
    print(json.dumps(run(args.binary.resolve())))
