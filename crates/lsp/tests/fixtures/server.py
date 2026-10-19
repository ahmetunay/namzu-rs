#!/usr/bin/env python3
"""Independent toy LSP oracle. Standard framing, UTF-16 positions, no model calls."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--mode', default='normal')
parser.add_argument('--marker')
a = parser.parse_args()
doc = None


def send(value):
    data = json.dumps(value, ensure_ascii=False).encode()
    frame = f'Content-Length: {len(data)}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n'.encode() + data
    if a.mode == 'fragment':
        for i in range(0, len(frame), 3):
            sys.stdout.buffer.write(frame[i:i+3])
            sys.stdout.buffer.flush()
    else:
        sys.stdout.buffer.write(frame)
        sys.stdout.buffer.flush()


def receive():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line == b'\r\n': break
        name, value = line.decode().split(':', 1)
        headers[name.lower()] = value.strip()
    size = int(headers['content-length'])
    assert 0 < size <= 1024 * 1024
    return json.loads(sys.stdin.buffer.read(size))


def response(ident, value): send({'jsonrpc': '2.0', 'id': ident, 'result': value})

def diagnostics(version=None):
    item = {'range': {'start': {'line': 0, 'character': 4}, 'end': {'line': 0, 'character': 9}},
            'severity': 2, 'code': 'fixture', 'source': 'toy-lsp', 'message': 'STALE diagnostic' if version is not None and version != doc['version'] else 'Untrusted fixture diagnostic'}
    return {'uri': doc['uri'], 'version': doc['version'] if version is None else version, 'diagnostics': [item]}


while True:
    message = receive()
    if message is None: break
    method = message.get('method')
    params = message.get('params') or {}
    ident = message.get('id')
    if method == 'initialize':
        assert params['capabilities']['general']['positionEncodings'] == ['utf-16']
        assert params['capabilities']['workspace']['applyEdit'] is False
        assert os.environ.get('NAMZU_LSP_ENV_SECRET_FIXTURE') is None
        if a.mode == 'stderr':
            sys.stderr.write('private stderr content' * 100000)
            sys.stderr.flush()
        if a.mode == 'partial':
            sys.stdout.buffer.write(b'Content-Length: 100\r\n\r\n{')
            sys.stdout.buffer.flush()
            break
        if a.mode == 'oversize':
            sys.stdout.buffer.write(b'Content-Length: 1048577\r\n\r\n')
            sys.stdout.buffer.flush()
            continue
        if a.mode == 'descendant':
            child = subprocess.Popen(['/bin/sleep', '120'])
            Path(a.marker).write_text(str(child.pid))
        if a.mode == 'server_requests':
            send({'jsonrpc': '2.0', 'id': 'cfg', 'method': 'workspace/configuration', 'params': {'items': [{}]}})
            assert receive()['result'] == [None]
            send({'jsonrpc': '2.0', 'id': 'edit', 'method': 'workspace/applyEdit', 'params': {'edit': {'changes': {}}}})
            assert receive()['result']['applied'] is False
        capabilities = {'positionEncoding': 'utf-8' if a.mode == 'utf8' else 'utf-16', 'textDocumentSync': {'openClose': True, 'change': 2},
                        'definitionProvider': True, 'referencesProvider': True, 'renameProvider': True}
        if a.mode == 'pull': capabilities['diagnosticProvider'] = {'interFileDependencies': False, 'workspaceDiagnostics': False}
        response(ident + 1 if a.mode == 'wrong_id' else ident, {'capabilities': capabilities})
    elif method == 'textDocument/didOpen':
        doc = params['textDocument']
        if a.mode != 'pull':
            if a.mode == 'stale': send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': diagnostics(0)})
            send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': diagnostics()})
    elif method == 'textDocument/didChange':
        assert params['textDocument']['version'] == doc['version'] + 1
        doc['text'] = params['contentChanges'][0]['text']
        doc['version'] = params['textDocument']['version']
        if a.mode == 'stale': send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': diagnostics(doc['version'] - 1)})
        send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': diagnostics()})
    elif method == 'textDocument/diagnostic':
        response(ident, {'kind': 'full', 'items': diagnostics()['diagnostics']})
    elif method in ('textDocument/definition', 'textDocument/references'):
        if a.mode == 'stall':
            Path(a.marker).write_text(json.dumps({'pid': os.getpid()}))
            continue
        uri = 'file:///tmp/outside-lsp-root.rs' if a.mode == 'outside' else doc['uri']
        target = {'uri': uri, 'range': {'start': {'line': 0, 'character': 4}, 'end': {'line': 0, 'character': 9}}}
        if a.mode == 'link':
            response(ident, [{'targetUri': uri, 'targetRange': target['range'], 'targetSelectionRange': target['range']}])
        elif method.endswith('/references'):
            response(ident, [target, {'uri': uri, 'range': {'start': {'line': 1, 'character': 0}, 'end': {'line': 1, 'character': 5}}}])
        else: response(ident, target)
    elif method == 'textDocument/rename':
        changes = [{'range': {'start': {'line': 0, 'character': 4}, 'end': {'line': 0, 'character': 9}}, 'newText': params['newName']},
                   {'range': {'start': {'line': 1, 'character': 0}, 'end': {'line': 1, 'character': 5}}, 'newText': params['newName']}]
        if a.mode == 'multi': response(ident, {'changes': {doc['uri']: changes, 'file:///tmp/other.rs': changes}})
        elif a.mode == 'version': response(ident, {'documentChanges': [{'textDocument': {'uri': doc['uri'], 'version': 999}, 'edits': changes}]})
        else: response(ident, {'changes': {doc['uri']: changes}})
    elif method == 'shutdown': response(ident, None)
    elif method == 'exit': break
