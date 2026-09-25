Rename the constant `MAX_RETRIES` to `RETRY_LIMIT` throughout this repository: in code, tests, and documentation.

- Replace every occurrence of the exact, case-sensitive identifier `MAX_RETRIES` with `RETRY_LIMIT`. There are 11 occurrences in five files:
  - `src/httpkit/settings.py`: 1
  - `src/httpkit/client.py`: 4
  - `tests/test_client.py`: 2
  - `README.md`: 2
  - `docs/configuration.md`: 2
- `MAX_RETRIES_PER_HOST` is a different constant. Leave every occurrence of it unchanged, and leave `src/httpkit/pool.py` unchanged.
- Leave the lowercase `max_retries` in the test function name unchanged.
- The two names have the same length, so the Markdown table in `docs/configuration.md` stays aligned without any other change.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
