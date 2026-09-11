# Changelog

All notable changes to Ultra Edit are documented here. Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[0.2.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.2.0
[0.1.0]: https://github.com/subashc2023/ultra-edit/releases/tag/v0.1.0
