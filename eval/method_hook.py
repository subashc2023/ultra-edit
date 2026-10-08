"""PreToolUse hook for the benchmark arms held to one editing method.

Denies a shell command that writes files other than by the arm's method, so that
shell-sed measures sed, shell-python Python, and shell-patch git apply rather than
whatever the model reaches for, and a third-party MCP arm (method mcp) its server's
tools rather than sed or Python through Bash or the server's own process tool.
Usage, from the run's settings:

    python3 method_hook.py --method {sed,python,patch,mcp} [--how TEXT]  < hook event JSON

Anything it cannot read is allowed; the harness reports off-method writes that got
through as an integrity warning.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from run_eval import METHOD_DENIAL, METHODS, method_command, off_method_write  # noqa: E402


def decision(event: object, method: str, how: str | None = None) -> dict | None:
    """The hook output for one event: a deny decision, or None to allow. The settings'
    matcher picks the tools (Bash, or a server's process tool) whose commands count."""
    if not isinstance(event, dict) or not isinstance(event.get("tool_name"), str):
        return None
    if not off_method_write(method_command(event.get("tool_input")), method):
        return None
    return {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": METHOD_DENIAL.format(how=how or METHODS[method]),
        }
    }


def main(argv: list[str]) -> int:
    how = None
    if len(argv) == 4 and argv[2] == "--how" and argv[3]:
        argv, how = argv[:2], argv[3]
    if len(argv) != 2 or argv[0] != "--method" or argv[1] not in METHODS:
        print(f"usage: method_hook.py --method {{{','.join(METHODS)}}} [--how TEXT]", file=sys.stderr)
        return 2
    try:
        event = json.loads(sys.stdin.read() or "null")
    except ValueError:
        return 0
    output = decision(event, argv[1], how)
    if output is not None:
        print(json.dumps(output))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
