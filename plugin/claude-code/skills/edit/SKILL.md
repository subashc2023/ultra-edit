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
- Reuse the exact request ID and arguments for a retry. Changed intent needs a
  new ID. After lost output, retrieve the receipt before any new mutation.
- Confirm `commit: "committed"` before reporting all edits applied. `partial`
  and `outcome_unknown` require recovery; stop further mutations on uncertainty.
  Current bytes matching the candidate do not prove historical success.

## Normal flow

1. Call `ultra_edit_snapshot` with `path` and a focused `selection`: an inclusive
   line `range`, or a literal `search`. Inspect the returned source and spans.
   Use `full` only when the complete file is needed. Combine each file's changes
   under one base snapshot. Full reads default to 24,000 bytes and 400 lines;
   oversized reads require a deliberate exact `expected_bytes` override. Search
   pages return `next_offset`; supply it and the returned `snapshot` to continue
   the same original bytes.
2. Call `ultra_edit` with one request containing the intended changes. Use a
   disclosed `span`, or `exact` with an explicit scope when only the inspected
   region should match. Resolve all targets against that base.
   `scope` is a returned span ID, not source text. `all.expected` counts
   non-overlapping, left-to-right replacements.
3. Inspect the returned outcome. Call `ultra_edit_status` with
   `{"query":{"kind":"receipt","request_id":"..."}}` after lost output;
   add `"full":true` inside `query` for every file outcome. Run project validation
   separately; the engine runs no tests.

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
  "request_id": "retry-limit-1",
  "files": [{
    "base": "REPLACE_WITH_RETURNED_SNAPSHOT",
    "changes": [{
      "id": "retry-limit",
      "target": { "kind": "span", "span": "m1", "expect": "const RETRIES: usize = 2;" },
      "text": "const RETRIES: usize = 3;"
    }]
  }]
}
```

For the recorded outcome, call `ultra_edit_status`:

```json
{ "query": { "kind": "receipt", "request_id": "retry-limit-1" } }
```

## Read only the reference needed

| Task or symptom | Reference |
| --- | --- |
| Request identity, byte preservation, limits, or what a receipt proves | [Contract](references/contract.md) |
| Choose range/search/full; ambiguous matches; replace-all; span overlap | [Targets](references/targets.md) |
| Preview/diff then commit; correct a batch; preflight retry; inspect/reconcile uncertainty; undo | [Recovery](references/recovery.md) |
| Connect the plugin; missing tools; fixed workspace root; host permissions | [Claude Code](references/claude-code.md) |

The plugin supplies context hooks without permission grants or tool-blocking
hooks. Follow the user's existing authorization and this host's permissions.
File creation, deletion, and rename are outside this tool's scope. Use native
Write for new files or isolated full rewrites, and native Edit or Ultra Edit for
isolated targeted edits. Do not split a coordinated multi-file edit into native
calls to avoid its required route.
