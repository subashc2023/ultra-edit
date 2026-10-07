#!/usr/bin/env python3
"""Measure Ultra Edit against native Claude Code editing on fixture tasks.

Each run copies one task fixture into a fresh Git repository, launches
``claude -p`` for one arm without a shell, saves the raw stream-json transcript,
and scores the final bytes against the task's expected files.

Real runs spend API credits. Start with --dry-run (prints commands and settings)
and --preflight (local checks only, no model calls). See eval/README.md.
"""

from __future__ import annotations

import argparse
import contextlib
import csv
import datetime
import difflib
import json
import os
import platform
import posixpath
import random
import re
import shlex
import shutil
import signal
import stat
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from collections import Counter
from collections.abc import Collection, Container, Iterable, Mapping, Sequence
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

EVAL_DIR = Path(__file__).resolve().parent
REPO_ROOT = EVAL_DIR.parent
DEFAULT_TASKS_DIR = EVAL_DIR / "tasks"
DEFAULT_RESULTS_DIR = EVAL_DIR / "results"
DEFAULT_PLUGIN_SOURCE = REPO_ROOT / "plugin" / "claude-code"
DEFAULT_RUNTIME_DIR = REPO_ROOT / "target" / "release"
THIRD_PARTY_DIR = EVAL_DIR / "third_party"
DEFAULT_THIRD_PARTY_ARMS = THIRD_PARTY_DIR / "arms.json"
DEFAULT_THIRD_PARTY_INSTALL = THIRD_PARTY_DIR / "install"
THIRD_PARTY_README = "eval/third_party/README.md"

# The arms that run when no --arm is given; --arm all adds every other arm.
DEFAULT_ARMS = ("native", "native-guard", "ultra-edit")
PLUGIN_NAME = "ultra-edit"
# Plugin MCP tools are namespaced mcp__plugin_<plugin>_<server>__<tool>.
ULTRA_TOOL_PREFIX = "mcp__plugin_ultra-edit_ultra-edit__"
ULTRA_PERMISSION_RULE = "mcp__plugin_ultra-edit_ultra-edit"
# Installed copies that must not leak into any arm (marketplace and skills-dir).
KNOWN_PLUGIN_IDS = ("ultra-edit@ultra-edit", "ultra-edit@skills-dir")
GUARD_HOOK_ARGS = ("--claude-hook", "PreToolUse")
GUARD_HOOK_TIMEOUT_S = 30
# The plugin's matcher: the guard classifies Bash and PowerShell commands.
DEFAULT_GUARD_MATCHER = "Bash|PowerShell"
RUNTIME_NAMES = ("ultra-edit", "ultra-edit-mcp", "ultra-edit.exe", "ultra-edit-mcp.exe")
IGNORED_TOP_LEVEL = frozenset({".git", ".ultra-edit"})
SCORED_OUTCOMES = ("pass", "fail", "timeout")

MIN_CLAUDE_VERSION = (2, 1, 139)  # hook "args" (exec form) arrived in 2.1.139
REQUIRED_FLAGS = (
    "--print",
    "--output-format",
    "--verbose",
    "--settings",
    "--setting-sources",
    "--plugin-dir",
    "--permission-mode",
    "--no-session-persistence",
)
OPTIONAL_FLAGS = ("--permission-prompts", "--include-hook-events", "--max-budget-usd", "--effort")
PERMISSION_MODES = ("bypassPermissions", "acceptEdits", "dontAsk", "auto", "default", "manual")
EFFORT_LEVELS = ("low", "medium", "high", "xhigh", "max")
DEFAULT_ALLOWED_TOOLS = ("Bash", "Edit", "Write", "Read", "Glob", "Grep", "Skill", ULTRA_PERMISSION_RULE)

# Variables that mark a parent Claude Code session or inject configuration into
# a child session. Authentication and provider variables are kept.
STRIPPED_ENV = (
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_CODE_PLUGIN_DIRS",
    "CLAUDE_CODE_SIMPLE",
    "CLAUDE_CODE_SAFE_MODE",
    "CLAUDE_CODE_RESTRICTED",
    "CLAUDE_ADDITIONAL_DIRECTORIES",
    "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_AFTER_LAST_COMPACT",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    # Behavior knobs that a host (such as a cloud Claude Code session) can set for
    # itself and that would otherwise change every arm's effort, tools, or MCP
    # startup: the evaluation measures Claude Code's defaults.
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_DEBUG",
    "CLAUDE_CODE_DIAGNOSTICS_FILE",
    "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH",
    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    "MCP_CONNECTION_NONBLOCKING",
    "MCP_TOOL_TIMEOUT",
    "ENABLE_TOOL_SEARCH",
    "CLAUDE_AUTO_BACKGROUND_TASKS",
    "CLAUDE_CODE_BG_TASKS_REPORT_RUNNING",
    "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_HOLD_UNANSWERED_PARKED_PERMISSION",
    "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH",
    "CLAUDE_CODE_TEE_SDK_STDOUT",
    "CLAUDE_CODE_DISABLE_BUILTIN_ANTMCP",
    "CLAUDE_CODE_USE_CCR_V2",
    "CLAUDE_CODE_WORKER_EPOCH",
    "CLAUDE_CODE_MODEL_CAPABILITIES",
    "DOCUMENTS_MCP_SCRATCH_ROOT",
    "AI_AGENT",
)
# Prefixes of host-specific variables that add tools or sync state into a session.
STRIPPED_ENV_PREFIXES = (
    "CLAUDE_CODE_ARTIFACT_",
    "CLAUDE_CODE_REMOTE",
    "CLAUDE_CODE_SYNC_",
    "CLAUDE_CODE_MESSAGING_",
)
CHILD_ENV = {
    "DISABLE_AUTOUPDATER": "1",
    "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "CLAUDE_CODE_SKIP_PROMPT_HISTORY": "1",
}
AUTH_ENV = (
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
)
GIT_NAME = "Ultra Edit Eval"
GIT_EMAIL = "eval@ultra-edit.invalid"
GIT_DATE = "2026-01-01T00:00:00+00:00"

NATIVE_WRITE_TOOLS = frozenset({"Edit", "Write", "MultiEdit", "NotebookEdit"})
SHELL_TOOLS = frozenset({"Bash", "PowerShell"})
ULTRA_TOOLS = frozenset(
    {
        "ultra_edit_snapshot",
        "ultra_edit",
        "ultra_edit_status",
        "ultra_edit_prepare",
        "ultra_edit_commit",
        "ultra_edit_retry",
        "ultra_edit_diff",
        "ultra_edit_inspect",
        "ultra_edit_reconcile",
        "ultra_edit_repair",
        "ultra_edit_undo",
    }
)
ULTRA_WRITE_TOOLS = frozenset({"ultra_edit", "ultra_edit_commit", "ultra_edit_retry", "ultra_edit_undo"})
ULTRA_RECOVERY_TOOLS = frozenset(
    {"ultra_edit_undo", "ultra_edit_repair", "ultra_edit_retry", "ultra_edit_reconcile", "ultra_edit_inspect"}
)
INFRA_ERRORS = frozenset(
    {
        "authentication_failed",
        "oauth_org_not_allowed",
        "account_on_hold",
        "billing_error",
        "rate_limit",
        "overloaded",
        "model_not_found",
        "server_error",
        "cloud_credential_error",
    }
)


class EvalError(RuntimeError):
    """A task, configuration, or environment problem that stops the evaluation."""


# ---------------------------------------------------------------------------
# Arms


NATIVE_EDIT_TOOL_NAMES = ("Edit", "Write", "MultiEdit", "NotebookEdit")
_SHELL_ONLY = (
    "For this session the Edit, Write, MultiEdit, and NotebookEdit tools are unavailable. "
    "Make every file change by "
)
_SERVER_NAME = re.compile(r"^[A-Za-z0-9_-]+$")
_ARM_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


@dataclass(frozen=True)
class ArmSpec:
    """How one arm differs from the others; everything else is shared."""

    name: str
    description: str
    plugin: bool = False  # load the staged Ultra Edit plugin with --plugin-dir
    guard_hook: bool = False  # install the shell guard PreToolUse hook in settings
    disallowed_tools: tuple[str, ...] = ()
    append_system_prompt: str | None = None
    server_name: str | None = None  # third-party MCP server (see eval/third_party/)
    mcp_server: Mapping[str, Any] | None = field(default=None, hash=False, compare=False)
    edit_tools: tuple[str, ...] = NATIVE_EDIT_TOOL_NAMES  # full names; counted as edit calls
    required_tools: tuple[str, ...] = ()  # full names that must be listed at init

    @property
    def third_party(self) -> bool:
        return self.server_name is not None

    @property
    def permission_rule(self) -> str | None:
        return f"mcp__{self.server_name}" if self.server_name else None

    def needs_flags(self) -> list[str]:
        flags = []
        if self.disallowed_tools:
            flags.append("--disallowedTools")
        if self.append_system_prompt:
            flags.append("--append-system-prompt")
        if self.mcp_server is not None:
            flags.append("--mcp-config")
        return flags

    def to_json(self) -> dict[str, Any]:
        return {
            "description": self.description,
            "plugin": self.plugin,
            "guard_hook": self.guard_hook,
            "disallowed_tools": list(self.disallowed_tools),
            "append_system_prompt": self.append_system_prompt,
            "server_name": self.server_name,
            "mcp_server": self.mcp_server,
            "edit_tools": list(self.edit_tools),
            "required_tools": list(self.required_tools),
        }


def _shell_arm(name: str, method: str, how: str) -> ArmSpec:
    return ArmSpec(
        name,
        f"Native tools without Edit/Write; every change through Bash with {method}.",
        disallowed_tools=NATIVE_EDIT_TOOL_NAMES,
        append_system_prompt=_SHELL_ONLY + how,
    )


BUILTIN_ARMS: dict[str, ArmSpec] = {
    spec.name: spec
    for spec in (
        ArmSpec("native", "Claude Code only: native Read/Edit/Write/Bash."),
        ArmSpec("native-guard", "Native tools plus Ultra Edit's shell guard hook.", guard_hook=True),
        ArmSpec("ultra-edit", "The full Ultra Edit plugin alongside native tools.", plugin=True),
        ArmSpec(
            "ultra-edit-only",
            "The full Ultra Edit plugin with Edit/Write/MultiEdit/NotebookEdit disallowed.",
            plugin=True,
            disallowed_tools=NATIVE_EDIT_TOOL_NAMES,
        ),
        _shell_arm(
            "shell-sed",
            "sed/awk/perl",
            "running sed (or awk or perl one-liners) through the Bash tool.",
        ),
        _shell_arm(
            "shell-python",
            "Python scripts",
            "running Python 3 code through the Bash tool, for example python3 - <<'PY' ... PY.",
        ),
        _shell_arm(
            "shell-patch",
            "git apply",
            "applying unified diffs with git apply through the Bash tool, "
            "for example git apply <<'PATCH' ... PATCH.",
        ),
    )
}


@dataclass
class ArmRegistry:
    """Built-in arms plus the third-party arms from eval/third_party/arms.json."""

    arms: dict[str, ArmSpec]
    unavailable: dict[str, str] = field(default_factory=dict)  # arm name -> why it cannot run
    third_party_servers: frozenset[str] = frozenset()  # every server name in arms.json
    source: Path | None = None

    def resolve(self, names: Sequence[str] | None) -> list[ArmSpec]:
        """Arm specs for --arm values; none means the default arms, `all` every arm."""
        requested = list(names or DEFAULT_ARMS)
        expanded: list[str] = []
        for name in requested:
            if name == "all":
                expanded += [*self.arms, *self.unavailable]
            else:
                expanded.append(name)
        selected = list(dict.fromkeys(expanded))
        problems = []
        for name in selected:
            if name in self.unavailable:
                problems.append(f"arm {name!r} is unavailable: {self.unavailable[name]}")
            elif name not in self.arms:
                problems.append(f"unknown arm {name!r}")
        if problems:
            available = ", ".join([*self.arms, "all"])
            raise EvalError(
                "; ".join(problems)
                + f". Available arms: {available}. Third-party arms are configured in "
                + f"{self.source or DEFAULT_THIRD_PARTY_ARMS}; see {THIRD_PARTY_README} for setup."
            )
        return [self.arms[name] for name in selected]


def _string_list(value: Any, what: str) -> tuple[str, ...]:
    if value is None:
        return ()
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ValueError(f"{what} must be a list of tool names")
    return tuple(value)


def full_tool_name(server_name: str, tool: str) -> str:
    return tool if tool.startswith("mcp__") else f"mcp__{server_name}__{tool}"


def third_party_arm(name: str, entry: Any) -> ArmSpec:
    """One arms.json entry as an ArmSpec. Raises ValueError with the reason."""
    if not isinstance(entry, dict):
        raise ValueError("entry must be an object")
    server_name = entry.get("server_name")
    if not isinstance(server_name, str) or not _SERVER_NAME.match(server_name):
        raise ValueError("server_name must be letters, digits, '_' or '-'")
    if is_ultra_name(server_name):
        raise ValueError("server_name must not name Ultra Edit")
    server = entry.get("mcp_server")
    if not isinstance(server, dict) or not isinstance(server.get("command"), str) or not server["command"]:
        raise ValueError("mcp_server must be an object with a command")
    if not isinstance(server.get("args", []), list) or not isinstance(server.get("env", {}), dict):
        raise ValueError("mcp_server args must be a list and env an object")
    prompt = entry.get("append_system_prompt")
    if prompt is not None and not isinstance(prompt, str):
        raise ValueError("append_system_prompt must be a string or null")
    edit_tools = _string_list(entry.get("edit_tools"), "edit_tools")
    if not edit_tools:
        # Without them the server's edits and failures would not count as edit calls.
        raise ValueError("edit_tools must name the server's file-changing tools")
    required = _string_list(entry.get("required_tools"), "required_tools")
    if "disallowed_tools" in entry:
        disallowed = _string_list(entry.get("disallowed_tools"), "disallowed_tools")
    else:
        disallowed = NATIVE_EDIT_TOOL_NAMES
    return ArmSpec(
        name,
        str(entry.get("description") or f"Third-party MCP server {server_name}."),
        disallowed_tools=disallowed,
        append_system_prompt=prompt or None,
        server_name=server_name,
        mcp_server=server,
        edit_tools=tuple(full_tool_name(server_name, tool) for tool in edit_tools),
        required_tools=tuple(full_tool_name(server_name, tool) for tool in required),
    )


def load_arm_registry(arms_file: Path | None = DEFAULT_THIRD_PARTY_ARMS) -> ArmRegistry:
    """Built-in arms plus arms.json. A missing file adds nothing; a bad entry is unavailable."""
    registry = ArmRegistry(dict(BUILTIN_ARMS), source=arms_file)
    if arms_file is None or not Path(arms_file).is_file():
        return registry
    try:
        data = json.loads(Path(arms_file).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise EvalError(f"cannot read {arms_file}: {error}; see {THIRD_PARTY_README}") from error
    entries = data.get("arms") if isinstance(data, dict) else None
    if not isinstance(entries, dict):
        raise EvalError(f'{arms_file} must hold {{"arms": {{...}}}}; see {THIRD_PARTY_README}')
    servers = set()
    for name, entry in entries.items():
        if isinstance(entry, dict) and isinstance(entry.get("server_name"), str):
            servers.add(entry["server_name"])
        if name in BUILTIN_ARMS or name == "all":
            registry.unavailable[name] = f"{arms_file} redefines a built-in arm name"
            continue
        if not _ARM_NAME.match(name) or "__" in name:
            # Arm names become run directory names (<task>__<arm>__r<rep>).
            registry.unavailable[name] = f"{arms_file}: arm names must be letters, digits, '.', '_' or '-'"
            continue
        try:
            registry.arms[name] = third_party_arm(name, entry)
        except ValueError as error:
            registry.unavailable[name] = f"{arms_file}: {error}"
    registry.third_party_servers = frozenset(servers)
    return registry


def substitute_placeholders(value: Any, replacements: Mapping[str, str]) -> Any:
    """Replace {repo} and {third_party_dir} in every string of a JSON value."""
    if isinstance(value, str):
        for key, replacement in replacements.items():
            value = value.replace("{" + key + "}", replacement)
        return value
    if isinstance(value, list):
        return [substitute_placeholders(item, replacements) for item in value]
    if isinstance(value, dict):
        return {key: substitute_placeholders(item, replacements) for key, item in value.items()}
    return value


def mcp_config(arm: ArmSpec, repo: Any, third_party_dir: Any) -> dict[str, Any]:
    """The --mcp-config document for one run of a third-party arm."""
    if arm.mcp_server is None or arm.server_name is None:
        raise EvalError(f"arm {arm.name} has no MCP server")
    replacements = {"repo": str(repo), "third_party_dir": str(third_party_dir)}
    return {"mcpServers": {arm.server_name: substitute_placeholders(dict(arm.mcp_server), replacements)}}


def _executable_found(command: str) -> bool:
    if os.path.sep in command or (os.path.altsep and os.path.altsep in command):
        return Path(command).is_file()
    return shutil.which(command) is not None


def third_party_problems(arm: ArmSpec, third_party_dir: Path) -> list[str]:
    """Local checks for a third-party arm: its command (and any install path) exists."""
    if arm.mcp_server is None:
        return []
    install = str(Path(third_party_dir).resolve())
    server = mcp_config(arm, Path(tempfile.gettempdir()) / "ue-eval-dummy-repo", install)["mcpServers"][
        arm.server_name
    ]
    problems = []
    command = server["command"]
    if not _executable_found(command):
        problems.append(f"arm {arm.name}: MCP server command not found: {command}")
    for item in server.get("args") or []:
        if isinstance(item, str) and item.startswith(install) and not Path(item).exists():
            problems.append(f"arm {arm.name}: missing install path {item}")
    if problems:
        problems[-1] += f" (install it as described in {THIRD_PARTY_README})"
    return problems


# ---------------------------------------------------------------------------
# Tasks and file trees


@dataclass
class Task:
    name: str
    path: Path
    prompt: str
    fixture: dict[str, bytes]
    expected: dict[str, bytes]

    def expected_tree(self) -> dict[str, bytes]:
        tree = dict(self.fixture)
        tree.update(self.expected)
        return tree


def read_tree(root: Path, ignore_top_level: Container[str] = ()) -> dict[str, bytes]:
    """Map POSIX relative paths to exact bytes. Symlinks are recorded, not followed."""
    root = Path(root)
    tree: dict[str, bytes] = {}
    for current, dirnames, filenames in os.walk(root):
        base = Path(current)
        relative = base.relative_to(root)
        top = relative == Path(".")
        kept = []
        for name in sorted(dirnames):
            path = base / name
            if top and name in ignore_top_level:
                continue
            if path.is_symlink():
                tree[(relative / name).as_posix()] = b"symlink -> " + os.fsencode(os.readlink(path))
                continue
            kept.append(name)
        dirnames[:] = kept
        for name in sorted(filenames):
            if top and name in ignore_top_level:
                continue
            path = base / name
            key = (relative / name).as_posix()
            if path.is_symlink():
                tree[key] = b"symlink -> " + os.fsencode(os.readlink(path))
            else:
                tree[key] = path.read_bytes()
    return tree


def write_tree(root: Path, files: Mapping[str, bytes]) -> None:
    for relative, data in files.items():
        path = Path(root).joinpath(*relative.split("/"))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def load_task(path: Path) -> Task:
    path = Path(path)
    prompt_path = path / "prompt.md"
    fixture_dir = path / "fixture"
    expected_dir = path / "expected"
    for required in (prompt_path, fixture_dir, expected_dir):
        if not required.exists():
            raise EvalError(f"task {path.name}: missing {required.name}")
    try:
        prompt = prompt_path.read_bytes().decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvalError(f"task {path.name}: prompt.md is not UTF-8: {error}") from error
    return Task(path.name, path, prompt, read_tree(fixture_dir), read_tree(expected_dir))


def validate_task(task: Task) -> list[str]:
    problems = []
    if not task.prompt.strip():
        problems.append("prompt.md is empty")
    if not task.fixture:
        problems.append("fixture/ has no files")
    if not task.expected:
        problems.append("expected/ has no files")
    for relative, data in sorted(task.expected.items()):
        if relative not in task.fixture:
            problems.append(f"expected/{relative} has no counterpart in fixture/")
        elif task.fixture[relative] == data:
            problems.append(f"expected/{relative} is identical to the fixture")
    for relative in sorted(task.fixture):
        if relative.split("/", 1)[0] in IGNORED_TOP_LEVEL:
            problems.append(f"fixture/{relative} is inside a directory the scorer ignores")
    return problems


def discover_tasks(tasks_dir: Path, names: Sequence[str] | None = None) -> list[Task]:
    tasks_dir = Path(tasks_dir)
    if not tasks_dir.is_dir():
        raise EvalError(f"tasks directory not found: {tasks_dir}")
    available = {
        entry.name: entry
        for entry in sorted(tasks_dir.iterdir())
        if entry.is_dir() and (entry / "prompt.md").is_file()
    }
    if names:
        unknown = [name for name in names if name not in available]
        if unknown:
            raise EvalError(f"unknown task(s): {', '.join(unknown)}; available: {', '.join(available)}")
        selected = [available[name] for name in dict.fromkeys(names)]
    else:
        selected = list(available.values())
    if not selected:
        raise EvalError(f"no tasks found in {tasks_dir}")
    tasks = [load_task(path) for path in selected]
    for task in tasks:
        problems = validate_task(task)
        if problems:
            raise EvalError(f"task {task.name} is malformed: " + "; ".join(problems))
    return tasks


@dataclass
class Comparison:
    mismatched: list[str]
    missing: list[str]
    unexpected: list[str]

    @property
    def correct(self) -> bool:
        return not (self.mismatched or self.missing or self.unexpected)

    def to_json(self) -> dict[str, list[str]]:
        return {"mismatched": self.mismatched, "missing": self.missing, "unexpected": self.unexpected}

    def reason(self) -> str:
        parts = []
        for label, paths in (
            ("wrong bytes", self.mismatched),
            ("missing", self.missing),
            ("unexpected", self.unexpected),
        ):
            if paths:
                shown = ", ".join(paths[:4]) + (f" (+{len(paths) - 4})" if len(paths) > 4 else "")
                parts.append(f"{label}: {shown}")
        return "; ".join(parts)


def compare_trees(expected: Mapping[str, bytes], actual: Mapping[str, bytes]) -> Comparison:
    """Exact byte comparison of two trees keyed by POSIX relative path."""
    common = expected.keys() & actual.keys()
    return Comparison(
        mismatched=sorted(path for path in common if expected[path] != actual[path]),
        missing=sorted(expected.keys() - actual.keys()),
        unexpected=sorted(actual.keys() - expected.keys()),
    )


def _visible_lines(data: bytes) -> list[str]:
    text = data.decode("utf-8", errors="replace")
    return [json.dumps(line) for line in text.splitlines(keepends=True)] or ['""']


def describe_differences(
    expected: Mapping[str, bytes], actual: Mapping[str, bytes], comparison: Comparison
) -> str:
    """Readable report; each diff line is a JSON string so CR, tabs, and spaces show."""
    out = ["Final repository bytes differ from the expected state.", ""]
    for path in comparison.mismatched:
        out.append(f"== wrong bytes: {path} ==")
        out.extend(
            difflib.unified_diff(
                _visible_lines(expected[path]),
                _visible_lines(actual[path]),
                fromfile=f"expected/{path}",
                tofile=f"actual/{path}",
                lineterm="",
            )
        )
        out.append("")
    for path in comparison.missing:
        out.append(f"== missing: {path} ==")
    for path in comparison.unexpected:
        out.append(f"== unexpected: {path} ({len(actual[path])} bytes) ==")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------
# Stream-json parsing


@dataclass
class ToolResult:
    is_error: bool
    text: str
    structured: Any = None


@dataclass
class ToolCall:
    id: str
    name: str
    input: Any
    parent_tool_use_id: str | None
    result: ToolResult | None = None


@dataclass
class ApiCall:
    """One model request. Stream lines that share a message id are one call."""

    id: str
    parent_tool_use_id: str | None
    input_tokens: int = 0
    cache_read_input_tokens: int = 0
    cache_creation_input_tokens: int = 0
    output_tokens: int = 0

    @property
    def context(self) -> int:
        return self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens

    def absorb(self, usage: Mapping[str, Any]) -> None:
        """Repeated lines repeat (or grow) the same usage: keep the largest value per field."""
        for key in (
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ):
            value = _number(usage.get(key))
            if value is not None:
                setattr(self, key, max(getattr(self, key), int(value)))


@dataclass
class Transcript:
    init: dict[str, Any] | None = None
    result: dict[str, Any] | None = None
    tool_calls: list[ToolCall] = field(default_factory=list)
    api_calls: list[ApiCall] = field(default_factory=list)
    assistant_message_ids: list[str] = field(default_factory=list)
    hook_responses: list[dict[str, Any]] = field(default_factory=list)
    permission_denied_events: list[dict[str, Any]] = field(default_factory=list)
    api_errors: list[str] = field(default_factory=list)
    api_retries: int = 0
    orphan_results: int = 0
    parse_errors: int = 0
    lines: int = 0


def _content_text(content: Any) -> str:
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(str(block.get("text", "")))
            elif isinstance(block, str):
                parts.append(block)
        return "\n".join(parts)
    return json.dumps(content)


def _message_body(message: Mapping[str, Any]) -> dict[str, Any]:
    body = message.get("message")
    return body if isinstance(body, dict) else {}


def parse_stream(lines: Iterable[str]) -> Transcript:
    """Collect tool calls, results, hook events, and the final result message."""
    transcript = Transcript()
    calls: dict[str, ToolCall] = {}
    seen_messages: set[str] = set()
    api_calls: dict[str, ApiCall] = {}
    for raw in lines:
        raw = raw.strip()
        if not raw:
            continue
        transcript.lines += 1
        try:
            message = json.loads(raw)
        except json.JSONDecodeError:
            transcript.parse_errors += 1
            continue
        if not isinstance(message, dict):
            transcript.parse_errors += 1
            continue
        kind = message.get("type")
        if kind == "system":
            subtype = message.get("subtype")
            if subtype == "init" and transcript.init is None:
                transcript.init = message
            elif subtype == "hook_response":
                transcript.hook_responses.append(message)
            elif subtype == "permission_denied":
                transcript.permission_denied_events.append(message)
            elif subtype == "api_retry":
                transcript.api_retries += 1
        elif kind == "assistant":
            body = _message_body(message)
            parent = message.get("parent_tool_use_id")
            if message.get("error"):
                transcript.api_errors.append(str(message["error"]))
            message_id = body.get("id")
            if parent is None and isinstance(message_id, str) and message_id not in seen_messages:
                seen_messages.add(message_id)
                transcript.assistant_message_ids.append(message_id)
            if body.get("model") != "<synthetic>":  # locally generated error messages
                key = message_id if isinstance(message_id, str) else f"line-{transcript.lines}"
                api_call = api_calls.get(key)
                if api_call is None:
                    api_call = api_calls[key] = ApiCall(key, parent)
                    transcript.api_calls.append(api_call)
                usage = body.get("usage")
                if isinstance(usage, dict):
                    api_call.absorb(usage)
            content = body.get("content")
            for block in content if isinstance(content, list) else []:
                if not isinstance(block, dict) or block.get("type") != "tool_use":
                    continue
                tool_id = str(block.get("id", ""))
                if tool_id and tool_id in calls:
                    continue
                call = ToolCall(tool_id, str(block.get("name", "")), block.get("input"), parent)
                if tool_id:
                    calls[tool_id] = call
                transcript.tool_calls.append(call)
        elif kind == "user":
            content = _message_body(message).get("content")
            blocks = [
                block
                for block in (content if isinstance(content, list) else [])
                if isinstance(block, dict) and block.get("type") == "tool_result"
            ]
            # tool_use_result describes the message's single result; ambiguous otherwise.
            structured = message.get("tool_use_result") if len(blocks) == 1 else None
            for block in blocks:
                owner = calls.get(str(block.get("tool_use_id", "")))
                if owner is None:
                    transcript.orphan_results += 1
                elif owner.result is None:
                    owner.result = ToolResult(
                        bool(block.get("is_error")), _content_text(block.get("content")), structured
                    )
        elif kind == "result":
            transcript.result = message
    return transcript


def parse_stream_file(path: Path) -> Transcript:
    if not Path(path).exists():
        return Transcript()
    with open(path, encoding="utf-8", errors="replace") as handle:
        return parse_stream(handle)


# ---------------------------------------------------------------------------
# Metrics


def mcp_parts(name: str) -> tuple[str, str] | None:
    if not name.startswith("mcp__"):
        return None
    rest = name[5:]
    split = rest.find("__")
    if split <= 0:
        return None
    return rest[:split], rest[split + 2 :]


def short_tool_name(name: str) -> str:
    parts = mcp_parts(name)
    return f"mcp:{parts[1]}" if parts else name


def is_ultra_name(name: Any) -> bool:
    text = str(name or "").lower()
    return "ultra-edit" in text or "ultra_edit" in text


def ultra_tool(name: str) -> str | None:
    """The server-side Ultra Edit tool name, or None for any other tool."""
    parts = mcp_parts(name)
    if parts and is_ultra_name(parts[0]) and parts[1] in ULTRA_TOOLS:
        return parts[1]
    return None


def _json_object(text: Any) -> dict[str, Any] | None:
    if not isinstance(text, str):
        return None
    text = text.strip()
    candidates = [text]
    start, end = text.find("{"), text.rfind("}")
    if 0 < start < end:
        candidates.append(text[start : end + 1])
    for candidate in candidates:
        try:
            value = json.loads(candidate)
        except (json.JSONDecodeError, ValueError):
            continue
        if isinstance(value, dict):
            return value
    return None


def ultra_result_status(tool: str, result: ToolResult | None) -> str:
    """ok, rejected, error, commit:<status>, or missing for one Ultra Edit call."""
    if result is None:
        return "missing"
    data = _json_object(result.text)
    if data is None and isinstance(result.structured, dict):
        data = result.structured
    if data is not None:
        kind = data.get("kind")
        if kind == "error":
            return "error"
        if kind == "rejected" or data.get("ready") is False:
            return "rejected"
        commit = data.get("commit")
        if tool in ULTRA_WRITE_TOOLS and isinstance(commit, str) and commit != "committed":
            return f"commit:{commit}"
    return "error" if result.is_error else "ok"


_QUOTED = re.compile(r"'[^']*'|\"(?:\\.|[^\"\\])*\"")
# Heredoc marker (<<EOF, <<'EOF', <<-"EOF", <<\EOF), not a <<< here-string.
_HEREDOC = re.compile(r"(?<!<)<<(?!<)-?[ \t]*(?:'([^'\n]*)'|\"([^\"\n]*)\"|\\?([A-Za-z_][A-Za-z0-9_.-]*))")
_REDIRECT = re.compile(r"(?<![0-9&<>])>>?(?![>&|(])[ \t]*(?!/dev/(?:null|stdout|stderr|tty)\b)[^\s|;&<>()]")
_SEGMENT_SPLIT = re.compile(r"\|\||&&|[|;\n]")
_ECHO = re.compile(r"^(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*(?:builtin\s+|command\s+)?(?:echo|printf)\b")
# A flag cluster holding i (sed -i, perl -pi, perl -0pi), or --in-place.
_IN_PLACE_FLAG = re.compile(r"\s(?:-(?!-)[A-Za-z0-9]*i|--in-place)")
_IN_PLACE_EDITOR = re.compile(r"\b(?:g?sed|perl)\b")
_AWK_IN_PLACE = re.compile(r"\b(?:g?awk|mawk|nawk)\b.*\s(?:-i\s*|--include[=\s]\s*)inplace(?:\.awk)?\b")
# Edits that do not embed content in the command, so the guard allows them and they
# are not Bash writes, but they still change files: applying a patch file, and a
# filter's output redirected into a file (sed ... > f.tmp && mv f.tmp f).
_ENV_PREFIX = r"^(?:env\s+)?(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*"
_PATCH_APPLY = re.compile(
    _ENV_PREFIX
    + r"(?:git(?:\s+-[Cc]\s+\S+|\s+--[A-Za-z-]+=\S+)*\s+apply\b"
    + r"(?!.*\s--(?:check|stat|numstat|summary)\b)"
    + r"|patch\b(?!.*\s--(?:dry-run|check)\b))"
)
_FILTER = re.compile(
    _ENV_PREFIX + r"(?:g?sed|g?awk|mawk|nawk|perl|tr|grep|egrep|fgrep|cut|sort|uniq|head|tail|cat|iconv|jq|yq"
    r"|envsubst|expand|unexpand|dos2unix|unix2dos)\b"
)
_TEE = re.compile(r"\btee\b(?:\s+-{1,2}[A-Za-z-]+)*\s+(?!/dev/(?:null|stdout|stderr|tty)\b)[^\s|;&<>()-]")
_HEREDOC_WRITER = re.compile(r"\btee\b|\bgit\s+apply\b|\bpatch\b")
_INTERPRETER = re.compile(
    r"(?:^|[\s;&|(])(?:python[0-9.]*|py|node|deno|bun|ruby|php|perl)(?:\.exe)?"
    r"(?:\s+-[A-Za-z]+)*?\s+(?:(?:-c|-e|--eval|-)(?=[\s'\"]|$)|(?=<<))"
)
_POWERSHELL = re.compile(r"(?i)\b(?:pwsh|powershell)(?:\.exe)?\b")
_WRITE_API = re.compile(
    r"\.write_text\(|\.write_bytes\(|(?<!stdout)(?<!stderr)\.write\("
    r"|\bopen\([^)\n]*,\s*['\"][rwa]?[wa]b?\+?['\"]"
    r"|writeFileSync|appendFileSync|\bwriteFile\(|\bappendFile\(|\bFile\.write|\bIO\.write|file_put_contents"
)
_PS_WRITE = re.compile(
    r"(?i)\b(?:Set-Content|Add-Content|Out-File)\b|\bNew-Item\b[^\n|;]*-Value\b"
    r"|\[(?:System\.)?IO\.File\]::(?:Write|Append)"
)


def _strip_heredoc_bodies(command: str) -> str:
    kept: list[str] = []
    pending: list[str] = []
    for line in command.split("\n"):
        if pending:
            if line.lstrip("\t") == pending[0]:
                pending.pop(0)
            continue
        kept.append(line)
        for match in _HEREDOC.finditer(line):
            pending.append(next(group for group in match.groups() if group is not None))
    return "\n".join(kept)


def shell_write_kinds(command: Any, tool: str = "Bash") -> list[str]:
    """Heuristic: does this shell command write file content? Returns matched kinds.

    Kinds: heredoc_write (heredoc feeding a redirect, tee, git apply, or patch),
    echo_redirect (echo/printf redirected to a file), in_place (sed/perl -i),
    tee_write, inline_script (python/node/... -c/-e/stdin script calling a write
    API), and powershell_write (Set-Content, Out-File, redirects, ...).
    """
    if not isinstance(command, str) or not command.strip():
        return []
    kinds: set[str] = set()
    without_bodies = _strip_heredoc_bodies(command)
    for heredoc in _HEREDOC.finditer(without_bodies):
        # The command line that opens the heredoc decides where its body goes.
        line_start = without_bodies.rfind("\n", 0, heredoc.start()) + 1
        line_end = without_bodies.find("\n", heredoc.end())
        head = _QUOTED.sub(
            "''", without_bodies[line_start : len(without_bodies) if line_end < 0 else line_end]
        )
        if _REDIRECT.search(head) or _HEREDOC_WRITER.search(head):
            kinds.add("heredoc_write")
    body_free = _QUOTED.sub("''", without_bodies)
    for segment in _SEGMENT_SPLIT.split(body_free):
        segment = segment.strip().lstrip("({ ").strip()
        if not segment:
            continue
        if _ECHO.match(segment) and _REDIRECT.search(segment):
            kinds.add("echo_redirect")
        if (_IN_PLACE_EDITOR.search(segment) and _IN_PLACE_FLAG.search(segment)) or _AWK_IN_PLACE.search(
            segment
        ):
            kinds.add("in_place")
        if _TEE.search(segment):
            kinds.add("tee_write")
    if _INTERPRETER.search(command) and _WRITE_API.search(command):
        kinds.add("inline_script")
    powershell = tool == "PowerShell"
    if (powershell or _POWERSHELL.search(command)) and (
        _PS_WRITE.search(command) or (powershell and _REDIRECT.search(_QUOTED.sub("''", command)))
    ):
        kinds.add("powershell_write")
    return sorted(kinds)


def shell_edit_kinds(command: Any) -> list[str]:
    """Shell file edits that embed no content, so shell_write_kinds misses them:
    patch_apply (git apply or patch, not a check or stat run) and filter_redirect
    (sed, awk, perl, grep, ... with stdout redirected into a file). They count as edit
    calls, so a failed `git apply fix.patch` is a failed edit, but not as Bash writes."""
    if not isinstance(command, str) or not command.strip():
        return []
    kinds: set[str] = set()
    body_free = _QUOTED.sub("''", _strip_heredoc_bodies(command))
    for segment in _SEGMENT_SPLIT.split(body_free):
        segment = segment.strip().lstrip("({ ").strip()
        if _PATCH_APPLY.match(segment):
            kinds.add("patch_apply")
        elif _FILTER.match(segment) and "<<" not in segment and _REDIRECT.search(segment):
            kinds.add("filter_redirect")
    return sorted(kinds)


def hook_denied(event: Mapping[str, Any]) -> bool:
    if event.get("exit_code") == 2:
        return True
    for key in ("stdout", "output"):
        data = _json_object(event.get(key))
        if not data:
            continue
        specific = data.get("hookSpecificOutput")
        if isinstance(specific, dict) and specific.get("permissionDecision") == "deny":
            return True
        if data.get("decision") in ("block", "deny"):
            return True
    return False


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return value


def token_usage(result: Mapping[str, Any] | None) -> dict[str, Any]:
    """Token totals from modelUsage (all models, subagents included), else usage."""
    keys = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
    usage: dict[str, Any] = {key: None for key in keys}
    usage.update({"total_input_tokens": None, "cost_usd": None, "models": [], "token_source": None})
    if not result:
        return usage
    model_usage = result.get("modelUsage")
    camel = ("inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens")
    if isinstance(model_usage, dict) and model_usage:
        totals = dict.fromkeys(keys, 0)
        for entry in model_usage.values():
            if isinstance(entry, dict):
                for key, source in zip(keys, camel):
                    totals[key] += int(_number(entry.get(source)) or 0)
        usage.update(totals)
        usage["models"] = sorted(str(name) for name in model_usage)
        usage["token_source"] = "modelUsage"
    elif isinstance(result.get("usage"), dict):
        for key in keys:
            usage[key] = int(_number(result["usage"].get(key)) or 0)
        usage["token_source"] = "usage"
    if usage["input_tokens"] is not None:
        usage["total_input_tokens"] = (
            usage["input_tokens"] + usage["cache_read_input_tokens"] + usage["cache_creation_input_tokens"]
        )
    cost = _number(result.get("total_cost_usd"))
    usage["cost_usd"] = float(cost) if cost is not None else None
    return usage


_DRIVE = re.compile(r"^[A-Za-z]:/")


def _read_target(call: ToolCall, cwd: str | None) -> str | None:
    """The normalized path a Read or Ultra Edit snapshot call reads, else None."""
    if not isinstance(call.input, dict):
        return None
    if call.name == "Read":
        path = call.input.get("file_path")
    elif ultra_tool(call.name) == "ultra_edit_snapshot":
        path = call.input.get("path")
    else:
        return None
    if not isinstance(path, str) or not path:
        return None
    text = path.replace("\\", "/")
    if cwd and not (text.startswith("/") or _DRIVE.match(text)):
        text = cwd.replace("\\", "/").rstrip("/") + "/" + text
    text = posixpath.normpath(text)
    return text.lower() if _DRIVE.match(text) else text


def _utf8_len(text: str) -> int:
    return len(text.encode("utf-8", errors="replace"))


def _input_bytes(value: Any) -> int:
    return _utf8_len(json.dumps(value, separators=(",", ":"), ensure_ascii=False))


def api_call_metrics(transcript: Transcript) -> dict[str, Any]:
    """Per-request context and output, from assistant messages deduplicated by id."""
    calls = transcript.api_calls
    main = [call for call in calls if not call.parent_tool_use_id]
    return {
        "api_calls": len(calls),
        "subagent_api_calls": len(calls) - len(main),
        "first_call_context_tokens": main[0].context if main else None,
        "peak_context_tokens": max((call.context for call in calls), default=None),
        "context_tokens_total": sum(call.context for call in calls) if calls else None,
        "output_tokens_calls": sum(call.output_tokens for call in calls) if calls else None,
        "context_series": [[call.context, call.output_tokens] for call in main],
    }


def compute_metrics(transcript: Transcript, edit_tools: Collection[str] = ()) -> dict[str, Any]:
    """Run metrics. edit_tools adds full tool names (third-party MCP edit tools) to the
    edit calls; native writes, content-writing shell commands, and Ultra Edit commits
    always count."""
    by_name: Counter[str] = Counter()
    ultra_statuses: Counter[str] = Counter()
    write_kinds: Counter[str] = Counter()
    edit_kinds: Counter[str] = Counter()
    counts: Counter[str] = Counter()
    input_bytes: Counter[str] = Counter()
    result_bytes: Counter[str] = Counter()
    cwd = (transcript.init or {}).get("cwd")
    read_paths: set[str] = set()
    for call in transcript.tool_calls:
        by_name[call.name] += 1
        result = call.result
        input_bytes[call.name] += _input_bytes(call.input)
        if result is not None:
            result_bytes[call.name] += _utf8_len(result.text)
        if call.parent_tool_use_id:
            counts["subagent_tool_calls"] += 1
        if result is None:
            counts["unanswered_tool_calls"] += 1
        failed = result is not None and result.is_error
        is_edit = False
        ultra = ultra_tool(call.name)
        target = _read_target(call, cwd if isinstance(cwd, str) else None)
        if call.name == "Read" or ultra == "ultra_edit_snapshot":
            counts["read_calls"] += 1
            if target is not None and target in read_paths:
                counts["reread_calls"] += 1
        if target is not None:
            read_paths.add(target)
        if call.name == "ToolSearch":
            counts["toolsearch_calls"] += 1
        elif call.name == "Skill":
            counts["skill_calls"] += 1
        if ultra:
            status = ultra_result_status(ultra, result)
            ultra_statuses[status] += 1
            if status not in ("ok", "missing"):
                failed = True
            if status == "rejected" or status.startswith("commit:"):
                counts["ultra_rejections"] += 1
            if ultra in ULTRA_RECOVERY_TOOLS:
                counts["ultra_recovery_calls"] += 1
            is_edit = ultra in ULTRA_WRITE_TOOLS
        elif call.name in NATIVE_WRITE_TOOLS or call.name in edit_tools:
            is_edit = True
        elif call.name in SHELL_TOOLS:
            counts["shell_calls"] += 1
            if result is not None and result.is_error:
                counts["shell_errors"] += 1
            command = call.input.get("command") if isinstance(call.input, dict) else None
            kinds = shell_write_kinds(command, call.name)
            other_edits = shell_edit_kinds(command) if call.name == "Bash" else []
            edit_kinds.update(other_edits)
            is_edit = bool(kinds or other_edits)
            if kinds:
                counts["bash_write_attempts"] += 1
                write_kinds.update(kinds)
                if result is not None and not result.is_error:
                    counts["bash_writes"] += 1
                elif result is not None:
                    counts["bash_writes_blocked"] += 1
        if failed:
            counts["tool_errors"] += 1
        if is_edit:
            counts["edit_calls"] += 1
            if failed:
                counts["edit_failures"] += 1
    final = transcript.result or {}
    turns = final.get("num_turns")
    denials = final.get("permission_denials")
    final_text = final.get("result")
    metrics: dict[str, Any] = {
        "tool_calls": len(transcript.tool_calls),
        "tool_calls_by_name": dict(sorted(by_name.items())),
        "tool_errors": counts["tool_errors"],
        "edit_calls": counts["edit_calls"],
        "edit_failures": counts["edit_failures"],
        "bash_write_attempts": counts["bash_write_attempts"],
        "bash_writes": counts["bash_writes"],
        "bash_writes_blocked": counts["bash_writes_blocked"],
        "bash_write_kinds": dict(sorted(write_kinds.items())),
        "shell_edit_kinds": dict(sorted(edit_kinds.items())),
        "ultra_edit_statuses": dict(sorted(ultra_statuses.items())),
        "ultra_rejections": counts["ultra_rejections"],
        "ultra_recovery_calls": counts["ultra_recovery_calls"],
        "hook_denials": sum(
            1
            for event in transcript.hook_responses
            if event.get("hook_event") == "PreToolUse" and hook_denied(event)
        ),
        "permission_denials": len(denials) if isinstance(denials, list) else 0,
        "subagent_tool_calls": counts["subagent_tool_calls"],
        "unanswered_tool_calls": counts["unanswered_tool_calls"],
        "shell_calls": counts["shell_calls"],
        "shell_errors": counts["shell_errors"],
        "toolsearch_calls": counts["toolsearch_calls"],
        "read_calls": counts["read_calls"],
        "reread_calls": counts["reread_calls"],
        "skill_calls": counts["skill_calls"],
        "tool_input_bytes": sum(input_bytes.values()),
        "tool_result_bytes": sum(result_bytes.values()),
        "tool_input_bytes_by_tool": dict(sorted(input_bytes.items())),
        "tool_result_bytes_by_tool": dict(sorted(result_bytes.items())),
        "final_text_bytes": _utf8_len(final_text) if isinstance(final_text, str) else None,
        "turns": turns if isinstance(turns, int) else len(transcript.assistant_message_ids),
        "result_subtype": final.get("subtype"),
        "result_is_error": final.get("is_error"),
        "stop_reason": final.get("stop_reason"),
        "terminal_reason": final.get("terminal_reason"),
        "duration_ms": final.get("duration_ms"),
        "duration_api_ms": final.get("duration_api_ms"),
        "api_errors": list(transcript.api_errors),
        "api_retries": transcript.api_retries,
        "stream_lines": transcript.lines,
        "stream_parse_errors": transcript.parse_errors,
    }
    metrics.update(api_call_metrics(transcript))
    metrics.update(token_usage(transcript.result))
    return metrics


def _normalized_path(value: str) -> str:
    return os.path.normcase(os.path.realpath(os.path.abspath(value)))


def _same_path(left: Any, right: Path) -> bool:
    return isinstance(left, str) and bool(left) and _normalized_path(left) == _normalized_path(str(right))


def guarded_tools(matcher: str) -> frozenset[str]:
    """The shell tools that an exact `A|B` guard matcher names."""
    return frozenset(matcher.split("|")) & SHELL_TOOLS


def arm_spec(arm: str | ArmSpec) -> ArmSpec:
    if isinstance(arm, ArmSpec):
        return arm
    if arm not in BUILTIN_ARMS:
        raise EvalError(f"unknown arm {arm!r}")
    return BUILTIN_ARMS[arm]


def _server_named(name: Any, servers: Collection[str]) -> str | None:
    text = str(name or "")
    return text if text in servers else None


def _tool_server(tool: str, servers: Collection[str]) -> str | None:
    parts = mcp_parts(tool)
    return parts[0] if parts and parts[0] in servers else None


def _third_party_errors(
    spec: ArmSpec, transcript: Transcript, servers: list[dict[str, Any]], tools: list[str]
) -> list[str]:
    """A third-party arm needs its server connected and its required tools listed."""
    errors = []
    own = [entry for entry in servers if str(entry.get("name")) == spec.server_name]
    statuses = sorted({str(entry.get("status")) for entry in own})
    if not own:
        errors.append(f"{spec.server_name} MCP server missing from init")
    elif "connected" not in statuses:
        errors.append(f"{spec.server_name} MCP server status: {', '.join(statuses)}")
    missing = [tool for tool in spec.required_tools if tool not in tools]
    if missing:
        errors.append(f"required {spec.server_name} tools missing at init: {', '.join(missing)}")
    elif not any(_tool_server(tool, {str(spec.server_name)}) for tool in tools):
        # A connected server with no tools leaves only native tools: not this arm.
        errors.append(f"no {spec.server_name} tools listed at init")
    return errors


def check_integrity(
    arm: str | ArmSpec,
    transcript: Transcript,
    staged_plugin_dir: Path | None = None,
    hook_events_expected: bool = True,
    guard_matcher: str = DEFAULT_GUARD_MATCHER,
    third_party_servers: Collection[str] = (),
) -> tuple[list[str], list[str]]:
    """Errors mean the run did not test its arm (leaked or missing plugin, hook, or server,
    or a disallowed tool that was still offered)."""
    spec = arm_spec(arm)
    name = spec.name
    errors: list[str] = []
    warnings: list[str] = []
    init = transcript.init
    if init is None:
        return ["no system/init message: Claude Code did not start a session"], warnings
    plugins = [entry for entry in init.get("plugins") or [] if isinstance(entry, dict)]
    ultra_plugins = [entry for entry in plugins if str(entry.get("name", "")).lower() == PLUGIN_NAME]
    other_plugins = sorted(str(entry.get("name")) for entry in plugins if entry not in ultra_plugins)
    servers = [entry for entry in init.get("mcp_servers") or [] if isinstance(entry, dict)]
    ultra_servers = [entry for entry in servers if is_ultra_name(entry.get("name"))]
    tools = [str(tool) for tool in init.get("tools") or []]
    listed_ultra_tools = [tool for tool in tools if ultra_tool(tool)]
    ultra_calls = [call for call in transcript.tool_calls if ultra_tool(call.name)]
    hooks = transcript.hook_responses
    if spec.plugin:
        if not ultra_plugins:
            errors.append("Ultra Edit plugin did not load")
        elif len(ultra_plugins) > 1:
            errors.append(f"{len(ultra_plugins)} Ultra Edit plugins loaded")
        elif staged_plugin_dir is not None and not _same_path(
            ultra_plugins[0].get("path"), staged_plugin_dir
        ):
            # --plugin-dir overrides a same-named installed copy (Claude Code 2.1.74+), and
            # installed ids are disabled in settings, so treat a path mismatch as a warning.
            warnings.append(
                f"Ultra Edit reported path {ultra_plugins[0].get('path')}, staged {staged_plugin_dir}"
            )
        for problem in init.get("plugin_errors") or []:
            if "ultra" in json.dumps(problem).lower():
                errors.append(f"plugin error: {json.dumps(problem)[:300]}")
        statuses = sorted({str(entry.get("status")) for entry in ultra_servers})
        if not ultra_servers:
            errors.append("Ultra Edit MCP server missing from init")
        elif "connected" not in statuses:
            if "pending" in statuses:
                warnings.append("Ultra Edit MCP server still pending at init")
            else:
                errors.append(f"Ultra Edit MCP server status: {', '.join(statuses)}")
        if not listed_ultra_tools and not ultra_calls:
            warnings.append("no Ultra Edit tools listed at init")
        session = [event for event in hooks if event.get("hook_event") == "SessionStart"]
        if any(event.get("outcome") == "error" for event in session):
            errors.append("SessionStart routing hook failed")
        elif not session:
            warnings.append("no SessionStart hook event; routing context unverified")
    else:
        if ultra_plugins:
            errors.append(f"Ultra Edit plugin loaded in the {name} arm")
        if ultra_servers or listed_ultra_tools:
            errors.append(f"Ultra Edit MCP tools present in the {name} arm")
        if ultra_calls:
            errors.append(f"Ultra Edit tools called in the {name} arm")
    foreign = set(third_party_servers) - {spec.server_name}
    if spec.third_party:
        errors += _third_party_errors(spec, transcript, servers, tools)
    leaked_servers = sorted(
        {str(entry.get("name")) for entry in servers if _server_named(entry.get("name"), foreign)}
        | {server for tool in tools if (server := _tool_server(tool, foreign))}
    )
    if leaked_servers:
        errors.append(f"third-party MCP server(s) present in the {name} arm: {', '.join(leaked_servers)}")
    leaked_calls = sorted({call.name for call in transcript.tool_calls if _tool_server(call.name, foreign)})
    if leaked_calls:
        errors.append(f"third-party tools called in the {name} arm: {', '.join(leaked_calls)}")
    offered = [tool for tool in spec.disallowed_tools if tool in tools]
    if offered:
        errors.append(f"disallowed tool(s) listed at init: {', '.join(offered)}")
    pre_tool = [event for event in hooks if event.get("hook_event") == "PreToolUse"]
    if spec.guard_hook:
        failed = [event for event in pre_tool if event.get("outcome") == "error"]
        if failed:
            errors.append(f"guard hook failed {len(failed)} time(s)")
        guarded = guarded_tools(guard_matcher)
        shell_calls = sorted({call.name for call in transcript.tool_calls if call.name in guarded})
        if hook_events_expected and shell_calls and not pre_tool:
            errors.append(
                f"{' and '.join(shell_calls)} called but no PreToolUse hook event was reported; "
                "the guard did not run "
                "(use --no-hook-check if this Claude Code omits PreToolUse hook events)"
            )
    elif not spec.plugin and pre_tool:
        warnings.append(f"PreToolUse hooks ran in the {name} arm (managed hooks?)")
    expected_servers = {spec.server_name} | set(third_party_servers)
    other_servers = sorted(
        str(entry.get("name"))
        for entry in servers
        if entry not in ultra_servers and str(entry.get("name")) not in expected_servers
    )
    if other_servers:
        warnings.append("other MCP servers: " + ", ".join(other_servers))
    if other_plugins:
        warnings.append("other plugins: " + ", ".join(other_plugins))
    return errors, warnings


def classify(
    transcript: Transcript, comparison: Comparison, integrity_errors: Sequence[str], timed_out: bool
) -> str:
    """pass/fail/timeout are scored; invalid and infra_error are excluded from rates."""
    if transcript.init is None:
        return "infra_error"
    if integrity_errors:
        return "invalid"
    if timed_out:
        return "timeout"
    if transcript.result is None:
        return "infra_error"
    if any(error in INFRA_ERRORS for error in transcript.api_errors):
        return "infra_error"
    return "pass" if comparison.correct else "fail"


def first_attempt(metrics: Mapping[str, Any], correct: bool) -> bool:
    return bool(
        correct
        and metrics.get("edit_failures", 0) == 0
        and metrics.get("ultra_rejections", 0) == 0
        and metrics.get("ultra_recovery_calls", 0) == 0
    )


# ---------------------------------------------------------------------------
# Claude CLI, settings, and commands


@dataclass
class ClaudeInfo:
    argv: list[str]
    version: tuple[int, int, int] | None = None
    version_text: str = ""
    flags: frozenset[str] = frozenset()
    probed: bool = False

    def supports(self, flag: str) -> bool:
        return not self.probed or flag in self.flags


def parse_version(text: str) -> tuple[int, int, int] | None:
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", text or "")
    return (int(match.group(1)), int(match.group(2)), int(match.group(3))) if match else None


def flags_from_help(text: str) -> frozenset[str]:
    return frozenset(re.findall(r"(?<![\w-])(--[a-z][A-Za-z0-9-]*)", text or ""))


def run_text(argv: Sequence[str], **options: Any) -> subprocess.CompletedProcess:
    """Run without a shell, capturing output as UTF-8 regardless of the console code page."""
    options.setdefault("stdin", subprocess.DEVNULL)
    return subprocess.run(
        list(argv), capture_output=True, encoding="utf-8", errors="replace", check=False, **options
    )


def resolve_claude(explicit: str | None) -> list[str] | None:
    candidate = explicit or "claude"
    if os.path.sep in candidate or (os.path.altsep and os.path.altsep in candidate):
        return [candidate] if Path(candidate).exists() else None
    found = shutil.which(candidate)
    return [found] if found else None


def probe_claude(argv: Sequence[str], env: Mapping[str, str]) -> ClaudeInfo:
    """Run only `claude --version` and `claude --help`; neither contacts a model."""
    info = ClaudeInfo(list(argv))
    version = run_text([*argv, "--version"], env=dict(env), timeout=120)
    info.version_text = (version.stdout or version.stderr).strip()
    info.version = parse_version(info.version_text)
    help_text = run_text([*argv, "--help"], env=dict(env), timeout=120)
    info.flags = flags_from_help(help_text.stdout + help_text.stderr)
    info.probed = True
    return info


def claude_problems(info: ClaudeInfo) -> list[str]:
    problems = []
    if info.version is None:
        problems.append(f"could not read the Claude Code version from: {info.version_text!r}")
    elif info.version < MIN_CLAUDE_VERSION:
        minimum = ".".join(map(str, MIN_CLAUDE_VERSION))
        problems.append(
            f"Claude Code {info.version_text} is older than {minimum}; update with `claude update`"
        )
    missing = [flag for flag in REQUIRED_FLAGS if flag not in info.flags]
    if missing:
        problems.append("claude --help lacks required flags: " + ", ".join(missing))
    return problems


def deep_merge(base: Mapping[str, Any], extra: Mapping[str, Any]) -> dict[str, Any]:
    merged = dict(base)
    for key, value in extra.items():
        if isinstance(value, Mapping) and isinstance(merged.get(key), Mapping):
            merged[key] = deep_merge(merged[key], value)
        elif isinstance(value, list) and isinstance(merged.get(key), list):
            merged[key] = [*merged[key], *value]
        else:
            merged[key] = value
    return merged


def build_settings(
    arm: str | ArmSpec,
    guard_executable: str | None = None,
    guard_matcher: str = DEFAULT_GUARD_MATCHER,
    extra: Mapping[str, Any] | None = None,
    disabled_plugins: Sequence[str] = (),
) -> dict[str, Any]:
    """The per-run --settings document. Only the guard arm adds a hook."""
    spec = arm_spec(arm)
    settings: dict[str, Any] = {
        "disableClaudeAiConnectors": True,
        "syncClaudeAiPlugins": False,
        "syncClaudeAiSkills": False,
        "autoMemoryEnabled": False,
        "enabledPlugins": {plugin_id: False for plugin_id in (*KNOWN_PLUGIN_IDS, *disabled_plugins)},
    }
    if spec.guard_hook:
        if not guard_executable:
            raise EvalError(f"the {spec.name} arm needs the staged ultra-edit-mcp executable")
        settings["hooks"] = {
            "PreToolUse": [
                {
                    "matcher": guard_matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "command": str(guard_executable),
                            "args": list(GUARD_HOOK_ARGS),
                            "timeout": GUARD_HOOK_TIMEOUT_S,
                        }
                    ],
                }
            ]
        }
    return deep_merge(settings, extra) if extra else settings


def build_command(
    claude_argv: Sequence[str],
    *,
    arm: str | ArmSpec,
    settings_path: Any,
    plugin_dir: Any = None,
    mcp_config_path: Any = None,
    permission_mode: str = "bypassPermissions",
    max_turns: int = 50,
    model: str | None = None,
    max_budget_usd: float | None = None,
    effort: str | None = None,
    setting_sources: str = "",
    allowed_tools: Sequence[str] = DEFAULT_ALLOWED_TOOLS,
    prompt: str | None = None,
    claude: ClaudeInfo | None = None,
) -> list[str]:
    """argv for one run. With prompt=None the prompt is sent on stdin."""
    spec = arm_spec(arm)
    supports = claude.supports if claude else (lambda flag: True)
    argv = [*claude_argv, "-p"]
    if prompt is not None:
        argv.append(prompt)  # directly after -p so no variadic option can absorb it
    argv += ["--output-format", "stream-json", "--verbose"]
    argv.append(f"--setting-sources={setting_sources}")
    argv += ["--settings", str(settings_path)]
    argv.append("--no-session-persistence")
    argv += ["--permission-mode", permission_mode]
    if supports("--permission-prompts"):
        argv += ["--permission-prompts", "none"]
    if supports("--include-hook-events"):
        argv.append("--include-hook-events")
    argv += ["--max-turns", str(max_turns)]
    if max_budget_usd is not None and supports("--max-budget-usd"):
        argv += ["--max-budget-usd", f"{max_budget_usd:g}"]
    if model:
        argv += ["--model", model]
    if effort and supports("--effort"):
        argv += ["--effort", effort]
    rules = [tool for tool in allowed_tools if tool not in spec.disallowed_tools]
    if spec.permission_rule and spec.permission_rule not in rules:
        rules.append(spec.permission_rule)
    if permission_mode != "bypassPermissions" and rules:
        argv += ["--allowedTools", ",".join(rules)]
    if spec.disallowed_tools:
        argv += ["--disallowedTools", ",".join(spec.disallowed_tools)]
    if spec.append_system_prompt:
        argv += ["--append-system-prompt", spec.append_system_prompt]
    if spec.mcp_server is not None:
        if mcp_config_path is None:
            raise EvalError(f"the {spec.name} arm needs an --mcp-config file")
        argv += ["--mcp-config", str(mcp_config_path)]
    if spec.plugin:
        if plugin_dir is None:
            raise EvalError(f"the {spec.name} arm needs a staged plugin directory")
        argv += ["--plugin-dir", str(plugin_dir)]
    return argv


def format_command(argv: Sequence[str], windows: bool | None = None) -> str:
    windows = os.name == "nt" if windows is None else windows
    return subprocess.list2cmdline(list(argv)) if windows else shlex.join(list(argv))


def build_env(base: Mapping[str, str], config_dir: Any = None) -> tuple[dict[str, str], list[str]]:
    env = dict(base)
    upper = {name.upper(): name for name in env}
    removed = []
    for name in STRIPPED_ENV:
        actual = upper.get(name)
        if actual is not None:
            env.pop(actual)
            removed.append(name)
    for upper_name, actual in sorted(upper.items()):
        if upper_name.startswith(STRIPPED_ENV_PREFIXES) and actual in env:
            env.pop(actual)
            removed.append(upper_name)
    env.update(CHILD_ENV)
    if config_dir is not None:
        env["CLAUDE_CONFIG_DIR"] = str(config_dir)
    return env, removed


# ---------------------------------------------------------------------------
# Staging and preflight


@dataclass
class StagedPlugin:
    root: Path
    mcp_executable: Path
    runtime_files: list[str]
    plugin_version: str | None


def mcp_executable_name(windows: bool) -> str:
    return "ultra-edit-mcp.exe" if windows else "ultra-edit-mcp"


def plugin_version(source: Path) -> str | None:
    try:
        manifest = json.loads((Path(source) / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    version = manifest.get("version") if isinstance(manifest, dict) else None
    return version if isinstance(version, str) else None


def stage_plugin(
    source: Path, runtime_dir: Path, destination: Path, windows: bool | None = None
) -> StagedPlugin:
    """Copy plugin/claude-code and place the built executables in its runtime/."""
    windows = os.name == "nt" if windows is None else windows
    source, runtime_dir, destination = Path(source), Path(runtime_dir), Path(destination)
    if not (source / ".claude-plugin" / "plugin.json").is_file():
        raise EvalError(f"not a plugin directory (no .claude-plugin/plugin.json): {source}")
    mcp_name = mcp_executable_name(windows)
    if not (runtime_dir / mcp_name).is_file():
        raise EvalError(
            f"{runtime_dir} has no {mcp_name}. Build it with `cargo build --locked --release` "
            "or pass --runtime-dir DIR"
        )
    if destination.exists():
        remove_tree(destination)
    source_key = os.path.normcase(str(source.resolve()))
    shutil.copytree(
        source,
        destination,
        ignore=lambda directory, names: (
            {"runtime", "__pycache__"} & set(names)
            if os.path.normcase(str(Path(directory).resolve())) == source_key
            else {"__pycache__"} & set(names)
        ),
    )
    runtime = destination / "runtime"
    runtime.mkdir()
    copied = []
    for name in RUNTIME_NAMES:
        if (runtime_dir / name).is_file():
            shutil.copy2(runtime_dir / name, runtime / name)
            copied.append(name)
    if (runtime_dir / "targets").is_dir():
        shutil.copytree(runtime_dir / "targets", runtime / "targets")
        copied.append("targets/")
    executable = runtime / mcp_name
    if not windows:
        for name in copied:
            path = runtime / name
            if path.is_file():
                path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return StagedPlugin(destination.resolve(), executable.resolve(), copied, plugin_version(source))


def hook_output_denies(returncode: int, stdout: bytes | str) -> bool:
    text = stdout.decode("utf-8", errors="replace") if isinstance(stdout, bytes) else stdout
    return hook_denied({"exit_code": returncode, "stdout": text})


def check_guard_hook(
    hook_argv: Sequence[str],
    cwd: Path,
    env: Mapping[str, str],
    guard_matcher: str = DEFAULT_GUARD_MATCHER,
) -> list[str]:
    """Feed the guard synthetic PreToolUse input: it must deny a heredoc write and allow a
    read, and likewise a here-string write and a read when the matcher names PowerShell."""
    probes = [
        ("Bash", "cat > notes.txt <<'EOF'\nC:\\Temp\\new\\file\nEOF", True),
        ("Bash", "git status --short", False),
    ]
    if "PowerShell" in guarded_tools(guard_matcher):
        probes += [
            ("PowerShell", "@'\nC:\\Temp\\new\\file\n'@ | Set-Content notes.txt", True),
            ("PowerShell", "Get-ChildItem -Name", False),
        ]
    problems = []
    for tool, command, should_deny in probes:
        payload = {
            "session_id": "ultra-edit-eval-preflight",
            "transcript_path": str(Path(cwd) / "preflight.jsonl"),
            "cwd": str(cwd),
            "permission_mode": "bypassPermissions",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": {"command": command, "description": "eval preflight"},
            "tool_use_id": "toolu_eval_preflight",
        }
        try:
            completed = subprocess.run(
                list(hook_argv),
                input=json.dumps(payload).encode("utf-8"),
                capture_output=True,
                cwd=str(cwd),
                env=dict(env),
                timeout=GUARD_HOOK_TIMEOUT_S,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            return [f"guard hook could not run: {error}"]
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()[:300]
        denied = hook_output_denies(completed.returncode, completed.stdout)
        if completed.returncode not in (0, 2):
            problems.append(f"guard hook exited {completed.returncode}: {stderr}")
        elif denied != should_deny:
            verdict = "allowed" if should_deny else "denied"
            detail = f" (exit {completed.returncode}; stderr: {stderr})" if stderr else ""
            problems.append(
                f"guard hook {verdict} {tool} {command.splitlines()[0]!r}; expected the opposite{detail}"
            )
    if problems and any("usage" in problem.lower() for problem in problems):
        mode = " ".join(GUARD_HOOK_ARGS)
        problems.append(f"the staged runtime seems to lack `{mode}`; rebuild from a commit that has it")
    return problems


def auth_hint(env: Mapping[str, str], extra_settings: Mapping[str, Any] | None) -> bool:
    return any(env.get(name) for name in AUTH_ENV) or bool(
        extra_settings and extra_settings.get("apiKeyHelper")
    )


# ---------------------------------------------------------------------------
# Running


@dataclass
class RunSpec:
    task: Task
    arm: str
    rep: int

    @property
    def run_id(self) -> str:
        return f"{self.task.name}__{self.arm}__r{self.rep}"


@dataclass
class Config:
    claude: ClaudeInfo
    out_dir: Path
    permission_mode: str = "bypassPermissions"
    max_turns: int = 50
    model: str | None = None
    max_budget_usd: float | None = 2.0
    effort: str | None = None
    timeout_s: float = 900.0
    setting_sources: str = ""
    isolate_config: bool = False
    prompt_via: str = "stdin"
    allowed_tools: tuple[str, ...] = DEFAULT_ALLOWED_TOOLS
    guard_matcher: str = DEFAULT_GUARD_MATCHER
    extra_settings: dict[str, Any] | None = None
    work_dir: Path | None = None
    keep_workdirs: bool = False
    staged: StagedPlugin | None = None
    base_env: Mapping[str, str] | None = None
    hook_check: bool = True
    arm_specs: dict[str, ArmSpec] = field(default_factory=lambda: dict(BUILTIN_ARMS))
    third_party_servers: frozenset[str] = frozenset()
    third_party_dir: Path = DEFAULT_THIRD_PARTY_INSTALL
    disabled_plugins: tuple[str, ...] = ()

    def spec(self, arm: str) -> ArmSpec:
        if arm not in self.arm_specs:
            raise EvalError(f"unknown arm {arm!r}")
        return self.arm_specs[arm]


def plan_runs(tasks: Sequence[Task], arms: Sequence[str], reps: int, seed: int | None = 0) -> list[RunSpec]:
    """Interleave arms per task and repetition; shuffle arm order to spread cache effects."""
    rng = random.Random(seed)
    plan: list[RunSpec] = []
    for rep in range(1, reps + 1):
        for task in tasks:
            order = list(arms)
            if seed is not None:
                rng.shuffle(order)
            plan.extend(RunSpec(task, arm, rep) for arm in order)
    return plan


def _git(args: Sequence[str], cwd: Path, env: Mapping[str, str]) -> None:
    completed = run_text(["git", *args], cwd=str(cwd), env=dict(env))
    if completed.returncode != 0:
        raise EvalError(f"git {' '.join(args)} failed: {completed.stderr.strip()}")


def prepare_repo(files: Mapping[str, bytes], repo: Path, base_env: Mapping[str, str] | None = None) -> None:
    """Write the fixture and commit it with a fixed identity and date.

    Works without any global Git configuration. The repository-local settings
    also keep the model's own git commands from converting line endings,
    signing, or running the user's global hooks.
    """
    repo.mkdir(parents=True)
    write_tree(repo, files)
    env, _ = build_env(os.environ if base_env is None else base_env)
    env.update(GIT_AUTHOR_DATE=GIT_DATE, GIT_COMMITTER_DATE=GIT_DATE)
    _git(["-c", "init.defaultBranch=main", "init", "-q", "--template="], repo, env)
    no_hooks = repo / ".git" / "eval-no-hooks"
    no_hooks.mkdir()
    local = (
        ("core.autocrlf", "false"),
        ("core.safecrlf", "false"),
        ("core.hooksPath", str(no_hooks)),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("user.name", GIT_NAME),
        ("user.email", GIT_EMAIL),
    )
    for key, value in local:
        _git(["config", key, value], repo, env)
    fixed = [item for key, value in local for item in ("-c", f"{key}={value}")]
    _git([*fixed, "add", "--all", "--force"], repo, env)
    _git([*fixed, "commit", "-q", "--no-verify", "-m", "Fixture"], repo, env)


def kill_process_tree(process: subprocess.Popen) -> None:
    """Stop a run and its children: taskkill /T on Windows, the process group elsewhere."""
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(process.pid)], capture_output=True, check=False)
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)  # Claude Code ends its Bash process trees on SIGTERM
    except (ProcessLookupError, PermissionError):
        return
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(process.pid, signal.SIGKILL)


_ACTIVE: set[subprocess.Popen] = set()
_ACTIVE_LOCK = threading.Lock()
_STOPPING = threading.Event()


def stop_active_processes() -> None:
    """Kill every running session and refuse new ones (parallel runs on Ctrl+C: each
    session has its own process group, so the terminal's SIGINT never reaches it)."""
    with _ACTIVE_LOCK:
        _STOPPING.set()
        processes = list(_ACTIVE)
    for process in processes:
        kill_process_tree(process)


def run_process(
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    stdin_bytes: bytes | None,
    stdout_path: Path,
    stderr_path: Path,
    timeout_s: float,
) -> tuple[int | None, bool, float]:
    """Run without a shell in its own process group. Returns (exit code, timed out, seconds)."""
    options: dict[str, Any] = {}
    if os.name == "nt":
        options["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0x200)
    else:
        options["start_new_session"] = True
    start = time.monotonic()
    timed_out = False
    with open(stdout_path, "wb") as stdout, open(stderr_path, "wb") as stderr:
        with _ACTIVE_LOCK:
            if _STOPPING.is_set():
                raise EvalError("evaluation interrupted before this run started")
            process = subprocess.Popen(
                list(argv),
                cwd=str(cwd),
                env=dict(env),
                stdin=subprocess.PIPE if stdin_bytes is not None else subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                **options,
            )
            _ACTIVE.add(process)
        try:
            process.communicate(input=stdin_bytes, timeout=timeout_s)
        except subprocess.TimeoutExpired:
            timed_out = True
            kill_process_tree(process)
            process.communicate()
        except BaseException:
            kill_process_tree(process)
            process.communicate()
            raise
        finally:
            with _ACTIVE_LOCK:
                _ACTIVE.discard(process)
    if os.name != "nt":
        # Reap anything the session left behind in its process group.
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(process.pid, signal.SIGKILL)
    return process.returncode, timed_out, time.monotonic() - start


def remove_tree(path: Path, attempts: int = 5) -> bool:
    """rmtree that clears read-only bits (Git objects on Windows) and retries locked files."""

    def clear_and_retry(function: Any, target: Any, _error: Any) -> None:
        try:
            os.chmod(target, stat.S_IWRITE)
            function(target)
        except OSError:
            pass

    path = Path(path)
    for attempt in range(attempts):
        if not path.exists():
            return True
        try:
            if sys.version_info >= (3, 12):
                shutil.rmtree(path, onexc=clear_and_retry)
            else:
                shutil.rmtree(path, onerror=clear_and_retry)
        except OSError:
            pass
        if not path.exists():
            return True
        time.sleep(0.5 * (attempt + 1))
    return not path.exists()


def _stderr_tail(path: Path, limit: int = 300) -> str:
    try:
        lines = [line.strip() for line in path.read_text(encoding="utf-8", errors="replace").splitlines()]
    except OSError:
        return ""
    lines = [line for line in lines if line]
    return lines[-1][:limit] if lines else ""


def _write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=False) + "\n", encoding="utf-8")


def run_one(config: Config, spec: RunSpec) -> dict[str, Any]:
    run_dir = config.out_dir / "runs" / spec.run_id
    run_dir.mkdir(parents=True)
    arm = config.spec(spec.arm)
    guard = str(config.staged.mcp_executable) if config.staged else None
    settings = build_settings(
        arm, guard, config.guard_matcher, config.extra_settings, config.disabled_plugins
    )
    settings_path = run_dir / "settings.json"
    _write_json(settings_path, settings)
    temp_root = Path(
        tempfile.mkdtemp(prefix="ue-eval-", dir=str(config.work_dir) if config.work_dir else None)
    )
    repo = temp_root / "repo"
    base_env = os.environ if config.base_env is None else config.base_env
    exit_code: int | None = None
    timed_out = False
    wall_s = 0.0
    actual: dict[str, bytes] = {}
    setup_error: str | None = None
    try:
        prepare_repo(spec.task.fixture, repo, base_env)
        config_dir = None
        if config.isolate_config:
            config_dir = temp_root / "claude-config"
            config_dir.mkdir()
        env, removed = build_env(base_env, config_dir)
        use_arg = config.prompt_via == "arg"
        mcp_path = None
        if arm.mcp_server is not None:
            mcp_path = (run_dir / "mcp.json").resolve()
            _write_json(mcp_path, mcp_config(arm, repo.resolve(), Path(config.third_party_dir).resolve()))
        argv = build_command(
            config.claude.argv,
            arm=arm,
            settings_path=settings_path.resolve(),
            plugin_dir=config.staged.root if config.staged else None,
            mcp_config_path=mcp_path,
            permission_mode=config.permission_mode,
            max_turns=config.max_turns,
            model=config.model,
            max_budget_usd=config.max_budget_usd,
            effort=config.effort,
            setting_sources=config.setting_sources,
            allowed_tools=config.allowed_tools,
            prompt=spec.task.prompt if use_arg else None,
            claude=config.claude,
        )
        _write_json(
            run_dir / "command.json",
            {
                "argv": ["<prompt.md>" if use_arg and item == spec.task.prompt else item for item in argv],
                "cwd": str(repo),
                "prompt": f"{spec.task.name}/prompt.md via {'argument' if use_arg else 'stdin'}",
                "env_set": {**CHILD_ENV, **({"CLAUDE_CONFIG_DIR": str(config_dir)} if config_dir else {})},
                "env_removed": removed,
            },
        )
        exit_code, timed_out, wall_s = run_process(
            argv,
            cwd=repo,
            env=env,
            stdin_bytes=None if use_arg else spec.task.prompt.encode("utf-8"),
            stdout_path=run_dir / "stream.jsonl",
            stderr_path=run_dir / "stderr.txt",
            timeout_s=config.timeout_s,
        )
        actual = read_tree(repo, IGNORED_TOP_LEVEL)
    except (EvalError, OSError, subprocess.SubprocessError) as error:
        setup_error = f"{type(error).__name__}: {error}"
    finally:
        if config.keep_workdirs:
            (run_dir / "workdir.txt").write_text(str(temp_root) + "\n", encoding="utf-8")
        elif not remove_tree(temp_root):
            print(f"warning: could not remove {temp_root}", file=sys.stderr)
    transcript = parse_stream_file(run_dir / "stream.jsonl")
    expected = spec.task.expected_tree()
    comparison = compare_trees(expected, actual)
    if not comparison.correct and setup_error is None:
        (run_dir / "diff.txt").write_text(
            describe_differences(expected, actual, comparison), encoding="utf-8"
        )
    metrics = compute_metrics(transcript, arm.edit_tools)
    errors, warnings = check_integrity(
        arm,
        transcript,
        config.staged.root if config.staged and arm.plugin else None,
        hook_events_expected=config.hook_check and config.claude.supports("--include-hook-events"),
        guard_matcher=config.guard_matcher,
        third_party_servers=config.third_party_servers,
    )
    outcome = classify(transcript, comparison, errors, timed_out)
    if setup_error is not None:
        outcome = "infra_error"
        errors = [f"harness: {setup_error}", *errors]
    elif transcript.init is None:
        tail = _stderr_tail(run_dir / "stderr.txt")
        if tail:
            errors = [*errors, f"stderr: {tail}"]
    init = transcript.init or {}
    if outcome == "fail":
        reason = comparison.reason()
    elif outcome == "timeout":
        reason = f"timed out after {config.timeout_s:g}s"
    elif outcome in ("invalid", "infra_error"):
        reason = "; ".join([*errors, *transcript.api_errors])[:400]
    else:
        reason = ""
    record = {
        "run_id": spec.run_id,
        "task": spec.task.name,
        "arm": spec.arm,
        "rep": spec.rep,
        "outcome": outcome,
        "reason": reason,
        "correct": comparison.correct,
        "first_attempt": first_attempt(metrics, comparison.correct),
        "timed_out": timed_out,
        "exit_code": exit_code,
        "wall_s": round(wall_s, 3),
        "comparison": comparison.to_json(),
        "integrity": {"errors": errors, "warnings": warnings},
        "metrics": metrics,
        "init": {
            "claude_code_version": init.get("claude_code_version"),
            "model": init.get("model"),
            "permission_mode": init.get("permissionMode"),
            "plugins": init.get("plugins"),
            "mcp_servers": init.get("mcp_servers"),
            "tools": init.get("tools"),
        },
        "files": {"dir": f"runs/{spec.run_id}"},
    }
    _write_json(run_dir / "result.json", record)
    return record


# ---------------------------------------------------------------------------
# Summary


def _numbers(values: Iterable[Any]) -> list[float]:
    return [float(value) for value in values if _number(value) is not None]


def _mean(values: Iterable[Any]) -> float | None:
    numbers = _numbers(values)
    return sum(numbers) / len(numbers) if numbers else None


def _stdev(values: Iterable[Any]) -> float | None:
    """Sample standard deviation; None below two values."""
    numbers = _numbers(values)
    return statistics.stdev(numbers) if len(numbers) >= 2 else None


def _median(values: Iterable[Any]) -> float | None:
    numbers = _numbers(values)
    return statistics.median(numbers) if numbers else None


def _cv(values: Iterable[Any]) -> float | None:
    """Coefficient of variation (sd / mean); None below two values or at a zero mean."""
    numbers = _numbers(values)
    mean = _mean(numbers)
    deviation = _stdev(numbers)
    return deviation / mean if deviation is not None and mean else None


def _fmt(value: float | None, digits: int = 1) -> str:
    if value is None:
        return "-"
    if digits == 0:
        return f"{value:,.0f}"
    return f"{value:,.{digits}f}"


def _rate(count: int, total: int) -> str:
    return f"{count}/{total} ({100 * count / total:.0f}%)" if total else "-"


# Reported as mean, standard deviation, and median; the rest as means.
SPREAD_METRICS = ("cost_usd", "context_tokens_total", "output_tokens", "turns", "tool_calls")
MEAN_METRICS = (
    "tool_errors",
    "shell_errors",
    "edit_calls",
    "tool_input_bytes",
    "tool_result_bytes",
    "first_call_context_tokens",
    "total_input_tokens",
)


def scored_records(records: Iterable[Mapping[str, Any]]) -> list[Mapping[str, Any]]:
    return [record for record in records if record.get("outcome") in SCORED_OUTCOMES]


def _metric(records: Iterable[Mapping[str, Any]], key: str) -> list[Any]:
    return [(record.get("metrics") or {}).get(key) for record in records]


def aggregate(records: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    scored = scored_records(records)
    outcomes = Counter(record.get("outcome") for record in records)
    correct = sum(1 for record in scored if record.get("correct"))
    costs = _numbers(_metric(scored, "cost_usd"))
    stats: dict[str, Any] = {
        "runs": len(records),
        "scored": len(scored),
        "correct": correct,
        "first_attempt": sum(1 for record in scored if record.get("first_attempt")),
        "bash_write_attempts": sum(value or 0 for value in _metric(scored, "bash_write_attempts")),
        "bash_writes": sum(value or 0 for value in _metric(scored, "bash_writes")),
        "wall_s": _mean(record.get("wall_s") for record in scored),
        "cost_total_usd": sum(costs) if costs else None,
        "cost_per_correct": sum(costs) / correct if costs and correct else None,
        "invalid": outcomes["invalid"],
        "infra_error": outcomes["infra_error"],
        "timeout": outcomes["timeout"],
    }
    for key in SPREAD_METRICS:
        values = _metric(scored, key)
        stats[key] = _mean(values)
        stats[f"{key}_sd"] = _stdev(values)
        stats[f"{key}_median"] = _median(values)
    for key in MEAN_METRICS:
        stats[key] = _mean(_metric(scored, key))
    return stats


def variance_by_task(records: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    """Mean over tasks of the per-task coefficient of variation across repetitions."""
    by_task: dict[str, list[Mapping[str, Any]]] = {}
    for record in scored_records(records):
        by_task.setdefault(str(record.get("task")), []).append(record)
    repeated = [runs for runs in by_task.values() if len(runs) >= 2]
    return {
        "tasks": len(repeated),
        "cost_cv": _mean(_cv(_metric(runs, "cost_usd")) for runs in repeated),
        "context_cv": _mean(_cv(_metric(runs, "context_tokens_total")) for runs in repeated),
    }


_TABLE_HEADER = (
    "| {first} | Scored | Correct | First try | Tool calls | Edit calls | Tool errors "
    "| Bash writes | Turns | Input tok | Output tok | Cost USD | Wall s |"
)
_SPREAD_HEADER = (
    "| {first} | n | Correct | First try | Cost USD | USD per correct | Context tok "
    "| Output tok | Turns | Tool calls |"
)
_MEANS_HEADER = (
    "| {first} | n | Tool errors | Shell errors | Edit calls | Tool input bytes "
    "| Tool result bytes | First-call context |"
)


def _table_row(label: str, stats: Mapping[str, Any]) -> str:
    return (
        f"| {label} | {stats['scored']} | {_rate(stats['correct'], stats['scored'])} "
        f"| {_rate(stats['first_attempt'], stats['scored'])} | {_fmt(stats['tool_calls'])} "
        f"| {_fmt(stats['edit_calls'])} | {_fmt(stats['tool_errors'])} "
        f"| {stats['bash_writes']}/{stats['bash_write_attempts']} | {_fmt(stats['turns'])} "
        f"| {_fmt(stats['total_input_tokens'], 0)} | {_fmt(stats['output_tokens'], 0)} "
        f"| {_fmt(stats['cost_usd'], 4)} | {_fmt(stats['wall_s'])} |"
    )


def _spread(stats: Mapping[str, Any], key: str, digits: int) -> str:
    """mean ± sd (median); the sd is omitted below two runs."""
    if stats[key] is None:
        return "-"
    deviation = stats[f"{key}_sd"]
    spread = f" ± {_fmt(deviation, digits)}" if deviation is not None else ""
    return f"{_fmt(stats[key], digits)}{spread} ({_fmt(stats[f'{key}_median'], digits)})"


def _spread_row(label: str, stats: Mapping[str, Any]) -> str:
    return (
        f"| {label} | {stats['scored']} | {_rate(stats['correct'], stats['scored'])} "
        f"| {_rate(stats['first_attempt'], stats['scored'])} | {_spread(stats, 'cost_usd', 4)} "
        f"| {_fmt(stats['cost_per_correct'], 4)} | {_spread(stats, 'context_tokens_total', 0)} "
        f"| {_spread(stats, 'output_tokens', 0)} | {_spread(stats, 'turns', 1)} "
        f"| {_spread(stats, 'tool_calls', 1)} |"
    )


def _means_row(label: str, stats: Mapping[str, Any]) -> str:
    return (
        f"| {label} | {stats['scored']} | {_fmt(stats['tool_errors'], 2)} | {_fmt(stats['shell_errors'], 2)} "
        f"| {_fmt(stats['edit_calls'], 1)} | {_fmt(stats['tool_input_bytes'], 0)} "
        f"| {_fmt(stats['tool_result_bytes'], 0)} | {_fmt(stats['first_call_context_tokens'], 0)} |"
    )


def _ordered(values: Iterable[str], preferred: Sequence[str] = ()) -> list[str]:
    unique = list(dict.fromkeys(values))
    return [item for item in preferred if item in unique] + sorted(
        item for item in unique if item not in preferred
    )


def _record_key(record: Mapping[str, Any]) -> tuple[str, str, int, str]:
    """Canonical record order, so summaries never depend on completion order."""
    rep = record.get("rep")
    rep = rep if isinstance(rep, int) else 0
    return (str(record.get("task")), str(record.get("arm")), rep, str(record.get("run_id")))


def _separator(header: str) -> str:
    return "|" + " --- |" * (header.count("|") - 1)


def _tables(groups: Sequence[tuple[str, Sequence[Mapping[str, Any]]]], first: str, heading: str) -> list[str]:
    """The three per-group tables: rates and means, spread, and error and byte means."""
    stats = [(label, aggregate(subset)) for label, subset in groups]
    lines = ["", f"## {heading}", "", _TABLE_HEADER.format(first=first), _separator(_TABLE_HEADER)]
    lines += [_table_row(label, item) for label, item in stats]
    lines += ["", f"### Spread {heading[0].lower() + heading[1:]}", "", "Cells are mean ± sd (median).", ""]
    lines += [_SPREAD_HEADER.format(first=first), _separator(_SPREAD_HEADER)]
    lines += [_spread_row(label, item) for label, item in stats]
    lines += ["", f"### Errors and bytes {heading[0].lower() + heading[1:]}", ""]
    lines += [_MEANS_HEADER.format(first=first), _separator(_MEANS_HEADER)]
    lines += [_means_row(label, item) for label, item in stats]
    return lines


def summarize(records: Sequence[Mapping[str, Any]], title: str = "Ultra Edit evaluation") -> str:
    records = sorted(records, key=_record_key)
    arms = _ordered((record["arm"] for record in records), list(BUILTIN_ARMS))
    tasks = _ordered(record["task"] for record in records)
    total = aggregate(records)
    versions = sorted(
        {
            str(record["init"].get("claude_code_version"))
            for record in records
            if (record.get("init") or {}).get("claude_code_version")
        }
    )
    models = sorted({model for record in records for model in record["metrics"].get("models") or []})
    by_arm = {arm: [record for record in records if record["arm"] == arm] for arm in arms}
    lines = [
        f"# {title}",
        "",
        f"Runs: {total['runs']} total, {total['scored']} scored, {total['invalid']} invalid, "
        f"{total['infra_error']} infrastructure errors, {total['timeout']} timeouts.",
        f"Claude Code: {', '.join(versions) or 'unknown'}. Models: {', '.join(models) or 'unknown'}.",
        "",
        "Scored runs exclude invalid and infrastructure runs; timeouts count as failures. "
        "Correct means exact final bytes with no extra or missing files. First try means correct with no "
        "failed edit call, rejected Ultra Edit plan, or Ultra Edit recovery call. Bash writes are "
        "succeeded/attempted content-writing shell commands (heuristic), summed over runs. Other columns "
        "are means per scored run; Input tok includes cache reads and writes. Context tok is the input "
        "processed over all API calls (uncached + cache reads + cache writes, summed per call), which "
        "does not depend on cache state. USD per correct is total scored cost over correct runs.",
    ]
    lines += _tables([(arm, by_arm[arm]) for arm in arms], "Arm", "By arm")
    lines += [
        "",
        "## Variance",
        "",
        "Per arm, the mean over tasks of the coefficient of variation (sd / mean) across that task's "
        "repetitions; only tasks with at least two scored runs count. Lower is more predictable.",
        "",
        "| Arm | Tasks | Cost CV | Context tok CV |",
        "| --- | --- | --- | --- |",
    ]
    for arm in arms:
        variance = variance_by_task(by_arm[arm])
        lines.append(
            f"| {arm} | {variance['tasks']} | {_fmt(variance['cost_cv'], 2)} | {_fmt(variance['context_cv'], 2)} |"
        )
    lines += [
        "",
        "## Fixed context overhead",
        "",
        "Median context tokens of the first API call: system prompt, tool definitions, and the task "
        "prompt before any work. Every later request pays it again, mostly as cache reads.",
        "",
        "| Arm | Runs | First-call context tok |",
        "| --- | --- | --- |",
    ]
    for arm in arms:
        first_calls = _numbers(_metric(scored_records(by_arm[arm]), "first_call_context_tokens"))
        lines.append(f"| {arm} | {len(first_calls)} | {_fmt(_median(first_calls), 0)} |")
    groups = [
        (f"{task} / {arm}", subset)
        for task in tasks
        for arm in arms
        if (subset := [record for record in by_arm[arm] if record["task"] == task])
    ]
    lines += _tables(groups, "Task / arm", "By task and arm")
    tool_counts: dict[str, dict[str, int]] = {}
    for record in scored_records(records):
        for name, count in (record["metrics"].get("tool_calls_by_name") or {}).items():
            per_arm = tool_counts.setdefault(short_tool_name(name), {})
            per_arm[record["arm"]] = per_arm.get(record["arm"], 0) + count
    if tool_counts:
        scored_by_arm = {arm: len(scored_records(by_arm[arm])) for arm in arms}
        lines += [
            "",
            "## Tool calls per scored run",
            "",
            "| Tool | " + " | ".join(arms) + " |",
            "|" + " --- |" * (len(arms) + 1),
        ]
        for name in sorted(tool_counts, key=lambda item: (-sum(tool_counts[item].values()), item)):
            cells = [
                _fmt(tool_counts[name].get(arm, 0) / scored_by_arm[arm], 2) if scored_by_arm[arm] else "-"
                for arm in arms
            ]
            lines.append(f"| {name} | " + " | ".join(cells) + " |")
    failures = [record for record in records if record.get("outcome") in ("fail", "timeout")]
    if failures:
        lines += ["", "## Failed runs", ""]
        for record in failures:
            reason = Comparison(**record["comparison"]).reason() or "bytes correct"
            subtype = record["metrics"].get("result_subtype")
            extra = " timed out;" if record.get("timed_out") else ""
            lines.append(
                f"- `{record['run_id']}`:{extra} {reason} (result: {subtype}); see `{record['files']['dir']}`"
            )
    excluded = [record for record in records if record.get("outcome") in ("invalid", "infra_error")]
    if excluded:
        lines += ["", "## Invalid and infrastructure runs", ""]
        for record in excluded:
            details = (
                record["integrity"]["errors"]
                or record["metrics"].get("api_errors")
                or [f"result: {record['metrics'].get('result_subtype')}, exit code {record.get('exit_code')}"]
            )
            lines.append(f"- `{record['run_id']}` ({record['outcome']}): " + "; ".join(map(str, details)))
    warnings = Counter(warning for record in records for warning in record["integrity"]["warnings"])
    if warnings:
        lines += ["", "## Warnings", ""]
        lines += [f"- {warning} ({count} run(s))" for warning, count in sorted(warnings.items())]
    return "\n".join(lines) + "\n"


RECORD_COLUMNS = (
    "run_id",
    "task",
    "arm",
    "rep",
    "outcome",
    "correct",
    "first_attempt",
    "timed_out",
    "exit_code",
    "wall_s",
)
# Scalar metrics in a fixed order; other scalar metrics follow alphabetically.
METRIC_COLUMNS = (
    "cost_usd",
    "turns",
    "api_calls",
    "subagent_api_calls",
    "first_call_context_tokens",
    "peak_context_tokens",
    "context_tokens_total",
    "output_tokens",
    "output_tokens_calls",
    "input_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
    "total_input_tokens",
    "tool_calls",
    "tool_errors",
    "edit_calls",
    "edit_failures",
    "shell_calls",
    "shell_errors",
    "bash_write_attempts",
    "bash_writes",
    "bash_writes_blocked",
    "read_calls",
    "reread_calls",
    "toolsearch_calls",
    "skill_calls",
    "tool_input_bytes",
    "tool_result_bytes",
    "final_text_bytes",
    "ultra_rejections",
    "ultra_recovery_calls",
    "hook_denials",
    "permission_denials",
    "subagent_tool_calls",
    "unanswered_tool_calls",
    "duration_ms",
    "duration_api_ms",
    "api_retries",
    "result_subtype",
    "result_is_error",
    "stop_reason",
    "terminal_reason",
    "token_source",
    "stream_lines",
    "stream_parse_errors",
)


def _scalar(value: Any) -> bool:
    return value is None or isinstance(value, (str, int, float, bool))


def csv_rows(records: Sequence[Mapping[str, Any]]) -> tuple[list[str], list[dict[str, Any]]]:
    """Columns and rows for runs.csv: one row per run, scalar values only, stable order."""
    ordered = sorted(records, key=_record_key)
    present = {
        key
        for record in ordered
        for key, value in (record.get("metrics") or {}).items()
        if _scalar(value) and value is not None
    }
    metric_columns = [*METRIC_COLUMNS, *sorted(present - set(METRIC_COLUMNS) - set(RECORD_COLUMNS))]
    columns = [*RECORD_COLUMNS, *metric_columns, "reason"]
    rows = []
    for record in ordered:
        metrics = record.get("metrics") or {}
        row = {key: record.get(key) for key in RECORD_COLUMNS}
        row.update({key: metrics.get(key) if _scalar(metrics.get(key)) else None for key in metric_columns})
        row["reason"] = record.get("reason", "")
        rows.append(row)
    return columns, rows


def write_runs_csv(records: Sequence[Mapping[str, Any]], path: Path) -> None:
    columns, rows = csv_rows(records)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({key: "" if value is None else value for key, value in row.items()})


def load_records(results_dir: Path) -> list[dict[str, Any]]:
    path = Path(results_dir) / "runs.jsonl"
    if not path.exists():
        raise EvalError(f"no runs.jsonl in {results_dir}")
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


# ---------------------------------------------------------------------------
# Command line


def _timestamp() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d-%H%M%S")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--dry-run", action="store_true", help="print commands and settings; start nothing")
    mode.add_argument("--preflight", action="store_true", help="run local checks only (no model calls)")
    mode.add_argument(
        "--summarize", type=Path, metavar="RESULTS_DIR", help="rebuild summary.md from runs.jsonl"
    )
    mode.add_argument(
        "--rescore",
        type=Path,
        metavar="RESULTS_DIR",
        help="recompute metrics from saved transcripts (keeps runs.jsonl.bak), then rebuild summary.md",
    )
    parser.add_argument(
        "--task", action="append", dest="tasks", metavar="NAME", help="repeatable; default all"
    )
    parser.add_argument(
        "--arm",
        action="append",
        dest="arms",
        metavar="NAME",
        help="repeatable; default: " + ", ".join(DEFAULT_ARMS) + "; `all` adds every built-in and "
        "third-party arm. Built-in: " + ", ".join(BUILTIN_ARMS),
    )
    parser.add_argument(
        "--third-party-arms",
        type=Path,
        default=DEFAULT_THIRD_PARTY_ARMS,
        metavar="FILE",
        help=f"third-party MCP arm definitions (see {THIRD_PARTY_README})",
    )
    parser.add_argument(
        "--third-party-dir",
        type=Path,
        default=DEFAULT_THIRD_PARTY_INSTALL,
        metavar="DIR",
        help="install directory substituted for {third_party_dir} in arms.json",
    )
    parser.add_argument(
        "--disable-plugin",
        action="append",
        dest="disable_plugins",
        default=[],
        metavar="ID",
        help="repeatable; adds enabledPlugins {ID: false} to every run's settings",
    )
    parser.add_argument("--jobs", type=int, default=1, help="runs in parallel")
    parser.add_argument("--reps", type=int, default=1, help="repetitions per task and arm")
    parser.add_argument("--model", help="model alias or full name; recorded from init either way")
    parser.add_argument("--effort", choices=EFFORT_LEVELS, help="passed to claude --effort")
    parser.add_argument("--max-turns", type=int, default=50)
    parser.add_argument("--max-budget-usd", type=float, default=2.0, help="per-run cap; 0 disables")
    parser.add_argument(
        "--max-total-usd", type=float, help="stop starting runs once recorded spend reaches this"
    )
    parser.add_argument("--timeout", type=float, default=900.0, help="seconds per run")
    parser.add_argument("--permission-mode", choices=PERMISSION_MODES, default="bypassPermissions")
    parser.add_argument(
        "--allowed-tools",
        default=",".join(DEFAULT_ALLOWED_TOOLS),
        help="comma-separated --allowedTools, used when the permission mode is not bypassPermissions",
    )
    parser.add_argument("--setting-sources", default="", help="claude --setting-sources value; '' loads none")
    parser.add_argument(
        "--isolate-config",
        action="store_true",
        help="fresh CLAUDE_CONFIG_DIR per run; needs ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN",
    )
    parser.add_argument("--extra-settings", type=Path, help="JSON merged into every arm's settings")
    parser.add_argument(
        "--guard-matcher", default=DEFAULT_GUARD_MATCHER, help="PreToolUse matcher for the guard hook"
    )
    parser.add_argument(
        "--no-hook-check",
        action="store_true",
        help="do not invalidate native-guard runs that report no PreToolUse hook events",
    )
    parser.add_argument("--prompt-via", choices=("stdin", "arg"), default="stdin")
    parser.add_argument("--claude", help="claude executable (default: `claude` on PATH)")
    parser.add_argument("--runtime-dir", type=Path, default=DEFAULT_RUNTIME_DIR, help="built executables")
    parser.add_argument("--plugin-source", type=Path, default=DEFAULT_PLUGIN_SOURCE)
    parser.add_argument("--tasks-dir", type=Path, default=DEFAULT_TASKS_DIR)
    parser.add_argument("--out", type=Path, help="results directory (default eval/results/<UTC time>)")
    parser.add_argument(
        "--work-dir", type=Path, help="parent for temporary repositories (default system temp)"
    )
    parser.add_argument("--keep-workdirs", action="store_true", help="keep temporary repositories")
    parser.add_argument("--seed", type=int, default=0, help="arm-order shuffle seed")
    parser.add_argument("--no-shuffle", action="store_true", help="run arms in the listed order")
    parser.add_argument("--yes", action="store_true", help="start paid runs without the confirmation prompt")
    args = parser.parse_args(argv)
    if args.reps < 1:
        parser.error("--reps must be at least 1")
    if args.max_turns < 1:
        parser.error("--max-turns must be at least 1")
    if args.jobs < 1:
        parser.error("--jobs must be at least 1")
    return args


def _load_extra_settings(path: Path | None) -> dict[str, Any] | None:
    if path is None:
        return None
    try:
        value = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise EvalError(f"cannot read --extra-settings {path}: {error}") from error
    if not isinstance(value, dict):
        raise EvalError("--extra-settings must hold a JSON object")
    return value


def _print(text: str = "") -> None:
    print(text, flush=True)


def _needs_staging(specs: Sequence[ArmSpec]) -> bool:
    return any(spec.plugin or spec.guard_hook for spec in specs)


def _dry_run(
    args: argparse.Namespace,
    tasks: Sequence[Task],
    specs: Sequence[ArmSpec],
    plan: Sequence[RunSpec],
    out_dir: Path,
) -> int:
    windows = os.name == "nt"
    claude_argv = resolve_claude(args.claude) or ["claude"]
    staged_root = out_dir / "plugin"
    guard = staged_root / "runtime" / mcp_executable_name(windows)
    runtime_ok = (Path(args.runtime_dir) / mcp_executable_name(windows)).is_file()
    extra = _load_extra_settings(args.extra_settings)
    third_party_dir = Path(args.third_party_dir).resolve()
    counts = f"{len(tasks)} task(s) x {len(specs)} arm(s) x {args.reps} rep(s) = {len(plan)} run(s)"
    _print(f"Dry run: {counts}, {args.jobs} at a time. Nothing is started.")
    _print(f"claude: {format_command(claude_argv)} (flags not probed; --preflight checks them)")
    if _needs_staging(specs):
        _print(
            f"stage: {args.plugin_source} -> {staged_root}, runtime from {args.runtime_dir} "
            f"({mcp_executable_name(windows)} {'found' if runtime_ok else 'MISSING'})"
        )
    for spec in specs:
        for problem in third_party_problems(spec, third_party_dir):
            _print(f"WARNING: {problem}")
    _print(f"results: {out_dir}")
    _print("each run: fresh temporary Git repository with the fixture committed; cwd = <temp>/repo")
    env, removed = build_env(os.environ)
    _print(f"child environment sets: {', '.join(f'{k}={v}' for k, v in CHILD_ENV.items())}")
    if removed:
        _print(f"child environment removes: {', '.join(removed)}")
    for arm in specs:
        spec = next(item for item in plan if item.arm == arm.name)
        run_dir = out_dir / "runs" / spec.run_id
        settings = build_settings(arm, str(guard), args.guard_matcher, extra, args.disable_plugins)
        argv = build_command(
            claude_argv,
            arm=arm,
            settings_path=run_dir / "settings.json",
            plugin_dir=staged_root,
            mcp_config_path=run_dir / "mcp.json",
            permission_mode=args.permission_mode,
            max_turns=args.max_turns,
            model=args.model,
            max_budget_usd=args.max_budget_usd or None,
            effort=args.effort,
            setting_sources=args.setting_sources,
            allowed_tools=tuple(filter(None, args.allowed_tools.split(","))),
            prompt="<contents of prompt.md>" if args.prompt_via == "arg" else None,
        )
        _print("")
        _print(f"== {arm.name} (example: {spec.run_id}) ==")
        _print(arm.description)
        _print(f"settings file {run_dir / 'settings.json'}:")
        _print(json.dumps(settings, indent=2))
        if arm.mcp_server is not None:
            _print(f"MCP config file {run_dir / 'mcp.json'}:")
            _print(json.dumps(mcp_config(arm, "<temp>/repo", third_party_dir), indent=2))
        _print("command:")
        _print(format_command(argv))
        if args.prompt_via == "stdin":
            _print(f"stdin: {spec.task.path / 'prompt.md'}")
    _print("")
    _print("run order:")
    for index, spec in enumerate(plan, 1):
        _print(f"  {index:3d}. {spec.task.name} / {spec.arm} / r{spec.rep}")
    return 0


def _preflight(
    args: argparse.Namespace, specs: Sequence[ArmSpec], out_dir: Path, extra: dict[str, Any] | None
) -> tuple[ClaudeInfo | None, StagedPlugin | None, list[str], list[str]]:
    problems: list[str] = []
    notes: list[str] = []
    env, removed = build_env(os.environ)
    if removed:
        notes.append("removed from the child environment: " + ", ".join(removed))
    if os.environ.get("CLAUDECODE"):
        notes.append(
            "running inside a Claude Code session; nested-session markers are stripped for child runs"
        )
    if shutil.which("git") is None:
        problems.append("git is not on PATH")
    claude_argv = resolve_claude(args.claude)
    info = None
    if claude_argv is None:
        problems.append("claude not found; install Claude Code or pass --claude PATH")
    else:
        try:
            info = probe_claude(claude_argv, env)
        except (OSError, subprocess.SubprocessError) as error:
            problems.append(f"cannot run {claude_argv[0]}: {error}")
        else:
            notes.append(f"claude: {claude_argv[0]} ({info.version_text})")
            problems += claude_problems(info)
            unsupported = [flag for flag in OPTIONAL_FLAGS if not info.supports(flag)]
            if unsupported:
                notes.append("optional flags unavailable and skipped: " + ", ".join(unsupported))
            for spec in specs:
                missing = [flag for flag in spec.needs_flags() if not info.supports(flag)]
                if missing:
                    problems.append(f"arm {spec.name} needs claude flags: {', '.join(missing)}")
    if (
        args.permission_mode == "bypassPermissions"
        and os.name != "nt"
        and hasattr(os, "geteuid")
        and os.geteuid() == 0
    ):
        notes.append(
            "WARNING: Claude Code refuses bypassPermissions as root outside a recognized sandbox; "
            "run as a normal user or pass --permission-mode acceptEdits"
        )
    if args.isolate_config and not auth_hint(os.environ, extra):
        problems.append(
            "--isolate-config starts logged out: set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN "
            "(from `claude setup-token`), or provide apiKeyHelper via --extra-settings"
        )
    for spec in specs:
        if spec.third_party:
            found = third_party_problems(spec, Path(args.third_party_dir))
            problems += found
            if not found:
                notes.append(f"arm {spec.name}: MCP server command found")
    staged = None
    if _needs_staging(specs):
        try:
            staged = stage_plugin(args.plugin_source, args.runtime_dir, out_dir / "plugin")
        except (EvalError, OSError) as error:
            problems.append(f"staging failed: {error}")
        else:
            notes.append(f"staged plugin {staged.root} (runtime: {', '.join(staged.runtime_files)})")
            try:
                version = run_text([str(staged.mcp_executable), "--version"], timeout=60, env=env)
            except (OSError, subprocess.SubprocessError) as error:
                problems.append(f"staged {staged.mcp_executable.name} cannot run: {error}")
            else:
                notes.append(f"runtime: {(version.stdout or version.stderr).strip()}")
            if any(spec.guard_hook for spec in specs):
                scratch = Path(tempfile.mkdtemp(prefix="ue-eval-hook-"))
                try:
                    hook_problems = check_guard_hook(
                        [str(staged.mcp_executable), *GUARD_HOOK_ARGS], scratch, env, args.guard_matcher
                    )
                finally:
                    remove_tree(scratch)
                problems += hook_problems
                if not hook_problems:
                    notes.append("guard hook denies a heredoc write and allows a read")
            if info is not None and any(spec.plugin for spec in specs):
                # Local manifest validation; no model call.
                validate = run_text(
                    [*info.argv, "plugin", "validate", str(staged.root)], timeout=120, env=env
                )
                if validate.returncode != 0:
                    notes.append(
                        "WARNING: claude plugin validate: "
                        + (validate.stdout + validate.stderr).strip()[:500]
                    )
    return info, staged, problems, notes


def _report(notes: Sequence[str], problems: Sequence[str]) -> None:
    for note in notes:
        _print(note)
    for problem in problems:
        _print(f"PROBLEM: {problem}")


def _confirm(count: int, assume_yes: bool) -> bool:
    if assume_yes:
        return True
    if not sys.stdin.isatty():
        _print("Refusing to start paid runs without --yes in a non-interactive session.")
        return False
    answer = input(f"Start {count} Claude Code run(s)? They spend API credits. [y/N] ")
    return answer.strip().lower() in ("y", "yes")


def _progress_line(record: Mapping[str, Any], prefix: str = "          ") -> str:
    metrics = record["metrics"]
    fields = (
        f"tools={metrics['tool_calls']}",
        f"edits={metrics['edit_calls']}",
        f"errors={metrics['tool_errors']}",
        f"bash-writes={metrics['bash_writes']}/{metrics['bash_write_attempts']}",
        f"turns={metrics['turns']}",
        f"context={metrics.get('context_tokens_total') or 0}",
        f"cost=${metrics.get('cost_usd') or 0:.4f}",
        f"wall={record['wall_s']:.1f}s",
    )
    return f"{prefix}{record['outcome'].upper():11} " + " ".join(fields)


def _harness_commit() -> str | None:
    try:
        completed = run_text(["git", "rev-parse", "HEAD"], cwd=str(REPO_ROOT), timeout=30)
    except (OSError, subprocess.SubprocessError):
        return None
    return (completed.stdout.strip() or None) if completed.returncode == 0 else None


def rescore_records(results_dir: Path, registry: ArmRegistry) -> list[dict[str, Any]]:
    """Recompute each run's metrics from its saved stream.jsonl with the current code.

    Outcomes, byte comparisons, and integrity results are kept: the temporary
    repositories are gone, so only transcript-derived values can be recomputed.
    """
    results_dir = Path(results_dir)
    records = load_records(results_dir)
    manifest_path = results_dir / "manifest.json"
    recorded_specs: dict[str, Any] = {}
    if manifest_path.exists():
        recorded_specs = json.loads(manifest_path.read_text(encoding="utf-8")).get("arm_specs") or {}
    rescored = []
    for record in records:
        arm = record.get("arm")
        if isinstance(recorded_specs.get(arm), dict) and "edit_tools" in recorded_specs[arm]:
            edit_tools = tuple(recorded_specs[arm]["edit_tools"])
        elif arm in registry.arms:
            edit_tools = registry.arms[arm].edit_tools
        else:
            edit_tools = NATIVE_EDIT_TOOL_NAMES
        stream = results_dir / "runs" / str(record.get("run_id")) / "stream.jsonl"
        updated = dict(record)
        if stream.exists():
            metrics = compute_metrics(parse_stream_file(stream), edit_tools)
            updated["metrics"] = metrics
            updated["first_attempt"] = first_attempt(metrics, bool(record.get("correct")))
        rescored.append(updated)
    backup = results_dir / "runs.jsonl.bak"
    if not backup.exists():
        shutil.copyfile(results_dir / "runs.jsonl", backup)
    with open(results_dir / "runs.jsonl", "w", encoding="utf-8") as handle:
        for record in rescored:
            handle.write(json.dumps(record, ensure_ascii=False) + "\n")
    return rescored


def write_results(records: Sequence[Mapping[str, Any]], results_dir: Path) -> str:
    """summary.md and runs.csv from the records; returns the summary text."""
    summary = summarize(records) if records else "# Ultra Edit evaluation\n\nNo runs completed.\n"
    (Path(results_dir) / "summary.md").write_text(summary, encoding="utf-8")
    write_runs_csv(records, Path(results_dir) / "runs.csv")
    return summary


def main(argv: Sequence[str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="replace")
    args = parse_args(argv)
    try:
        if args.summarize:
            summary = write_results(load_records(args.summarize), args.summarize)
            _print(summary)
            return 0
        if args.rescore:
            records = rescore_records(args.rescore, load_arm_registry(args.third_party_arms))
            _print(write_results(records, args.rescore))
            return 0
        registry = load_arm_registry(args.third_party_arms)
        specs = registry.resolve(args.arms)
        arms = tuple(spec.name for spec in specs)
        tasks = discover_tasks(args.tasks_dir, args.tasks)
        plan = plan_runs(tasks, arms, args.reps, None if args.no_shuffle else args.seed)
        out_dir = Path(args.out) if args.out else DEFAULT_RESULTS_DIR / _timestamp()
        if args.dry_run:
            return _dry_run(args, tasks, specs, plan, out_dir)
        extra = _load_extra_settings(args.extra_settings)
        if args.preflight:
            scratch = Path(tempfile.mkdtemp(prefix="ue-eval-preflight-"))
            try:
                _, _, problems, notes = _preflight(args, specs, scratch, extra)
            finally:
                remove_tree(scratch)
            _report(notes, problems)
            _print("preflight passed" if not problems else "preflight failed")
            return 0 if not problems else 1
        if out_dir.exists() and any(out_dir.iterdir()):
            raise EvalError(f"results directory is not empty: {out_dir}")
        out_dir.mkdir(parents=True, exist_ok=True)
        info, staged, problems, notes = _preflight(args, specs, out_dir, extra)
        _report(notes, problems)
        if problems or info is None:
            return 1
        if not _confirm(len(plan), args.yes):
            return 1
        config = Config(
            claude=info,
            out_dir=out_dir,
            permission_mode=args.permission_mode,
            max_turns=args.max_turns,
            model=args.model,
            max_budget_usd=args.max_budget_usd or None,
            effort=args.effort,
            timeout_s=args.timeout,
            setting_sources=args.setting_sources,
            isolate_config=args.isolate_config,
            prompt_via=args.prompt_via,
            allowed_tools=tuple(filter(None, args.allowed_tools.split(","))),
            guard_matcher=args.guard_matcher,
            extra_settings=extra,
            work_dir=args.work_dir,
            keep_workdirs=args.keep_workdirs,
            staged=staged,
            hook_check=not args.no_hook_check,
            arm_specs={spec.name: spec for spec in specs},
            third_party_servers=registry.third_party_servers,
            third_party_dir=Path(args.third_party_dir),
            disabled_plugins=tuple(args.disable_plugins),
        )
        return execute(config, plan, args, arms, tasks)
    except EvalError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


class RunLog:
    """Appends finished runs to runs.jsonl under a lock and tracks recorded spend."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.lock = threading.Lock()
        self.records: list[dict[str, Any]] = []
        self.spent = 0.0

    def add(self, record: dict[str, Any]) -> None:
        with self.lock:
            with open(self.path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")
            self.records.append(record)
            self.spent += record["metrics"].get("cost_usd") or 0.0


def run_plan(
    config: Config,
    plan: Sequence[RunSpec],
    log: RunLog,
    jobs: int = 1,
    max_total_usd: float | None = None,
) -> int:
    """Run the plan with up to `jobs` sessions at once. Returns how many runs were skipped
    because recorded spend reached max_total_usd; in-flight runs always finish."""
    width = max(len(f"{spec.task.name} / {spec.arm} / r{spec.rep}") for spec in plan)
    queue = list(enumerate(plan, 1))
    in_flight: dict[Future, tuple[int, str]] = {}
    skipped = 0
    _STOPPING.clear()

    def label(index: int, spec: RunSpec) -> str:
        text = f"{spec.task.name} / {spec.arm} / r{spec.rep}"
        return f"[{index:3d}/{len(plan)}] {text.ljust(width)}"

    def finish(future: Future, text: str) -> None:
        try:
            record = future.result()
        except Exception as error:  # a harness bug in one run must not kill the in-flight ones
            _print(f"{text} HARNESS ERROR (not recorded): {type(error).__name__}: {error}")
            traceback.print_exception(type(error), error, error.__traceback__, file=sys.stderr)
            return
        log.add(record)
        if jobs == 1:
            _print(_progress_line(record))
        else:
            _print(_progress_line(record, prefix=f"{text} "))
        if record["reason"]:
            _print(f"          {record['reason'][:200]}")

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        try:
            while queue or in_flight:
                while queue and len(in_flight) < jobs:
                    if max_total_usd is not None and log.spent >= max_total_usd:
                        skipped = len(queue)
                        queue.clear()
                        _print(
                            f"stopping: recorded spend ${log.spent:.4f} reached --max-total-usd "
                            f"{max_total_usd:g}; {skipped} planned run(s) skipped"
                        )
                        break
                    index, spec = queue.pop(0)
                    text = label(index, spec)
                    if jobs == 1:
                        _print(f"{text} ...")
                    in_flight[pool.submit(run_one, config, spec)] = (index, text)
                if not in_flight:
                    break
                # A timeout keeps Ctrl+C responsive where untimed waits block it (Windows).
                done, _ = wait(list(in_flight), timeout=1.0, return_when=FIRST_COMPLETED)
                for future in sorted(done, key=lambda item: in_flight[item][0]):
                    _, text = in_flight.pop(future)
                    finish(future, text)
        except BaseException:
            for future in in_flight:
                future.cancel()
            stop_active_processes()
            raise
    return skipped


def execute(
    config: Config,
    plan: Sequence[RunSpec],
    args: argparse.Namespace,
    arms: Sequence[str],
    tasks: Sequence[Task],
) -> int:
    manifest = {
        "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "harness_commit": _harness_commit(),
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "claude": {"argv": config.claude.argv, "version": config.claude.version_text},
        "plugin": {
            "root": str(config.staged.root) if config.staged else None,
            "version": config.staged.plugin_version if config.staged else None,
            "runtime_files": config.staged.runtime_files if config.staged else [],
        },
        "tasks": [task.name for task in tasks],
        "arms": list(arms),
        "arm_specs": {name: config.spec(name).to_json() for name in arms},
        "reps": args.reps,
        "options": {
            key: (str(value) if isinstance(value, Path) else value)
            for key, value in vars(args).items()
            if key not in ("dry_run", "preflight", "summarize", "yes")
        },
    }
    _write_json(config.out_dir / "manifest.json", manifest)
    log = RunLog(config.out_dir / "runs.jsonl")
    try:
        run_plan(config, plan, log, args.jobs, args.max_total_usd)
    except KeyboardInterrupt:
        _print("interrupted; summarizing completed runs")
    summary = write_results(log.records, config.out_dir)
    _print("")
    _print(summary)
    _print(f"results: {config.out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
