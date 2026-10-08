Move the duration parser `parse_duration` out of `src/taskclock/utils.py` and into `src/taskclock/timeparse.py`, which already holds the other parsing and formatting helpers for dates, times, and durations. Then update every import, module-qualified call, and documentation reference so that it uses the new location. This request does not quote the lines to change, so read each file to find them.

What moves: the function `parse_duration` (its `def` line through its `return` line, docstring included, 20 lines) and the module-level compiled regular expression `_DURATION_RE`, which only `parse_duration` uses. Neither has a comment or decorator attached. Both move with their exact text: do not edit, reformat, or rename any moved line.

A blank line below is an empty line with no spaces. Seven files change:

- `src/taskclock/utils.py`: three removals and nothing else.
  - Remove the `_DURATION_RE` line on its own. It is the second line of a two-line constant block; the `MAX_SLUG_LENGTH` line above it and the blank lines below it stay.
  - Remove `parse_duration` together with the two blank lines directly above its `def` line. The two blank lines below it stay, so `slugify` and `chunked` end up separated by two blank lines.
  - Remove the import of the `re` module on its own: nothing else in the file uses `re`. The other imports (`random`, `unicodedata`, and the `datetime` import, which `jittered` still needs for `timedelta`) stay as they are.
  - Do not leave an alias, a re-export, or any import of `parse_duration` in this file; nothing else in it uses the function.
- `src/taskclock/timeparse.py`: two insertions and nothing else. Its imports stay unchanged: it already imports `re` and `timedelta`, which is everything the moved code needs, so it gains no import.
  - Insert the `_DURATION_RE` line directly below the last line of the existing constant block (the `WEEKDAYS` line), with no blank line between them. The two blank lines that followed the block now follow the inserted line.
  - Insert `parse_duration` between `parse_weekdays` and `format_duration`, so that exactly two blank lines separate it from each of them.
- In the code, three from-imports name `parse_duration` from the utils module: one in `src/taskclock/schedule.py` (relative, between two other names), one in `src/taskclock/runner.py` (relative, the only name), and one in `tests/test_utils.py` (absolute, before one other name).
  - On a line that imports other names too, delete `parse_duration` with the comma and space that separated it from the next name, so the remaining names keep their order, separated by a comma and a space. Then add a new line directly below that line which imports only `parse_duration` from the timeparse module, written the same way as the line above it: relative (`.timeparse`) in `src/taskclock/schedule.py`, absolute (`taskclock.timeparse`) in `tests/test_utils.py`. Keep the new line directly below even though that breaks alphabetical order.
  - On the line where `parse_duration` is the only name (`src/taskclock/runner.py`), replace the module `utils` with `timeparse` and change nothing else; no line is added there.
  - Do not reorder, merge, or otherwise change any other import line.
  - In total, `src/taskclock/schedule.py` and `tests/test_utils.py` each change one line and gain one line, and `src/taskclock/runner.py` changes one line.
- `src/taskclock/cli.py`: one line changes. The single call to `parse_duration` written as an attribute of the `utils` module becomes the same call through the `timeparse` module. The file already imports both modules on one line, and that line stays unchanged because `utils` is still used for `slugify`.
- `docs/api.md`: the reference entry for `parse_duration` moves from the `taskclock.utils` section to the `taskclock.timeparse` section. The entry is the 13 lines from its HTML anchor line through the closing fence of its example.
  - Remove the entry from the utils section together with the one blank line directly above it, so the `slugify` and `chunked` entries are separated by one blank line.
  - Insert it in the timeparse section between the `parse_weekdays` and `format_duration` entries, with one blank line between it and each of them.
  - In the moved entry, change exactly two lines: the anchor id becomes `taskclock.timeparse.parse_duration`, and the import in its example takes `parse_duration` from `taskclock.timeparse` instead of `taskclock.utils`. Its heading, prose, and the rest of its example keep their exact text.
  - Two links elsewhere in the file point to the old anchor, one in the `format_duration` entry and one in the `parse_duration_ms` entry. In each, change only the fragment to the new anchor id. Do not rewrap the lines that get longer.

Everything else stays as it is, including:
- Calls to `parse_duration` through the imported name in `src/taskclock/schedule.py`, `src/taskclock/runner.py`, and `tests/test_utils.py`. Only the import lines in those files change. The tests of `parse_duration` stay in `tests/test_utils.py` with their names.
- `parse_duration_ms` in `src/taskclock/runner.py`: its definition, its body, the lines that import it in `src/taskclock/cli.py` and `tests/test_runner.py`, its calls and tests, and its docs entry, apart from the link fragment named above.
- Everything else in the utils module and every other reference to it: `slugify`, `chunked`, `clamp`, `jittered`, and `MAX_SLUG_LENGTH`, the names that `src/taskclock/schedule.py` and `tests/test_utils.py` still import from it, the call to `utils.slugify` in `src/taskclock/cli.py`, the `taskclock.utils` section heading and the remaining utils entries and anchors in `docs/api.md`, and the `slugify` example that imports from `taskclock.utils`.
- Existing imports from `taskclock.timeparse`, such as the one in the `format_duration` example in `docs/api.md`.
- Local variables named `duration` (in `src/taskclock/runner.py` and `tests/test_utils.py`) and other words that contain "duration", such as `format_duration` and `parse_duration_ms`.
- Mentions of `parse_duration` in docstrings (in `format_duration` and `parse_duration_ms`) and the visible link text of the two links.
- Prose that says "parse a duration", in any capitalization: the `--every` error message in `src/taskclock/cli.py`, the job file section of `README.md`, and the first sentences of the moved docstring and docs entry, which move unchanged.

`README.md` and `tests/test_runner.py` stay unchanged. Do not add a compatibility alias, a deprecation shim, a changelog entry, or new tests or docs. Every file uses LF line endings and ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
