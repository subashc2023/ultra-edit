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
facts = load_module("facts")


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


def call(message_id, tool_id, name, tool_input, context=1000):
    return {
        "type": "assistant",
        "message": {
            "id": message_id,
            "usage": {"input_tokens": 0, "cache_read_input_tokens": context, "output_tokens": 10},
            "content": [{"type": "tool_use", "id": tool_id, "name": name, "input": tool_input}],
        },
    }


def answer(tool_id, text, is_error=False):
    return {
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": tool_id, "content": text, "is_error": is_error}]},
    }


def run_facts_for(events, arm="native", edit_tools=None):
    """Facts for one run whose transcript is `events`; context per API call comes from usage."""
    contexts = []
    for event in events:
        if event["type"] == "assistant" and event["message"]["id"] not in [c[0] for c in contexts]:
            contexts.append((event["message"]["id"], event["message"]["usage"]["cache_read_input_tokens"]))
    with tempfile.TemporaryDirectory() as directory:
        results = Path(directory)
        run_dir = results / "runs" / f"t__{arm}__r1"
        run_dir.mkdir(parents=True)
        (run_dir / "stream.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events), encoding="utf-8")
        rec = record("t", arm, 1, 0.1)
        rec["metrics"]["context_series"] = [[context, 10] for _, context in contexts]
        tools = edit_tools if edit_tools is not None else facts.harness.NATIVE_EDIT_TOOL_NAMES
        return facts.run_facts(rec, results, tools)


class FactsTests(unittest.TestCase):
    def test_a_failed_edit_and_its_recovery_are_costed_in_context_tokens(self):
        result = run_facts_for(
            [
                call("m1", "t1", "Read", {"file_path": "/r/a.py"}, 1000),
                answer("t1", "1\tx = 1"),
                call("m2", "t2", "Edit", {"file_path": "/r/a.py", "old_string": "y", "new_string": "z"}, 1100),
                answer("t2", "<tool_use_error>String to replace not found in file.</tool_use_error>", True),
                call("m3", "t3", "Edit", {"file_path": "/r/a.py", "old_string": "x", "new_string": "z"}, 1200),
                answer("t3", "The file /r/a.py has been updated."),
                call("m4", "t4", "Bash", {"command": "git diff a.py | cat -A"}, 1300),
                answer("t4", "-x = 1$\n+z = 1$"),
            ]
        )
        self.assertEqual([s["kind"] for s in result["steps"]], ["read", "edit", "edit", "verify"])
        self.assertEqual(result["edit_failures"], 1)
        self.assertEqual(result["errors"], {"not_found": 1})
        self.assertEqual(
            result["recoveries"],
            [{"step": 1, "error": "not_found", "masked": False, "fixed_at": 2, "steps_between": 0, "context_tokens": 1200}],
        )
        self.assertTrue(result["post_edit"]["diff_content"])
        self.assertIn("byte_view", result["post_edit"]["features"])

    def test_a_script_failure_hidden_by_a_later_command_is_a_masked_failed_edit(self):
        script = "python3 - <<'EOF'\nimport pathlib\np = pathlib.Path('a.py')\ns = p.read_text()\nassert s.count('y') == 1\np.write_text(s)\nEOF\ngit diff --stat"
        result = run_facts_for(
            [
                call("m1", "t1", "Bash", {"command": script}),
                answer("t1", "Traceback (most recent call last):\n  File \"<stdin>\", line 4\nAssertionError"),
            ],
            arm="shell-python",
        )
        step = result["steps"][0]
        self.assertEqual((step["channel"], step["error"], step["masked"], step["failed"]), ("bash:python", "python:AssertionError", True, True))
        self.assertEqual(result["masked_failures"], 1)

    def test_a_patch_built_with_sed_is_attributed_to_git_apply_and_a_missing_viewer_does_not_fail_it(self):
        command = "sed -i 's/a/b/' $T/b/x.py\ngit apply <<'PATCH'\n--- a/x.py\n+++ b/x.py\nPATCH\nxxd x.py | head"
        result = run_facts_for(
            [
                call("m1", "t1", "Bash", {"command": command}),
                answer("t1", "Exit code 127\n x.py | 2 +-\n/bin/bash: line 7: xxd: command not found", True),
            ],
            arm="shell-patch",
        )
        step = result["steps"][0]
        self.assertEqual((step["channel"], step["error"], step["failed"]), ("bash:git_apply", "command_not_found", False))
        self.assertEqual(result["edit_failures"], 0)

    def test_an_error_inside_a_successful_mcp_result_is_masked(self):
        tool = "mcp__text-editor__patch_text_file_contents"
        payload = json.dumps({"result": "error", "reason": "Content range hash mismatch", "file_hash": None})
        result = run_facts_for(
            [call("m1", "t1", tool, {"file_path": "/r/a.py", "patches": []}), answer("t1", payload)],
            arm="mcp-text-editor",
            edit_tools=(tool,),
        )
        step = result["steps"][0]
        self.assertEqual(step["error"], "text_editor:content_range_hash_mismatch")
        self.assertTrue(step["masked"] and step["failed"])


if __name__ == "__main__":
    unittest.main()
