#!/usr/bin/env python3
"""Real Docker SDK/CLI desktop acceptance; native GUI oracle, no model calls."""
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
    image = json.loads((root / 'artifacts/desktop-image.json').read_text())['image']
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'): env.pop(key, None)
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    actual = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-desktop:local'], env=env, text=True).strip()
    assert actual == image, 'desktop tag changed from pinned build manifest'
    with tempfile.TemporaryDirectory(prefix='namzu-desktop-oracle-image-') as context:
        shutil.copy2(root / 'target/release/examples/desktop_fixture', Path(context, 'desktop-fixture'))
        Path(context, 'Dockerfile').write_text('FROM namzu-desktop:local\nCOPY --chmod=0555 desktop-fixture /usr/local/bin/desktop-fixture\n')
        subprocess.run([*docker, 'build', '--network=none', '--pull=false', '-t', 'namzu-desktop-oracle:local', context], env=env, check=True)
    fixture_image = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-desktop-oracle:local'], env=env, text=True).strip()
    with tempfile.TemporaryDirectory(prefix='namzu-desktop-sdk-') as directory:
        subprocess.run([str(root / 'target/release/examples/desktop_smoke'), directory, fixture_image], env=env, check=True)
        for name in ('screen-before.png', 'screen-after.png'): shutil.copy2(Path(directory, name), root / 'artifacts' / name)
    binary = root / 'target/release/namzu-rs'
    with tempfile.TemporaryDirectory(prefix='namzu-desktop-cli-') as directory:
        work = Path(directory)
        opts = options(work, SimpleNamespace(server_port=1))
        pal = command(binary, opts, '--guest', 'create', 'Desktop CLI')
        ident = pal['id']
        try:
            command(binary, opts, '--image', image, 'computer', ident, 'create', '--desktop', '--width', '600', '--height', '400')
            request = work / 'request.json'
            request.write_text(json.dumps({'action': 'info'}))
            info = command(binary, opts, 'computer', ident, 'desktop', str(request))
            assert info['width'] == 600 and info['height'] == 400
            before = command(binary, opts, 'computer', ident, 'show')
            request.write_text(json.dumps({'action': 'move', 'x': 150, 'y': 120}))
            command(binary, opts, 'computer', ident, 'desktop', str(request), success=False)
            after = command(binary, opts, 'computer', ident, 'show')
            assert after['phase'] == before['phase'] == 'running' and after['pending'] is None
            command(binary, opts, '--allow-input', 'computer', ident, 'desktop', str(request))
            request.write_text(json.dumps({'action': 'info'}))
            info = command(binary, opts, 'computer', ident, 'desktop', str(request))
            assert (info['pointer_x'], info['pointer_y']) == (150, 120)
            request.write_text(json.dumps({'action': 'screenshot'}))
            target = work / 'screen.png'
            command(binary, opts, 'computer', ident, 'desktop', str(request), '--output', str(target))
            assert target.read_bytes().startswith(b'\x89PNG\r\n\x1a\n') and target.stat().st_mode & 0o777 == 0o600
            command(binary, opts, 'computer', ident, 'desktop', str(request), '--output', str(target), success=False)
        finally:
            command(binary, opts, 'computer', ident, 'cleanup')
    print(json.dumps({'native_gui_pixels_and_input': 'passed', 'cli_permission_screen_and_png': 'passed', 'image': image, 'live_model_calls': 0}))


if __name__ == '__main__':
    main()
