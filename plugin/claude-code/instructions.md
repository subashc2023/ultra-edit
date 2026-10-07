# Ultra Edit

Edit existing text files with the `ultra_edit` MCP tool (its name ends in
`__ultra_edit`). Explore with native Read/Grep/Glob; ordinary edits need no
`ultra_edit_snapshot`.

Put ALL related changes, across all files, in ONE call. Name each file by the
same absolute `path` you Read: the server root never follows `cd` or a worktree.
{"files":[{"path":"/abs/src/app.py","changes":[
 {"old":"retries = 2","new":"retries = 3"},
 {"old":"cfg","new":"config","count":2},
 {"after":1,"expect":"import os","new":"import re"},
 {"lines":[7,8],"expect":["def main():","    run()"],"new":"def main(argv):\n    run(argv)"}]}]}
- `old` is copied verbatim from Read, without the line-number gutter, and must
  occur exactly once: add context, or limit it with `"in":[first,last]`.
  `"count":N` replaces exactly N occurrences.
- `lines:[a,b]` replaces those whole lines (Read's numbers); `"new":""` deletes
  them. `after:n` inserts lines after line n (0 = top). Both need `expect`: the
  current text of line n or a, or `[line a, line b]` for a range, matching
  nowhere else in the file.
- Every change applies to the file as it was when the call started, never to
  another change's output. Text is literal: no whitespace or quote changes. In a
  file whose lines all end in CRLF, LF in your text is written as CRLF.

Results: success has `commit: "committed"`. `rejected` wrote nothing: fix only
the listed changes (diagnostics show the closest current text and lines) and
resend the whole call. On `partial` or `outcome_unknown`, stop and report; never
resend under a new `request_id`. An identical call replays its recorded result
and writes nothing; a `REPLAYED_FILE_CHANGED` warning means it was applied
before and the file has changed since.

Never write project files through the shell (heredocs, echo/printf redirects,
`sed -i`, inline scripts); a hook denies them. Use Write for new files; a single
isolated replacement may use native Edit. Paths outside the server root are
rejected: report it. User instructions take precedence.
