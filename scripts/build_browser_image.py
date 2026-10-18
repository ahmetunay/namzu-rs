#!/usr/bin/env python3
"""Explicit browser image build with managed proxy and build-only CA mount."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    subprocess.run([shutil.which('cargo') or '/home/agent/.cargo/bin/cargo', 'build', '--release', '--locked', '--workspace', '--bins', '--examples'], cwd=root, check=True)
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'): env.pop(key, None)
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    with tempfile.TemporaryDirectory(prefix='namzu-browser-image-') as directory:
        for name in ('namzu-guest', 'namzu-desktop', 'namzu-browser'): shutil.copy2(root / 'target/release' / name, Path(directory, name))
        binaries = {name: hashlib.sha256(Path(directory, name).read_bytes()).hexdigest()
                    for name in ('namzu-guest', 'namzu-desktop', 'namzu-browser')}
        shutil.copy2(root / 'containers/pal/Browser.Dockerfile', Path(directory, 'Dockerfile'))
        subprocess.run([*docker, 'build', '--pull=false', '--secret', 'id=system_ca,src=/etc/ssl/certs/ca-certificates.crt', '-t', 'namzu-browser:local', directory], env=env, check=True)
    image = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-browser:local'], env=env, text=True).strip()
    out = root / 'artifacts/browser-image.json'
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({'image': image, 'binaries': binaries}) + '\n')
    print(json.dumps({'image': image, 'binaries': binaries}))


if __name__ == '__main__':
    main()
