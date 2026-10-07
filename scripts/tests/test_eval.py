"""Offline tests for the model-level evaluation harness in eval/run_eval.py.

None of these tests start Claude Code, use the network, or need credentials.
End-to-end runner tests launch a small fake `claude` script instead.
"""

from __future__ import annotations

import base64
import csv
import importlib.util
import io
import json
import os
import shutil
import stat
import statistics
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime
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
ORIGINAL_TASKS = [
    "crlf-and-lf",
    "large-file-two-regions",
    "markdown-hard-breaks",
    "rename-constant",
    "tabs-makefile-go",
    "windows-paths-escapes",
]
NEW_TASKS = [
    "many-scattered-edits",
    "move-and-delete-blocks",
    "replace-function-body",
    "sed-hostile-regex",
    "shell-hostile-scripts",
    "signature-threading",
    "unicode-quotes",
    "yaml-near-duplicates",
]
# Keeps tests independent of whatever eval/third_party/arms.json holds.
NO_THIRD_PARTY = str(REPO_ROOT / "eval" / "third_party" / "no-such-arms-file.json")
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
            ("perl -0pi -e 's/a\\nb/c/' file", "Bash", ["in_place"]),
            ("perl -0777 -ne 'print' file", "Bash", []),
            ("gawk -i inplace '{sub(/a/, \"b\")} 1' file", "Bash", ["in_place"]),
            ("awk '{print $1}' file", "Bash", []),
            ("git apply fix.patch", "Bash", []),
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

    def test_shell_edit_kinds(self):
        cases = [
            ("git apply fix.patch", ["patch_apply"]),
            ("git -C repo apply -p1 /tmp/x.diff", ["patch_apply"]),
            ("cat fix.diff | git apply", ["patch_apply"]),
            ("patch -p1 < fix.diff", ["patch_apply"]),
            ("git apply <<'PATCH'\n--- a/x\n+++ b/x\nPATCH", ["patch_apply"]),
            ("git apply --check fix.patch", []),
            ("git apply --stat x.diff && git apply x.diff", ["patch_apply"]),
            ("patch --dry-run -p1 < fix.diff", []),
            ("sed 's/a/b/' f > f.tmp && mv f.tmp f", ["filter_redirect"]),
            ("LC_ALL=C awk '{print}' f > f.new", ["filter_redirect"]),
            ("grep x f 2>/dev/null", []),
            ("cat f > /dev/null", []),
            ("cat > f <<'EOF'\nx > y\nEOF", []),
            ("echo 'git apply x' && git status", []),
            ("sed -n '1,5p' f", []),
            (None, []),
        ]
        for command, expected in cases:
            with self.subTest(command=command):
                self.assertEqual(evaluation.shell_edit_kinds(command), expected)

    def test_failed_patch_file_apply_is_a_failed_edit_but_not_a_bash_write(self):
        transcript = transcript_from(
            init_message(),
            assistant("m1", tool_use("t1", "Bash", {"command": "git apply fix.patch"})),
            tool_result("t1", "error: patch failed: a.txt:1", is_error=True),
            assistant(
                "m2", tool_use("t2", "Bash", {"command": "sed 's/a/b/' a.txt > a.tmp && mv a.tmp a.txt"})
            ),
            tool_result("t2", ""),
            assistant("m3", tool_use("t3", "Bash", {"command": "git apply --check fix.patch"})),
            tool_result("t3", ""),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual((metrics["edit_calls"], metrics["edit_failures"]), (2, 1))
        self.assertEqual((metrics["bash_write_attempts"], metrics["bash_writes"]), (0, 0))
        self.assertEqual(metrics["shell_edit_kinds"], {"filter_redirect": 1, "patch_apply": 1})
        self.assertFalse(evaluation.first_attempt(metrics, correct=True))


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

    def test_rescore_recomputes_metrics_from_saved_streams(self):
        with tempfile.TemporaryDirectory() as directory:
            results = Path(directory)
            run_dir = results / "runs" / "t__native__r1"
            run_dir.mkdir(parents=True)
            stream = [
                init_message(),
                assistant("msg_1", tool_use("t1", "Edit", {"file_path": "a", "old_string": "x"})),
                tool_result("t1", "not found", is_error=True),
                assistant("msg_2", tool_use("t2", "Edit", {"file_path": "a", "old_string": "y"})),
                tool_result("t2", "updated"),
                result_message(),
            ]
            (run_dir / "stream.jsonl").write_text(
                "".join(json.dumps(message) + "\n" for message in stream), encoding="utf-8"
            )
            stale = {
                "run_id": "t__native__r1",
                "task": "t",
                "arm": "native",
                "rep": 1,
                "outcome": "pass",
                "correct": True,
                "first_attempt": True,
                "comparison": {"mismatched": [], "missing": [], "unexpected": []},
                "integrity": {"errors": [], "warnings": []},
                "metrics": {"tool_calls": 0, "edit_failures": 0},
            }
            (results / "runs.jsonl").write_text(json.dumps(stale) + "\n", encoding="utf-8")
            with redirect_stdout(io.StringIO()):
                self.assertEqual(evaluation.main(["--rescore", str(results)]), 0)
            [record] = evaluation.load_records(results)
            self.assertEqual(record["outcome"], "pass")
            self.assertEqual(record["metrics"]["tool_calls"], 2)
            self.assertEqual(record["metrics"]["edit_failures"], 1)
            self.assertFalse(record["first_attempt"])
            self.assertTrue((results / "runs.jsonl.bak").exists())
            self.assertTrue((results / "summary.md").exists())


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
        for arm in evaluation.DEFAULT_ARMS:
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
                        "matcher": "Bash|PowerShell",
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
            "Bash",
            {"env": {"A": "1"}, "hooks": {"PreToolUse": [{"matcher": "Edit"}]}},
        )
        self.assertEqual(merged["env"], {"A": "1"})
        self.assertEqual([entry["matcher"] for entry in merged["hooks"]["PreToolUse"]], ["Bash", "Edit"])

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

    def test_child_environment_drops_host_behavior_knobs(self):
        base = {
            "PATH": "/usr/bin",
            "ANTHROPIC_BASE_URL": "https://proxy.invalid",
            "CLAUDE_EFFORT": "xhigh",
            "MCP_CONNECTION_NONBLOCKING": "1",
            "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH": "4096",
            "CLAUDE_CODE_ARTIFACT_DB": "1",
            "CLAUDE_CODE_REMOTE_SESSION_ID": "cse_x",
            "CLAUDE_CODE_SYNC_PLUGINS": "1",
        }
        env, removed = evaluation.build_env(base)
        self.assertEqual(env["ANTHROPIC_BASE_URL"], "https://proxy.invalid")
        for name in base:
            if name not in ("PATH", "ANTHROPIC_BASE_URL"):
                self.assertNotIn(name, env)
                self.assertIn(name, removed)

    def test_plan_interleaves_and_shuffles_arm_order_deterministically(self):
        tasks = [evaluation.Task(name, Path(name), "p", {"a": b"1"}, {"a": b"2"}) for name in ("t1", "t2")]
        plan = evaluation.plan_runs(tasks, evaluation.DEFAULT_ARMS, 2, seed=7)
        self.assertEqual(len(plan), 12)
        self.assertEqual([spec.task.name for spec in plan[:3]], ["t1"] * 3)
        self.assertEqual(sorted(spec.arm for spec in plan[:3]), sorted(evaluation.DEFAULT_ARMS))
        self.assertEqual(
            [spec.run_id for spec in plan],
            [spec.run_id for spec in evaluation.plan_runs(tasks, evaluation.DEFAULT_ARMS, 2, seed=7)],
        )
        fixed = evaluation.plan_runs(tasks, evaluation.DEFAULT_ARMS, 1, seed=None)
        self.assertEqual([spec.arm for spec in fixed[:3]], list(evaluation.DEFAULT_ARMS))

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
                        "--third-party-arms",
                        NO_THIRD_PARTY,
                    ]
                )
            text = output.getvalue()
            self.assertEqual(code, 0)
            self.assertFalse(out.exists())
            for arm in evaluation.DEFAULT_ARMS:
                self.assertIn(f"== {arm} (example: crlf-and-lf__{arm}__r1) ==", text)
            self.assertIn('"--claude-hook"', text)
            self.assertIn("--plugin-dir", text)
            self.assertIn("--model sonnet", text)
            self.assertIn("stdin: ", text)
            self.assertEqual(text.count("crlf-and-lf / "), 3)


class TaskFixtureTests(unittest.TestCase):
    def test_all_tasks_are_well_formed(self):
        tasks = evaluation.discover_tasks(TASKS_DIR)
        self.assertEqual([task.name for task in tasks], sorted(ORIGINAL_TASKS + NEW_TASKS))
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
        tasks = {task.name: task for task in evaluation.discover_tasks(TASKS_DIR, ORIGINAL_TASKS)}
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

    def test_guard_arm_requires_hook_events_for_shell_calls(self):
        powershell = (
            assistant("m", tool_use("t", "PowerShell", {"command": "Get-ChildItem"})),
            tool_result("t", "a"),
        )
        silent = transcript_from(init_message(), *powershell, result_message())
        errors, _ = evaluation.check_integrity("native-guard", silent)
        self.assertTrue(errors and errors[0].startswith("PowerShell called"), errors)
        # A matcher without PowerShell expects no hook event for it.
        self.assertEqual(evaluation.check_integrity("native-guard", silent, guard_matcher="Bash"), ([], []))
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


# A stand-in ultra-edit-mcp whose `--claude-hook PreToolUse` denies heredocs and
# here-strings. Any other arguments get a usage error and exit 2, like a build
# without the mode.
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
    command = json.load(sys.stdin)["tool_input"]["command"]
    if "<<" in command or "@'" in command:
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
        # A runtime that only guards Bash fails the PowerShell probe unless the
        # matcher leaves PowerShell out.
        bash_only = self.root / "bash_only_runtime.py"
        bash_only.write_text(
            GUARD_RUNTIME.replace("""if "<<" in command or "@'" in command:""", 'if "<<" in command:'),
            encoding="utf-8",
        )
        argv = [sys.executable, str(bash_only), *evaluation.GUARD_HOOK_ARGS]
        problems = evaluation.check_guard_hook(argv, self.root, env)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("allowed PowerShell", problems[0])
        self.assertEqual(evaluation.check_guard_hook(argv, self.root, env, "Bash"), [])

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
        pinned = int(datetime.fromisoformat(evaluation.GIT_DATE).timestamp())
        self.assertEqual(log["git_log"], f"Ultra Edit Eval|eval@ultra-edit.invalid|{pinned}|Fixture")
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
                    "--third-party-arms",
                    NO_THIRD_PARTY,
                    "--yes",
                ]
            )
        self.assertEqual(code, 0, output.getvalue())
        records = {record["arm"]: record for record in evaluation.load_records(out)}
        self.assertEqual(sorted(records), sorted(evaluation.DEFAULT_ARMS))
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
        for arm in evaluation.DEFAULT_ARMS:
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


# ---------------------------------------------------------------------------
# Arm registry, third-party arms, metrics, summaries, and parallel runs


def third_party_entry(**overrides):
    entry = {
        "description": "Reference filesystem server",
        "server_name": "fs",
        "mcp_server": {
            "type": "stdio",
            "command": sys.executable,
            "args": ["{third_party_dir}/server.py", "--root", "{repo}"],
            "env": {"FS_ROOT": "{repo}", "PLAIN": "x"},
        },
        "edit_tools": ["edit_file", "write_file"],
        "required_tools": ["edit_file"],
        "append_system_prompt": "Use the fs tools for every file change.",
        "disallowed_tools": ["Edit", "Write", "MultiEdit", "NotebookEdit"],
    }
    entry.update(overrides)
    return entry


def write_arms_file(directory, arms):
    path = Path(directory) / "arms.json"
    path.write_text(json.dumps({"arms": arms}), encoding="utf-8")
    return path


def names(specs):
    return [spec.name for spec in specs]


class ArmRegistryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def command(self, arm, **options):
        return evaluation.build_command(
            ["claude"],
            arm=arm,
            settings_path="/r/settings.json",
            plugin_dir="/r/plugin",
            mcp_config_path="/r/mcp.json",
            permission_mode="acceptEdits",
            **options,
        )

    def test_builtin_arms_and_commands(self):
        self.assertEqual(evaluation.DEFAULT_ARMS, ("native", "native-guard", "ultra-edit"))
        self.assertEqual(
            list(evaluation.BUILTIN_ARMS),
            [
                "native",
                "native-guard",
                "ultra-edit",
                "ultra-edit-only",
                "shell-sed",
                "shell-python",
                "shell-patch",
            ],
        )
        default_rules = ",".join(evaluation.DEFAULT_ALLOWED_TOOLS)
        without_edit = ",".join(
            tool for tool in evaluation.DEFAULT_ALLOWED_TOOLS if tool not in ("Edit", "Write")
        )
        expected = {
            "native": (default_rules, None, None, False),
            "native-guard": (default_rules, None, None, False),
            "ultra-edit": (default_rules, None, None, True),
            "ultra-edit-only": (without_edit, "Edit,Write,MultiEdit,NotebookEdit", None, True),
            "shell-sed": (without_edit, "Edit,Write,MultiEdit,NotebookEdit", "running sed (or awk", False),
            "shell-python": (without_edit, "Edit,Write,MultiEdit,NotebookEdit", "python3 - <<'PY'", False),
            "shell-patch": (without_edit, "Edit,Write,MultiEdit,NotebookEdit", "git apply <<'PATCH'", False),
        }
        for name, (allowed, disallowed, prompt, plugin) in expected.items():
            with self.subTest(arm=name):
                argv = self.command(name)
                self.assertEqual(argv[argv.index("--allowedTools") + 1], allowed)
                if disallowed:
                    self.assertEqual(argv[argv.index("--disallowedTools") + 1], disallowed)
                else:
                    self.assertNotIn("--disallowedTools", argv)
                if prompt:
                    text = argv[argv.index("--append-system-prompt") + 1]
                    self.assertTrue(
                        text.startswith("For this session the Edit, Write, MultiEdit, and NotebookEdit")
                    )
                    self.assertIn(prompt, text)
                else:
                    self.assertNotIn("--append-system-prompt", argv)
                self.assertEqual("--plugin-dir" in argv, plugin)
                self.assertNotIn("--mcp-config", argv)
                self.assertNotIn("--strict-mcp-config", argv)
                bypass = evaluation.build_command(
                    ["claude"], arm=name, settings_path="s.json", plugin_dir="/p"
                )
                self.assertNotIn("--allowedTools", bypass)
                self.assertEqual("--disallowedTools" in bypass, bool(disallowed))
        guard = evaluation.build_settings("native-guard", "/x/ultra-edit-mcp")
        self.assertIn("hooks", guard)
        for name in ("ultra-edit-only", "shell-sed", "shell-python", "shell-patch"):
            self.assertEqual(evaluation.build_settings(name), evaluation.build_settings("native"))
        with self.assertRaises(evaluation.EvalError):
            evaluation.build_command(["claude"], arm="ultra-edit-only", settings_path="s.json")
        with self.assertRaises(evaluation.EvalError):
            evaluation.build_settings("no-such-arm")

    def test_disable_plugin_settings(self):
        settings = evaluation.build_settings(
            "native", disabled_plugins=["cc-plugin-telemetry@builtin", "other@market"]
        )
        self.assertEqual(
            settings["enabledPlugins"],
            {
                "ultra-edit@ultra-edit": False,
                "ultra-edit@skills-dir": False,
                "cc-plugin-telemetry@builtin": False,
                "other@market": False,
            },
        )

    def test_third_party_arm_loading_and_mcp_config(self):
        path = write_arms_file(
            self.root,
            {
                "fs-test": third_party_entry(),
                "bad-name": third_party_entry(server_name="has space"),
                "native": third_party_entry(server_name="clash"),
                "no-command": third_party_entry(server_name="nocmd", mcp_server={"args": []}),
            },
        )
        registry = evaluation.load_arm_registry(path)
        self.assertEqual(registry.third_party_servers, frozenset({"fs", "has space", "clash", "nocmd"}))
        self.assertEqual(sorted(registry.unavailable), ["bad-name", "native", "no-command"])
        self.assertEqual(registry.arms["native"], evaluation.BUILTIN_ARMS["native"])
        arm = registry.arms["fs-test"]
        self.assertTrue(arm.third_party)
        self.assertEqual(arm.edit_tools, ("mcp__fs__edit_file", "mcp__fs__write_file"))
        self.assertEqual(arm.required_tools, ("mcp__fs__edit_file",))
        config = evaluation.mcp_config(arm, "/tmp/run/repo", "/opt/tp")
        self.assertEqual(
            config,
            {
                "mcpServers": {
                    "fs": {
                        "type": "stdio",
                        "command": sys.executable,
                        "args": ["/opt/tp/server.py", "--root", "/tmp/run/repo"],
                        "env": {"FS_ROOT": "/tmp/run/repo", "PLAIN": "x"},
                    }
                }
            },
        )
        argv = self.command(arm)
        self.assertEqual(argv[argv.index("--mcp-config") + 1], "/r/mcp.json")
        self.assertTrue(argv[argv.index("--allowedTools") + 1].endswith(",mcp__fs"))
        self.assertEqual(argv[argv.index("--disallowedTools") + 1], "Edit,Write,MultiEdit,NotebookEdit")
        self.assertEqual(
            argv[argv.index("--append-system-prompt") + 1], "Use the fs tools for every file change."
        )
        self.assertNotIn("--plugin-dir", argv)
        self.assertNotIn("--strict-mcp-config", argv)
        with self.assertRaises(evaluation.EvalError):
            evaluation.build_command(["claude"], arm=arm, settings_path="s.json")
        defaults = evaluation.third_party_arm("x", third_party_entry(append_system_prompt=None))
        self.assertIsNone(defaults.append_system_prompt)
        entry = third_party_entry()
        del entry["disallowed_tools"]
        self.assertEqual(
            evaluation.third_party_arm("x", entry).disallowed_tools, evaluation.NATIVE_EDIT_TOOL_NAMES
        )

    def test_third_party_arm_names_and_edit_tools_are_validated(self):
        path = write_arms_file(
            self.root,
            {
                "ok.v1": third_party_entry(),
                "../escape": third_party_entry(server_name="a"),
                "x__y": third_party_entry(server_name="b"),
                "no-edit-tools": third_party_entry(server_name="c", edit_tools=[]),
                "missing-edit-tools": {
                    key: value
                    for key, value in third_party_entry(server_name="d").items()
                    if key != "edit_tools"
                },
            },
        )
        registry = evaluation.load_arm_registry(path)
        self.assertIn("ok.v1", registry.arms)
        self.assertEqual(
            sorted(registry.unavailable), ["../escape", "missing-edit-tools", "no-edit-tools", "x__y"]
        )
        self.assertIn("arm names must be", registry.unavailable["../escape"])
        self.assertIn("edit_tools", registry.unavailable["no-edit-tools"])
        # Unavailable entries still name servers that must not leak into other arms.
        self.assertEqual(registry.third_party_servers, frozenset({"fs", "a", "b", "c", "d"}))

    def test_resolve_default_all_unknown_and_unavailable(self):
        path = write_arms_file(
            self.root,
            {"fs-test": third_party_entry(), "broken": third_party_entry(server_name="bad name")},
        )
        registry = evaluation.load_arm_registry(path)
        self.assertEqual(names(registry.resolve(None)), list(evaluation.DEFAULT_ARMS))
        self.assertEqual(
            names(registry.resolve(["shell-sed", "native", "shell-sed"])), ["shell-sed", "native"]
        )
        with self.assertRaisesRegex(evaluation.EvalError, "unknown arm 'nope'.*eval/third_party/README.md"):
            registry.resolve(["nope"])
        with self.assertRaisesRegex(evaluation.EvalError, "arm 'broken' is unavailable"):
            registry.resolve(["broken"])
        with self.assertRaisesRegex(evaluation.EvalError, "broken"):
            registry.resolve(["all"])
        good = evaluation.load_arm_registry(write_arms_file(self.root, {"fs-test": third_party_entry()}))
        self.assertEqual(names(good.resolve(["all"])), [*evaluation.BUILTIN_ARMS, "fs-test"])
        missing = evaluation.load_arm_registry(self.root / "absent.json")
        self.assertEqual(names(missing.resolve(["all"])), list(evaluation.BUILTIN_ARMS))
        self.assertEqual(missing.third_party_servers, frozenset())
        (self.root / "bad.json").write_text("{not json", encoding="utf-8")
        with self.assertRaisesRegex(evaluation.EvalError, "README"):
            evaluation.load_arm_registry(self.root / "bad.json")
        (self.root / "list.json").write_text("[]", encoding="utf-8")
        with self.assertRaises(evaluation.EvalError):
            evaluation.load_arm_registry(self.root / "list.json")

    def test_third_party_preflight_checks_executables(self):
        install = self.root / "install"
        install.mkdir()
        arm = evaluation.third_party_arm("fs-test", third_party_entry())
        problems = evaluation.third_party_problems(arm, install)
        self.assertEqual(len(problems), 1)
        self.assertIn("missing install path", problems[0])
        self.assertIn("eval/third_party/README.md", problems[0])
        (install / "server.py").write_text("", encoding="utf-8")
        self.assertEqual(evaluation.third_party_problems(arm, install), [])
        absent = evaluation.third_party_arm(
            "x", third_party_entry(mcp_server={"command": "no-such-mcp-server-binary-1234", "args": []})
        )
        self.assertIn("command not found", evaluation.third_party_problems(absent, install)[0])
        self.assertEqual(evaluation.third_party_problems(evaluation.BUILTIN_ARMS["native"], install), [])

    def run_main(self, *args):
        output = io.StringIO()
        with redirect_stdout(output), mock.patch("sys.stderr", new=io.StringIO()) as stderr:
            code = evaluation.main(list(args))
        return code, output.getvalue(), stderr.getvalue()

    def test_dry_run_with_new_and_third_party_arms(self):
        arms_file = write_arms_file(self.root, {"fs-test": third_party_entry()})
        out = self.root / "results"
        code, text, _ = self.run_main(
            "--dry-run",
            "--task",
            "crlf-and-lf",
            "--arm",
            "all",
            "--out",
            str(out),
            "--third-party-arms",
            str(arms_file),
            "--third-party-dir",
            str(self.root / "install"),
            "--disable-plugin",
            "cc-plugin-telemetry@builtin",
            "--permission-mode",
            "acceptEdits",
            "--jobs",
            "4",
        )
        self.assertEqual(code, 0, text)
        self.assertFalse(out.exists())
        for name in [*evaluation.BUILTIN_ARMS, "fs-test"]:
            self.assertIn(f"== {name} (example: crlf-and-lf__{name}__r1) ==", text)
        self.assertIn('"cc-plugin-telemetry@builtin": false', text)
        self.assertIn("WARNING: arm fs-test: missing install path", text)
        self.assertIn(f'"{self.root / "install" / "server.py"}"', text)
        self.assertIn('"<temp>/repo"', text)
        self.assertIn("--mcp-config", text)
        self.assertIn(",mcp__fs ", text)
        self.assertIn("4 at a time", text)
        self.assertEqual(text.count("crlf-and-lf / "), len(evaluation.BUILTIN_ARMS) + 1)
        code, _, stderr = self.run_main("--dry-run", "--arm", "nope", "--third-party-arms", str(arms_file))
        self.assertEqual(code, 2)
        self.assertIn("unknown arm 'nope'", stderr)
        self.assertIn("eval/third_party/README.md", stderr)
        code, _, stderr = self.run_main("--preflight", "--arm", "nope", "--third-party-arms", str(arms_file))
        self.assertEqual(code, 2)
        with redirect_stdout(io.StringIO()), mock.patch("sys.stderr", new=io.StringIO()), self.assertRaises(
            SystemExit
        ):
            evaluation.main(["--jobs", "0"])


class ThirdPartyIntegrityTests(unittest.TestCase):
    arm = evaluation.third_party_arm("fs-test", third_party_entry())
    servers = frozenset({"fs", "other-fs"})

    def check(self, arm, *messages):
        return evaluation.check_integrity(arm, transcript_from(*messages), third_party_servers=self.servers)

    def test_third_party_arm_requires_connected_server_and_tools(self):
        tools = ("Read", "Bash", "mcp__fs__edit_file", "mcp__fs__write_file")
        good = init_message(tools=tools, mcp_servers=[{"name": "fs", "status": "connected"}])
        self.assertEqual(self.check(self.arm, good, result_message()), ([], []))
        missing = init_message(tools=("Read",), mcp_servers=[])
        errors, _ = self.check(self.arm, missing, result_message())
        self.assertIn("fs MCP server missing from init", errors)
        self.assertIn("required fs tools missing at init: mcp__fs__edit_file", errors)
        failed = init_message(tools=tools, mcp_servers=[{"name": "fs", "status": "failed"}])
        self.assertEqual(self.check(self.arm, failed, result_message())[0], ["fs MCP server status: failed"])
        pending = init_message(tools=tools, mcp_servers=[{"name": "fs", "status": "pending"}])
        self.assertEqual(
            self.check(self.arm, pending, result_message())[0], ["fs MCP server status: pending"]
        )
        offered = init_message(tools=(*tools, "Edit"), mcp_servers=[{"name": "fs", "status": "connected"}])
        self.assertEqual(
            self.check(self.arm, offered, result_message())[0], ["disallowed tool(s) listed at init: Edit"]
        )
        ultra = init_message(
            tools=(*tools, ULTRA + "ultra_edit"),
            mcp_servers=[{"name": "fs", "status": "connected"}],
        )
        self.assertIn(
            "Ultra Edit MCP tools present in the fs-test arm",
            self.check(self.arm, ultra, result_message())[0],
        )
        other = init_message(
            tools=(*tools, "mcp__other-fs__read_file"),
            mcp_servers=[{"name": "fs", "status": "connected"}, {"name": "other-fs", "status": "connected"}],
        )
        self.assertEqual(
            self.check(self.arm, other, result_message())[0],
            ["third-party MCP server(s) present in the fs-test arm: other-fs"],
        )

    def test_third_party_arm_without_required_tools_needs_one_server_tool(self):
        arm = evaluation.third_party_arm("fs-test", third_party_entry(required_tools=[]))
        connected = [{"name": "fs", "status": "connected"}]
        empty = init_message(tools=("Read", "Bash", "mcp__other-fs__edit_file"), mcp_servers=connected)
        errors, _ = self.check(arm, empty, result_message())
        self.assertIn("no fs tools listed at init", errors)
        listed = init_message(tools=("Read", "Bash", "mcp__fs__read_file"), mcp_servers=connected)
        self.assertEqual(self.check(arm, listed, result_message())[0], [])

    def test_other_arms_reject_third_party_leaks_and_disallowed_tools(self):
        leaked_server = init_message(mcp_servers=[{"name": "fs", "status": "connected"}])
        errors, warnings = self.check("native", leaked_server, result_message())
        self.assertEqual(errors, ["third-party MCP server(s) present in the native arm: fs"])
        self.assertEqual(warnings, [])
        leaked_tool = init_message(tools=("Read", "mcp__fs__edit_file"))
        self.assertEqual(
            self.check("shell-sed", leaked_tool, result_message())[0],
            ["third-party MCP server(s) present in the shell-sed arm: fs"],
        )
        called = (
            init_message(tools=("Read", "Bash")),
            assistant("m", tool_use("t", "mcp__fs__edit_file", {})),
            tool_result("t", "ok"),
            result_message(),
        )
        self.assertEqual(
            self.check("native", *called)[0],
            ["third-party tools called in the native arm: mcp__fs__edit_file"],
        )
        offered = init_message(tools=("Read", "Edit", "Write", "Bash"))
        self.assertEqual(
            self.check("shell-python", offered, result_message())[0],
            ["disallowed tool(s) listed at init: Edit, Write"],
        )
        self.assertEqual(self.check("native", offered, result_message()), ([], []))
        clean = init_message(tools=("Read", "Bash"))
        self.assertEqual(self.check("shell-patch", clean, result_message()), ([], []))
        hooked = (clean, hook_response("PreToolUse"), result_message())
        self.assertIn(
            "PreToolUse hooks ran in the shell-sed arm (managed hooks?)", self.check("shell-sed", *hooked)[1]
        )

    def test_ultra_edit_only_has_the_ultra_edit_requirements(self):
        server = {"name": "plugin:ultra-edit:ultra-edit", "status": "connected"}
        plugin = {"name": "ultra-edit", "path": "/staged"}
        good = (
            hook_response("SessionStart"),
            init_message(
                tools=("Read", "Bash", ULTRA + "ultra_edit"), mcp_servers=[server], plugins=[plugin]
            ),
            result_message(),
        )
        self.assertEqual(self.check("ultra-edit-only", *good), ([], []))
        missing = (init_message(tools=("Read", "Edit")), result_message())
        errors, _ = self.check("ultra-edit-only", *missing)
        self.assertIn("Ultra Edit plugin did not load", errors)
        self.assertIn("Ultra Edit MCP server missing from init", errors)
        self.assertIn("disallowed tool(s) listed at init: Edit", errors)


def usage(input_tokens=0, read=0, write=0, output=0):
    return {
        "input_tokens": input_tokens,
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": write,
        "output_tokens": output,
    }


def assistant_with_usage(message_id, block, used, parent=None, model="claude-test"):
    message = assistant(message_id, block, parent=parent)
    message["message"]["usage"] = used
    message["message"]["model"] = model
    return message


class RichMetricsTests(unittest.TestCase):
    def test_api_calls_are_deduplicated_by_message_id(self):
        first = usage(10, 0, 15000, 0)
        text = {"type": "text", "text": "Reading."}
        transcript = transcript_from(
            init_message(),
            assistant_with_usage("msg_1", text, first),
            # The same response again with the final output count.
            assistant_with_usage(
                "msg_1", tool_use("t1", "Read", {"file_path": "/tmp/repo/a.txt"}), usage(10, 0, 15000, 80)
            ),
            tool_result("t1", "hello"),
            assistant_with_usage("msg_2", tool_use("t2", "Task", {"prompt": "x"}), usage(5, 15000, 200, 40)),
            assistant_with_usage("sub_1", text, usage(3, 0, 4000, 30), parent="t2"),
            assistant_with_usage("sub_1", text, usage(3, 0, 4000, 30), parent="t2"),
            tool_result("t2", "done"),
            assistant_with_usage("msg_3", text, usage(2, 15200, 0, 25)),
            assistant_with_usage("synthetic", text, usage(), model="<synthetic>"),
            result_message(result="All done."),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual(metrics["api_calls"], 4)
        self.assertEqual(metrics["subagent_api_calls"], 1)
        self.assertEqual(metrics["first_call_context_tokens"], 15010)
        self.assertEqual(metrics["peak_context_tokens"], 15205)
        self.assertEqual(metrics["context_tokens_total"], 15010 + 15205 + 4003 + 15202)
        self.assertEqual(metrics["output_tokens_calls"], 80 + 40 + 30 + 25)
        self.assertEqual(metrics["context_series"], [[15010, 80], [15205, 40], [15202, 25]])
        self.assertEqual(metrics["final_text_bytes"], len("All done."))
        empty = evaluation.compute_metrics(transcript_from(init_message()))
        self.assertEqual(empty["api_calls"], 0)
        self.assertIsNone(empty["first_call_context_tokens"])
        self.assertIsNone(empty["context_tokens_total"])
        self.assertIsNone(empty["final_text_bytes"])
        self.assertEqual(empty["context_series"], [])

    def test_parallel_tool_calls_interleave_lines_of_one_response(self):
        # As in Claude Code 2.1.292: one response with three parallel tool calls is
        # three stream lines with the same id and usage, with results in between.
        used = usage(2, 0, 22293, 24)
        transcript = transcript_from(
            init_message(),
            assistant_with_usage("msg_a", tool_use("t1", "Read", {"file_path": "a"}), used),
            tool_result("t1", "a"),
            assistant_with_usage("msg_a", tool_use("t2", "Read", {"file_path": "b"}), used),
            assistant_with_usage("msg_a", tool_use("t3", "Read", {"file_path": "c"}), used),
            tool_result("t2", "b"),
            tool_result("t3", "c"),
            assistant_with_usage("msg_b", {"type": "text", "text": "Done."}, usage(2, 22293, 1124, 388)),
            result_message(
                modelUsage={
                    "claude-test": {
                        "inputTokens": 4,
                        "outputTokens": 900,
                        "cacheReadInputTokens": 22293,
                        "cacheCreationInputTokens": 23417,
                    }
                }
            ),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual((metrics["api_calls"], metrics["tool_calls"]), (2, 3))
        self.assertEqual(metrics["context_tokens_total"], metrics["total_input_tokens"])
        self.assertEqual(metrics["context_series"], [[22295, 24], [23419, 388]])
        # Stream output counts are snapshots; modelUsage holds the real output.
        self.assertLess(metrics["output_tokens_calls"], metrics["output_tokens"])

    def test_bytes_shell_reads_and_tool_counters(self):
        snapshot = ULTRA + "ultra_edit_snapshot"
        read_input = {"file_path": "/tmp/repo/src/a.py"}
        transcript = transcript_from(
            init_message(),
            assistant("m1", tool_use("t1", "Read", read_input)),
            tool_result("t1", "héllo"),
            assistant("m2", tool_use("t2", "Read", {"file_path": "/tmp/repo/src/./a.py", "offset": 5})),
            tool_result("t2", "x"),
            assistant("m3", tool_use("t3", snapshot, {"path": "src/a.py", "selection": {"kind": "full"}})),
            tool_result("t3", ultra_json(snapshot="s1")),
            assistant("m4", tool_use("t4", snapshot, {"path": "src/b.py", "selection": {"kind": "full"}})),
            tool_result("t4", ultra_json(snapshot="s2")),
            assistant("m5", tool_use("t5", "Bash", {"command": "ls"})),
            tool_result("t5", "a\nb"),
            assistant("m6", tool_use("t6", "Bash", {"command": "false"})),
            tool_result("t6", "Exit code 1", is_error=True),
            assistant("m7", tool_use("t7", "ToolSearch", {"query": "select:x"})),
            tool_result("t7", "found"),
            assistant("m8", tool_use("t8", "Skill", {"skill": "edit"})),
            tool_result("t8", "loaded"),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript)
        self.assertEqual((metrics["read_calls"], metrics["reread_calls"]), (4, 2))
        self.assertEqual((metrics["shell_calls"], metrics["shell_errors"]), (2, 1))
        self.assertEqual((metrics["toolsearch_calls"], metrics["skill_calls"]), (1, 1))
        read_bytes = len(json.dumps(read_input, separators=(",", ":")).encode()) + len(
            json.dumps({"file_path": "/tmp/repo/src/./a.py", "offset": 5}, separators=(",", ":")).encode()
        )
        self.assertEqual(metrics["tool_input_bytes_by_tool"]["Read"], read_bytes)
        self.assertEqual(metrics["tool_result_bytes_by_tool"]["Read"], len("héllo".encode()) + 1)
        self.assertEqual(metrics["tool_result_bytes_by_tool"]["Bash"], 3 + len("Exit code 1"))
        self.assertEqual(metrics["tool_input_bytes"], sum(metrics["tool_input_bytes_by_tool"].values()))
        self.assertEqual(metrics["tool_result_bytes"], sum(metrics["tool_result_bytes_by_tool"].values()))
        self.assertEqual(metrics["tool_errors"], 1)
        self.assertEqual(metrics["edit_calls"], 0)

    def test_third_party_edit_tools_count_as_edit_calls(self):
        edit = "mcp__fs__edit_file"
        transcript = transcript_from(
            init_message(tools=("Read", edit)),
            assistant("m1", tool_use("t1", edit, {"path": "a", "edits": []})),
            tool_result("t1", "Error: no match", is_error=True),
            assistant("m2", tool_use("t2", edit, {"path": "a", "edits": []})),
            tool_result("t2", "ok"),
            assistant("m3", tool_use("t3", "mcp__fs__read_file", {"path": "a"})),
            tool_result("t3", "text"),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript, (edit,))
        self.assertEqual((metrics["edit_calls"], metrics["edit_failures"], metrics["tool_errors"]), (2, 1, 1))
        self.assertFalse(evaluation.first_attempt(metrics, correct=True))
        plain = evaluation.compute_metrics(transcript)
        self.assertEqual((plain["edit_calls"], plain["tool_errors"]), (0, 1))


    def test_masked_and_missing_viewer_edit_outcomes(self):
        edit = "mcp__text-editor__patch_text_file_contents"
        hidden = "python3 - <<'EOF'\nimport pathlib\npathlib.Path('a').write_text('x')\nEOF\ngit diff --stat"
        viewer = "sed -i 's/a/b/' a\nxxd a"
        broken = "sed -i 's/a/b/' a"
        transcript = transcript_from(
            init_message(tools=("Bash", edit)),
            assistant("m1", tool_use("t1", "Bash", {"command": hidden})),
            tool_result("t1", "Traceback (most recent call last):\nAssertionError"),
            assistant("m2", tool_use("t2", "Bash", {"command": viewer})),
            tool_result("t2", "Exit code 127\n/bin/bash: line 2: xxd: command not found", is_error=True),
            assistant("m3", tool_use("t3", "Bash", {"command": broken})),
            tool_result("t3", "Exit code 127\n/bin/bash: line 1: sed: command not found", is_error=True),
            assistant("m4", tool_use("t4", edit, {"file_path": "a", "patches": []})),
            tool_result("t4", json.dumps({"result": "error", "reason": "Content range hash mismatch"})),
            result_message(),
        )
        metrics = evaluation.compute_metrics(transcript, (edit,))
        # The traceback and the error inside the MCP result are masked failures; a missing
        # xxd after a sed that ran is not a failed edit; a missing sed is.
        self.assertEqual(
            (metrics["edit_calls"], metrics["edit_failures"], metrics["masked_edit_failures"]), (4, 3, 2)
        )
        self.assertEqual(metrics["tool_errors"], 2)
        self.assertFalse(evaluation.first_attempt(metrics, correct=True))


class SpreadSummaryTests(unittest.TestCase):
    def records(self):
        return [
            record(
                "t1",
                "native",
                1,
                "pass",
                True,
                cost_usd=0.1,
                context_tokens_total=1000,
                first_call_context_tokens=900,
            ),
            record(
                "t1",
                "native",
                2,
                "pass",
                True,
                cost_usd=0.3,
                context_tokens_total=3000,
                first_call_context_tokens=910,
            ),
            record(
                "t1",
                "native",
                3,
                "fail",
                False,
                cost_usd=0.2,
                context_tokens_total=2000,
                first_call_context_tokens=920,
            ),
            record(
                "t2",
                "native",
                1,
                "pass",
                True,
                cost_usd=0.5,
                context_tokens_total=5000,
                first_call_context_tokens=930,
            ),
            record(
                "t1", "shell-sed", 1, "pass", True, cost_usd=0.2, context_tokens_total=2000, shell_errors=2
            ),
            record(
                "t1", "shell-sed", 2, "pass", True, cost_usd=0.2, context_tokens_total=2000, shell_errors=0
            ),
            record("t1", "shell-sed", 3, "invalid", False, cost_usd=5.0),
        ]

    def test_aggregate_spread_and_cost_per_correct(self):
        native = evaluation.aggregate([r for r in self.records() if r["arm"] == "native"])
        self.assertAlmostEqual(native["cost_usd"], 0.275)
        self.assertAlmostEqual(native["cost_usd_median"], 0.25)
        self.assertAlmostEqual(native["cost_usd_sd"], statistics.stdev([0.1, 0.3, 0.2, 0.5]))
        self.assertAlmostEqual(native["cost_total_usd"], 1.1)
        self.assertAlmostEqual(native["cost_per_correct"], 1.1 / 3)
        self.assertEqual(native["context_tokens_total_median"], 2500)
        self.assertEqual(native["first_call_context_tokens"], 915)
        sed = evaluation.aggregate([r for r in self.records() if r["arm"] == "shell-sed"])
        self.assertEqual((sed["scored"], sed["shell_errors"], sed["cost_usd_sd"]), (2, 1.0, 0.0))
        single = evaluation.aggregate([self.records()[0]])
        self.assertIsNone(single["cost_usd_sd"])
        failed = evaluation.aggregate([self.records()[2]])
        self.assertIsNone(failed["cost_per_correct"])

    def test_variance_uses_tasks_with_repetitions(self):
        native = [r for r in self.records() if r["arm"] == "native"]
        variance = evaluation.variance_by_task(native)
        self.assertEqual(variance["tasks"], 1)  # t2 has one run
        self.assertAlmostEqual(variance["cost_cv"], statistics.stdev([0.1, 0.3, 0.2]) / 0.2)
        self.assertAlmostEqual(variance["context_cv"], statistics.stdev([1000, 3000, 2000]) / 2000)
        sed = evaluation.variance_by_task([r for r in self.records() if r["arm"] == "shell-sed"])
        self.assertEqual((sed["tasks"], sed["cost_cv"]), (1, 0.0))
        self.assertEqual(evaluation.variance_by_task([]), {"tasks": 0, "cost_cv": None, "context_cv": None})

    def test_summary_sections_are_order_independent_and_accept_old_records(self):
        records = self.records()
        summary = evaluation.summarize(records)
        self.assertEqual(summary, evaluation.summarize(list(reversed(records))))
        self.assertIn("### Spread by arm", summary)
        self.assertIn("| native | 4 | 3/4 (75%) | 3/4 (75%) | 0.2750 ± 0.1708 (0.2500) | 0.3667 |", summary)
        self.assertIn("## Variance", summary)
        self.assertIn("| native | 1 | 0.50 | 0.50 |", summary)
        self.assertIn("| shell-sed | 1 | 0.00 | 0.00 |", summary)
        self.assertIn("## Fixed context overhead", summary)
        self.assertIn("| native | 4 | 915 |", summary)
        self.assertIn("| shell-sed | 0 | - |", summary)
        self.assertIn("### Errors and bytes by task and arm", summary)
        self.assertIn("| t1 / shell-sed | 2 | 0.00 | 1.00 |", summary)
        self.assertLess(summary.index("| native |"), summary.index("| shell-sed |"))
        old = [record("t1", "native", 1, "pass", True)]
        for item in old:
            item["metrics"].pop("cost_usd")
        text = evaluation.summarize(old)
        self.assertIn("| native | 1 | 1/1 (100%) | 1/1 (100%) | - | - | - |", text)

    def test_runs_csv_has_one_scalar_row_per_run(self):
        records = self.records()
        records[0]["metrics"]["context_series"] = [[1, 2]]
        records[0]["metrics"]["tool_input_bytes_by_tool"] = {"Read": 3}
        records[0]["metrics"]["zz_new_scalar"] = 7
        columns, rows = evaluation.csv_rows(records)
        self.assertEqual(columns[:4], ["run_id", "task", "arm", "rep"])
        self.assertEqual(columns[-1], "reason")
        self.assertIn("context_tokens_total", columns)
        self.assertIn("zz_new_scalar", columns)
        for unwanted in (
            "context_series",
            "tool_input_bytes_by_tool",
            "tool_calls_by_name",
            "api_errors",
            "models",
        ):
            self.assertNotIn(unwanted, columns)
        self.assertEqual(evaluation.csv_rows(list(reversed(records))), (columns, rows))
        self.assertEqual(len(rows), len(records))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "runs.csv"
            evaluation.write_runs_csv(records, path)
            with open(path, encoding="utf-8", newline="") as handle:
                parsed = list(csv.DictReader(handle))
        self.assertEqual(len(parsed), len(records))
        self.assertEqual(parsed[0]["run_id"], "t1__native__r1")
        self.assertEqual(parsed[0]["cost_usd"], "0.1")
        self.assertEqual(parsed[0]["peak_context_tokens"], "")

    def test_summarize_option_writes_runs_csv(self):
        with tempfile.TemporaryDirectory() as directory:
            results = Path(directory)
            with open(results / "runs.jsonl", "w", encoding="utf-8") as handle:
                for item in self.records():
                    handle.write(json.dumps(item) + "\n")
            with redirect_stdout(io.StringIO()):
                self.assertEqual(evaluation.main(["--summarize", str(results)]), 0)
            self.assertTrue((results / "runs.csv").is_file())


@unittest.skipUnless(HAS_GIT and os.name != "nt", "needs git and executable scripts")
class ParallelAndThirdPartyRunTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.fake_claude = self.root / "fake_claude.py"
        self.fake_claude.write_text(f"#!{sys.executable}\n" + FAKE_CLAUDE, encoding="utf-8")
        self.fake_claude.chmod(0o755)
        self.plan_path = self.root / "plan.json"
        self.logs = self.root / "logs"
        self.logs.mkdir()
        self.work = self.root / "work"
        self.work.mkdir()
        self.task = evaluation.load_task(TASKS_DIR / "markdown-hard-breaks")

    def write_plan(self, **extra):
        writes = {path: base64.b64encode(data).decode("ascii") for path, data in self.task.expected.items()}
        self.plan_path.write_text(json.dumps({"writes": writes, **extra}), encoding="utf-8")

    def env(self):
        return {"FAKE_CLAUDE_PLAN": str(self.plan_path), "FAKE_CLAUDE_LOG_DIR": str(self.logs)}

    def config(self, out):
        env = dict(os.environ)
        env.update(self.env())
        return evaluation.Config(
            claude=evaluation.ClaudeInfo([sys.executable, str(self.fake_claude)]),
            out_dir=out,
            work_dir=self.work,
            base_env=env,
        )

    def test_parallel_runs_record_every_run_regardless_of_completion_order(self):
        # The first planned run finishes last.
        self.write_plan(sleep={"native": 1.0})
        out = self.root / "results"
        out.mkdir()
        config = self.config(out)
        plan = evaluation.plan_runs([self.task], ["native", "shell-sed", "shell-python"], 1, seed=None)
        log = evaluation.RunLog(out / "runs.jsonl")
        with redirect_stdout(io.StringIO()) as output:
            skipped = evaluation.run_plan(config, plan, log, jobs=3)
        self.assertEqual(skipped, 0)
        records = evaluation.load_records(out)
        self.assertEqual(sorted(r["arm"] for r in records), ["native", "shell-python", "shell-sed"])
        self.assertEqual(records[-1]["arm"], "native")
        self.assertTrue(all(r["outcome"] == "pass" for r in records), [r["integrity"] for r in records])
        lines = [line for line in output.getvalue().splitlines() if line.startswith("[")]
        self.assertEqual(len(lines), 3, output.getvalue())
        self.assertTrue(all("PASS" in line for line in lines))
        self.assertEqual(evaluation.summarize(records), evaluation.summarize(log.records))
        metrics = records[0]["metrics"]
        # Two stream lines per response share one id and one usage.
        self.assertEqual(metrics["api_calls"], len(self.task.expected))
        self.assertEqual(metrics["first_call_context_tokens"], 1000)
        sed_log = json.loads(
            (self.logs / "markdown-hard-breaks__shell-sed__r1.json").read_text(encoding="utf-8")
        )
        argv = sed_log["argv"]
        self.assertEqual(argv[argv.index("--disallowedTools") + 1], "Edit,Write,MultiEdit,NotebookEdit")
        self.assertIn("sed", argv[argv.index("--append-system-prompt") + 1])

    def test_rate_limited_runs_are_retried_and_their_attempts_kept(self):
        self.write_plan(rate_limited={"native": 2})
        out = self.root / "results"
        out.mkdir()
        config = self.config(out)
        config.transient_retries = 3
        config.retry_delay_s = 0.01
        plan = evaluation.plan_runs([self.task], ["native", "shell-sed"], 1, seed=None)
        log = evaluation.RunLog(out / "runs.jsonl")
        with redirect_stdout(io.StringIO()):
            evaluation.run_plan(config, plan, log, jobs=2)
        records = {r["arm"]: r for r in evaluation.load_records(out)}
        self.assertEqual(records["native"]["outcome"], "pass")
        self.assertEqual(records["native"]["transient_retries"], 2)
        self.assertNotIn("transient_retries", records["shell-sed"])
        runs = out / "runs"
        self.assertTrue((runs / "markdown-hard-breaks__native__r1.attempt-1" / "stream.jsonl").exists())
        self.assertTrue((runs / "markdown-hard-breaks__native__r1.attempt-2").exists())
        self.assertFalse((runs / "markdown-hard-breaks__native__r1.attempt-3").exists())

    def test_retries_stop_after_the_limit(self):
        self.write_plan(rate_limited={"native": 9})
        out = self.root / "results"
        out.mkdir()
        config = self.config(out)
        config.transient_retries = 1
        config.retry_delay_s = 0.01
        plan = evaluation.plan_runs([self.task], ["native"], 1, seed=None)
        with redirect_stdout(io.StringIO()):
            evaluation.run_plan(config, plan, evaluation.RunLog(out / "runs.jsonl"))
        [record] = evaluation.load_records(out)
        self.assertEqual((record["outcome"], record["transient_retries"]), ("infra_error", 1))

    def test_resume_reruns_infrastructure_errors_and_named_arms_only(self):
        out = self.root / "results"
        out.mkdir()
        arms = ["native", "shell-sed", "shell-python"]
        plan = evaluation.plan_runs([self.task], arms, 1, seed=None)
        self.write_plan(rate_limited={"native": 1})
        config = self.config(out)
        with redirect_stdout(io.StringIO()):
            evaluation.run_plan(config, plan, evaluation.RunLog(out / "runs.jsonl"))
        first = {r["arm"]: r for r in evaluation.load_records(out)}
        self.assertEqual(first["native"]["outcome"], "infra_error")
        kept, todo = evaluation.resume_plan(out, plan, ["shell-python"], apply=False)
        self.assertEqual(sorted(spec.arm for spec in todo), ["native", "shell-python"])
        self.assertEqual(len(evaluation.load_records(out)), 3, "a dry resume plan changes nothing")
        kept, todo = evaluation.resume_plan(out, plan, ["shell-python"])
        self.assertEqual([r["arm"] for r in kept], ["shell-sed"])
        self.assertEqual([r["arm"] for r in evaluation.load_records(out)], ["shell-sed"])
        self.assertTrue((out / "runs" / "markdown-hard-breaks__native__r1.attempt-1").exists())
        log = evaluation.RunLog(out / "runs.jsonl", kept)
        with redirect_stdout(io.StringIO()):
            evaluation.run_plan(config, todo, log)
        records = {r["arm"]: r for r in evaluation.load_records(out)}
        self.assertEqual(sorted(records), sorted(arms))
        self.assertTrue(all(r["outcome"] == "pass" for r in records.values()))
        self.assertEqual(len(log.records), 3)

    def test_a_harness_error_in_one_run_does_not_stop_the_others(self):
        self.write_plan()
        out = self.root / "results"
        out.mkdir()
        real_run_one = evaluation.run_one

        def flaky(config, spec):
            if spec.arm == "shell-sed":
                raise KeyError("boom")
            return real_run_one(config, spec)

        plan = evaluation.plan_runs([self.task], ["native", "shell-sed", "shell-python"], 1, seed=None)
        log = evaluation.RunLog(out / "runs.jsonl")
        with mock.patch.object(evaluation, "run_one", flaky), redirect_stdout(io.StringIO()) as output:
            with mock.patch("sys.stderr", io.StringIO()):
                evaluation.run_plan(self.config(out), plan, log, jobs=2)
        self.assertEqual(sorted(r["arm"] for r in evaluation.load_records(out)), ["native", "shell-python"])
        self.assertIn("HARNESS ERROR (not recorded): KeyError", output.getvalue())

    def test_budget_stops_launching_new_runs(self):
        # Each fake run costs $0.0125, so a $0.02 limit is reached after two recorded runs.
        self.write_plan()
        arms = ["native", "shell-sed", "shell-python", "shell-patch"]
        for jobs in (1, 2):
            with self.subTest(jobs=jobs):
                out = self.root / f"results-{jobs}"
                out.mkdir()
                plan = evaluation.plan_runs([self.task], arms, 1, None)
                log = evaluation.RunLog(out / "runs.jsonl")
                with redirect_stdout(io.StringIO()) as output:
                    skipped = evaluation.run_plan(self.config(out), plan, log, jobs=jobs, max_total_usd=0.02)
                recorded = len(evaluation.load_records(out))
                # With two at a time, a third run may start after the first one is recorded.
                self.assertIn(recorded, (2,) if jobs == 1 else (2, 3))
                self.assertEqual(recorded + skipped, len(arms))
                self.assertIn(f"{skipped} planned run(s) skipped", output.getvalue())
                self.assertGreaterEqual(log.spent, 0.02)

    def test_main_runs_a_third_party_arm_with_mcp_config(self):
        self.write_plan(mcp_tools=["edit_file", "read_file"], mcp_edit_tool="edit_file")
        install = self.root / "install"
        install.mkdir()
        (install / "server.py").write_text("", encoding="utf-8")
        arms_file = write_arms_file(self.root, {"fs-test": third_party_entry()})
        out = self.root / "results"
        output = io.StringIO()
        with mock.patch.dict(os.environ, self.env()), redirect_stdout(output):
            code = evaluation.main(
                [
                    "--task", "markdown-hard-breaks",
                    "--arm", "fs-test",
                    "--arm", "shell-sed",
                    "--claude", str(self.fake_claude),
                    "--permission-mode", "acceptEdits",
                    "--third-party-arms", str(arms_file),
                    "--third-party-dir", str(install),
                    "--disable-plugin", "cc-plugin-telemetry@builtin",
                    "--jobs", "2",
                    "--out", str(out),
                    "--work-dir", str(self.work),
                    "--yes",
                ]
            )  # fmt: skip
        self.assertEqual(code, 0, output.getvalue())
        self.assertIn("arm fs-test: MCP server command found", output.getvalue())
        self.assertNotIn("staged plugin", output.getvalue())
        records = {r["arm"]: r for r in evaluation.load_records(out)}
        fs = records["fs-test"]
        self.assertEqual(fs["outcome"], "pass", fs["integrity"])
        self.assertEqual(fs["metrics"]["edit_calls"], len(self.task.expected))
        self.assertEqual(records["shell-sed"]["outcome"], "pass", records["shell-sed"]["integrity"])
        run_dir = out / "runs" / "markdown-hard-breaks__fs-test__r1"
        mcp = json.loads((run_dir / "mcp.json").read_text(encoding="utf-8"))
        server = mcp["mcpServers"]["fs"]
        log = json.loads((self.logs / "markdown-hard-breaks__fs-test__r1.json").read_text(encoding="utf-8"))
        repo = log["cwd"]
        self.assertEqual(
            server["args"], [str((install / "server.py").resolve()), "--root", str(Path(repo).resolve())]
        )
        self.assertEqual(server["env"]["FS_ROOT"], str(Path(repo).resolve()))
        self.assertEqual(log["mcp_config"], mcp)
        argv = log["argv"]
        self.assertEqual(Path(argv[argv.index("--mcp-config") + 1]), (run_dir / "mcp.json").resolve())
        self.assertIn("mcp__fs", argv[argv.index("--allowedTools") + 1].split(","))
        self.assertNotIn("--strict-mcp-config", argv)
        self.assertFalse((out / "runs" / "markdown-hard-breaks__shell-sed__r1" / "mcp.json").exists())
        settings = json.loads((run_dir / "settings.json").read_text(encoding="utf-8"))
        self.assertIs(settings["enabledPlugins"]["cc-plugin-telemetry@builtin"], False)
        manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["arm_specs"]["fs-test"]["server_name"], "fs")
        self.assertTrue((out / "runs.csv").is_file())
        self.assertIn("## Fixed context overhead", (out / "summary.md").read_text(encoding="utf-8"))
        self.assertEqual(list(self.work.iterdir()), [])

    def test_main_rejects_an_unavailable_third_party_arm_before_paid_runs(self):
        self.write_plan()
        arms_file = write_arms_file(
            self.root,
            {"fs-test": third_party_entry(mcp_server={"command": "no-such-mcp-server-binary-1234"})},
        )
        out = self.root / "results"
        output = io.StringIO()
        with mock.patch.dict(os.environ, self.env()), redirect_stdout(output):
            code = evaluation.main(
                [
                    "--task", "markdown-hard-breaks",
                    "--arm", "fs-test",
                    "--claude", str(self.fake_claude),
                    "--third-party-arms", str(arms_file),
                    "--out", str(out),
                    "--yes",
                ]
            )  # fmt: skip
        self.assertEqual(code, 1)
        self.assertIn("PROBLEM: arm fs-test: MCP server command not found", output.getvalue())
        self.assertIn("eval/third_party/README.md", output.getvalue())
        self.assertFalse((out / "runs.jsonl").exists())


if __name__ == "__main__":
    unittest.main()
