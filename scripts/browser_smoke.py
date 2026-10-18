#!/usr/bin/env python3
"""Actual Chromium SDK/CLI acceptance with a guest-local HTTP oracle; no model calls."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
from pals_smoke import command, options


def main():
    root = Path(__file__).resolve().parents[1]
    cargo = shutil.which('cargo') or '/home/agent/.cargo/bin/cargo'
    subprocess.run([cargo, 'build', '--release', '--workspace', '--bins', '--examples', '--locked'], cwd=root, check=True)
    manifest = json.loads((root / 'artifacts/browser-image.json').read_text())
    image = manifest['image']
    for name, digest in manifest['binaries'].items():
        assert hashlib.sha256((root / 'target/release' / name).read_bytes()).hexdigest() == digest, 'rebuild browser image for the current release binaries'
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'):
        env.pop(key, None)
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    def run(*args, check=True):
        return subprocess.run([*docker, *args], env=env, check=check, capture_output=True, text=True, timeout=60)
    assert run('image', 'inspect', '--format', '{{.Id}}', 'namzu-browser:local').stdout.strip() == image
    with tempfile.TemporaryDirectory(prefix='namzu-browser-oracle-image-') as context:
        shutil.copy2(root / 'target/release/examples/browser_fixture', Path(context, 'browser-fixture'))
        Path(context, 'Dockerfile').write_text('FROM namzu-browser:local\nCOPY --chmod=0555 browser-fixture /usr/local/bin/browser-fixture\n')
        run('build', '--network=none', '--pull=false', '-t', 'namzu-browser-oracle:local', context)
    fixture_image = run('image', 'inspect', '--format', '{{.Id}}', 'namzu-browser-oracle:local').stdout.strip()
    with tempfile.TemporaryDirectory(prefix='namzu-browser-sdk-') as directory:
        subprocess.run([str(root / 'target/release/examples/browser_smoke'), directory, fixture_image], env=env, check=True, timeout=300)
        for name in ('browser-screen.png', 'browser-snapshot.json'):
            shutil.copy2(Path(directory, name), root / 'artifacts' / name)
    binary = root / 'target/release/namzu-rs'
    with tempfile.TemporaryDirectory(prefix='namzu-browser-cli-') as directory:
        work = Path(directory)
        opts = options(work, SimpleNamespace(server_port=1))
        ident = command(binary, opts, '--guest', 'create', 'Browser CLI')['id']
        record = None
        try:
            record = command(binary, opts, '--image', fixture_image, 'computer', ident, 'create', '--desktop', '--browser', '--width', '800', '--height', '600')
            request = work / 'request.json'
            request.write_text(json.dumps({'action': 'info'}))
            assert command(binary, opts, 'computer', ident, 'browser', str(request))['page']['url'] == 'about:blank'
            run('exec', '--detach', record['container'], '/usr/local/bin/browser-fixture')
            request.write_text(json.dumps({'action': 'navigate', 'url': 'http://127.0.0.1:8765/'}))
            command(binary, opts, 'computer', ident, 'browser', str(request), success=False)
            state = command(binary, opts, 'computer', ident, 'show')
            assert state['phase'] == 'running' and state['pending'] is None
            page = command(binary, opts, '--allow-navigate', 'computer', ident, 'browser', str(request))['page']
            assert page['title'] == 'Namzu Browser Fixture'
            request.write_text(json.dumps({'action': 'snapshot'}))
            snapshot = command(binary, opts, 'computer', ident, 'browser', str(request))
            assert any(n['role'] == 'button' and n['name'] == 'Run task' for n in snapshot['accessibility'])
            assert any(n['text'] == 'Kontrol paneli' for n in snapshot['dom'])
            request.write_text(json.dumps({'action': 'navigate', 'url': 'javascript:alert(1)'}))
            command(binary, opts, '--allow-navigate', 'computer', ident, 'browser', str(request), success=False)
            assert command(binary, opts, 'computer', ident, 'show')['phase'] == 'running'
        finally:
            command(binary, opts, 'computer', ident, 'cleanup')
        if record:
            assert run('container', 'inspect', record['container'], check=False).returncode != 0
            assert run('volume', 'inspect', record['volume'], check=False).returncode != 0
    report = {'chromium_sdk_navigation_dom_ax_screen_fence': 'passed', 'cli_navigation_permission_and_inspection': 'passed', 'owned_cleanup': 'passed', 'image': image, 'live_model_calls': 0}
    (root / 'artifacts/browser42-docker-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
