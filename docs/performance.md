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

A model-level comparison runs the same edit tasks through each route. The
[evaluation harness](../eval/README.md) runs six fixture tasks, including
escape-heavy, line-ending, and large-file cases, under three arms: `native` (no
plugin), `native-guard` (native tools plus only the shell guard hook), and
`ultra-edit` (the full plugin). It scores exact final bytes, first-try success,
tool calls, tool errors, Bash file writes, turns, tokens, and cost, so the
plugin's instruction and snapshot overhead counts. It has no stale-file task
yet. No harness results are recorded here, so token savings and model cost
claims still wait for that evidence.
