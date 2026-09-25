"""Offline tests for the model-level evaluation harness in eval/run_eval.py.

None of these tests start Claude Code, use the network, or need credentials.
End-to-end runner tests launch a small fake `claude` script instead.
"""

from __future__ import annotations

import base64
import importlib.util
import io
import json
import os
import shutil
import stat
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path, PureWindowsPath
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "eval" / "run_eval.py"
SPEC = importlib.util.spec_from_file_location("ultra_edit_eval", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
evaluation = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = evaluation
SPEC.loader.exec_module(evaluation)

TASKS_DIR = REPO_ROOT / "eval" / "tasks"
ULTRA = evaluation.ULTRA_TOOL_PREFIX
HAS_GIT = shutil.which("git") is not None


# ---------------------------------------------------------------------------
# Synthetic stream-json builders


def init_message(tools=("Read", "Edit", "Write", "Bash"), mcp_servers=(), plugins=(), **extra):
    message = {
        "type": "system",
        "subtype": "init",
        "session_id": "session",
        "uuid": "init",
        "cwd": "/tmp/repo",
        "tools": list(tools),
        "mcp_servers": list(mcp_servers),
        "model": "claude-test",
        "permissionMode": "bypassPermissions",
        "claude_code_version": "2.1.282",
        "plugins": list(plugins),
        "slash_commands": [],
        "skills": [],
        "apiKeySource": "none",
        "output_style": "default",
    }
    message.update(extra)
    return message


def assistant(message_id, *blocks, parent=None, error=None):
    message = {
        "type": "assistant",
        "uuid": f"a-{message_id}",
        "session_id": "session",
        "parent_tool_use_id": parent,
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": list(blocks),
            "stop_reason": None,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    }
    if error:
        message["error"] = error
    return message


def tool_use(tool_id, name, tool_input):
    return {"type": "tool_use", "id": tool_id, "name": name, "input": tool_input}


def tool_result(tool_id, content, is_error=False, structured=None, parent=None):
    message = {
        "type": "user",
        "uuid": f"u-{tool_id}",
        "session_id": "session",
        "parent_tool_use_id": parent,
        "message": {
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": tool_id, "content": content, "is_error": is_error}
            ],
        },
    }
    if structured is not None:
        message["tool_use_result"] = structured
    return message


def hook_response(event, outcome="success", stdout="", exit_code=0):
    return {
        "type": "system",
        "subtype": "hook_response",
        "hook_id": f"hook-{event}",
        "hook_name": f"{event}:Bash",
        "hook_event": event,
        "output": stdout,
        "stdout": stdout,
        "stderr": "",
        "exit_code": exit_code,
        "outcome": outcome,
        "uuid": "h",
        "session_id": "session",
    }


def result_message(**overrides):
    message = {
        "type": "result",
        "subtype": "success",
        "uuid": "result",
        "session_id": "session",
        "duration_ms": 12000,
        "duration_api_ms": 9000,
        "is_error": False,
        "num_turns": 7,
        "result": "Done.",
        "stop_reason": "end_turn",
        "total_cost_usd": 0.1234,
        "usage": {
            "input_tokens": 10,
            "output_tokens": 500,
            "cache_creation_input_tokens": 2000,
            "cache_read_input_tokens": 30000,
        },
        "modelUsage": {
            "claude-test": {
                "inputTokens": 12,
                "outputTokens": 600,
                "cacheReadInputTokens": 31000,
                "cacheCreationInputTokens": 2100,
                "webSearchRequests": 0,
                "costUSD": 0.12,
                "contextWindow": 200000,
                "maxOutputTokens": 64000,
            },
            "claude-haiku-test": {
                "inputTokens": 100,
                "outputTokens": 20,
                "cacheReadInputTokens": 0,
                "cacheCreationInputTokens": 0,
                "webSearchRequests": 0,
                "costUSD": 0.0034,
                "contextWindow": 200000,
                "maxOutputTokens": 8192,
            },
        },
        "permission_denials": [],
    }
    message.update(overrides)
    return message


def transcript_from(*messages):
    return evaluation.parse_stream(json.dumps(message) for message in messages)


def ultra_json(**fields):
    return json.dumps(fields)


class StreamParsingTests(unittest.TestCase):
    def test_native_edit_transcript_counts_calls_errors_turns_and_tokens(self):
        transcript = transcript_from(
            init_message(),
            # Claude Code streams one content block per event; both share a message id.
            assistant("msg_1", {"type": "text", "text": "Reading the file."}),
            assistant("msg_1", tool_use("t1", "Read", {"file_path": "VERSION"})),
            tool_result("t1", "2.3.1"),
            assistant("msg_2", tool_use("t2", "Edit", {"file_path": "VERSION", "old_string": "2.3.2"})),
            tool_result(
                "t2", "<tool_use_error>String to replace not found in file.</tool_use_error>", is_error=True
            ),
            assistant("msg_3", tool_use("t3", "Edit", {"file_path": "VERSION", "old_string": "2.3.1"})),
            tool_result("t3", [{"type": "text", "text": "The file VERSION has been updated."}]),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["tool_calls"], 3)
        self.assertEqual(metrics["tool_calls_by_name"], {"Edit": 2, "Read": 1})
        self.assertEqual(metrics["tool_errors"], 1)
        self.assertEqual(metrics["edit_calls"], 2)
        self.assertEqual(metrics["edit_failures"], 1)
        self.assertEqual(metrics["turns"], 7)
        self.assertEqual(transcript.assistant_message_ids, ["msg_1", "msg_2", "msg_3"])
        # modelUsage covers every model; usage alone would miss the helper model.
        self.assertEqual(metrics["token_source"], "modelUsage")
        self.assertEqual(metrics["input_tokens"], 112)
        self.assertEqual(metrics["output_tokens"], 620)
        self.assertEqual(metrics["cache_read_input_tokens"], 31000)
        self.assertEqual(metrics["cache_creation_input_tokens"], 2100)
        self.assertEqual(metrics["total_input_tokens"], 112 + 31000 + 2100)
        self.assertAlmostEqual(metrics["cost_usd"], 0.1234)
        self.assertEqual(metrics["models"], ["claude-haiku-test", "claude-test"])
        self.assertFalse(evaluation.first_attempt(metrics, correct=True))

    def test_bash_heredoc_write_is_counted_and_read_only_bash_is_not(self):
        heredoc = "cat > build/release.sh <<'EOF'\n#!/bin/sh\nAPP_VERSION=2.4.0\nEOF"
        transcript = transcript_from(
            init_message(),
            assistant("m1", tool_use("t1", "Bash", {"command": "git diff --stat"})),
            tool_result("t1", " 1 file changed"),
            assistant("m2", tool_use("t2", "Bash", {"command": heredoc})),
            tool_result("t2", ""),
            result_message(num_turns=3),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["bash_write_attempts"], 1)
        self.assertEqual(metrics["bash_writes"], 1)
        self.assertEqual(metrics["bash_write_kinds"], {"heredoc_write": 1})
        self.assertEqual(metrics["edit_calls"], 1)
        self.assertEqual(metrics["tool_errors"], 0)
        self.assertTrue(evaluation.first_attempt(metrics, correct=True))

    def test_guard_denial_is_an_error_result_and_a_hook_denial(self):
        deny = json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "use Ultra Edit",
                }
            }
        )
        transcript = transcript_from(
            init_message(),
            assistant("m1", tool_use("t1", "Bash", {"command": "sed -i 's/2.3.1/2.4.0/' VERSION"})),
            hook_response("PreToolUse", stdout=deny),
            tool_result("t1", "PreToolUse:Bash hook blocked: use Ultra Edit", is_error=True),
            result_message(permission_denials=[{"tool_name": "Bash", "tool_use_id": "t1", "tool_input": {}}]),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["bash_write_attempts"], 1)
        self.assertEqual(metrics["bash_writes"], 0)
        self.assertEqual(metrics["bash_writes_blocked"], 1)
        self.assertEqual(metrics["hook_denials"], 1)
        self.assertEqual(metrics["permission_denials"], 1)
        self.assertEqual(metrics["tool_errors"], 1)
        self.assertEqual(metrics["edit_failures"], 1)

    def test_namespaced_ultra_edit_calls_and_rejections(self):
        snapshot = ULTRA + "ultra_edit_snapshot"
        edit = ULTRA + "ultra_edit"
        status = ULTRA + "ultra_edit_status"
        transcript = transcript_from(
            init_message(tools=("Read", snapshot, edit, status)),
            assistant("m1", tool_use("t1", snapshot, {"path": "VERSION", "selection": {"kind": "full"}})),
            tool_result("t1", [{"type": "text", "text": ultra_json(snapshot="s1", text="2.3.1")}]),
            assistant("m2", tool_use("t2", edit, {"request_id": "r1", "files": []})),
            tool_result(
                "t2",
                [{"type": "text", "text": ultra_json(kind="rejected", ready=False, diagnostic_count=1)}],
                is_error=True,
            ),
            assistant("m3", tool_use("t3", edit, {"request_id": "r2", "files": []})),
            tool_result("t3", ultra_json(kind="completed", request_id="r2", commit="committed")),
            assistant("m4", tool_use("t4", status, {"query": {"kind": "receipt", "request_id": "r0"}})),
            # A read-only receipt query that reports an old partial commit is not a failed call.
            tool_result("t4", ultra_json(kind="completed", request_id="r0", commit="partial")),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["tool_calls_by_name"], {snapshot: 1, edit: 2, status: 1})
        self.assertEqual(metrics["ultra_edit_statuses"], {"ok": 3, "rejected": 1})
        self.assertEqual(metrics["ultra_rejections"], 1)
        self.assertEqual(metrics["edit_calls"], 2)
        self.assertEqual(metrics["edit_failures"], 1)
        self.assertEqual(metrics["tool_errors"], 1)
        self.assertFalse(evaluation.first_attempt(metrics, correct=True))
        self.assertEqual(evaluation.short_tool_name(edit), "mcp:ultra_edit")
        self.assertEqual(evaluation.ultra_tool(edit), "ultra_edit")
        self.assertIsNone(evaluation.ultra_tool("mcp__ultrasearch__ultra_edit"))
        self.assertIsNone(evaluation.ultra_tool("Edit"))

    def test_non_committed_edit_detected_from_content_or_structured_result(self):
        edit = ULTRA + "ultra_edit"
        partial = evaluation.ToolResult(True, ultra_json(kind="completed", commit="partial"))
        self.assertEqual(evaluation.ultra_result_status("ultra_edit", partial), "commit:partial")
        unknown = evaluation.ToolResult(
            False, "result: " + ultra_json(kind="completed", commit="outcome_unknown")
        )
        self.assertEqual(evaluation.ultra_result_status("ultra_edit", unknown), "commit:outcome_unknown")
        structured = evaluation.ToolResult(
            False, "see structured content", {"kind": "rejected", "ready": False}
        )
        self.assertEqual(evaluation.ultra_result_status("ultra_edit", structured), "rejected")
        error = evaluation.ToolResult(True, ultra_json(kind="error", error={"code": "STALE_BASE"}))
        self.assertEqual(evaluation.ultra_result_status("ultra_edit_snapshot", error), "error")
        self.assertEqual(evaluation.ultra_result_status("ultra_edit", None), "missing")
        transcript = transcript_from(
            init_message(),
            assistant("m1", tool_use("t1", edit, {})),
            tool_result("t1", "done", structured={"kind": "completed", "commit": "not_committed"}),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["ultra_edit_statuses"], {"commit:not_committed": 1})
        self.assertEqual(metrics["edit_failures"], 1)

    def test_error_results_subagents_orphans_and_bad_lines(self):
        lines = [
            json.dumps(init_message()),
            "not json at all",
            json.dumps(assistant("m1", tool_use("t1", "Task", {"prompt": "edit"}))),
            json.dumps(assistant("m2", tool_use("t2", "Edit", {}), parent="t1")),
            json.dumps(tool_result("t2", "ok", parent="t1")),
            json.dumps(tool_result("t9", "orphan")),
            json.dumps(
                result_message(subtype="error_max_turns", is_error=True, num_turns=50, errors=["max turns"])
            ),
        ]
        transcript = evaluation.parse_stream(lines)
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(transcript.parse_errors, 1)
        self.assertEqual(transcript.orphan_results, 1)
        self.assertEqual(metrics["subagent_tool_calls"], 1)
        self.assertEqual(metrics["unanswered_tool_calls"], 1)
        self.assertEqual(metrics["result_subtype"], "error_max_turns")
        # Subagent messages do not count as main-thread turns in the fallback.
        self.assertEqual(transcript.assistant_message_ids, ["m1"])

    def test_usage_fallback_and_missing_result(self):
        transcript = transcript_from(init_message(), result_message(modelUsage={}, total_cost_usd=0.5))
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["token_source"], "usage")
        self.assertEqual(metrics["total_input_tokens"], 10 + 30000 + 2000)
        self.assertEqual(metrics["cost_usd"], 0.5)
        empty = evaluation.compute_metrics(transcript_from(init_message()))
        self.assertIsNone(empty["cost_usd"])
        self.assertIsNone(empty["total_input_tokens"])
        self.assertEqual(empty["turns"], 0)


class ShellHeuristicTests(unittest.TestCase):
    def test_write_kinds(self):
        cases = [
            ("cat > src/a.py <<'EOF'\nprint('x > y')\nEOF", "Bash", ["heredoc_write"]),
            ('cat <<EOF >> "docs/my notes.md"\nhello\nEOF', "Bash", ["heredoc_write"]),
            (
                "tee Makefile <<'EOF' >/dev/null\n\t$(GO) vet ./...\nEOF",
                "Bash",
                ["heredoc_write", "tee_write"],
            ),
            ("git apply <<'EOF'\n--- a/x\n+++ b/x\nEOF", "Bash", ["heredoc_write"]),
            ("grep -c x <<'A'\nx\nA\ncat > out.txt <<'B'\ny\nB", "Bash", ["heredoc_write"]),
            ("git commit -F - <<'EOF'\nsubject > body\nEOF", "Bash", []),
            ("git commit -F - <<'EOF'\ndocs: explain cat <<X > file\nEOF", "Bash", []),
            ("git commit -m \"$(cat <<'EOF'\nfix: a > b\nEOF\n)\"", "Bash", []),
            ("sed -i 's/2.3.1/2.4.0/' VERSION", "Bash", ["in_place"]),
            ("sed -i.bak -e 's/a/b/' file", "Bash", ["in_place"]),
            ("sed -n '1,5p' file && sed --version", "Bash", []),
            ("perl -pi -e 's/a/b/' file", "Bash", ["in_place"]),
            ("echo 2.4.0 > VERSION", "Bash", ["echo_redirect"]),
            ("printf '%s\\n' 'SIGN_BUILD=1' >> build/release.sh", "Bash", ["echo_redirect"]),
            ('echo "a > b"', "Bash", []),
            ("echo done 2>/dev/null; ls > /dev/null", "Bash", []),
            ("grep -rn MAX_RETRIES . 2>&1 | head", "Bash", []),
            ("cat a.txt | tee b.txt", "Bash", ["tee_write"]),
            ("make test | tee /dev/null", "Bash", []),
            (
                "python3 - <<'EOF'\nfrom pathlib import Path\nPath('VERSION').write_text('2.4.0')\nEOF",
                "Bash",
                ["inline_script"],
            ),
            ("python3 -c \"open('f.txt', 'w').write('x')\"", "Bash", ["inline_script"]),
            ("python3 -c \"import sys; sys.stdout.write('hi')\"", "Bash", []),
            ("python3 -m pytest -q", "Bash", []),
            ("node -e \"require('fs').writeFileSync('a', 'b')\"", "Bash", ["inline_script"]),
            ("cat <<< 'text' | wc -c", "Bash", []),
            (
                "Set-Content -LiteralPath VERSION -Value '2.4.0' -NoNewline",
                "PowerShell",
                ["powershell_write"],
            ),
            ("'2.4.0' | Out-File VERSION", "PowerShell", ["powershell_write"]),
            ("Get-Content VERSION", "PowerShell", []),
            ("pwsh -Command \"Set-Content a.txt 'x'\"", "Bash", ["powershell_write"]),
            ("grep -rn Set-Content scripts", "Bash", []),
            ("", "Bash", []),
            (None, "Bash", []),
        ]
        for command, tool, expected in cases:
            with self.subTest(command=command):
                self.assertEqual(evaluation.shell_write_kinds(command, tool), expected)


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_crlf_and_trailing_space_differences_are_mismatches(self):
        expected = {"build/release.cmd": b"set A=1\r\nset B=2\r\n", "docs/a.md": b"line  \n"}
        actual = {"build/release.cmd": b"set A=1\r\nset B=2\n", "docs/a.md": b"line\n"}
        comparison = evaluation.compare_trees(expected, actual)
        self.assertFalse(comparison.correct)
        self.assertEqual(comparison.mismatched, ["build/release.cmd", "docs/a.md"])
        report = evaluation.describe_differences(expected, actual, comparison)
        self.assertIn('-"set B=2\\r\\n"', report)
        self.assertIn('+"set B=2\\n"', report)
        self.assertIn('-"line  \\n"', report)

    def test_extra_and_missing_files(self):
        expected = {"a.txt": b"a", "b.txt": b"b"}
        actual = {"a.txt": b"a", "a.txt.bak": b"a", "c.txt": b"c"}
        comparison = evaluation.compare_trees(expected, actual)
        self.assertEqual(comparison.missing, ["b.txt"])
        self.assertEqual(comparison.unexpected, ["a.txt.bak", "c.txt"])
        self.assertEqual(comparison.mismatched, [])
        self.assertIn("missing: b.txt", comparison.reason())
        self.assertTrue(evaluation.compare_trees(expected, dict(expected)).correct)

    def test_read_tree_ignores_git_and_state_only_at_top_level(self):
        files = {
            "a.txt": b"one\r\ntwo\r\n",
            "sub/b.txt": b"\tindented\n",
            ".git/HEAD": b"ref: refs/heads/main\n",
            ".ultra-edit/journal": b"state",
            "sub/.git/config": b"nested repositories are real changes",
        }
        evaluation.write_tree(self.root, files)
        tree = evaluation.read_tree(self.root, evaluation.IGNORED_TOP_LEVEL)
        self.assertEqual(
            tree,
            {
                "a.txt": b"one\r\ntwo\r\n",
                "sub/.git/config": files["sub/.git/config"],
                "sub/b.txt": b"\tindented\n",
            },
        )
        self.assertEqual(len(evaluation.read_tree(self.root)), 5)


def record(task, arm, rep, outcome, correct, first=None, **metrics):
    base = {
        "tool_calls": 10,
        "tool_calls_by_name": {"Edit": 2, "Read": 3},
        "tool_errors": 0,
        "edit_calls": 2,
        "edit_failures": 0,
        "bash_write_attempts": 0,
        "bash_writes": 0,
        "turns": 8,
        "total_input_tokens": 40000,
        "output_tokens": 900,
        "cost_usd": 0.1,
        "result_subtype": "success",
        "api_errors": [],
        "models": ["claude-test"],
    }
    base.update(metrics)
    return {
        "run_id": f"{task}__{arm}__r{rep}",
        "task": task,
        "arm": arm,
        "rep": rep,
        "outcome": outcome,
        "correct": correct,
        "first_attempt": correct if first is None else first,
        "timed_out": outcome == "timeout",
        "exit_code": 0,
        "wall_s": 30.0,
        "comparison": {"mismatched": [] if correct else ["VERSION"], "missing": [], "unexpected": []},
        "integrity": {
            "errors": ["Ultra Edit plugin loaded in the native arm"] if outcome == "invalid" else [],
            "warnings": [],
        },
        "metrics": base,
        "init": {"claude_code_version": "2.1.282", "model": "claude-test"},
        "files": {"dir": f"runs/{task}__{arm}__r{rep}"},
    }


class SummaryTests(unittest.TestCase):
    def records(self):
        return [
            record("t1", "native", 1, "pass", True, tool_calls=12, cost_usd=0.2),
            record(
                "t1",
                "native",
                2,
                "fail",
                False,
                tool_calls=8,
                cost_usd=0.1,
                bash_write_attempts=2,
                bash_writes=1,
            ),
            record("t1", "native", 3, "invalid", False, tool_calls=99, cost_usd=9.0),
            record(
                "t1", "ultra-edit", 1, "pass", True, first=False, tool_calls_by_name={ULTRA + "ultra_edit": 2}
            ),
            record("t2", "ultra-edit", 1, "pass", True),
            record("t2", "native-guard", 1, "timeout", False),
            record("t2", "native-guard", 2, "infra_error", False, api_errors=["overloaded"]),
        ]

    def test_aggregate_excludes_invalid_and_infrastructure_runs(self):
        native = evaluation.aggregate([r for r in self.records() if r["arm"] == "native"])
        self.assertEqual((native["runs"], native["scored"], native["correct"]), (3, 2, 1))
        self.assertEqual(native["tool_calls"], 10.0)
        self.assertAlmostEqual(native["cost_usd"], 0.15)
        self.assertEqual((native["bash_writes"], native["bash_write_attempts"]), (1, 2))
        self.assertEqual(native["invalid"], 1)
        guard = evaluation.aggregate([r for r in self.records() if r["arm"] == "native-guard"])
        self.assertEqual(
            (guard["scored"], guard["correct"], guard["timeout"], guard["infra_error"]), (1, 0, 1, 1)
        )

    def test_markdown_tables_and_sections(self):
        summary = evaluation.summarize(self.records())
        self.assertIn("## By arm", summary)
        self.assertIn("| native | 2 | 1/2 (50%) | 1/2 (50%) | 10.0 |", summary)
        self.assertIn("| ultra-edit | 2 | 2/2 (100%) | 1/2 (50%) |", summary)
        self.assertIn("| t1 / ultra-edit | 1 | 1/1 (100%) | 0/1 (0%) |", summary)
        self.assertIn("| mcp:ultra_edit |", summary)
        self.assertIn("`t1__native__r2`: wrong bytes: VERSION", summary)
        self.assertIn("`t2__native-guard__r1`: timed out;", summary)
        self.assertIn("`t1__native__r3` (invalid): Ultra Edit plugin loaded in the native arm", summary)
        self.assertIn("`t2__native-guard__r2` (infra_error): overloaded", summary)
        # Arms keep the canonical order in tables.
        self.assertLess(summary.index("| native |"), summary.index("| native-guard |"))
        self.assertLess(summary.index("| native-guard |"), summary.index("| ultra-edit |"))

    def test_summarize_option_rebuilds_summary_from_runs_jsonl(self):
        with tempfile.TemporaryDirectory() as directory:
            results = Path(directory)
            with open(results / "runs.jsonl", "w", encoding="utf-8") as handle:
                for item in self.records():
                    handle.write(json.dumps(item) + "\n")
            output = io.StringIO()
            with redirect_stdout(output):
                self.assertEqual(evaluation.main(["--summarize", str(results)]), 0)
            written = (results / "summary.md").read_text(encoding="utf-8")
            self.assertEqual(written, evaluation.summarize(self.records()))
            self.assertIn("## By task and arm", output.getvalue())


SAMPLE_HELP = """Usage: claude [options] [command] [prompt]
  -p, --print                           Print response and exit
  --output-format <format>              Output format
  --verbose                             Override verbose mode setting
  --settings <file-or-json>             Path to a settings JSON file
  --setting-sources <sources>           Comma-separated list of setting sources
  --plugin-dir <path>                   Load a plugin from a directory
  --permission-mode <mode>              Permission mode
  --no-session-persistence              Disable session persistence
  --max-budget-usd <amount>             Maximum dollar amount
  --model <model>                       Model for the current session
"""


class CommandTests(unittest.TestCase):
    windows_claude = r"C:\Users\Dev Name\.local\bin\claude.exe"
    windows_settings = PureWindowsPath(
        r"C:\Users\Dev Name\AppData\Local\Temp\ue eval\runs\t__native__r1\settings.json"
    )
    windows_plugin = PureWindowsPath(r"C:\Users\Dev Name\ultra-edit\eval\results\20260925-120000\plugin")
    windows_guard = str(windows_plugin / "runtime" / "ultra-edit-mcp.exe")

    def command(self, arm, **options):
        return evaluation.build_command(
            [self.windows_claude],
            arm=arm,
            settings_path=self.windows_settings,
            plugin_dir=self.windows_plugin,
            **options,
        )

    def test_each_arm_with_windows_paths(self):
        for arm in evaluation.ARMS:
            with self.subTest(arm=arm):
                argv = self.command(arm, model="claude-sonnet-4-5", max_budget_usd=2.0, max_turns=40)
                self.assertEqual(argv[:2], [self.windows_claude, "-p"])
                self.assertIn("--setting-sources=", argv)
                self.assertEqual(argv[argv.index("--settings") + 1], str(self.windows_settings))
                self.assertEqual(argv[argv.index("--output-format") + 1], "stream-json")
                self.assertIn("--verbose", argv)
                self.assertIn("--no-session-persistence", argv)
                self.assertEqual(argv[argv.index("--permission-mode") + 1], "bypassPermissions")
                self.assertEqual(argv[argv.index("--max-turns") + 1], "40")
                self.assertEqual(argv[argv.index("--max-budget-usd") + 1], "2")
                self.assertEqual(argv[argv.index("--model") + 1], "claude-sonnet-4-5")
                self.assertNotIn("--strict-mcp-config", argv)
                self.assertNotIn("--allowedTools", argv)
                if arm == "ultra-edit":
                    self.assertEqual(argv[argv.index("--plugin-dir") + 1], str(self.windows_plugin))
                else:
                    self.assertNotIn("--plugin-dir", argv)
                display = evaluation.format_command(argv, windows=True)
                self.assertIn(r'"C:\Users\Dev Name\.local\bin\claude.exe" -p', display)
                self.assertIn(r'--settings "C:\Users\Dev Name\AppData\Local\Temp\ue eval\runs', display)

    def test_settings_per_arm(self):
        native = evaluation.build_settings("native")
        self.assertNotIn("hooks", native)
        self.assertIs(native["disableClaudeAiConnectors"], True)
        self.assertIs(native["syncClaudeAiPlugins"], False)
        self.assertEqual(
            native["enabledPlugins"], {"ultra-edit@ultra-edit": False, "ultra-edit@skills-dir": False}
        )
        self.assertEqual(evaluation.build_settings("ultra-edit"), native)
        guard = evaluation.build_settings("native-guard", self.windows_guard)
        self.assertEqual(
            guard["hooks"],
            {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {
                                "type": "command",
                                "command": r"C:\Users\Dev Name\ultra-edit\eval\results\20260925-120000"
                                r"\plugin\runtime\ultra-edit-mcp.exe",
                                "args": ["--claude-hook", "PreToolUse"],
                                "timeout": evaluation.GUARD_HOOK_TIMEOUT_S,
                            }
                        ],
                    }
                ]
            },
        )
        with self.assertRaises(evaluation.EvalError):
            evaluation.build_settings("native-guard")
        merged = evaluation.build_settings(
            "native-guard",
            "/opt/ue/ultra-edit-mcp",
            "Bash|PowerShell",
            {"env": {"A": "1"}, "hooks": {"PreToolUse": [{"matcher": "Edit"}]}},
        )
        self.assertEqual(merged["env"], {"A": "1"})
        self.assertEqual(
            [entry["matcher"] for entry in merged["hooks"]["PreToolUse"]], ["Bash|PowerShell", "Edit"]
        )

    def test_prompt_argument_follows_print_flag_and_optional_flags_follow_help(self):
        info = evaluation.ClaudeInfo(
            ["claude"], (2, 1, 250), "2.1.250 (Claude Code)", evaluation.flags_from_help(SAMPLE_HELP), True
        )
        argv = evaluation.build_command(
            ["claude"],
            arm="native",
            settings_path="/tmp/s.json",
            permission_mode="acceptEdits",
            prompt="Rename\n`A` to `B`.",
            effort="high",
            claude=info,
        )
        self.assertEqual(argv[:3], ["claude", "-p", "Rename\n`A` to `B`."])
        self.assertNotIn("--permission-prompts", argv)
        self.assertNotIn("--include-hook-events", argv)
        self.assertNotIn("--effort", argv)
        self.assertEqual(argv[argv.index("--allowedTools") + 1], ",".join(evaluation.DEFAULT_ALLOWED_TOOLS))
        self.assertIn("--max-budget-usd", evaluation.flags_from_help(SAMPLE_HELP))
        full = evaluation.ClaudeInfo(["claude"], probed=False)
        stdin_argv = evaluation.build_command(["claude"], arm="native", settings_path="s.json", claude=full)
        self.assertEqual(stdin_argv[stdin_argv.index("--permission-prompts") + 1], "none")
        self.assertIn("--include-hook-events", stdin_argv)
        self.assertEqual(stdin_argv[2], "--output-format")
        with self.assertRaises(evaluation.EvalError):
            evaluation.build_command(["claude"], arm="ultra-edit", settings_path="s.json")

    def test_version_and_required_flags(self):
        self.assertEqual(evaluation.parse_version("2.1.282 (Claude Code)"), (2, 1, 282))
        self.assertIsNone(evaluation.parse_version("claude"))
        good = evaluation.ClaudeInfo(
            ["claude"], (2, 1, 282), "2.1.282", evaluation.flags_from_help(SAMPLE_HELP), True
        )
        self.assertEqual(evaluation.claude_problems(good), [])
        old = evaluation.ClaudeInfo(["claude"], (2, 1, 100), "2.1.100", good.flags, True)
        self.assertIn("older than 2.1.139", evaluation.claude_problems(old)[0])
        bare = evaluation.ClaudeInfo(["claude"], (2, 1, 282), "2.1.282", frozenset({"--print"}), True)
        self.assertIn("--setting-sources", evaluation.claude_problems(bare)[0])

    def test_child_environment(self):
        base = {
            "PATH": "/usr/bin",
            "ANTHROPIC_API_KEY": "sk-test",
            "CLAUDECODE": "1",
            "CLAUDE_CODE_PLUGIN_DIRS": "/somewhere/ultra-edit",
            "GIT_DIR": "/elsewhere/.git",
        }
        env, removed = evaluation.build_env(base, PureWindowsPath(r"C:\t\claude-config"))
        self.assertEqual(sorted(removed), ["CLAUDECODE", "CLAUDE_CODE_PLUGIN_DIRS", "GIT_DIR"])
        self.assertEqual(env["ANTHROPIC_API_KEY"], "sk-test")
        self.assertEqual(env["DISABLE_AUTOUPDATER"], "1")
        self.assertEqual(env["ENABLE_CLAUDEAI_MCP_SERVERS"], "false")
        self.assertEqual(env["CLAUDE_CONFIG_DIR"], r"C:\t\claude-config")
        self.assertNotIn("CLAUDECODE", env)
        self.assertEqual(base["CLAUDECODE"], "1")

    def test_plan_interleaves_and_shuffles_arm_order_deterministically(self):
        tasks = [evaluation.Task(name, Path(name), "p", {"a": b"1"}, {"a": b"2"}) for name in ("t1", "t2")]
        plan = evaluation.plan_runs(tasks, evaluation.ARMS, 2, seed=7)
        self.assertEqual(len(plan), 12)
        self.assertEqual([spec.task.name for spec in plan[:3]], ["t1"] * 3)
        self.assertEqual(sorted(spec.arm for spec in plan[:3]), sorted(evaluation.ARMS))
        self.assertEqual(
            [spec.run_id for spec in plan],
            [spec.run_id for spec in evaluation.plan_runs(tasks, evaluation.ARMS, 2, seed=7)],
        )
        fixed = evaluation.plan_runs(tasks, evaluation.ARMS, 1, seed=None)
        self.assertEqual([spec.arm for spec in fixed[:3]], list(evaluation.ARMS))

    def test_dry_run_prints_each_arm_and_starts_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "results"
            output = io.StringIO()
            with redirect_stdout(output):
                code = evaluation.main(
                    [
                        "--dry-run",
                        "--task",
                        "crlf-and-lf",
                        "--out",
                        str(out),
                        "--claude",
                        str(Path(directory) / "missing" / "claude"),
                        "--model",
                        "sonnet",
                    ]
                )
            text = output.getvalue()
            self.assertEqual(code, 0)
            self.assertFalse(out.exists())
            for arm in evaluation.ARMS:
                self.assertIn(f"== {arm} (example: crlf-and-lf__{arm}__r1) ==", text)
            self.assertIn('"--claude-hook"', text)
            self.assertIn("--plugin-dir", text)
            self.assertIn("--model sonnet", text)
            self.assertIn("stdin: ", text)
            self.assertEqual(text.count("crlf-and-lf / "), 3)


class TaskFixtureTests(unittest.TestCase):
    def test_all_tasks_are_well_formed(self):
        tasks = evaluation.discover_tasks(TASKS_DIR)
        self.assertEqual(
            [task.name for task in tasks],
            [
                "crlf-and-lf",
                "large-file-two-regions",
                "markdown-hard-breaks",
                "rename-constant",
                "tabs-makefile-go",
                "windows-paths-escapes",
            ],
        )
        for task in tasks:
            with self.subTest(task=task.name):
                self.assertEqual(evaluation.validate_task(task), [])
                self.assertTrue(task.prompt.strip())
                for relative, expected in task.expected.items():
                    self.assertIn(relative, task.fixture)
                    self.assertNotEqual(task.fixture[relative], expected)
                    self.assertIn(f"`{relative}`", task.prompt, "the prompt names every file that changes")
                    self.assert_same_style(task.fixture[relative], expected, relative)

    def assert_same_style(self, before, after, relative):
        """Edits never change a file's newline convention, BOM, or final newline."""
        for data in (before, after):
            self.assertEqual(data.count(b"\r"), data.count(b"\r\n"), f"{relative}: stray CR")
        crlf_before, lf_before = before.count(b"\r\n"), before.count(b"\n") - before.count(b"\r\n")
        crlf_after, lf_after = after.count(b"\r\n"), after.count(b"\n") - after.count(b"\r\n")
        self.assertEqual((crlf_before > 0, lf_before > 0), (crlf_after > 0, lf_after > 0), relative)
        self.assertEqual(before.startswith(b"\xef\xbb\xbf"), after.startswith(b"\xef\xbb\xbf"), relative)
        self.assertEqual(before.endswith(b"\n"), after.endswith(b"\n"), relative)

    def test_fixtures_cover_the_byte_hazards(self):
        tasks = {task.name: task for task in evaluation.discover_tasks(TASKS_DIR)}
        rename = tasks["rename-constant"]
        self.assertGreaterEqual(len(rename.expected), 3)
        self.assertTrue(any(path.endswith(".md") for path in rename.expected))
        self.assertIn(b"MAX_RETRIES_PER_HOST", rename.expected["src/httpkit/settings.py"])
        paths = tasks["windows-paths-escapes"]
        self.assertIn(b'"D:\\\\AcmeData\\\\logs\\\\current"', paths.expected["config/agent.json"])
        self.assertIn(b"\\[([A-Z]+)\\]", paths.expected["agent/paths.py"])
        json.loads(paths.expected["config/agent.json"].decode("utf-8"))
        crlf = tasks["crlf-and-lf"]
        self.assertIn(b"set SIGN_BUILD=1\r\n", crlf.expected["build/release.cmd"])
        self.assertNotIn(b"\r", crlf.expected["build/release.sh"])
        self.assertEqual(crlf.expected["VERSION"], b"2.4.0")
        markdown = tasks["markdown-hard-breaks"]
        self.assertIn(b"410 Harbor Street, Suite 200  \n", markdown.expected["docs/contact.md"])
        self.assertIn(b"Priya Raman  \n", markdown.expected["docs/release-notes.md"])
        large = tasks["large-file-two-regions"]
        routes = large.fixture["src/routes.py"]
        self.assertGreaterEqual(routes.count(b"\n"), 1900)
        changed = [
            number
            for number, (old, new) in enumerate(
                zip(routes.split(b"\n"), large.expected["src/routes.py"].split(b"\n")), 1
            )
            if old != new
        ]
        self.assertEqual(changed, [148, 1876, 1878, 1879])
        tabs = tasks["tabs-makefile-go"]
        self.assertIn(b"\n\t$(GO) vet ./...\n\t$(GO) test ./...\n", tabs.expected["Makefile"])
        self.assertIn(
            b'\n\t\tfmt.Fprintln(os.Stderr, "PORT not set; using 9090")\n',
            tabs.expected["cmd/server/main.go"],
        )

    def test_large_file_matches_its_generator(self):
        path = TASKS_DIR / "large-file-two-regions" / "generate.py"
        spec = importlib.util.spec_from_file_location("ultra_edit_eval_large_file", path)
        assert spec is not None and spec.loader is not None
        generator = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(generator)
        task = evaluation.load_task(TASKS_DIR / "large-file-two-regions")
        self.assertEqual(task.fixture["src/routes.py"], generator.fixture_bytes())
        self.assertEqual(task.expected["src/routes.py"], generator.expected_bytes())

    def test_malformed_tasks_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "broken"
            evaluation.write_tree(
                root,
                {
                    "prompt.md": b"   \n",
                    "fixture/a.txt": b"a",
                    "expected/b.txt": b"b",
                    "expected/a.txt": b"a",
                },
            )
            problems = evaluation.validate_task(evaluation.load_task(root))
            self.assertIn("prompt.md is empty", problems)
            self.assertIn("expected/b.txt has no counterpart in fixture/", problems)
            self.assertIn("expected/a.txt is identical to the fixture", problems)
            with self.assertRaises(evaluation.EvalError):
                evaluation.discover_tasks(Path(directory))
            with self.assertRaises(evaluation.EvalError):
                evaluation.discover_tasks(TASKS_DIR, ["no-such-task"])


class IntegrityTests(unittest.TestCase):
    staged = Path(tempfile.gettempdir()) / "staged-plugin"
    ultra_plugin = {"name": "ultra-edit", "path": str(staged)}
    ultra_server = {"name": "plugin:ultra-edit:ultra-edit", "status": "connected"}

    def test_native_arms_reject_ultra_edit_leaks(self):
        leaked = transcript_from(
            init_message(
                tools=("Edit", ULTRA + "ultra_edit"),
                mcp_servers=[self.ultra_server],
                plugins=[self.ultra_plugin],
            ),
            assistant("m", tool_use("t", ULTRA + "ultra_edit", {})),
            result_message(),
        )
        errors, _ = evaluation.check_integrity("native", leaked)
        self.assertEqual(len(errors), 3)
        clean = transcript_from(
            init_message(mcp_servers=[{"name": "other", "status": "connected"}]), result_message()
        )
        errors, warnings = evaluation.check_integrity("native", clean)
        self.assertEqual(errors, [])
        self.assertEqual(warnings, ["other MCP servers: other"])

    def test_ultra_edit_arm_requires_plugin_server_and_routing_hook(self):
        good = transcript_from(
            hook_response("SessionStart"),
            init_message(
                tools=("Edit", ULTRA + "ultra_edit"),
                mcp_servers=[self.ultra_server],
                plugins=[self.ultra_plugin],
            ),
            result_message(),
        )
        self.assertEqual(evaluation.check_integrity("ultra-edit", good, self.staged), ([], []))
        moved = transcript_from(
            hook_response("SessionStart"),
            init_message(
                tools=(ULTRA + "ultra_edit",),
                mcp_servers=[self.ultra_server],
                plugins=[{"name": "ultra-edit", "path": "/cache/ultra-edit"}],
            ),
            result_message(),
        )
        errors, warnings = evaluation.check_integrity("ultra-edit", moved, self.staged)
        self.assertEqual(errors, [])
        self.assertIn("Ultra Edit reported path /cache/ultra-edit", warnings[0])
        missing = transcript_from(init_message(), result_message())
        errors, _ = evaluation.check_integrity("ultra-edit", missing, self.staged)
        self.assertIn("Ultra Edit plugin did not load", errors)
        self.assertIn("Ultra Edit MCP server missing from init", errors)
        failed = transcript_from(
            hook_response("SessionStart", outcome="error"),
            init_message(
                mcp_servers=[{"name": "plugin:ultra-edit:ultra-edit", "status": "failed"}],
                plugins=[self.ultra_plugin],
            ),
            result_message(),
        )
        errors, _ = evaluation.check_integrity("ultra-edit", failed, self.staged)
        self.assertIn("Ultra Edit MCP server status: failed", errors)
        self.assertIn("SessionStart routing hook failed", errors)
        pending = transcript_from(
            hook_response("SessionStart"),
            init_message(
                mcp_servers=[{"name": "plugin:ultra-edit:ultra-edit", "status": "pending"}],
                plugins=[self.ultra_plugin],
            ),
            result_message(),
        )
        errors, warnings = evaluation.check_integrity("ultra-edit", pending, self.staged)
        self.assertEqual(errors, [])
        self.assertIn("Ultra Edit MCP server still pending at init", warnings)

    def test_guard_arm_requires_hook_events_for_bash_calls(self):
        bash = (assistant("m", tool_use("t", "Bash", {"command": "ls"})), tool_result("t", "a"))
        silent = transcript_from(init_message(), *bash, result_message())
        errors, _ = evaluation.check_integrity("native-guard", silent)
        self.assertTrue(errors and "guard did not run" in errors[0])
        self.assertEqual(
            evaluation.check_integrity("native-guard", silent, hook_events_expected=False), ([], [])
        )
        heard = transcript_from(
            init_message(), bash[0], hook_response("PreToolUse"), bash[1], result_message()
        )
        self.assertEqual(evaluation.check_integrity("native-guard", heard), ([], []))
        broken = transcript_from(
            init_message(), bash[0], hook_response("PreToolUse", outcome="error"), bash[1], result_message()
        )
        self.assertIn("guard hook failed 1 time(s)", evaluation.check_integrity("native-guard", broken)[0])

    def test_classification(self):
        right = evaluation.Comparison([], [], [])
        wrong = evaluation.Comparison(["VERSION"], [], [])
        started = transcript_from(init_message(), result_message())
        self.assertEqual(evaluation.classify(started, right, [], False), "pass")
        self.assertEqual(evaluation.classify(started, wrong, [], False), "fail")
        self.assertEqual(evaluation.classify(started, right, ["leak"], False), "invalid")
        self.assertEqual(evaluation.classify(started, right, [], True), "timeout")
        self.assertEqual(evaluation.classify(transcript_from(), right, [], False), "infra_error")
        self.assertEqual(
            evaluation.classify(transcript_from(init_message()), right, [], False), "infra_error"
        )
        overloaded = transcript_from(
            init_message(), assistant("m", {"type": "text", "text": ""}, error="overloaded"), result_message()
        )
        self.assertEqual(evaluation.classify(overloaded, right, [], False), "infra_error")
        max_turns = transcript_from(init_message(), result_message(subtype="error_max_turns", is_error=True))
        self.assertEqual(evaluation.classify(max_turns, wrong, [], False), "fail")


# A stand-in ultra-edit-mcp whose `--claude-hook PreToolUse` denies heredocs.
# Any other arguments get a usage error and exit 2, like a build without the mode.
GUARD_RUNTIME = r"""
import json
import sys

DENY = {
    "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": "use Ultra Edit",
    }
}
args = sys.argv[1:]
if args == ["--version"]:
    print("ultra-edit-mcp 0.0.0-test")
elif args == ["--claude-hook", "PreToolUse"]:
    if "<<" in json.load(sys.stdin)["tool_input"]["command"]:
        print(json.dumps(DENY))
else:
    sys.stderr.write("Usage: ultra-edit-mcp --root WORKSPACE\n")
    sys.exit(2)
"""

# A stand-in for `claude`; see eval_fake_claude.py. It never contacts a model.
FAKE_CLAUDE = (Path(__file__).resolve().parent / "eval_fake_claude.py").read_text(encoding="utf-8")


class HarnessProcessTests(unittest.TestCase):
    """Staging, guard preflight, and end-to-end runs with fake executables."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.fake_claude = self.root / "fake_claude.py"
        self.fake_claude.write_text(f"#!{sys.executable}\n" + FAKE_CLAUDE, encoding="utf-8")
        self.fake_claude.chmod(0o755)
        self.plan_path = self.root / "plan.json"
        self.log_path = self.root / "fake-claude-log.json"
        self.work = self.root / "work"
        self.work.mkdir()

    def write_plan(self, writes):
        self.plan_path.write_text(
            json.dumps(
                {"writes": {path: base64.b64encode(data).decode("ascii") for path, data in writes.items()}}
            ),
            encoding="utf-8",
        )

    def config(self, out):
        env = dict(os.environ)
        env.update(FAKE_CLAUDE_PLAN=str(self.plan_path), FAKE_CLAUDE_LOG=str(self.log_path), CLAUDECODE="1")
        return evaluation.Config(
            claude=evaluation.ClaudeInfo([sys.executable, str(self.fake_claude)]),
            out_dir=out,
            work_dir=self.work,
            base_env=env,
        )

    def test_stage_plugin_copies_plugin_and_runtime(self):
        source = self.root / "plugin-source"
        shutil.copytree(
            REPO_ROOT / "plugin" / "claude-code", source, ignore=shutil.ignore_patterns("runtime")
        )
        (source / "runtime").mkdir()
        (source / "runtime" / "stale-binary").write_bytes(b"old")
        runtime = self.root / "target-release"
        (runtime / "targets" / "x86_64-unknown-linux-musl").mkdir(parents=True)
        for name in ("ultra-edit-mcp", "ultra-edit", "ultra-edit.d"):
            (runtime / name).write_bytes(b"#!/bin/sh\n")
        (runtime / "targets" / "x86_64-unknown-linux-musl" / "ultra-edit-mcp").write_bytes(b"bin")
        staged = evaluation.stage_plugin(source, runtime, self.root / "staged", windows=False)
        self.assertEqual(staged.runtime_files, ["ultra-edit", "ultra-edit-mcp", "targets/"])
        self.assertEqual(
            staged.mcp_executable, (self.root / "staged" / "runtime" / "ultra-edit-mcp").resolve()
        )
        self.assertFalse((self.root / "staged" / "runtime" / "stale-binary").exists())
        self.assertFalse((self.root / "staged" / "runtime" / "ultra-edit.d").exists())
        for relative in (
            ".claude-plugin/plugin.json",
            ".mcp.json",
            "hooks/hooks.json",
            "skills/edit/SKILL.md",
        ):
            self.assertTrue((self.root / "staged" / relative).is_file(), relative)
        self.assertEqual(
            staged.plugin_version, evaluation.plugin_version(REPO_ROOT / "plugin" / "claude-code")
        )
        if os.name != "nt":
            self.assertTrue(staged.mcp_executable.stat().st_mode & stat.S_IXUSR)
        with self.assertRaisesRegex(evaluation.EvalError, "ultra-edit-mcp.exe"):
            evaluation.stage_plugin(source, runtime, self.root / "staged-windows", windows=True)
        (runtime / "ultra-edit-mcp.exe").write_bytes(b"MZ")
        windows = evaluation.stage_plugin(source, runtime, self.root / "staged-windows", windows=True)
        self.assertEqual(windows.mcp_executable.name, "ultra-edit-mcp.exe")

    def test_guard_hook_preflight(self):
        hook = self.root / "guard_runtime.py"
        hook.write_text(GUARD_RUNTIME, encoding="utf-8")
        env = dict(os.environ)
        self.assertEqual(
            evaluation.check_guard_hook(
                [sys.executable, str(hook), *evaluation.GUARD_HOOK_ARGS], self.root, env
            ),
            [],
        )
        problems = evaluation.check_guard_hook([sys.executable, str(hook), "--unsupported"], self.root, env)
        self.assertTrue(any("git status" in problem for problem in problems), problems)
        self.assertTrue(any("rebuild" in problem for problem in problems), problems)

    @unittest.skipUnless(HAS_GIT, "git is required")
    def test_run_one_scores_exact_bytes_and_records_artifacts(self):
        task = evaluation.load_task(TASKS_DIR / "crlf-and-lf")
        self.write_plan(task.expected)
        out = self.root / "results"
        result = evaluation.run_one(self.config(out), evaluation.RunSpec(task, "native", 1))
        self.assertEqual(result["outcome"], "pass", result)
        self.assertTrue(result["correct"])
        self.assertTrue(result["first_attempt"])
        self.assertEqual(result["metrics"]["tool_calls_by_name"], {"Edit": 3})
        self.assertAlmostEqual(result["metrics"]["cost_usd"], 0.0125)
        run_dir = out / "runs" / "crlf-and-lf__native__r1"
        for name in ("settings.json", "command.json", "stream.jsonl", "stderr.txt", "result.json"):
            self.assertTrue((run_dir / name).is_file(), name)
        self.assertFalse((run_dir / "diff.txt").exists())
        self.assertEqual(list(self.work.iterdir()), [], "temporary repositories are removed")
        log = json.loads(self.log_path.read_text(encoding="utf-8"))
        self.assertEqual(log["prompt"], task.prompt, "the prompt arrives on stdin unchanged")
        self.assertNotIn(task.prompt, log["argv"])
        self.assertEqual(
            log["git_log"], "Ultra Edit Eval|eval@ultra-edit.invalid|2026-01-01T00:00:00+00:00|Fixture"
        )
        self.assertEqual(log["autocrlf"], "false")
        self.assertEqual(
            log["env"],
            {"CLAUDECODE": None, "DISABLE_AUTOUPDATER": "1", "ENABLE_CLAUDEAI_MCP_SERVERS": "false"},
        )
        command = json.loads((run_dir / "command.json").read_text(encoding="utf-8"))
        self.assertIn("CLAUDECODE", command["env_removed"])
        self.assertIn("--setting-sources=", command["argv"])

    @unittest.skipUnless(HAS_GIT, "git is required")
    def test_run_one_detects_newline_damage_and_extra_files(self):
        task = evaluation.load_task(TASKS_DIR / "crlf-and-lf")
        damaged = dict(task.expected)
        damaged["build/release.cmd"] = damaged["build/release.cmd"].replace(b"\r\n", b"\n")
        damaged["build/release.cmd.bak"] = b"backup"
        self.write_plan(damaged)
        out = self.root / "results"
        result = evaluation.run_one(self.config(out), evaluation.RunSpec(task, "native", 2))
        self.assertEqual(result["outcome"], "fail")
        self.assertEqual(result["comparison"]["mismatched"], ["build/release.cmd"])
        self.assertEqual(result["comparison"]["unexpected"], ["build/release.cmd.bak"])
        self.assertFalse(result["first_attempt"])
        diff = (out / "runs" / "crlf-and-lf__native__r2" / "diff.txt").read_text(encoding="utf-8")
        self.assertIn('-"set SIGN_BUILD=1\\r\\n"', diff)
        self.assertIn('+"set SIGN_BUILD=1\\n"', diff)

    @unittest.skipUnless(HAS_GIT, "git is required")
    def test_run_one_reports_a_missing_claude_as_infrastructure(self):
        task = evaluation.load_task(TASKS_DIR / "crlf-and-lf")
        config = self.config(self.root / "results")
        config.claude = evaluation.ClaudeInfo([str(self.root / "no-such-claude")])
        result = evaluation.run_one(config, evaluation.RunSpec(task, "native", 1))
        self.assertEqual(result["outcome"], "infra_error")
        self.assertTrue(result["integrity"]["errors"][0].startswith("harness: "))

    @unittest.skipUnless(HAS_GIT and os.name != "nt", "needs git and executable scripts")
    def test_main_runs_all_three_arms_end_to_end(self):
        task = evaluation.load_task(TASKS_DIR / "markdown-hard-breaks")
        self.write_plan(task.expected)
        runtime = self.root / "runtime"
        runtime.mkdir()
        guard = runtime / "ultra-edit-mcp"
        guard.write_text(f"#!{sys.executable}\n" + GUARD_RUNTIME, encoding="utf-8")
        guard.chmod(0o755)
        out = self.root / "results"
        env = {"FAKE_CLAUDE_PLAN": str(self.plan_path), "FAKE_CLAUDE_LOG": str(self.log_path)}
        output = io.StringIO()
        with mock.patch.dict(os.environ, env), redirect_stdout(output):
            code = evaluation.main(
                [
                    "--task",
                    "markdown-hard-breaks",
                    "--claude",
                    str(self.fake_claude),
                    "--runtime-dir",
                    str(runtime),
                    "--permission-mode",
                    "acceptEdits",
                    "--out",
                    str(out),
                    "--work-dir",
                    str(self.work),
                    "--yes",
                ]
            )
        self.assertEqual(code, 0, output.getvalue())
        records = {record["arm"]: record for record in evaluation.load_records(out)}
        self.assertEqual(sorted(records), sorted(evaluation.ARMS))
        for arm, record in records.items():
            self.assertEqual(record["outcome"], "pass", (arm, record["integrity"]))
            self.assertEqual(record["integrity"]["errors"], [], arm)
        guard_metrics = records["native-guard"]["metrics"]
        self.assertEqual((guard_metrics["hook_denials"], guard_metrics["bash_writes_blocked"]), (1, 1))
        self.assertEqual(records["ultra-edit"]["metrics"]["ultra_edit_statuses"], {"ok": 2})
        self.assertEqual(records["native"]["metrics"]["tool_calls_by_name"], {"Edit": 2})
        guard_settings = json.loads(
            (out / "runs" / "markdown-hard-breaks__native-guard__r1" / "settings.json").read_text(
                encoding="utf-8"
            )
        )
        hook = guard_settings["hooks"]["PreToolUse"][0]["hooks"][0]
        self.assertEqual(Path(hook["command"]), (out / "plugin" / "runtime" / "ultra-edit-mcp").resolve())
        summary = (out / "summary.md").read_text(encoding="utf-8")
        for arm in evaluation.ARMS:
            self.assertIn(f"| markdown-hard-breaks / {arm} | 1 | 1/1 (100%) |", summary)
        manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["claude"]["version"], "2.1.282 (Claude Code)")
        self.assertEqual(manifest["plugin"]["runtime_files"], ["ultra-edit-mcp"])
        self.assertIn("guard hook denies a heredoc write and allows a read", output.getvalue())
        self.assertEqual(list(self.work.iterdir()), [])

    def test_main_refuses_a_non_empty_results_directory_and_bad_options(self):
        out = self.root / "results"
        out.mkdir()
        (out / "runs.jsonl").write_text("", encoding="utf-8")
        with redirect_stdout(io.StringIO()), mock.patch("sys.stderr", new=io.StringIO()) as stderr:
            code = evaluation.main(["--arm", "native", "--task", "crlf-and-lf", "--out", str(out), "--yes"])
        self.assertEqual(code, 2)
        self.assertIn("not empty", stderr.getvalue())
        with (
            redirect_stdout(io.StringIO()),
            mock.patch("sys.stderr", new=io.StringIO()),
            self.assertRaises(SystemExit),
        ):
            evaluation.main(["--reps", "0"])


if __name__ == "__main__":
    unittest.main()
