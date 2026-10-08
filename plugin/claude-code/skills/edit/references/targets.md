# Targets and focused snapshots

## Select what to inspect

Call `ultra_edit_snapshot` with one existing `path` and one `selection`:

| Selection | Shape | Returned editable references |
| --- | --- | --- |
| Inclusive line range | `{"kind":"range","first":12,"last":18}` | `spans: ["r12..r18", "selection"]`, plus one `lines` entry per line |
| Range continuing a snapshot | `{"kind":"range","first":40,"last":42,"snapshot":"s…"}` | The prior snapshot's references plus these lines, e.g. `spans: ["r12..r18", "r40..r42", "selection"]`; `lines` for this range only |
| Literal search | `{"kind":"search","query":"RETRIES","offset":0}` | Returned match spans `m1`, `m2`, etc., each with its line number |
| Complete file | `{"kind":"full"}` | `spans: ["r0", "r1..r400"]`, where `r0` is all bytes and `r1`… are line bodies, plus `lines`; within the default full-read limits |

Range and full responses summarize their disclosed IDs in `spans`, collapsing
consecutive line IDs into `r12..r18` (itself a usable span; see below), and list every disclosed line body in
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
Each range read allows at most 200 lines and 6,000 source Unicode characters.
Oversized selections fail rather than silently clipping editable text.

To edit distant regions of one file in one request, read range A, then read
range B with A's `snapshot` in its selection. The read uses the bytes A's
snapshot retained, even if the file changed since, and mints a new snapshot that
keeps every span A disclosed plus B's lines. Use B's snapshot as the file's
single base and target A's and B's `r{n}` IDs in one file entry. Its `spans`
summarizes every targetable ID in disclosure order, while `lines`, `text`,
`start`, and `end` describe only B. `selection` always covers the latest range.
Ranges and searches can continue each other's snapshots, and a range can
continue a full read. The path must still resolve to the snapshot's file.
`stale` is false for a fresh read; on a continuation, `true` means the file no
longer matches the retained bytes or cannot be read, so an edit against that
base is rejected with `STALE_SNAPSHOT`: read again instead of editing. Two
separately read snapshots of one file cannot share a request (`TARGET_ALIAS`);
without continuation, keep one base and target text with an unscoped exact `old`.

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

`offset` counts matches from zero, and `next_offset: null` marks the end. Like a
continued range, a later page reads the retained bytes, and its snapshot keeps
the references disclosed before it; match IDs stay absolute (`m21` etc.). One
`ultra_edit` request using the LAST page's snapshot can therefore address every
match paged through. Continuing with a different `query` drops the previous
query's match references, whose ordinals no longer apply. `omitted_matches`
counts all matches outside the current page, including preceding ones. Omitting
`snapshot` reads fresh bytes. An empty match list is successful but grants no
target.

## Choose the replacement target

Each change names one target and its literal replacement `new` (`text` is an
alias). A `path` file supports `old`, `count`, `in` with lines, `lines`, and
`after`; spans other than `r0` need a snapshot `base`.

| Intent | Change | Requirement |
| --- | --- | --- |
| Replace unique exact text anywhere in the file | `{"old":"a","new":"b"}` | Exactly one occurrence in the entire original file, on word boundaries. |
| Replace unique exact text within some lines | `{"old":"a","new":"b","in":[72,87]}` | Exactly one occurrence wholly inside lines 72-87. |
| Replace unique exact text inside an inspected span | `{"old":"a","new":"b","in":"selection"}` | The span belongs to this base; one occurrence inside it. |
| Replace every occurrence | `{"old":"a","new":"b","count":3}` | Exactly 3 non-overlapping occurrences in the file, or in `in`, inside words or not. |
| Replace or delete whole lines | `{"lines":[40,42],"expect":["fn load(cfg) {","}"],"new":"..."}` | Lines exist; `expect` matches their first and last lines. `""` deletes them. |
| Insert whole lines | `{"after":3,"expect":"use std::fs;","new":"use std::io;"}` | Line 3 exists; `expect` matches the lines ending at 3. `after:0` inserts at the top. |
| Replace a disclosed line, selection, or match | `{"span":"m1","expect":"old text","new":"b"}` | The span belongs to this base; `expect` equals its bytes exactly. |
| Replace a run of disclosed lines' bodies | `{"span":"r146..r150","expect":"…","new":"b"}` | This base disclosed every line from 146 to 150. |

`lines` takes `[first,last]`; `[12]`, `12`, and `"12-14"` are read as `[12,12]`
and `[12,14]`. Line numbers are the ones native Read shows; the empty line Read
shows after a final newline stands for the last line.

A line target replaces whole lines, terminators included. Text that does not end
in a line feed keeps the last replaced line's ending, so a CRLF file stays CRLF
and a file without a final newline still lacks one; text that ends in a line
feed is written as given. Deleting the last line of a file without a final
newline also removes the line ending before it, so the file still lacks one.
Inserted lines get the file's line ending the same way.

`expect` on `lines` and `after` is compared line by line, ignoring line endings.
For `after`, it gives the lines ending at `after`. For `lines`, a string gives
the first lines of the range, and `[first, last]` gives its first and last
lines (each may hold several lines). It is required unless the base disclosed
every addressed line (a `path` file discloses none), and then must reach the
range's last line, by giving every line or the `[first, last]` pair, and must
have 8 or more visible characters or match nowhere else in the file
(`LINE_GUARD_REQUIRED`, `LINE_GUARD_WEAK`), so a line added or removed inside
the range since your Read is caught. Repeated lines such as `port = 8080` are
what line numbers tell apart, so a long guard may repeat. Lines between the
first and last parts are not compared, and a blank last line pins the end only
weakly, so prefer lines with text as `[first, last]`: when the range ends in
blank lines, the last part may name the text line just before them. `after:0`
takes none. A mismatch is `EXPECTED_TEXT_MISMATCH`, and when the expected lines
occur elsewhere the message gives the current range. A line past the end is
`LINE_OUT_OF_RANGE`. On `span`, `expect` is byte-exact.

Unrestricted `old` searches the complete original file even after a focused
read. Empty `old` is rejected with `EMPTY_TARGET`; insert with `after` instead,
and give `after` nonempty text (`EMPTY_INSERTION`). `after` keeps its line, so
text whose leading lines restate the lines ending at it is refused
(`INSERT_REPEATS_LINE`, 8 or more visible characters in those lines) unless
other changes in the call replace each of them whole. Drop the restated lines
from `new`, or send the named `lines` replacement if the following lines of
`new` were copied from the file too; a `lines` replacement that gives lines
twice duplicates them on purpose. `in` takes lines or a span ID, never literal
text.

A single-line `old` must start and end on word boundaries: a match that begins
or ends inside an ASCII word or number, such as `retries = 2` inside
`max_retries = 20` or `2.3.1` inside `v2.3.1`, is `OLD_INSIDE_WORD`, since text
written without reading the file can match a longer name by accident. Extend
`old` to whole words, or add `"count":1` to replace part of a word on purpose.
Words with non-ASCII letters, escapes such as `\nRestart`, and multi-line `old`
are exempt.

A line range `rA..rB` works as a `span` or `in` when the base disclosed every
line from A to B, for instance across a range continued with another range. It
behaves like `selection` for those lines: from the start of line A's body to the
end of line B's, so B's terminator stays, `""` blanks the lines instead of
deleting them, and `expect` must equal the bodies joined by the file's own line
endings. Use `lines` to replace or delete whole lines. `r150..r146`, a range
with an undisclosed line, and shapes such as `146-150` or `L146` are
`UNKNOWN_SPAN`; its message lists what the base discloses, as in
`lines 146-150, 1875-1879; selection = lines 1875-1879`.

The verbose form `{"id":"…","target":{…},"text":"…"}` is still accepted and is
how requests are stored: `old` is `{"kind":"exact","old":…,"scope":…}`, `count`
is `{"kind":"all","old":…,"expected":N}`, `span` is
`{"kind":"span","span":…,"expect":…}`, `lines` is
`{"kind":"lines","lines":[a,b],"expect":…,"expect_last":…}`, and `after` is
`{"kind":"insert","after":n,"expect":…}`. Both spellings of one change derive
the same request ID.

Search and `exact` ambiguity counts include overlapping starts: `aa` occurs twice in `aaa`.
Ambiguous exact matches are errors. In `TARGET_AMBIGUOUS`, `actual` remains the
overlapping-start count; the message adds, in parentheses, the non-overlapping
count for a corresponding `count` change in the same scope, and the lines of
the first five starts: `found 6 overlapping starts (4 non-overlapping) at lines
41, 89, 137`. Add surrounding text, restrict `old` with `"in":[first,last]`, or
deliberately replace them all with that parenthesized value as `count`.
Replace-all proceeds left to right: `aa` in `aaaa` requires `count: 2`, and eight
spaces contain four replacements of `"  "`. Separate changes can still conflict with these
regions.

Overlapping replacements and coincident insertions reject the complete batch.
An insertion touching either boundary of another replacement also conflicts in
this version; express the intended result as one replacement. Adjacent nonempty
replacements are allowed. Changing request order cannot make an overlapping
batch valid, because every target refers to the original snapshot.

## When a target is not found

`TARGET_NOT_FOUND` on an `exact` or `all` target, and `EXPECTED_TEXT_MISMATCH` on
a span, can carry up to three `candidates` with `kind`, `line`, `end_line`,
`text`, and `similarity`. Lines are 1-based and inclusive, numbered like `r{n}`.
`text` is the exact current bytes; above 2,000 characters it is omitted, never
clipped, so read `line..end_line` instead. The first tier with results wins:

| `kind` | Found | Use |
| --- | --- | --- |
| `exact` | The unmet `expect` text elsewhere, or a scoped target's text outside its scope | For `expect`, copy `text` into `old`; for a scope, pick a scope that contains the line or drop it. |
| `whitespace` | Text equal after CRLF→LF, dropping trailing spaces/tabs, and collapsing space/tab runs | Copy `text` verbatim; it keeps the file's indentation, such as `"\tlet x = 1;"`. Write the replacement with the file's tabs and line endings. The message says when only CRLF line endings differ. |
| `similar` | Text at least 70% similar (`similarity` gives the percentage) | Confirm it is the intended region first; it can be a different line of similar shape. |

A scoped target first checks whether its exact text occurs elsewhere in the file,
since an off-by-one scope is a common miss; otherwise a missing target is searched
within its scope, or the whole file if unscoped. An `expect` mismatch searches
the whole snapshot, and its message quotes what the span holds. Send
`ultra_edit_repair` with the draft `reference`, replacing only the failed change
ID. Copied text may occur more than once; add a scope if the repair reports
`TARGET_AMBIGUOUS`. Ambiguous targets and count mismatches where something matched
get no candidates, and only the first six failed targets of a request are
searched.
