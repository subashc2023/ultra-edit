# Rust engine performance

Measured September 7, 2026, on Windows. A
[September 25 follow-up](#blob-store-follow-up-2026-09-25) on Linux measures the
content-addressed blob store separately. This benchmark measures the Rust
library shared by the CLI and the persistent MCP server. It does not measure
Claude Code, model tokens, model latency, CLI process startup, or MCP
transport/serialization.

## Reproduce

```text
cargo run --locked --release --example benchmark
```

The [benchmark example](../examples/benchmark.rs) uses only existing dependencies.
It creates disposable workspaces under the OS temporary directory. An optional
existing directory selects the temporary parent and therefore the backing drive:

```text
cargo run --locked --release --example benchmark -- /path/to/temporary-parent
```

Every case has three warmups followed by 31 samples. It reports the median and
nearest-rank p95, verifies every result against deterministic expected bytes, and
fails on rejected edits or mismatched output. Fixture creation, workspace opening,
verification, result destruction, and temporary-directory cleanup are outside the
timers. The fixtures have just been written, so source reads are warm-cache.
Every filesystem sample uses a fresh workspace; these figures do not measure
performance with a long history of retained plans and journals.

## Timing boundaries

| Case | Input | What the timer includes |
| --- | --- | --- |
| Focused read | 100,000 fixed-width ASCII/LF lines; exactly 5,000,000 bytes; lines 50,000–50,009 return 499 source bytes, 10 listed lines, and the 11 disclosed references summarized as `["r50000..r50009", "selection"]` | `Workspace::read_range`: file read, whole-file hash, line scanning, selected references, and complete-byte snapshot persistence |
| Batch planning | 8 preloaded full snapshots, each 1,000 lines / 50,000 bytes; 8 exact changes per file | `compiler::compile`: digest/span validation, exact matching, conflict checks, output construction, and plan allocation; no snapshot creation or file I/O |
| Snapshots + batch edit | 2 files, each 200 lines / 10,000 bytes; 4 exact changes per file | Two `Workspace::read` calls, request construction, and `Workspace::edit`, including normal validation, retained snapshots/plans, staging, journals, and sync calls |

Each changed line replaces `100` with `250` in a uniquely numbered Rust constant.
Planning and editing use unscoped exact targets. No durability checks, resource
limits, or syncing are disabled. Returned source-byte counts are not complete
JSON payload sizes or token counts. These boundaries describe the September 7
engine; the blob store changed what focused-read persistence writes
([2026-09-25](#blob-store-follow-up-2026-09-25)).

## Environment and results

- Windows 11 Pro 10.0.26200, x86-64.
- AMD Ryzen 7 7800X3D, 8 cores / 16 logical processors.
- Local C: drive, NTFS on an NVMe SSD.
- Rust 1.98.0 (`88d9e12ae`, LLVM 22.1.8), default Cargo release profile,
  checked-in lockfile, Ultra Edit 0.1.0.
- Baseline engine: commit `8f871fd`, compiled with the same benchmark example.
  Optimized engine: the allocation change described below; benchmark unchanged.
- Sequential run order: baseline 1, optimized 1, baseline 2, optimized 2,
  baseline 3, optimized 3. No other builds or tests ran during these measurements.

Each cell is **median / p95 in milliseconds** for that run's 31 samples.

| Case | Run | Baseline | Optimized |
| --- | --- | --- | --- |
| Focused read | 1 | 299.999 / 322.627 | 228.708 / 282.060 |
| Focused read | 2 | 252.439 / 372.275 | 245.048 / 308.740 |
| Focused read | 3 | 254.061 / 298.183 | 216.816 / 252.053 |
| Batch planning | 1 | 6.002 / 6.757 | 5.601 / 6.144 |
| Batch planning | 2 | 4.553 / 5.977 | 4.684 / 5.836 |
| Batch planning | 3 | 4.773 / 5.948 | 4.980 / 6.829 |
| Snapshots + batch edit | 1 | 55.997 / 60.889 | 91.079 / 153.046 |
| Snapshots + batch edit | 2 | 78.127 / 149.701 | 128.217 / 195.200 |
| Snapshots + batch edit | 3 | 97.986 / 124.966 | 80.848 / 150.922 |

All 612 warmup and measured operations completed with exact output checks.
The README reports the range of the three optimized medians, rounded for reading.
The median of the three focused-read medians fell from 254.061 to 228.708 ms,
about 10% in this local experiment. That is not a pooled-sample median or a
statistical confidence estimate. Planning and complete-edit timings do not show
a consistent improvement; the complete-edit median was higher in two optimized
runs. No speedup is claimed for those paths. Their variability warrants a more
controlled experiment before drawing a performance-regression conclusion.

## Optimization

The shared line scanner previously created a `Span`, including an allocated
`rN` string, before the focused reader decided whether the line was selected.
The scanner now yields byte ranges. Full reads still construct every line
reference; focused reads construct them only after selection.

For a 10-line selection in a 100,000-line file, this removes 99,990 unnecessary
line-ID strings. It still scans all lines to report the exact total and reject
out-of-range requests. BOM, UTF-8 offsets, LF/CRLF handling, disclosed references,
whole-file stale detection, and persistence semantics are unchanged. Existing
compiler and focused-read tests cover those boundaries.

The improvement adds no dependency and changes no public API. Matching changes,
plan-storage redesign, and journal-history indexing were then deferred until
profiling established a need. The storage deferral is superseded: snapshots,
plans, and inspections now share the content-addressed blob store measured
below, and 0.2.0 indexes uncertain journals under `.ultra-edit/uncertain`.

## Blob store follow-up (2026-09-25)

These figures come from release builds in a shared 4-core Linux container. They
are not comparable with the Windows figures above. Each pair compares the engine
before and after the content-addressed blob store, which stores each string of
at least 4096 UTF-8 bytes once under `.ultra-edit/blobs`.

The focused-read timing boundary changed. `Workspace::read_range` used to
persist one snapshot object embedding the file's complete bytes. It now writes a
blob only when the content is new, plus a small snapshot object. Every benchmark
sample uses a fresh workspace, so each measured read still writes the 5 MB file
once, as a blob.

Benchmark medians from three alternating runs, as the range of the run medians
in milliseconds:

| Case | Before | After |
| --- | --- | --- |
| Focused read | 84.2–88.1 | 66.8–66.9 |
| Batch planning | ~10 | ~10 |
| Snapshots + batch edit | 17.5–18.7 | 19.1–21.5 |

The focused read was about 21% faster, and planning, which does no file I/O, was
unchanged. The complete edit in a fresh workspace was slower: each new piece of
content of at least 4 KiB costs an extra file and directory sync.

Workspace state for a 1,080,000-byte, 30,000-line JavaScript file plus a small
file, measured as the size of `.ultra-edit` in bytes:

| Measured after | Before the blob store | With the blob store |
| --- | --- | --- |
| Five six-line range reads | 5,553,807 | 1,084,172 |
| …then one more read of each file and a two-file edit | 9,998,607 | 2,169,262 |

Re-reading an unchanged file now writes only a small (about 1 KB) snapshot
object. A one-line edit of a large file adds one blob, the new content, plus
small objects.

## Comparisons that need another experiment

Native Claude Code Edit already supports literal old/new replacement and session
checkpoints. The README compares documented capabilities, not unmeasured speed.
A bare Bash or Python substitution also does less work than an edit with retained
snapshots, conditional writes, and synced recovery records, so its execution
time is not an equivalent performance baseline.

## Model-level results

A model-level comparison runs the same edit tasks through each route. The
[evaluation harness](../eval/README.md) runs 23 fixture tasks, including
escape-heavy, line-ending, near-duplicate, large-file, intent-level, and
stale-file cases, under 10 arms: native Claude Code tools with and without the
shell guard, the full plugin and the plugin with native edit tools disabled,
three shell-edit arms, and three third-party MCP edit servers. It scores exact
final bytes, first-try success, tool calls, tool errors, Bash file writes,
turns, tokens, and cost, so the plugin's instruction and snapshot overhead
counts.

One round on 2026-10-08: Claude Code 2.1.294, Sonnet, 3 repetitions, 420 runs,
none invalid. The two Ultra Edit arms were run again later that day, after the
session card was trimmed and began allowing `old` edits without a prior Read;
the table gives those runs. Cost is the geometric mean of per-task cost ratios
against `native`, with a 95% bootstrap interval.

| Arm | Correct | First try | Cost vs native |
| --- | --- | --- | --- |
| `native` | 42/42 | 39/42 | 1.00 |
| `native-guard` | 41/42 | 15/42 | 1.58 (1.53-1.64) |
| `ultra-edit` | 42/42 | 37/42 | 1.06 (1.02-1.09) |
| `ultra-edit-only` | 41/42 | 34/42 | 1.05 (1.02-1.09) |
| `shell-python` | 42/42 | 41/42 | 0.85 (0.82-0.88) |
| `shell-sed` | 42/42 | 39/42 | 0.99 (0.94-1.04) |
| `shell-patch` | 42/42 | 26/42 | 1.38 (1.29-1.46) |
| `desktop-commander` | 42/42 | 41/42 | 1.81 (1.74-1.89) |
| `mcp-filesystem` | 30/42 | 30/42 | 1.66 (1.59-1.73) |
| `mcp-text-editor` | 38/42 | 29/42 | 2.48 (2.36-2.61) |

- All 12 `mcp-filesystem` failures and three of the four `mcp-text-editor`
  failures wrote wrong bytes while the tool reported success: the first
  rewrote CRLF files with LF, the second added a final newline to a file that
  had none.
- The task prompts state every change exactly, so a blind shell substitution
  is enough, and `native` wrote through the shell in 30 of 42 runs. Where shell
  edits are not acceptable, `native-guard` is the comparable native route; it
  attempted shell writes in 26 runs, all blocked, and cost 1.50x (1.46-1.54)
  what `ultra-edit` did.
- Before the card change, both Ultra Edit arms cost 1.20x `native`, with 2.5
  Read calls per run against native's 1.4. Allowing `old` edits without a Read
  halved that to 1.3 and cut mean cost per run from $0.074 to $0.065; turns fell
  to 0.75x native's and tool calls to 0.72x. The fixed context is about 2.0k
  tokens per API call for the card and the always-loaded tool schema.
- First try fell from 39 to 37 and from 37 to 34, 13 misses against 8. Most of
  the rise is `unicode-quotes`, from 1 miss to 5: blind `old` edits that typed
  the prompt's no-break space as an ordinary space. Each was rejected without a
  write; four copied the near match the rejection quoted and one shortened
  `old`, and the task still cost less than reading first.
- The one `ultra-edit-only` failure restated an anchor line as the first line of
  an `after` insertion, leaving the line twice. The server now refuses that
  (`INSERT_REPEATS_LINE`); no other recorded insertion of 616 did it.
- Cost varied least across repetitions in `ultra-edit-only`, `shell-python`,
  and `ultra-edit`: a mean per-task coefficient of variation of 0.05-0.06,
  against 0.10 for `native`.

Three intent-level tasks, run on 2026-10-08 with the same settings, describe the
change without quoting the lines, so the sites have to be found: a version bump
beside dependency pins and history at the same number, an option removal beside
a look-alike option, and a function rename beside methods and another module's
function of the same name. With 3 repetitions each:

| Arm | Correct | First try | Cost vs native |
| --- | --- | --- | --- |
| `native` | 8/9 | 8/9 | 1.00 |
| `native-guard` | 8/9 | 5/9 | 1.15 (1.03-1.28) |
| `ultra-edit` | 9/9 | 9/9 | 1.03 (0.92-1.14) |
| `ultra-edit-only` | 9/9 | 9/9 | 0.97 (0.88-1.07) |

The two failures were model slips: `native` dropped a space from a user-agent
string, and `native-guard` removed one blank line too many. On the version bump
and the rename, both Ultra Edit arms finished in two tool calls: a search, then
one edit built from its output.

Five more intent-level tasks were run on 2026-10-08 with the same settings, after
`OLD_INSIDE_WORD` and `INSERT_REPEATS_LINE` landed: a CSS class renamed beside
hyphenated names that contain it (`btn-primary-outline`, `--btn-primary-bg`), a
feature flag retired by keeping its on-branches dedented, a default of 10 raised
beside another default of 10 on the next line, one YAML section's `pool_size`
key renamed while two other sections keep the name, and a function moved to
another module with its imports and docs entry. For each, an agent that never
saw the expected bytes carried out the prompt and matched them. The $4.50 cap
stopped the round in its third repetition; the table counts the 44 runs in task
repetitions that all four arms finished.

| Arm | Correct | First try | Cost vs native |
| --- | --- | --- | --- |
| `native` | 11/11 | 11/11 | 1.00 |
| `native-guard` | 7/11 | 5/11 | 1.52 (1.43-1.62) |
| `ultra-edit` | 11/11 | 10/11 | 1.22 (1.16-1.28) |
| `ultra-edit-only` | 11/11 | 7/11 | 1.27 (1.16-1.37) |

- On both renames, `native` wrote every site with one `perl` or `sed` command
  whose regex excluded the look-alikes; on the CSS rename that took 0.8k output
  tokens, against 3.1k for a 29-change `ultra_edit` call. That is the route the
  shell guard blocks.
  `native-guard`, left with Edit, failed all three CSS renames and one of two
  YAML renames, and cost 1.25x (1.17-1.33) what `ultra-edit` did.
- Every `native-guard` CSS failure was the same slip, three runs out of three:
  an `old_string` ending in a space whose `new_string` dropped it, as in
  `".form-actions .btn-primary + "` written as `".form-actions .btn-accent +"`,
  which joins `+` to the next selector. Ultra Edit would write that text too,
  but answers it with a `WHITESPACE_EDGE` warning quoting the joined line; the
  one such warning in these rounds, a doubled indent in a Haiku run, was fixed
  in the next call.
- On the other three tasks Ultra Edit cost the same as `native`, except the
  function move at 1.25x: after a multi-hunk batch, the model looked at the
  result again with `git diff` or Read in 5 and 6 of 11 runs of the two Ultra
  Edit arms, against 2 of 12 `native` runs, which Claude Code tells that their
  file state is current.
  The five first-try misses were blank-line or ```` ``` ```` guards refused as
  `LINE_GUARD_WEAK`, one missed `old`, one call with no changes, and one call
  whose JSON Claude Code could not parse.

## Haiku results

The four Claude Code arms were run with Haiku on all 23 tasks on 2026-10-08,
with the same settings and 2 repetitions each: 184 runs, none invalid, $1.33 in
all.

| Arm | Correct | First try | Cost vs native |
| --- | --- | --- | --- |
| `native` | 46/46 | 43/46 | 1.00 |
| `native-guard` | 44/46 | 28/46 | 1.21 (1.15-1.26) |
| `ultra-edit` | 46/46 | 36/46 | 1.09 (1.05-1.14) |
| `ultra-edit-only` | 46/46 | 39/46 | 1.05 (1.00-1.09) |

- The smaller model did not open a correctness gap: Haiku finished every task
  with native tools as well. Both `native-guard` failures were the CSS rename's
  dropped trailing space, the slip Sonnet made in all three of its runs.
- Ultra Edit took about half the turns (0.54x) and tool calls (0.49x) of
  `native`, and 0.91x its context tokens, but 1.16x its output tokens; it cost
  0.91x what `native-guard` did.
- Each of the 17 first-try misses in the Ultra Edit arms was refused without a
  write and fixed in a later call: 8 `old` texts that missed, half of them on
  `unicode-quotes`; 3 calls whose JSON Claude Code could not parse because the
  model closed the nested `files` and `changes` arrays with one brace too many
  (Sonnet did that in 2 of about 490 Ultra Edit runs); 2 stale `expect` guards
  on the stale-file task; 2 ambiguous `old` texts; and 2 weak guards.

## Stale-file results

`edit-while-file-changes` simulates another writer. As soon as the model has
been shown the retry settings, the writer inserts a block above them and
rewords the comment on a line the model edits; after the model's first write to
the file, it inserts a line above the next edit site (see
[`concurrent.json`](../eval/README.md#tasks)). Run on 2026-10-08 under all 10
arms with the same settings, 3 repetitions each:

| Arm | Correct | First try | Cost vs native |
| --- | --- | --- | --- |
| `native` | 3/3 | 0/3 | 1.00 |
| `native-guard` | 3/3 | 1/3 | 0.88 (0.74-1.02) |
| `ultra-edit` | 3/3 | 2/3 | 0.71 (0.58-0.87) |
| `ultra-edit-only` | 3/3 | 2/3 | 0.68 (0.56-0.85) |
| `shell-python` | 3/3 | 0/3 | 0.72 (0.65-0.77) |
| `shell-sed` | 3/3 | 3/3 | 0.54 (0.49-0.58) |
| `shell-patch` | 3/3 | 0/3 | 0.83 (0.72-0.96) |
| `desktop-commander` | 3/3 | 3/3 | 1.05 (0.92-1.17) |
| `mcp-filesystem` | 3/3 | 3/3 | 0.70 (0.63-0.75) |
| `mcp-text-editor` | 3/3 | 2/3 | 1.36 (1.23-1.50) |

- In every run the first change landed after the model had been shown the
  file, and every run ended with both writers' changes. Each route either
  matched text in the current file (Edit's `old_string`, `ultra_edit`'s `old`,
  sed patterns, MCP `old`/`new` pairs) or noticed the change: native Edit's
  modification-time check, `expect` on an Ultra Edit line target,
  `mcp-text-editor`'s file hash against a whole-file rewrite, `git apply`
  comparing the lines it removes, and an assertion the model wrote into its own
  Python script.
- Recovery cost differed. `native` Edit refused twice per run with "File has
  been modified since read", and the model read the file again each time: 4.0
  Read calls per run, against 2.3-2.7 in the Ultra Edit arms.
- One `ultra-edit` run inserted with `after:19` from its earlier read after the
  anchor had moved to line 23. `expect` refused the batch, the message named
  line 23, and the next call committed. The other Ultra Edit miss was an `old`
  that also matched inside `WEBHOOK_RETRY_LIMIT`.
