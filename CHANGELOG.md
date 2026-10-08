# Changelog

All notable changes to Ultra Edit are documented here. Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[0.3.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.3.0
[0.2.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.2.0
[0.1.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.1.0
