#!/usr/bin/env python3
"""Explicit, bounded read-only live smoke. Never called by normal tests.

--live, an exact --provider and --model are mandatory. This may consume quota.
Only the CLI reads credentials. Reports never copy model output or server bodies.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
PROVIDERS = ('openai-compatible', 'openrouter', 'zen', 'zen-go', 'anthropic', 'google',
             'deepseek', 'bedrock', 'lmstudio', 'ollama', 'codex')


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--live', action='store_true', required=True,
                   help='Explicitly allow one real model task; may consume quota.')
    p.add_argument('--provider', choices=PROVIDERS, required=True)
    p.add_argument('--model', required=True, help='Exact ID; no discovery or fallback.')
    p.add_argument('--binary', type=Path, default=ROOT / 'target/release/namzu-rs')
    p.add_argument('--state-dir', type=Path, help='Existing private state, e.g. for Codex auth.')
    p.add_argument('--base-url', help='Explicit custom endpoint; Codex endpoints remain fixed.')
    p.add_argument('--api-key-env', help='Environment variable NAME, never a key value.')
    p.add_argument('--format', choices=('auto', 'chat', 'responses'), default='auto')
    p.add_argument('--stream', action='store_true', help='Only where implemented.')
    p.add_argument('--bedrock-auth', choices=('bearer', 'sigv4'))
    p.add_argument('--aws-region')
    p.add_argument('--timeout', type=int, choices=range(1, 301), default=300, metavar='1..300')
    p.add_argument('--out', type=Path, help='Create a NEW private JSON report; never overwrite.')
    return p


def run(args, invoke=subprocess.run):
    """One model turn, no retries. invoke is injectable for offline runner tests."""
    if not args.live:
        raise ValueError('explicit --live required')
    if not args.model.strip() or len(args.model) > 256 or any(ord(c) < 32 for c in args.model):
        raise ValueError('invalid model ID')
    if args.provider == 'codex' and (args.base_url or args.api_key_env or args.format != 'auto'
                                    or args.stream or args.bedrock_auth or args.aws_region):
        raise ValueError('API options cannot change the Codex subscription transport')
    if args.provider != 'bedrock' and (args.bedrock_auth or args.aws_region):
        raise ValueError('AWS options require --provider bedrock')
    if args.provider == 'codex' and args.state_dir is None:
        raise ValueError('Codex requires existing authenticated private --state-dir')
    if args.api_key_env and (not args.api_key_env.isascii() or not args.api_key_env.isidentifier()):
        raise ValueError('--api-key-env accepts a variable NAME, never a key')
    # Reserve the destination before making any request; never follow a report symlink.
    output = os.fdopen(os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600),
                       'w', encoding='utf-8') if args.out else None
    started = time.monotonic()
    report = {'format_version': 1, 'kind': 'provider_live_smoke', 'live': True,
              'provider': args.provider, 'model': args.model, 'status': 'failed',
              'automatic_retry': False, 'fallback': False, 'tool_effect': 'read_only'}
    try:
        with tempfile.TemporaryDirectory(prefix='namzu-live-smoke-') as directory:
            work = Path(directory)
            token = 'NAMZU_SMOKE_' + secrets.token_hex(16)
            (work / 'oracle.txt').write_text(token + '\n', encoding='utf-8')
            # An explicit isolated config avoids inherited capability/sandbox/model settings.
            # Leave provider at the config default; the explicit CLI selector then
            # resolves that service's own endpoint/key-env instead of OpenAI's defaults.
            config = {'version': 1, 'model': args.model,
                      'timeout': args.timeout, 'max_steps': 3, 'max_tool_calls': 2,
                      'max_text_bytes': 65536, 'tool_allow': ['read_file']}
            if args.provider != 'codex':
                config['api_options'] = {'format': args.format, 'stream': args.stream,
                                         'max_output_tokens': 128}
                if args.bedrock_auth: config['api_options']['bedrock_auth'] = args.bedrock_auth
                if args.aws_region: config['api_options']['aws_region'] = args.aws_region
            config_path = work / 'config.json'
            config_path.write_text(json.dumps(config), encoding='utf-8')
            config_path.chmod(0o600)
            command = [str(args.binary.resolve()), 'exec',
                       'Use read_file to read oracle.txt. Return exactly its single line and nothing else. '
                       'Do not guess the line. Do not call other tools.',
                       '--config', str(config_path), '--provider', args.provider,
                       '--model', args.model, '--cwd', str(work), '--json']
            command += ['--state-dir', str(args.state_dir.resolve() if args.state_dir else work / 'state')]
            if args.base_url: command += ['--base-url', args.base_url]
            if args.api_key_env: command += ['--api-key-env', args.api_key_env]
            try:
                result = invoke(command, capture_output=True, timeout=args.timeout + 15, check=False)
            except subprocess.TimeoutExpired:
                report['failure'] = 'process_timeout'
            except OSError:
                report['failure'] = 'process_start'
            else:
                try:
                    data = json.loads(result.stdout)
                    if not isinstance(data, dict): raise ValueError('expected object')
                except (ValueError, UnicodeError):
                    report['failure'] = 'invalid_cli_json'
                else:
                    stats = data.get('stats')
                    stats = stats if isinstance(stats, dict) else {}
                    answer = data.get('output')
                    correct = isinstance(answer, str) and answer.strip() == token
                    tools = stats.get('tool_calls')
                    calls = stats.get('provider_calls')
                    tools = tools if type(tools) is int and 0 <= tools <= 2 else None
                    calls = calls if type(calls) is int and 0 <= calls <= 3 else None
                    intact = (work / 'oracle.txt').read_text(encoding='utf-8') == token + '\n'
                    report.update(exact_output=correct, input_unchanged=intact,
                                  provider_calls=calls, tool_calls=tools)
                    if (result.returncode == 0 and data.get('status') == 'ok' and correct and intact
                            and isinstance(tools, int) and 1 <= tools <= 2
                            and isinstance(calls, int) and 2 <= calls <= 3):
                        report['status'] = 'passed'
                    else:
                        report['failure'] = 'cli_error' if result.returncode else 'oracle_mismatch'
                    # Usage, output, arbitrary error fields and credentials are intentionally absent.
        report['elapsed_ms'] = round((time.monotonic() - started) * 1000)
        if output:
            output.write(json.dumps(report, indent=2) + '\n')
            output.flush()
        return report
    finally:
        if output: output.close()


def main():
    p = parser()
    args = p.parse_args()
    try:
        report = run(args)
    except (ValueError, OSError):
        # Do not interpolate custom paths, CLI/server output, or endpoint credentials.
        p.exit(2, 'Live smoke configuration/report destination is invalid; no result was published.\n')
    print(json.dumps(report))
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
