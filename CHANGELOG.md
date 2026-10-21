# Changelog

All notable changes to Ultra Edit are documented here. Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-10-21

### Added

- An experimental `ULTRA_EDIT_CARD` environment variable selects a native-first
  variant of the plugin's card, for evaluation. `short` makes the `ultra_edit`
  description a 1.2 KB card that prefers native Edit and Write and keeps
  `ultra_edit` for shell writes, several files, and repeated text; `deferred`
  keeps the full card but leaves `ultra_edit` behind `ToolSearch`. Both change
  the `SessionStart` and `SubagentStart` context to prefer native edits. Unset
  or empty keeps the default card, and any other value is refused at startup.
  The eval's `ultra-edit-short` and `ultra-edit-deferred` arms set it. In a
  Windows round of all 23 tasks, 3 times each, both were exact in all 69 runs,
  as the default card was, but right on the first try in 54 and 61 against its
  66, and cost 1.11x (1.08-1.13) and 1.14x (1.11-1.17) as much, the second
  charged for the prompt-cache entries it read from `native` runs, so the
  plugin keeps the default card.

### Changed

- The eight rarely needed MCP tools are now actions of one deferred tool,
  `ultra_edit_plan`. A call sends `{"action": {"kind": …}}`, where `kind` is
  `prepare`, `diff`, `commit`, `repair`, `retry`, `status`, `inspect`, or
  `reconcile` and the other fields are the arguments the tool of that name
  took: `ultra_edit_commit`'s `{plan}` becomes
  `{"action": {"kind": "commit", "plan": …}}`. `ultra_edit`,
  `ultra_edit_snapshot`, `ultra_edit_undo`, and the CLI are unchanged. Claude
  Code names every deferred tool in each session's uncached context, a
  subagent's included: counted with the token-counting API on
  claude-sonnet-5-5 in Claude Code 2.1.306's listing, the ten names took 325
  tokens and the three left take 98, about $0.0006 of the $0.056 an Ultra Edit
  run cost in the fourteenth Windows round. In the fifteenth, the first on this
  build, every Ultra Edit run's first API call carried 225 to 230 fewer tokens
  than the same task and repetition in the fourteenth, while native's changed
  by at most 5, and Ultra Edit cost 0.99x (0.95-1.03) native's, against 1.01x
  (0.98-1.04). In the 897 Ultra Edit runs of the 21 eval rounds before this
  change, the eight were called three times, `repair` twice in one run and
  `commit` once. The plan tool is marked destructive and not
  read-only, so one permission rule covers all its actions, and Claude Code's
  plan mode holds back the receipt and diff reads that the read-only
  `ultra_edit_status` and `ultra_edit_diff` could make there. Diagnostics that
  named `ultra_edit_repair` now end `(ultra_edit_plan can repair just this
  change)`, and the eval counts a plan call as the tool its action replaced.
- `after` with empty `new` inserts one empty line, ended as any text there
  would be, as `after` already writes `new` followed by that line ending. It was
  `EMPTY_INSERTION`, which now refuses empty text only in an empty file, where
  text is written as given, and after a last line without a line ending, where
  it would only add one. In the five full Windows eval rounds before this
  change, all three `EMPTY_INSERTION` rejections of Ultra Edit runs were meant
  as one empty line. Replayed, the two with a text `expect` now commit and
  leave the file as the task expects, where each run took four more calls; the
  third also gave a blank `expect` and is still refused, as `LINE_GUARD_WEAK`.
- The plugin card states the line guard as the server checks it, 8 or more
  visible characters in the whole `expect` rather than in each part, so the
  text at one end of a range can carry a blank line at the other, and tells
  `after` to anchor on a text line, with blank lines in `new`. In those rounds,
  Ultra Edit runs sent a blank `expect` with `after` ten times, between
  functions or sections, and every one was refused, eight as
  `LINE_GUARD_WEAK`. A card showing `["return ok",""]` for `after` instead cut
  blank-line rejections in 20 runs of move-function-between-modules and
  replace-function-body to two, from ten in those tasks' 30 earlier runs, but
  five of its nine failed calls gave a `lines` range with blank ends an
  `expect` item that was neither its first nor its last line, as if the list
  held neighbours, as that example's does. With this card, two rounds of the
  same 20 runs gave such a list once in all, but blank `after` anchors were not
  fewer: 4 and 10 calls. Five of the 10 came from replace-function-body runs
  that inserted their changelog entry after the blank line below its
  subheading, with a blank `expect`, which the round of 4 had not done. A card
  that showed a second `after` example instead, anchored on a function's last
  line with the blank lines in `new`, also gave 10, so no wording tried moved
  blank anchors by more than rounds of one card vary. Each refusal names the
  lines that would guard it, and in the round of 10 on this card, the call
  after each one committed. In a full Windows round on this card, the five
  Ultra Edit runs that missed their first try, all in those two tasks, each
  sent a blank `after` anchor, and each next call committed. To stay within
  2,048 bytes, the card drops its heading and shortens its replay note.
- A change takes what callers add to a deletion: `delete: true`, or
  `delete_line: true`, beside the `"new":""` of an `old`, `span`, or `lines`
  change, which it repeats, and fields whose names begin with `_`, as JSON's
  conventional comments do, as notes. Neither is stored, so the request ID is
  unchanged. A null `new` deletes as `""` does, and is refused with `after`;
  any other field a change does not take is still refused. In the 17 eval
  rounds recorded before this change, all ten such refusals of Ultra Edit calls
  were a correct deletion of one line in move-function-between-modules with one
  addition (`delete` three times, `delete_line` once, `_` four times, `_note`
  once, and a null `new` once), and each cost its run a call. Replayed, all ten
  commit and leave every file they touch as the task expects. In the three
  rounds since, ten deletions came with an addition, `delete: true` four times,
  a null `new` four times, and `_note` twice, and all were taken; one with
  `delete: false` is refused, since it could mean an emptied line.
- When the last lines of a `lines` change's `expect` are found where its range
  does not end, the refusal offers the range that ends there as a condition,
  `use lines [a,b] if the range ends there`, rather than stating `the range is
  [a,b]`. Of the 14 such refusals in Ultra Edit runs of those rounds, 11 named a
  range that would have cut the wrong lines: the last `expect` item was not the
  range's last line but a line inside it, such as the one before a closing
  fence or parenthesis, or the neighbour below it.
- An MCP result whose `commit` is `committed` leaves out `report`, which only
  repeated its request and plan IDs, the plan again as the undo reference, its
  warnings, and each file's path, which `changed` lists after a new commit. A
  replayed commit's report is the replay notice alone. A result where some file
  did not commit keeps its report, as every CLI result does. In the thirteenth
  Windows round, Ultra Edit's tool results came to 9,036 bytes per run against
  native's 7,391, and the reports of committed `ultra_edit` results to 787 of
  them: 705 of an average committed result's 2,560 bytes. Replayed through this
  build, the round's 79 `ultra_edit` calls outside the concurrent-writer task
  keep their outcomes, and each committed result is its old one without that
  field. In the fourteenth round, the first on this build, committed
  `ultra_edit` results came to 1,958 bytes each, Ultra Edit's tool results to
  8,059 bytes per run against native's 7,651, and its later API calls wrote
  0.1k more prompt-cache tokens per run than native's, against 0.7k in the
  thirteenth. Its cost was 1.01x (0.98-1.04) native's, within the 1.00x to
  1.05x of the six rounds before.

### Fixed

- Eval: each arm's prompts start with a marker of their own, hashed from the
  invocation's marker and the arm's name, so arms whose tools and system prompt
  match no longer read each other's cached task prompts. In the twelfth Windows
  round, 39 `native` runs read a task prompt that an `ultra-edit-deferred` run
  had cached, which put `ultra-edit`'s cost at 1.08x `native`'s rather than
  1.03x.

## [0.5.0] - 2026-10-20

### Added

- `"all": true` on an `old` change replaces every non-overlapping occurrence
  of `old` that cuts no word, at least one, in the file or within `in`, as a
  rename needs, without counting them first. Occurrences inside longer names
  stay, and a nonblocking `WHOLE_WORDS_ONLY` warning names the first; with no
  whole-word occurrence the change is `TARGET_NOT_FOUND`. `count` and `all`
  exclude each other, and `all: false` is accepted and not stored. Plans store
  the target as `{"kind":"every",...}`, which earlier releases report as
  `STORE_CORRUPT`. In round 8 of the Windows eval, Ultra Edit cost 95% more than
  native edits on `rename-css-class` and 53% more on `rename-constant`: native
  runs rewrote the files with `sed -i`, an inline script, or a PowerShell
  rewrite, while Ultra Edit runs wrote one change per line, up to 6,514 output
  tokens. One call with an `"all": true` change per file now commits each
  task's expected output.
- On Windows, a path that names no file as given but is spelled as Git Bash
  prints it names the file it stands for: `/c/x` is `C:\x`, and `/tmp/x` is in
  the user's temporary directory. A path copied from `pwd` in Claude Code's
  Bash tool no longer fails with `TARGET_MISSING`.
- An `old` without `count` that occurs nowhere as written takes its one match
  where the file holds a no-break or thin space (U+00A0, U+2007, U+2009, or
  U+202F) for one of its ASCII spaces, since rendered text such as a task's
  prompt shows those as ordinary spaces. `new` is written with the file's
  spaces in the text it repeats from `old`, their common prefix and suffix, and
  a nonblocking `SPACES_ADAPTED` warning names the changes and spaces. A second
  such match, an adapted space in text `new` changes or that an inserted or
  deleted part could sit on either side of, an `old` that holds one of these
  spaces, `count`, and undo keep the previous behaviour. In the round-three
  Windows eval, one `unicode-quotes` run typed an ordinary space in
  `limit is 10 km` where the file holds U+00A0 and was rejected; replayed
  against the task's files, that first call now commits the expected output.
- A nonblocking `BLANK_LINES_JOINED` warning names the output lines where a
  change's edge has whitespace on both sides that now holds more blank lines in
  a row than the file had anywhere, as when an `old` takes the blank lines above
  a function but stops before its last line ending. Beside a deletion, the start
  or end of the file counts as whitespace. Bytes are still written as given, and
  the warnings follow each file's `WHITESPACE_EDGE` warnings, at most eight per
  file. One Windows eval run committed three blank lines where its file never
  had more than two, with no warning; replayed, that call now carries this one.
  No eval task's expected files hold a longer run of blank lines than its
  fixture files, so no single correct edit of one triggers it.
- A nonblocking `BLANK_LINES_DROPPED` warning names the output lines where a
  change that removes text leaves fewer blank lines in a row around an edge
  than the file had around both of its ends, as when an `old` takes the blank
  lines above a function and one of those below it. Blank lines at the start
  or end of the file separate nothing, so whitespace reaching either counts
  none, and a change that removes only whitespace, such as an insertion, is not
  judged. Bytes are still written as given, and the warnings follow each file's
  `BLANK_LINES_JOINED` warnings, at most eight per file. The Windows eval's
  ninth round had one wrong Ultra Edit result: a deletion in `decode.py` left
  one blank line where both sides had two, with no warning; replayed, that call
  now carries this one. Replaying the first nine rounds' 361 `ultra_edit` calls
  changes three other outcomes, each only by this warning on a call that left
  a blank line out, which its run then found and restored, once over three
  more calls. No eval task's correct edit, planned from its fixture, triggers
  it.
- A nonblocking `FINAL_LINE_BREAK_KEPT` warning flags a `lines` or `after`
  change whose `new` ends in a line feed, which is written as given, and names
  the text's last line, the blank lines after it, and the next line. Text that
  lists its lines joined by line feeds, as the session card's examples do, comes
  out one blank line short: `"x\n\n"` writes `x` and one blank line, not two.
  Text of blank lines alone is not flagged. Bytes are still written as given,
  and the warnings follow each file's `BLANK_LINES_JOINED` warnings, at most
  eight per file. Of the eight line targets in the Windows eval whose `new`
  ended in a line feed, seven were a blank line short of the task's layout and
  the eighth was a model's correction of one; one of the seven made the fifth
  round's only wrong result.
- A committed `ultra_edit` call returns `changed`: each committed file's edited
  lines as they now read, numbered as Read numbers them, with one line of
  context, within 40 lines of 160 characters (`omitted` counts the rest). In
  the benchmark's intent-level rounds the model ran `git diff` or Read after a
  multi-hunk batch in 5 and 6 of 11 Ultra Edit runs, against 2 of 12 native
  ones; the new line numbers also serve later `lines` and `after` targets.
  Replays, `ultra_edit_commit`, and receipts read later carry none. The session
  card says so. In the Rust API, `EditResult::Completed` gains a `changed`
  field.

### Changed

- Where `old` begins or ends with a name that joins words with a hyphen, as
  the CSS class `btn-primary` does, a hyphen continues the word at that end of
  a match, for the exact-once rule, `count`'s whole-word matches, and `all`
  alike: `btn-primary` is no longer whole inside `btn-primary-outline`,
  `--btn-primary-bg`, or `data-btn-primary-label`. A word `old` begins or ends
  with as context keeps hyphens as separators. Replayed with this rule, the 278
  recorded `ultra_edit` calls of the Windows eval's first eight rounds keep
  their outcomes, apart from parse errors that now list `all` among the
  accepted fields, but one: a `rename-css-class` call refused for three
  ambiguous `old`s now commits the bytes its run's corrected second call wrote.
- The Claude Code plugin starts the server with the new `--plugin-card` flag,
  which sends no server instructions, as `--no-instructions` does, and gives
  the plugin's routing card as the `ultra_edit` description. The session and
  subagent hooks now add only a short context that routes edits to
  `ultra_edit` and its description. Claude Code caches tool definitions across
  sessions but writes hook context anew in each: measured in the first API call
  of a Windows eval session with Claude Code 2.1.303, the plugin's uncached
  tokens fell from 1,184 to 489, and its cached ones rose from 919 to 1,554.
  `--no-instructions` alone keeps the generic description.
- A `lines` or `after` change whose lines no longer hold its `expect`, because
  the file changed since it was read or the lines were miscounted, now applies
  where `expect` holds instead of being refused: when `expect` has 8 or more
  visible characters, reaches the change's end as a required one must, and
  holds at exactly one place where the whole change fits, both parts of a
  `[first, last]` pair shifted together. A nonblocking `LINES_MOVED` warning
  names the lines given and the lines used, and the stored plan records the
  lines used. A shorter `expect`, one that holds at several places, parts that
  moved apart, and a moved change that would overlap another change in the
  request are still refused with `EXPECTED_TEXT_MISMATCH`. Six first-try
  rejections in the Windows eval were of this kind.
- An `old` without `count` whose `in` lines hold no occurrence, for the same
  reasons, now applies at the one occurrence an unrestricted `old` would take,
  its only one in the file or its only one that cuts no word, when `old` has 8
  or more visible characters. The same `LINES_MOVED` warning names the `in`
  lines given and the lines used, and the stored plan records the lines used.
  Shorter text, several such occurrences, a span `in`, and a moved change that
  would overlap another change in the request are still refused with
  `TARGET_NOT_FOUND`. In the sixth round of the Windows eval, one
  `edit-while-file-changes` run gave each `old` an `in` from its first Read, and
  the other writer then added four lines above them; replayed against the
  changed file, that rejected first call now commits the expected output.
- Matches inside longer names no longer get in the way of a rename: an `old`
  that occurs several times but exactly once as whole words takes that match,
  and a `count` that numbers only the whole-word matches replaces just those, so
  `MAX_RETRIES` is no longer refused because `MAX_RETRIES_PER_HOST` holds it
  too. A nonblocking `WHOLE_WORDS_ONLY` warning names the first match left
  inside a longer word. A count of every match still replaces them all, and a
  count that is neither now gives the whole-word number in its
  `EXPECTED_COUNT_MISMATCH` advice as well.
- `in` beside `lines` or `after` is accepted when it holds the lines the target
  names, as `count: 1` is accepted anywhere; it is not stored. Any other `in`
  there, and any `in` on a `span`, is still refused while parsing.
- The Claude Code session card adds the fixes for the first-try rejections seen
  in the Windows eval: prefer a short `old` to line numbers, take `count` from
  Grep, give `expect` whole lines with their indentation, and join a
  neighbouring line into a blank or short `expect` part.
- `LINE_GUARD_WEAK` names the fewest lines, by number, that would make the
  guard strong: which part of `[first, last]` to grow, which lines to give as
  an insertion's `expect`, or which neighbour to widen a range to. It never
  quotes their text, since the number it checks may be the stale one. The card
  lists a ```` ``` ```` fence among the weak guards; such guards were the most
  common first-try miss in the intent-level rounds.
- The `Eval` workflow can run a Windows round. It runs Claude Code 2.1.294, the
  build of the recorded rounds, as its native executable rather than npm's
  `claude.cmd`, from PowerShell rather than Git Bash; names Git Bash for Claude
  Code; makes `python3` run Python; and keeps temporary repositories off the
  runner's 8.3 short temp path. New inputs choose the arms, the number of
  sessions at once, `both` runners for the same round on Linux and Windows, and
  a `preflight-only` check that starts no model session. An evaluation that
  reaches its time limit still summarizes and uploads the runs that finished.
  Run locally on Windows, the harness also runs the native executable when
  `PATH` finds npm's `claude.cmd`.
- The third-party arms start on Windows: Node servers run as `node` with their
  entry script instead of npm's extensionless `.bin` scripts, the Python server
  from `{venv_bin}` (`venv/Scripts` on Windows) with `{exe}`, and Desktop
  Commander's `dc-home` isolation also sets `USERPROFILE`. `setup.sh` runs under
  Git Bash, and the Chrome warm-up starts the server as its arm does.

### Fixed

- Eval: every run disables the built-in `plugin-authoring@builtin` plugin,
  which loaded in some runs depending on a GrowthBook flag.
- Eval: every prompt starts with `<!-- MARKER -->`, 8 hex digits that each
  invocation draws afresh, so no run reads a task prompt another invocation
  cached. The tenth Windows round started 24 minutes after the ninth: the
  native arm's first repetitions read the ninth round's cached task prompts,
  while Ultra Edit's, whose tool description had changed, paid to write theirs,
  near 4% of Ultra Edit's cost over the round (1.01× native over all runs,
  0.99× over the other two repetitions).

## [0.4.0] - 2026-10-08

### Added

- Path mode: a file entry can name its file by `path` instead of a snapshot
  `base`, as native Edit does. The server reads the file's current bytes under
  the workspace lock as the base, so an ordinary edit needs no
  `ultra_edit_snapshot` call. Batches stay all-or-nothing across files, with
  receipts, replay, and undo. Spans other than `r0` still need a snapshot base
  (`SPAN_NEEDS_BASE`). Paths are normalized lexically before request IDs are
  derived, a rejected derived-ID path request is not bound so an identical
  resend is evaluated again, and replaying a committed path request whose files
  changed since warns `REPLAYED_FILE_CHANGED`. Stored plans and drafts keep the
  0.3.0 shape, a base per file.

- Shorthand changes: `{"old","new"}` with optional `"count":N` (replace exactly
  N occurrences, anywhere in the file unless restricted) and `"in"` (lines
  `[a,b]` or a span ID), `{"span","new","expect"}`, `{"lines":[a,b],"new",
  "expect"}`, and `{"after":n,"new","expect"}`; `text` is an alias of `new`. The
  verbose `{"target":{…},"text":…}` form is still accepted and remains the
  stored form, so every spelling of a change derives the same request ID and
  0.3.0 stores load unchanged. Malformed changes are refused while parsing with
  a message naming the accepted form.
- Line targets: `lines` replaces or deletes whole lines by the numbers native
  Read shows, and `after` inserts whole lines, so inserting or deleting a line
  no longer needs a neighbouring line restated. Replacement text without a final
  line feed keeps the replaced line's ending, a missing final newline stays
  missing, and the empty line Read shows after a final newline stands for the
  last line. `expect` on a line target is compared line by line, ignoring line
  endings. Unless the base disclosed the lines (a `path` file discloses none),
  it is required, must reach the range's last line, as every line or as
  `[first line, last line]`, and needs 8 or more visible characters or must
  match nowhere else in the file (`LINE_GUARD_REQUIRED`, `LINE_GUARD_WEAK`):
  short repeated guards such as a blank line or `}` are refused, while long
  repeated lines such as `port = 8080`, which line numbers exist to tell
  apart, are accepted. So a stale line number,
  including a line added inside the range, is rejected rather than applied,
  and the message gives the current range. A path that names no file but ends
  like a workspace file, as when a directory was left out, is answered with
  that file's path.
  New diagnostics: `LINE_OUT_OF_RANGE`, `EMPTY_INSERTION`, and an
  `OVERLAPPING_CHANGES` message naming the single `lines` change to send.
  `INSERT_REPEATS_LINE` refuses an `after` insertion whose leading lines
  restate the lines ending at the line it follows (8 or more visible characters
  in those lines, ignoring trailing whitespace): a replacement written as an
  insertion, which would leave those lines twice. It says to drop them from
  `new`, or to send a `lines` replacement when the following lines were copied
  as well; lines below the anchor that only look the same, such as a sibling
  method's decorator, are offered as possibly new. Among equal lines it names
  the longest run. It stays quiet when other changes in the request delete
  every restated and copied line whole; when they edit those lines in place,
  it says to drop them from `new`, and when they touch them any other way, it
  names one `lines` change covering every change touching or bordering them
  to send in place of all of those. It compares at most 200 lines above the anchor, reads each
  line once per file, and follows a copy below it to its end; every message
  fits 240 characters at 8-digit lines. One benchmark session sent such an
  insertion and committed the duplicate; none of the other 614 recorded
  insertions restated their anchor.
- `OLD_INSIDE_WORD`: an `old` without `count` whose match starts inside an
  ASCII word or number, such as `retries = 2` inside `max_retries = 20`, or,
  with text on one line, ends inside one, such as `timeout = 30` inside
  `timeout = 300` (also with blank lines around it), is refused, naming the
  word and its line. Text written without reading the file can otherwise match
  a longer name. `count`, even 1, still matches inside words on purpose; words
  with cased non-ASCII letters near the cut (`Hauptstraße`) and letters after a
  backslash escape are exempt, while Han, kana, and other uncased scripts
  separate words and a cut between digits always counts (`10` in `100µs`).
  Ambiguity advice names `in` lines only where a whole-word match is the only
  match, found by a linear scan of at most 10,000 matches. Replayed against each task's initial
  files, none of the 3,709 `ultra_edit` `old` strings in the benchmark
  transcripts that match once would be refused (3,546 on one line, 163 across
  lines); of 1,712 native Edit `old_string`s, one would: `once_with(` cut from
  `assert_called_once_with`.
- Line-ending adaptation: in a file whose every line ends in CRLF, `old`, `new`,
  and `expect` text holding LF but no CR is matched and written as CRLF, with an
  `EOL_ADAPTED` warning. Native Read hides the CR, so multi-line text copied
  from it never matched such a file before. Text holding a CR, changes whose
  `old` holds one (so CRLF can still be converted to LF), mixed files, and undo
  stay literal; an undo's draft can no longer be repaired, since a repair
  would plan new text instead of the recorded bytes.
- A request refused while parsing names the change, as in `file 2, change 3:
  give `new` or `text`, not both`, and duplicate keys in a verbose `target` are
  rejected as in 0.3.0.
- Span ranges `rA..rB` work as a `span` or `scope` when the base disclosed
  every line from A to B, such as the `r146..r150` a range read lists in
  `spans`. A range runs from the start of line A's body to the end of line B's,
  like `selection`, so `""` blanks the lines and `expect` is byte-exact. A
  reversed or partly undisclosed range is `UNKNOWN_SPAN`.
- A nonblocking `WHITESPACE_EDGE` warning quotes the resulting line when an
  `exact` or `all` change's `old` and `new` differ only in whitespace at an edge
  and that joins text (`unit_price * Decimal` written as `unit_price *Decimal`)
  or lands beside more whitespace. Bytes are still written as given. These
  warnings follow the plan's `NUL_BYTE` and `MIXED_LINE_ENDINGS` warnings, so
  compact responses keep showing those.
- The evaluation harness now also runs shell edits (`sed`, Python, `git apply`,
  each held to its method by a hook) and three third-party MCP edit servers,
  with parallel runs, spend caps, transient-error retries, and paired
  statistics, over 23 tasks. Eight are intent-level: they describe a change
  without quoting its lines. In `edit-while-file-changes`, another writer
  changes a file during the session; a task's `concurrent.json` drives that
  through `PostToolUse` and `Stop` hooks.

### Changed

- A snapshot range that runs past the file's last line now reads through it,
  as native Read does, instead of failing with `INVALID_LINE_RANGE`; a range
  that starts past the last line still fails. The benchmark saw five such
  failures, each costing a round trip.
- The plugin's session routing card is rewritten path-first and cut from 4,191
  to 1,802 bytes, and the server instructions from 1,431 to about 650
  characters. The edit skill now covers recovery and span targets; ordinary
  edits need only the card. The card now says an `old` edit needs no prior Read
  when the task or earlier output gives the exact text: a miss writes nothing
  and shows the closest text, and a match inside a longer word is refused. Line
  targets still take Read's numbers, and their guards should be lines with text.
- The plugin starts the server with the new `--no-instructions` flag: its session
  card already gives the routing, and the duplicate server instructions cost
  about 280 tokens on every model call. The edit tool's schema also drops
  property descriptions that its description already gives, from 2,927 to 2,209
  bytes. Other hosts still receive the instructions.
- `ultra_edit` carries `_meta: {"anthropic/alwaysLoad": true}`, so Claude Code
  loads its schema up front instead of behind a `ToolSearch` round trip, which
  the benchmark measured at about 0.9 extra API calls per editing session.
  `ultra_edit_snapshot`, which path mode makes unnecessary for ordinary edits,
  and the recovery tools stay deferred.
- Diagnostic messages, which are free text, now say more. `UNKNOWN_SPAN` lists
  what the base discloses (`lines 146-150, 1875-1879; selection = lines
  1875-1879`) and answers IDs of other shapes, such as `146-150`, with the shapes
  span IDs take. `TARGET_AMBIGUOUS` names the lines of the first five matches.
  `TARGET_NOT_FOUND` says when the best whitespace candidate differs from `old`
  only in CRLF line endings. Codes, fields, and the `spans` summary are unchanged.
- The shell guard now checks scripts. When Write or a shell command saves a
  script outside the project whose `#!` line or extension names its language,
  or a command runs a script it saved
  (`cat > /tmp/edit.py <<'EOF' … EOF; python3 /tmp/edit.py`, `source`, or by
  path), the guard denies it as `a python3 script that may write project files`
  when a write call's target is not a literal path outside the project. Scripts
  that write only such paths are allowed. The guard reads no files from disk,
  so downloaded installers, earlier scratch files, and a project's own tools
  are never judged. The `PreToolUse` matcher is now `Bash|PowerShell|Write`. In
  the benchmark, 4 of 14 guarded native sessions wrote project files past the
  guard by saving a script with Write and running it.
- A `lines` range that ends in blank lines may give, as the last part of
  `expect`, the text line just before them, as the plugin's guidance to prefer
  text lines over blank ones suggests. The range is applied as given. Before,
  `lines:[25,38]` with the text of line 37 was rejected and the diagnostic
  proposed `[25,37]`, which would have kept the blank line the model meant to
  delete; 3 benchmark sessions hit it.
- `count: 1` on a span, lines, or after change is accepted and ignored, since
  every target replaces one place; another count there is still an error.
- An insertion at either edge of a deletion no longer conflicts, since either
  order gives the same text: `{after:11}` with `{lines:[11,11],new:""}` puts
  the new lines where line 11 was. Insertions beside replaced text or inside a
  deleted range still conflict.
- Two changes whose targets partly overlap no longer reject the batch when
  both keep the shared bytes unchanged, as when two `old` anchors share a few
  bytes of context: each applies to its own side. Contained or rewritten
  overlaps are still rejected as `OVERLAPPING_CHANGES`.
- The guard resolves script and inline-code targets instead of requiring a
  bare literal. Constants, f-strings and template literals, `+`, `%`,
  `.format()`, `os.path.join` and `Path('/tmp') / name` with a known leading
  directory, `tempfile`, `os.tmpdir()`, `Path.home()`, `os.devnull`, pytest's
  `tmp_path`, and stream targets count as outside; arguments and parameters
  still count as inside. Saves are judged only in temporary directories, so
  Write can create another repository's sources or a `~/.claude` hook; a
  script saved anywhere is still judged when the same command runs it. Moves
  and copies into the project (`shutil.move`, `os.replace`, `fs.renameSync`)
  count as writes, a `#!` launcher such as `uv run --script` or `tsx` no longer
  hides a `.py` or `.ts` script, and `[IO.File]::WriteAllText` saves are judged
  like `Set-Content`. Inline code that writes only outside the project, such as
  `python3 -c "open('/tmp/x', 'w')"`, is allowed, as heredocs to `/tmp` already
  were. Bash variables assigned once (`OUT=/tmp/o.txt`, `t=$(mktemp)`) and
  targets with a known leading directory (`/tmp/out_$i.txt`) are outside. Each
  saved file is judged once, on its final text, so self-appending commands no
  longer take seconds.

## [0.3.0] - 2026-09-25

### Added

- When an `exact` or `all` target is not found, or a span's `expect` fails,
  diagnostics list up to three `candidates` (`exact`, `whitespace`, or `similar`)
  with line numbers and the exact current text to copy. A scoped target whose
  text lies outside its scope reports where it is. Only the first six failed
  targets of a request are searched. `similar` skips lines that only share a
  one-line target's shape: short targets need up to 85%, and below 80% a
  candidate must contain more than half of the target's content words.
- Range reads can continue a snapshot (`read-range PATH FIRST LAST SNAPSHOT`, MCP
  `range.snapshot`) and keep its spans, so one request can edit distant regions
  of a file. Range responses report `stale`.
- `request_id` and change `id` are optional; omitted IDs are derived from the
  request (`auto-…` and `"1.2"`). Retry still requires an explicit request ID.
- Responses mark a recorded result returned without a new attempt with
  `replayed: true`, and replayed reports start with a notice. CLI preparation
  output now includes `request_id`, so a derived ID can be used with `receipt`.
- A `PreToolUse` hook (`ultra-edit-mcp --claude-hook PreToolUse`, matched to
  `Bash|PowerShell`) denies Bash and PowerShell commands that write file content
  into the project through the shell, including here-strings, `Set-Content`,
  `Out-File`, `-replace` rewrites, and .NET write APIs. Writes certainly outside
  the project (`CLAUDE_PROJECT_DIR`, else the event's `cwd`), such as
  `$GITHUB_OUTPUT`, `/etc/hosts`, `~/.bashrc`, or temporary files, are allowed.
  Set `ULTRA_EDIT_SHELL_WRITES=allow` in Claude Code's environment to disable
  it. The hook allows anything it cannot parse, stops lexing a command after
  250,000 tokens and allows the rest, and exits with a non-blocking error when
  misconfigured rather than denying every command.
- An evaluation harness (`eval/`) compares native editing, native editing with
  the guard, and Ultra Edit on fixture tasks.
- A manual Eval workflow runs that harness on a GitHub-hosted Linux or Windows
  runner with an `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` repository
  secret, and redacts the secret from the uploaded results.
- `prune-snapshots` reports and removes unreferenced blobs (`reclaimable_blobs`,
  `reclaimable_blob_bytes`, `retained_blobs`, `removed_blobs`).

### Changed

- `.ultra-edit` stores each string of 4 KiB or more once, in `blobs/` by SHA-256.
  Re-reading an unchanged file writes only a small snapshot. State from 0.2.0
  loads without migration, but earlier versions cannot read state written by this
  one and fail with `STORE_CORRUPT`; do not mix versions on one workspace.
- `TARGET_ALIAS`, `DUPLICATE_TARGET_PATH`, and `EXPECTED_TEXT_MISMATCH` messages
  say how to recover; `EXPECTED_TEXT_MISMATCH` shows the span's actual text.
- The plugin instructions are shorter and cover the guard, derived IDs,
  continuation, and candidates.
- The engine contract moved from the README to `docs/reference.md`, and the
  README lists what routing edits through MCP gives up.
- Release packaging requires the shell guard hook, matched to Bash and
  PowerShell.

### Fixed

- The MCP server freed an answered request ID only after writing the reply, so a
  client that reused the ID immediately could be refused as a duplicate.
- The unwritable-state-directory test no longer fails when run as root.

## [0.2.0] - 2026-09-11

### Added

- Range and full reads return a `spans` summary and a `lines` listing
  (`"r12 | body"`), so models no longer count lines or read byte offsets.
- Committed file outcomes carry an `after` snapshot id; `after_digest` is now
  `intended_digest` (stored inspections still read the old name).
- Continued search pages keep the references disclosed by earlier pages, and
  report `stale: true` when the retained source no longer matches the file.
- New diagnostics `TARGET_MISSING`, `INVALID_PATH`, and `STATE_DIR_UNAVAILABLE`;
  compact diagnostics include `file`.
- Line deletion is documented with an example.

### Changed

- Install the marketplace from the repository with
  `claude plugin marketplace add subashc2023/ultra-edit`. Claude Code clones any
  `github.com` source it is given, so the previously documented release-asset URL
  could not be added. Releases now commit the same SHA-256-pinned catalog to
  `.claude-plugin/marketplace.json` on `main`.
- A failed replacement is reobserved under the workspace lock: an unchanged
  target records `REPLACEMENT_FAILED` and is retryable, visible candidate bytes
  record `committed`, and only the remaining case stays `outcome_unknown`.
- Uncertain plans are indexed in `.ultra-edit/uncertain`, so commits no longer
  parse every journal.
- `.ultra-edit` ignores itself with `.gitignore` and `CACHEDIR.TAG`; a drive root
  or a state directory is refused as a workspace.
- No-op edits leave the target file untouched.
- Size-limit diagnostics name both limits and suggest only routes that work.
- Documented the UNC and drive-letter root spelling rule.

### Fixed

- The MCP server answers pre-initialize requests with `-32002` instead of
  exiting, refuses duplicate in-flight request ids, rejects malformed
  `tools/call` envelopes with `-32602`, and answers accepted requests before
  exiting on end of input or a fatal frame.
- Whitespace-only request ids are rejected before anything is persisted.
- An unavailable base snapshot is reported once.
- Tool schemas require `first`, `last`, and `expected` to be at least 1.

## [0.1.0] - 2026-09-08

### Added

- Snapshot-based, verifiable edits across multiple existing UTF-8 files.
- Focused range and search reads, previews, durable receipts, repair, retry,
  conditional undo, and explicit recovery reconciliation.
- A local stdio MCP server and Claude Code plugin with automatic edit-routing
  context.
- Native release packages for Windows x64, Linux x64 and ARM64, and macOS Intel
  and Apple Silicon.
- A self-hosted Claude Code marketplace package with SHA-256 verification and
  GitHub build-provenance attestations.

[0.6.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.6.0
[0.5.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.5.0
[0.4.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.4.0
[0.3.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.3.0
[0.2.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.2.0
[0.1.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.1.0
