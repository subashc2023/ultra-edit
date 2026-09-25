# ULTRA Edit

**A Rust-powered Claude Code plugin for coordinated, verifiable edits across
multiple existing UTF-8 files.**

Claude often edits files through Bash heredocs and inline Python instead of its
Edit tool, especially for changes that span several files. Claude Code's auto mode
encourages this: its system prompt says small, mechanical file changes can be
made with sed, heredocs, or short scripts instead of Read, Edit, or Write. On
Windows that route corrupts files. The Bash tool has been observed dropping
backslashes (one of each `\\` pair), `<`, `>`, `..`, and more from command
payloads before the shell parses them, so quoting does not help.

Ultra Edit gives Claude a multi-file edit tool that never touches a shell. Changes
travel as MCP JSON arguments to a local Rust engine, are checked against immutable
snapshots of the original files, and land with durable receipts. A hook blocks
file writes through Bash, and the plugin's guidance loads automatically for
sessions, resumes, compaction, and subagents.

## Quickstart

**Requires:** [Claude Code](https://code.claude.com/docs/en/terminal-guide)
2.1.224 or newer, installed and signed in. The release package includes the
native executables; Git, Rust, a source checkout, and a `PATH` change are not
needed.

### 1. Add the marketplace

```text
claude plugin marketplace add subashc2023/ultra-edit
```

### 2. Install Ultra Edit

```text
claude plugin install ultra-edit@ultra-edit
```

The repository's catalog pins the latest release's multi-platform plugin archive
by SHA-256. Claude Code downloads that archive and installs it in its versioned
plugin cache with the MCP server, context and shell-guard hooks, skill, licenses,
and native executables together.

### 3. Start Claude Code

Change to the separate project you want Claude to edit and start a **new**
session:

```text
claude
```

Claude discovers the plugin as `ultra-edit@ultra-edit`. Inside Claude Code,
run `/mcp` and confirm that the `ultra-edit` server is connected. Then make an
ordinary request that changes two existing UTF-8 files—for example, update a
setting in a source file and its README documentation.

To update later:

```text
claude plugin marketplace update ultra-edit
claude plugin update ultra-edit@ultra-edit
```

Restart Claude Code or run `/reload-plugins` after an update. See the
[detailed Claude Code setup](plugin/claude-code/skills/edit/references/claude-code.md#install-from-a-release)
for source builds, session-only loading, and troubleshooting.

## Why use it?

| Capability | Claude Code's native Edit | Ultra Edit |
| --- | --- | --- |
| Target a change | Send exact old and new text | Use exact text, scoped matching, or a snapshot's span ID plus replacement text |
| Coordinate changes | File-specific replacement calls | One batch planned against the original snapshots of multiple files |
| Replace repeated text | Explicit `replace_all` | Explicit scope and expected occurrence count |
| File changed since reading | May proceed if the old text still matches uniquely | Reject the batch if any base is stale at preflight |
| Old text almost matches | A single whitespace or indentation difference misses | Up to three near-miss candidates with their exact current text and lines |
| Recover an edit | Claude Code session checkpoints | Persistent request receipts, replay of identical requests, and conditional undo |

Native Edit is already a useful tool for isolated replacements; see its
[documented behavior](https://code.claude.com/docs/en/tools-reference#edit-tool-behavior)
and [checkpoint recovery](https://code.claude.com/docs/en/checkpointing).
Ultra Edit adds coordinated planning and explicit evidence. Planning errors
reject the whole batch, and every change targets the original source, so an
earlier replacement cannot accidentally become a later match. File writes are
sequential; a multi-file commit is not an atomic filesystem transaction.

A span edit avoids retransmitting the old block. Focused snapshots return only
the requested source, and direct MCP arguments avoid shell quoting and generated
editing code. These are concrete ways to reduce payloads compared with heredoc
rewrites; actual token and cost savings depend on the task, model, and retries.
Snapshots and tool instructions also cost context. No model-level token-savings
percentage or speed advantage over native Edit has been measured yet; the
[evaluation harness](eval/README.md) exists to measure exactly that.

### What you give up

Claude Code gives its own Edit and Write tools some integrations that an MCP
edit tool does not get:

- **`/rewind` checkpoints** track only Claude's file-editing tools, so Ultra Edit
  writes cannot be rewound there. Use `ultra_edit_undo` or git instead. See
  [checkpoint limitations](https://code.claude.com/docs/en/checkpointing#limitations).
- **Hooks matched on `Edit|Write`**, such as formatters and linters in
  `PostToolUse`, do not run for Ultra Edit writes. Add the Ultra Edit tool name,
  as listed by `/mcp`, to those matchers.
- **IDE diff review** in the VS Code and JetBrains integrations shows native edits
  only.
- **Per-path Edit permission rules**, such as `Edit(secrets/**)`, do not apply.
  Ultra Edit confines edits to its workspace root and relies on Claude Code's
  MCP tool permissions.

## How an edit works

Claude reads a focused snapshot, then sends every related change in one request.
A snapshot is an immutable copy of the file; the response lists editable span IDs
such as `r12` (line 12) or `m1` (a search match):

```json
{ "path": "src/retry.rs", "selection": { "kind": "range", "first": 10, "last": 14 } }
```

The edit names that snapshot as its base. Request and change IDs are optional;
the engine derives them from the request, so repeating an identical call returns
the recorded result with `"replayed": true` instead of writing twice.

```json
{
  "files": [{
    "base": "s_RETURNED_SNAPSHOT",
    "changes": [
      { "target": { "kind": "span", "span": "r12", "expect": "const retries = 2;" }, "text": "const retries = 3;" },
      { "target": { "kind": "exact", "old": "const delayMs = 100;" }, "text": "const delayMs = 250;" }
    ]
  }]
}
```

Every target resolves against the original bytes, and the whole batch is
rejected if any target is missing, ambiguous, overlapping, or stale. When a
target is not found or an `expect` guard fails, the diagnostic lists up to three
candidates with their exact current text and lines: the text outside a wrong
scope, a region that differs only in whitespace or line endings, or one at least
70% similar, so the model can copy text instead of guessing. To edit
distant parts of one file in one batch, a range read can continue an earlier
snapshot, keeping its spans. Receipts report `committed`, `partial`,
`not_committed`, or `outcome_unknown` per file; nothing outside the declared spans
changes, including line endings, BOMs, and trailing whitespace.

The [skill](plugin/claude-code/skills/edit/SKILL.md) and its references describe
the MCP tools; the [reference](docs/reference.md) covers the full CLI and engine
contract, persistence, crash reconciliation, and limits.

## Shell-write guard

The plugin registers a `PreToolUse` hook for Claude Code's Bash tool. It parses
each command and denies ones that write content embedded in the command into
files:

- heredocs and `echo`/`printf` output redirected or `tee`d into files;
- inline Python, Node, Perl, Ruby, PHP, or PowerShell code that calls file-write
  APIs;
- in-place editors such as `sed -i` and `perl -pi`;
- patches or edit JSON piped into `patch`, `git apply`, or `ultra-edit`.

The deny reason tells Claude to use Ultra Edit, native Edit, or Write instead.
Ordinary output redirection (`cargo test > log.txt`), `/dev/null`, and heredocs
passed to commands that don't write them to files (`git commit -F -`, Claude
Code's `git commit -m "$(cat <<'EOF' …)"` pattern) are allowed. The guard allows
anything it cannot parse and is not a sandbox: it misses dynamic commands,
redirects on grouped commands, rewrites through temporary files, scripts already
on disk, and Claude Code's PowerShell tool.

It also blocks legitimate `echo`/`printf` writes, such as appending to
`$GITHUB_OUTPUT`. To turn the guard off, set `ULTRA_EDIT_SHELL_WRITES=allow` in
Claude Code's environment, for example `"env": {"ULTRA_EDIT_SHELL_WRITES": "allow"}`
in `settings.json`. Setting it inside a Bash command has no effect.

## Performance

The plugin's persistent MCP server and standalone JSON CLI share one native Rust
engine. The server calls it directly, without launching a shell, script
interpreter, CLI process, or another model for each edit.

Measured with version 0.1.0 on Windows 11, a Ryzen 7 7800X3D, and a local NVMe SSD,
using a release build. These are the **ranges of median times from three runs**,
each with three warmups and 31 measured samples:

| Operation | Workload | Median time |
| --- | --- | --- |
| Plan a batch in memory | 64 exact changes across 8 files, 400 KB total | **4.7–5.6 ms** |
| Snapshot and commit a batch | 8 changes across 2 files, 20 KB total | **81–128 ms** |
| Read and persist a focused snapshot | 10 selected lines from a 5 MB, 100,000-line file | **217–245 ms** |

The file operations include normal hashing, persistence, and sync calls. Timings
exclude model latency, MCP transport, and CLI startup; the planning row also
excludes file I/O and snapshot creation.

`.ultra-edit` now stores large strings once, by SHA-256, so re-reading an
unchanged file writes only a small snapshot. In one measured session of six
focused reads of a 1 MB file and a two-file edit, state fell from 10.0 MB to
2.2 MB. See
[methodology and results](docs/performance.md), and reproduce the timings in
disposable workspaces:

```text
cargo run --locked --release --example benchmark
```

## Claude Code

The [Quickstart](#quickstart) installs a user-scoped plugin for all projects.
Each release keeps `.claude-plugin`, `.mcp.json`, `runtime`, `hooks`, `skills`,
licenses, and both executables in one versioned archive. The release workflow
validates the packaged plugin with Claude Code 2.1.263 before publishing it.

For a session-only source test, first follow the
[source-build staging instructions](plugin/claude-code/skills/edit/references/claude-code.md#connect-a-source-build).
Then, from the project Claude should edit, load that prepared directory:

```text
claude --plugin-dir /absolute/path/to/ultra-edit/plugin/claude-code
```

The plugin starts `${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp` with the
arguments `["--root", "${CLAUDE_PROJECT_DIR}"]`; hook commands run the same
executable with their own argument arrays, without a shell. The root is fixed for
that server and is not a tool argument.

The [canonical instructions](plugin/claude-code/instructions.md) load through
`SessionStart` hooks on startup, resume, clear, compaction, and fork, plus
`SubagentStart` for delegated work. They tell Claude to **always use Ultra Edit
for coordinated edits to two or more existing UTF-8 files**, to use Write for new
files and native Edit or Ultra Edit for isolated edits, and to report unavailable
tools instead of falling back to shell writes. No slash command is required;
`/ultra-edit:edit` loads optional workflow detail. Inspect the exact context
without starting a server:

```text
/absolute/plugin/directory/runtime/ultra-edit-mcp --claude-context SessionStart
```

On Windows, run `runtime/ultra-edit-mcp.exe` with PowerShell's `&` operator. The
executable embeds the instructions at build time, so copied source plugins need
rebuilt executables and a fresh session after updates. Keep `.ultra-edit` local
and out of version control; it stores source history and ignores itself with its
own `.gitignore`. See the [host setup guide](plugin/claude-code/skills/edit/references/claude-code.md)
and the [MCP adapter contract](docs/mcp-adapter.md).

## Development

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

Run the same gates as CI before sending a change:

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --doc
python -m unittest discover -s scripts/tests -v
```

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
evidence; `shell_guard.rs` classifies Bash commands; `main.rs` is the CLI;
`mcp.rs` declares the MCP tools; `bin/ultra-edit-mcp.rs` runs the stdio server and
the hook modes. Use `Workspace` for host integration; low-level `Storage` calls
require the caller to hold its coordinator lock.

The next milestone is the model-level evaluation: run the
[harness](eval/README.md) on Windows against native editing and native editing
with only the guard, then use the results to decide which tools and instructions
earn their context. Earlier live checks are recorded under
[validation](docs/mcp-adapter.md#validation); in one, Claude guessed span IDs
wrongly on its first batch and recovered with undo.

File creation, deletion, rename, contextual patches, regex, semantic refactors,
and rebasing are outside the current scope. The
[historical design](docs/ULTRA-EDIT.md) and
[experiment follow-ups](docs/followups.md) record earlier decisions. The
tag-driven release process is documented in [Releasing](docs/releasing.md).

## License

Ultra Edit is available under the [MIT License](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. Distributed binaries also
include notices for [Cargo dependencies](THIRD_PARTY_LICENSES.txt) and the
[Rust standard library](RUST_STANDARD_LIBRARY_LICENSES.html),
[Rust compiler-builtins](RUST_COMPILER_BUILTINS_LICENSE.txt), and its bundled
[libm code](RUST_LIBM_LICENSE.txt), plus the [musl C library](MUSL_COPYRIGHT),
[LLVM libunwind](LLVM_LIBUNWIND_LICENSE.txt), and
[LLVM compiler-rt](LLVM_COMPILER_RT_LICENSE.txt) licenses.
