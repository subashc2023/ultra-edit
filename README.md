# Ultra Edit

**Checked, multi-file edits for Claude Code, without shell heredocs.**

[![Release](https://img.shields.io/github/v/release/subashc2023/ultra-edit)](https://github.com/subashc2023/ultra-edit/releases/latest)
[![CI](https://github.com/subashc2023/ultra-edit/actions/workflows/ci.yml/badge.svg)](https://github.com/subashc2023/ultra-edit/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Claude often edits files through Bash heredocs, `sed -i`, and inline Python
instead of its Edit tool, and auto mode's system prompt encourages it. On
Windows, the Bash tool has been observed dropping backslashes, `<`, `>`, and
`..` from command payloads before the shell sees them, so those edits corrupt
files no matter how they are quoted.

Ultra Edit gives Claude an edit tool that never touches a shell. Changes travel
as MCP JSON to a native Rust engine, are checked against the files' current
bytes, and land all together or not at all. A hook blocks shell commands that
write file content into the project.

## Features

- **One call for related changes across files.** Every change is planned
  against the files as they were before the call. If any target is missing,
  ambiguous, or overlapping, nothing is written.
- **Edits by text or by line number.** `old`/`new` pairs work like native Edit
  and refuse matches inside longer words. `lines` and `after` take Read's line
  numbers, guarded by the text expected there, so a stale number is refused
  instead of applied.
- **Near-miss help.** When text isn't found, the error quotes up to three close
  matches with their line numbers (whitespace, line endings, or 70%+ similar),
  so Claude copies the text instead of guessing.
- **Byte-exact output.** Line endings, BOMs, and trailing whitespace outside the
  changes stay as they were. In an all-CRLF file, text copied from Read's
  LF-only view is written as CRLF.
- **Results Claude can check.** A committed edit returns the changed lines with
  their new line numbers, so there's no need to read the file again. Receipts are
  stored, an identical call replays its result without writing twice, and undo
  is conditional on the file still holding what was written.
- **Shell-write guard.** A `PreToolUse` hook denies heredocs, `sed -i`, inline
  scripts, `Set-Content`, and similar writes into project files through Bash and
  PowerShell, and tells Claude to use Ultra Edit, Edit, or Write instead.
- **No setup.** The release bundles native executables for Windows, macOS, and
  Linux. Git, Rust, Python, and `PATH` changes are not needed.

## Installation

Requires [Claude Code](https://code.claude.com/docs/en/terminal-guide) 2.1.224
or newer.

```text
claude plugin marketplace add subashc2023/ultra-edit
claude plugin install ultra-edit@ultra-edit
```

Then start a **new** session in the project you want to edit and run `/mcp`
to confirm that the `ultra-edit` server is connected.

The marketplace catalog pins the release archive by SHA-256. To update:

```text
claude plugin marketplace update ultra-edit
claude plugin update ultra-edit@ultra-edit
```

Then restart Claude Code or run `/reload-plugins`. The
[host guide](plugin/claude-code/skills/edit/references/claude-code.md) covers
source builds, session-only loading with `--plugin-dir`, and troubleshooting.

## Usage

No commands needed: ask for a change as usual. Session-start hooks load a short
routing card (about 1.9 KB) that tells Claude to edit existing files with Ultra
Edit, create new files with Write, and never write project files through the
shell. Subagents, resumed sessions, and compacted sessions get the card too.
`/ultra-edit:edit` loads optional detail on recovery and span targets.

A typical call puts every related change in one request:

```json
{
  "files": [{
    "path": "/work/app/src/retry.rs",
    "changes": [
      { "old": "const DELAY_MS: u64 = 100;", "new": "const DELAY_MS: u64 = 250;" },
      { "lines": [12, 12], "expect": "const RETRIES: u32 = 2;", "new": "const RETRIES: u32 = 3;" },
      { "after": 3, "expect": "use std::time::Duration;", "new": "use std::thread;" }
    ]
  }]
}
```

| Change | Meaning |
| --- | --- |
| `{"old","new"}` | Replace text that occurs exactly once, as whole words. Add `"count":N` to replace N occurrences, or `"in":[a,b]` to search only those lines. |
| `{"lines":[a,b],"expect","new"}` | Replace whole lines a–b; `"new":""` deletes them. `expect` is their text, or `[first line, last line]`. |
| `{"after":n,"expect","new"}` | Insert whole lines below line n, which stays. |

The response reports `commit: "committed"` and lists the changed lines as they
now read. A `rejected` response wrote nothing and names each failing change.
The [tool contract](docs/reference.md) covers span targets for files too large to
Read, preview and repair, retry, undo, and crash recovery.

## Shell-write guard

The guard parses each Bash and PowerShell command, and each script saved with
Write to a temporary directory, and denies ones that write content embedded in
the command into project files:

- heredocs, here-strings, and `echo`/`printf` output redirected or `tee`d into a file;
- in-place editors such as `sed -i` and `perl -pi`;
- inline Python, Node, Perl, Ruby, PHP, or PowerShell that calls file-write APIs;
- patches piped into `patch` or `git apply`, and saved scripts that write the project.

Writes certainly outside the project are allowed: `/tmp`, `$GITHUB_OUTPUT`,
`/dev/null`, and other absolute paths. So are ordinary output redirection
(`cargo test > log.txt`), plain copies, and `git commit -F -`. The guard allows
anything it cannot parse; it is a routing aid, not a sandbox. See
[the full rules](docs/shell-guard.md).

To turn it off, set the variable in Claude Code's `settings.json`:

```text
"env": { "ULTRA_EDIT_SHELL_WRITES": "allow" }
```

Setting it inside a shell command has no effect.

## Benchmarks

Measured with the [evaluation harness](eval/README.md) on Sonnet, Claude Code
2.1.294, on Linux. Cost is the geometric mean of per-task cost ratios against
native Claude Code.

| Task set | Ultra Edit | Native | Native, shell writes blocked |
| --- | --- | --- | --- |
| 14 tasks with the exact text given | 42/42 correct, 1.06x | 42/42, 1.00x | 41/42, 1.58x |
| 5 intent-level tasks (find the sites) | 11/11 correct, 1.22x | 11/11, 1.00x | 7/11, 1.52x |
| Another writer changes a file mid-edit | 3/3 correct, 0.71x | 3/3, 1.00x | 3/3, 0.88x |

Native Claude Code is as accurate on Linux, mostly because it writes through the
shell (30 of 42 runs on the first set). With shell writes blocked, Ultra Edit is
cheaper and more reliable than native Edit alone. Two of three third-party MCP
edit servers reported success while writing wrong bytes, by converting CRLF to
LF or adding a final newline. Windows runs, where shell writes are expected to
fail, are still to come. See [all results](docs/performance.md#model-level-results),
including Haiku, and the engine's [timings](docs/performance.md).

## Compared with native Edit

| | Native Edit | Ultra Edit |
| --- | --- | --- |
| Several files | One call per file | One all-or-nothing call |
| Repeated text | `replace_all` | An exact occurrence count |
| File changed since read | Refused if modified since Read | Old text matched against current bytes; line targets guarded by `expect` |
| Old text almost matches | Not found | Up to three near matches, quoted |
| After the edit | Claude Code tracks file state | Changed lines returned with new numbers |
| Recovery | `/rewind` checkpoints | Stored receipts, replay, conditional undo |

What you give up:

- **`/rewind`** does not cover Ultra Edit writes. Use `ultra_edit_undo` or git.
- **`PostToolUse` hooks matched on `Edit|Write`**, such as formatters, do not
  run. Add `mcp__plugin_ultra-edit_ultra-edit__ultra_edit` to their matchers.
- **IDE diff review** in VS Code and JetBrains shows native edits only.
- **Per-path Edit permission rules** such as `Edit(secrets/**)` do not apply.
  Edits are confined to the project root and use MCP tool permissions.

Ultra Edit edits existing UTF-8 files only. File creation, deletion, and
renames stay with Write and the shell. File writes are sequential, so a
multi-file commit is not an atomic filesystem transaction; a failure part way
is reported per file as `partial` or `outcome_unknown`.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `ultra-edit` missing from `/mcp` | Start a new session after installing, or run `/reload-plugins`. |
| Marketplace rejects the archive | Update Claude Code to 2.1.224 or newer. |
| Every Bash call shows a hook error | The plugin's executable is missing or blocked; reinstall it. Release binaries are unsigned, so some systems need a source build. |
| A legitimate command was blocked | Ask Claude to make the write with Ultra Edit, Edit, or Write, or disable the guard as above. |
| Claude still edits through the shell | Check that the plugin's hooks are enabled; the card is guidance and the guard catches only the patterns it recognizes. |

More in the [host guide](plugin/claude-code/skills/edit/references/claude-code.md#troubleshooting).

## Documentation

- [Host guide](plugin/claude-code/skills/edit/references/claude-code.md): installation, source builds, hooks, and permissions
- [Reference](docs/reference.md): CLI and engine contract, persistence, recovery, and limits
- [MCP adapter](docs/mcp-adapter.md): tools, responses, and validation
- [Shell-write guard](docs/shell-guard.md): what is denied and allowed
- [Performance](docs/performance.md): engine timings and model-level results
- [Changelog](CHANGELOG.md) and [releasing](docs/releasing.md)

## Contributing

Requires Rust 1.89 or newer. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup,
the checks CI runs, the source layout, and the evaluation harness.

```text
cargo build --locked
cargo run --locked --example walkthrough
```

## License

Ultra Edit is available under the [MIT License](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. Distributed binaries also
include notices for [Cargo dependencies](THIRD_PARTY_LICENSES.txt) and the
[Rust standard library](RUST_STANDARD_LIBRARY_LICENSES.html),
[Rust compiler-builtins](RUST_COMPILER_BUILTINS_LICENSE.txt), and its bundled
[libm code](RUST_LIBM_LICENSE.txt), plus the [musl C library](MUSL_COPYRIGHT),
[LLVM libunwind](LLVM_LIBUNWIND_LICENSE.txt), and
[LLVM compiler-rt](LLVM_COMPILER_RT_LICENSE.txt) licenses.
