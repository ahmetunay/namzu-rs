#!/usr/bin/env python3
"""6.5 deterministic provider acceptance entry point. No live model calls.

Requires the release CLI and Rust toolchain. Reports failed/skipped work as failure;
never substitutes a live smoke for the loopback contract. Logs stay in artifacts.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cargo', default='cargo')
    parser.add_argument('--out', type=Path, default=ROOT / 'artifacts/provider-conformance')
    args = parser.parse_args()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    suites = [
        ('sdk_matrix', [args.cargo, 'test', '-p', 'namzu-provider', '--lib', 'conformance::', '--locked']),
        ('live_runner_offline', [sys.executable, 'scripts/test_provider_live_smoke.py']),
        ('live_runner_loopback', [sys.executable, 'scripts/live_smoke_fixture.py']),
        ('catalog', [sys.executable, 'scripts/catalog_smoke.py']),
        ('native', [sys.executable, 'scripts/native_providers_smoke.py']),
        ('services', [sys.executable, 'scripts/services_smoke.py']),
        ('contract', [sys.executable, 'scripts/provider_contract_smoke.py']),
        ('tui', [sys.executable, 'scripts/tui_smoke.py', '--binary', 'target/release/namzu-rs']),
    ]
    report = {'format_version': 1, 'kind': 'provider_conformance', 'live_model_calls': 0,
              'status': 'running', 'suites': [{'name': name, 'status': 'pending'} for name, _ in suites]}
    def publish():
        temporary = output / 'report.json.tmp'
        temporary.write_text(json.dumps(report, indent=2) + '\n')
        temporary.replace(output / 'report.json')
    publish()
    environment = dict(os.environ)
    environment.update(CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', CARGO_INCREMENTAL='0')
    for index, (name, command) in enumerate(suites):
        print(f'Running {name}', file=sys.stderr, flush=True)
        report['suites'][index]['status'] = 'running'
        publish()
        with (output / f'{name}.log').open('wb') as log:
            try:
                result = subprocess.run(command, cwd=ROOT, env=environment, stdout=log,
                                        stderr=subprocess.STDOUT, timeout=300, check=False)
                status = 'passed' if result.returncode == 0 else 'failed'
            except (OSError, subprocess.TimeoutExpired):
                status = 'failed'
        report['suites'][index]['status'] = status
        # Publish incremental results so an interruption cannot leave an old green report.
        publish()
    report['status'] = 'passed' if all(s['status'] == 'passed' for s in report['suites']) else 'failed'
    publish()
    print(json.dumps(report))
    return int(report['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
