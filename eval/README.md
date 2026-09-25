# Model-level evaluation

`run_eval.py` measures whether Ultra Edit beats native Claude Code editing on
small, deterministic repositories: exact final bytes, first-attempt success,
tool calls, tokens, cost, and shell writes. It runs headless `claude -p`
sessions, saves each raw stream-json transcript, and scores the final bytes.

> **Cost warning.** Every run is a real Claude Code session billed to your
> account: 6 tasks × 3 arms × `--reps` sessions. `--dry-run` and `--preflight`
> never call a model. Paid runs need confirmation (or `--yes`), each run is
> capped by `--max-budget-usd` (default 2), and `--max-total-usd` stops the
> evaluation once recorded spend reaches a limit. If `ANTHROPIC_API_KEY` is set,
> `claude -p` always bills that key, even when you are logged in.

## Arms

| Arm | What loads | What it measures |
| --- | --- | --- |
| `native` | Claude Code only | The baseline: native Read/Edit/Write/Bash. |
| `native-guard` | Native tools plus Ultra Edit's shell guard: a `PreToolUse` hook (matcher `Bash\|PowerShell`, set by `--guard-matcher`) running `<staged runtime>/ultra-edit-mcp --claude-hook PreToolUse`, configured in a generated `--settings` file | Whether blocking heredoc, here-string, inline-script, `sed -i`, `-replace`, and `echo`/`printf`/`Set-Content` writes into the project alone improves native editing. |
| `ultra-edit` | The full plugin via `--plugin-dir <staged plugin>`: MCP tools, routing hooks, skill | The complete product against both baselines. |

Every arm gets the same prompt, flags, and isolation; only the plugin or hook
differs. Arm order is shuffled per task and repetition (`--seed`) to spread
prompt-cache effects across arms.

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

## Setup

1. Install Git and Python 3.10 or newer. Claude Code must be 2.1.139 or newer;
   the flags were verified against 2.1.282.
2. Build the runtime from the repository root:
   `cargo build --locked --release`. The harness stages a copy of
   `plugin/claude-code` with the executables in its `runtime/` directory
   inside the results folder. Pass `--runtime-dir DIR` to use another build,
   such as an extracted release's `runtime/`. The `native-guard` arm needs a
   build that supports `--claude-hook PreToolUse`; `--preflight` feeds it a
   Bash heredoc write and a read, plus a PowerShell here-string write and a
   read when the matcher names PowerShell, and fails if the hook doesn't deny
   the writes and allow the reads.
3. Authenticate. The default mode uses your normal login. With
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
  `ultra-edit@skills-dir`). For `native-guard` it also holds the hook.
- `--no-session-persistence`, plus these environment variables:
  `DISABLE_AUTOUPDATER=1`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`,
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `CLAUDE_CODE_SKIP_PROMPT_HISTORY=1`.
  The harness removes `CLAUDE_CODE_PLUGIN_DIRS` and the markers a parent
  Claude Code session exports (`CLAUDECODE`, ...).

Every run is then checked against its `system/init` message and hook events:

- A native arm that shows any Ultra Edit plugin, MCP server, or tool is marked
  `invalid`.
- An `ultra-edit` run whose plugin or server did not load, or whose
  `SessionStart` hook failed, is marked `invalid`.
- A `native-guard` run is marked `invalid` if a shell tool that the matcher
  names ran without any `PreToolUse` hook event, or if the hook failed.

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
for Bash, the file tools, Skill, and the Ultra Edit MCP server.

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
every arm uses the same model. Other options: `--max-turns` (default 50),
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

- `summary.md`: tables by arm and by task × arm, tool calls per run, failed
  runs, and invalid runs.
- `runs.jsonl`: one record per run with every metric; `manifest.json`: the
  configuration, Claude Code version, and harness commit.
- `runs/<task>__<arm>__r<n>/`: the raw `stream.jsonl`, `stderr.txt`, the exact
  `settings.json` and `command.json`, `result.json`, and `diff.txt` for wrong
  bytes. The diff renders each line as a JSON string, so `\r`, tabs, and
  trailing spaces show.
- `plugin/`: the staged plugin that the runs used.

Metrics:

- **Correct**: exact final bytes, ignoring `.git/` and `.ultra-edit/`.
- **First try**: correct, with no failed edit call, rejected or non-committed
  Ultra Edit result, or Ultra Edit recovery call (undo, repair, retry,
  reconcile, inspect). A second native Edit that silently fixes an earlier
  successful one is not detected.
- **Tool calls, Edit calls, Tool errors**: edit calls are Edit, Write,
  MultiEdit, NotebookEdit, content-writing shell commands, and the Ultra Edit
  commit tools. Tool errors are results flagged `is_error`, plus Ultra Edit
  results showing `ready: false`, `kind: rejected`/`error`, or a commit that
  is not `committed`.
- **Bash writes**: shell commands that match a heuristic (heredoc into a
  redirect, `tee`, `git apply`, or `patch`; `sed`/`perl -i`; `echo`/`printf`
  redirect; inline Python/Node scripts calling write APIs; PowerShell
  writers), shown as succeeded/attempted. In `native-guard`, attempts blocked
  by the hook also appear in `hook_denials`.
- **Turns, tokens, cost**: `num_turns`, `modelUsage` summed over all models
  (subagents included), and `total_cost_usd` from the final `result` message.
  **Input tok** is uncached input plus cache reads plus cache writes. Cost
  depends on cache hits, so compare it only across shuffled, repeated runs.

Outcomes: `pass`, `fail`, and `timeout` are scored. `invalid` (the arm did not
load as intended) and `infra_error` (no session, authentication or API
failures) are excluded from rates.

Check correctness first, then first-try rate and tool errors. Then compare
Bash writes between `native` and `native-guard`, and cost per correct run.
Read a failure's `diff.txt` before drawing conclusions.
`python eval/run_eval.py --summarize eval/results/<dir>` rebuilds `summary.md`
from `runs.jsonl`.

## Adding a task

Create `tasks/<name>/prompt.md`, `fixture/`, and `expected/`. State the exact
old and new text, so exactly one byte-level result is correct. Name each
changed file in backticks, and use a prompt that needs no formatting judgment.
`python -m unittest discover -s scripts/tests` checks the task: expected files
exist in the fixture and differ from it, and each keeps its newline style, BOM,
and final newline.
