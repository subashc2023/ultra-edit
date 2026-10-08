Remove the deprecated `legacy-export` handler and fix the order of definitions in the worker. Three files change: `app/handlers.py`, `app/registry.py`, and `docs/handlers.md`. Every other file, including `app/__init__.py`, `app/models.py`, and `tests/test_handlers.py`, stays exactly as it is.

`app/handlers.py` and `app/registry.py` use LF line endings. `docs/handlers.md` uses Windows CRLF line endings; keep them. A "blank line" below means an empty line with no spaces.

## 1. `app/handlers.py`

**1a. Import order.** Reorder these three consecutive lines

```python
from app.models import HandlerResult, Job, JobStatus
from app.audit import record_event
from app.config import settings
```

into exactly this order (the line `from app.storage import open_output, upload` stays directly after them):

```python
from app.audit import record_event
from app.config import settings
from app.models import HandlerResult, Job, JobStatus
```

**1b. `__all__`.** Delete the line `    "legacy_export",` (4 spaces of indentation) from the `__all__` list. Every other entry stays, including `"RetryPolicy"`.

**1c. Delete `legacy_export`.** Delete the 16 lines from the comment line `# Deprecated since 2.0: use export-csv. Scheduled for removal in 3.0.` through the last line of `legacy_export`, `    return HandlerResult(status=JobStatus.SUCCEEDED, output=url)` (the first line with that exact text after the comment), inclusive. This removes the comment, the decorator lines `@deprecated("legacy-export is deprecated; use export-csv")` and `@register("legacy-export", retry=RetryPolicy(max_attempts=1))`, and the whole function, including the blank line inside its body. Also delete the two blank lines that follow that `return` line. Afterwards the last line of `export_json` is followed by exactly two blank lines and then `@register("import-jobs", retry=RetryPolicy(max_attempts=5, base_delay_s=10.0))`.

Keep `LEGACY_EXPORT_COLUMNS` (it is still used by `import_jobs`) and the `deprecated` helper function.

**1d. Move `RetryPolicy`.** The class `RetryPolicy` is the last definition in the file: 27 lines from `class RetryPolicy:` through its last line `        )`, which is the last line of the file. It is preceded by two blank lines after the end of `run_job`. Move the class, byte for byte with its blank lines and indentation unchanged, to directly after the line `logger = logging.getLogger(__name__)`, so that the result reads: that `logger` line, two blank lines, the whole `RetryPolicy` class, two blank lines, and then `DEFAULT_TIMEOUT_S = 30` (the line that already followed the two blank lines after the `logger` line). At the old location, remove the class and the two blank lines before it: the file then ends with the line `        return result` (the last line of `run_job`), followed by a single newline and nothing else.

## 2. `app/registry.py`

Delete these three lines from the `ENABLED_HANDLERS` list (each line is deleted whole, including its trailing comment, if any):

```python
    "legacy-export",
    "legacy-import",  # replaced by import-jobs in 2.4
    "legacy-reindex",
```

The other six entries keep their order and text. Leave `REMOVED_IN_3_0` and `HANDLER_QUEUES` unchanged, even though they mention the same names.

## 3. `docs/handlers.md`

**3a.** Delete this table row:

```markdown
| `legacy-export` | exports | 1 |
```

**3b.** Delete the whole `## Legacy export` section: the 14 lines from the line `## Legacy export` through the blank line directly before `## Retry policy`, inclusive. That includes its `sh` code block and its `### Migrating` subsection, whose last text line is:

```markdown
position must skip the `owner` column.
```

Afterwards the fence line of three backticks that closes the code block in `## Export CSV` is followed by exactly one blank line and then `## Retry policy`. The `## Import jobs` section, which mentions the legacy layout, stays.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
