`src/routes.py` is a long file of nearly identical route handlers (about 2,000 lines). Make exactly these two changes:

1. In `handle_route_0021` (it starts at line 146), change `timeout_ms = 1500` to `timeout_ms = 2500`. Many other handlers also contain `timeout_ms = 1500`; change only this one.
2. In `handle_route_0268` (it starts at line 1875), move the route to API v2 and raise its retries:
   - `/api/v1/items/268` becomes `/api/v2/items/268` in both the docstring and the `return` line.
   - `retries = 2` becomes `retries = 4`.

Every other handler keeps its values.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
