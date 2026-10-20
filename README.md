# Ultra Edit

**Checked, multi-file edits for Claude Code, without shell heredocs.**

[![Release](https://img.shields.io/github/v/release/subashc2023/ultra-edit)](https://github.com/subashc2023/ultra-edit/releases/latest)
[![CI](https://github.com/subashc2023/ultra-edit/actions/workflows/ci.yml/badge.svg)](https://github.com/subashc2023/ultra-edit/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Claude often edits files through Bash heredocs, `sed -i`, and inline Python
instead of its Edit tool, and auto mode's system prompt encourages it. On
Windows, the Bash tool has been observed dropping backslashes, `<`, `>`, and
`..` from command payloads before the shell sees them, so those edits corrupt
files no matter how they are quoted. Claude Code's tracker has reports of
backslashes halved inside quoted heredocs
([anthropics/claude-code#89392](https://github.com/anthropics/claude-code/issues/89392))
and of commands cut off near 8,000 characters
([anthropics/claude-code#93618](https://github.com/anthropics/claude-code/issues/93618)).

Ultra Edit gives Claude an edit tool that never touches a shell. Changes travel
as MCP JSON to a native Rust engine, are checked against the files' current
bytes, and land all together or not at all. A hook blocks shell commands that
write file content into the project.

Every model-level benchmark so far ran on Linux, where native Claude Code is as
accurate and on most tasks slightly cheaper, mostly because it writes through
the shell.
Ultra Edit pays off where that route is unsafe or unwanted: with shell writes
blocked, it was the cheapest route that stayed exact, and unlike two of the
three MCP edit servers tested, it never changed bytes outside the requested
edits. See [Benchmarks](#benchmarks).

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

No commands needed: ask for a change as usual. The plugin starts the server with
`["--root", "${CLAUDE_PROJECT_DIR}", "--plugin-card"]`, so the `ultra_edit`
description is a short routing card (about 2.2 KB) that tells Claude to edit
existing files with Ultra Edit, create new files with Write, and never write
project files through the shell. Claude Code caches it with the tool list, and
session-start hooks, including for subagents, resumed sessions, and compacted
sessions, add a short context that routes edits to it. `/ultra-edit:edit` loads
optional detail on recovery and span targets.

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
| `{"old","new"}` | Replace text that occurs exactly once, as whole words; matches inside longer names don't count. Add `"all":true` to replace every whole-word occurrence, as a rename needs, `"count":N` to replace N occurrences, or `"in":[a,b]` to search only those lines. |
| `{"lines":[a,b],"expect","new"}` | Replace whole lines a–b; `"new":""` deletes them. `expect` is their text, or `[first line, last line]`. |
| `{"after":n,"expect","new"}` | Insert whole lines below line n, which stays. |

A stale line number never edits other text: a long `expect` that holds at
exactly one other place moves the change there, as does a long `old` found once
outside its `in` lines, with a `LINES_MOVED` warning; anything else is rejected.

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

The [evaluation harness](eval/README.md) runs each task in a fresh, isolated
headless Claude Code session and scores the exact final bytes of every file.
Its 23 tasks are small repositories of 1 to 10 files: 14 state the exact text
to change, 8 describe the change and leave finding the sites to the model, and
in one another writer edits a file mid-session. The rounds in the first table
ran on Linux with Claude Code 2.1.294; Sonnet ran each task 3 times per route
and Haiku twice. Cost is the geometric mean of per-task cost ratios against
native Claude Code.

| Task set | Ultra Edit | Native | Native, shell writes blocked |
| --- | --- | --- | --- |
| 14 tasks with the exact text given | 42/42 exact, 1.06x | 42/42, 1.00x | 41/42, 1.58x |
| 3 intent-level tasks (find the sites) | 9/9 exact, 1.03x | 8/9, 1.00x | 8/9, 1.15x |
| 5 more intent-level tasks | 11/11 exact, 1.22x | 11/11, 1.00x | 7/11, 1.52x |
| Another writer changes a file mid-edit | 3/3 exact, 0.71x | 3/3, 1.00x | 3/3, 0.88x |
| All 23 tasks on Haiku | 46/46 exact, 1.09x | 46/46, 1.00x | 44/46, 1.21x |

### Windows

Four more rounds ran all 23 tasks on Windows 11 on 2026-10-19 and 20, with
Claude Code 2.1.303 and Sonnet 5.5, 3 times each, comparing Ultra Edit with
native Claude Code. They ran development builds between 0.4.0 and 0.5.0. The
last round, whose prompts carry a marker drawn per run so that no run reuses
another's cached prompt, gave:

| | Ultra Edit | Native |
| --- | --- | --- |
| Exact final bytes | 69/69 | 68/69 |
| Right on the first try | 65/69 | 63/69 |
| Cost | 1.00x (0.97-1.04) | 1.00x |
| Turns | 0.64x (0.60-0.69) | 1.00x |
| Tool calls | 0.59x (0.55-0.65) | 1.00x |
| Output tokens | 0.95x (0.91-0.99) | 1.00x |

Over all four rounds, Ultra Edit was exact in 275 of 276 runs and native in
272 (p = 0.37), and right on the first try in 252 and 249. Native wrote
through the shell in 142 of its 276 runs, and none of those ended wrong; all
four of its wrong results were written with Edit. Ultra Edit's one wrong
result deleted a function with an `old` that also took one of the two blank
lines below it, which the server now flags as `BLANK_LINES_DROPPED`.

### Failure rates

Pooled over the Linux rounds:

| | Ultra Edit | Native | Native, shell writes blocked |
| --- | --- | --- | --- |
| Wrong final bytes, Sonnet (65 runs each) | 0 | 1 | 6 |
| Wrong final bytes, Haiku (46 runs each) | 0 | 0 | 2 |
| Not right on the first try, Sonnet | 7 (11%) | 7 (11%) | 39 (60%) |
| Not right on the first try, Haiku | 10 (22%) | 3 (7%) | 18 (39%) |

A run is not right on the first try if it ended wrong or any edit call on the
way failed: a refused Ultra Edit call, an Edit error, or a blocked shell write.
No wrong runs in 111 puts Ultra Edit's rate below 3.3% (95% Wilson interval),
and the gap to native with shell writes blocked, 8 wrong in 111, is significant
(Fisher's exact test, p = 0.007). Native's first-try lead on Haiku is not
(p = 0.07).

### How each route failed

- **Ultra Edit** checks the whole batch before writing, so a missing,
  ambiguous, or stale target is refused with nothing changed. The common
  refusals were text typed differently from the file (a no-break space typed
  as a space; the refusal quotes the near match), a guard on a weak line such
  as a blank line or a fence, a line number that a concurrent change had moved,
  and JSON that Claude Code could not parse (3 of 92 Haiku runs; 2 of about 490
  Sonnet runs, earlier rounds included). Of 222 runs with the plugin, with
  native edit tools available or disabled, one committed a wrong result: an
  insertion that repeated the line it followed, which the server now refuses.
- **Native Edit with shell writes blocked:** on the exact-text tasks the model
  tried a shell write in 26 of 42 runs, all blocked, which accounts for nearly
  all of its first-try misses there. Five of its eight wrong results were one
  slip that Edit wrote as given: an `old_string` ending in a space whose
  `new_string` dropped it, joining two CSS selectors. Ultra Edit writes such
  text too, but warns and quotes the joined line.
- **Third-party MCP edit servers**, on the exact-text tasks: two of three wrote
  wrong bytes while reporting success, one rewriting CRLF files as LF (12 of 42
  runs) and the other adding a final newline to a file without one (3 of 42).
  The third was exact in every run, at 1.81x native's cost.
- **Shell edits:** native Claude Code wrote through the shell in 30 of 42
  exact-text runs. Held to one method, `sed`, Python, and `git apply` were exact
  in every run; on the exact-text tasks Python was the cheapest route measured,
  at 0.85x native, and `git apply` missed its first try in 16 of 42.
- **A file changed mid-edit:** every route kept both writers' changes. Native
  Edit refused twice per run as modified since read and re-read the file (4.0
  Reads per run, against 2.3-2.7 for Ultra Edit); Ultra Edit's `expect` caught
  the one stale line number.

### Context and cost

- **Fixed overhead:** about 2.0k tokens on every API call (2.1k in the last
  Windows round), mostly the `ultra_edit` description, which is the routing
  card, and its schema. Both load up front so an edit needs no `ToolSearch`
  round trip. The prompt cache serves them after the first call, and the
  snapshot and recovery tools stay deferred until used.
- **Fewer, larger calls:** on Sonnet's exact-text tasks, Ultra Edit runs took
  0.75x native's turns and 0.72x its tool calls; on Haiku, across all tasks,
  0.54x and 0.49x, with 0.91x the context tokens but 1.16x the output tokens;
  on Windows, across all tasks, 0.64x and 0.59x at the same cost.
  Each change restates its text: a 29-site CSS rename was 3.1k output tokens as
  one `ultra_edit` call and 0.8k as native's one `perl` regex. In the 1.22x
  intent-level round, the two renames and a function move, after which the
  model re-checked its batch, cost more; the other two tasks cost the same as
  native.
- **No Read before a known edit:** when the prompt or earlier output gives the
  exact text, the card has Claude send an `old` edit without reading the file,
  since a miss writes nothing and quotes the closest match. That cut Reads from
  2.5 to 1.3 per run (native: 1.4) and cost from 1.20x to 1.06x native.
- **No re-read after an edit:** after a multi-hunk batch, the model ran
  `git diff` or Read again in about half the Ultra Edit runs, against 2 of 12
  native runs. A committed call now returns the changed lines with their new
  numbers instead; the Linux rounds predate it.
- **Steadier cost:** on the exact-text tasks, cost varied about half as much
  across repetitions as native's (mean coefficient of variation 0.05-0.06,
  against 0.10).

### Not yet measured

- Opus, Haiku on Windows, and the shell, shell-guard, and third-party routes
  on Windows.
- Large repositories and long sessions, including compaction. The fixtures are
  1 to 10 files, most under 200 lines.
- Permission prompts, other plugins, and `CLAUDE.md` files: runs use isolated
  settings and `bypassPermissions`.

The exact-text prompts let a blind `sed` or Python substitution succeed, which
favors the shell routes. The [per-round results](docs/performance.md#model-level-results)
cover all ten routes, and the [engine timings](docs/performance.md) are measured
separately.

## Compared with native Edit

| | Native Edit | Ultra Edit |
| --- | --- | --- |
| Several files | One call per file | One all-or-nothing call |
| Repeated text | `replace_all` | `"all"` for every whole-word occurrence, or an exact count |
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
- **About 2.0k tokens of context** on every API call, mostly served from the
  prompt cache; see [context and cost](#context-and-cost).
- **A `.ultra-edit/` directory** in the project keeps snapshots, receipts, and
  journals for replay and undo. It ignores itself in Git and stores each large
  string once; nothing is removed automatically, and
  [`prune-snapshots`](docs/reference.md#persistence-and-evidence) reclaims old
  snapshots.

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
