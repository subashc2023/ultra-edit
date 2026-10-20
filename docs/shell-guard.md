# Shell-write guard

The plugin registers a `PreToolUse` hook for Claude Code's Bash, PowerShell,
and Write tools. It parses each command and denies ones that write content
embedded in the command into project files:

- heredocs, here-strings, and `echo`/`printf`, `Write-Output`, or string
  literals redirected, `tee`d, or passed to `Set-Content`, `Add-Content`,
  `Out-File`, or `New-Item -Value`;
- inline Python, Node, Perl, Ruby, PHP, or PowerShell code that calls file-write
  APIs, including .NET's `[IO.File]::WriteAllText`;
- in-place editors such as `sed -i` and `perl -pi`, and `Get-Content` text
  rewritten with `-replace` or `.Replace()` and written back;
- patches or edit JSON piped into `patch`, `git apply`, or `ultra-edit`;
- scripts that may write project files: ones that call those write APIs, or
  move or copy files, on a target not certainly outside the project, or run
  such commands. A script is judged when Write or a shell command saves it in
  a temporary directory and its `#!` line or extension (`.py`, `.js`, `.pl`,
  `.rb`, `.php`, `.ps1`, `.sh`) names its language, and when a command runs a
  script it saved anywhere (`cat > /tmp/edit.py <<'EOF' … EOF; python3
  /tmp/edit.py`, `source`, or by path). No file is read from disk, so
  downloaded installers, a project's own tools, and other repositories'
  sources are never judged on their own.

Script and inline code targets are resolved through string literals,
f-strings and template literals, `+` and path joins (`os.path.join`,
`Path('/tmp') / name`), names assigned once, and the temporary-directory,
home-directory, and stream APIs (`tempfile`, `os.tmpdir()`,
`Path.home()`, `os.devnull`, pytest's `tmp_path`). A target counts as outside
when it resolves to a path, or a leading directory, outside the project, so a
benchmark that writes `f'/tmp/bench_{n}.json'` is allowed; command-line
arguments, loop variables, and parameters count as inside.

Only writes that may land in the project count. The project is
`CLAUDE_PROJECT_DIR`, or the hook event's working directory. Targets certainly
outside it are allowed: `$GITHUB_OUTPUT` and the other runner files, `/dev/null`
and `$null`, and absolute paths elsewhere such as `/etc/hosts`, `~/.bashrc`, or
`$TMPDIR` and `$env:TEMP` files, including ones named through a variable the
command assigns once (`OUT=/tmp/o.txt`, `t=$(mktemp)`) or after a known leading
directory (`/tmp/out_$i.txt`). Paths are compared lexically, and Windows
drive, UNC, and Git Bash `/c/…` paths ignore case. Relative paths, other
variables, and expansions that may lead into the project count as inside.

The deny reason tells Claude to use Ultra Edit, native Edit, or Write instead.
Ordinary output redirection (`cargo test > log.txt`, `git diff > d.patch`),
plain copies, and heredocs passed to commands that don't write them to files
(`git commit -F -`, Claude Code's `git commit -m "$(cat <<'EOF' …)"` pattern)
are allowed. The guard allows anything it cannot parse and is not a sandbox: it
misses dynamic commands, redirects on grouped commands, rewrites through
temporary files, scripts it did not see saved (downloaded, edited after saving,
or saved without a `#!` line or extension and run by a later call), and
symlinks into the project.

To turn the guard off, set `ULTRA_EDIT_SHELL_WRITES=allow` in Claude Code's
environment, for example `"env": {"ULTRA_EDIT_SHELL_WRITES": "allow"}` in
`settings.json`. Setting it inside a shell command has no effect.
