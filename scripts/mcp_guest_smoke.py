#!/usr/bin/env python3
"""Build an explicit local offline fixture image, then test owned guest MCP."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from pals_smoke import command, options
from tui_smoke import Fixture, answer, tools


def main():
    root = Path(__file__).resolve().parents[1]
    cargo = shutil.which('cargo') or '/home/agent/.cargo/bin/cargo'
    subprocess.run([cargo, 'build', '--release', '--workspace', '--examples', '--bins', '--locked'], cwd=root, check=True)
    env = dict(os.environ)
    for key in ('DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH'):
        env.pop(key, None)
    env['NAMZU_MCP_ENV_FIXTURE'] = 'tcp://unreachable.fixture.invalid:2375'
    docker = [shutil.which('docker') or '/usr/local/bin/docker', '--host=unix:///var/run/docker.sock']
    # Proxy/CA variables are inherited. Build has no network and never pulls.
    with tempfile.TemporaryDirectory(prefix='namzu-mcp-image-') as directory:
        context = Path(directory)
        shutil.copy2(root / 'target/release/namzu-guest', context / 'namzu-guest')
        shutil.copy2(root / 'target/release/examples/guest_server', context / 'mcp-fixture')
        dockerfile = (root / 'containers/pal/Dockerfile').read_text()
        dockerfile = dockerfile.replace('USER 65532:65532', 'COPY --chmod=0555 mcp-fixture /usr/local/bin/mcp-fixture\nUSER 65532:65532')
        (context / 'Dockerfile').write_text(dockerfile)
        subprocess.run([*docker, 'build', '--network=none', '--pull=false', '-t', 'namzu-mcp-fixture:local', directory], env=env, check=True)
    image = subprocess.check_output([*docker, 'image', 'inspect', '--format', '{{.Id}}', 'namzu-mcp-fixture:local'], env=env, text=True).strip()
    with tempfile.TemporaryDirectory(prefix='namzu-mcp-computer-') as directory:
        subprocess.run([str(root / 'target/release/examples/mcp_smoke'), directory, image], env=env, check=True)
    # Exercise the CLI Pal adapter too, with a loopback model and a sole alias.
    binary = root / 'target/release/namzu-rs'
    with tempfile.TemporaryDirectory(prefix='namzu-mcp-pal-') as directory:
        work = Path(directory)
        spec = work / 'server.json'
        definition = {'transport': {'type': 'stdio', 'program': str(root / 'target/release/examples/guest_server'), 'args': [], 'cwd': str(work)}, 'policy': {'require_approval': True}}
        spec.write_text(json.dumps(definition))
        base = [str(binary), 'mcp', '--state-dir', str(work / 'state')]
        subprocess.run([*base, 'add', 'fixture', '--spec', str(spec)], check=True, capture_output=True)
        aliases = json.loads(subprocess.check_output([*base, 'discover', 'fixture', 'tools', '--approve', '--agent-names'], text=True))
        name = aliases['output']['items'][0]['alias']
        definition['transport'].update(program='/usr/local/bin/mcp-fixture', cwd='/workspace', env={'DOCKER_HOST': 'NAMZU_MCP_ENV_FIXTURE'})
        spec.write_text(json.dumps(definition))
        subprocess.run([*base, 'update', 'fixture', '--spec', str(spec)], check=True, capture_output=True)
        def callback(body, count):
            assert [t['function']['name'] for t in body['tools']] == [name]
            if count == 0: return tools([('guest-one', name, {})])
            message = next(m for m in body['messages'] if m['role'] == 'tool')
            assert json.loads(json.loads(message['content'])['content'])['structuredContent']['answer'] == 42
            return answer('GUEST_MCP_DONE')
        server = Fixture(callback)
        opts = [*options(work, server), '--mcp-server', 'fixture', '--tool-allow', name, '--mcp-approve']
        os.environ['NAMZU_MCP_ENV_FIXTURE'] = env['NAMZU_MCP_ENV_FIXTURE']
        pal = command(binary, opts, '--guest', 'create', 'Guest MCP')
        server.start()
        try:
            result = command(binary, opts, '--image', image, 'ask', pal['id'], 'run MCP')
            assert result['output'] == 'GUEST_MCP_DONE'
            snapshot = command(binary, opts, 'computer', pal['id'], 'show')
            assert snapshot['phase'] == 'closed' and snapshot['pending'] is None
            assert not (Path(pal['workspace']) / 'mcp-result').exists()
        finally:
            try:
                command(binary, opts, 'computer', pal['id'], 'cleanup')
            finally:
                server.close()
    print(json.dumps({'owned_guest_stdio': 'passed', 'uncertain_resume': 'passed', 'pal_cli_guest': 'passed', 'image': image, 'live_model_calls': 0}))


if __name__ == '__main__':
    main()
