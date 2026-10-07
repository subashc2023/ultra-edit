# Third-party comparison arms

`arms.json` defines evaluation arms that replace Claude Code's native file
tools with a popular third-party MCP edit server, so `run_eval.py` can compare
them with native editing, shell editing, and Ultra Edit on the same tasks.

| Arm | Server | Edit tools | Tools / schema size |
| --- | --- | --- | --- |
| `mcp-filesystem` | [`@modelcontextprotocol/server-filesystem`](https://www.npmjs.com/package/@modelcontextprotocol/server-filesystem) 2026.8.31, the MCP reference server | `edit_file` (list of `oldText`/`newText`), `write_file` | 14 tools, 13.7 KB |
| `desktop-commander` | [`@wonderwhy-er/desktop-commander`](https://www.npmjs.com/package/@wonderwhy-er/desktop-commander) 0.2.52 | `edit_block` (`old_string`/`new_string`, `expected_replacements`), `write_file` | 26 tools, 58.4 KB |
| `mcp-text-editor` | [`mcp-text-editor`](https://pypi.org/project/mcp-text-editor/) 1.2.2 | `patch_text_file_contents` (hash-checked line ranges) and insert/delete/append | 6 tools, 6.1 KB |

For scale, Ultra Edit 0.3.0 exposes 11 tools with 17.6 KB of schemas, plus
1.4 KB of server instructions and 4.3 KB of `SessionStart` context. Schema
sizes are the compact JSON of each server's `tools/list` response.

Each arm removes `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` and appends a
one-sentence system prompt naming the server's edit tool, so the model uses the
server rather than native tools. Bash stays available, as it would for a real
user; shell writes are still counted.

## Setup

```sh
eval/third_party/setup.sh            # installs into eval/third_party/install
python3 eval/run_eval.py --preflight --arm mcp-filesystem --arm desktop-commander --arm mcp-text-editor
```

The script needs Node.js 18+, npm, and Python 3.10+ (it uses `uv` when
present). It installs the versions pinned in `package.json`,
`package-lock.json`, and `requirements.txt`, with npm install scripts disabled,
so Desktop Commander's install-tracking script does not run. Pass another
directory as the first argument and give the same directory to `run_eval.py
--third-party-dir`.

Desktop Commander needs extra isolation:

- Its arm sets `HOME` to `install/dc-home`, so it never reads or changes your own
  `~/.claude-server-commander` configuration.
- `DESKTOP_COMMANDER_DISABLE_TELEMETRY=1` turns telemetry off, and the setup
  script clears the welcome-onboarding flags in the arm's config.
- On every start it downloads Chrome, for its PDF tools, unless a copy is
  cached. The setup script downloads it once (about 400 MB) so that
  parallel runs don't race on the same download. Set
  `ULTRA_EDIT_EVAL_SKIP_CHROME=1` to skip that step.

All Desktop Commander runs share `install/dc-home`, including its tool-call log.
Its configuration allows every directory, as it does by default.

## Behavior observed during setup

Direct JSON-RPC probes, before any model run, on Linux:

- `server-filesystem`'s `edit_file` rewrote a CRLF file with LF line endings
  everywhere, including lines it did not change.
- `mcp-text-editor` reads files in text mode, so a CRLF file's content is shown
  with LF line endings.
- Desktop Commander's `edit_block` kept CRLF line endings, and its failure message
  reported the closest fuzzy match with a similarity score.

## Adding a server

Add an entry to `arms.json`:

- `server_name`: the MCP server name; tools are `mcp__<server_name>__<tool>`.
- `mcp_server`: a stdio server definition. `{repo}` is replaced by the run's
  temporary repository and `{third_party_dir}` by the install directory.
- `edit_tools`: tool names counted as edit calls.
- `required_tools`: tool names that must be listed at session start, or the run
  is marked `invalid`.
- `append_system_prompt` and `disallowed_tools`: how the arm routes edits.

Pin the package in `package.json` and `package-lock.json`, or
`requirements.txt`, and install it in `setup.sh`.
