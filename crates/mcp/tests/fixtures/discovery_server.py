#!/usr/bin/env python3
"""Independent deterministic MCP discovery peer, no accounts/model/network."""
import json
from pathlib import Path
import signal
import sys

mode = sys.argv[1]

def emit(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)

def changed(kind):
    emit({'jsonrpc': '2.0', 'method': 'notifications/' + kind + '/list_changed'})

def reply(request, result):
    emit({'jsonrpc': '2.0', 'id': request['id'], 'result': result})

def tool(name):
    return {'name': name, 'description': 'untrusted definition',
            'inputSchema': {'type': 'object', 'properties': {'path': {'type': 'string'}}, 'required': ['path']},
            'outputSchema': {'type': 'object', 'properties': {'answer': {'type': 'integer'}}, 'required': ['answer']}}

for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    with Path('requests.jsonl').open('a') as file:
        file.write(json.dumps(request) + '\n')
    if method == 'initialize':
        capabilities = {'tools': {'listChanged': mode != 'unannounced'},
                        'prompts': {'listChanged': True},
                        'resources': {'listChanged': True, 'subscribe': True}}
        if mode == 'no_tools':
            del capabilities['tools']
        reply(request, {'protocolVersion': '2025-11-25', 'capabilities': capabilities,
                        'serverInfo': {'name': 'discovery-fixture', 'version': '1'}})
    elif method == 'notifications/initialized':
        continue
    elif method == 'tools/list':
        cursor = request.get('params', {}).get('cursor')
        if mode == 'rpc_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32603, 'message': 'DO_NOT_LOG_REMOTE_SECRET'}})
        elif mode == 'large':
            page = int(cursor or 0)
            item = tool('tool_' + str(page)); item['padding'] = 'x' * 500000
            result = {'tools': [item]}
            if page < 3:
                result['nextCursor'] = str(page + 1)
            reply(request, result)
        elif mode == 'too_many_pages':
            page = int(cursor or 0)
            reply(request, {'tools': [tool('tool_' + str(page))], 'nextCursor': str(page + 1)})
        elif mode == 'bad_schema':
            item = tool('read'); item['inputSchema']['required'] = 'not-an-array'
            reply(request, {'tools': [item]})
        elif mode == 'remote_ref':
            item = tool('read'); item['inputSchema']['$ref'] = 'https://no-network.fixture.invalid/schema'
            reply(request, {'tools': [item]})
        elif mode == 'bad_name':
            reply(request, {'tools': [tool('space name')]})
        elif mode == 'malformed':
            reply(request, {'tools': 'not-an-array'})
        elif mode == 'huge':
            reply(request, {'tools': [tool('tool_' + str(i)) for i in range(1025)]})
        elif cursor is None:
            if mode == 'unrelated_change':
                changed('prompts')
            reply(request, {'tools': [tool('read')], 'nextCursor': 'page 2/+?=✓'})
        else:
            assert cursor == 'page 2/+?=✓'
            if mode == 'wait_page':
                Path('page_started').write_text('yes')
                while True:
                    signal.pause()
            if mode == 'change_during':
                changed('tools')
            if mode == 'duplicate_cursor':
                reply(request, {'tools': [tool('write')], 'nextCursor': cursor})
            elif mode == 'duplicate_item':
                reply(request, {'tools': [tool('read')]})
            else:
                reply(request, {'tools': [tool('write')]})
    elif method == 'resources/list':
        reply(request, {'resources': [{'uri': 'mem://resource/α', 'name': 'Readme', 'mimeType': 'text/plain'}]})
    elif method == 'resources/templates/list':
        reply(request, {'resourceTemplates': [{'uriTemplate': 'mem://{path}', 'name': 'Files'}]})
    elif method == 'prompts/list':
        reply(request, {'prompts': [{'name': 'review', 'arguments': [{'name': 'path', 'required': True}]}]})
    elif method == 'tools/call':
        if mode == 'agent_rpc_error':
            emit({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32603, 'message': 'DO_NOT_LOG_REMOTE_SECRET'}})
        elif mode == 'agent_tool_error':
            reply(request, {'content': [{'type': 'text', 'text': 'ordinary tool failure'}], 'isError': True})
        else:
            reply(request, {'content': [{'type': 'text', 'text': '42'}], 'structuredContent': {'answer': 'wrong' if mode == 'agent_bad_output' else 42}})
    elif method == 'resources/read':
        reply(request, {'contents': [{'uri': request['params']['uri'], 'text': 'hello', 'mimeType': 'text/plain'}]})
    elif method == 'prompts/get':
        reply(request, {'messages': [{'role': 'user', 'content': {'type': 'text', 'text': 'Review ' + request['params']['arguments']['path']}}]})
    elif method == 'ping':
        if mode in ('notify', 'unannounced'):
            changed('tools')
        if mode == 'resource_notify':
            changed('resources')
        if mode == 'updated':
            emit({'jsonrpc': '2.0', 'method': 'notifications/resources/updated', 'params': {'uri': 'file:///DO_NOT_OPEN'}})
        if mode == 'flood':
            for i in range(65):
                emit({'jsonrpc': '2.0', 'method': 'notifications/progress', 'params': {'progress': i}})
        reply(request, {})
        if mode == 'idle_notify':
            changed('tools')
    else:
        emit({'jsonrpc': '2.0', 'id': request.get('id'), 'error': {'code': -32601, 'message': 'unsupported'}})
