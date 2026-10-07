# Ultra Edit

Edit existing text files with the `ultra_edit` MCP tool (its name ends in
`__ultra_edit`). Explore with native Read/Grep/Glob; ordinary edits need no
`ultra_edit_snapshot`.

Put ALL related changes, across all files, in ONE call. Name each file by the
same absolute `path` you Read: the server root never follows `cd` or a worktree.
{"files":[{"path":"/abs/src/a.py","changes":[
 {"target":{"kind":"exact","old":"retries = 2"},"text":"retries = 3"},
 {"target":{"kind":"all","old":"cfg","scope":"r0","expected":2},"text":"config"}]}]}
- `old` is copied verbatim from Read, without the line-number gutter, and must
  occur exactly once; add neighbouring lines until it does. `all` replaces
  exactly `expected` occurrences in `scope` (`r0` is the whole file).
- To insert, put a neighbouring line in `old` and keep it in `text`; to delete,
  leave the lines out of `text`.
- Every change applies to the file as it was when the call started, never to
  another change's output. Text is literal: no whitespace, quote, or line-ending
  changes. Read hides `\r`: in a CRLF file, multi-line text needs `\r\n`.

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
