"""PostToolUse and Stop hook that changes a file while the model works on it.

A task with a `concurrent.json` simulates another writer (a teammate, a formatter, a
second agent) editing one file during the session. Each event in the spec is
applied once, in order, right after the tool call that triggers it, so the model's
next call works from a view that is out of date:

- `"after": "seen"` fires after the first call whose input or result contains one
  of the event's `seen` strings (the model was shown that part of the file), or
  that changed the file;
- `"after": "write"` fires after the first call that changed the file since the
  previous event.

Events that have not fired when the session stops are applied by the Stop hook, so
every run ends with every event applied and one expected tree scores all arms. Each
change is an exact substitution whose `old` must occur exactly once. Usage, from
the run's settings:

    python3 concurrent_hook.py --spec concurrent.json --repo DIR --state FILE --log FILE
        < hook event JSON

It never blocks a tool and prints nothing. What it did is appended to the log as
JSON lines, which the harness records with the run.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

LOCK_WAIT_S = 10.0


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_spec(path: Path) -> dict[str, Any]:
    spec = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(spec, dict) or not isinstance(spec.get("file"), str):
        raise ValueError("concurrent spec must name a file")
    events = spec.get("events")
    if not isinstance(events, list) or not events:
        raise ValueError("concurrent spec must list events")
    for index, event in enumerate(events):
        where = f"event {index + 1}"
        if not isinstance(event, dict) or event.get("after") not in ("seen", "write"):
            raise ValueError(f"{where}: after must be 'seen' or 'write'")
        if event["after"] == "seen":
            seen = event.get("seen")
            if not isinstance(seen, list) or not seen or not all(isinstance(s, str) and s for s in seen):
                raise ValueError(f"{where}: seen must list non-empty strings")
        changes = event.get("changes")
        if not isinstance(changes, list) or not changes:
            raise ValueError(f"{where}: changes must be a non-empty list")
        for change in changes:
            if (
                not isinstance(change, dict)
                or not isinstance(change.get("old"), str)
                or not isinstance(change.get("new"), str)
                or not change["old"]
                or change["old"] == change["new"]
            ):
                raise ValueError(f"{where}: each change needs a non-empty old and a different new")
    return spec


def apply_changes(data: bytes, changes: list[dict[str, str]]) -> bytes:
    """Apply exact substitutions in order; each old must occur exactly once."""
    for change in changes:
        old, new = change["old"].encode("utf-8"), change["new"].encode("utf-8")
        count = data.count(old)
        if count != 1:
            raise LookupError(f"{change['old']!r} occurs {count} times")
        data = data.replace(old, new)
    return data


def apply_all(data: bytes, spec: dict[str, Any]) -> bytes:
    for event in spec["events"]:
        data = apply_changes(data, event["changes"])
    return data


def mentions(payload: Any, raw: str, needles: list[str]) -> bool:
    """Whether the hook input shows any needle, raw or as JSON escapes it."""
    if payload is None:
        return False
    text = raw or json.dumps(payload, ensure_ascii=False)
    for needle in needles:
        if needle in text or json.dumps(needle, ensure_ascii=False)[1:-1] in text:
            return True
    return False


class Lock:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.held = False

    def __enter__(self) -> "Lock":
        deadline = time.monotonic() + LOCK_WAIT_S
        while True:
            try:
                os.close(os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY))
                self.held = True
                return self
            except FileExistsError:
                if time.monotonic() > deadline:
                    raise TimeoutError(f"could not lock {self.path}") from None
                time.sleep(0.02)

    def __exit__(self, *_: object) -> None:
        if self.held:
            self.path.unlink(missing_ok=True)


def handle(args: argparse.Namespace, raw: str) -> list[dict[str, Any]]:
    """Apply the events this hook call triggers; return the log entries."""
    try:
        payload = json.loads(raw) if raw.strip() else None
    except ValueError:
        payload = None
    hook_event = payload.get("hook_event_name") if isinstance(payload, dict) else None
    at_stop = args.at_stop or hook_event in ("Stop", "SubagentStop")
    tool = payload.get("tool_name") if isinstance(payload, dict) else None
    tool_use_id = payload.get("tool_use_id") if isinstance(payload, dict) else None
    spec = load_spec(args.spec)
    target = Path(args.repo).joinpath(*spec["file"].split("/"))
    entries: list[dict[str, Any]] = []
    with Lock(args.state.with_name(args.state.name + ".lock")):
        state = json.loads(args.state.read_text(encoding="utf-8"))
        events = spec["events"]
        while state["next"] < len(events):
            index = state["next"]
            event = events[index]
            try:
                data = target.read_bytes()
            except FileNotFoundError:
                entries.append({"event": index + 1, "applied": False, "error": "apply_failed: the file is gone"})
                state["next"] = len(events)
                break
            written = digest(data) != state["sha256"]
            if at_stop:
                trigger = "stop"
            elif written:
                trigger = "write"
            elif event["after"] == "seen" and mentions(payload, raw, event["seen"]):
                trigger = "seen"
            else:
                break
            entry: dict[str, Any] = {
                "event": index + 1,
                "trigger": trigger,
                "tool": tool,
                "tool_use_id": tool_use_id,
                "call": state.get("calls", 0),
            }
            try:
                changed = apply_changes(data, event["changes"])
            except LookupError as error:
                entry.update(applied=False, error=f"apply_failed: {error}")
                entries.append(entry)
                state["next"] = len(events)
                break
            target.write_bytes(changed)
            entry["applied"] = True
            entries.append(entry)
            state["next"] = index + 1
            state["sha256"] = digest(changed)
            # One event per tool call: a write right after this event must be the
            # model's, not ours.
            if not at_stop:
                break
        if not at_stop and hook_event == "PostToolUse":
            state["calls"] = state.get("calls", 0) + 1
        args.state.write_text(json.dumps(state), encoding="utf-8")
    return entries


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--spec", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--at-stop", action="store_true", help="apply every event left")
    args = parser.parse_args(argv)
    raw = sys.stdin.read()
    try:
        entries = handle(args, raw)
    except Exception as error:  # never block the session; the harness reads the log
        entries = [{"error": f"hook_error: {type(error).__name__}: {error}"}]
    if entries:
        with args.log.open("a", encoding="utf-8") as log:
            for entry in entries:
                log.write(json.dumps(entry) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
