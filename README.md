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
plugin cache with the MCP server, automatic context hooks, skill, licenses, and
native executables together.

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
| Recover an edit | Claude Code session checkpoints | Persistent request receipts, retry identity, and conditional undo |

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
percentage or speed advantage over native Edit has been measured yet.

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

The routing rules are always-loaded prompt guidance while the plugin is enabled;
hooks add context rather than rewriting Claude's system prompt. See the
[canonical instructions](plugin/claude-code/instructions.md) and
[Claude Code hook behavior](https://code.claude.com/docs/en/hooks#add-context-for-claude).

## Rust engine and performance

The plugin's persistent MCP server and standalone JSON CLI share the same native
Rust engine. The server calls it directly, without launching a shell, script
interpreter, CLI process, or another model for each edit. Literal matching,
snapshot checks, output construction, and journaled persistence run locally.

Measured on Windows 11, a Ryzen 7 7800X3D, and a local NVMe SSD, using a release
build. These are the **ranges of median times from three runs**, each with three
warmups and 31 measured samples:

| Operation | Workload | Median time |
| --- | --- | --- |
| Plan a batch in memory | 64 exact changes across 8 files, 400 KB total | **4.7–5.6 ms** |
| Snapshot and commit a batch | 8 changes across 2 files, 20 KB total | **81–128 ms** |
| Read and persist a focused snapshot | 10 selected lines from a 5 MB, 100,000-line file | **217–245 ms** |

The file operations include normal hashing, persistence, and sync calls. Timings
exclude model latency, MCP transport, and CLI startup; the planning row also
excludes file I/O and snapshot creation. Disk timings varied across runs, so
these are local measurements, not a general speed guarantee.

The benchmark also exposed an avoidable allocation: focused reads used to create
line-reference strings for every line. The scanner now produces byte ranges and
creates references only for selected lines, avoiding **99,990 unnecessary line-ID
strings** in the 100,000-line fixture. Whole-file stale checks and persisted
original bytes remain intact. See [methodology and before/after results](docs/performance.md).

Reproduce the measurements in disposable temporary workspaces:

```text
cargo run --locked --release --example benchmark
```

## Run

Rust 1.89 or newer is required. The checked-in `Cargo.lock` fixes dependency
versions. From this directory:

```text
cargo build --locked
cargo run --locked --example walkthrough
cargo run --locked -- --help
```

The walkthrough edits disposable files in its own temporary directory and shows
preview, commit, retry, and undo. It checks the file bytes at each stage and does
not edit this project. Headings, grouped file changes, and red/green edits make
the terminal output easier to scan. The walkthrough uses conventional Windows
display paths; the CLI and MCP response contract is described below.

Color is automatic on a terminal and disabled for redirected output or a nonempty
`NO_COLOR` environment variable. To override automatic detection:

```text
cargo run --locked --example walkthrough -- --color=always
cargo run --locked --example walkthrough -- --color=never
```

The JSON CLI and plain report functions remain free of ANSI sequences. Hosts can
opt into `report::terminal` for human output; report budgets count printable text
before styling.

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

Quote a plugin path containing spaces. The plugin starts its own
`${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp` with separate arguments
`["--root", "${CLAUDE_PROJECT_DIR}"]`. Claude Code substitutes the plugin and
project roots; the native Windows launcher resolves the `.exe` filename. Hook
commands use the same plugin-local executable and separate argument arrays,
without a shell. The root is fixed for that server; it is not a model tool
argument. See the official
[Claude Code MCP documentation](https://code.claude.com/docs/en/mcp).

Use `/mcp` to check the server connection, then make an ordinary edit request.
The plugin automatically tells Claude to **ALWAYS use direct Ultra Edit MCP
calls for coordinated edits to two or more existing UTF-8 files**, and never
write file contents through Bash heredocs or inline editing scripts. Native
Write remains appropriate for new files or isolated full rewrites; native Edit
or Ultra Edit can handle isolated targeted edits. Required tools being unavailable
or denied is a blocker to report, not a reason to change editing routes.

The [canonical instructions](plugin/claude-code/instructions.md) load through
`SessionStart` hooks on startup, resume, clear, compaction, and fork, plus
`SubagentStart` for delegated work. No slash command is required;
`/ultra-edit:edit` loads optional workflow detail. This supplies prompt context,
not forced tool enforcement. The shell rule addresses a user-reported Bash
backslash-loss observation from 2026-09-05; this project has not established that
behavior for every Claude host.

Inspect the hook's exact context without starting a server or opening a workspace:

```text
/absolute/plugin/directory/runtime/ultra-edit-mcp --claude-context SessionStart
```

On Windows, use the installed `runtime/ultra-edit-mcp.exe` path with PowerShell's
`&` operator; the [host setup guide](plugin/claude-code/skills/edit/references/claude-code.md#automatic-routing-context)
has an example. The executable embeds these instructions at build time. Copied
source plugins require manual updates: rebuild, stage the matching executables,
update the complete plugin copy, then start a new Claude session. Marketplace
installs use the update commands in [Quickstart](#quickstart). `--plugin-dir`
loads the prepared plugin only for that session.

The normal flow is a focused `ultra_edit_snapshot`, a batch `ultra_edit`, and
outcome inspection; `ultra_edit_status` retrieves receipts and explicit evidence.
Native Read/Grep can guide exploration, but the edit's base must come from an
Ultra Edit snapshot. Preview/commit, repair, and conditional undo are separate
tools. Diff review, crash inspection, and explicit operator reconciliation are
also available through MCP.

The plugin supplies context hooks without permission grants and does not
inherit native Edit's per-path permission policy. Keep `.ultra-edit` local and
outside version control because it stores complete source history. See the
[complete workflow and routing index](plugin/claude-code/skills/edit/SKILL.md),
[Claude Code setup](plugin/claude-code/skills/edit/references/claude-code.md), and
[MCP adapter contract](docs/mcp-adapter.md).

## Development

Source work requires [Git](https://git-scm.com/downloads) and
[Rust/Cargo 1.89 or newer](https://rust-lang.org/tools/install/). Keep the
checkout separate from the project Claude will edit:

```text
git clone https://github.com/subashc2023/ultra-edit.git ~/src/ultra-edit
cd ~/src/ultra-edit
```

To contribute through a fork, use `gh repo fork subashc2023/ultra-edit --clone`
instead; GitHub CLI configures your fork as `origin` and this repository as
`upstream`. The [source-build guide](plugin/claude-code/skills/edit/references/claude-code.md#connect-a-source-build)
has complete Windows and Unix staging, persistent-copy, and session-only commands.

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --all-targets
cargo test --locked --all-targets
cargo test --locked --doc
python -m unittest discover -s scripts/tests -v
```

Tests cover byte preservation, original-snapshot matching, overlapping ambiguity,
order independence, stale previews, repaired conflicts, durable retry identity,
conditional undo, path aliases, report limits, and injected persistence/journal
failures. Focused read/search tests cover disclosed references, Unicode and
line-ending boundaries, overlapping matches, and resource limits. CLI tests
launch separate processes to exercise persistent state. Reconciliation tests
cover interrupted and corrupt journals, changed observations, binary/missing
targets, retained historical outcomes, concurrent decisions, and restart recovery.
MCP process tests cover tool discovery, focused batch edits, stale bases, malformed
arguments and frames, buffered messages, root confinement, restart retries,
cancellation, and uncertain receipts.

`compiler.rs` is pure planning; `reading.rs` constructs focused source views;
`storage.rs` owns persistence and recovery; `storage/reconciliation.rs` captures
evidence and records operator resolutions; `workspace.rs` owns the
reference/request protocol; `report.rs` formats evidence;
`main.rs` is the thin CLI. `mcp.rs` declares the typed MCP tools and compact
results; `bin/ultra-edit-mcp.rs` runs the fixed-root stdio server. Use `Workspace`
for coordinated host integration; low-level `Storage` calls require the caller
to hold its coordinator lock.

Claude Code 2.1.263 passed an automatic-routing smoke test on 2026-09-07: the
`SessionStart` hook loaded the policy, and an ordinary request produced snapshots
and one two-file MCP edit without invoking the skill. Native Read, Edit, Write,
and Bash remained available. Exact output checks passed for literal backslashes,
regex and escape text, Windows paths, BOM, CRLF/LF, trailing spaces, and a missing
final newline. The earlier skill-driven smoke also verified receipt retrieval.
A separate persistent-install smoke confirmed discovery without `--plugin-dir`
and exact final bytes. Claude guessed span IDs incorrectly on its first batch,
then undid it and submitted a corrected batch; the exactly-one-edit assertion
failed. This demonstrates persistent routing and recovery, not reliable
first-attempt target selection. See the [validation details](docs/mcp-adapter.md#validation).
The next milestone is a broader evaluation of Claude Code edit tasks: correct
bytes, appropriate tool selection, stale-base handling, and recovery after lost
output. Use those results to improve the MCP contract and focused instructions
before adding more editing modes. Platform metadata support remains a separate
limitation to address when ordinary source-file replacement is insufficient.

File creation, deletion, rename, general editor adapters, contextual patches,
regex, semantic refactors, and rebasing are outside the current edit-only scope.
The [broader historical design](docs/ULTRA-EDIT.md) is retained for reference;
those features are not the next delivery milestones.

Workspace-wide search and arbitrary byte-range reads remain deferred until a
concrete workflow needs them. Single-file search now pages through all matches.
See [experiment follow-ups](docs/followups.md) for the report's resolved issues,
current limitations, and deferred extensions.

The tag-driven release process and its local gates are documented in
[Releasing](docs/releasing.md).

## License

Ultra Edit is available under the [MIT License](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. Distributed binaries also
include notices for [Cargo dependencies](THIRD_PARTY_LICENSES.txt) and the
[Rust standard library](RUST_STANDARD_LIBRARY_LICENSES.html),
[Rust compiler-builtins](RUST_COMPILER_BUILTINS_LICENSE.txt), and its bundled
[libm code](RUST_LIBM_LICENSE.txt), plus the [musl C library](MUSL_COPYRIGHT),
[LLVM libunwind](LLVM_LIBUNWIND_LICENSE.txt), and
[LLVM compiler-rt](LLVM_COMPILER_RT_LICENSE.txt) licenses.
