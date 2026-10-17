#!/usr/bin/env python3
"""Linux PTY acceptance. Fixtures are offline; --live-model explicitly enables Codex.

No credentials are read or logged. Requires Python 3.11+ and a release CLI.
"""
import argparse
import contextlib
import codecs
import re
import unicodedata
import fcntl
import http.server
import json
import os
from pathlib import Path
import pty
import select
import shlex
import struct
import sys
import tempfile
import termios
import threading
import time


class Screen:
    """Small VT subset used by Ratatui; assertions inspect cells, not ANSI bytes."""
    def __init__(self):
        self.cells = [[' '] * 100 for _ in range(28)]
        self.row = self.col = 0
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending == '\x1b':
                return
            if self.pending.startswith('\x1b['):
                match = re.match(r'\x1b\[([0-?]*)([ -/]*)([@-~])', self.pending)
                if not match:
                    return
                params, _, code = match.groups()
                self.pending = self.pending[match.end():]
                values = [int(x) if x else 0 for x in params.split(';')] if not params.startswith('?') else []
                n = (values[0] if values else 0) or 1
                if code in ('H', 'f'):
                    self.row = min(27, max(0, n - 1))
                    self.col = min(99, max(0, ((values[1] if len(values) > 1 else 1) or 1) - 1))
                elif code == 'G': self.col = min(99, n - 1)
                elif code == 'A': self.row = max(0, self.row - n)
                elif code == 'B': self.row = min(27, self.row + n)
                elif code == 'C': self.col = min(99, self.col + n)
                elif code == 'D': self.col = max(0, self.col - n)
                elif code == 'J' and n == 2: self.cells = [[' '] * 100 for _ in range(28)]
                elif code == 'K':
                    mode = values[0] if values else 0
                    start, end = (0, 100) if mode == 2 else ((0, self.col + 1) if mode == 1 else (self.col, 100))
                    self.cells[self.row][start:end] = [' '] * (end - start)
                continue
            c, self.pending = self.pending[0], self.pending[1:]
            if c == '\r': self.col = 0
            elif c == '\n': self.row = min(27, self.row + 1)
            elif ord(c) >= 32 and not unicodedata.combining(c):
                self.cells[self.row][self.col] = c
                width = 2 if unicodedata.east_asian_width(c) in ('W', 'F') else 1
                self.col = min(99, self.col + width)

    def text(self):
        return '\n'.join(''.join(row) for row in self.cells)


class Terminal:
    def __init__(self, binary, args, cwd):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))
        self.original = termios.tcgetattr(self.slave)
        self.buffer = bytearray()
        self.screen = Screen()
        self.seen = ""
        self.status = None
        self.pid = os.fork()
        if self.pid == 0:
            try:
                os.close(self.master)
                os.login_tty(self.slave)
                os.chdir(cwd)
                env = dict(os.environ)
                env['TERM'] = 'xterm-256color'
                env['NAMZU_TUI_FIXTURE_KEY'] = 'fixture-only-not-a-real-key'
                for name in ('NO_PROXY', 'no_proxy'):
                    env[name] = ','.join(filter(None, [env.get(name), '127.0.0.1', 'localhost']))
                os.execve(str(binary), [str(binary), 'tui', *args], env)
            except BaseException:
                os._exit(127)

    def send(self, value):
        os.write(self.master, value)

    def pump(self, interval=0.03):
        if select.select([self.master], [], [], interval)[0]:
            try:
                data = os.read(self.master, 65536)
                self.buffer.extend(data)
                self.screen.feed(data)
                self.seen = (self.seen + "\n" + self.screen.text())[-1024 * 1024:]
                # Bound even an unexpectedly chatty fixture.
                del self.buffer[:-1024 * 1024]
            except OSError:
                pass
        if self.status is None:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = status

    def until(self, predicate, timeout=15):
        deadline = time.monotonic() + timeout
        while not predicate():
            self.pump()
            if self.status is not None:
                raise AssertionError(f'TUI exited early: {os.waitstatus_to_exitcode(self.status)}')
            if time.monotonic() >= deadline:
                raise AssertionError('PTY condition timed out')

    def contains(self, text):
        return text in self.screen.text() or text in self.seen

    def quit(self):
        self.send(b'\x1b')
        deadline = time.monotonic() + 15
        while self.status is None and time.monotonic() < deadline:
            self.pump()
        assert self.status is not None, 'TUI failed to join its worker'
        assert os.waitstatus_to_exitcode(self.status) == 0, 'TUI exit failed'
        assert termios.tcgetattr(self.slave) == self.original, 'terminal mode was not restored'
        assert b'\x1b[?1049l' in self.buffer, 'alternate screen was not left'
        assert b'\x1b[?2004l' in self.buffer, 'bracketed paste was not disabled'

    def close(self):
        if self.status is None:
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
        os.close(self.master)
        os.close(self.slave)


def answer(text, finish='stop'):
    return {'choices': [{'finish_reason': finish, 'message': {'role': 'assistant', 'content': text}}]}


def tools(calls):
    return {'choices': [{'finish_reason': 'tool_calls', 'message': {'role': 'assistant', 'content': None,
        'tool_calls': [{'id': name, 'type': 'function', 'function': {'name': tool,
            'arguments': json.dumps(args)}} for name, tool, args in calls]}}]}


class Fixture(http.server.HTTPServer):
    def __init__(self, callback):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    assert self.path == '/v1/chat/completions'
                    length = int(self.headers['Content-Length'])
                    assert 0 < length < 1024 * 1024
                    body = json.loads(self.rfile.read(length))
                    reply = callback(body, len(self.server.requests))
                    self.server.requests.append(body)
                    payload = json.dumps(reply).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append(type(error).__name__)
                    self.send_error(500)

            def log_message(self, *_):
                pass

        super().__init__(('127.0.0.1', 0), Handler)
        self.requests = []
        self.errors = []
        self.thread = None

    def start(self):
        # Start after the PTY fork so the child never forks a threaded interpreter.
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        if self.thread:
            self.shutdown()
            self.thread.join()
        self.server_close()
        assert not self.errors, f'fixture validation failed: {self.errors}'


def sessions(state):
    return [json.loads(p.read_text()) for p in state.glob('s-*.json')]


def completed(state, count=1):
    return sum(x['status'] == 'complete' for x in sessions(state)) == count


def setup_args(root, server, shell=False):
    args = ['--provider', 'openai-compatible', '--model', 'fixture', '--base-url',
        f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY',
        '--state-dir', str(root / 'state'), '--cwd', str(root), '--timeout', '10']
    if shell:
        args += ['--allow-shell', '--allow-write']
    return args


def fixture_case(binary, kind):
    with tempfile.TemporaryDirectory(prefix='namzu-tui-pty-') as directory:
        root = Path(directory)
        (root / 'input.txt').write_text('fixture input\n')
        state = root / 'state'

        def callback(body, index):
            assert body['model'] == 'fixture'
            if kind == 'resume':
                if index == 0:
                    assert all(t['function']['name'] not in ('shell', 'write_file') for t in body['tools'])
                    return tools([('read-one', 'read_file', {'path': 'input.txt'})])
                if index == 1:
                    result = body['messages'][-1]
                    assert result['tool_call_id'] == 'read-one'
                    assert json.loads(result['content'])['content'] == 'fixture input\n'
                    return answer('PTY_FIXTURE_OK')
                assert index == 2
                assert any(m.get('content') == 'PTY_FIXTURE_OK' for m in body['messages'])
                assert body['messages'][-1]['content'] == 'second turn'
                return answer('PTY_RESUME_OK')
            if kind == 'failure':
                if index == 0:
                    return answer('UNACCEPTED_OUTPUT', 'length')
                assert index == 1
                assert body['messages'] == [{'role': 'user', 'content': 'fresh request'}]
                return answer('PTY_NEW_OK')
            assert kind in ('cancel_shell', 'exit_shell') and index == 0
            command = shlex.join([sys.executable, '-c',
                'import os,time,pathlib; pathlib.Path("child.pid").write_text(str(os.getpid())); time.sleep(60)'])
            return tools([('shell-one', 'shell', {'command': command, 'timeout_seconds': 60}),
                ('never-run', 'write_file', {'path': 'should-not-exist.txt', 'content': 'bad'})])

        server = Fixture(callback)
        terminal = Terminal(binary, setup_args(root, server, 'shell' in kind), root)
        server.start()
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'first request\r')
            if kind == 'resume':
                terminal.until(lambda: completed(state) and terminal.contains('PTY_FIXTURE_OK') and 'Ready' in terminal.screen.text())
                terminal.send(b'second turn\r')
                terminal.until(lambda: terminal.contains('PTY_RESUME_OK') and any(
                    any(m.get('content') == 'PTY_RESUME_OK' for m in s['messages']) for s in sessions(state)))
                assert len(sessions(state)) == 1 and len(server.requests) == 3
            elif kind == 'failure':
                terminal.until(lambda: terminal.contains('Turn failed'))
                assert not terminal.contains('UNACCEPTED_OUTPUT')
                before = sessions(state)
                assert len(before) == 1 and before[0]['status'] == 'failed'
                terminal.send(b'do not auto replay\r\x0e')
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'fresh request\r')
                terminal.until(lambda: completed(state) and terminal.contains('PTY_NEW_OK'))
                assert len(sessions(state)) == 2 and len(server.requests) == 2
                assert next(s for s in sessions(state) if s['id'] == before[0]['id']) == before[0]
            else:
                terminal.until(lambda: (root / 'child.pid').exists())
                child = int((root / 'child.pid').read_text())
                with contextlib.ExitStack() as cleanup:
                    pidfd = os.pidfd_open(child)
                    cleanup.callback(os.close, pidfd)
                    if kind == 'cancel_shell':
                        terminal.send(b'\x03')
                        terminal.until(lambda: terminal.contains('Turn failed'))
                    terminal.quit()
                    assert select.select([pidfd], [], [], 1)[0], 'shell child survived cancellation'
                assert not (root / 'should-not-exist.txt').exists(), 'tool effect ran after cancellation'
                assert len(server.requests) == 1
                assert sessions(state)[0]['status'] == 'failed'
                return
            terminal.quit()
        finally:
            terminal.close()
            server.close()


def demo_case(binary):
    with tempfile.TemporaryDirectory(prefix='namzu-tui-demo-') as directory:
        root = Path(directory)
        terminal = Terminal(binary, ['--demo', '--state-dir', str(root / 'unused-state')], root)
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'run fixture\r')
            terminal.until(lambda: terminal.contains('2 tool calls'))
            assert terminal.contains('read_file') and terminal.contains('write_file')
            assert not (root / 'unused-state').exists()
            terminal.quit()
        finally:
            terminal.close()


def live_case(binary, model):
    with tempfile.TemporaryDirectory(prefix='namzu-tui-live-') as directory:
        root = Path(directory)
        (root / 'input.txt').write_text('TUI_LIVE_OK')
        terminal = Terminal(binary, ['--provider', 'codex', '--model', model, '--effort', 'low',
            '--cwd', str(root), '--timeout', '60', '--tool-allow', 'read_file'], root)
        try:
            terminal.until(lambda: terminal.contains('Enter a message'))
            terminal.send(b'Read input.txt with read_file and reply with its exact contents. Do not modify files or run shell commands.\r')
            terminal.until(lambda: terminal.contains('TUI_LIVE_OK') and 'Ready' in terminal.screen.text(), timeout=65)
            assert terminal.contains('read_file')
            terminal.quit()
        finally:
            terminal.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/namzu-rs'))
    parser.add_argument('--live-model', help='Explicitly consume Codex quota for one read-only task')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    result = {'demo': 'pending'}
    demo_case(binary)
    result['demo'] = 'passed'
    for case in ('resume', 'failure', 'cancel_shell', 'exit_shell'):
        fixture_case(binary, case)
        result[case] = 'passed'
    if args.live_model:
        live_case(binary, args.live_model)
        result['live'] = {'model': args.live_model, 'status': 'passed'}
    print(json.dumps({'pty_checks': result, 'terminal_restore': 'passed',
        'live_model_calls_enabled': bool(args.live_model)}))


if __name__ == '__main__':
    main()
