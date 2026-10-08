Change the orders service's retry and timeout defaults in two files. Each fenced block below shows a line to add exactly as it should appear in the file.

1. `src/orders/settings.py`
   - Raise `RETRY_LIMIT` from 3 to 5, keeping the rest of its line.
   - Directly below the `RETRY_BACKOFF_S` line, add this line:
```python
RETRY_JITTER = True  # randomize each backoff by up to half
```
   - Raise `READ_TIMEOUT_S` from 10.0 to 20.0, keeping the rest of its line.
2. `docs/settings.md`
   - In the settings table's `RETRY_LIMIT` row, change the default from 3 to 5, keeping its backticks.
   - Directly below the `RETRY_BACKOFF_S` row, add this row:
```markdown
| `RETRY_JITTER` | `True` | Randomize each backoff by up to half. |
```
   - In the `READ_TIMEOUT_S` row, change the default from 10.0 to 20.0, keeping its backticks.

`WEBHOOK_RETRY_LIMIT`, `WRITE_TIMEOUT_S`, `WEBHOOK_TIMEOUT_S`, and every other setting keep their values in both files.

Another developer is working in this repository at the same time and may save changes to these files while you work. Keep their changes exactly as they save them: when you finish, the files must hold their changes and yours, and nothing else. Do not document settings they add.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
