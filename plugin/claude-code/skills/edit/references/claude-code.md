# Claude Code host

## Install from a release

Claude Code 2.1.224 or newer can install the complete, SHA-256-pinned release
archive without Git, Rust, or a source checkout:

```text
claude plugin marketplace add https://github.com/subashc2023/ultra-edit/releases/latest/download/marketplace.json
claude plugin install ultra-edit@ultra-edit
```

The default scope is the current user, so the plugin is available across
projects. Start a new Claude Code session from the workspace to edit, run `/mcp`,
and confirm that the `ultra-edit` server is connected. Claude Code identifies
this installation as `ultra-edit@ultra-edit`.

The release archive keeps the manifest, MCP configuration, hooks, instructions,
skill, licenses, and all supported native executables together. Windows resolves
the adjacent x64 `.exe`; the Unix launchers select Linux x64 or ARM64 and macOS
Intel or Apple Silicon. The plugin adds no global command and needs no `PATH`
change.

Refresh the catalog and plugin when a new release is available:

```text
claude plugin marketplace update ultra-edit
claude plugin update ultra-edit@ultra-edit
```

Restart Claude Code or run `/reload-plugins` after updating. The catalog's stable
URL selects the latest non-prerelease GitHub release, while each catalog entry
names a versioned archive and pins its exact SHA-256 digest.

## Connect a source build

For development or an unreleased commit, build the two private executables with
Rust 1.89 or newer from the Ultra Edit repository:

```text
cargo build --locked --release
```

Stage them inside the plugin. PowerShell:

```powershell
New-Item -ItemType Directory -Force -Path './plugin/claude-code/runtime' | Out-Null
Copy-Item -LiteralPath './target/release/ultra-edit.exe', './target/release/ultra-edit-mcp.exe' -Destination './plugin/claude-code/runtime' -Force
```

macOS/Linux:

```sh
mkdir -p ./plugin/claude-code/runtime
cp ./target/release/ultra-edit ./target/release/ultra-edit-mcp ./plugin/claude-code/runtime/
```

The plugin launches these files directly. No global executable installation or
`PATH` change is required. Release archives already contain the executables and
skip these source-build steps. Generated `runtime/` files are not committed to
source control.

For persistent use across projects, copy the entire prepared
`plugin/claude-code` directory to `~/.claude/skills/ultra-edit`. On Windows the
default destination is `%USERPROFILE%\.claude\skills\ultra-edit`. If
`CLAUDE_CONFIG_DIR` is set, use its `skills/ultra-edit` directory instead.

For a first installation, run from the Ultra Edit repository. PowerShell:

```powershell
$ultraEditConfig = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $env:USERPROFILE '.claude' }
$ultraEditPlugin = Join-Path $ultraEditConfig 'skills/ultra-edit'
if (Test-Path -LiteralPath $ultraEditPlugin) { throw 'Plugin destination already exists; update that copy explicitly.' }
New-Item -ItemType Directory -Force -Path (Join-Path $ultraEditConfig 'skills') | Out-Null
Copy-Item -LiteralPath './plugin/claude-code' -Destination $ultraEditPlugin -Recurse -Force
```

macOS/Linux:

```sh
ultra_edit_config="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
ultra_edit_plugin="$ultra_edit_config/skills/ultra-edit"
mkdir -p "$ultra_edit_config/skills"
mkdir "$ultra_edit_plugin" && cp -R ./plugin/claude-code/. "$ultra_edit_plugin/"
```

The destination must be a plugin directory containing `.claude-plugin`,
`.mcp.json`, `runtime`, `hooks`, `skills`, and the other bundled files. Preserve
hidden files; copying only `skills/edit` would omit automatic routing, the MCP
connection, and the executables.
Then launch normally **from the workspace to edit**:

```text
claude
```

Claude Code discovers a copied source build as `ultra-edit@skills-dir` across
projects, without a marketplace or slash command. The bundled personal
installation was tested with Claude Code 2.1.263, with no global Ultra Edit
executable. See the official
[skills-directory plugin documentation](https://code.claude.com/docs/en/plugins-reference#skills-directory-plugins).

To test only one session without copying the prepared plugin, pass its absolute
directory:

```text
claude --plugin-dir /absolute/path/to/ultra-edit/plugin/claude-code
```

For example, on Windows:

```powershell
claude --plugin-dir "$(Resolve-Path -LiteralPath './plugin/claude-code')"
```

`--plugin-dir` loads this local plugin for the session. Its `.mcp.json` points to
`${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp` and supplies
`["--root", "${CLAUDE_PROJECT_DIR}"]` as separate arguments. Claude Code substitutes
the plugin and project roots; Windows native launch resolves the `.exe` suffix.
The context hooks use the same private executable with explicit argument arrays,
so hook launches do not pass through a shell. The server canonicalizes its root
once. The plugin directory and later shell working directories do not select
the edit workspace. See the official
[MCP configuration documentation](https://code.claude.com/docs/en/mcp) and
[plugin reference](https://code.claude.com/docs/en/plugins-reference).

Check `/mcp` for the plugin server, then make an ordinary multi-file edit request.
The plugin automatically supplies its routing instructions; `/ultra-edit:edit`
is optional workflow detail. Use the actual MCP tool names Claude Code exposes;
plugin tools may be namespaced.
The core names are `ultra_edit_snapshot`, `ultra_edit`, and `ultra_edit_status`.
Preview/commit, repair, preflight retry, undo, diff, inspection, and explicit
reconciliation have separate tools. See the official
[local plugin testing instructions](https://code.claude.com/docs/en/plugins#test-your-plugins-locally).

Copied source plugins do not update automatically. Update the installed plugin
copy manually when updating the source, after rebuilding and staging matching
native executables in `runtime/`. Marketplace installations instead use the two
update commands under [Install from a release](#install-from-a-release).
Do not nest a second `claude-code` directory inside the existing plugin. This
source-copy procedure is separate from the versioned marketplace cache.

## Automatic routing context

The [canonical instructions](../../../instructions.md) require direct Ultra Edit
MCP calls for coordinated edits to two or more existing UTF-8 files. They forbid
writing file contents through Bash heredocs, generated-content redirection,
inline editing scripts, and shell-piped edit JSON. Native Write serves new files
or isolated full rewrites; native Edit or Ultra Edit serves isolated targeted
edits. Missing or denied required tools are a blocker to report.
Explicit user instructions and host permissions take precedence over plugin
guidance. Report contradictory workflow instructions; generic sed/heredoc advice
does not by itself cancel the user's explicit choice of Ultra Edit.

The shell rule responds to a user's report on 2026-09-05 that Bash tool payloads
lost backslashes upstream of shell parsing, even with quoted heredocs. This is
not a claim that Ultra Edit reproduced the issue on all Claude hosts. Replacement
text travels directly in MCP arguments with normal JSON escaping; do not apply
Bash backslash-doubling workarounds to it.

The plugin's [hooks](../../../hooks/hooks.json) inject the policy at
`SessionStart` with no matcher, including startup, resume, clear, compaction,
and fork. `SubagentStart` injects it into delegated agents. This is automatically
loaded prompt context, not a tool-enforcement boundary. No native tool is
disabled. See the official
[hook reference](https://code.claude.com/docs/en/hooks#sessionstart).

Inspect the exact installed context in PowerShell:

```powershell
$ultraEditConfig = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $env:USERPROFILE '.claude' }
$ultraEditMcp = Join-Path $ultraEditConfig 'skills/ultra-edit/runtime/ultra-edit-mcp.exe'
& $ultraEditMcp --claude-context SessionStart
& $ultraEditMcp --claude-context SubagentStart
```

macOS/Linux:

```sh
"${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/ultra-edit/runtime/ultra-edit-mcp" --claude-context SessionStart
"${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/ultra-edit/runtime/ultra-edit-mcp" --claude-context SubagentStart
```

These commands emit hook JSON and exit without opening a workspace. The binary
embeds the instructions at build time, so rebuild and stage matching executables,
update the entire plugin copy, then start a new Claude session. Bundling a
plugin-root `CLAUDE.md` would not load these instructions automatically; the
plugin does not edit the user's `CLAUDE.md` or settings files.

## Root and permissions

Each server uses one fixed root and workspace-local `.ultra-edit` state.
It never follows a shell `cd` or a subagent into a separate worktree. When working
elsewhere, pass the intended file's absolute path to snapshot. If that file lies
outside the server root, report the blocker; never use a relative path that would
edit the parent checkout instead. Editing a different root requires a separately
configured server or session for that workspace.

Native Read/Grep/Glob can help exploration, but only an Ultra Edit snapshot
supplies the immutable base needed to edit. Do not substitute a filesystem path,
digest, native Read result, or guessed reference for that base.

MCP tool permissions are controlled by Claude Code. Ultra Edit enforces its own
canonical-root confinement; it does not inherit native Edit's per-path permission
rules. This plugin contains no `allowed-tools` grant, permission settings, or hook
that disables native tools. Do not broaden permissions or switch roots to bypass
a host denial. See Claude Code's
[MCP permission rules](https://code.claude.com/docs/en/permissions).

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Marketplace rejects the archive source | Run `claude --version` and update Claude Code to 2.1.224 or newer before adding the release catalog again. |
| Archive integrity check fails | Refresh the `ultra-edit` marketplace and retry. Do not bypass the SHA-256 check; report the release version and digest if it still fails. |
| Server executable missing or cannot launch | Confirm the installed plugin contains the matching OS/architecture executable under `runtime/`, run that full path with `--help`, and rebuild/stage or replace the incomplete package. Unix executables must retain executable permission. No `PATH` change is needed. |
| Operating system blocks a downloaded executable | Current release binaries are unsigned. Use a reviewed source build on that machine; signing and macOS notarization are required before broader distribution. |
| Plugin or skill absent | Confirm the absolute plugin directory contains `.claude-plugin/plugin.json`, `.mcp.json`, `runtime/`, `hooks/hooks.json`, and `skills/edit/SKILL.md`; run `claude plugin validate PATH`. |
| Tools not connected | Inspect `/mcp`; after changing the plugin, use `/reload-plugins` or restart the session. |
| Routing instructions absent or stale | Run the installed runtime executable with `--claude-context SessionStart` as above; rebuild and stage matching binaries if the option is missing or the text is stale, confirm the plugin's hooks are enabled, and start a new session. |
| A reference is missing | Verify that this session uses the same canonical root as the session that created it; references are not global. |
| File lies outside the root | Start a separately configured session at the intended workspace; do not accept a root from model tool arguments. |
| Host permission denied | Respect the denial and report the blocked action; snapshot freshness is not authorization. |
| Malformed JSON message | The server answers `-32700` and continues; fix JSON escaping. Oversized or invalid-UTF-8 frames are fatal and exit nonzero; reconnect and inspect any in-flight receipt. |
| Conflicting shell-editing instructions | Follow explicit user instructions and host permissions, report the conflict, and avoid silently replacing the requested Ultra Edit route. |

The hooks and skill provide guidance, not a guarantee that Claude chooses the
required tool. The server's tool descriptions and schemas also state the critical
editing contract. The server still validates requests and original bases for
every edit made through Ultra Edit.
