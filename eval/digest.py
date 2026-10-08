#!/usr/bin/env python3
"""Write a compact, human-readable digest of every run in an eval results dir.

    python3 eval/digest.py RESULTS_DIR OUT_DIR

OUT_DIR/<run_id>.txt holds one run: outcome and totals, then each API call in
order with its context/output tokens, the assistant's text, each tool call
(name, input bytes, abbreviated input) and each result (error flag, bytes,
abbreviated text), then the byte diff for failed runs.
"""

import json
import sys
from pathlib import Path

INPUT_CHARS = 700
RESULT_CHARS = 400
TEXT_CHARS = 400


def short(text, limit):
    text = text if isinstance(text, str) else json.dumps(text, ensure_ascii=False)
    return text if len(text) <= limit else text[:limit] + f"...[+{len(text) - limit} chars]"


def result_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for item in content:
            if isinstance(item, dict):
                if item.get("type") == "text":
                    parts.append(item.get("text", ""))
                else:
                    parts.append(json.dumps(item, ensure_ascii=False))
        return "\n".join(parts)
    return json.dumps(content, ensure_ascii=False)


def digest(run_dir: Path, record: dict) -> str:
    lines = []
    keys = (
        "outcome",
        "cost_usd",
        "context_tokens_total",
        "output_tokens",
        "first_call_context_tokens",
        "turns",
        "api_calls",
        "tool_calls",
        "tool_errors",
        "edit_calls",
        "edit_failures",
        "shell_errors",
        "bash_write_attempts",
        "bash_writes",
        "toolsearch_calls",
        "read_calls",
        "reread_calls",
        "tool_input_bytes",
        "tool_result_bytes",
        "duration_ms",
    )
    metrics = record.get("metrics", record)
    head = {key: metrics.get(key, record.get(key)) for key in keys}
    lines.append(
        f"RUN {record.get('run_id')}  task={record.get('task')} arm={record.get('arm')} rep={record.get('rep')}"
    )
    lines.append("  " + "  ".join(f"{k}={v}" for k, v in head.items() if v is not None))
    tools_by = metrics.get("tool_calls_by_name") or record.get("tool_calls_by_name")
    if tools_by:
        lines.append("  tools: " + ", ".join(f"{k}x{v}" for k, v in tools_by.items()))
    stream = run_dir / "stream.jsonl"
    if not stream.exists():
        lines.append("  (no stream.jsonl)")
        return "\n".join(lines)
    seen_ids = set()
    step = 0
    for raw in stream.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(raw)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        if kind == "assistant":
            message = event.get("message", {})
            mid = message.get("id")
            sub = " [subagent]" if event.get("parent_tool_use_id") else ""
            if mid not in seen_ids:
                seen_ids.add(mid)
                step += 1
                usage = message.get("usage", {})
                ctx = sum(
                    int(usage.get(k) or 0)
                    for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
                )
                lines.append(f"\n#{step}{sub} ctx={ctx} out={usage.get('output_tokens')}")
            for block in message.get("content", []):
                if block.get("type") == "text" and block.get("text", "").strip():
                    lines.append("  TEXT: " + short(block["text"].strip(), TEXT_CHARS))
                elif block.get("type") == "tool_use":
                    payload = json.dumps(block.get("input"), ensure_ascii=False, separators=(",", ":"))
                    name = block.get("name", "")
                    name = name.replace("mcp__plugin_ultra-edit_ultra-edit__", "UE:")
                    lines.append(f"  CALL {name} ({len(payload.encode())}B): " + short(payload, INPUT_CHARS))
        elif kind == "user":
            content = event.get("message", {}).get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_result":
                        text = result_text(block.get("content"))
                        flag = "ERROR " if block.get("is_error") else ""
                        lines.append(
                            f"  -> {flag}({len(text.encode())}B): "
                            + short(text, RESULT_CHARS).replace("\n", "\\n")
                        )
        elif kind == "system" and event.get("subtype") == "hook_response":
            if "deny" in json.dumps(event):
                lines.append("  HOOK DENY: " + short(json.dumps(event, ensure_ascii=False), 300))
        elif kind == "result":
            lines.append(
                f"\nRESULT subtype={event.get('subtype')} turns={event.get('num_turns')} cost={event.get('total_cost_usd')}"
            )
            lines.append("  FINAL: " + short(event.get("result") or "", 600))
    diff = run_dir / "diff.txt"
    if diff.exists():
        text = diff.read_text(encoding="utf-8", errors="replace").splitlines()
        lines.append(f"\nDIFF ({len(text)} lines, first 80):")
        lines.extend("  " + line for line in text[:80])
    return "\n".join(lines)


def main():
    results = Path(sys.argv[1])
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    count = 0
    for raw in (results / "runs.jsonl").read_text(encoding="utf-8").splitlines():
        record = json.loads(raw)
        run_id = record.get("run_id")
        run_dir = results / "runs" / run_id
        (out / f"{run_id}.txt").write_text(digest(run_dir, record), encoding="utf-8")
        count += 1
    print(f"wrote {count} digests to {out}")


if __name__ == "__main__":
    main()
