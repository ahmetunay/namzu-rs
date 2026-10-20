#!/usr/bin/env python3
"""Small authenticated loopback AG-UI terminal client; no model/provider credentials.

Reads the server capability from --token-env, defaults to denying approval requests.
A failed/truncated stream never becomes success. Content is private task data.
"""
import argparse
import http.client
import ipaddress
import json
import os
import sys
import urllib.parse
import uuid


def endpoint(url):
    u = urllib.parse.urlsplit(url)
    if u.scheme != 'http' or u.username or u.password or u.query or u.fragment or u.path != '/agent':
        raise ValueError('expected a loopback http://IP:PORT/agent endpoint')
    if not ipaddress.ip_address(u.hostname).is_loopback:
        raise ValueError('loopback IP required')
    return u.hostname, u.port or 80


def post(url, token, path, body):
    host, port = endpoint(url)
    conn = http.client.HTTPConnection(host, port, timeout=310)
    conn.request('POST', path, json.dumps(body).encode(), {'Authorization': 'Bearer ' + token,
                 'Content-Type': 'application/json', 'Accept': 'text/event-stream'})
    return conn, conn.getresponse()


def events(response):
    data = bytearray()
    total = 0
    while True:
        line = response.readline(1024 * 1024 + 2)
        if not line:
            if data: raise ValueError('incomplete SSE event')
            return
        total += len(line)
        if len(line) > 1024 * 1024 or total > 16 * 1024 * 1024: raise ValueError('stream limit')
        line = line.rstrip(b'\r\n')
        if not line:
            if data:
                yield json.loads(data)
                data.clear()
        elif line.startswith(b'data:'):
            if data: data.extend(b'\n')
            data.extend(line[5:].lstrip(b' '))
        elif not line.startswith(b':'): raise ValueError('unsupported SSE field')
        if len(data) > 1024 * 1024: raise ValueError('event limit')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--url', required=True)
    p.add_argument('--token-env', default='NAMZU_AGUI_TOKEN')
    p.add_argument('--thread-id', default=None)
    p.add_argument('--run-id', default=None)
    p.add_argument('--approval', choices=('deny', 'ask'), default='deny')
    p.add_argument('prompt')
    args = p.parse_args()
    token = os.environ.get(args.token_env)
    if not token: p.exit(2, 'Server capability environment variable is missing.\n')
    thread = args.thread_id or str(uuid.uuid4())
    run = args.run_id or str(uuid.uuid4())
    body = {'threadId': thread, 'runId': run, 'messages': [{'id': str(uuid.uuid4()),
            'role': 'user', 'content': args.prompt}], 'tools': [], 'context': [], 'state': {}, 'forwardedProps': {}}
    conn = None
    try:
        conn, response = post(args.url, token, '/agent', body)
        if response.status != 200: raise ValueError('request rejected')
        started = False
        terminal = False
        output = None
        for event in events(response):
            if terminal: raise ValueError('event after terminal')
            kind = event.get('type')
            if kind == 'RUN_STARTED':
                if started or event.get('runId') != run or event.get('threadId') != thread: raise ValueError('identity mismatch')
                started = True
            elif not started: raise ValueError('missing start')
            elif kind == 'CUSTOM' and event.get('name') == 'namzu.approval_required':
                value = event['value']
                allowed = False
                if args.approval == 'ask' and sys.stdin.isatty():
                    print(json.dumps({'tool': value['name'], 'arguments': value['arguments']}), file=sys.stderr)
                    allowed = input('Allow once? [y/N] ').strip().lower() == 'y'
                decision = {'threadId': thread, 'runId': run, 'approvalId': value['approvalId'],
                            'decision': 'allow' if allowed else 'deny'}
                control, result = post(args.url, token, '/approve', decision)
                try:
                    if result.status != 202: raise ValueError('approval not accepted')
                    result.read(4096)
                finally: control.close()
            elif kind == 'RUN_FINISHED':
                if event.get('runId') != run or event.get('threadId') != thread: raise ValueError('identity mismatch')
                output = event['result']['output']
                if not isinstance(output, str): raise ValueError('missing final result')
                terminal = True
            elif kind == 'RUN_ERROR':
                raise ValueError('agent run failed')
        if not terminal: raise ValueError('truncated run')
        print(json.dumps({'threadId': thread, 'runId': run, 'output': output}, ensure_ascii=False))
        return 0
    except KeyboardInterrupt:
        try:
            control, result = post(args.url, token, '/cancel', {'threadId': thread, 'runId': run})
            result.read(4096)
            control.close()
        except (OSError, ValueError, http.client.HTTPException): pass
        print('Run cancelled; inspect host session before retrying.', file=sys.stderr)
        return 130
    except (OSError, ValueError, KeyError, TypeError, http.client.HTTPException):
        print('AG-UI request failed or stream ended without a settled result.', file=sys.stderr)
        return 1
    finally:
        if conn: conn.close()


if __name__ == '__main__': raise SystemExit(main())
