# Contributing

## Setup

Source work requires [Git](https://git-scm.com/downloads) and
[Rust/Cargo 1.89 or newer](https://rust-lang.org/tools/install/). Keep the
checkout separate from the project Claude will edit:

```text
git clone https://github.com/subashc2023/ultra-edit.git ~/src/ultra-edit
cd ~/src/ultra-edit
```

To contribute through a fork, use `gh repo fork subashc2023/ultra-edit --clone`
instead. The [source-build guide](plugin/claude-code/skills/edit/references/claude-code.md#connect-a-source-build)
has Windows and Unix staging, persistent-copy, and session-only commands.

```text
cargo build --locked
cargo run --locked --example walkthrough
cargo run --locked -- --help
```

The walkthrough edits disposable files in its own temporary directory and shows
preview, commit, retry, and undo, checking the bytes at each stage. Color is
automatic on a terminal; pass `-- --color=always` or `-- --color=never` to
override it. The JSON CLI never emits ANSI sequences.

## Checks

Run the same gates as CI before sending a change:

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --doc
python -m unittest discover -s scripts/tests -v
```

## Layout

Tests cover byte preservation, original-snapshot matching, overlap and ambiguity
rules, stale bases, repair, derived request IDs and replay, conditional undo, path
aliases, near-miss candidates, range and search continuation, blob storage and
collection, report limits, injected persistence and journal failures, crash
reconciliation, the MCP transport, and the shell-write guard. CLI and MCP tests
launch separate processes to exercise persistent state.

`compiler.rs` is pure planning and `candidates.rs` finds near misses;
`reading.rs` builds focused views; `workspace.rs` owns the reference and request
protocol; `storage.rs` owns persistence, blobs, and recovery, with
`storage/reconciliation.rs` for operator resolutions; `report.rs` formats
evidence; `shell_guard.rs` classifies Bash commands, with `shell_guard/` for
PowerShell and the project scope; `main.rs` is the CLI;
`mcp.rs` declares the MCP tools; `bin/ultra-edit-mcp.rs` runs the stdio server and
the hook modes. Use `Workspace` for host integration; low-level `Storage` calls
require the caller to hold its coordinator lock.

## Evaluation

The [harness](eval/README.md) runs fixture tasks through native Claude Code, the
plugin, shell-edit routes, and third-party MCP edit servers, and scores exact
final bytes, first-try success, tool calls, and cost. The recorded rounds in
[performance](docs/performance.md#model-level-results) all ran on Linux; a
Windows round, locally or with the manual `Eval` workflow on `windows-2025`, is
the next one to run, since Windows is where Claude Code's Bash tool corrupts
command payloads. Earlier live checks are recorded under
[validation](docs/mcp-adapter.md#validation).

## Scope

File creation, deletion, rename, contextual patches, regex, semantic refactors,
and rebasing are outside the current scope. The
[historical design](docs/ULTRA-EDIT.md) and
[experiment follow-ups](docs/followups.md) record earlier decisions. The
tag-driven release process is documented in [Releasing](docs/releasing.md).
