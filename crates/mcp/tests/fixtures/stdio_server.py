#!/usr/bin/env python3
"""Independent stdlib-only MCP fixture. Production transport is native Rust."""
import json
import os
from pathlib import Path
import signal
import sys

mode = sys.argv[1]
Path('leader.pid').write_text(str(os.getpid()))
if mode == 'ignore':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)


def emit(value):
    sys.stdout.buffer.write(json.dumps(value, ensure_ascii=False).encode() + b'\n')
    sys.stdout.buffer.flush()


def reply(request, result):
    emit({'jsonrpc': '2.0', 'id': request['id'], 'result': result})


def child():
    pid = os.fork()
    if pid == 0:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        while True:
            signal.pause()
    return pid


if mode == 'stderr_flood':
    os.write(2, b'FIXTURE_PRIVATE_LOG' * 500)
if mode == 'stdout_noise':
    os.write(1, b'FIXTURE_PRIVATE_NOISE\n')
if mode == 'oversize':
    os.write(1, b'x' * 100000)
    while True:
        signal.pause()
if mode == 'truncated':
    os.write(1, b'{"jsonrpc":"2.0"')
    sys.exit(0)

pending = []
for line in sys.stdin.buffer:
    request = json.loads(line)
    method = request.get('method')
    if method == 'initialize':
        if mode == 'init_stall':
            Path('seen').write_text('initialize')
            while True:
                signal.pause()
        result = {'protocolVersion': '2099-01-01' if mode == 'version' else '2025-11-25',
                  'capabilities': {'tools': {}}, 'serverInfo': {'name': 'python-fixture', 'version': '1'}}
        reply(request, result)
    elif method == 'notifications/initialized':
        Path('initialized').write_text('yes')
        if mode == 'partial_read':
            prefix = os.read(0, 64)
            emit({'jsonrpc': '2.0', 'method': 'notifications/partial_read', 'params': {'bytes': len(prefix)}})
            while True:
                signal.pause()
        if mode == 'duplex':
            emit({'jsonrpc': '2.0', 'method': 'notifications/progress', 'params': {'padding': 'x' * 100000}})
            os.write(2, b'L' * 200000)
    elif method == 'notifications/cancelled':
        Path('cancelled').write_text('yes')
    elif method in ('tools/call', 'ping', 'tools/list'):
        Path('effects').write_text(Path('effects').read_text() + '1' if Path('effects').exists() else '1')
        if mode in ('wait', 'ignore', 'early_exit'):
            pid = child()
            emit({'jsonrpc': '2.0', 'method': 'notifications/fixture_started', 'params': {'pid': pid}})
            if mode == 'early_exit':
                sys.exit(0)
            while True:
                signal.pause()
        if mode == 'stdout_flood':
            for _ in range(1000):
                emit({'jsonrpc': '2.0', 'method': 'notifications/progress', 'params': {'padding': 'x' * 1024}})
        if mode == 'bad_id':
            emit({'jsonrpc': '2.0', 'id': 'unknown', 'result': {}})
        elif mode == 'tool_error':
            reply(request, {'content': [{'type': 'text', 'text': 'fixture failure'}], 'isError': True})
        elif mode == 'rpc_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32602, 'message': 'FIXTURE_PRIVATE_LOG'}})
        elif mode == 'pipeline':
            pending.append(request)
            if len(pending) == 2:
                for item in reversed(pending):
                    reply(item, {'marker': item['params']['marker']})
                pending.clear()
        elif mode == 'exit_reply':
            reply(request, {'answer': 42})
            sys.exit(0)
        elif mode == 'environment':
            reply(request, {'cwd': os.getcwd(), 'explicit': os.environ.get('MCP_FIXTURE_VALUE'),
                            'forbidden_present': any(k in os.environ for k in ('OPENAI_API_KEY', 'HOME', 'PYTHONPATH', 'LD_PRELOAD')),
                            'literal_args': sys.argv[2:]})
        elif mode == 'server_request':
            emit({'jsonrpc': '2.0', 'id': 99, 'method': 'ping'})
            response = json.loads(next(sys.stdin.buffer))
            assert response['id'] == 99 and response['result'] == {}
            emit({'jsonrpc': '2.0', 'id': 100, 'method': 'roots/list'})
            response = json.loads(next(sys.stdin.buffer))
            assert response['id'] == 100 and response['error']['code'] == -32601
            reply(request, {'answered_server_requests': True})
        elif mode == 'duplex':
            reply(request, {'bytes': len(request['params']['padding'])})
        else:
            reply(request, {'answer': 42})
if mode == 'ignore':
    while True:
        signal.pause()
Path('graceful').write_text('yes')
