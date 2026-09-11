# Targets and focused snapshots

## Select what to inspect

Call `ultra_edit_snapshot` with one existing `path` and one `selection`:

| Selection | Shape | Returned editable references |
| --- | --- | --- |
| Inclusive line range | `{"kind":"range","first":12,"last":18}` | `spans: ["r12..r18", "selection"]`, plus one `lines` entry per line |
| Literal search | `{"kind":"search","query":"RETRIES","offset":0}` | Returned match spans `m1`, `m2`, etc., each with its line number |
| Complete file | `{"kind":"full"}` | `spans: ["r0", "r1..r400"]`, where `r0` is all bytes and `r1`… are line bodies, plus `lines`; within the default full-read limits |

Range and full responses summarize their disclosed IDs in `spans`, collapsing
consecutive line IDs into `r12..r18`, and list every disclosed line body in
`lines` as `"r12 | const retries = 2;"`. A blank line is `"r14 | "`, and whole-file
`r0` has no listing entry. Choose `r{n}` from that listing instead of counting
newlines; `text` still holds the exact selected bytes, which is what `expect`
guards and multi-line `exact` targets copy. Byte offsets stay server-side: full
evidence retains byte offsets, retrieved with `ultra_edit_status` evidence.

Every snapshot response identifies its base in `snapshot`. Stored full evidence
retains the engine's internal `id`. Use only span IDs actually returned for that base. A focused
snapshot has no hidden `r0`, unshown line references, or omitted match references.
Retrieving full evidence does not add new span IDs to an existing snapshot.
Large full/evidence responses may be externalized or clipped by the host. Read
the returned output artifact before relying on source not yet inspected; an
issued snapshot proves the server captured bytes, not that the model saw them.
Full reads permit at most 24,000 source UTF-8 bytes and 400 lines by default.
`READ_TOO_LARGE` gives the current byte count and directs you to range/search.
Only deliberately request `{"kind":"full","expected_bytes":CURRENT_BYTE_COUNT}`
when the complete larger response is needed. The count must match exactly or
`READ_SIZE_CHANGED` is returned; the 16 MiB source ceiling still applies.

Ranges use one-based inclusive lines. Individual line spans exclude the leading
UTF-8 BOM and CRLF/LF terminators. `selection` includes internal original newlines
but excludes the last selected line's terminator. A trailing newline adds no
extra line reference. A blank line has a zero-width line span; an empty file has
zero-width `r0` and `r1`, and a BOM-only file has a zero-width `r1` after its BOM.
Any returned zero-width span permits insertion at that position.
Ranges allow at most 200 lines and 6,000 source Unicode characters. Oversized
selections fail rather than silently clipping editable text.

Search is case-sensitive and literal, with 1–1,000 Unicode characters in `query`.
It counts overlapping occurrences and returns at most 20 matches per page in
byte order. Match `before`/`after` fragments are context only; the exact editable
text is `query`. Continue with the returned `next_offset` and `snapshot`:

```json
{
  "path": "src/retry.rs",
  "selection": {
    "kind": "search",
    "query": "RETRIES",
    "offset": 20,
    "snapshot": "REPLACE_WITH_PREVIOUS_PAGE_SNAPSHOT"
  }
}
```

`offset` counts matches from zero, and `next_offset: null` marks the end.
Later pages retain absolute match IDs (`m21` etc.) and receive new snapshots that
also retain the earlier pages' match references and any line references of the
snapshot they continued. One `ultra_edit` request using the LAST page's snapshot
can therefore address every match disclosed while paging through it; a request
still allows only one entry per file. Continuing with a different `query` drops
the previous query's match references, whose ordinals no longer apply.
`omitted_matches` counts all matches outside the current page, including
preceding ones. A supplied snapshot keeps the original source fixed across
external file edits; omitting it reads fresh bytes. The requested path must still
resolve to the same file. `stale: true` means the file no longer matches that
retained source, so an edit against this snapshot is rejected with
`STALE_SNAPSHOT`: read again instead of editing. An empty match list is
successful but grants no target.

## Choose the replacement target

All examples are the `target` field of a change; replacement content goes in the
separate literal `text` field.

| Intent | Target | Requirement |
| --- | --- | --- |
| Replace a disclosed line, selection, or match | `{"kind":"span","span":"m1"}` | The span belongs to this base. |
| Guard a positional target with known content | `{"kind":"span","span":"m1","expect":"old text"}` | The selected original bytes equal `expect`, or `EXPECTED_TEXT_MISMATCH` rejects the batch. |
| Replace unique exact text inside an inspected range | `{"kind":"exact","old":"old text","scope":"selection"}` | Exactly one occurrence wholly inside that scope. |
| Replace unique exact text anywhere in the original file | `{"kind":"exact","old":"old text"}` | Exactly one occurrence in the entire file. |
| Replace every exact occurrence in a scope | `{"kind":"all","old":"old text","scope":"r0","expected":3}` | Explicit disclosed scope and exact positive count of non-overlapping replacements. |

Unscoped `exact` searches the complete original file even after a focused read.
Use a scope when the intended edit is limited to the inspected region. Empty
`old` is rejected with `EMPTY_TARGET`. To insert, target a returned zero-width
span with `expect: ""` where one exists. No zero-width target is synthesized
between adjacent nonblank lines. Line spans exclude their terminators, so to add
an `inserted` line between `first\r\nsecond`, replace `first` with
`first\r\ninserted` or replace `second` with `inserted\r\nsecond`. Use
`expect` to guard the restated original bytes. Never derive an edit from a
clipped context fragment.
`scope` is a span ID, never literal text. To select a multi-line region, first
read its range and use `selection`; inline `{first,last}` scopes are unsupported.

Because line spans exclude terminators, replacing a line span with `""` only
blanks that line; its terminator survives. To delete a line, read a range that
also covers the following line, then either replace the line plus its actual line
ending as exact text inside `selection` (the `selection` excludes only the LAST
selected line's terminator, so the deleted line must not be the last selected
line):

```json
{ "kind": "exact", "old": "gamma\n", "scope": "selection" }
```

or replace `selection` of the line and its neighbour with the surviving
neighbour's body alone. Use the file's own line ending: write `"gamma\r\n"` in
CRLF source.

Search and `exact` ambiguity counts include overlapping starts: `aa` occurs twice in `aaa`.
Ambiguous exact matches are errors. In `TARGET_AMBIGUOUS`, `actual` remains the
overlapping-start count; the message adds, in parentheses, the non-overlapping
count for a corresponding same-scope `{"kind":"all"}` target:
`found 6 overlapping starts
(4 non-overlapping)`. Select a disclosed match, narrow the scope, or deliberately
switch to `all` using that parenthesized value as `expected`, with the same `old`
and disclosed scope. An unscoped `exact` from a focused snapshot has no equivalent
`all` repair unless that snapshot discloses a span covering the intended region;
otherwise take a suitable snapshot and start a new request. Replace-all proceeds
left to right: `aa` in `aaaa` requires `expected: 2`, and eight spaces contain
four replacements of `"  "`. Separate changes can still conflict with these
regions.

Overlapping replacements and coincident insertions reject the complete batch.
An insertion touching either boundary of another replacement also conflicts in
this version; express the intended result as one replacement. Adjacent nonempty
replacements are allowed. Changing request order cannot make an overlapping
batch valid, because every target refers to the original snapshot.
