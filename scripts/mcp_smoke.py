#!/usr/bin/env python3
"""Offline MCP TUI acceptance with real PTYs and independent local peers."""
import json
import itertools
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from tui_smoke import Fixture, Terminal, answer, tools


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/namzu-rs').resolve()
    peer = Path(__file__).resolve().parents[1] / 'crates/mcp/tests/fixtures/discovery_server.py'
    results = {}
    for mode, decision in itertools.product(('tui', 'pal'), ('allow', 'deny', 'cancel')):
        with tempfile.TemporaryDirectory(prefix='namzu-mcp-pty-') as directory:
            root = Path(directory)
            state = root / 'state'
            spec = root / 'server.json'
            spec.write_text(json.dumps({'transport': {'type': 'stdio', 'program': '/usr/bin/python3', 'args': [str(peer), 'normal'], 'cwd': str(root)}, 'policy': {'require_approval': True}}))
            subprocess.run([str(binary), 'mcp', '--state-dir', str(state), 'add', 'fixture', '--spec', str(spec)], check=True, capture_output=True)
            discovered = subprocess.check_output([str(binary), 'mcp', '--state-dir', str(state), 'discover', 'fixture', 'tools', '--approve', '--agent-names'], text=True)
            # Discover's machine output supplies aliases; no duplicated hash implementation.
            data = json.loads(discovered)
            names = []
            def walk(value):
                if isinstance(value, dict):
                    for key, item in value.items():
                        if key == 'alias' and isinstance(item, str): names.append(item)
                        walk(item)
                elif isinstance(value, list):
                    for item in value: walk(item)
            walk(data)
            assert names, data
            name = next(n for n in names if '_read_' in n)
            (root / 'requests.jsonl').unlink()
            def callback(body, count):
                if count == 0: return tools([('one', name, {'path': 'a'})])
                message = next(m for m in body['messages'] if m['role'] == 'tool')
                outer = json.loads(message['content'])
                assert outer['is_error'] == (decision != 'allow')
                if decision == 'allow': assert json.loads(outer['content'])['structuredContent']['answer'] == 42
                return answer('MCP_PTY_DONE')
            fixture = Fixture(callback)
            args = ['--state-dir', str(state), '--cwd', str(root), '--model', 'fixture', '--base-url', f'http://127.0.0.1:{fixture.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY', '--mcp-server', 'fixture', '--timeout', '30']
            if mode == 'pal':
                args = [a for a in args]
                index = args.index('--cwd')
                del args[index:index + 2]
                args += ['--tool-allow', name]
                created = subprocess.check_output([str(binary), 'pal', *args, 'create', 'MCP PTY'], text=True, env={**os.environ, 'NAMZU_TUI_FIXTURE_KEY': 'fixture-only'})
                pal_id = json.loads(created)['result']['id']
                terminal = Terminal(binary, [*args, 'tui', pal_id], root, command=['pal'])
            else:
                terminal = Terminal(binary, args, root)
            fixture.start()
            try:
                terminal.until(lambda: terminal.contains('Enter a message'))
                terminal.send(b'test MCP\r')
                # Connect + two tools pages + resources + prompts, then actual tool.
                for _ in range(5):
                    terminal.until(lambda: 'Allow once?' in terminal.screen.text())
                    terminal.send(b'y')
                    terminal.until(lambda: 'Allow once?' not in terminal.screen.text())
                terminal.until(lambda: 'Allow once?' in terminal.screen.text())
                terminal.send({'allow': b'y', 'deny': b'n', 'cancel': b'\x03'}[decision])
                if decision == 'cancel':
                    terminal.until(lambda: terminal.contains('cancelled') or terminal.contains('Cancelled'))
                else:
                    terminal.until(lambda: terminal.contains('MCP_PTY_DONE'))
                terminal.quit()
            finally:
                terminal.close()
                fixture.close()
            calls = [json.loads(line) for line in (root / 'requests.jsonl').read_text().splitlines()]
            assert sum(r.get('method') == 'tools/call' for r in calls) == (decision == 'allow')
            results[f'{mode}_{decision}'] = 'passed'
    print(json.dumps(results))


if __name__ == '__main__':
    main()
