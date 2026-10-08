# Model-level evaluation

`run_eval.py` compares editing methods head-to-head on small, deterministic
repositories: native Claude Code tools, Ultra Edit, shell edits (sed, Python,
`git apply`), and third-party MCP edit servers. It measures exact final bytes,
first-attempt success, tool calls and errors, tokens, context size, cost, and
the spread of each across repetitions. It runs headless `claude -p` sessions,
saves each raw stream-json transcript, and scores the final bytes.

> **Cost warning.** Every run is a real Claude Code session billed to your
> account: tasks × arms × `--reps` sessions (14 tasks × 3 default arms = 42
> runs per repetition; `--arm all` runs every arm). `--dry-run` and
> `--preflight` never call a model. Paid runs need confirmation (or `--yes`),
> each run is capped by `--max-budget-usd` (default 2), and `--max-total-usd`
> stops starting runs once recorded spend reaches a limit. If
> `ANTHROPIC_API_KEY` is set, `claude -p` always bills that key, even when you
> are logged in.

## Arms

Without `--arm`, the evaluation runs `native`, `native-guard`, and
`ultra-edit`. `--arm NAME` is repeatable; `--arm all` selects every built-in arm
and every third-party arm in `third_party/arms.json`.

| Arm | What loads | What it measures |
| --- | --- | --- |
| `native` | Claude Code only | The baseline: native Read/Edit/Write/Bash. |
| `native-guard` | Native tools plus Ultra Edit's shell guard: a `PreToolUse` hook (matcher `Bash\|PowerShell`, set by `--guard-matcher`) running `<staged runtime>/ultra-edit-mcp --claude-hook PreToolUse`, configured in a generated `--settings` file | Whether blocking heredoc, here-string, inline-script, `sed -i`, `-replace`, and `echo`/`printf`/`Set-Content` writes into the project alone improves native editing. |
| `ultra-edit` | The full plugin via `--plugin-dir <staged plugin>`: MCP tools, routing hooks, skill | The complete product against both baselines. |
| `ultra-edit-only` | `ultra-edit` plus `--disallowedTools Edit,Write,MultiEdit,NotebookEdit` | Ultra Edit when the model cannot fall back to native edits. |
| `shell-sed` | Native tools minus Edit/Write/MultiEdit/NotebookEdit; `--append-system-prompt` asks for sed, awk, or perl one-liners through Bash; a method hook enforces it | Stream-editor edits. |
| `shell-python` | Same, asking for Python 3 code through Bash (`python3 - <<'PY' ... PY`) | Scripted edits. |
| `shell-patch` | Same, asking for unified diffs applied with `git apply` through Bash | Patch-based edits. |
| third-party | An MCP server from `third_party/arms.json` via `--mcp-config <run>/mcp.json`, usually with native edit tools disallowed | Popular MCP edit servers against Ultra Edit. |

Every arm gets the same task prompt, flags, and isolation; only the plugin,
hook, MCP server, disallowed tools, and appended system prompt differ. The
shell arms' appended prompt starts "For this session the Edit, Write,
MultiEdit, and NotebookEdit tools are unavailable." Arm order is shuffled per
task and repetition (`--seed`) to spread prompt-cache effects across arms.

Each shell arm also gets a `PreToolUse` hook on Bash, `method_hook.py --method
{sed,python,patch}`, that denies a command writing files any other way, and the
appended prompt says such commands are blocked. Without it the arms measured
whatever the model reached for: in earlier rounds only 14 of 42 `shell-patch`
sessions used `git apply` for every write, the rest using `sed -i` or Python.
Other programs may read files and print output, and `>` or `tee` may save that
output under the temporary directory (a patch built from `sed -n` slices, say),
but no other program may write a file itself, scratch files included, so a
Python-generated diff does not pass for `git apply`. Each shell run records `off_method_attempts` and `off_method_writes` (attempts that ran
unblocked; any are an integrity warning).

Before an arm's measured runs, one short session per arm caches its shared
system-and-tools prompt prefix (`warmup.jsonl`; `--no-warmup` skips it), so an
arm measured after a pause is not charged for writing that prefix while another
measured right after it is. Each run records the Claude Code build and the
Ultra Edit executable's SHA-256 it ran with, and `--resume` refuses to mix runs
measured with different ones unless `--allow-drift` is given.

### Third-party arms

Third-party arms are defined in `third_party/arms.json`; installation and the
list of servers are in [`third_party/README.md`](third_party/README.md). Each
entry has a `description`, a `server_name`, an `mcp_server` (`command`, `args`,
`env`), the `edit_tools` that count as edit calls (required), the
`required_tools` that must be listed at init, an optional `append_system_prompt`, and
`disallowed_tools` (default: the four native edit tools). In `command`, `args`,
and `env`, `{repo}` becomes the run's temporary repository and
`{third_party_dir}` the install directory (`third_party/install`, or
`--third-party-dir DIR`). Tool names are `mcp__<server_name>__<tool>`.

Each run writes the substituted server to `<run>/mcp.json` and passes
`--mcp-config` without `--strict-mcp-config`. With a permission mode other than
`bypassPermissions`, `mcp__<server_name>` is added to `--allowedTools`.
`--dry-run` prints each arm's `mcp.json`; `--preflight` checks that the server's
command (and any `{third_party_dir}` path in its arguments) exists without
starting it. An unknown arm, a malformed entry, or a missing server is an
error before any paid run. Arm names may use letters, digits, `.`, `_`, and `-`
(no `__`), since they become run directory names. A run of a third-party arm is
invalid unless its server is `connected` at init and its `required_tools` (or,
without any, at least one of its tools) are listed. `--third-party-arms FILE` reads another arms file.

## Tasks

Each `tasks/<name>/` holds `prompt.md`, `fixture/` (initial repository), and
`expected/` (exact bytes of every file that must change). Every other file must
stay byte-identical, and no file may be added or removed.

| Task | Hazard |
| --- | --- |
| `rename-constant` | Rename across 5 files including docs, with a longer look-alike constant that must not change |
| `windows-paths-escapes` | Windows paths, regex escapes, JSON-doubled backslashes, UNC path with `$`, BOM + CRLF PowerShell |
| `crlf-and-lf` | CRLF `.cmd` edited alongside LF files, an inserted CRLF line, a file without a final newline |
| `markdown-hard-breaks` | Trailing double-space line breaks that must survive the edit |
| `large-file-two-regions` | Two distant edits in a 1,998-line file of near-identical functions (`generate.py` recreates it) |
| `tabs-makefile-go` | Tab-indented Makefile recipes and Go code, with inserted tab-indented lines |
| `many-scattered-edits` | Many single-line edits in a ~2,650-line TOML file whose lines repeat across tables (`generate.py` recreates it) |
| `move-and-delete-blocks` | Reordering and deleting blocks across Python and a CRLF Markdown file |
| `replace-function-body` | Replacing a whole function body and signature with exact indentation |
| `sed-hostile-regex` | Lines full of regex and replacement metacharacters (`\`, `\|`, `$1`, `&`, `/`) |
| `shell-hostile-scripts` | Shell, YAML, and Makefile lines with `$`, quotes, backslashes, and backticks |
| `signature-threading` | A new parameter threaded through Python, CRLF TypeScript, tests, and docs, beside look-alike names |
| `unicode-quotes` | Curly quotes, dashes, no-break spaces, and invisible characters that must keep their code points |
| `yaml-near-duplicates` | Near-identical staging and prod YAML documents where only one occurrence changes |

## Setup

1. Install Git and Python 3.10 or newer. Claude Code must be 2.1.139 or newer;
   the flags were verified against 2.1.282, and `--disallowedTools`,
   `--append-system-prompt`, and `--mcp-config` (used by the newer arms)
   against 2.1.292. `--preflight` checks the flags each selected arm needs.
2. Build the runtime from the repository root:
   `cargo build --locked --release`. The harness stages a copy of
   `plugin/claude-code` with the executables in its `runtime/` directory
   inside the results folder. Pass `--runtime-dir DIR` to use another build,
   such as an extracted release's `runtime/`. The `native-guard` arm needs a
   build that supports `--claude-hook PreToolUse`; `--preflight` feeds it a
   Bash heredoc write and a read, plus a PowerShell here-string write and a
   read when the matcher names PowerShell, and fails if the hook doesn't deny
   the writes and allow the reads.
3. For third-party arms, install their servers as described in
   [`third_party/README.md`](third_party/README.md).
4. Authenticate. The default mode uses your normal login. With
   `--isolate-config`, each run gets an empty `CLAUDE_CONFIG_DIR`, and
   therefore no stored login: set `ANTHROPIC_API_KEY`, or
   `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`.

### Isolation

Your installed plugins, MCP servers, hooks, and `CLAUDE.md` files must not
leak into any arm. By default each run uses:

- `--setting-sources=` (empty): no user, project, or local settings files. This
  also skips user and project `CLAUDE.md` files (including parent
  directories), user and project MCP servers, `~/.claude/skills` plugins, and
  your `enabledPlugins`.
- `--settings <run>/settings.json`: always loaded. It turns off claude.ai
  connectors, claude.ai plugin and skill sync, and auto memory, and disables
  installed copies of Ultra Edit (`ultra-edit@ultra-edit`,
  `ultra-edit@skills-dir`) plus every `--disable-plugin ID` (repeatable; for
  example a cloud environment's `cc-plugin-telemetry@builtin`). For
  `native-guard` it also holds the hook.
- `--no-session-persistence`, plus these environment variables:
  `DISABLE_AUTOUPDATER=1`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`,
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `CLAUDE_CODE_SKIP_PROMPT_HISTORY=1`.
  The harness removes `CLAUDE_CODE_PLUGIN_DIRS` and the markers a parent
  Claude Code session exports (`CLAUDECODE`, ...). It also removes behavior
  settings a host may set for itself, such as `CLAUDE_EFFORT`,
  `MCP_CONNECTION_NONBLOCKING`, `ENABLE_TOOL_SEARCH`, and
  `CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH`, plus variables starting with
  `CLAUDE_CODE_ARTIFACT_`, `CLAUDE_CODE_REMOTE`, or `CLAUDE_CODE_SYNC_`, so
  every arm runs with Claude Code's defaults. Pass `--effort` to set effort.

Every run is then checked against its `system/init` message and hook events:

- A run of an arm without the plugin that shows any Ultra Edit plugin, MCP
  server, or tool is marked `invalid`.
- An `ultra-edit` or `ultra-edit-only` run whose plugin or server did not load,
  or whose `SessionStart` hook failed, is marked `invalid`.
- A `native-guard` run is marked `invalid` if a shell tool that the matcher
  names ran without any `PreToolUse` hook event, or if the hook failed.
- A third-party run is marked `invalid` if its server is missing or not
  `connected` at init, or a required tool is not listed. Any other arm that
  shows a server named in `arms.json`, or its tools, is marked `invalid`.
- A run whose init tool list still contains one of its disallowed tools is
  marked `invalid`.

Invalid runs are excluded from the rates and listed in the summary.

Tradeoffs:

- `--isolate-config` is stronger: it also drops global state in
  `~/.claude.json`. It needs token or API-key authentication.
- Organization-managed settings always apply and cannot be overridden.
- `--strict-mcp-config` is not used. It also drops plugin-provided MCP servers,
  which would remove Ultra Edit from its own arm.
- `--bare` and `--safe-mode` are not used. They skip hooks and plugins, and
  `--bare` also narrows the tool set.
- Without `--isolate-config`, each temporary repository still adds a project
  entry to `~/.claude.json`.

### Permissions

The default `--permission-mode bypassPermissions` avoids permission prompts
and denials, so only the guard hook blocks anything. PreToolUse hooks run before
permission checks. The model has full shell access while it works in a
temporary Git repository. The tasks are benign, but use a VM or Windows Sandbox
if that matters to you. Claude Code refuses this mode as root on Linux and
macOS. There, use `--permission-mode acceptEdits`, which adds `--allowedTools`
for Bash, the file tools, Skill, the Ultra Edit MCP server, and a third-party
arm's server. An arm's disallowed tools are left out of `--allowedTools`.

## Run it

Windows (PowerShell, from the repository root):

```powershell
cargo build --locked --release
py -3 eval\run_eval.py --dry-run
py -3 eval\run_eval.py --preflight
py -3 eval\run_eval.py --model sonnet --reps 1 --max-total-usd 5
py -3 eval\run_eval.py --model sonnet --reps 5 --yes
```

macOS and Linux:

```sh
cargo build --locked --release
python3 eval/run_eval.py --dry-run
python3 eval/run_eval.py --preflight
python3 eval/run_eval.py --model sonnet --reps 1 --max-total-usd 5
python3 eval/run_eval.py --arm all --model sonnet --reps 5 --jobs 4 --max-total-usd 60
```

GitHub Actions: add an `ANTHROPIC_API_KEY` repository secret (a dedicated key
with a spend limit), or `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`, then
run the manual `Eval` workflow (`.github/workflows/eval.yml`). It builds the
runtime on `ubuntu-24.04` or `windows-2025`, installs the pinned Claude Code,
runs the preflight, then evaluates with the chosen model, repetitions, spend
limit, and tasks. The runner is a fresh VM, so `bypassPermissions` is contained.
The job summary shows `summary.md`; the `eval-<runner>-<run id>` artifact holds
the full results, with the secret's value redacted from every file.

Narrow a run with repeatable `--task NAME` and `--arm NAME`. Pin `--model` so
every arm uses the same model. `--jobs N` runs N sessions at once (default 1);
each run has its own temporary repository, the plugin is staged once, and
results are appended to `runs.jsonl` as runs finish. With `--max-total-usd`, no
new run starts once recorded spend reaches the limit; runs in flight finish and
are recorded, and the number of skipped runs is printed. Parallel sessions
share rate limits, so a high `--jobs` can add `api_retries` and wall time, or
end runs with a `rate_limit` error once Claude Code's own retries give up. Such
a run (rate limit, overload, or server error) starts again after a pause of
`--retry-delay` seconds (default 60), doubling each time, up to
`--transient-retries` times (default 3). Earlier attempts stay in
`runs/<id>.attempt-N`, and the record counts them in `transient_retries` and
`transient_retry_cost_usd`.

`--resume RESULTS_DIR`, with the same `--task`, `--arm`, `--reps`, and `--seed`
as the original run, keeps every recorded run and starts the planned runs that
have no record or ended in `infra_error`. `--rerun-arm NAME` also restarts every
run of that arm, for example after fixing its configuration. Replaced runs'
directories move to `runs/<id>.attempt-N`.
Other options: `--max-turns` (default 50),
`--timeout` seconds per run (default 900), `--effort`, `--keep-workdirs` (keep
the temporary repositories for inspection), and `--extra-settings FILE` (JSON
merged into every arm's settings, for example an `apiKeyHelper` or `env` your
login needs). Run `--help` for the full list.

### Windows notes

- The harness finds `claude` with `shutil.which` (`claude.exe` from the native
  installer, or npm's `claude.cmd`) and never uses a shell. The prompt goes to
  stdin, so newlines, quotes, and backslashes never pass through `cmd.exe`.
  Prefer `claude.exe`; with `claude.cmd`, avoid `&`, `%`, and `^` in the
  checkout and temp paths.
- Claude Code needs Git for Windows (Git Bash) and may also offer a
  PowerShell tool. The guard covers both by default (`--guard-matcher Bash`
  limits it to Bash), and the metrics count PowerShell writes
  (`powershell_write`).
- `.gitattributes` marks `eval/tasks/**` as `-text`, so checkouts keep CRLF and
  LF bytes even with `core.autocrlf=true`. Each temporary repository also sets
  `core.autocrlf=false`.
- What to look for: `windows-paths-escapes` (lost or doubled backslashes, a
  dropped BOM) and `crlf-and-lf` (CRLF converted to LF, or an added final
  newline in `VERSION`).

## Reading the results

Each evaluation writes `eval/results/<UTC time>/` (ignored by Git):

- `summary.md`: tables by arm and by task × arm (rates and means; spread as
  mean ± sd (median); errors and bytes), variance, fixed context overhead, tool
  calls per run, failed runs, and invalid runs. Records are sorted by task,
  arm, and repetition, so the summary does not depend on completion order.
- `runs.jsonl`: one record per run with every metric, in completion order;
  `runs.csv`: the same runs, one row each, scalar values only, with a fixed
  column order (lists and per-tool dictionaries stay in `runs.jsonl`);
  `manifest.json`: the configuration, every arm's specification, the Claude
  Code version, and the harness commit.
- `runs/<task>__<arm>__r<n>/`: the raw `stream.jsonl`, `stderr.txt`, the exact
  `settings.json`, `command.json`, and (third-party arms) `mcp.json`,
  `result.json`, and `diff.txt` for wrong bytes. The diff renders each line as
  a JSON string, so `\r`, tabs, and trailing spaces show. `mcp.json` holds the
  server's `env` values as configured.
- `plugin/`: the staged plugin that the runs used.

Metrics:

- **Correct**: exact final bytes, ignoring `.git/` and `.ultra-edit/`.
- **First try**: correct, with no failed edit call, rejected or non-committed
  Ultra Edit result, or Ultra Edit recovery call (undo, repair, retry,
  reconcile, inspect). A failed edit call includes a masked one
  (`masked_edit_failures`): the tool reported success, but its output shows a
  script traceback, a `git apply` or `perl` error, or an MCP server's
  `{"result": "error"}`. A shell edit whose only error is a missing viewer
  (`sed -i ... && xxd f` without `xxd`) is not a failed edit. A second native
  Edit that silently fixes an earlier successful one is not detected.
- **Tool calls, Edit calls, Tool errors**: edit calls are Edit, Write,
  MultiEdit, NotebookEdit, content-writing shell commands (Bash writes, below),
  other shell edits (`shell_edit_kinds`: `patch_apply` for `git apply` or
  `patch` from a file or pipe, `filter_redirect` for `sed`, `awk`, `perl`,
  `grep`, ... redirected into a file), the Ultra Edit commit tools, and a
  third-party arm's `edit_tools`. So a failed `git apply fix.patch` is a failed
  edit, while running a script file the model wrote (`python3 fix.py`) is only
  a shell call. Tool errors are results
  flagged `is_error`, plus Ultra Edit results showing `ready: false`,
  `kind: rejected`/`error`, or a commit that is not `committed`.
- **Shell calls, shell errors**: Bash and PowerShell calls, and those whose
  result is flagged `is_error` (a failed command, a hook denial, a quoting
  mistake). A failed shell edit usually costs a retry turn.
- **Reads**: `read_calls` counts Read and Ultra Edit snapshot calls;
  `reread_calls` counts those whose path was already read or snapshotted
  earlier in the run. `toolsearch_calls` counts ToolSearch calls (MCP tools are
  deferred behind it in recent Claude Code), `skill_calls` Skill calls.
- **Bash writes**: shell commands that match a heuristic (heredoc into a
  redirect, `tee`, `git apply`, or `patch`; `sed -i`, `perl -pi`, or
  `awk -i inplace`; `echo`/`printf` redirect; inline Python/Node scripts
  calling write APIs; PowerShell writers), shown as succeeded/attempted. In
  `native-guard`, attempts blocked by the hook also appear in `hook_denials`.
- **Turns, tokens, cost**: `num_turns`, `modelUsage` summed over all models
  (subagents included), and `total_cost_usd` from the final `result` message.
  **Input tok** is uncached input plus cache reads plus cache writes. Cost
  depends on cache hits, so compare it only across shuffled, repeated runs.
- **API calls and context**: Claude Code splits one API response into several
  stream lines that repeat the same usage, so calls are counted once per
  message id. Per call, context is input plus cache reads plus cache writes.
  `api_calls` (`subagent_api_calls` of them in subagents),
  `first_call_context_tokens`, `peak_context_tokens` (largest single request),
  `context_tokens_total` (**Context tok**: the sum over calls, the input the
  model processed regardless of cache state), `output_tokens_calls`, and
  `context_series` (`[context, output]` per main-thread call, for plotting
  growth). The output count in stream lines is a snapshot taken before each
  response finished, so `output_tokens_calls` and the output in
  `context_series` undercount (often by more than half); use `output_tokens`
  (`modelUsage`) for output. `context_tokens_total` matches `total_input_tokens`
  unless Claude Code made calls that do not appear in the stream (subagents
  do appear).
- **Bytes**: `tool_input_bytes` (UTF-8 length of each tool input as compact
  JSON) and `tool_result_bytes` (UTF-8 length of each result's text), with
  per-tool breakdowns in `tool_input_bytes_by_tool` and
  `tool_result_bytes_by_tool`, and `final_text_bytes` for the final answer.
  They show where context goes: large `old_string`s, whole-file reads, or
  verbose tool results.

Outcomes: `pass`, `fail`, and `timeout` are scored. `invalid` (the arm did not
load as intended) and `infra_error` (no session, authentication or API
failures) are excluded from rates.

Check correctness first, then first-try rate and tool errors. Then compare
Bash writes between `native` and `native-guard`, and cost per correct run
(**USD per correct**: total cost of scored runs over correct runs, so failures
count against an arm). Read a failure's `diff.txt` before drawing conclusions.

- **Spread**: each spread cell is mean ± sample standard deviation (median).
  A median well below the mean points to a few expensive runs, such as
  repeated retries after failed shell commands.
- **Variance**: per arm, the mean over tasks of the coefficient of variation
  (sd / mean) of cost and of context tokens across a task's repetitions; only
  tasks with at least two scored runs count. It measures how predictable an
  arm is on the same task, independent of the task's size. Use at least
  `--reps 3`.
- **Fixed context overhead**: per arm, the median context of the first API
  call: system prompt, tool definitions, appended prompt, and task prompt,
  before any work. Every later request carries it again, mostly as cache
  reads. Differences between arms come from tool definitions and prompts.

`python eval/run_eval.py --summarize eval/results/<dir>` rebuilds `summary.md`
and `runs.csv` from `runs.jsonl`; records from older harness versions lack the
new metrics and show `-` there.
`--rescore eval/results/<dir>` recomputes every run's metrics from its saved
`stream.jsonl` with the current harness, keeps the original as
`runs.jsonl.bak`, and rebuilds the summary. Outcomes and byte comparisons are
kept, because the temporary repositories no longer exist.

### Comparing arms and reading transcripts

`summary.md` averages raw costs, so one expensive task can dominate an arm's
mean. `python3 eval/stats.py eval/results/<dir> > stats.md` pairs arms task by
task instead: it reports correctness and first-try rates with Wilson 95%
intervals, then each arm's cost, context, output tokens, turns, and calls as
the geometric mean of per-task ratios against `native` and against
`ultra-edit`, with a bootstrap 95% interval from resampling repetitions within
each task. Several directories are pooled; `DIR:SUFFIX` renames that
directory's arms (`ultra-edit-SUFFIX`) so two builds of one arm can be compared.

`python3 eval/digest.py eval/results/<dir> OUT_DIR` writes one short text file
per run: every API call's context size, the assistant's text, each tool call's
input size and abbreviated input, each result's size and error flag, the final
message, and the byte diff of a failed run. The digests are small enough to read
dozens of runs side by side.

`python3 eval/facts.py eval/results/<dir>` writes `facts.jsonl` and `facts.md`:
behaviour counted from the transcripts, so analysis does not recount by
reading. Every tool call becomes a step with the API call that issued it, a
kind (read, search, inspect, edit, verify), shell features (`git diff`, a
diffstat, byte views such as `cat -A`, tests, asserts), and an error class. Per
run it gives the edit channels (`Edit`, `mcp:edit_file`, `bash:python`,
`bash:git_apply`, ...), the checks made before the first and after the last
edit, masked failures, and each recovery from a failed edit to the next
successful one with the context tokens it cost. A masked failure is one the
tool reported as success: a script's traceback hidden by a later command's exit
status, a `git apply` error printed by a command that still exited 0, or an
error inside an MCP result. A shell edit whose only failure is a missing viewer
(`... && xxd file` without `xxd`) is not a failed edit.

## Adding a task

Create `tasks/<name>/prompt.md`, `fixture/`, and `expected/`. State the exact
old and new text, so exactly one byte-level result is correct. Name each
changed file in backticks, and use a prompt that needs no formatting judgment.
`python -m unittest discover -s scripts/tests` checks the task: expected files
exist in the fixture and differ from it, and each keeps its newline style, BOM,
and final newline.
