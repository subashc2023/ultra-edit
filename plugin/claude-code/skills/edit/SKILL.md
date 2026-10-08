---
name: edit
description: Ultra Edit recovery and advanced use; ordinary edits need only the routing card. Use for span targets, previews, repair, undo, lost responses, or uncertain outcomes.
---

# Ultra Edit

The plugin loads its [routing card](../../instructions.md) automatically in main
sessions and subagents; invoking this skill is not a prerequisite for an edit.
Explicit user instructions and host permissions take precedence; report
conflicting editing guidance or unavailable tools rather than silently switching
routes. Never carry file contents or edit JSON through Bash heredocs,
generated-content redirection, or inline editing scripts.

The server-side names are below; select the corresponding namespaced tools
exposed by this host.

- Name each file by its absolute `path`. The server reads the file's current
  bytes under its lock as the base, so every change resolves against the file as
  it was when the call started, never another change's output. The server root
  stays fixed after directory changes or a subagent's worktree change; never use
  a relative path that would redirect a worktree edit to the parent checkout,
  and report files outside the root.
- Replacement text is literal. Preserve intended Unicode, whitespace, and
  newline bytes; no regex or formatting runs. One exception: in a file whose
  lines all end in CRLF, which Read shows without `\r`, text holding LF but no
  CR is matched and written as CRLF, with an `EOL_ADAPTED` warning, unless the
  change's `old` holds a `\r`.
- Omit `request_id` and change `id`. Identical arguments derive the same IDs, so
  after lost output, repeat the identical call: it returns the recorded result,
  marked `replayed: true`, without writing again. A `REPLAYED_FILE_CHANGED`
  warning means the file changed since that commit; pass a new `request_id` only
  if the change is still needed. A rejected path request is not recorded, so
  resending it after fixing the cause evaluates it again.
- Confirm `commit: "committed"` before reporting all edits applied. `partial`
  and `outcome_unknown` require recovery; stop further mutations on uncertainty.
  Current bytes matching the candidate do not prove historical success.

## Normal flow

1. Read the files with native Read, Grep, or Glob.
2. Call `ultra_edit` once with every related change, across files:

   ```json
   {
     "files": [{
       "path": "/abs/src/retry.rs",
       "changes": [
         { "old": "const RETRIES: usize = 2;", "new": "const RETRIES: usize = 3;" },
         { "lines": [40, 42], "expect": ["fn backoff(attempt: u32) {", "}"], "new": "" },
         { "after": 3, "expect": "use std::time::Duration;", "new": "use std::thread;" }
       ]
     }]
   }
   ```

   `old` must occur exactly once in the file; add neighbouring text until it
   does, or restrict it to whole lines with `"in": [first, last]`. With
   `"count": N` it replaces exactly N non-overlapping occurrences. `lines`
   replaces whole lines by Read's numbers (`""` deletes them; text without a
   final newline keeps the last line's ending) and `after: n` inserts whole lines
   after line n (0 is the top). Both need `expect`, compared without line
   endings: line n's current text, line a's for a one-line range, or
   `[line a, line b]` (or every line) for a longer one, with 8 or more visible
   characters or text found nowhere else in the file, so a stale line number is
   rejected, not applied.
3. Inspect the outcome. A rejection wrote nothing; its diagnostics can list
   `candidates` with the current text and lines. Copy an `exact` or `whitespace`
   candidate's `text` verbatim into `old`, confirm a `similar` one is the
   intended region, and resend the whole call (or call `ultra_edit_repair` with
   the draft `reference`, replacing only the failed `change_id`). Run project
   validation separately; the engine runs no tests.

## Span targets

A file too large to Read, or an edit that should be pinned to an inspected
region, can use a snapshot instead of `path`. Call `ultra_edit_snapshot` with a
focused `selection`: an inclusive line `range`, or a literal `search`. Range and
full reads summarize disclosed IDs in `spans` (`["r12..r18", "selection"]`) and
list each line in `lines` as `"r12 | const retries = 2;"`; pick `r{n}` from
that listing rather than counting newlines. Pass the returned `snapshot` as the
file's `base` and target a disclosed span, `{"span": "r12..r14", "expect": ...,
"new": ...}` (`expect` is byte-exact here), or restrict `old` to one with
`"in": "r12"`. Line targets on lines this base disclosed need no `expect`. To reach a distant region of the same file, continue the
read with the previous `snapshot`; the new snapshot keeps every reference
already disclosed. `stale: true` means read again. `full` defaults to 24,000
bytes and 400 lines; larger reads need an exact `expected_bytes`.

## Read only the reference needed

| Task or symptom | Reference |
| --- | --- |
| Request identity and replay, byte preservation, limits, or what a receipt proves | [Contract](references/contract.md) |
| Choose range/search/full; continue a range; ambiguous matches; near-miss candidates; replace-all; span overlap | [Targets](references/targets.md) |
| Preview/diff then commit; correct a batch; lost response; preflight retry; inspect/reconcile uncertainty; undo | [Recovery](references/recovery.md) |
| Connect the plugin; missing tools; fixed workspace root; host permissions; blocked shell command | [Claude Code](references/claude-code.md) |

The plugin grants no permissions; follow the user's existing authorization and
this host's permissions. Its PreToolUse hook denies Bash and PowerShell commands
that write file content into the project, such as heredoc, here-string, `echo`,
or `Set-Content` writes, inline interpreter writes, `sed -i`, and `-replace`
rewrites; redo a blocked write with Ultra Edit, Edit, or Write, never by
rephrasing the command to evade the guard.
File creation, deletion, and rename are outside this tool's scope. Use native
Write for new files or isolated full rewrites, and native Edit or Ultra Edit for
isolated targeted edits. Do not split a coordinated multi-file edit into native
calls to avoid its required route.
