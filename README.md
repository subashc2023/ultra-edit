# ULTRA Edit

**A Claude Code plugin that gives Claude a better tool for coordinated file
edits, powered by Rust.**

Install it once and its editing instructions load automatically into Claude's
working context: at session start, on resume, after compaction, and for subagents.
They tell Claude to use Ultra Edit for changes across multiple existing UTF-8 files
and **stop writing file contents through Bash heredocs or inline editing
scripts**. No slash command or repeated reminder is needed.

Built for models that reach for Bash to edit, Ultra Edit aims to cut the token
waste of generated scripts, whole-file rewrites, and escape-repair attempts.
That waste gets expensive on premium models. Claude sends focused changes
directly to the tool; the local Rust engine does the heavy lifting: matching,
validation, byte-preserving output, and durable writes.

[Install for Claude Code](#claude-code) ·
[Compare editing workflows](#why-use-it) ·
[Performance](#rust-engine-and-performance)

## Why use it?

| Capability | Claude Code's native Edit | Ultra Edit |
| --- | --- | --- |
| Target a change | Send exact old and new text | Use exact text, scoped matching, or a snapshot's span ID plus replacement text |
| Coordinate changes | File-specific replacement calls | One batch planned against the original snapshots of multiple files |
| Replace repeated text | Explicit `replace_all` | Explicit scope and expected occurrence count |
| File changed since reading | May proceed if the old text still matches uniquely | Reject the batch if any base is stale at preflight |
| Recover an edit | Claude Code session checkpoints | Persistent request receipts, retry identity, and conditional undo |

Native Edit is already a useful tool for isolated replacements; see its
[documented behavior](https://code.claude.com/docs/en/tools-reference#edit-tool-behavior)
and [checkpoint recovery](https://code.claude.com/docs/en/checkpointing).
Ultra Edit adds coordinated planning and explicit evidence. Planning errors
reject the whole batch, and every change targets the original source, so an
earlier replacement cannot accidentally become a later match. File writes are
sequential; a multi-file commit is not an atomic filesystem transaction.

A span edit avoids retransmitting the old block. Focused snapshots return only
the requested source, and direct MCP arguments avoid shell quoting and generated
editing code. These are concrete ways to reduce payloads compared with heredoc
rewrites; actual token and cost savings depend on the task, model, and retries.
Snapshots and tool instructions also cost context. No model-level token-savings
percentage or speed advantage over native Edit has been measured yet.

The routing rules are always-loaded **prompt guidance while the plugin and its
hooks are enabled**, not a Bash permission block. Hooks add context rather than
rewriting Claude's system prompt. See the
[canonical instructions](plugin/claude-code/instructions.md) and
[Claude Code hook behavior](https://code.claude.com/docs/en/hooks#add-context-for-claude).

## Rust engine and performance

The plugin's persistent MCP server and standalone JSON CLI share the same native
Rust engine. The server calls it directly, without launching a shell, script
interpreter, CLI process, or another model for each edit. Literal matching,
snapshot checks, output construction, and journaled persistence run locally.

Measured on Windows 11, a Ryzen 7 7800X3D, and a local NVMe SSD, using a release
build. These are the **ranges of median times from three runs**, each with three
warmups and 31 measured samples:

| Operation | Workload | Median time |
| --- | --- | --- |
| Plan a batch in memory | 64 exact changes across 8 files, 400 KB total | **4.7–5.6 ms** |
| Snapshot and commit a batch | 8 changes across 2 files, 20 KB total | **81–128 ms** |
| Read and persist a focused snapshot | 10 selected lines from a 5 MB, 100,000-line file | **217–245 ms** |

The file operations include normal hashing, persistence, and sync calls. Timings
exclude model latency, MCP transport, and CLI startup; the planning row also
excludes file I/O and snapshot creation. Disk timings varied across runs, so
these are local measurements, not a general speed guarantee.

The benchmark also exposed an avoidable allocation: focused reads used to create
line-reference strings for every line. The scanner now produces byte ranges and
creates references only for selected lines, avoiding **99,990 unnecessary line-ID
strings** in the 100,000-line fixture. Whole-file stale checks and persisted
original bytes remain intact. See [methodology and before/after results](docs/performance.md).

Reproduce the measurements in disposable temporary workspaces:

```text
cargo run --locked --release --example benchmark
```

## Run

Rust 1.89 or newer is required. The checked-in `Cargo.lock` fixes dependency
versions. From this directory:

```text
cargo build --locked
cargo run --locked --example walkthrough
cargo run --locked -- --help
```

The walkthrough edits disposable files in its own temporary directory and shows
preview, commit, retry, and undo. It checks the file bytes at each stage and does
not edit this project. Headings, grouped file changes, and red/green edits make
the terminal output easier to scan. Windows paths appear without doubled
separators or the extended path prefix; stored identities remain canonical.

Color is automatic on a terminal and disabled for redirected output or a nonempty
`NO_COLOR` environment variable. To override automatic detection:

```text
cargo run --locked --example walkthrough -- --color=always
cargo run --locked --example walkthrough -- --color=never
```

The JSON CLI and plain report functions remain free of ANSI sequences. Hosts can
opt into `report::terminal` for human output; report budgets count printable text
before styling.

## Claude Code

Build the plugin's private executables from this source directory:

```text
cargo build --locked --release
```

Create `plugin/claude-code/runtime` and copy both `ultra-edit` and `ultra-edit-mcp`
from `target/release` into it, using the `.exe` filenames on Windows. These
executables stay inside the plugin; no global command or `PATH` change is needed.
A prepared archive for the user's OS and architecture would include them and
require no Rust installation. Public release archives are not available yet.

For persistent use, copy the **entire prepared** `plugin/claude-code` directory to
`~/.claude/skills/ultra-edit` (`%USERPROFILE%\.claude\skills\ultra-edit` on Windows).
If `CLAUDE_CONFIG_DIR` is set, use its `skills/ultra-edit` directory instead.
Retain `.claude-plugin`, `.mcp.json`, `runtime`, `hooks`, `skills`, and the other
bundled files.
The [host setup guide](plugin/claude-code/skills/edit/references/claude-code.md#connect-locally)
has copy commands for Windows and macOS/Linux.

Then launch normally from **the workspace you want to edit**:

```text
claude
```

Claude Code discovers this as `ultra-edit@skills-dir` on startup, across projects.
The bundled personal installation was tested with Claude Code 2.1.263, with no
Ultra Edit executable on `PATH`. No marketplace is required; see the official
[skills-directory plugin documentation](https://code.claude.com/docs/en/plugins-reference#skills-directory-plugins).

For a session-only test without copying the plugin:

```text
claude --plugin-dir /absolute/path/to/ultra-edit/plugin/claude-code
```

Quote a plugin path containing spaces. The plugin starts its own
`${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp` with separate arguments
`["--root", "${CLAUDE_PROJECT_DIR}"]`. Claude Code substitutes the plugin and
project roots; the native Windows launcher resolves the `.exe` filename. Hook
commands use the same plugin-local executable and separate argument arrays,
without a shell. The root is fixed for that server; it is not a model tool
argument. See the official
[Claude Code MCP documentation](https://code.claude.com/docs/en/mcp).

Use `/mcp` to check the server connection, then make an ordinary edit request.
The plugin automatically tells Claude to **ALWAYS use direct Ultra Edit MCP
calls for coordinated edits to two or more existing UTF-8 files**, and never
write file contents through Bash heredocs or inline editing scripts. Native
Write remains appropriate for new files or isolated full rewrites; native Edit
or Ultra Edit can handle isolated targeted edits. Required tools being unavailable
or denied is a blocker to report, not a reason to change editing routes.

The [canonical instructions](plugin/claude-code/instructions.md) load through
`SessionStart` hooks on startup, resume, clear, compaction, and fork, plus
`SubagentStart` for delegated work. No slash command is required;
`/ultra-edit:edit` loads optional workflow detail. This supplies prompt context,
not forced tool enforcement. The shell rule addresses a user-reported Bash
backslash-loss observation from 2026-09-05; this project has not established that
behavior for every Claude host.

Inspect the hook's exact context without starting a server or opening a workspace:

```text
/absolute/plugin/directory/runtime/ultra-edit-mcp --claude-context SessionStart
```

On Windows, use the installed `runtime/ultra-edit-mcp.exe` path with PowerShell's
`&` operator; the [host setup guide](plugin/claude-code/skills/edit/references/claude-code.md#automatic-routing-context)
has an example. The executable embeds these instructions at build time. Copied
plugins require manual updates: rebuild, stage the matching executables, update
the complete plugin copy, then start a new Claude session. `--plugin-dir` loads
the prepared plugin only for that session.

The normal flow is a focused `ultra_edit_snapshot`, a batch `ultra_edit`, and
outcome inspection; `ultra_edit_status` retrieves receipts and explicit evidence.
Native Read/Grep can guide exploration, but the edit's base must come from an
Ultra Edit snapshot. Preview/commit, repair, and conditional undo are separate
tools. Diff review, crash inspection, and explicit operator reconciliation are
also available through MCP.

The plugin supplies context hooks without permission grants and does not
inherit native Edit's per-path permission policy. Keep `.ultra-edit` local and
outside version control because it stores complete source history. See the
[complete workflow and routing index](plugin/claude-code/skills/edit/SKILL.md),
[Claude Code setup](plugin/claude-code/skills/edit/references/claude-code.md), and
[MCP adapter contract](docs/mcp-adapter.md).

## CLI

The workspace defaults to the current directory. Use the **same canonical root**
for every cooperating process. References, original bytes, candidates, requests,
and journals are persisted under that workspace's `.ultra-edit` directory.
The bare `ultra-edit` commands in this README assume a separately available CLI.
With the self-contained plugin, replace that name with the full installed
`runtime/ultra-edit` path (`runtime/ultra-edit.exe` on Windows, invoked with `&`
in PowerShell). The plugin does not add it to `PATH`.

```text
ultra-edit --root WORKSPACE read PATH [EXPECTED_BYTES]
ultra-edit --root WORKSPACE read-range PATH FIRST LAST
ultra-edit --root WORKSPACE search PATH QUERY [OFFSET [SNAPSHOT]]
ultra-edit --root WORKSPACE prepare < request.json
ultra-edit --root WORKSPACE edit < request.json
ultra-edit --root WORKSPACE commit PLAN
ultra-edit --root WORKSPACE retry PLAN NEW_REQUEST_ID
ultra-edit --root WORKSPACE repair < corrections.json
ultra-edit --root WORKSPACE receipt REQUEST_ID
ultra-edit --root WORKSPACE inspect PLAN
ultra-edit --root WORKSPACE reconcile < resolution.json
ultra-edit --root WORKSPACE get REFERENCE
ultra-edit --root WORKSPACE diff PLAN
ultra-edit --root WORKSPACE undo PLAN NEW_REQUEST_ID
ultra-edit --root WORKSPACE prune-snapshots OLDER_THAN_SECONDS [--apply]
```

The redirection examples use a POSIX shell. In PowerShell, pipe JSON through
`Get-Content -Raw request.json | ... prepare`. Use UTF-8 when piping Unicode.
The executable produced by `cargo build` is `target/debug/ultra-edit.exe` on
Windows and `target/debug/ultra-edit` on Unix.

`read` returns the complete, unnormalized text, a `snapshot` ID, a SHA-256 digest,
and snapshot-local span references. `r0` covers the whole file, including any
UTF-8 BOM. `r1`, `r2`, etc. cover individual line bodies, excluding the BOM and
CRLF/LF terminators. A trailing newline does not create another line reference.
An empty file has an empty `r1`, which permits insertion. Span offsets are UTF-8
byte offsets; the compiler validates their boundaries.

Full reads default to at most 24,000 source UTF-8 bytes and 400 lines. Larger
files return `READ_TOO_LARGE` before generating line references or serializing a
snapshot. Use a range or search, or deliberately pass the exact current source
byte count as `EXPECTED_BYTES` (MCP `selection.expected_bytes`) to permit a larger
response. A changed byte count returns `READ_SIZE_CHANGED`; the 16 MiB source
ceiling still applies. This override can produce a large response, including all
line references. Full, range, and search responses all use `snapshot`; stored
snapshot evidence and the Rust `Snapshot` type retain their internal `id` field.

To read a small region, use `read-range src/retry.rs 12 18`. Line numbers are
one-based and inclusive. It returns `snapshot`, `path`, `digest`, file totals,
the exact selected `text`, absolute byte `start`/`end`, and editable `spans`:
`r12` through `r18` for individual line bodies, plus `selection` for the entire
selected range. The text excludes the leading BOM and the last selected line's
terminator; original newlines **inside** the range are included unchanged.
Empty and BOM-only files have an empty line 1. Invalid ranges fail explicitly.
Focused reads allow at most 200 lines and 6,000 source Unicode characters. They
reject oversized selections instead of issuing references to clipped text. For
a very long line, search for its exact target or use the complete `read` command.

`search src/retry.rs RETRIES` performs case-sensitive literal search in one file.
The response contains `snapshot`, `path`, `digest`, the exact `query`, `offset`,
`next_offset`, `total_matches`, `omitted_matches`, and up to 20 `matches` in byte order. Each
match has an editable `span` (`m1`, `m2`, etc.), a one-based starting line number,
and `before`/`after` context fragments of at most 80 Unicode characters, stopping
at CR or LF. The exact target text of every match is `query`; fragments are only
context and have no span references. Overlapping matches are counted (`aa` has
two starting positions in `aaa`). Queries may contain literal newlines and
Unicode, and must contain 1–1,000 Unicode characters. No matches is a successful
search with an empty match list. Continue with the returned `next_offset` and
`snapshot`, for example `search src/retry.rs RETRIES 20 s_RETURNED_ID`. Offsets
count matches from zero; span IDs retain absolute ordinals (`m21`–`m40` on the
second page). `next_offset: null` marks the end. `omitted_matches` counts all
matches absent from this page, including earlier pages.

Supplying `SNAPSHOT` searches its immutable original bytes even if current file
contents have changed. The requested path must still resolve to that snapshot's
file; removed or redirected targets fail explicitly. Omitting `SNAPSHOT` captures
fresh bytes, so offsets alone do not guarantee continuity across external edits.
Each page creates its own snapshot with only its disclosed references; edits
still reject stale source bytes.

Use either response's `snapshot` as an edit request's `base`, then target a
returned span, or scope an exact search to `selection` or a returned line.
Focused snapshots retain the whole original file internally for byte
preservation and stale detection, but persist **only the disclosed references**:
there is no hidden `r0`, unshown line reference, or omitted match reference.
Unscoped exact replacements still search the complete original file; use an
explicit scope to restrict matching. `get SNAPSHOT` explicitly retrieves the full
stored source as evidence. Each read/search captures a new snapshot; references
remain valid across process restarts, and a change anywhere in the file makes
their original base stale. Combine changes to one file using one base snapshot.

An edit request uses snapshot IDs returned by `read`:

```json
{
  "request_id": "edit-42",
  "files": [{
    "base": "s_REPLACE_WITH_RETURNED_ID",
    "changes": [
      {
        "id": "retries",
        "target": { "kind": "exact", "old": "const retries = 2;" },
        "text": "const retries = 3;"
      },
      {
        "id": "delay",
        "target": { "kind": "span", "span": "r5", "expect": "const delayMs = 100;" },
        "text": "const delayMs = 250;"
      }
    ]
  }]
}
```

Every change ID is unique across the entire request. Available targets:

| Target | Contract |
| --- | --- |
| `{"kind":"exact","old":"text"}` | Exactly one occurrence in the original file. |
| `{"kind":"exact","old":"text","scope":"r5"}` | Exactly one occurrence contained in a disclosed span. |
| `{"kind":"all","old":"text","scope":"r0","expected":3}` | Explicit scope and exact positive count of non-overlapping, left-to-right replacements. |
| `{"kind":"span","span":"r5"}` | Replace that span with literal `text`. |
| `{"kind":"span","span":"r5","expect":"old line"}` | Replace only if the selected original bytes equal `expect` exactly. |

`scope` is a disclosed span ID such as `r5`, `selection`, or `m2`, never literal
source text. Obtain an arbitrary line region with a range snapshot and use its
`selection`; inline `{first,last}` scopes are not supported. Prefer `exact` or
add `span.expect` when a mistaken positional ID should fail instead of replacing
the wrong text. An expectation mismatch returns `EXPECTED_TEXT_MISMATCH`.

Empty exact search strings are rejected. `exact` ambiguity checks and search
count overlapping starts: `aa` occurs at two starts in `aaa`. `all` instead counts
and replaces non-overlapping matches from left to right: `aa` in `aaaa` requires
`expected: 2`, and eight spaces contain four replacements of `"  "`.
Overlapping replacements from different changes and
coincident insertions reject the entire batch. This initial compiler also rejects
insertions that touch either boundary of another replacement; combine them into
one change. Adjacent nonempty replacements are allowed.

Replacement strings are literal UTF-8, including `$`, backslashes, tabs, Unicode
forms, trailing spaces, and supplied newlines. There is **no newline conversion**:
write `\r\n` if newly inserted text should use CRLF. All undeclared bytes remain
identical. Invalid UTF-8 is rejected explicitly; UTF-16 and other encodings are
not decoded. No formatter, shell command, or model runs inside the engine.
Ready plans and receipts carry nonblocking `NUL_BYTE` and `MIXED_LINE_ENDINGS`
warnings when candidate output contains NUL or both CRLF and bare LF. Warnings
also report pre-existing conditions retained in the candidate; they do not alter
bytes or reject the request. Inspect them before committing a preview.

## Preview, repair, retry, and undo

`prepare` writes an immutable preview and returns a `p...` reference, or retains
the failed request and diagnostics under a `d...` reference. It changes no target
files. `edit` performs prepare and commit in one call.

`commit PLAN` applies the stored output only if every current base still matches.
A stale file rejects the complete preflight, even if an identical search string
now appears elsewhere. Snapshot identity is the canonical logical path plus exact
bytes; recreating a file at that same path with identical bytes is accepted.
Symlink retargeting to a different canonical path is rejected at commit.

Repair replaces changes by ID in their original positions, then recompiles the
whole batch against its original snapshots under a new request ID:

```json
{
  "reference": "d_REPLACE_WITH_RETURNED_ID",
  "request_id": "edit-43",
  "changes": [{
    "id": "retries",
    "target": { "kind": "exact", "old": "const retries = 1;" },
    "text": "const retries = 3;"
  }]
}
```

Corrections cannot add unknown IDs or change retained snapshots. A stale base
requires a fresh read and request. Repair closes after **any commit attempt**.
For an environmental preflight failure, fix the cause and use
`retry PLAN NEW_REQUEST_ID` (MCP `ultra_edit_retry`). New journals must provide
durable proof that preflight completed unsuccessfully with no write intent. It clones
the exact candidate and original bases into a new plan, then attempts that plan;
no re-snapshot or matching is performed. The old request, journal, and failed
receipt remain unchanged. Partial, uncertain, and interrupted failures are
ineligible. New journals also exclude post-preflight failures. Older journals
lack a phase marker: a completed journal with no write intent and recognized
preflight errors is accepted, including an indistinguishable single-file stale
recheck before writing. Absence of write intent proves no target mutation in
that legacy case. New source bytes still fail the usual stale checks.

Repeating an identical request ID returns its original preview, draft, or recorded
receipt, including across process restarts. Different arguments under that ID
fail. Repeating `prepare` or `repair` after a commit surfaces that receipt as well.
References are random and workspace-local; missing references fail explicitly.

`undo PLAN NEW_REQUEST_ID` restores the original bytes of confirmed committed
files only if they still equal the recorded candidate. It is another journaled,
idempotent batch. Newer work causes rejection. Partial receipts can undo their
confirmed files. An uncertain plan remains unavailable for automatic undo;
reconcile to permit fresh edits, then restore any desired original bytes through
a new, explicit edit. Reconciliation does not establish which uncertain writes
happened.

## Persistence and evidence

The filesystem adapter holds one workspace lock across an operation, validates
all targets, stages each candidate beside its target, syncs it, rechecks its base,
then uses the platform's [rename replacement](https://doc.rust-lang.org/std/fs/fn.rename.html).
Canonical path aliases and hardlink aliases cannot produce competing candidates
within one batch. Read-only files fail preflight.

The adapter appends checksummed, synced journal records before and after each
write. It stops after the first persistence failure. Receipts distinguish
`committed`, `not_committed`, `partial`, and `outcome_unknown`, with per-file
outcomes. Counts describe confirmed committed change IDs; replace-all occurrences
are separate regions of one change. `after_digest` identifies the **intended**
candidate and is evidence of actual output only for confirmed committed files.
The CLI's `validation: "not_requested"` means no external validation command was
run. MCP receipt responses omit this unconfigurable field. Run project checks
separately; a confirmed write is not evidence that tests passed.

A missing durable outcome after a write intent is `outcome_unknown`, even when
current bytes happen to match the candidate. Repeating that plan returns its
recorded/recovered outcome without writing. Any unresolved uncertain journal
blocks new mutations throughout the workspace. `JOURNAL_UNCERTAIN` errors carry the request
and plan references with explicit `outcome_unknown`; retrieve their evidence.
The operator workflow below records a resolution while retaining that uncertainty.

Compact reports default to 60 lines and 6,000 Unicode characters, including long
single lines. CLI diagnostics show at most six shortened entries. `get` retrieves
full plans/drafts/snapshots/inspections; `receipt` retrieves every file outcome.
`diff` returns a unified diff with changed regions, surrounding context, deletions,
and missing-final-newline markers. MCP `ultra_edit_diff` takes `{plan, offset?}`
and returns at most 6,000 Unicode characters with `next_offset` for continuation;
diff offsets count characters, whereas search offsets count matches. Its quoted
paths are for inspection; patch import is not implemented. Explicit evidence
commands are not subject to compact-report limits.

Diff computation has bounded work: at most 200,000 changed-middle line references,
edit distance 1,024, and 64 MiB of line-comparison bytes. If a bound is reached,
the diff shows the complete changed middle as one coarse hunk while retaining
common outer context. It remains complete but may include unchanged interior lines.

To review reclaimable snapshot storage, run `prune-snapshots 604800` for objects
older than seven days. The JSON result lists `eligible` snapshot IDs and their
stored-object `bytes`, `eligible_bytes`, and retained/removed snapshot counts;
these byte counts include serialization overhead. Add `--apply` to rescan and
remove eligible standalone snapshots under the workspace lock. Pruned standalone
references become unavailable. Retained plans, drafts, and inspections keep their
required snapshots, and request, receipt, journal, and resolution history stays.
Uncertain or invalid recovery evidence blocks pruning; it is not a shortcut for
resolving a failed edit.

Exit codes are `0` for successful reads/previews/commits/reconciliations, `2` for
rejected requests or command/output errors, and `3` for commits that are not fully confirmed.
Inspect JSON `commit` and `error.code`, not the exit code alone. If output is lost,
retrieve the receipt by request ID before attempting another mutation.

## Crash reconciliation

`inspect PLAN` captures an immutable `i...` inspection. It includes the complete
plan (original text and intended output), exact journal bytes and digest, the
historical receipt or its diagnostic, and current state for every target. UTF-8
evidence uses `{"encoding":"utf8","text":"..."}`; non-UTF-8 evidence uses
`{"encoding":"binary","bytes":[255,0]}`. Missing targets are recorded explicitly.
`get INSPECTION` retrieves the saved evidence after a restart. Inspection writes
only workspace state, never target files.

MCP `ultra_edit_inspect` accepts `{plan}` and returns a compact description with
the inspection reference. Retrieve complete inspection evidence using
`ultra_edit_status` with `query.kind: "evidence"` and that reference. After
reviewing the evidence, submit an explicit operator decision through
`ultra_edit_reconcile` or the CLI:

```json
{
  "inspection": "i_REPLACE_WITH_RETURNED_ID",
  "decision": "accept_current",
  "note": "Reviewed the journal and all current files; accept this state and prepare fresh edits."
}
```

```powershell
Get-Content -Raw resolution.json | ultra-edit --root WORKSPACE reconcile
```

`accept_current` acknowledges the inspected file states as the starting point for
further work. It makes **no claim about historical execution** and performs no
target writes. An operator note containing 1–1,000 Unicode characters and some
non-whitespace text is required. No automatic acceptance or replay occurs.

The engine holds the workspace coordinator lock, rechecks exact journal bytes
and every observed target state, and publishes one immutable resolution for that
plan using the checksummed, synced object store. Changed evidence produces
`STALE_INSPECTION`; inspect again before deciding. Torn or corrupt regular journals
can be acknowledged when their immutable plan remains available. The original
journal and receipt remain unchanged: an unknown outcome stays unknown, and a
corrupt historical receipt continues to return its error.

Once every uncertain plan has a valid resolution, new mutations are allowed and
still pass their usual base checks. Other unresolved journals keep the workspace
blocked. Repeating the exact reconciliation request returns the original record,
including after later edits or a process restart. A different request for an
already resolved plan returns `RECONCILIATION_CONFLICT`. `inspect PLAN` also shows
the recorded resolution, including its original request, when one exists.

Current files may be binary or safely absent, but redirected paths, directories,
unreadable files, or excessive data produce `unavailable` observations and prevent
acceptance. Each file and journal is limited to 16 MiB; current file bytes total at
most 64 MiB per inspection. Missing paths are checked through their nearest
existing ancestor to prevent accepting a redirected location. Inspection does not
make binary or missing files editable through the existing-file UTF-8 adapter.

Keep the plan, inspection, resolution, and original journal intact. Changed or
missing journal evidence after reconciliation blocks further mutation and cannot
reactivate the original plan. Missing or corrupt immutable state requires
restoring that evidence; reconciliation does not repair the state store. The
coordinator protects cooperating processes only, and observations do not exclude
arbitrary external writers. Conditional base checks still apply to every later edit.

## Initial limits

- The lock coordinates clients using the same workspace root. It does not exclude
  arbitrary external writers or coordinate overlapping workspace roots. A final
  byte check followed by rename is not a compare-and-swap operation.
- Single-file replacement visibility depends on the backing filesystem. Multiple
  file replacements are not an atomic transaction. Windows file and journal
  contents are synced for process-crash recovery; portable directory flushing is
  unavailable through `std`, so power-loss durability is not promised.
- File permissions are copied. Ownership, ACLs, extended attributes, timestamps,
  and hardlink relationships are not preserved by replacement. This first adapter
  targets ordinary source files; richer metadata preservation needs platform work.
- Workspace operations permit 64 files and 64 MiB of base text per batch; each file
  and candidate is limited to 16 MiB. The compiler permits 1,000 changes, 10,000
  resolved spans, and 16 MiB of inserted bytes per plan. It stops discovering
  conflicts after 128 overlap diagnostics with an explicit limit diagnostic.
  Exceeding a limit rejects the batch. CLI input is limited to 16 MiB.
- `.ultra-edit` retains complete source history. The `prune-snapshots` command
  previews old standalone snapshots that no retained plan,
  draft, or inspection needs; add `--apply` to delete the listed eligible objects.
  This is explicit maintenance, not automatic collection. Keep recovery evidence
  and request history intact; pruning does not delete them.
  Keep it local and exclude it from version control in workspaces you edit. The
  state directory is trusted local storage, not a security boundary against a
  hostile process running as the same user. New Unix state directories/files use
  owner-only permissions; Windows state inherits the workspace ACLs.

## Development

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --all-targets
cargo test --locked --all-targets
cargo test --locked --doc
```

Tests cover byte preservation, original-snapshot matching, overlapping ambiguity,
order independence, stale previews, repaired conflicts, durable retry identity,
conditional undo, path aliases, report limits, and injected persistence/journal
failures. Focused read/search tests cover disclosed references, Unicode and
line-ending boundaries, overlapping matches, and resource limits. CLI tests
launch separate processes to exercise persistent state. Reconciliation tests
cover interrupted and corrupt journals, changed observations, binary/missing
targets, retained historical outcomes, concurrent decisions, and restart recovery.
MCP process tests cover tool discovery, focused batch edits, stale bases, malformed
arguments and frames, buffered messages, root confinement, restart retries,
cancellation, and uncertain receipts.

`compiler.rs` is pure planning; `reading.rs` constructs focused source views;
`storage.rs` owns persistence and recovery; `storage/reconciliation.rs` captures
evidence and records operator resolutions; `workspace.rs` owns the
reference/request protocol; `report.rs` formats evidence;
`main.rs` is the thin CLI. `mcp.rs` declares the typed MCP tools and compact
results; `bin/ultra-edit-mcp.rs` runs the fixed-root stdio server. Use `Workspace`
for coordinated host integration; low-level `Storage` calls require the caller
to hold its coordinator lock.

Claude Code 2.1.263 passed an automatic-routing smoke test on 2026-09-07: the
`SessionStart` hook loaded the policy, and an ordinary request produced snapshots
and one two-file MCP edit without invoking the skill. Native Read, Edit, Write,
and Bash remained available. Exact output checks passed for literal backslashes,
regex and escape text, Windows paths, BOM, CRLF/LF, trailing spaces, and a missing
final newline. The earlier skill-driven smoke also verified receipt retrieval.
A separate persistent-install smoke confirmed discovery without `--plugin-dir`
and exact final bytes. Claude guessed span IDs incorrectly on its first batch,
then undid it and submitted a corrected batch; the exactly-one-edit assertion
failed. This demonstrates persistent routing and recovery, not reliable
first-attempt target selection. See the [validation details](docs/mcp-adapter.md#validation).
The next milestone is a broader evaluation of Claude Code edit tasks: correct
bytes, appropriate tool selection, stale-base handling, and recovery after lost
output. Use those results to improve the MCP contract and focused instructions
before adding more editing modes. Platform metadata support remains a separate
limitation to address when ordinary source-file replacement is insufficient.

File creation, deletion, rename, general editor adapters, contextual patches,
regex, semantic refactors, and rebasing are outside the current edit-only scope.
The [broader historical design](docs/ULTRA-EDIT.md) is retained for reference;
those features are not the next delivery milestones.

Workspace-wide search and arbitrary byte-range reads remain deferred until a
concrete workflow needs them. Single-file search now pages through all matches.
See [experiment follow-ups](docs/followups.md) for the report's resolved issues,
current limitations, and deferred extensions.
