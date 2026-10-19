#!/usr/bin/env python3
"""4.4 real owned Chromium, HTTP provider wire and PTY approval acceptance.

Deterministic fixture selects calls; this tests integration, not model reasoning.
Requires scripts/build_browser_image.py and a release CLI; uses no account keys.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
from pals_smoke import command, options, environment, tasks, docker_command
from tui_smoke import Terminal, Fixture, tools, answer

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'
DOCKER = shutil.which('docker') or '/usr/local/bin/docker'


def docker(*args, check=True):
    return docker_command(DOCKER, *args, check=check)


def result(body, ident):
    message = next(m for m in reversed(body['messages']) if m.get('tool_call_id') == ident)
    value = json.loads(message['content'])
    assert not value['is_error'], value
    return json.loads(value['content'])


def target(snapshot, ident):
    node = next(n for n in snapshot['dom'] if ['id', ident] in n['attributes'])
    return {'document': {k: snapshot['page'][k] for k in ('frame_id', 'loader_id', 'url')},
            'backend_node_id': node['backend_node_id']}


def visual(body):
    parts = [p for m in body['messages'] if m['role'] == 'user' and isinstance(m['content'], list) for p in m['content']]
    data = next(p['image_url']['url'] for p in parts if p['type'] == 'image_url')
    assert data.startswith('data:image/png;base64,')
    png = base64.b64decode(data.split(',', 1)[1], validate=True)
    assert png[:8] == b'\x89PNG\r\n\x1a\n'
    assert int.from_bytes(png[16:20], 'big') == 1024
    assert int.from_bytes(png[20:24], 'big') == 768
    assert any('Untrusted screenshot' in p.get('text', '') for p in parts)
    assert all(data not in m.get('content', '') for m in body['messages'] if m['role'] == 'tool')


def setup_guest(record):
    docker('exec', '--detach', record['container'], '/usr/local/bin/browser-fixture')
    deadline = time.monotonic() + 10
    while docker('exec', record['container'], '/bin/test', '-f', '/workspace/browser-ready', check=False).returncode:
        assert time.monotonic() < deadline, 'guest HTTP fixture readiness'


def absent(record):
    assert docker('container', 'inspect', record['container'], check=False).returncode
    assert docker('volume', 'inspect', record['volume'], check=False).returncode


def exec_case(image, pty=False, deny=False, cancel=False):
    with tempfile.TemporaryDirectory(prefix='namzu-visual-agent-') as directory:
        root = Path(directory)
        record = None
        def callback(body, index):
            nonlocal record
            names = {t['function']['name'] for t in body['tools']}
            assert {'computer_screenshot', 'browser_action', 'browser_snapshot'} <= names
            assert not {'shell', 'computer_input', 'browser_upload', 'browser_download'} & names
            if index == 0:
                records = list((root / 'state' / 'exec-computers').glob('*/computer.json'))
                assert len(records) == 1
                record = json.loads(records[0].read_text())
                setup_guest(record)
                return tools([('nav', 'browser_navigate', {'url': 'http://127.0.0.1:8765/actions'})])
            if index == 1:
                return tools([('snapshot', 'browser_snapshot', {})])
            if index == 2:
                snapshot = result(body, 'snapshot')
                return tools([('fill', 'browser_action', {'request': {'action': 'fill', 'target': target(snapshot, 'edit'), 'text': 'Rust Türkçe ✓'}})])
            if index == 3:
                snapshot = result(body, 'snapshot')
                return tools([('click', 'browser_action', {'request': {'action': 'click', 'target': target(snapshot, 'run')}})])
            if index == 4:
                return tools([('after', 'browser_snapshot', {}), ('screen', 'computer_screenshot', {})])
            assert index == 5
            assert any(n['text'] == 'clicked:Rust Türkçe ✓' for n in result(body, 'after')['dom'])
            visual(body)
            return answer('VISUAL_AGENT_OK')
        # PTY uses read-only image before one input approval; no navigation needed.
        if pty:
            def callback(body, index):
                nonlocal record
                if index == 0:
                    record = json.loads(next((root / 'state' / 'exec-computers').glob('*/computer.json')).read_text())
                    return tools([('screen', 'computer_screenshot', {})])
                if index == 1:
                    visual(body)
                    return tools([('input', 'computer_input', {'request': {'action': 'move', 'x': 41, 'y': 57}})])
                assert index == 2
                item = next(m for m in reversed(body['messages']) if m.get('tool_call_id') == 'input')
                assert json.loads(item['content'])['is_error'] == deny
                return tools([('position', 'computer_info', {})])
        if pty:
            previous = callback
            def callback(body, index):
                if index <= 2:
                    return previous(body, index)
                assert index == 3
                info = result(body, 'position')
                if not deny:
                    assert (info['pointer_x'], info['pointer_y']) == (41, 57), info
                else:
                    assert (info['pointer_x'], info['pointer_y']) != (41, 57), info
                return answer('VISUAL_PTY_OK')
        server = Fixture(callback)
        args = [*options(root, server), '--computer-image', image, '--computer-browser', '--visual-read']
        if pty: args += ['--visual-input']
        else: args += ['--visual-navigate', '--visual-actions']
        args[args.index('--timeout')+1] = '120'
        if not pty:
            server.start()
            try:
                p = subprocess.run([str(BINARY), 'exec', 'fixture visual task', *args, '--json'], env=environment(), capture_output=True, timeout=180)
                assert p.returncode == 0, (p.stdout.decode(), p.stderr.decode())
                assert 'VISUAL_AGENT_OK' in p.stdout.decode()
            finally: server.close()
        else:
            terminal = Terminal(BINARY, args, root)
            server.start()
            try:
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'look\r')
                terminal.until(lambda: 'Allow once?' in terminal.screen.text(), timeout=60)
                terminal.send(b'\x1b[18~')  # F7
                terminal.until(lambda: 'Screenshot' in terminal.screen.text() and 'approval pending' in terminal.screen.text())
                assert '1024×768' in terminal.screen.text()
                terminal.send(b'y')
                # Hidden approval cannot be answered while viewing the image.
                for _ in range(10): terminal.pump()
                assert len(server.requests) == 2
                if cancel:
                    terminal.send(b'\x03')
                    terminal.until(lambda: any(json.loads(p.read_text())['status'] == 'failed' for p in (root / 'state').glob('s-*.json')))
                else:
                    terminal.send(b'\x1b[18~')
                    terminal.until(lambda: 'Allow once?' in terminal.screen.text())
                    terminal.send(b'n' if deny else b'y')
                    terminal.until(lambda: terminal.contains('VISUAL_PTY_OK') and 'Ready' in terminal.screen.text(), timeout=60)
                terminal.send(b'\x1b')
                terminal.wait_exit()
            finally:
                terminal.close()
                server.close()
        assert record
        absent(record)


def uncertain_case(image):
    with tempfile.TemporaryDirectory(prefix='namzu-visual-fence-') as directory:
        root = Path(directory)
        record = None
        def callback(body, index):
            nonlocal record
            if index == 0:
                record = json.loads(next((root / 'state' / 'exec-computers').glob('*/computer.json')).read_text())
                setup_guest(record)
                return tools([('nav', 'browser_navigate', {'url': 'http://127.0.0.1:8765/actions'})])
            if index == 1:
                return tools([('snapshot', 'browser_snapshot', {})])
            if index == 2:
                stale = target(result(body, 'snapshot'), 'run')
                stale['document']['loader_id'] = 'stale-loader'
                return tools([('stale', 'browser_action', {'request': {'action': 'click', 'target': stale}})])
            assert index == 3
            last = body['messages'][-1]
            assert json.loads(last['content'])['is_error']
            return answer('UNCERTAIN_NOT_REPLAYED')
        server = Fixture(callback)
        opts = [*options(root, server), '--computer-image', image, '--computer-browser', '--visual-read', '--visual-navigate', '--visual-actions']
        opts[opts.index('--timeout')+1] = '120'
        server.start()
        def session(*args):
            p = subprocess.run([str(BINARY), 'session', '--state-dir', str(root / 'state'), 'computer', ident, *args], capture_output=True, env=environment(), timeout=45)
            assert p.returncode == 0, p.stderr.decode()
            return json.loads(p.stdout)
        try:
            p = subprocess.run([str(BINARY), 'exec', 'stale target fixture', *opts, '--json'], capture_output=True, env=environment(), timeout=180)
            assert p.returncode == 0, p.stderr.decode()
            saved = json.loads(next((root / 'state').glob('s-*.json')).read_text())
            ident = saved['id']
            state = session('show')
            assert state['phase'] == 'stopped' and state['pending']['kind'] == 'browser_action'
            assert docker('container', 'inspect', '--format', '{{.State.Running}}', record['container']).stdout.strip() == b'false'
            assert base64.b64decode(session('read', 'browser-ready')['base64'])
            assert session('show')['pending'] == state['pending'], 'operator read must not acknowledge effects'
            # Same history cannot silently resume with the host backend.
            p = subprocess.run([str(BINARY), 'exec', 'resume fixture', *options(root, server), '--resume', ident, '--json'], capture_output=True, env=environment(), timeout=45)
            assert p.returncode != 0 and len(server.requests) == 4
        finally:
            server.close()
            if record:
                session('cleanup')
                absent(record)


def pal_case(image):
    with tempfile.TemporaryDirectory(prefix='namzu-visual-pal-') as directory:
        root = Path(directory)
        pages = []
        def callback(body, index):
            names = {t['function']['name'] for t in body['tools']}
            assert 'computer_screenshot' in names
            assert not {'computer_input', 'browser_action', 'browser_upload', 'browser_download'} & names
            if index in (0, 2):
                return tools([(f'info{index}', 'browser_info', {}), (f'screen{index}', 'computer_screenshot', {})])
            assert index in (1, 3)
            pages.append(result(body, f'info{index-1}')['page'])
            visual(body)
            return answer('PAL_VISUAL_OK')
        server = Fixture(callback)
        opts = options(root, server)
        opts[opts.index('--timeout')+1] = '120'
        ident = command(BINARY, opts, '--guest', '--visual-read', 'create', 'Visual Pal')['id']
        record = command(BINARY, opts, '--image', image, 'computer', ident, 'create', '--desktop', '--browser')
        server.start()
        try:
            first = command(BINARY, opts, '--visual-read', 'ask', ident, 'look')
            assert command(BINARY, opts, 'computer', ident, 'show')['phase'] == 'running'
            command(BINARY, opts, '--visual-read', '--visual-actions', 'ask', ident, 'look again', '--follow', tasks(root, ident)[0]['id'])
            assert pages[0] == pages[1], 'healthy persistent visual turn must retain browser document'
        finally:
            server.close()
            command(BINARY, opts, 'computer', ident, 'cleanup')
        absent(record)


def main():
    manifest = json.loads((ROOT / 'artifacts/browser-image.json').read_text())
    assert docker('image', 'inspect', '--format', '{{.Id}}', 'namzu-browser:local').stdout.decode().strip() == manifest['image']
    for name, digest in manifest['binaries'].items():
        assert hashlib.sha256((ROOT / 'target/release' / name).read_bytes()).hexdigest() == digest, 'rebuild the browser image for current release binaries'
    with tempfile.TemporaryDirectory(prefix='namzu-visual-oracle-image-') as directory:
        shutil.copy2(ROOT / 'target/release/examples/browser_fixture', Path(directory, 'browser-fixture'))
        Path(directory, 'Dockerfile').write_text('FROM namzu-browser:local\nCOPY --chmod=0555 browser-fixture /usr/local/bin/browser-fixture\n')
        docker('build', '--network=none', '--pull=false', '-t', 'namzu-browser-oracle:local', directory)
    image = docker('image', 'inspect', '--format', '{{.Id}}', 'namzu-browser-oracle:local').stdout.decode().strip()
    checks = []
    for name, kwargs in [('exec_dom_and_image', {}), ('pty_allow_preview', {'pty': True}), ('pty_deny_preview', {'pty': True, 'deny': True}), ('pty_cancel_preview', {'pty': True, 'cancel': True})]:
        exec_case(image, **kwargs)
        checks.append(name)
        print(name, 'passed', flush=True)
    uncertain_case(image)
    checks.append('uncertain_stopped_inspection_cleanup_and_route_pin')
    pal_case(image)
    checks.append('pal_image_ceiling_and_browser_continuity')
    report = {'checks': checks, 'image': image, 'live_model_calls': 0}
    (ROOT / 'artifacts/visual44-acceptance.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
