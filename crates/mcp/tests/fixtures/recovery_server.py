#!/usr/bin/env python3
"""Independent stdio recovery/subscription peer. All effects are fixture files."""
import json
from pathlib import Path
import os
import signal
import sys

subscribed = set()
Path('peer-pid').write_text(str(os.getpid()))
def emit(value):
    print(json.dumps(value), flush=True)
def result(request, value):
    emit({'jsonrpc': '2.0', 'id': request['id'], 'result': value})
def update(uri):
    emit({'jsonrpc': '2.0', 'method': 'notifications/resources/updated', 'params': {'uri': uri}})
for line in sys.stdin:
    request = json.loads(line)
    with Path('requests.jsonl').open('a') as out: out.write(json.dumps(request) + '\n')
    mode = Path('mode').read_text().strip() if Path('mode').exists() else 'normal'
    method = request['method']
    if method == 'initialize':
        result(request, {'protocolVersion': '2025-06-18' if mode == 'legacy' else '2025-11-25', 'capabilities': {'tools': {}, 'resources': {'subscribe': mode != 'no_subscribe', 'listChanged': True}}, 'serverInfo': {'name': 'independent-recovery-peer', 'version': '1'}})
    elif method == 'notifications/initialized': pass
    elif method == 'tools/list':
        result(request, {'tools': [{'name': 'effect', 'inputSchema': {'type': 'object'}}]})
    elif method == 'resources/list':
        result(request, {'resources': [{'name': 'Data', 'uri': 'mem://fixture/data'}]})
    elif method == 'tools/call':
        if mode == 'rpc_tool_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32603, 'message': 'DO_NOT_PRINT_REMOTE_SECRET'}})
            continue
        with Path('effects').open('a') as out: out.write('once\n')
        Path('started').write_text('ready')
        if mode == 'drop_tool': sys.exit(0)
        if mode == 'block_tool':
            while True: signal.pause()
        result(request, {'content': [{'type': 'text', 'text': 'done'}]})
    elif method == 'resources/subscribe':
        if mode == 'rpc_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32603, 'message': 'DO_NOT_PRINT_REMOTE_SECRET'}})
        else:
            subscribed.add(request['params']['uri'])
            result(request, {})
            if mode != 'stall_events': update(request['params']['uri'])
    elif method == 'resources/unsubscribe':
        if mode == 'rpc_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32603, 'message': 'DO_NOT_PRINT_REMOTE_SECRET'}})
        else:
            subscribed.discard(request['params']['uri'])
            result(request, {})
    elif method == 'resources/read':
        result(request, {'contents': [{'uri': request['params']['uri'], 'text': 'data'}]})
    elif method == 'ping':
        if mode == 'drop_ping' and not Path('ping_dropped').exists():
            Path('ping_dropped').write_text('once')
            sys.exit(0)
        if mode == 'updates':
            for uri in subscribed:
                update(uri); update(uri)
            update('file:///DO_NOT_OPEN')
        if mode == 'malformed_update':
            emit({'jsonrpc': '2.0', 'method': 'notifications/resources/updated', 'params': {'uri': 42}})
        result(request, {})
    else:
        emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'unsupported'}})
Path('peer-closed').write_text('closed')
