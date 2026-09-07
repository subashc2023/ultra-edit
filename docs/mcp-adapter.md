# MCP adapter and Claude Code package

Ultra Edit exposes the existing `Workspace` API through a local stdio MCP server.
The Claude Code plugin distributes its server configuration, context hooks, and
a small usage skill. The Rust engine remains responsible for snapshots, planning,
conditional commit, retry identity, receipts, and recovery.

## Start and scope

```text
/absolute/plugin/directory/runtime/ultra-edit-mcp --root /absolute/path/to/workspace
```

The plugin owns its executables under `runtime/`; it requires no global CLI
installation or `PATH` change. On Windows use `runtime/ultra-edit-mcp.exe` and
PowerShell's `&` operator. To prepare the plugin from source, run
`cargo build --locked --release`, then copy both native executables from
`target/release` into `plugin/claude-code/runtime`. The
[host setup guide](../plugin/claude-code/skills/edit/references/claude-code.md#connect-locally)
has complete platform-specific commands. A prepared OS/architecture-specific
archive would contain the binaries and need no Rust; public archives are not
available yet.

Server mode requires a root at startup, canonicalized once and absent from tool
input schemas. All cooperating processes must use that same canonical root. The
server uses JSON-RPC over stdio; stdout is reserved for protocol messages. A
snapshot operation records state under `.ultra-edit` but changes no target file.
Input JSON-RPC lines are limited to 16 MiB. Malformed JSON receives JSON-RPC
`-32700` with `id: null`; invalid request envelopes receive `-32600`. Both leave
the server reading subsequent messages, including messages already buffered in
the same write. Oversized frames, invalid UTF-8 framing, and stream I/O failures
close the transport with a stderr diagnostic and nonzero exit. Connection loss
does not imply rollback.

For persistent Claude Code use, copy all of the prepared
[plugin/claude-code](../plugin/claude-code/.claude-plugin/plugin.json) into
`~/.claude/skills/ultra-edit`, or `CLAUDE_CONFIG_DIR/skills/ultra-edit` when using
a custom configuration directory. Launch `claude` normally from the target
workspace. Claude Code discovers this as `ultra-edit@skills-dir`; it requires no
marketplace. See the official
[skills-directory plugin documentation](https://code.claude.com/docs/en/plugins-reference#skills-directory-plugins).
For a session-only test, use `claude --plugin-dir ABSOLUTE_PLUGIN_DIRECTORY`.
The packaged `.mcp.json` runs `${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp` with
the argument array `["--root", "${CLAUDE_PROJECT_DIR}"]`. The native launcher
resolves `.exe` on Windows. Hook entries point to the same executable with an
explicit argument array, so they launch directly without a shell. `runtime/`
keeps the binaries private; a plugin `bin/` directory would be added to Claude's
Bash session `PATH`. Plugin-root and project-root substitution are supported by
[Claude Code's MCP documentation](https://code.claude.com/docs/en/mcp).
See [host setup and permissions](../plugin/claude-code/skills/edit/references/claude-code.md).
The copied personal plugin applies across projects; `--plugin-dir` applies only
to its session. This repository does not yet include public release downloads.

The server root stays fixed after shell directory changes and when a subagent
enters a separate worktree. From another working directory, pass the intended
file's absolute path to snapshot. An intended file outside that root is a blocker;
never substitute a relative path that would edit the parent checkout instead.

The same executable also serves a non-MCP hook mode:

```text
/absolute/plugin/directory/runtime/ultra-edit-mcp --claude-context SessionStart
/absolute/plugin/directory/runtime/ultra-edit-mcp --claude-context SubagentStart
```

Each prints hook JSON containing the requested `hookEventName` and the embedded
editing policy as `additionalContext`, then exits without opening a workspace.
No `--root` is needed in this mode. The policy is compiled into the executable,
so plugin instruction updates require rebuilding and staging matching binaries
as well as manually updating the complete installed plugin copy. Generated
`runtime/` binaries are excluded from source control.

## Tools

The names below are server-side names; the host may add a namespace.

| Tool | Arguments | Engine operation |
| --- | --- | --- |
| `ultra_edit_snapshot` | `{path, selection: {kind: "range", first, last}}`, `{path, selection: {kind: "search", query, offset?, snapshot?}}`, or `{path, selection: {kind: "full", expected_bytes?}}` | Focused range, paged literal search, or bounded full read |
| `ultra_edit` | `EditRequest`: `{request_id, files: [{base, changes: [{id, target, text}]}]}` | Prepare and commit through `edit` |
| `ultra_edit_status` | `{query: {kind: "receipt", request_id, full?: boolean}}` or `{query: {kind: "evidence", reference}}` | Retrieve a compact outcome (default), full receipt (`full: true`), or explicit evidence |
| `ultra_edit_prepare` | The same direct `EditRequest` as `ultra_edit` | Persist a preview or rejected draft |
| `ultra_edit_commit` | `{plan}` | Commit the stored candidate |
| `ultra_edit_retry` | `{plan, request_id}` | Retry a proven preflight failure under a new ID, retaining the exact original candidate |
| `ultra_edit_repair` | `{reference, request_id, changes}` | Replace retained changes by ID and prepare again |
| `ultra_edit_undo` | `{plan, request_id}` | Conditionally restore confirmed committed files |
| `ultra_edit_diff` | `{plan, offset?}` | Read a unified diff in pages of at most 6,000 Unicode characters |
| `ultra_edit_inspect` | `{plan}` | Persist current recovery evidence and return a compact description and inspection reference |
| `ultra_edit_reconcile` | `{inspection, decision: "accept_current", note}` | Record an explicit reviewed operator decision; never automatically accept an uncertain outcome |

Ordinary work uses snapshot → edit → outcome inspection, with status for receipt
details or lost responses. Mutation results carry compact reports and references;
full source, diagnostics, and candidates require explicit evidence retrieval.
Every snapshot response uses `snapshot` for its base. Full reads default to
24,000 source bytes and 400 lines; an exact `expected_bytes` deliberately permits
a larger response up to the existing source-file limit. Search pages use
zero-based match offsets and return `next_offset`; supply the returned snapshot
to keep searching its immutable bytes. Each page discloses only its own editable
matches. Diff offsets instead count Unicode characters in the complete diff.
Read the [skill's complete example](../plugin/claude-code/skills/edit/SKILL.md)
and [contract](../plugin/claude-code/skills/edit/references/contract.md) for precise
request semantics. Inspect the returned `commit`, request ID, and plan ID rather
than treating a successful transport exchange as confirmed persistence.

Engine results contain structured JSON and equivalent JSON text. Compact completed
results use `{kind: "completed", request_id, plan_id, commit, report}`. A
preparation uses `kind: "ready"` or `"rejected"`, with `request_id`, `reference`,
`ready`, diagnostic count, compact diagnostics, and a report. Receipt requests
with `full: true` return `{kind: "receipt", receipt}`; a recorded request without
a receipt returns `{kind: "receipt_unavailable", request_id, receipt: null}`.
Evidence uses the engine's `{kind, value}` shape. Engine errors use
`{kind: "error", error: {code, message}, request_id, plan_id, commit}`.
Malformed tool arguments and protocol errors can use the MCP SDK's standard
error shapes, including text-only tool errors.
Stored snapshot evidence retains its internal `id`, rather than the snapshot
response's `snapshot` field. MCP receipt responses omit the engine's
unconfigurable `validation: "not_requested"`; run external project checks
separately. Nonblocking `NUL_BYTE` and `MIXED_LINE_ENDINGS` warnings describe the
candidate output without changing its bytes.

On Windows, response filesystem `path` fields, diagnostic and warning `file`
fields, and diff headers use conventional drive or UNC display spelling instead
of normal extended-length prefixes. Stored snapshots, plans, receipts, and
identity checks retain canonical paths. Other verbatim namespaces remain
unchanged. This projection changes presentation only; JSON still escapes
backslashes according to JSON syntax.

Rejected or incompletely committed mutation calls set MCP `isError`. Successful
status reads do not set it merely because a historical receipt describes a
failure. Cancellation or disconnection cannot establish rollback; inspect the
original request's receipt before starting new work.

Every tool schema has an object at its root. The `selection` and `query` fields
contain the typed alternatives for snapshot and status requests, respectively.

Recovery tools expose the same inspection and reconciliation protocol as the
CLI. Inspect the complete evidence through status before submitting an explicit
operator decision; an ordinary edit request does not authorize accepting an
uncertain state. There is no file creation/deletion/rename tool, generic shell
dispatcher, automatic reconciliation, or model-supplied workspace root.

## Instruction layout

The [canonical routing policy](../plugin/claude-code/instructions.md) is loaded
by the plugin's [hooks](../plugin/claude-code/hooks/hooks.json). `SessionStart`
has no matcher, so it covers every session start source, including startup,
resume, clear, compaction, and fork. `SubagentStart` supplies the same policy to
subagents. Claude Code receives this as context before the first prompt; an
ordinary edit request requires no skill invocation. See the official
[hook context contract](https://code.claude.com/docs/en/hooks#add-context-for-claude).

The policy requires direct Ultra Edit MCP calls for coordinated changes across
two or more existing UTF-8 files and forbids file writes through Bash heredocs,
generated-content redirection, inline scripts, or shell-piped edit JSON. Native
tools remain available for new files and isolated edits. If required MCP tools
are unavailable or denied, Claude is instructed to report the blocker. The
backslash concern comes from a user's 2026-09-05 Bash observation, not a claim
that this project reproduced it on every host. The hooks inject instructions;
they do not intercept or block native tools.
Explicit user instructions and host permissions take precedence over plugin
guidance. Report conflicting workflow instructions; generic advice to use sed
or heredocs is not a reason to silently abandon the user's explicit Ultra Edit
route.

The runtime contract is also carried in server instructions, tool descriptions,
and schemas. The [thin skill](../plugin/claude-code/skills/edit/SKILL.md) keeps the
ordinary flow and non-optional base, literal-text, retry, and outcome rules up
front. Its task/symptom table links to four focused references: contract, targets,
recovery, and Claude Code. Those references are inside the plugin, so installation
does not break their paths. Design and research documents are maintainer context,
not prerequisites for a normal edit.

The plugin grants no permissions and changes no Claude Code settings. MCP
authorization is distinct from native Edit's path rules; canonical-root checks
inside Ultra Edit do not implement those host-specific policies. Installation
and session startup are explicit operator actions; enabled plugin hooks run
automatically. A plugin-root `CLAUDE.md` is not automatically loaded by Claude
Code, so the package uses context hooks instead of modifying a user's instruction
files. See the official
[plugin layout reference](https://code.claude.com/docs/en/plugins-reference#plugin-directory-structure).

## Validation

Validate packaging without installing the plugin:

```text
claude plugin validate plugin/claude-code
```

Run the repository's configured Rust gates from [the README](../README.md#development).
The adapter's process tests cover initialization and tool discovery,
focused snapshot → edit → receipt, literal bytes, stale bases, retry after
restart, buffered messages, malformed frames and tool arguments, and
persistence-error reporting. A real
Claude Code session is a separate integration check: use a disposable workspace,
verify `/mcp` connects, confirm hook context loaded, ask for a multi-file edit
without invoking the skill, inspect exact resulting bytes, and test stale/retry
behavior. Manifest validation alone does not prove that the host can locate the
executable or discover and invoke the tools.

The initial smoke test passed with Claude Code 2.1.263 on 2026-09-07: the plugin
server connected, Claude loaded the skill, called snapshot → edit → status,
and the resulting file differed only at the requested span, preserving CRLF and
surrounding bytes. It used a disposable workspace and session-only tool grants.
This establishes the basic host integration; broader model comparisons and
multi-task evaluations remain separate work.

A second smoke passed with the same Claude Code version on 2026-09-07:
`SessionStart` loaded the policy automatically, and an ordinary request used
direct snapshots followed by one `ultra_edit` request containing two files,
without invoking the skill. Read, Edit, Write, Bash, and Skill remained available
alongside MCP. Exact resulting bytes passed checks for literal backslashes,
regex and escape text, Windows paths, BOM, CRLF/LF, trailing spaces, and a missing
final newline. Bash was used only for read-only inspection; a `git diff` command
failed because the disposable directory was not a Git repository, without
affecting the edits or byte checks. This verifies automatic routing for the
tested request, not guaranteed model compliance on every task.

Persistent discovery was tested separately with Claude Code 2.1.263 by copying
the whole plugin into an isolated `CLAUDE_CONFIG_DIR/skills/ultra-edit` and
launching without `--plugin-dir` or a slash command. `SessionStart` succeeded,
MCP connected, and Claude routed edits through two-file Ultra Edit batches.
The final exact-byte check passed. The first batch used incorrectly guessed
full-snapshot span IDs; Claude explicitly undid it and applied a corrected second
batch. Consequently the scratch check requiring exactly one edit failed. Plugin
discovery and routing passed, but first-attempt edit quality did not. This was a
model target-selection error, not an engine failure; broader model evaluation
remains necessary.

Those smoke results predate bundling executables in the plugin's `runtime/`
directory. The updated Windows and Linux MCP process suites passed all nine
tests, including direct hook and MCP launches with an empty `PATH` and a plugin
path containing spaces, an apostrophe, and `$`.

The bundled personal installation then passed a live Claude Code 2.1.263 check
on 2026-09-07. Neither Ultra Edit executable was available as a global command;
Claude launched normally without `--plugin-dir`, discovered the enabled plugin,
ran `SessionStart` successfully, and connected its MCP server. An ordinary edit
request used direct snapshots and one two-file batch, with no skill invocation
or native file/shell tool calls. Both final files matched the expected bytes,
including preserved backslashes, BOM, CRLF/LF, trailing spaces, and the missing
final newline. Tool grants were limited to that disposable verification session.
