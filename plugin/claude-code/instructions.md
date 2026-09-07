# Ultra Edit: required file-edit routing

ALWAYS use Ultra Edit's direct MCP tools for coordinated edits to two or more
existing UTF-8 files in the server's fixed launch workspace. Obtain an
`ultra_edit_snapshot` for each file. Inspect its source and span bounds before
choosing targets; never guess span IDs from the order of requested changes.
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

Discover the connected Ultra Edit tools if deferred; use the host's namespaced
names. If required tools are unavailable or denied, report the blocker instead
of silently falling back to shell writes. Follow existing authorization and host
permissions. After lost output, query `ultra_edit_status` by the original request
ID before another mutation. Require `commit: "committed"` to report completion;
stop and inspect partial or unknown outcomes. Run project validation separately.

For detailed targets, examples, and recovery, load `/ultra-edit:edit` as needed.
