# ULTRA Edit

A Rust library and JSON CLI for editing immutable UTF-8 snapshots. This is
an early implementation of [the design](docs/ULTRA-EDIT.md), with a filesystem host
for existing files. It is not the complete first release described in that plan.

The compiler resolves every change against the original snapshot, rejects the
whole batch on planning errors, and constructs a candidate from original byte
slices. Commit persists that candidate after checking its original bases. It
never reruns a matcher against changed content.

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

## CLI

The workspace defaults to the current directory. Use the **same canonical root**
for every cooperating process. References, original bytes, candidates, requests,
and journals are persisted under that workspace's `.ultra-edit` directory.

```text
ultra-edit --root WORKSPACE read PATH
ultra-edit --root WORKSPACE prepare < request.json
ultra-edit --root WORKSPACE edit < request.json
ultra-edit --root WORKSPACE commit PLAN
ultra-edit --root WORKSPACE repair < corrections.json
ultra-edit --root WORKSPACE receipt REQUEST_ID
ultra-edit --root WORKSPACE get REFERENCE
ultra-edit --root WORKSPACE diff PLAN
ultra-edit --root WORKSPACE undo PLAN NEW_REQUEST_ID
```

The redirection examples use a POSIX shell. In PowerShell, pipe JSON through
`Get-Content -Raw request.json | ... prepare`. Use UTF-8 when piping Unicode.
The executable produced by `cargo build` is `target/debug/ultra-edit.exe` on
Windows and `target/debug/ultra-edit` on Unix.

`read` returns the complete, unnormalized text, a snapshot ID, a SHA-256 digest,
and snapshot-local span references. `r0` covers the whole file, including any
UTF-8 BOM. `r1`, `r2`, etc. cover individual line bodies, excluding the BOM and
CRLF/LF terminators. A trailing newline does not create another line reference.
An empty file has an empty `r1`, which permits insertion. Span offsets are UTF-8
byte offsets; the compiler validates their boundaries.

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
        "target": { "kind": "span", "span": "r5" },
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
| `{"kind":"all","old":"text","scope":"r0","expected":3}` | Explicit scope and exact positive count. |
| `{"kind":"span","span":"r5"}` | Replace that span with literal `text`. |

Empty exact search strings are rejected. Overlapping candidate occurrences are
counted: `aa` occurs at two starts in `aaa`. Overlapping replacements and
coincident insertions reject the entire batch. This initial compiler also rejects
insertions that touch either boundary of another replacement; combine them into
one change. Adjacent nonempty replacements are allowed.

Replacement strings are literal UTF-8, including `$`, backslashes, tabs, Unicode
forms, trailing spaces, and supplied newlines. There is **no newline conversion**:
write `\r\n` if newly inserted text should use CRLF. All undeclared bytes remain
identical. Invalid UTF-8 is rejected explicitly; UTF-16 and other encodings are
not decoded. No formatter, shell command, or model runs inside the engine.

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
requires a fresh read and request. The first milestone closes repair after **any
commit attempt**, including a failed preflight. Earlier closure avoids ambiguous
reuse; relaxing it for proven zero-write failures is a later protocol change.

Repeating an identical request ID returns its original preview, draft, or recorded
receipt, including across process restarts. Different arguments under that ID
fail. Repeating `prepare` or `repair` after a commit surfaces that receipt as well.
References are random and workspace-local; missing references fail explicitly.

`undo PLAN NEW_REQUEST_ID` restores the original bytes of confirmed committed
files only if they still equal the recorded candidate. It is another journaled,
idempotent batch. Newer work causes rejection. Partial receipts can undo their
confirmed files; an uncertain receipt must be reconciled first.

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
`validation: "not_requested"` means no external validation command was run.

A missing durable outcome after a write intent is `outcome_unknown`, even when
current bytes happen to match the candidate. Repeating that plan returns its
recorded/recovered outcome without writing. Any uncertain journal blocks new
mutations throughout the workspace. `JOURNAL_UNCERTAIN` errors carry the request
and plan references with explicit `outcome_unknown`; retrieve their evidence.
An operator reconciliation API is still pending, so retain the state and inspect
it before further work through this adapter.

Compact reports default to 60 lines and 6,000 Unicode characters, including long
single lines. CLI diagnostics show at most six shortened entries. `get` retrieves
full plans/drafts/snapshots; `receipt` retrieves every file outcome. `diff` returns
a complete before/after diff, including deletions and missing-final-newline markers.
The initial full diff uses one full-file replacement hunk, so unchanged lines also
appear on both sides. Its quoted paths are for inspection; patch import is not
implemented. Explicit evidence commands are not subject to compact-report limits.

Exit codes are `0` for successful reads/previews/commits, `2` for rejected requests
or command/output errors, and `3` for commits that are not fully confirmed.
Inspect JSON `commit` and `error.code`, not the exit code alone. If output is lost,
retrieve the receipt by request ID before attempting another mutation.

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
- `.ultra-edit` retains complete source history and has no garbage collection yet.
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
failures. CLI tests launch separate processes to exercise persistent state.

`compiler.rs` is pure planning;
`storage.rs` owns persistence and recovery; `workspace.rs` owns the
reference/request protocol; `report.rs` formats evidence;
`main.rs` is the thin CLI. Use `Workspace` for coordinated host integration;
low-level `Storage` calls require the caller to hold its coordinator lock.

Next milestones from the design are distinct file creation/deletion/rename
operations, an explicit reconciliation workflow, recovery candidates,
host/MCP/editor adapters, and platform metadata support. Contextual patches,
regex, semantic refactors, rebasing, and model-driven
benchmarks follow evidence from those integrations.

Focused line reads and literal search with editable references follow in the next
milestone; this version returns complete snapshots through `read`.
