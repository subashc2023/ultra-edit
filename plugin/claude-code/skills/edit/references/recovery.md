# Preview, repair, and recovery

## Preview before committing

When the task calls for a preview, send the normal edit request to
`ultra_edit_prepare`. Inspect the compact report and candidate warnings. Review
the unified diff with `ultra_edit_diff` using `{plan}`; follow its `next_offset`
in subsequent `{plan, offset}` calls for pages of at most 6,000 Unicode
characters. Diff offsets count characters, not search matches. Retrieve full
plan evidence through `ultra_edit_status` when needed. A ready preparation has a plan
`reference`; commit that exact stored candidate with:

```json
{ "plan": "REPLACE_WITH_RETURNED_PLAN" }
```

Send this to `ultra_edit_commit`. It rechecks every base and never reruns matching
against changed files. A failed preflight rejects the complete batch. A returned
prior receipt is a historical outcome, not a new uncommitted preview.

## Correct a rejected request

`ultra_edit_repair` replaces known changes by ID, retains the original snapshots,
and recompiles the entire request under a new ID:

```json
{
  "reference": "REPLACE_WITH_RETURNED_DRAFT_OR_PLAN",
  "request_id": "retry-limit-correction-1",
  "changes": [{
    "id": "retry-limit",
    "target": { "kind": "exact", "old": "const RETRIES: usize = 1;" },
    "text": "const RETRIES: usize = 3;"
  }]
}
```

Use inspected original text for corrections. Unknown change IDs cannot be added,
and a repair cannot replace the base snapshots. A successful repair is a preview;
commit its returned plan separately. Any commit attempt closes repair, including
a failed preflight. For changed source (`STALE_SNAPSHOT`), inspect the recorded
outcome and take fresh snapshots for a new request. A proven environmental
preflight failure has the separate retry route below.

## Lost response or retry

First call `ultra_edit_status` with
`{"query":{"kind":"receipt","request_id":"ORIGINAL_REQUEST_ID"}}`.
Keep that ID and its exact arguments until the outcome is understood. If no
receipt exists, an identical retry of the original operation can recover the
bound preview/draft or resume its first commit. `UNKNOWN_REQUEST` means the ID
was not recorded in this root; verify the configured root before retrying.

Never change arguments under a bound ID. Never invent a new ID merely to bypass
a timeout, recorded failure, or uncertain journal. Repeating a committed,
partially committed, or uncertain plan returns the stored/recovered result and
does not replay writes.

After correcting an environmental failure such as a read-only target, call
`ultra_edit_retry` with `{"plan":"FAILED_PLAN","request_id":"NEW_RETRY_ID"}`.
For new journals, this requires proof of completed failed preflight and no write
intent. It copies the original candidate and bases into
a new plan and attempts that plan, preserving the previous failure receipt.
The original `commit` remains idempotent. New source bytes still fail stale
checks. Partial, uncertain, or incomplete failures cannot use this route. New
journals also reject post-preflight failures. An identical retry of this retry
operation keeps its new ID.

Legacy journals lack the phase marker. A completed legacy journal with no write
intent and recognized preflight errors is accepted. A single-file stale recheck
after staging but before writing is indistinguishable from failed preflight in
that format; it is safe to retry because no write intent proves no target
mutation. New journals distinguish and reject this post-staging case.

## Partial or uncertain persistence

For `partial`, request
`{"query":{"kind":"receipt","request_id":"ORIGINAL_REQUEST_ID","full":true}}`
and inspect all file outcomes and the retained plan before
deciding which fresh edits are needed. For `outcome_unknown`, `JOURNAL_UNCERTAIN`,
or `JOURNAL_CORRUPT`, stop new mutations and retain the request and plan references.
Matching current bytes do not resolve historical uncertainty. Do not delete
state, replay under a fresh ID, or attempt automatic undo.

Crash inspection and reconciliation are available through MCP. Call
`ultra_edit_inspect` with `{plan}`, then retrieve the complete saved inspection
with `ultra_edit_status` and `query.kind: "evidence"`. Only after an explicit
operator decision based on that evidence, call `ultra_edit_reconcile` with
`{inspection, decision: "accept_current", note}`.

The same workflow is available through the CLI using the server's root.
The bare `ultra-edit` name below assumes a standalone CLI
is already available. With the self-contained plugin, replace it with the full
installed `runtime/ultra-edit` path (`runtime/ultra-edit.exe` on Windows, invoked
with PowerShell's `&` operator). The plugin does not install a global command or
add the runtime directory to `PATH`.

```text
ultra-edit --root WORKSPACE inspect PLAN
ultra-edit --root WORKSPACE get INSPECTION
ultra-edit --root WORKSPACE reconcile < resolution.json
```

The last line uses POSIX redirection; in PowerShell, pipe
`Get-Content -Raw resolution.json` into the command. An operator reviews the
journal and original/intended/current bytes, then may record `accept_current`
with an inspection reference and a nonblank note. Reconciliation changes no
target files, preserves the uncertain historical outcome, and permits fresh
edits only after all uncertain journals have valid resolutions. This decision
must not be inferred from an ordinary edit request. Consult the repository
README's crash-reconciliation procedure when performing this operator task.

## Conditional undo

When undo is requested, call `ultra_edit_undo`:

```json
{
  "plan": "REPLACE_WITH_COMMITTED_PLAN",
  "request_id": "undo-retry-limit-1"
}
```

Undo is another journaled batch with its own idempotency ID. It restores only
confirmed committed files and requires their current bytes to equal the recorded
candidate. Newer work causes rejection. A partial plan can undo its confirmed
files; an uncertain plan cannot be automatically undone. After reconciliation,
restore any desired original content through a fresh, explicit edit.
Repeating the same undo request ID returns its recorded outcome. Undoing the
same original plan again under a different ID fails if the current bytes no
longer match its recorded output; the diagnostic explains that the file may
already be undone or contain newer work. Inspect before an explicit restoration.
