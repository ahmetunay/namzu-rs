#!/usr/bin/env python3
"""Exercise the exact opt-in live runner against ten real CLI loopback wires.

No live models, real keys, or auth stores. Native Codex fixed endpoint is tested
by the Rust matrix; it has no production CLI loopback override.
"""
import json
from pathlib import Path
import re
import subprocess
import tempfile
from native_providers_smoke import Fixture, answer, env
from provider_live_smoke import parser, run

ROOT = Path(__file__).resolve().parents[1]


def read_call(provider):
    v = answer(provider, True)
    arguments = {'path': 'oracle.txt'}
    if 'choices' in v:
        v['choices'][0]['message']['tool_calls'][0]['function'] = {'name': 'read_file', 'arguments': json.dumps(arguments)}
    elif provider == 'anthropic':
        v['content'][-1].update(name='read_file', input=arguments)
    elif provider == 'google':
        v['candidates'][0]['content']['parts'][0]['functionCall'] = {'name': 'read_file', 'args': arguments}
    elif provider == 'bedrock':
        v['output']['message']['content'][-1]['toolUse'].update(name='read_file', input=arguments)
    else:
        v['message']['tool_calls'][0]['function'] = {'name': 'read_file', 'arguments': arguments}
    return v


def main():
    checks = []
    for provider in ('openai-compatible', 'openrouter', 'zen', 'zen-go', 'deepseek', 'lmstudio',
                     'anthropic', 'google', 'bedrock', 'ollama'):
        native = provider if provider in ('anthropic', 'google', 'bedrock', 'ollama') else 'deepseek'
        def callback(path, headers, raw, body, index):
            assert index < 2, 'unexpected retry or extra model task'
            if index == 0:
                assert 'oracle.txt' in json.dumps(body)
                return read_call(native)
            token = re.search(r'NAMZU_SMOKE_[a-f0-9]{32}', json.dumps(body)).group(0)
            return json.loads(json.dumps(answer(native)).replace('NATIVE_OK', token))
        server = Fixture(callback)
        server.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                report_path = Path(directory) / 'report.json'
                args = parser().parse_args(['--live', '--provider', provider, '--model', 'fixture',
                    '--base-url', f'http://127.0.0.1:{server.server_port}',
                    '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY', '--out', str(report_path)])
                def invoke(*a, **kwargs):
                    return subprocess.run(*a, env=env(), **kwargs)
                report = run(args, invoke)
                assert report['status'] == 'passed', (provider, report)
                assert report['tool_calls'] == 1 and report['provider_calls'] == 2
                assert report['input_unchanged']
                assert len(server.requests) == 2
                checks.append(provider + '_live_runner_loopback_read_oracle')
        finally:
            server.close()
    print(json.dumps({'passed': len(checks), 'checks': checks, 'live_model_calls': 0}))


if __name__ == '__main__':
    main()
