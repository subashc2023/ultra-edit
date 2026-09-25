# Ultra Edit: required file-edit routing

ALWAYS use Ultra Edit's MCP tools for coordinated edits to two or more existing
UTF-8 files in the server's fixed launch workspace: `ultra_edit_snapshot` each
file, then send the related changes in one `ultra_edit` request. Do not split a
multi-file edit into native Edit calls or whole-file rewrites to avoid this. No
skill invocation is required first. Discover the tools if they are deferred, and
use the host's namespaced names.

NEVER create or modify files through Bash heredocs, echo/printf redirection,
inline interpreter scripts, `sed -i`, or shell-piped patches or edit JSON. Bash
payloads have been observed to lose backslashes before the shell parses them,
and quoting does not help. A plugin hook blocks these commands; when it does,
redo the change with Ultra Edit, native Edit, or Write instead of rephrasing the
command. Pass replacement text as MCP arguments with normal JSON escaping, never
Bash backslash-doubling. For multiline command input such as commit messages,
create a file with Write and pass its path. For other shell work that needs
backslashes, use PowerShell when available.

Use native Read/Grep/Glob to explore, Write for new files or full rewrites, and
native Edit or Ultra Edit for isolated edits. Native reads do not provide an
Ultra Edit base. These instructions yield to explicit user instructions and host
permissions: report conflicts, and report unavailable or denied tools instead of
falling back to shell writes. Generic advice to edit with sed or heredocs does
not cancel an explicit user request to use Ultra Edit.

The server root never follows `cd` or a subagent's worktree. From elsewhere, pass
the intended file's absolute path; if it is outside the root, report the blocker.
Never redirect a worktree edit to the parent checkout with a relative path.

Snapshots: prefer a line `range` or literal `search`; `full` is capped at 24,000
bytes and 400 lines. Pick `r{n}` from the `lines` listing (`"r12 | body"`) rather
than counting newlines, and use only span IDs the base disclosed. To edit distant
regions of one file in one request, continue a range or search by passing the
previous `snapshot`: the new snapshot keeps the earlier spans, and `selection`
means the latest range. Use one base per file. `stale: true` means read again.
Guard span targets with `expect`, or target exact text; `scope` is a span ID,
never text.

Requests: omit `request_id` and change `id`s; they are derived from the request.
Repeating an identical call returns the recorded result with `replayed: true` and
writes nothing; take fresh snapshots to apply a change again. Retry is the
exception: `ultra_edit_retry` needs a new explicit `request_id`. Require
`commit: "committed"` before reporting success; stop and inspect `partial` or
`outcome_unknown`, and never retry those under a new ID. Run project validation
separately.

When a target is not found, diagnostics may list `candidates` with exact current
text and line numbers. Copy an `exact` or `whitespace` candidate's text verbatim
into `old`; confirm a `similar` candidate is the region you meant before using
it. Correct only that change with `ultra_edit_repair` on the returned draft.

Replace-all needs a scope and an `expected` count of non-overlapping matches. For
`TARGET_AMBIGUOUS`, `actual` counts overlapping starts; the message's
parenthesized count is `expected` only for the same `old` and scope in an `all`
target. Exact text cannot be empty: insert through a returned zero-width span, or
replace a neighbouring line body with itself plus a line ending and the new line.
A failure that proves no target write (failed preflight, or `REPLACEMENT_FAILED`
with the target unchanged) can use `ultra_edit_retry` after fixing its cause.
`ultra_edit_inspect` captures evidence for uncertain outcomes; reconcile only
after an explicit operator decision.

For detailed targets, examples, and recovery, load `/ultra-edit:edit` as needed.
