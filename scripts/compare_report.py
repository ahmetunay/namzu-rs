#!/usr/bin/env python3
"""Validate a balanced Namzu-RS/Jcode live report and export numeric results only.

Does not run models, read auth files, or copy raw agent/checker output.
"""
import argparse
import json
import math
import os
from pathlib import Path
import re
from statistics import median

AGENTS = ("namzu-rs", "jcode")
TASKS = ("slug", "intervals", "toposort", "ledger", "refactor")
STATUSES = ("passed", "incorrect", "agent_error", "timeout", "output_limit", "oracle_error")


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def numeric(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def digest(value):
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None


def summarize(report):
    require(report.get("version") == 2 and report.get("status") == "complete",
            "requires a complete v2 report")
    require(report.get("mode") == "live" and report.get("sequential") is True,
            "requires a sequential live report; fixtures are not model evidence")
    require(report.get("timeout_seconds") == 300, "requires 300-second agent budgets")
    model = report.get("model")
    require(isinstance(model, str) and re.fullmatch(r"[A-Za-z0-9._:/-]{1,256}", model),
            "invalid model identifier")
    require(digest(report.get("suite_sha256")), "missing suite identity")
    rows = report.get("runs")
    require(isinstance(rows, list) and rows, "missing runs")
    indexed = {}
    public_rows = []
    programs = {}
    profiles = set()
    for position, row in enumerate(rows):
        require(row.get("agent") in AGENTS and row.get("task") in TASKS, "unexpected agent/task")
        require(type(row.get("repeat")) is int and 0 <= row["repeat"] < 10, "invalid repeat")
        require(row.get("execution_index") == position, "nonsequential execution index")
        key = (row["repeat"], row["task"], row["agent"])
        require(key not in indexed, "duplicate run")
        require(row.get("status") in STATUSES, "unknown status")
        require(numeric(row.get("elapsed_ms")), "missing elapsed time")
        rss = row.get("peak_rss_kib")
        require(rss is None or numeric(rss), "invalid RSS")
        for field in ("prompt_sha256", "input_sha256", "oracle_sha256", "program_sha256"):
            require(digest(row.get(field)), "missing identity hash")
        require(row.get("oracle_passed") is None or type(row["oracle_passed"]) is bool,
                "invalid oracle measurement")
        require(row["status"] != "passed" or
                (row["oracle_passed"] is True and row.get("exit_code") == 0),
                "pass without successful execution/oracle")
        require(programs.setdefault(row["agent"], row["program_sha256"]) == row["program_sha256"],
                "agent program changed during experiment")
        profile = row.get("permission_profile")
        require(isinstance(profile, str) and 0 < len(profile) <= 128, "missing permission declaration")
        profiles.add(profile)
        indexed[key] = row
        stats = row.get("stats")
        counts = {name: stats.get(name) if isinstance(stats, dict) else None
                  for name in ("provider_calls", "tool_calls")}
        require(all(value is None or (type(value) is int and value >= 0)
                    for value in counts.values()), "invalid call count")
        public_rows.append({field: row[field] for field in (
            "agent", "task", "repeat", "execution_index", "status", "oracle_passed",
            "elapsed_ms", "peak_rss_kib", "program_sha256", "prompt_sha256",
            "input_sha256", "oracle_sha256")} | {"stats": counts})
    require(len(profiles) == 1, "unequal declared permissions")
    repeats = sorted({row["repeat"] for row in rows})
    require(repeats == list(range(len(repeats))) and len(repeats) >= 2 and len(repeats) % 2 == 0,
            "requires contiguous, even, balanced repeats")
    expected = [(repeat, task, agent) for repeat in repeats for task in TASKS
                for agent in (AGENTS if repeat % 2 == 0 else AGENTS[::-1])]
    require(list(indexed) == expected and report.get("planned_runs") == len(expected),
            "incomplete matrix or unbalanced ordering")
    ratios = []
    for repeat in repeats:
        for task in TASKS:
            left, right = (indexed[(repeat, task, agent)] for agent in AGENTS)
            for field in ("prompt_sha256", "input_sha256", "oracle_sha256"):
                require(left[field] == right[field], "unequal task/oracle definitions")
            if left["status"] == right["status"] == "passed" and left["elapsed_ms"] > 0:
                ratios.append(right["elapsed_ms"] / left["elapsed_ms"])
    summary = []
    for agent in AGENTS:
        selected = [row for row in public_rows if row["agent"] == agent]
        elapsed = [row["elapsed_ms"] for row in selected]
        passed = [row["elapsed_ms"] for row in selected if row["status"] == "passed"]
        rss = [row["peak_rss_kib"] for row in selected if row["peak_rss_kib"] is not None]
        counts = {}
        for field in ("provider_calls", "tool_calls"):
            known = [row["stats"][field] for row in selected if row["stats"][field] is not None]
            counts[field] = {"known_runs": len(known), "total_runs": len(selected),
                             "sum_known": sum(known) if known else None}
        summary.append({"agent": agent, "total": len(selected),
                        "statuses": {status: sum(row["status"] == status for row in selected)
                                     for status in STATUSES},
                        "total_agent_ms": sum(elapsed), "median_agent_ms": median(elapsed),
                        "median_passed_agent_ms": median(passed) if passed else None,
                        "max_peak_rss_kib": max(rss) if rss else None,
                        "median_peak_rss_kib": median(rss) if rss else None, "calls": counts})
    return {"version": 1, "kind": "live_agent_comparison", "model": model,
            "timeout_seconds": 300, "repeats": len(repeats), "sequential": True,
            "suite_sha256": report["suite_sha256"],
            "memory_scope": "wait4 ru_maxrss; not aggregate simultaneous process-tree RSS/PSS",
            "permission_profile_is_declared": True, "summary": summary, "runs": public_rows,
            "both_passed_pairs": len(ratios),
            "median_paired_jcode_over_namzu_elapsed": median(ratios) if ratios else None,
            "limitations": ["Small sample; no statistical superiority or language-effect claim.",
                            "System prompts, internal limits and tool implementations differ.",
                            "Call counts are self-reported; unknown counts remain null.",
                            "Hashes do not identify all dependencies or the whole environment."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--out", type=Path, required=True, help="Create a NEW numeric report")
    args = parser.parse_args()
    data = summarize(json.loads(args.report.read_text()))
    with os.fdopen(os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(data, output, indent=2)
        output.write("\n")
    print(json.dumps({"written": str(args.out), "runs": len(data["runs"])}))


if __name__ == "__main__":
    main()
