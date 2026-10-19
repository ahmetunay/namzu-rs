#!/usr/bin/env python3
"""Actual Chromium page-action and file SDK/CLI acceptance with a guest-local HTTP oracle; no model calls."""
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
    with tempfile.TemporaryDirectory(prefix='namzu-browser-actions-sdk-') as directory:
        subprocess.run([str(root / 'target/release/examples/browser_actions_smoke'), directory, fixture_image], env=env, check=True, timeout=300)
        shutil.copy2(Path(directory, 'browser43-downloaded.bin'), root / 'artifacts/browser43-downloaded.bin')
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
            def call(payload, *flags, success=True):
                request.write_text(json.dumps(payload))
                return command(binary, opts, *flags, 'computer', ident, 'browser', str(request), success=success)
            call({'action': 'navigate', 'url': 'http://127.0.0.1:8765/actions'}, '--allow-navigate')
            snapshot = call({'action': 'snapshot'})
            document = {k: snapshot['page'][k] for k in ('frame_id', 'loader_id', 'url')}
            def target(ident):
                node = next(n for n in snapshot['dom'] if ['id', ident] in n['attributes'])
                return {'document': document, 'backend_node_id': node['backend_node_id']}
            fill = {'action': 'fill', 'target': target('edit'), 'text': 'CLI Türkçe ✓'}
            call(fill, '--allow-navigate', success=False)
            assert command(binary, opts, 'computer', ident, 'show')['phase'] == 'running'
            call(fill, '--allow-browser-actions')
            call({'action': 'click', 'target': target('run')}, '--allow-browser-actions')
            assert any(n['text'] == 'clicked:CLI Türkçe ✓' for n in call({'action': 'snapshot'})['dom'])
            payload = bytes([0, 255, 10, 13, 42])
            local = work / 'payload.bin'
            local.write_bytes(payload)
            command(binary, opts, '--allow-write', 'computer', ident, 'upload', 'guest-input.bin', str(local))
            upload = {'action': 'upload', 'target': target('file'), 'path': 'guest-input.bin'}
            call(upload, '--allow-browser-actions', success=False)
            call(upload, '--allow-upload')
            call({'action': 'click', 'target': target('send')}, '--allow-browser-actions')
            download = {'action': 'download', 'document': document, 'url': 'http://127.0.0.1:8765/download', 'path': 'guest-output.bin'}
            call(download, '--allow-upload', success=False)
            call(download, '--allow-download')
            output = work / 'export.bin'
            command(binary, opts, 'computer', ident, 'download', 'guest-output.bin', str(output))
            assert output.read_bytes() == payload and output.stat().st_mode & 0o777 == 0o600
            after = call({'action': 'info'})['page']
            assert all(after[k] == document[k] for k in document)
            assert command(binary, opts, 'computer', ident, 'show')['phase'] == 'running'
            command(binary, opts, 'computer', ident, 'download', 'guest-output.bin', str(output), success=False)
            assert output.read_bytes() == payload
        finally:
            command(binary, opts, 'computer', ident, 'cleanup')
        if record:
            assert run('container', 'inspect', record['container'], check=False).returncode != 0
            assert run('volume', 'inspect', record['volume'], check=False).returncode != 0
    report = {'browser42_regression': 'passed', 'page_actions_unicode_upload_download_and_cancel': 'passed', 'cli_separate_permissions_and_host_file_roundtrip': 'passed', 'owned_cleanup': 'passed', 'image': image, 'live_model_calls': 0}
    (root / 'artifacts/browser43-docker-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
