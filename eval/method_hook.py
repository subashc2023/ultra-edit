"""PreToolUse hook for the shell benchmark arms.

Denies a Bash command that writes files other than by the arm's method, so that
shell-sed measures sed, shell-python Python, and shell-patch git apply rather than
whatever the model reaches for. Usage, from the run's settings:

    python3 method_hook.py --method {sed,python,patch}  < hook event JSON

Anything it cannot read is allowed; the harness reports off-method writes that got
through as an integrity warning.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from run_eval import METHOD_DENIAL, SHELL_METHODS, off_method_write  # noqa: E402


def decision(event: object, method: str) -> dict | None:
    """The hook output for one event: a deny decision, or None to allow."""
    if not isinstance(event, dict) or event.get("tool_name") != "Bash":
        return None
    tool_input = event.get("tool_input")
    command = tool_input.get("command") if isinstance(tool_input, dict) else None
    if not off_method_write(command, method):
        return None
    return {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": METHOD_DENIAL.format(how=SHELL_METHODS[method][1]),
        }
    }


def main(argv: list[str]) -> int:
    if len(argv) != 2 or argv[0] != "--method" or argv[1] not in SHELL_METHODS:
        print(f"usage: method_hook.py --method {{{','.join(SHELL_METHODS)}}}", file=sys.stderr)
        return 2
    try:
        event = json.loads(sys.stdin.read() or "null")
    except ValueError:
        return 0
    output = decision(event, argv[1])
    if output is not None:
        print(json.dumps(output))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
