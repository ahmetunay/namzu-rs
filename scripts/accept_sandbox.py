#!/usr/bin/env python3
"""Explicit headless Docker + release CLI acceptance; no models, network builds or secrets."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    subprocess.run(['python3', 'scripts/build_pal_image.py'], cwd=root, check=True)
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'):
        env.pop(key, None)
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    base = json.loads((root / 'artifacts/pal-image.json').read_text())['image']
    base_ref = 'namzu-sandbox-base:' + base.removeprefix('sha256:')
    subprocess.run([*docker, 'image', 'tag', base, base_ref], env=env, check=True)
    with tempfile.TemporaryDirectory(prefix='namzu-sandbox-accept-') as directory:
        temp = Path(directory)
        shutil.copy2(root / 'target/release/examples/sandbox_smoke', temp / 'sandbox-probe')
        (temp / 'seed.txt').write_bytes(b'sandbox seed\n')
        (temp / 'Dockerfile').write_text(
            f'FROM {base_ref}\n'
            'COPY --chmod=0555 sandbox-probe /usr/local/bin/sandbox-probe\n'
            'COPY --chown=65532:65532 --chmod=0400 seed.txt /workspace/seed.txt\n')
        subprocess.run([*docker, 'build', '--network=none', '--pull=false', '-t', 'namzu-sandbox-oracle:local', directory], env=env, check=True)
        image = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-sandbox-oracle:local'], env=env, text=True).strip()
        result = subprocess.check_output([str(root / 'target/release/examples/sandbox_smoke'), image], cwd=root, text=True)
        sdk = json.loads(result.strip().splitlines()[-1])
        cli = root / 'target/release/namzu-rs'
        checks = []

        def call(*args, ok=True):
            completed = subprocess.run([str(cli), 'sandbox', '--state-dir', str(temp / 'state'), *args], capture_output=True, text=True)
            data = json.loads(completed.stdout)
            assert (completed.returncode == 0) == ok, (args, data)
            assert data['status'] == ('ok' if ok else 'error'), data
            return data

        assert call('capabilities')['output']['host_fallback'] is False
        checks.append('capability_report')
        for name in ('inspect', 'readonly', 'build'):
            data = call('policy', '--profile', name)['output']
            policy = temp / f'{name}.json'
            policy.write_text(json.dumps(data['policy']))
            assert call('check', str(policy))['output']['sha256'] == data['sha256']
        checks.append('profile_check_fingerprint')
        selected = ['--policy', str(temp / 'inspect.json'), '--image', image]
        data = call('read', *selected, 'seed.txt')['output']
        assert base64.b64decode(data['result']['base64']) == b'sandbox seed\n'
        assert data['cleaned'] and data['computer']['phase'] == 'closed'
        assert call('show', data['sandbox_id'])['output']['computer']['phase'] == 'closed'
        checks.append('read_cleanup_show')
        call('run', *selected, 'touch denied', ok=False)
        checks.append('tools_only_command_denied')
        selected = ['--policy', str(temp / 'build.json'), '--image', image]
        data = call('run', *selected, 'printf cli > result; cat result')['output']
        assert data['result']['stdout'] == 'cli' and data['cleaned']
        checks.append('write_read_cleanup')
        data = call('run', *selected, 'exit 7', ok=False)['output']
        assert data['result']['exit_code'] == 7 and data['cleaned']
        checks.append('completed_nonzero_cleanup_exit_status')
        policy = json.loads((temp / 'build.json').read_text())
        policy['limits']['output_bytes'] = 16
        (temp / 'small.json').write_text(json.dumps(policy))
        data = call('run', '--policy', str(temp / 'small.json'), '--image', image,
                    'while :; do printf 0123456789; done', ok=False)['output']
        assert not data['cleaned'] and data['computer']['phase'] == 'stopped'
        assert data['computer']['pending']
        retained = call('read-retained', data['sandbox_id'], 'seed.txt')['output']
        assert base64.b64decode(retained['base64']) == b'sandbox seed\n'
        assert retained['computer']['pending'] and retained['computer']['phase'] == 'stopped'
        call('cleanup', data['sandbox_id'])
        assert call('show', data['sandbox_id'])['output']['computer']['phase'] == 'closed'
        checks.append('bounded_output_retained_recovery')
        # A native request must fail without executing the host-side marker command.
        marker = temp / 'host-marker'
        policy['backend'] = 'native'
        (temp / 'native.json').write_text(json.dumps(policy))
        call('run', '--policy', str(temp / 'native.json'), '--image', image, f'touch {marker}', ok=False)
        assert not marker.exists()
        checks.append('native_rejected_no_host_fallback')
    report = {'sdk': sdk, 'cli_checks': checks, 'cli_passed': len(checks), 'image': image,
              'guest_sha256': hashlib.sha256((root / 'target/release/namzu-guest').read_bytes()).hexdigest(),
              'model_calls': 0}
    (root / 'artifacts/sandbox-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
