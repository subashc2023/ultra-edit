"""Tests for eval/stats.py and eval/digest.py, the result-analysis helpers."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str):
    spec = importlib.util.spec_from_file_location(
        f"ultra_edit_eval_{name}", REPO_ROOT / "eval" / f"{name}.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


stats = load_module("stats")
digest = load_module("digest")


def record(task, arm, rep, cost, correct=True, outcome="pass", context=1000.0):
    return {
        "run_id": f"{task}__{arm}__r{rep}",
        "task": task,
        "arm": arm,
        "rep": rep,
        "outcome": outcome,
        "correct": correct,
        "first_attempt": correct,
        "metrics": {
            "cost_usd": cost,
            "context_tokens_total": context,
            "output_tokens": 100,
            "turns": 3,
            "tool_calls": 2,
            "api_calls": 3,
        },
    }


def write_runs(directory: Path, records) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "runs.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records), encoding="utf-8")


class StatsTests(unittest.TestCase):
    def test_ratios_are_paired_per_task_and_task_balanced(self):
        # ultra-edit costs 2x native on a cheap task and 0.5x on an expensive one: the
        # geometric mean of per-task ratios is 1.00 although the summed costs differ.
        runs = [
            record("cheap", "native", 1, 0.01),
            record("cheap", "ultra-edit", 1, 0.02),
            record("dear", "native", 1, 1.00),
            record("dear", "ultra-edit", 1, 0.50),
        ]
        with tempfile.TemporaryDirectory() as directory:
            write_runs(Path(directory), runs)
            text = stats.report([directory])
        row = next(line for line in text.splitlines() if line.startswith("| ultra-edit | 1.00"))
        self.assertIn("1.00 (1.00-1.00)", row)

    def test_unscored_runs_are_excluded_and_failures_counted(self):
        runs = [
            record("t", "native", 1, 0.1),
            record("t", "native", 2, 0.1, correct=False, outcome="fail"),
            record("t", "native", 3, 0.0, correct=False, outcome="infra_error"),
        ]
        with tempfile.TemporaryDirectory() as directory:
            write_runs(Path(directory), runs)
            text = stats.report([directory])
        self.assertIn("| native | 2 | 1/2 (50%) |", text)

    def test_a_suffix_keeps_two_builds_of_one_arm_apart(self):
        with tempfile.TemporaryDirectory() as directory:
            old, new = Path(directory) / "old", Path(directory) / "new"
            write_runs(old, [record("t", "native", 1, 0.1), record("t", "ultra-edit", 1, 0.2)])
            write_runs(new, [record("t", "ultra-edit", 1, 0.1)])
            text = stats.report([str(old), f"{new}:lean"])
        self.assertIn("| ultra-edit-lean | 1 |", text)
        self.assertIn("| ultra-edit | 1 |", text)
        versus_old = text.split("## Versus ultra-edit")[1]
        self.assertIn("| ultra-edit-lean | 0.50 (0.50-0.50)", versus_old)

    def test_wilson_interval_bounds(self):
        low, high = stats.wilson(42, 42)
        self.assertAlmostEqual(high, 1.0)
        self.assertGreater(low, 0.9)


class DigestTests(unittest.TestCase):
    def test_digest_lists_calls_results_and_the_byte_diff(self):
        stream = [
            {"type": "system", "subtype": "init", "tools": ["Edit"]},
            {
                "type": "assistant",
                "message": {
                    "id": "m1",
                    "usage": {"input_tokens": 5, "cache_read_input_tokens": 1000, "output_tokens": 9},
                    "content": [
                        {"type": "tool_use", "id": "t1", "name": "Edit", "input": {"old_string": "a"}}
                    ],
                },
            },
            {
                "type": "user",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "t1", "content": "not found", "is_error": True}
                    ]
                },
            },
            {
                "type": "result",
                "subtype": "success",
                "num_turns": 2,
                "total_cost_usd": 0.01,
                "result": "done",
            },
        ]
        with tempfile.TemporaryDirectory() as directory:
            results = Path(directory) / "results"
            run_dir = results / "runs" / "t__native__r1"
            run_dir.mkdir(parents=True)
            (run_dir / "stream.jsonl").write_text(
                "".join(json.dumps(event) + "\n" for event in stream), encoding="utf-8"
            )
            (run_dir / "diff.txt").write_text('-"x\\r\\n"\n+"x\\n"\n', encoding="utf-8")
            write_runs(results, [record("t", "native", 1, 0.01, correct=False, outcome="fail")])
            text = digest.digest(run_dir, json.loads((results / "runs.jsonl").read_text()))
        self.assertIn("RUN t__native__r1", text)
        self.assertIn("#1 ctx=1005 out=9", text)
        self.assertIn('CALL Edit (18B): {"old_string":"a"}', text)
        self.assertIn("-> ERROR (9B): not found", text)
        self.assertIn('-"x\\r\\n"', text)


if __name__ == "__main__":
    unittest.main()
