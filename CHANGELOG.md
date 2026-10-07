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
  to under 2,048 bytes, and the server instructions from 1,431 to 613
  characters. The edit skill now covers recovery and span targets; ordinary
  edits need only the card.
- `ultra_edit_snapshot` and `ultra_edit` carry `_meta: {"anthropic/alwaysLoad":
  true}`, so Claude Code loads their schemas up front instead of behind a
  `ToolSearch` round trip, which the benchmark measured at about 0.9 extra API
  calls per editing session. Recovery tools stay deferred.
- Diagnostic messages, which are free text, now say more. `UNKNOWN_SPAN` lists
  what the base discloses (`lines 146-150, 1875-1879; selection = lines
  1875-1879`) and answers IDs of other shapes, such as `146-150`, with the shapes
  span IDs take. `TARGET_AMBIGUOUS` names the lines of the first five matches.
  `TARGET_NOT_FOUND` says when the best whitespace candidate differs from `old`
  only in CRLF line endings. Codes, fields, and the `spans` summary are unchanged.

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
