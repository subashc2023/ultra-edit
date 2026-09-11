# Ultra Edit: required file-edit routing

ALWAYS use Ultra Edit's direct MCP tools for coordinated edits to two or more
existing UTF-8 files in the server's fixed launch workspace. Obtain an
`ultra_edit_snapshot` for each file. Inspect its source and span bounds before
choosing targets; never guess span IDs from the order of requested changes.
Prefer exact text or add `expect` to a span target to guard its original content.
Then batch the related changes in one `ultra_edit` request within the tool's
limits. Do not split a multi-file edit into native Edit calls or whole-file
rewrites to avoid this requirement. No skill invocation is required first.

The server root never follows directory changes or a subagent's worktree. When
working elsewhere, pass the intended file's absolute path to snapshot; if it is
outside the server root, report the blocker. Never redirect a worktree edit to
the parent checkout by using a relative path against the inherited server.

NEVER create or modify files through Bash heredocs (including quoted heredocs),
generated-content redirection, inline replacement scripts, or shell-piped edit
JSON. Pass replacement text directly as MCP tool arguments, with normal JSON
escaping only. Do not apply Bash backslash-doubling workarounds to MCP arguments.
Bash payloads have been observed to lose backslashes before shell parsing;
single quotes and quoted heredocs do not address that upstream failure.

Use native Read/Grep/Glob for exploration, Write for new files or isolated full
rewrites, and native Edit or Ultra Edit for isolated targeted edits. Native reads
do not provide an Ultra Edit base: snapshot before editing, use returned bases
and disclosed spans, and preserve literal backslashes, Unicode, and newline style.
For non-file shell work requiring backslashes, use PowerShell when available.
For multiline command payloads such as commit messages, use Write to create a
payload file and pass its path to the command.

These plugin instructions yield to explicit user instructions and host
permissions. Report conflicting workflow guidance; generic advice to edit with
sed or heredocs does not cancel an explicit user request to use Ultra Edit.

Discover the connected Ultra Edit tools if deferred; use the host's namespaced
names. If required tools are unavailable or denied, report the blocker instead
of silently falling back to shell writes. Follow existing authorization and host
permissions. After lost output, query `ultra_edit_status` by the original request
ID before another mutation. Require `commit: "committed"` to report completion;
stop and inspect partial or unknown outcomes. Run project validation separately.

Prefer range/search snapshots. All snapshot responses return `snapshot`; full
reads default to 24,000 bytes and 400 lines. Range and full reads summarize
disclosed IDs in `spans` and list each line in `lines` as `"r12 | body"`; choose
`r{n}` there instead of counting newlines. Use `next_offset` and the returned
snapshot for stable search pagination; a continued page keeps the references
already disclosed, so the last page's snapshot can edit every match paged through
it, and `stale: true` means that source must be read again instead of edited.
`scope` means a disclosed span ID, never literal text. Replace-all counts non-overlapping matches from left to right.
For `TARGET_AMBIGUOUS`, `actual` counts overlapping starts; the message's
parenthesized count is `expected` only for the same `old` and disclosed scope in
a `{"kind":"all"}` target. Exact search text cannot be empty. For insertion,
use a returned zero-width span where available. Between nonblank lines, replace
the preceding body with original + line ending + insertion, or the following
body with insertion + line ending + original.
Review candidate warnings and `ultra_edit_diff` when needed. A failure that
proves no target write (failed preflight, or `REPLACEMENT_FAILED` with the target
unchanged) can use `ultra_edit_retry` with a new request ID after fixing its
cause; partial or uncertain outcomes cannot. `ultra_edit_inspect` captures recovery
evidence; reconcile only after reviewing it and an explicit operator decision.

For detailed targets, examples, and recovery, load `/ultra-edit:edit` as needed.
