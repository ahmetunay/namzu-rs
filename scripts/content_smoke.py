#!/usr/bin/env python3
"""5.1 release CLI/Pal/PTY document + attachment acceptance, no live model calls."""
import base64
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib
from pals_smoke import command, options, environment, tasks
from tui_smoke import Fixture, Terminal, tools, answer

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


def png():
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b'\0\xff\0\0\0\xff\0\0\0\0\xff\xff\xff\xff')) + chunk(b'IEND', b''))


def prepare(root):
    (root / 'large.txt').write_text('Türkçe 🌍\n' * 20000, encoding='utf-8')
    (root / 'data.json').write_text(json.dumps({'rows': [{'n': n} for n in range(40)]}), encoding='utf-8')
    (root / 'records.jsonl').write_text('\n{"x":1}\ntrue\n', encoding='utf-8')
    (root / 'image.png').write_bytes(png())


def value(body, ident):
    result = json.loads(next(m['content'] for m in body['messages'] if m.get('tool_call_id') == ident))
    assert not result['is_error'], result
    return json.loads(result['content'])


def check_image(body):
    messages = body['messages']
    image = next(m for m in reversed(messages) if m['role'] == 'user' and isinstance(m['content'], list))
    assert 'Untrusted image' in image['content'][0]['text']
    url = image['content'][1]['image_url']['url']
    assert url.startswith('data:image/png;base64,')
    data = base64.b64decode(url.split(',', 1)[1], validate=True)
    assert data[:8] == b'\x89PNG\r\n\x1a\n'
    assert struct.unpack('>II', data[16:24]) == (2, 2)
    assert all(url not in m.get('content', '') for m in messages if m['role'] == 'tool')


def exec_case():
    with tempfile.TemporaryDirectory(prefix='namzu-content-exec-') as directory:
        root = Path(directory)
        prepare(root)
        def callback(body, index):
            names = {t['function']['name'] for t in body['tools']}
            assert {'inspect_file', 'read_file_chunk', 'read_document', 'read_image'} <= names
            assert not {'shell', 'write_file', 'apply_patch'} & names
            if index == 0:
                return tools([('info', 'inspect_file', {'path': 'large.txt'})])
            info = value(body, 'info')
            assert info['sha256'] == hashlib.sha256((root / 'large.txt').read_bytes()).hexdigest()
            if index == 1:
                return tools([('page', 'read_file_chunk', {'path': 'large.txt', 'offset': 0, 'max_bytes': 65535, 'expected_sha256': info['sha256']})])
            if index == 2:
                page = value(body, 'page')
                assert page['content'].encode() == (root / 'large.txt').read_bytes()[:page['next_offset']]
                assert not page['eof'] and 0 < page['next_offset'] <= 65535
                return tools([('doc', 'read_document', {'path': 'data.json', 'format': 'json', 'pointer': '/rows', 'start': 37, 'limit': 3}),
                              ('log', 'read_document', {'path': 'records.jsonl', 'format': 'jsonl', 'start': 1, 'limit': 1}),
                              ('image', 'read_image', {'path': 'image.png'})])
            assert index == 3
            assert value(body, 'doc')['value'] == [{'n': 37}, {'n': 38}, {'n': 39}]
            assert value(body, 'log')['value'] == [{'line': 3, 'value': True}]
            assert value(body, 'image')['file']['sha256'] == hashlib.sha256(png()).hexdigest()
            check_image(body)
            # The complete batch of tool outputs precedes user multipart image data.
            assert [m['role'] for m in body['messages'][-4:]] == ['tool', 'tool', 'tool', 'user']
            return answer('CONTENT_EXEC_OK')
        server = Fixture(callback)
        server.start()
        try:
            result = subprocess.run([str(BINARY), 'exec', 'inspect attachments', *options(root, server), '--cwd', str(root), '--json'], env=environment(), capture_output=True, timeout=45)
            assert result.returncode == 0, (result.stdout.decode(), result.stderr.decode())
            assert json.loads(result.stdout)['output'] == 'CONTENT_EXEC_OK'
            session = json.loads(next((root / 'state').glob('s-*.json')).read_text())
            assert any(m['role'] == 'tool_image' for m in session['messages'])
            assert session['status'] == 'complete'
        finally: server.close()


def pty_case(pal=False):
    with tempfile.TemporaryDirectory(prefix='namzu-content-pty-') as directory:
        root = Path(directory)
        prepare(root)
        def callback(body, index):
            if index == 0:
                return tools([('image', 'read_image', {'path': 'image.png'})])
            assert index == 1
            check_image(body)
            return answer('CONTENT_IMAGE_OK')
        server = Fixture(callback)
        opts = options(root, server)
        ident = None
        if pal:
            profile = command(BINARY, opts, 'create', 'Attachment Pal')
            ident = profile['id']
            Path(profile['workspace'], 'image.png').write_bytes(png())
            terminal = Terminal(BINARY, [*opts, 'tui', ident], root, command=['pal'])
        else:
            terminal = Terminal(BINARY, [*opts, '--cwd', str(root)], root)
        server.start()
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'look at file\r')
            terminal.until(lambda: terminal.contains('CONTENT_IMAGE_OK') and 'Ready' in terminal.screen.text())
            assert 'Allow once?' not in terminal.seen, 'read-only attachment should not require an effect grant'
            terminal.send(b'\x1b[18~')
            terminal.until(lambda: 'Image preview' in terminal.screen.text() and '2×2 pixels' in terminal.screen.text())
            assert '▀' in terminal.screen.text()
            terminal.send(b'\x1b')
            terminal.until(lambda: 'Image preview' not in terminal.screen.text())
            # Open the saved conversation in a fresh TUI; preview must come from history.
            terminal.quit()
            terminal.close()
            if pal:
                assert tasks(root, ident)[0]['status'] == 'complete'
                terminal = Terminal(BINARY, [*opts, 'tui', ident], root, command=['pal'])
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'\x1b[15~')
                terminal.until(lambda: 'Sessions' in terminal.screen.text())
                terminal.send(b'\r')
            else:
                session = json.loads(next((root / 'state').glob('s-*.json')).read_text())
                terminal = Terminal(BINARY, [*opts, '--cwd', str(root), '--resume', session['id']], root)
            terminal.until(lambda: terminal.contains('CONTENT_IMAGE_OK'))
            terminal.send(b'\x1b[18~')
            terminal.until(lambda: 'Image preview' in terminal.screen.text() and '2×2 pixels' in terminal.screen.text())
            assert len(server.requests) == 2, 'history viewing must not consume a provider call'
            terminal.send(b'\x1b')
            terminal.until(lambda: 'Image preview' not in terminal.screen.text())
            terminal.quit()
        finally:
            terminal.close()
            server.close()


def main():
    checks = []
    for name, run in [('exec_documents_and_image_wire', exec_case), ('tui_attachment_and_history', pty_case), ('pal_attachment_and_history', lambda: pty_case(True))]:
        run()
        checks.append(name)
        print(name, 'passed', flush=True)
    report = {'checks': checks, 'live_model_calls': 0}
    (ROOT / 'artifacts/content51-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()
