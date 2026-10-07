#!/usr/bin/env python3
"""Per-run behaviour facts computed from saved transcripts, so analysis never recounts by reading.

    python3 eval/facts.py RESULTS_DIR [RESULTS_DIR ...]

Writes RESULTS_DIR/facts.jsonl (one record per run) and RESULTS_DIR/facts.md (per-arm
tables). Each run becomes a list of steps, one per tool call, labelled with the API call
that issued it, a kind (read, search, inspect, edit, verify, tool_search, other), shell
features (git diff, byte views such as cat -A, tests), and an error class. From the steps
come the run's edit channels, the checks made before the first and after the last edit,
the failures a tool reported as success (masked), and each recovery: the steps and
context tokens between a failed edit and the next successful one.

Everything here is mechanical. Questions that need judgement, such as whether the final
message claims checks the steps do not show, are for a later pass that reads these facts
next to the run's digest.
"""

from __future__ import annotations

import importlib.util
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Mapping, Sequence

_HERE = Path(__file__).resolve().parent


def _load_harness():
    name = "ultra_edit_eval_harness"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, _HERE / "run_eval.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()

READ_TOOLS = frozenset({"Read", "NotebookRead"})
SEARCH_TOOLS = frozenset({"Grep", "Glob", "LS"})
# Third-party MCP tools that only read (the arm's edit tools are passed in separately).
MCP_READ = re.compile(r"(?:^|_)(?:read|get|list|search|stat|info|tree)(?:_|$)")

# Shell features, matched on the command with quoted strings and heredoc bodies kept,
# since `git diff` or `cat -A` inside a python -c string is still that check.
FEATURES = {
    "git_diff": re.compile(r"\bgit\s+(?:-[Cc]\s+\S+\s+|--no-pager\s+)*diff\b(?![^\n|;&]*--(?:stat|numstat|shortstat|name-only|name-status)\b)"),
    "git_diff_stat": re.compile(r"\bgit\s+(?:-[Cc]\s+\S+\s+|--no-pager\s+)*diff\b[^\n|;&]*--(?:stat|numstat|shortstat|name-only|name-status)\b"),
    "git_status": re.compile(r"\bgit\s+(?:-[Cc]\s+\S+\s+)*status\b"),
    "byte_view": re.compile(r"\bcat\s+-[A-Za-z]*[Aave]|\b(?:od|xxd|hexdump)\b|\bsed\s+-n\s+['\"]?l\b|\brepr\("),
    "file_view": re.compile(r"(?:^|[|;&\s(])(?:cat|head|tail|nl|less|more)\s|\bsed\s+-n\b|\bawk\s+['\"]?NR"),
    "search": re.compile(r"(?:^|[|;&\s(])(?:grep|egrep|fgrep|rg|ag|find)\s"),
    "test_run": re.compile(
        r"\b(?:pytest|unittest|go\s+(?:test|build|vet)|cargo\s+(?:test|build|check)|npm\s+(?:test|run)"
        r"|node\s+--test|make\b|tsc\b|py_compile|compileall|ruff|mypy|bash\s+-n|sh\s+-n|shellcheck|yamllint"
        r"|json\.tool|jq\s+\.)"
    ),
    "assert": re.compile(r"\bassert\b"),
}
CHECK_FEATURES = ("git_diff", "git_diff_stat", "git_status", "byte_view", "file_view", "test_run", "search")

# Who wrote the bytes in a shell edit.
PROGRAMS = (
    ("python", re.compile(r"(?:^|[\s;&|(])python[0-9.]*\b")),
    ("perl", re.compile(r"\bperl\b")),
    ("sed", re.compile(r"\bg?sed\b")),
    ("awk", re.compile(r"\b(?:g?awk|mawk|nawk)\b")),
    ("git_apply", re.compile(r"\bgit\s+(?:-[Cc]\s+\S+\s+)*apply\b|\bpatch\b")),
    ("node", re.compile(r"\b(?:node|deno|bun)\b")),
    ("heredoc", re.compile(r"<<")),
    ("echo", re.compile(r"\b(?:echo|printf)\b")),
)

ERROR_CLASSES = (
    ("guard_blocked", re.compile(r"Ultra Edit guard blocked")),
    ("tool_unavailable", re.compile(r"No such tool available")),
    ("ambiguous", re.compile(r"Found \d+ matches of the string to replace|Expected \d+ occurrences?|found \d+ occurrences", re.I)),
    ("not_found", re.compile(r"String to replace not found|Search content not found|No exact match|Could not find", re.I)),
    ("no_op", re.compile(r"No changes to make|old_string and new_string are exactly the same")),
    ("not_read", re.compile(r"File has not been read yet")),
    ("stale", re.compile(r"modified since|has been modified|hash mismatch|Hash mismatch", re.I)),
    ("command_not_found", re.compile(r"^Exit code 127\b|command not found")),
    ("patch_failed", re.compile(r"patch does not apply|patch failed|corrupt patch|while searching for", re.I)),
    ("validation", re.compile(r"InputValidationError|invalid_type|Invalid arguments", re.I)),
)
# Output that shows a failure although the tool reported success (bash returns the exit
# status of its last command, so `python3 edit.py; git diff` hides the script's failure).
MASKED = re.compile(
    r"Traceback \(most recent call last\)|^\w*Error: |error: patch failed|patch does not apply"
    r"|^sed: -e expression|^sed: can't read|No such file or directory",
    re.M,
)
_EXCEPTION = re.compile(r"^(\w+(?:Error|Exception))\b", re.M)


def _command(call) -> str:
    if isinstance(call.input, dict):
        value = call.input.get("command")
        if isinstance(value, str):
            return value
    return ""


def shell_features(command: str) -> list[str]:
    return sorted(name for name, pattern in FEATURES.items() if pattern.search(command))


def write_program(command: str, kinds: Sequence[str] = ()) -> str:
    """The program that wrote: a patch applier wins over the sed or python that built the patch."""
    if "patch_apply" in kinds or ("heredoc_write" in kinds and PROGRAMS[4][1].search(command)):
        return "git_apply"
    for name, pattern in PROGRAMS:
        if pattern.search(command):
            return name
    return "other"


# Programs whose absence means the edit itself could not run.
WRITERS = frozenset({"python", "python3", "perl", "sed", "gsed", "awk", "gawk", "git", "patch", "node", "tee"})
_MISSING = re.compile(r"(?:^|[\s:])([\w.+-]+): command not found", re.M)


def edit_failed(step: Mapping[str, Any], text: str = "") -> bool:
    """A failed edit step, except a shell edit whose only failure is a missing viewer
    (`... && xxd file` without xxd): the write ran, the check after it did not."""
    if not step["error"]:
        return False
    if step["error"] == "command_not_found" and step["channel"] and step["channel"].startswith("bash:"):
        missing = set(_MISSING.findall(text))
        return not missing or bool(missing & WRITERS)
    return True


def _text_editor_error(text: str) -> str | None:
    """mcp-text-editor reports failures inside a successful result: {"result": "error", ...}."""
    data = harness._json_object(text)
    nested = data.get("result") if isinstance(data, dict) else None
    if isinstance(nested, list):
        for block in nested:
            if isinstance(block, dict) and isinstance(block.get("text"), str):
                inner = harness._json_object(block["text"])
                if isinstance(inner, dict):
                    data = inner
                    break
    if not isinstance(data, dict):
        return None
    candidates = [data] + [value for value in data.values() if isinstance(value, dict)]
    for candidate in candidates:
        if candidate.get("result") == "error":
            reason = str(candidate.get("reason") or "error")
            return "text_editor:" + re.sub(r"[^a-z]+", "_", reason.lower()).strip("_")[:40]
    return None


def _ultra_error(text: str, structured: Any) -> str | None:
    data = harness._json_object(text)
    if data is None and isinstance(structured, dict):
        data = structured
    if not isinstance(data, dict):
        return None
    for diagnostic in data.get("diagnostics") or []:
        if isinstance(diagnostic, dict) and diagnostic.get("code"):
            return f"ultra:{diagnostic['code']}"
    error = data.get("error")
    if isinstance(error, dict) and error.get("code"):
        return f"ultra:{error['code']}"
    return None


def error_class(call, ultra_status: str | None) -> tuple[str | None, bool]:
    """(class, masked) for one call. class is None when the call did not fail."""
    result = call.result
    if result is None:
        return ("unanswered", False)
    text = result.text or ""
    if ultra_status not in (None, "ok", "missing"):
        return (_ultra_error(text, result.structured) or f"ultra:{ultra_status}", False)
    editor = _text_editor_error(text) if call.name.startswith("mcp__") else None
    if editor:
        return (editor, True)
    if result.is_error:
        for name, pattern in ERROR_CLASSES:
            if pattern.search(text):
                return (name, False)
        exception = _EXCEPTION.search(text)
        if "Traceback" in text and exception:
            return (f"python:{exception.group(1)}", False)
        if text.startswith("Exit code"):
            return ("shell_exit", False)
        return ("other", False)
    if call.name in harness.SHELL_TOOLS and MASKED.search(text):
        exception = _EXCEPTION.search(text)
        if "Traceback" in text and exception:
            return (f"python:{exception.group(1)}", True)
        for name, pattern in ERROR_CLASSES:
            if pattern.search(text):
                return (name, True)
        return ("shell_output_error", True)
    return (None, False)


def _api_index(stream: Path) -> dict[str, int]:
    """tool_use id -> index of the main-agent API call that issued it."""
    order: dict[str, int] = {}
    owner: dict[str, int] = {}
    if not stream.exists():
        return owner
    for raw in stream.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict) or event.get("type") != "assistant" or event.get("parent_tool_use_id"):
            continue
        body = event.get("message") if isinstance(event.get("message"), dict) else {}
        if body.get("model") == "<synthetic>":
            continue
        message_id = body.get("id")
        if not isinstance(message_id, str):
            continue
        index = order.setdefault(message_id, len(order))
        for block in body.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("id"):
                owner.setdefault(str(block["id"]), index)
    return owner


def steps_for(transcript, stream: Path, edit_tools: Sequence[str]) -> list[dict[str, Any]]:
    owner = _api_index(stream)
    cwd = (transcript.init or {}).get("cwd")
    read_paths: set[str] = set()
    edited = False
    steps = []
    for index, call in enumerate(transcript.tool_calls):
        if call.parent_tool_use_id:
            continue
        ultra = harness.ultra_tool(call.name)
        status = harness.ultra_result_status(ultra, call.result) if ultra else None
        command = _command(call) if call.name in harness.SHELL_TOOLS else ""
        features = shell_features(command) if command else []
        channel = None
        if ultra:
            if ultra in harness.ULTRA_WRITE_TOOLS:
                channel = f"ultra:{ultra}"
        elif call.name in harness.NATIVE_WRITE_TOOLS or call.name in edit_tools:
            channel = harness.short_tool_name(call.name)
        elif command:
            kinds = harness.shell_write_kinds(command, call.name)
            if call.name == "Bash":
                kinds = kinds + harness.shell_edit_kinds(command)
            if kinds:
                channel = f"bash:{write_program(command, kinds)}"
        target = harness._read_target(call, cwd if isinstance(cwd, str) else None)
        if channel:
            kind = "edit"
        elif call.name == "ToolSearch":
            kind = "tool_search"
        elif call.name in READ_TOOLS or ultra == "ultra_edit_snapshot" or (
            call.name.startswith("mcp__") and not ultra and MCP_READ.search(harness.short_tool_name(call.name)[4:])
        ):
            kind = "verify" if edited else "read"
        elif call.name in SEARCH_TOOLS:
            kind = "search"
        elif command:
            checks = [feature for feature in features if feature in CHECK_FEATURES]
            if edited and checks:
                kind = "verify"
            elif features == ["search"] or (checks == ["search"] and not edited):
                kind = "search"
            elif checks:
                kind = "inspect"
            else:
                kind = "other"
        else:
            kind = "other"
        reread = target is not None and target in read_paths
        if target is not None:
            read_paths.add(target)
        if kind == "verify" and call.name in READ_TOOLS:
            features = features + ["reread"]
        failure, masked = error_class(call, status)
        result = call.result
        step = {
            "i": len(steps),
            "api": owner.get(call.id),
            "tool": harness.short_tool_name(call.name),
            "kind": kind,
            "channel": channel,
            "features": features,
            "reread": reread,
            "in_bytes": harness._input_bytes(call.input),
            "out_bytes": harness._utf8_len(result.text) if result is not None else 0,
            "error": failure,
            "masked": masked,
        }
        step["failed"] = kind == "edit" and edit_failed(step, result.text if result is not None else "")
        steps.append(step)
        if kind == "edit" and not step["failed"]:
            edited = True
    return steps


def run_facts(record: Mapping[str, Any], results_dir: Path, edit_tools: Sequence[str]) -> dict[str, Any]:
    run_id = str(record.get("run_id"))
    stream = results_dir / "runs" / run_id / "stream.jsonl"
    transcript = harness.parse_stream_file(stream)
    steps = steps_for(transcript, stream, edit_tools)
    metrics = record.get("metrics") or {}
    series = metrics.get("context_series") or []
    edits = [step for step in steps if step["kind"] == "edit"]
    good = [step for step in edits if not step["failed"]]
    first_edit = edits[0]["i"] if edits else len(steps)
    last_good = good[-1]["i"] if good else None
    before = steps[:first_edit]
    after = steps[last_good + 1 :] if last_good is not None else []
    after_features = sorted({feature for step in after for feature in step["features"]})
    recoveries = []
    for step in edits:
        if not step["failed"]:
            continue
        fixed = next((later for later in good if later["i"] > step["i"]), None)
        tokens = None
        if fixed is not None and step["api"] is not None and fixed["api"] is not None:
            tokens = sum(int(pair[0]) for pair in series[step["api"] + 1 : fixed["api"] + 1])
        recoveries.append(
            {
                "step": step["i"],
                "error": step["error"],
                "masked": step["masked"],
                "fixed_at": fixed["i"] if fixed else None,
                "steps_between": (fixed["i"] - step["i"] - 1) if fixed else None,
                "context_tokens": tokens,
            }
        )
    return {
        "run_id": run_id,
        "task": record.get("task"),
        "arm": record.get("arm"),
        "rep": record.get("rep"),
        "outcome": record.get("outcome"),
        "correct": record.get("correct"),
        "cost_usd": metrics.get("cost_usd"),
        "context_tokens_total": metrics.get("context_tokens_total"),
        "api_calls": len(series),
        "channels": dict(sorted(Counter(step["channel"] for step in edits).items())),
        "edit_steps": len(edits),
        "edit_failures": sum(1 for step in edits if step["failed"]),
        "masked_failures": sum(1 for step in steps if step["masked"]),
        "errors": dict(sorted(Counter(step["error"] for step in steps if step["error"]).items())),
        "kinds": dict(sorted(Counter(step["kind"] for step in steps).items())),
        "rereads": sum(1 for step in steps if step["reread"]),
        "pre_edit": {
            "steps": len(before),
            "byte_view": any("byte_view" in step["features"] for step in before),
            "kinds": dict(sorted(Counter(step["kind"] for step in before).items())),
        },
        "post_edit": {
            "steps": len(after),
            "features": after_features,
            "diff_content": "git_diff" in after_features,
            "stat_only": "git_diff_stat" in after_features
            and not ({"git_diff", "byte_view", "file_view", "reread"} & set(after_features)),
            "checked": bool(set(after_features) & {"git_diff", "git_diff_stat", "byte_view", "file_view", "reread", "test_run"}),
        },
        "in_command_checks": sum(
            1 for step in edits if set(step["features"]) & {"git_diff", "git_diff_stat", "byte_view", "file_view", "assert"}
        ),
        "recoveries": recoveries,
        "recovery_context_tokens": sum(item["context_tokens"] or 0 for item in recoveries),
        "steps": steps,
    }


def _edit_tools_by_arm(results_dir: Path) -> dict[str, tuple[str, ...]]:
    manifest = results_dir / "manifest.json"
    specs = {}
    if manifest.exists():
        specs = json.loads(manifest.read_text(encoding="utf-8")).get("arm_specs") or {}
    return {arm: tuple(spec.get("edit_tools") or ()) for arm, spec in specs.items() if isinstance(spec, dict)}


def collect(results_dir: Path) -> list[dict[str, Any]]:
    results_dir = Path(results_dir)
    tools = _edit_tools_by_arm(results_dir)
    facts = []
    for raw in (results_dir / "runs.jsonl").read_text(encoding="utf-8").splitlines():
        if raw.strip():
            record = json.loads(raw)
            arm = str(record.get("arm"))
            facts.append(run_facts(record, results_dir, tools.get(arm, harness.NATIVE_EDIT_TOOL_NAMES)))
    return facts


def _pct(count: int, total: int) -> str:
    return f"{count}/{total} ({count / total:.0%})" if total else "-"


def report(facts: Sequence[Mapping[str, Any]]) -> str:
    arms = sorted({str(fact["arm"]) for fact in facts})
    out = ["# Run facts", "", f"{len(facts)} runs. Counts come from the transcripts; nothing here is judged by a model.", ""]
    out += ["## Editing and checking", ""]
    out.append(
        "| Arm | Runs | Edit steps (mean) | Runs with a failed edit | Masked failures | Byte view before editing"
        " | Checked after last edit | Diff content after | Stat only after | Edits carrying a check | Rereads (mean) |"
    )
    out.append("| --- " * 11 + "|")
    for arm in arms:
        rows = [fact for fact in facts if fact["arm"] == arm]
        n = len(rows)
        out.append(
            f"| {arm} | {n} | {sum(f['edit_steps'] for f in rows) / n:.1f}"
            f" | {_pct(sum(1 for f in rows if f['edit_failures']), n)}"
            f" | {sum(f['masked_failures'] for f in rows)}"
            f" | {_pct(sum(1 for f in rows if f['pre_edit']['byte_view']), n)}"
            f" | {_pct(sum(1 for f in rows if f['post_edit']['checked']), n)}"
            f" | {_pct(sum(1 for f in rows if f['post_edit']['diff_content']), n)}"
            f" | {_pct(sum(1 for f in rows if f['post_edit']['stat_only']), n)}"
            f" | {sum(f['in_command_checks'] for f in rows)}/{sum(f['edit_steps'] for f in rows)}"
            f" | {sum(f['rereads'] for f in rows) / n:.1f} |"
        )
    out += ["", "## Edit channels (edit steps by channel)", ""]
    for arm in arms:
        channels: Counter[str] = Counter()
        for fact in facts:
            if fact["arm"] == arm:
                channels.update(fact["channels"])
        out.append(f"- {arm}: " + (", ".join(f"{k} {v}" for k, v in channels.most_common()) or "none"))
    out += ["", "## Errors by class (all steps; masked ones were reported as success)", ""]
    for arm in arms:
        errors: Counter[str] = Counter()
        masked: Counter[str] = Counter()
        for fact in facts:
            if fact["arm"] != arm:
                continue
            errors.update(fact["errors"])
            for step in fact["steps"]:
                if step["masked"]:
                    masked[step["error"]] += 1
        parts = [f"{k} {v}" + (f" ({masked[k]} masked)" if masked[k] else "") for k, v in errors.most_common()]
        out.append(f"- {arm}: " + (", ".join(parts) or "none"))
    out += ["", "## Recoveries (failed edit to next successful edit)", ""]
    out.append("| Arm | Failed edits | Recovered | Steps between (mean) | Context tokens spent (total) |")
    out.append("| --- | --- | --- | --- | --- |")
    for arm in arms:
        items = [item for fact in facts if fact["arm"] == arm for item in fact["recoveries"]]
        fixed = [item for item in items if item["fixed_at"] is not None]
        between = sum(item["steps_between"] for item in fixed) / len(fixed) if fixed else 0
        tokens = sum(item["context_tokens"] or 0 for item in fixed)
        out.append(f"| {arm} | {len(items)} | {len(fixed)} | {between:.1f} | {tokens:,} |")
    out += ["", "## Step kinds (mean per run)", ""]
    kinds = ("read", "search", "inspect", "edit", "verify", "tool_search", "other")
    out.append("| Arm | " + " | ".join(kinds) + " |")
    out.append("| --- |" + " --- |" * len(kinds))
    for arm in arms:
        rows = [fact for fact in facts if fact["arm"] == arm]
        out.append(
            f"| {arm} | " + " | ".join(f"{sum(f['kinds'].get(k, 0) for f in rows) / len(rows):.1f}" for k in kinds) + " |"
        )
    return "\n".join(out) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    dirs = list(argv if argv is not None else sys.argv[1:])
    if not dirs:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    for directory in dirs:
        results_dir = Path(directory)
        facts = collect(results_dir)
        with open(results_dir / "facts.jsonl", "w", encoding="utf-8") as handle:
            for fact in facts:
                handle.write(json.dumps(fact, ensure_ascii=False) + "\n")
        (results_dir / "facts.md").write_text(report(facts), encoding="utf-8")
        print(f"{results_dir}: {len(facts)} runs -> facts.jsonl, facts.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
