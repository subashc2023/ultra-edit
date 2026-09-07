# Targets and focused snapshots

## Select what to inspect

Call `ultra_edit_snapshot` with one existing `path` and one `selection`:

| Selection | Shape | Returned editable references |
| --- | --- | --- |
| Inclusive line range | `{"kind":"range","first":12,"last":18}` | `r12` … `r18`, plus `selection` |
| Literal search | `{"kind":"search","query":"RETRIES","offset":0}` | Returned match spans `m1`, `m2`, etc. |
| Complete file | `{"kind":"full"}` | `r0` for all bytes, plus line-body spans `r1`, `r2`, etc., within the default full-read limits |

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
extra line reference; empty and BOM-only files have an empty `r1` for insertion.
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
Later pages retain absolute match IDs (`m21` etc.) and receive new snapshots
containing only that page's references. `omitted_matches` counts all matches
outside the current page, including preceding ones. A supplied snapshot keeps
the original source fixed across external file edits; omitting it reads fresh
bytes. The requested path must still resolve to the same file. Stale snapshots
remain ineligible for edits. An empty match list is successful but grants no target.

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
`old` is rejected. Never derive an edit from a clipped context fragment.
`scope` is a span ID, never literal text. To select a multi-line region, first
read its range and use `selection`; inline `{first,last}` scopes are unsupported.

Search and `exact` ambiguity counts include overlapping starts: `aa` occurs twice in `aaa`.
Ambiguous exact matches are errors; select a disclosed match or narrow the scope
after inspecting evidence. Replace-all instead counts non-overlapping matches
from left to right: `aa` in `aaaa` requires `expected: 2`, and eight spaces
contain four replacements of `"  "`. Its expected count equals the number of
replacement regions. Separate changes can still conflict with these regions.

Overlapping replacements and coincident insertions reject the complete batch.
An insertion touching either boundary of another replacement also conflicts in
this version; express the intended result as one replacement. Adjacent nonempty
replacements are allowed. Changing request order cannot make an overlapping
batch valid, because every target refers to the original snapshot.
