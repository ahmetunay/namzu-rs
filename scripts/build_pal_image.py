#!/usr/bin/env python3
"""Build the native guest bridge image explicitly; runtime never pulls/builds images."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    cargo = shutil.which('cargo') or '/home/agent/.cargo/bin/cargo'
    subprocess.run([cargo, 'build', '--release', '--locked', '-p', 'namzu-computer', '--bin', 'namzu-guest'], cwd=root, check=True)
    binary = root / 'target/release/namzu-guest'
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'):
        env.pop(key, None)
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    with tempfile.TemporaryDirectory(prefix='namzu-pal-image-') as context:
        shutil.copy2(binary, Path(context, 'namzu-guest'))
        shutil.copy2(root / 'containers/pal/Dockerfile', Path(context, 'Dockerfile'))
        subprocess.run([*docker, 'build', '--network=none', '--pull=false', '-t', 'namzu-pal-guest:local', context], env=env, check=True)
    image = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-pal-guest:local'], env=env, text=True).strip()
    result = {'image': image, 'guest_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'protocol': 1}
    output = root / 'artifacts/pal-image.json'
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
