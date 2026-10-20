#!/usr/bin/env python3
"""Offline tests of the explicit live runner; no real CLI, model, auth or network."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from provider_live_smoke import parser, run


class LiveRunner(unittest.TestCase):
    def args(self, *extra):
        return parser().parse_args(['--live', '--provider', 'openrouter', '--model', 'fixture', *extra])

    def fake(self, command, **kwargs):
        self.assertTrue(kwargs['capture_output'])
        self.assertEqual(kwargs['timeout'], 315)
        self.assertNotIn('--allow-write', command)
        self.assertNotIn('--allow-shell', command)
        work = Path(command[command.index('--cwd') + 1])
        config = json.loads(Path(command[command.index('--config') + 1]).read_text())
        self.assertEqual(config['tool_allow'], ['read_file'])
        self.assertEqual(config['api_options']['max_output_tokens'], 128)
        return subprocess.CompletedProcess(command, 0, json.dumps({'status': 'ok',
            'output': (work / 'oracle.txt').read_text().strip(),
            'stats': {'provider_calls': 2, 'tool_calls': 1}}).encode(), b'')

    def test_exact_read_oracle_and_private_report(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'report.json'
            result = run(self.args('--out', str(path)), self.fake)
            self.assertEqual(result['status'], 'passed')
            self.assertEqual(json.loads(path.read_text()), result)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotIn('NAMZU_SMOKE_', path.read_text())

    def test_live_flag_required_before_work(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            parser().parse_args(['--provider', 'openrouter', '--model', 'fixture'])
        args = self.args()
        args.live = False
        with self.assertRaises(ValueError): run(args, self.fail)

    def test_output_must_match_and_tool_must_have_run(self):
        for stats in ({'provider_calls': 2, 'tool_calls': 1}, {'provider_calls': 1, 'tool_calls': 0}):
            result = run(self.args(), lambda *a, **k: subprocess.CompletedProcess(a, 0,
                json.dumps({'status': 'ok', 'output': 'WRONG', 'stats': stats}).encode(), b''))
            self.assertEqual(result['status'], 'failed')

    def test_no_tool_shortcut_even_if_output_matches(self):
        def fake(command, **kwargs):
            response = self.fake(command, **kwargs)
            data = json.loads(response.stdout)
            data['stats'] = {'provider_calls': 1, 'tool_calls': 0}
            response.stdout = json.dumps(data).encode()
            return response
        self.assertEqual(run(self.args(), fake)['status'], 'failed')

    def test_server_error_and_stderr_are_not_copied(self):
        secret = 'PRIVATE_ERROR_SECRET'
        result = run(self.args(), lambda *a, **k: subprocess.CompletedProcess(a, 1,
            json.dumps({'error': secret}).encode(), secret.encode()))
        self.assertEqual(result['failure'], 'cli_error')
        self.assertNotIn(secret, json.dumps(result))

    def test_bad_json_timeout_and_missing_binary(self):
        def timeout(*a, **k): raise subprocess.TimeoutExpired('private-command', 1)
        def missing(*a, **k): raise FileNotFoundError('private-path')
        for invoke, reason in ((lambda *a, **k: subprocess.CompletedProcess(a, 1, b'private', b'private'), 'invalid_cli_json'),
                               (timeout, 'process_timeout'), (missing, 'process_start')):
            self.assertEqual(run(self.args(), invoke)['failure'], reason)

    def test_malformed_fields_do_not_crash_or_copy_arbitrary_data(self):
        for data in ({'status': 'ok', 'output': [], 'stats': []},
                     {'status': 'ok', 'stats': {'provider_calls': 'PRIVATE', 'tool_calls': {}}}):
            result = run(self.args(), lambda *a, **k: subprocess.CompletedProcess(a, 0,
                json.dumps(data).encode(), b''))
            self.assertEqual(result['status'], 'failed')
            self.assertNotIn('PRIVATE', json.dumps(result))

    def test_existing_report_is_not_overwritten_and_sends_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'report.json'
            path.write_text('KEEP')
            with self.assertRaises(FileExistsError): run(self.args('--out', str(path)), self.fail)
            self.assertEqual(path.read_text(), 'KEEP')

    def test_codex_and_selectors_fail_closed(self):
        for extra in (('--api-key-env', 'sk-or-secret'), ('--aws-region', 'us-east-1')):
            with self.assertRaises(ValueError): run(self.args(*extra), self.fail)
        args = parser().parse_args(['--live', '--provider', 'codex', '--model', 'fixture'])
        with self.assertRaises(ValueError): run(args, self.fail)
        args.base_url = 'http://localhost'
        with self.assertRaises(ValueError): run(args, self.fail)


if __name__ == '__main__':
    unittest.main()
