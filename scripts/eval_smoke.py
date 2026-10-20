#!/usr/bin/env python3
"""Release CLI eval acceptance. Fixtures only; never calls a model service."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/namzu-rs"))
    parser.add_argument("--out", type=Path, default=Path("artifacts/eval-acceptance"))
    args = parser.parse_args()
    binary = args.binary.resolve()
    args.out.mkdir(parents=True, exist_ok=True, mode=0o700)
    report_file = args.out / "report.json"
    checks = []

    def record(name, function):
        try:
            function()
        except Exception:
            checks.append({"name": name, "passed": False})
            save("failed")
            raise
        checks.append({"name": name, "passed": True})
        save("running")

    def save(status):
        data = {"status": status, "mode": "fixture", "live_model_calls": 0, "checks": checks}
        temporary = report_file.with_suffix(".tmp")
        temporary.write_text(json.dumps(data, indent=2) + "\n")
        temporary.chmod(0o600)
        temporary.replace(report_file)

    def command(*argv, valid=True):
        result = subprocess.run([str(binary), "bench", *map(str, argv)],
                                capture_output=True, text=True, timeout=90)
        assert (result.returncode == 0) == valid, result.stderr
        return json.loads(result.stdout) if valid else None

    save("running")
    with tempfile.TemporaryDirectory(prefix="eval-", dir=args.out.resolve()) as folder:
        root = Path(folder)
        legacy = command("demo", "--out", root / "legacy")
        record("legacy_three_task_compatibility", lambda: equal(
            [item["passed"] for item in legacy["summary"]], [3, 0]))
        first = command("demo", "--task-set", "eval", "--out", root / "first")
        record("five_known_solutions", lambda: equal(first["summary"][0]["passed"], 5))
        record("five_seeded_or_empty_solutions_rejected", lambda: equal(
            (first["summary"][1]["passed"], first["summary"][1]["incorrect"]), (0, 5)))
        second = command("demo", "--task-set", "eval", "--out", root / "second")

        def deterministic_identity():
            fields = ["agent", "task", "status", "oracle_passed", "prompt_sha256",
                      "input_sha256", "oracle_sha256", "program_sha256"]
            identity = lambda report: [[row[field] for field in fields] for row in report["runs"]]
            equal(identity(first), identity(second))
            equal(first["suite_sha256"], second["suite_sha256"])
        record("repeatable_outcomes_and_all_identity_hashes", deterministic_identity)

        suite_path = root / "suite.json"
        suite = {"version": 1, "mode": "fixture", "model": "scripted-fixture",
                 "timeout_seconds": 300, "repeats": 1,
                 "agents": [{"name": name, "program": sys.executable,
                             "args": ["-I", "-c", code], "permission_profile": "fixture-python"}
                            for name, code in [
                                ("correct", "from pathlib import Path; Path('answer.txt').write_text('12\\n')"),
                                ("wrong", "print('done')"),
                                ("exit-error", "raise SystemExit(9)")]],
                 "tasks": [{"id": "file", "prompt": "Write exactly 12 newline to answer.txt",
                            "files": {}, "expected_files": {"answer.txt": "12\n"},
                            "check": [sys.executable, "-I", "-c", "print('private-oracle-marker')"]}]}

        def run_suite():
            suite_path.write_text(json.dumps(suite))
            return command("run", suite_path, "--out", root / "custom")
        result = run_suite()
        record("typed_file_and_execution_failures", lambda: equal(
            [row["status"] for row in result["runs"]], ["passed", "incorrect", "agent_error"]))

        def private_diagnostics():
            assert "private-oracle-marker" not in json.dumps(result)
            directory = Path(result["output_directory"])
            log = directory / "run-0.log.json"
            assert "private-oracle-marker" in log.read_text()
            equal(log.stat().st_mode & 0o777, 0o600)
            equal((directory / "report.json").stat().st_mode & 0o777, 0o600)
            equal(result["status"], "complete")
            equal(result["planned_runs"], 3)
        record("private_logs_and_complete_report", private_diagnostics)

        suite["agents"] = suite["agents"][:1]
        suite["tasks"][0]["check"][-1] = "print('x'*10000)"
        suite["tasks"][0]["check_output_limit"] = 32
        result = run_suite()
        record("unavailable_oracle_is_not_incorrect", lambda: equal(
            (result["runs"][0]["status"], result["runs"][0]["oracle_passed"],
             result["summary"][0]["incorrect"]), ("oracle_error", None, 0)))

        def preflight():
            suite["tasks"][0]["check_timeout_seconds"] = 31
            suite_path.write_text(json.dumps(suite))
            command("run", suite_path, "--out", root / "must-not-exist", valid=False)
            assert not (root / "must-not-exist").exists()
            suite["tasks"][0]["check_timeout_seconds"] = 30
            suite["tasks"][0]["check"][0] = "/no/such/checker"
            suite_path.write_text(json.dumps(suite))
            command("run", suite_path, "--out", root / "must-not-exist", valid=False)
            assert not (root / "must-not-exist").exists()
        record("invalid_limits_and_commands_fail_before_effects", preflight)

        def init_and_validate():
            live = root / "live.json"
            command("init", "--task-set", "eval", "--model", "fixture-never-called",
                    "--jcode", binary, "--file", live)
            equal(command("validate", live)["tasks"], 5)
            data = json.loads(live.read_text())
            equal(data["timeout_seconds"], 300)
            equal([task["id"] for task in data["tasks"]],
                  ["slug", "intervals", "toposort", "ledger", "refactor"])
            command("init", "--model", "fixture", "--jcode", binary, "--file", live, valid=False)
        record("expanded_init_and_validate_without_inference", init_and_validate)
    save("passed")
    print(json.dumps({"passed": len(checks), "failed": 0, "live_model_calls": 0,
                      "report": str(report_file)}))


def equal(actual, expected):
    assert actual == expected, (actual, expected)


if __name__ == "__main__":
    main()
