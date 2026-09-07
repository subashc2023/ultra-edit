# ULTRA Edit

A pure Rust compiler for editing immutable UTF-8 snapshots, implementing the
first part of [the design](docs/ULTRA-EDIT.md).

`compiler::snapshot` records exact text, its digest, and line-body span references.
`compiler::compile` resolves every requested change against that original snapshot
and constructs a candidate from original byte slices. Exact targets require one
match; replace-all requires an explicit span and positive expected count; span
targets replace precisely the disclosed range.

The compiler rejects missing or ambiguous targets, invalid snapshot digests,
invalid UTF-8 span boundaries, overlapping edits, duplicate change IDs, and
resource-limit violations. Adjacent nonempty edits are allowed; insertions that
touch another replacement boundary must be combined into one change. Literal
replacement bytes preserve all undeclared content, including BOMs and newlines.

This milestone has no filesystem writes or CLI. Persistence and host integration
follow in subsequent commits.

## Development

Rust 1.89 or newer is required. Run:

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --all-targets
cargo test --locked --all-targets
cargo test --locked --doc
```
