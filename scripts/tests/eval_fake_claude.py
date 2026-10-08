"""A stand-in for `claude` used by test_eval.py; it never contacts a model.

It answers --version, --help, and `plugin validate`, then acts like a
`claude -p --output-format stream-json` session: it reads the prompt from
stdin, writes the files listed in $FAKE_CLAUDE_PLAN into the working
directory, and prints stream-json messages. With --plugin-dir it reports the
Ultra Edit plugin, MCP server, and SessionStart hook in init. With --mcp-config
it reports each configured server as connected, lists the plan's "mcp_tools"
for it, and edits through its "mcp_edit_tool". --disallowedTools removes tools
from init. When the --settings file defines PreToolUse hooks it runs them, as
Claude Code does, on one read-only and one heredoc Bash call. The plan's
"reads" lists files it Reads before writing; PostToolUse hooks run after each
Read and each write, and Stop hooks at the end. The plan's
"sleep" maps an arm name (read from the run directory) to seconds to wait
before finishing. It logs what it saw to $FAKE_CLAUDE_LOG, or to
$FAKE_CLAUDE_LOG_DIR/<run directory>.json for parallel runs.
"""

import base64
import json
import os
import pathlib
import re
import subprocess
import sys
import time

HELP = """Usage: claude [options] [command] [prompt]
  -p, --print
  --output-format <format>
  --verbose
  --settings <file-or-json>
  --setting-sources <sources>
  --plugin-dir <path>
  --permission-mode <mode>
  --permission-prompts <target>
  --include-hook-events
  --no-session-persistence
  --max-budget-usd <amount>
  --allowedTools, --allowed-tools <tools...>
  --disallowedTools, --disallowed-tools <tools...>
  --append-system-prompt <prompt>
  --mcp-config <configs...>
"""
ULTRA_EDIT = "mcp__plugin_ultra-edit_ultra-edit__ultra_edit"


def emit(message):
    sys.stdout.write(json.dumps(message) + "\n")


def assistant(message_id, block, context=1000):
    """Like Claude Code, one API response becomes one line per content block, and
    every line repeats the response's usage."""
    usage = {
        "input_tokens": 3,
        "cache_read_input_tokens": context - 53,
        "cache_creation_input_tokens": 50,
        "output_tokens": 20,
    }
    for content in ({"type": "text", "text": "Working."}, block):
        emit(
            {
                "type": "assistant",
                "session_id": "fake",
                "parent_tool_use_id": None,
                "message": {
                    "id": message_id,
                    "role": "assistant",
                    "model": "claude-fake",
                    "content": [content],
                    "usage": usage,
                },
            }
        )


def tool_result(tool_id, content, is_error=False):
    block = {"type": "tool_result", "tool_use_id": tool_id, "content": content, "is_error": is_error}
    emit(
        {
            "type": "user",
            "session_id": "fake",
            "parent_tool_use_id": None,
            "message": {"role": "user", "content": [block]},
        }
    )


def hook_response(event, completed):
    emit(
        {
            "type": "system",
            "subtype": "hook_response",
            "hook_event": event,
            "hook_name": f"{event}:Bash",
            "outcome": "success" if completed.returncode in (0, 2) else "error",
            "exit_code": completed.returncode,
            "stdout": completed.stdout,
            "output": completed.stdout,
            "stderr": completed.stderr,
        }
    )


def run_hooks(settings, event, payload, tool_name=None):
    """Run the settings' hooks for one event, as Claude Code does, and report them."""
    for entry in settings.get("hooks", {}).get(event, []):
        matcher = entry.get("matcher")
        if tool_name is not None and matcher not in (None, "", "*"):
            if not re.fullmatch(matcher, tool_name):
                continue
        for hook in entry["hooks"]:
            completed = subprocess.run(
                [hook["command"], *hook.get("args", [])],
                input=json.dumps({"hook_event_name": event, "cwd": os.getcwd(), **payload}),
                capture_output=True,
                text=True,
                check=False,
            )
            hook_response(event, completed)


def option(args, name):
    return args[args.index(name) + 1] if name in args else None


def main():
    args = sys.argv[1:]
    if args == ["--version"]:
        print("2.1.282 (Claude Code)")
        return
    if args == ["--help"]:
        print(HELP)
        return
    if args[:2] == ["plugin", "validate"]:
        return
    prompt = sys.stdin.read()
    plan = json.loads(pathlib.Path(os.environ["FAKE_CLAUDE_PLAN"]).read_text(encoding="utf-8"))
    settings = json.loads(pathlib.Path(option(args, "--settings")).read_text(encoding="utf-8"))
    plugin_dir = option(args, "--plugin-dir")
    mcp_path = option(args, "--mcp-config")
    mcp = json.loads(pathlib.Path(mcp_path).read_text(encoding="utf-8")) if mcp_path else None
    disallowed = (option(args, "--disallowedTools") or "").split(",")
    run_name = pathlib.Path(option(args, "--settings")).parent.name
    arm = run_name.split("__")[1] if run_name.count("__") == 2 else ""
    # %at, not %aI: newer git prints a UTC %aI with "Z" instead of "+00:00".
    git_log = subprocess.run(
        ["git", "log", "-1", "--format=%an|%ae|%at|%s"], capture_output=True, text=True, check=False
    ).stdout.strip()
    autocrlf = subprocess.run(
        ["git", "config", "--get", "core.autocrlf"], capture_output=True, text=True, check=False
    ).stdout.strip()
    log = {
        "argv": args,
        "prompt": prompt,
        "cwd": os.getcwd(),
        "git_log": git_log,
        "autocrlf": autocrlf,
        "mcp_config": mcp,
        "env": {
            name: os.environ.get(name)
            for name in ("CLAUDECODE", "DISABLE_AUTOUPDATER", "ENABLE_CLAUDEAI_MCP_SERVERS")
        },
    }
    if os.environ.get("FAKE_CLAUDE_LOG_DIR"):
        log_path = pathlib.Path(os.environ["FAKE_CLAUDE_LOG_DIR"]) / f"{run_name}.json"
    else:
        log_path = pathlib.Path(os.environ["FAKE_CLAUDE_LOG"])
    log_path.write_text(json.dumps(log), encoding="utf-8")

    init = {
        "type": "system",
        "subtype": "init",
        "session_id": "fake",
        "cwd": os.getcwd(),
        "tools": ["Read", "Edit", "Write", "Bash"],
        "mcp_servers": [],
        "model": "claude-fake",
        "permissionMode": "bypassPermissions",
        "claude_code_version": "2.1.282",
        "plugins": [],
    }
    edit_tool = "Edit"
    if plugin_dir:
        edit_tool = ULTRA_EDIT
        init["plugins"] = [{"name": "ultra-edit", "path": plugin_dir}]
        init["mcp_servers"] = [{"name": "plugin:ultra-edit:ultra-edit", "status": "connected"}]
        init["tools"].append(ULTRA_EDIT)
        hook_response("SessionStart", subprocess.CompletedProcess([], 0, "{}", ""))
    for server in (mcp or {}).get("mcpServers", {}):
        init["mcp_servers"].append({"name": server, "status": "connected"})
        init["tools"] += [f"mcp__{server}__{tool}" for tool in plan.get("mcp_tools", [])]
        edit_tool = f"mcp__{server}__{plan.get('mcp_edit_tool', 'edit_file')}"
    init["tools"] = [tool for tool in init["tools"] if tool not in disallowed]
    emit(init)

    # plan["rate_limited"] = {arm: N}: the first N attempts of each run of that arm
    # end with an API rate-limit error, as a busy API would.
    limited = plan.get("rate_limited", {}).get(arm, 0)
    if limited:
        counter = pathlib.Path(os.environ["FAKE_CLAUDE_PLAN"]).with_name(f"attempts-{run_name}")
        attempts = int(counter.read_text()) if counter.exists() else 0
        counter.write_text(str(attempts + 1))
        if attempts < limited:
            emit(
                {
                    "type": "assistant",
                    "error": "rate_limit",
                    "message": {"id": "msg_limited", "role": "assistant", "content": [], "usage": {}},
                    "parent_tool_use_id": None,
                }
            )
            emit({"type": "result", "subtype": "error_during_execution", "is_error": True, "num_turns": 1})
            return

    for entry in settings.get("hooks", {}).get("PreToolUse", []):
        hook = entry["hooks"][0]
        for index, command in enumerate(("git status --short", "cat > notes.txt <<'EOF'\nx\nEOF")):
            tool_id = f"toolu_bash_{index}"
            assistant(
                f"msg_bash_{index}",
                {"type": "tool_use", "id": tool_id, "name": "Bash", "input": {"command": command}},
            )
            payload = {
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": {"command": command},
                "tool_use_id": tool_id,
                "cwd": os.getcwd(),
            }
            completed = subprocess.run(
                [hook["command"], *hook.get("args", [])],
                input=json.dumps(payload),
                capture_output=True,
                text=True,
                check=False,
            )
            hook_response("PreToolUse", completed)
            denied = completed.returncode == 2 or '"deny"' in completed.stdout
            tool_result(tool_id, "blocked by hook" if denied else "", is_error=denied)

    for index, relative in enumerate(plan.get("reads", [])):
        tool_id = f"toolu_read_{index}"
        tool_input = {"file_path": str(pathlib.Path(relative).resolve())}
        assistant(f"msg_read_{index}", {"type": "tool_use", "id": tool_id, "name": "Read", "input": tool_input})
        content = pathlib.Path(relative).read_text(encoding="utf-8")
        response = {"type": "text", "file": {"filePath": tool_input["file_path"], "content": content}}
        payload = {"tool_name": "Read", "tool_input": tool_input, "tool_response": response, "tool_use_id": tool_id}
        run_hooks(settings, "PostToolUse", payload, "Read")
        tool_result(tool_id, content)

    for index, (relative, encoded) in enumerate(sorted(plan["writes"].items())):
        tool_id = f"toolu_{index}"
        assistant(
            f"msg_{index}",
            {"type": "tool_use", "id": tool_id, "name": edit_tool, "input": {"file_path": relative}},
            context=1000 + 100 * index,
        )
        path = pathlib.Path(relative)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(base64.b64decode(encoded))
        payload = {"tool_name": edit_tool, "tool_input": {"file_path": relative}, "tool_response": "ok", "tool_use_id": tool_id}
        run_hooks(settings, "PostToolUse", payload, edit_tool)
        tool_result(tool_id, json.dumps({"kind": "completed", "commit": "committed"}) if plugin_dir else "ok")

    time.sleep(plan.get("sleep", {}).get(arm, 0))
    run_hooks(settings, "Stop", {"stop_hook_active": False})
    emit(
        {
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "num_turns": len(plan["writes"]) + 1,
            "duration_ms": 10,
            "duration_api_ms": 5,
            "result": "done",
            "session_id": "fake",
            "total_cost_usd": 0.0125,
            "usage": {
                "input_tokens": 3,
                "output_tokens": 40,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 900,
            },
            "modelUsage": {
                "claude-fake": {
                    "inputTokens": 3,
                    "outputTokens": 40,
                    "cacheReadInputTokens": 900,
                    "cacheCreationInputTokens": 100,
                    "costUSD": 0.0125,
                }
            },
            "permission_denials": [],
        }
    )


if __name__ == "__main__":
    main()
