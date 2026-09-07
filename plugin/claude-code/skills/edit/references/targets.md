# Targets and focused snapshots

## Select what to inspect

Call `ultra_edit_snapshot` with one existing `path` and one `selection`:

| Selection | Shape | Returned editable references |
| --- | --- | --- |
| Inclusive line range | `{"kind":"range","first":12,"last":18}` | `r12` … `r18`, plus `selection` |
| Literal search | `{"kind":"search","query":"RETRIES"}` | Returned match spans `m1`, `m2`, etc. |
| Complete file | `{"kind":"full"}` | `r0` for all bytes, plus line-body spans `r1`, `r2`, etc. |

Range and search results identify their base in `snapshot`. Full reads identify
it in `id`. Use only span IDs actually returned for that base. A focused
snapshot has no hidden `r0`, unshown line references, or omitted match references.
Retrieving full evidence does not add new span IDs to an existing snapshot.
Large full/evidence responses may be externalized or clipped by the host. Read
the returned output artifact before relying on source not yet inspected; an
issued snapshot proves the server captured bytes, not that the model saw them.

Ranges use one-based inclusive lines. Individual line spans exclude the leading
UTF-8 BOM and CRLF/LF terminators. `selection` includes internal original newlines
but excludes the last selected line's terminator. A trailing newline adds no
extra line reference; empty and BOM-only files have an empty `r1` for insertion.
Ranges allow at most 200 lines and 6,000 source Unicode characters. Oversized
selections fail rather than silently clipping editable text.

Search is case-sensitive and literal, with 1–1,000 Unicode characters in `query`.
It counts overlapping occurrences and returns at most the first 20 matches in
byte order. Match `before`/`after` fragments are context only; the exact editable
text is `query`. If `omitted_matches` is nonzero, narrow the query or read a range
around the intended target. An empty match list is a successful search, not an
editable target.

## Choose the replacement target

All examples are the `target` field of a change; replacement content goes in the
separate literal `text` field.

| Intent | Target | Requirement |
| --- | --- | --- |
| Replace a disclosed line, selection, or match | `{"kind":"span","span":"m1"}` | The span belongs to this base. |
| Replace unique exact text inside an inspected range | `{"kind":"exact","old":"old text","scope":"selection"}` | Exactly one occurrence wholly inside that scope. |
| Replace unique exact text anywhere in the original file | `{"kind":"exact","old":"old text"}` | Exactly one occurrence in the entire file. |
| Replace every exact occurrence in a scope | `{"kind":"all","old":"old text","scope":"r0","expected":3}` | Explicit disclosed scope and exact positive count. |

Unscoped `exact` searches the complete original file even after a focused read.
Use a scope when the intended edit is limited to the inspected region. Empty
`old` is rejected. Never derive an edit from a clipped context fragment.

Counts include overlapping starting positions: `aa` occurs twice in `aaa`.
Ambiguous exact matches are errors; select a disclosed match or narrow the scope
after inspecting evidence. A replace-all with the right count can still fail
when its replacements overlap.

Overlapping replacements and coincident insertions reject the complete batch.
An insertion touching either boundary of another replacement also conflicts in
this version; express the intended result as one replacement. Adjacent nonempty
replacements are allowed. Changing request order cannot make an overlapping
batch valid, because every target refers to the original snapshot.
