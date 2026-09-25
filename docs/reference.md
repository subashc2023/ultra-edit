# Ultra Edit reference

The complete CLI and engine contract: reads and references, edit requests and
targets, preview/repair/retry/undo, persistence and evidence, crash
reconciliation, and limits. The [README](../README.md) covers installation and
the Claude Code workflow; the plugin's
[skill references](../plugin/claude-code/skills/edit/SKILL.md) cover the MCP
tools a model uses.

## CLI

The workspace defaults to the current directory. Use the **same canonical root**
for every cooperating process. References, original bytes, candidates, requests,
and journals are persisted under that workspace's `.ultra-edit` directory.
The bare `ultra-edit` commands in this reference assume a separately available CLI.
With the self-contained plugin, replace that name with the full installed
`runtime/ultra-edit` path (`runtime/ultra-edit.exe` on Windows, invoked with `&`
in PowerShell). The plugin does not add it to `PATH`.

```text
ultra-edit --root WORKSPACE read PATH [EXPECTED_BYTES]
ultra-edit --root WORKSPACE read-range PATH FIRST LAST [SNAPSHOT]
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
ultra-edit --root WORKSPACE undo PLAN [NEW_REQUEST_ID]
ultra-edit --root WORKSPACE prune-snapshots OLDER_THAN_SECONDS [--apply]
```

The redirection examples use a POSIX shell. In PowerShell, pipe JSON through
`Get-Content -Raw request.json | ... prepare`. Use UTF-8 when piping Unicode.
The executable produced by `cargo build` is `target/debug/ultra-edit.exe` on
Windows and `target/debug/ultra-edit` on Unix.

`read` returns the complete, unnormalized text, a `snapshot` ID, a SHA-256 digest,
and snapshot-local span references. `spans` summarizes the disclosed reference
IDs, collapsing consecutive line IDs: `["r0", "r1..r400"]`, or `["r0", "r1"]` for
an empty file. `lines` lists each disclosed line body as `"r12 | const retries = 2;"`,
so a line can be chosen without counting newlines; whole-file `r0` has no listing
entry and a blank line reads `"r14 | "`. `r0` covers the whole file, including any
UTF-8 BOM. `r1`, `r2`, etc. cover individual line bodies, excluding the BOM and
CRLF/LF terminators. A trailing newline does not create another line reference.
A blank line body exposes a zero-width line span. An empty file has zero-width
`r0` and `r1`; a BOM-only file has a zero-width `r1` after the BOM. Any returned
zero-width span permits insertion at its position. Byte offsets stay server-side:
`get SNAPSHOT` evidence retains each span's UTF-8 byte `start`/`end`, and the
compiler validates their boundaries.

Full reads default to at most 24,000 source UTF-8 bytes and 400 lines. Larger
files return `READ_TOO_LARGE`, reporting both the byte count and the line count so
the exceeded limit is visible, before generating line references or serializing a
snapshot. Use a range or search, or deliberately pass the exact current source
byte count as `EXPECTED_BYTES` (MCP `selection.expected_bytes`) to permit a larger
response. A changed byte count returns `READ_SIZE_CHANGED`; the 16 MiB source
ceiling still applies. This override can produce a large response, including all
line references. Full, range, and search responses all use `snapshot`; stored
snapshot evidence and the Rust `Snapshot` type retain their internal `id` field.
On Windows, emitted filesystem `path` fields, diagnostic and warning `file`
fields, and diff headers use conventional drive or UNC display spelling. Normal
extended-length prefixes remain only in stored objects and internal path identity
checks. Other verbatim namespaces remain unchanged, and JSON still escapes
backslashes according to JSON syntax.

To read a small region, use `read-range src/retry.rs 12 18`. Line numbers are
one-based and inclusive. It returns `snapshot`, `path`, `digest`, file totals,
the exact selected `text`, absolute byte `start`/`end` of the selection, the
summarized editable `spans` — `["r12..r18", "selection"]`, where `r12`…`r18` are
individual line bodies and `selection` is the entire selected range — and a
`lines` listing such as `"r12 | const retries = 2;"`. Copy replacement bodies
from that listing or from `text`; `text` remains the exact selected bytes for
`expect` guards and multi-line `exact` targets. The text excludes the leading BOM
and the last selected line's terminator; original newlines **inside** the range
are included unchanged.
Empty and BOM-only files have an empty line 1. Invalid ranges fail explicitly.
Focused reads allow at most 200 lines and 6,000 source Unicode characters. They
reject oversized selections instead of issuing references to clipped text,
directing an oversized selection to a smaller range, a search, or an explicit
`EXPECTED_BYTES` full read rather than a plain full read that may also be over
its limits. For a very long line, search for its exact target.

Supplying `SNAPSHOT` continues a prior snapshot:
`read-range src/retry.rs 40 42 s_RETURNED_ID`. The read uses that snapshot's
retained original bytes even if the file has changed since, and mints a new
snapshot that keeps every span the continued one disclosed (line `r…`, `r0`, and
search `m…` IDs) plus the new range's line spans; the continued snapshot is
unchanged. `spans` summarizes every ID the new snapshot can target, in
disclosure order, collapsing line runs and listing `m` IDs individually:
reading lines 12–18 and then continuing with 40–42 gives
`["r12..r18", "r40..r42", "selection"]`. `lines`, `text`, `start`, and `end`
describe only the requested range, and `selection` always covers only the most
recent range; earlier line IDs stay targetable. A range can continue a search or
full-read snapshot, and a search that continues a range keeps its line IDs and
`selection`. Per-read limits are unchanged. The path must resolve to the
snapshot's file (`SNAPSHOT_PATH_MISMATCH`); a non-snapshot reference is
`INVALID_REFERENCE`, an unknown one `REFERENCE_NOT_FOUND`, and a removed file
`TARGET_MISSING`. Range responses report `stale`: false for a fresh read, and on
a continuation true when the current file differs from the retained bytes or
cannot be read. An edit against a stale base is rejected with `STALE_SNAPSHOT`,
so read again.

To edit distant regions of one file in one request, read range A, read range B
with A's snapshot, then use B's snapshot as the single base and target A's and
B's `r{n}` IDs (or `selection` for B) in one file entry. Two independently read
snapshots of the same file cannot share a request: `TARGET_ALIAS` and
`DUPLICATE_TARGET_PATH` require one entry and one base per file. Without
continuation, keep one base and target text with an unscoped exact `old`.

`search src/retry.rs RETRIES` performs case-sensitive literal search in one file.
The response contains `snapshot`, `path`, `digest`, the exact `query`, `offset`,
`stale`, `next_offset`, `total_matches`, `omitted_matches`, and up to 20 `matches`
in byte order. Each
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
A fresh search reports `stale: false`; a continued page reports `stale: true` once
the file no longer matches the retained source, meaning an edit against that
snapshot is rejected with `STALE_SNAPSHOT`, so read the file again instead.
Each page creates its own snapshot, which retains the references of the snapshot
it continued as well as its own page's matches. One edit request using the last
page's snapshot can therefore target every match disclosed while paging, within
the one-entry-per-file rule. Continuing with a different `query` drops the
previous query's match references, whose absolute ordinals no longer apply, and
keeps line references. Edits still reject stale source bytes.

Use either response's `snapshot` as an edit request's `base`, then target a
returned span, or scope an exact search to `selection` or a returned line.
Focused snapshots retain the whole original file internally for byte
preservation and stale detection, but persist **only the disclosed references**:
there is no hidden `r0`, unshown line reference, or omitted match reference.
Unscoped exact replacements still search the complete original file; use an
explicit scope to restrict matching. `get SNAPSHOT` explicitly retrieves the full
stored source as evidence. Every read, including a continuation, captures a new
snapshot; references remain valid across process restarts, and a change anywhere
in the file makes their original base stale. Combine changes to one file using
one base snapshot.

An edit request uses snapshot IDs returned by `read`:

```json
{
  "files": [{
    "base": "s_REPLACE_WITH_RETURNED_ID",
    "changes": [
      {
        "target": { "kind": "exact", "old": "const retries = 2;" },
        "text": "const retries = 3;"
      },
      {
        "target": { "kind": "span", "span": "r5", "expect": "const delayMs = 100;" },
        "text": "const delayMs = 250;"
      }
    ]
  }]
}
```

`request_id` and each change `id` are optional; these changes receive IDs `1.1`
and `1.2`. A missing or empty change ID becomes its 1-based `"{file}.{change}"`
position in the request. Explicit change IDs are kept and must be unique across
the request; a clash with a derived ID is `DUPLICATE_CHANGE_ID`. A missing or
empty `request_id` becomes `"auto-"` plus the first 32 lowercase hex characters
of SHA-256 over a per-operation domain separator (`ultra-edit:edit:v1\0`,
`ultra-edit:repair:v1\0`, or `ultra-edit:undo:v1\0`) followed by the canonical
JSON of the arguments: for `edit` and `prepare`, the `files` array after change
IDs are filled; for `repair`, `[reference, changes]`; for `undo`, the plan
reference. The same arguments give the same ID across processes, restarts, CLI,
and MCP. The ID is echoed as the response `request_id`, stored in plans, drafts,
and receipts, and accepted by `receipt`. Pass an explicit `request_id` to name a
request yourself: 1–256 UTF-8 bytes with no control characters, and not
whitespace-only (`INVALID_REQUEST_ID`). `retry` always requires one. Available
targets:

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

Empty exact search strings are rejected. Use a returned zero-width span with
`expect: ""` for insertion where one exists. Line snapshots do not synthesize a
zero-width target at every boundary. Because line spans exclude terminators, to
insert a new `inserted` line between `first\r\nsecond`, either replace `first`
with `first\r\ninserted` or replace `second` with `inserted\r\nsecond`,
preferably guarded by `span.expect`. The untouched line ending supplies the other
separator.

For the same reason, replacing a line span with `""` only blanks that line; its
terminator remains. To **delete** a line, read a range that also covers the
following line, then either match the line and its actual line ending as exact
text inside `selection` — `{"kind":"exact","old":"gamma\n","scope":"selection"}`,
using `"gamma\r\n"` in CRLF source — or replace the `selection` of that line and
its neighbour with the surviving neighbour's body alone. `selection` excludes only
the **last** selected line's terminator, so the deleted line must not be the last
line of the selection.

`exact` ambiguity checks and search count overlapping starts: `aa` occurs at two
starts in `aaa`. For `TARGET_AMBIGUOUS`, `actual` remains that overlapping-start
count; the message also gives the non-overlapping count required by a corresponding
same-scope `{"kind":"all"}` target, for example
`found 6 overlapping starts (4 non-overlapping)`. `all` counts and replaces those
non-overlapping matches from left to right: `aa` in `aaaa` requires `expected: 2`,
and eight spaces contain four replacements of `"  "`.
Use the parenthesized value only with the same `old` and disclosed scope. Because
`all` requires a scope, an unscoped `exact` from a focused snapshot may require a
new snapshot and request that disclose the intended region. Overlapping
replacements from different changes and
coincident insertions reject the entire batch. This initial compiler also rejects
insertions that touch either boundary of another replacement; combine them into
one change. Adjacent nonempty replacements are allowed.

A diagnostic can carry up to three near-miss `candidates`,
`{kind, line, end_line, text?, similarity?}`, for `TARGET_NOT_FOUND` on an
`exact` or `all` target, searching the change's scope or the whole stored file
if unscoped, and for `EXPECTED_TEXT_MISMATCH` on a span target, searching the
whole snapshot. That mismatch message quotes the span's actual text, as in
`Span holds "…", not expect; …`. `TARGET_AMBIGUOUS` and count mismatches where
something matched get none, and the key is absent when there are none. `line`
and `end_line` are 1-based and inclusive, numbered like `r{n}`. `text` is the
exact current bytes, omitted rather than clipped above 2,000 characters.
`similarity` is a percentage, present only for `similar`. The first tier with
results supplies the candidates:

- `exact`: an unmet expectation's text occurs literally elsewhere, or a scoped
  `exact`/`all` target's text occurs outside its scope. A wrong scope, such as an
  off-by-one line, is checked first, so it is never answered with a similar line
  inside the scope.
- `whitespace`: equal after CRLF becomes LF, spaces and tabs before line ends
  are dropped, other space/tab runs collapse to one space, and the needle's own
  leading and trailing spaces and tabs are trimmed. The region includes the
  file's matching leading and trailing whitespace, so a tab-indented line comes
  back as `"\tlet x = 1;"`.
- `similar`: at least 70% by edit distance on whitespace-squeezed text, with
  bounded work; skipped above 200,000 scope lines or 20,000 needle characters.
  Needles under 25 squeezed characters need more, up to 85% at 12, and below
  80% a one-line needle's candidate must also contain more than half of its
  content words (four or more characters, not numbers or common keywords). A
  line of the same shape, such as `return Err(Error::Timeout);` for
  `return Err(Error::NotFound);`, is therefore not suggested.

Only the first six failed targets of a request are searched, within 32 MiB of
scanned text, so a request with many failures cannot hold the workspace lock
for long; later failures get no candidates.
For `exact` and `whitespace` candidates, copy `text` verbatim into `old` and
write the replacement with the file's tabs and line endings. Confirm that a
`similar` candidate is the intended region first: it can be a structurally
similar but different line, most often for single-line targets. Then repair only
the failed change ID, or submit a new request. If `text` is absent, read
`line..end_line` first. Copied text may not be unique; add a scope if the retry
reports `TARGET_AMBIGUOUS`.

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
whole batch against its original snapshots under a new request ID, derived from
the reference and corrections when `request_id` is omitted. Each correction
still requires the `id` of the change it replaces (`INVALID_REPAIR` otherwise):

```json
{
  "reference": "d_REPLACE_WITH_RETURNED_ID",
  "changes": [{
    "id": "1.1",
    "target": { "kind": "exact", "old": "const retries = 1;" },
    "text": "const retries = 3;"
  }]
}
```

Corrections cannot add unknown IDs or change retained snapshots. A stale base
requires a fresh read and request. Repair closes after **any commit attempt**.
For an environmental failure, fix the cause and use
`retry PLAN NEW_REQUEST_ID` (MCP `ultra_edit_retry`). Retry always takes an
explicit new ID, because a derived one would replay the first attempt instead of
retrying. New journals must provide durable proof that no target was written:
either preflight completed unsuccessfully with no write intent, or every file
records a failed replacement the engine confirmed had left the original bytes and
file identity in place (`REPLACEMENT_FAILED`) or was never attempted. It clones
the exact candidate and original bases into a new plan, then attempts that plan;
no re-snapshot or matching is performed. The old request, journal, and failed
receipt remain unchanged. Partial, uncertain, and interrupted failures are
ineligible, as are staging failures and post-staging stale bases. Older journals
lack a phase marker: a completed journal with no write intent and recognized
preflight errors is accepted, including an indistinguishable single-file stale
recheck before writing. Absence of write intent proves no target mutation in
that legacy case. New source bytes still fail the usual stale checks.

Repeating a recorded request, whether under the same explicit ID or with the same
arguments under a derived one, returns its original preview, draft, or recorded
receipt, including across process restarts. The response carries
`"replayed": true` only when the request was already recorded and the call made
no new commit attempt; the field is omitted otherwise. This covers `edit`,
`prepare`, `repair`, `undo`, and `retry`, plus `commit` of a plan that already
has a receipt (including an interrupted one); `receipt` never sets it. `edit`
after `prepare` of the identical request commits the stored plan, so it is not a
replay; a second `edit` is. A replayed report begins "Replayed the recorded result;
nothing new was attempted. To try again, pass a new explicit request_id, or for
an edit, take fresh snapshots." Exit
codes are unchanged, and a replay never writes target files. To apply identical
content again, take fresh snapshots (a new base derives a new ID) or pass a new
explicit ID. Different arguments under an explicit ID fail with
`REQUEST_ID_REUSED`; a derived ID cannot produce that error. Repeating `prepare`
or `repair` after a commit surfaces that receipt as well. References are random
and workspace-local; missing references fail explicitly.

`undo PLAN [NEW_REQUEST_ID]` restores the original bytes of confirmed committed
files only if they still equal the recorded candidate. It is another journaled,
idempotent batch. Without `NEW_REQUEST_ID`, its ID is derived from the plan, so
repeating an undo replays its recorded result, including a stale rejection; to
try that undo again, pass a new explicit ID. Newer work causes rejection.
Partial receipts can undo their confirmed files. An uncertain plan remains
unavailable for automatic undo;
reconcile to permit fresh edits, then restore any desired original bytes through
a new, explicit edit. Reconciliation does not establish which uncertain writes
happened.

## Persistence and evidence

The filesystem adapter holds one workspace lock across an operation, validates
all targets, stages each candidate beside its target, syncs it, rechecks its base,
then uses the platform's [rename replacement](https://doc.rust-lang.org/std/fs/fn.rename.html).
Canonical path aliases and hardlink aliases cannot produce competing candidates
within one batch. Read-only files fail preflight. A candidate identical to the
current bytes is confirmed without staging or renaming, so an effectively empty
edit leaves file identity, timestamps, and explicit ACLs untouched.

The adapter appends checksummed, synced journal records before and after each
write. It stops after the first persistence failure. A replacement that reports an
error is reobserved under the same lock: the original bytes and file identity mean
`not_committed` with `REPLACEMENT_FAILED`, the candidate bytes mean `committed`, and
anything else stays `outcome_unknown`. Receipts distinguish
`committed`, `not_committed`, `partial`, and `outcome_unknown`, with per-file
outcomes. Counts describe confirmed committed change IDs; replace-all occurrences
are separate regions of one change. `intended_digest` identifies the **intended**
candidate and is evidence of actual output only for confirmed committed files.
Each committed file also carries `after`: a snapshot reference holding the bytes
that were written, with no disclosed spans, usable as the base of a follow-up
unscoped `exact` edit or search page without reading the file again.
The CLI's `validation: "not_requested"` means no external validation command was
run. MCP receipt responses omit this unconfigurable field. Run project checks
separately; a confirmed write is not evidence that tests passed.

A missing durable outcome after a write intent is `outcome_unknown`, even when
current bytes happen to match the candidate. Repeating that plan returns its
recorded/recovered outcome without writing. Any unresolved uncertain journal
blocks new mutations throughout the workspace. Each commit indexes its plan with an
empty marker file under `.ultra-edit/uncertain` before its first write and removes it
once a complete certain outcome is recorded, so later mutations read only the journals
still listed there instead of the whole history. Stale markers are resolved from their
journals automatically; do not add or delete them by hand.
`JOURNAL_UNCERTAIN` errors carry the request
and plan references with explicit `outcome_unknown`; retrieve their evidence.
The operator workflow below records a resolution while retaining that uncertainty.

Compact reports default to 60 lines and 6,000 Unicode characters, including long
single lines. CLI diagnostics show at most six shortened entries, each with its
near-miss candidates. `get` retrieves full plans/drafts/snapshots/inspections,
including every diagnostic's candidates; drafts stored before candidates existed
still load. `receipt` retrieves every file outcome.
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

Stored objects, such as snapshots, plans, drafts, inspections, and request
records, are JSON files under `.ultra-edit`. Each string of at least 4096 UTF-8
bytes in them, such as file content, is stored once as
`.ultra-edit/blobs/<sha256>`, named by its SHA-256 (for a snapshot, its
`digest`), and the object references it as `{"$blob":"…"}`. A blob is written
and synced before any object that references it. Journals never reference
blobs, and CLI and MCP outputs are unchanged. Re-reading an unchanged file writes
only a small (about 1 KB) snapshot object; a one-line edit of a large file adds
one blob for the new content plus small objects. Loading verifies each blob's
digest and UTF-8: a missing, altered, or non-UTF-8 blob yields `STORE_CORRUPT`
naming the blob, and restoring its exact bytes repairs every object that
references it. State written by 0.2.0 loads without migration, and both formats
coexist. A 0.2.0 binary reading a new object fails closed with `STORE_CORRUPT`,
and its pruning refuses, so do not mix an older plugin server or CLI with a newer
one on the same workspace.

To review reclaimable storage, run `prune-snapshots 604800` for snapshots older
than seven days. The JSON result lists `eligible` snapshot IDs and their
stored-object `bytes`, `eligible_bytes`, and `retained_snapshots` and
`removed_snapshots` counts; these count only snapshot object files, including
serialization overhead. `reclaimable_blobs` and `reclaimable_blob_bytes` count
the blobs no remaining object would reference once the eligible snapshots are
gone, including orphans of interrupted writes; blobs have no age threshold.
`retained_blobs` counts the rest. Add `--apply` to rescan and, under the
workspace lock, remove eligible standalone snapshots first and then reclaimable
blobs, reported in `removed_snapshots` and `removed_blobs`. Pruned standalone
references become unavailable. Retained plans, drafts, and inspections keep their
required snapshots, retained journals keep the committed-output snapshots their
receipts name, plans keep their base and output blobs, and request, receipt,
journal, and resolution history stays. Uncertain or invalid recovery evidence,
or a missing or damaged blob, blocks pruning before anything is deleted; pruning
is not a shortcut for resolving a failed edit.

Exit codes are `0` for successful reads/previews/commits/reconciliations, `2` for
rejected requests or command/output errors, and `3` for commits that are not fully confirmed.
Inspect JSON `commit` and `error.code`, not the exit code alone. If output is lost,
repeat the identical command, or retrieve the receipt by request ID, before
attempting another mutation.

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
- `.ultra-edit` retains complete source history, storing each string of at least
  4096 bytes once in `.ultra-edit/blobs`. The `prune-snapshots` command previews
  old standalone snapshots that no retained plan, draft, or inspection needs, and
  the blobs nothing would reference afterward; add `--apply` to delete them.
  This is explicit maintenance, not automatic collection. Keep recovery evidence
  and request history intact; pruning does not delete them.
  Opening a workspace also writes `.ultra-edit/.gitignore`, which ignores everything
  under it, and a `CACHEDIR.TAG` marker for backup tools. Both are created once and
  never overwritten, so local edits to them survive.
  Keep it local and exclude it from version control in workspaces you edit. The
  state directory is trusted local storage, not a security boundary against a
  hostile process running as the same user. New Unix state directories/files use
  owner-only permissions; Windows state inherits the workspace ACLs.
