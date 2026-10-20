#!/usr/bin/env python3
"""Synthetic inputs test report validation; never run or represent real models."""
import copy
import unittest
from compare_report import AGENTS, TASKS, summarize


def fixture():
    rows = []
    for repeat in range(2):
        for task in TASKS:
            for agent in (AGENTS if repeat == 0 else AGENTS[::-1]):
                rows.append({"agent": agent, "task": task, "repeat": repeat,
                             "execution_index": len(rows), "status": "passed",
                             "oracle_passed": True, "exit_code": 0, "elapsed_ms": 10,
                             "peak_rss_kib": 100, "stats": None,
                             "program_sha256": "a"*64, "prompt_sha256": "b"*64,
                             "input_sha256": "c"*64, "oracle_sha256": "d"*64,
                             "permission_profile": "fixture"})
    return {"version": 2, "status": "complete", "mode": "live", "model": "fixture",
            "sequential": True, "timeout_seconds": 300, "suite_sha256": "e"*64,
            "planned_runs": len(rows), "runs": rows}


class Comparison(unittest.TestCase):
    def test_balanced_matrix_unknown_counts_and_redaction(self):
        report = fixture()
        report["output_directory"] = "private-path"
        report["runs"][0]["output"] = "private-output"
        result = summarize(report)
        self.assertEqual(result["both_passed_pairs"], 10)
        self.assertEqual(result["summary"][0]["calls"]["tool_calls"]["sum_known"], None)
        self.assertNotIn("private", str(result))

    def test_fixture_partial_unequal_and_incomplete_inputs_rejected(self):
        for mutate in [lambda r: r.update(mode="fixture"),
                       lambda r: r.update(status="running"),
                       lambda r: r["runs"].pop(),
                       lambda r: r["runs"][0].update(prompt_sha256="f"*64),
                       lambda r: r["runs"][0].update(permission_profile="different"),
                       lambda r: r["runs"][0].update(oracle_passed=None)]:
            report = fixture()
            mutate(report)
            with self.assertRaises(ValueError):
                summarize(report)

    def test_wrong_answers_are_not_used_in_paired_speed_ratio(self):
        report = fixture()
        report["runs"][0].update(status="incorrect", oracle_passed=False, elapsed_ms=1)
        result = summarize(report)
        self.assertEqual(result["both_passed_pairs"], 9)
        self.assertEqual(result["summary"][0]["statuses"]["incorrect"], 1)

    def test_reported_counts_have_explicit_coverage(self):
        report = fixture()
        report["runs"][0]["stats"] = {"provider_calls": 2, "tool_calls": 1}
        counts = summarize(report)["summary"][0]["calls"]
        self.assertEqual(counts["provider_calls"], {"known_runs": 1, "total_runs": 10, "sum_known": 2})

    def test_program_changes_duplicates_and_order_bias_are_rejected(self):
        report = fixture()
        mutations = [lambda r: r["runs"][2].update(program_sha256="f"*64),
                     lambda r: r["runs"].append(copy.deepcopy(r["runs"][0])),
                     lambda r: r["runs"].__setitem__(slice(10, 12), r["runs"][10:12][::-1])]
        for mutate in mutations:
            case = copy.deepcopy(report)
            mutate(case)
            with self.assertRaises(ValueError):
                summarize(case)


if __name__ == "__main__":
    unittest.main()
