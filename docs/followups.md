# Experiment follow-ups

The reported failures and navigation gaps are addressed in the current source.
The changes preserve original-snapshot resolution, literal bytes, immutable
receipts, and explicit recovery decisions. This page records the outcome for
each item in the approximately 25-experiment report; it does not claim that an
older copied plugin installation has been updated. Rebuild and stage matching
executables before updating an installation.

## Reported issues

| Report item | Outcome and current contract |
| --- | --- |
| Malformed JSON kills the MCP server | Malformed JSON, including an unpaired surrogate escape, receives JSON-RPC `-32700` with `id: null`; subsequent messages continue. Invalid request envelopes receive `-32600`. Oversized frames, invalid UTF-8 framing, and stream I/O errors remain fatal with stderr and nonzero exit. A lost connection still requires receipt inspection for any in-flight mutation. |
| Replace-all cannot handle self-overlapping patterns | `all` counts and replaces non-overlapping matches from left to right. `"aa"` in `"aaaa"` has two replacements; eight spaces have four replacements of `"  "`. `expected` is that replacement count. Search and unique `exact` checks continue counting overlapping starts. In `TARGET_AMBIGUOUS`, `actual` remains the overlapping count and the message adds the non-overlapping count, such as `found 6 overlapping starts (4 non-overlapping)`. Use the parenthesized value only for the same `old` and disclosed scope in a `{"kind":"all"}` target. Conflicts between separate changes still reject the batch. |
| Full reads can flood context and allocate thousands of spans | Default full reads stop at 24,000 UTF-8 source bytes or 400 lines before building line spans or persisting a snapshot. `READ_TOO_LARGE` points to range/search and reports the byte count. An explicit exact `expected_bytes` permits a larger full response; a mismatched count returns `READ_SIZE_CHANGED`. The 16 MiB source ceiling remains. Full evidence retrieval is still an explicit potentially large operation. |
| A failed preflight requires rebuilding the plan | `retry PLAN NEW_REQUEST_ID` and `ultra_edit_retry` reuse the exact candidate and original bases after the environment is corrected. New journals must prove completed failed preflight and no write intent. A new plan and request preserve the original failure receipt. Existing `commit` replay stays idempotent; partial, unknown, and interrupted failures are ineligible. New journals also exclude post-preflight failures; older journals have the safe exception described below. |
| Diff replaces the whole file and recovery is unavailable over MCP | Unified diffs identify changed regions with context. `ultra_edit_diff` returns pages of at most 6,000 Unicode characters. `ultra_edit_inspect` returns a saved inspection reference; status retrieves its complete evidence. `ultra_edit_reconcile` records an explicit reviewed operator decision. None of these turns an ordinary edit request into permission to accept uncertainty. |
| Full uses `id`, range/search use `snapshot` | All CLI and MCP snapshot responses now use `snapshot`. The internal Rust snapshot and persisted evidence retain `id` for compatibility with existing stored plans and journals. |
| Windows extended-length prefixes leak into displayed paths | CLI and MCP filesystem `path` fields, diagnostic and warning `file` fields, and diff/report headers now use conventional drive or UNC display spelling. Persisted objects and identity checks retain canonical paths. Other verbatim namespaces remain unchanged, and JSON still escapes backslashes normally. |
| `scope` was mistaken for literal source text | Documentation and examples define it as a disclosed span ID: `r5`, `selection`, or `m2`. Read a line range and use its `selection` for a multi-line scope. Inline `{first,last}` scopes are deferred; add them only if obtaining an explicit range snapshot becomes a demonstrated workflow obstacle. |
| Search can disclose only matches 1–20 | Search accepts a zero-based match `offset` and returns `next_offset`. Supply the previous page's `snapshot` to use the same immutable source. Every page has only its own disclosed references, with absolute match IDs such as `m21`. Omitting the snapshot reads fresh bytes. The target path must still resolve to the original file. |
| Oversized nonexistent ranges report the size cap first | Range existence is checked before the 200-line disclosure limit. A request through line 900 of a two-line file reports `INVALID_LINE_RANGE` and the actual line count. Invalid ranges are not silently clamped. |
| A second undo gives ordinary-edit stale advice | Undo retains its recorded-after-bytes condition. Its stale diagnostic now explains that the file may already be undone or contain newer work, and directs inspection of the current file and original plan. Repeating the same undo request ID remains idempotent. |
| Every receipt says `validation: not_requested`, with no MCP way to configure it | MCP receipt responses omit the unconfigurable field. CLI and stored engine evidence retain the original contract. Project formatting, linting, type checks, and tests run outside the edit engine; confirmed persistence does not establish their success. |
| Prepare and commit mix region and change counts | Reports distinguish requested change IDs from resolved replacement regions. A replace-all is one change ID containing multiple replacement regions. Confirmed-change counts refer only to committed change IDs. |
| NUL can silently enter source | Candidate outputs containing NUL carry a nonblocking `NUL_BYTE` warning. Literal bytes remain intact, including deliberately supplied NUL. Existing NUL retained in the candidate is also reported. |
| Standalone snapshots accumulate forever | CLI `prune-snapshots OLDER_THAN_SECONDS` previews eligible old standalone snapshots; `--apply` explicitly removes them. Snapshots needed by retained plans, drafts, or inspections remain, as do requests, journals, and recovery evidence. There is no automatic collection or complete history-retention policy. |
| A mistaken span ID replaced the wrong line | A span target can include `expect` containing the intended original bytes. A mismatch rejects preparation with `EXPECTED_TEXT_MISMATCH`. Exact targets remain useful when reproducing a short unique old string is simpler. A bare span still intentionally replaces the disclosed position without a content assertion. |
| `EMPTY_TARGET` recommended an unavailable insertion span | Returned zero-width spans still permit insertion, including blank-line spans and the spans of an empty file. Line snapshots do not create zero-width spans between adjacent nonblank lines. To insert a line at such a boundary, replace the preceding body with original + line ending + insertion, or the following body with insertion + line ending + original; the diagnostic now gives both routes. |
| LF inserted into CRLF source had no warning | Candidate output containing both CRLF and bare LF carries a nonblocking `MIXED_LINE_ENDINGS` warning, including an existing mixture retained by the edit. No newline normalization is performed; replacements still require explicit `\r\n` when CRLF is intended. |
| Generic shell-editing instructions conflict with the plugin | Always-loaded instructions and host guidance now state that explicit user instructions and host permissions take precedence. Report contradictions and do not silently route around an explicit Ultra Edit request. Prompt guidance does not enforce tool permissions or guarantee that every model follows it. |

## Answers to the report's questions

Older journals lack the new `preflight_failed` phase marker. A completed legacy
journal with no write intent and recognized preflight error codes is eligible
for retry. For a single-file `STALE_SNAPSHOT`, those records cannot distinguish
failed preflight from a stale recheck after staging but before writing. That
legacy case is accepted because no write intent proves that no target mutation
occurred. New journals explicitly record successful preflight and reject this
post-staging case. Retries always retain the original candidate and recheck its
source bytes.

1. **Malformed input:** a malformed message is recoverable. Only framing or I/O
   failure ends the transport, unsuccessfully. Automatic host restart is not
   implemented by this server; reconnect through the host and inspect receipts
   after a fatal transport failure.
2. **Overlapping counts:** overlapping discovery remains intentional for search
   and `exact` ambiguity, but no longer defines replace-all cardinality. To
   replace paired indentation spaces, scope `all` to the intended range and use
   the count of non-overlapping pairs. `TARGET_AMBIGUOUS` includes that repair
   count in parentheses after the overlapping-start count.
3. **MCP review and recovery:** diff, inspect, and reconcile are exposed. Inspect
   first, retrieve complete evidence where needed, and reconcile only after an
   explicit operator decision. Current matching bytes do not prove historical
   writes succeeded.
4. **Retention:** pruning is manual and limited to unreachable standalone
   snapshots older than the selected threshold. Its byte counts measure stored
   snapshot objects, including serialization overhead. It refuses uncertain or
   invalid recovery evidence, and removed standalone references become unavailable.
   Automatic scheduling and removal
   of retained history are deferred until a concrete retention policy is chosen.
5. **Routing conflicts:** the plugin declares its place beneath explicit user
   instructions and host permissions and asks for conflicts to be reported.
   Neither the hooks nor the engine rewrites unrelated host guidance.

## Preserved guarantees and limits

The literal-fidelity, original-base, request-identity, alias, and stale-source
checks exercised by the report remain. Batch preflight can reject all targets
before any write; later persistence failures can still leave partial or unknown
outcomes. The workspace lock coordinates cooperating clients only. A final byte
check followed by replacement is not a filesystem compare-and-swap, so
"TOCTOU-proof" and multi-file atomicity are stronger claims than this adapter
makes. Keep per-file receipts and recovery evidence when assessing an outcome.

Focused reads still cap at 200 lines and 6,000 Unicode characters; the effective
number of readable lines depends on their length. Search pagination supplies a
bounded route to later targets. Highly divergent diffs can reach the documented
work limits and fall back to one complete changed-middle hunk; finer review of
those cases would require a differ with lower memory requirements.
Whole-workspace search, arbitrary byte-range
reads, automatic newline conversion, automatic reconciliation, and broad history
deletion remain outside this follow-up's scope.

The runnable regression coverage is in [reading tests](../tests/reading.rs),
[compiler tests](../tests/compiler.rs), [workspace tests](../tests/workspace.rs),
[storage tests](../src/storage.rs), [report tests](../tests/report.rs), and
[MCP process tests](../tests/mcp.rs). Run the configured gates from
[Development](../README.md#development); historical benchmark and Claude smoke
measurements remain documented separately and are not new measurements of these
follow-ups.
