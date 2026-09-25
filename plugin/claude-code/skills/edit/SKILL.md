---
name: edit
description: Detailed workflow for Ultra Edit's required multi-file MCP editing route. Use for snapshot and batch examples, checked original bases, previews, repair, or conditional undo; the plugin loads its routing policy automatically.
---

# Ultra Edit

The plugin loads its [routing policy](../../instructions.md) automatically in
main sessions and subagents. ALWAYS use direct Ultra Edit MCP calls for
coordinated edits to two or more existing UTF-8 files. This skill supplies
workflow detail; invoking it is not a prerequisite for an ordinary edit request.
Never carry file contents or edit JSON through Bash heredocs, generated-content
redirection, or inline editing scripts. Report unavailable or denied required
tools instead of silently changing routes.
Explicit user instructions and host permissions take precedence; report
conflicting editing guidance rather than silently switching routes.

The server-side names are below; select the corresponding namespaced tools
exposed by this host.

- Read a snapshot before editing. Inspect its source and span bounds, and use
  only the returned base and disclosed span IDs; never infer IDs from change
  order. Native Read/Grep output does not supply an Ultra Edit base. Every change
  resolves against the original snapshots, never another change's output.
  All snapshot responses return `snapshot`; use exact text or optional
  `span.expect` to guard against selecting a mistaken span.
- The server root stays fixed after directory changes or a subagent's worktree
  change. When working elsewhere, snapshot the intended absolute path. Report
  files outside the server root; never use a relative path that would redirect
  a worktree edit to the parent checkout.
- Replacement `text` is literal. Preserve intended Unicode, whitespace, and
  newline bytes; write `\r\n` for new CRLF text. No regex or newline conversion runs.
- Omit `request_id` and change `id`. Identical arguments derive the same IDs, so
  after lost output, repeat the identical call: it returns the recorded result,
  marked `replayed: true`, without writing again. Pass an explicit new
  `request_id` only to name a request yourself; `ultra_edit_retry` always
  requires one.
- Confirm `commit: "committed"` before reporting all edits applied. `partial`
  and `outcome_unknown` require recovery; stop further mutations on uncertainty.
  Current bytes matching the candidate do not prove historical success.

## Normal flow

1. Call `ultra_edit_snapshot` with `path` and a focused `selection`: an inclusive
   line `range`, or a literal `search`. Inspect the returned source and spans.
   Range and full reads summarize disclosed IDs in `spans` (`["r12..r18",
   "selection"]`) and list each line in `lines` as `"r12 | const retries = 2;"`;
   pick `r{n}` from that listing rather than counting newlines in `text`.
   Use `full` only when the complete file is needed. Full reads default to 24,000
   bytes and 400 lines; oversized reads require a deliberate exact
   `expected_bytes` override. Combine each file's changes under one base
   snapshot. To reach a distant region of the same file, continue the read: add
   the previous `snapshot` to the next range selection, or pass a search page's
   `next_offset` and `snapshot`. The new snapshot keeps every reference already
   disclosed (`selection` names only the latest range), so use the last one as
   the file's base. `stale: true` means the file changed since that source was
   read, so read again instead of editing.
2. Call `ultra_edit` with one request containing the intended changes. Use a
   disclosed `span`, or `exact` with an explicit scope when only the inspected
   region should match. Resolve all targets against that base.
   `scope` is a returned span ID, not source text. `all.expected` counts
   non-overlapping, left-to-right replacements. For `TARGET_AMBIGUOUS`, `actual`
   counts overlapping starts; use the message's parenthesized count only with the
   same `old` and disclosed scope in a `{"kind":"all"}` target. Empty exact text
   is invalid; insert through a returned zero-width span where available. To add
   a line between nonblank lines, replace the preceding body with original + line
   ending + insertion, or the following body with insertion + line ending + original.
   A missing target or failed `expect` can return `candidates`. Copy an `exact`
   or `whitespace` candidate's `text` verbatim into `old`; confirm a `similar`
   one is the intended region first. Then call `ultra_edit_repair` with the
   draft `reference`, replacing only the failed `change_id`.
3. Inspect the returned outcome. `replayed: true` marks a recorded result; take
   fresh snapshots to apply a change again. For every file outcome, call
   `ultra_edit_status` with
   `{"query":{"kind":"receipt","request_id":"RETURNED_ID","full":true}}`.
   Run project validation separately; the engine runs no tests.

Example: change a known line in an existing file. First call
`ultra_edit_snapshot`:

```json
{
  "path": "src/retry.rs",
  "selection": { "kind": "search", "query": "const RETRIES: usize = 2;" }
}
```

After inspecting the result, if the desired match has returned span `m1`, call
`ultra_edit` with the result's `snapshot` value substituted for the base below.
The placeholder is not a valid snapshot; do not send it literally.

```json
{
  "files": [{
    "base": "REPLACE_WITH_RETURNED_SNAPSHOT",
    "changes": [{
      "target": { "kind": "span", "span": "m1", "expect": "const RETRIES: usize = 2;" },
      "text": "const RETRIES: usize = 3;"
    }]
  }]
}
```

The change gets ID `1.1` (file entry 1, change 1), and the response's
`request_id` is the derived receipt key.

## Read only the reference needed

| Task or symptom | Reference |
| --- | --- |
| Request identity and replay, byte preservation, limits, or what a receipt proves | [Contract](references/contract.md) |
| Choose range/search/full; continue a range; ambiguous matches; near-miss candidates; replace-all; span overlap | [Targets](references/targets.md) |
| Preview/diff then commit; correct a batch; lost response; preflight retry; inspect/reconcile uncertainty; undo | [Recovery](references/recovery.md) |
| Connect the plugin; missing tools; fixed workspace root; host permissions; blocked shell command | [Claude Code](references/claude-code.md) |

The plugin grants no permissions; follow the user's existing authorization and
this host's permissions. Its PreToolUse hook denies Bash and PowerShell commands
that write file content into the project, such as heredoc, here-string, `echo`,
or `Set-Content` writes, inline interpreter writes, `sed -i`, and `-replace`
rewrites; redo a blocked write with Ultra Edit, Edit, or Write, never by
rephrasing the command to evade the guard.
File creation, deletion, and rename are outside this tool's scope. Use native
Write for new files or isolated full rewrites, and native Edit or Ultra Edit for
isolated targeted edits. Do not split a coordinated multi-file edit into native
calls to avoid its required route.
