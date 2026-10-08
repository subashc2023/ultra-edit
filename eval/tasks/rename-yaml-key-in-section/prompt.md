Rename the configuration key `pool_size` of the `database` section to `max_connections` in parcelhub. Three sections of the configuration have a key named `pool_size` (`database`, `cache`, and `http`), and operators keep confusing the database one, which caps the connections each worker process opens to PostgreSQL, with the other two. Only the `database` key is renamed; the `cache` and `http` keys keep the name `pool_size`. This request does not quote the lines to change, so read each file to find them.

Rename every place where `pool_size` is the key of the `database` section: as a YAML key nested under `database:`, as the key that Python code reads from or writes to the `database` mapping, and where docs, messages, or tests name that key. There are 17 such lines, in these six files:

- `config/base.yaml`: 1 line, the key in the `database` section.
- `config/production.yaml`: 1 line, the key in the `database` section.
- `deploy/windows/service.yaml`: 1 line, the key in the `database` section.
- `src/parcelhub/settings.py`: 5 lines:
  - the `ENV_OVERRIDES` entry for `APP_DB_POOL_SIZE`, whose tuple names the `database` section and its key;
  - in `build_settings`, the `.get(...)` call on the `database` mapping, on the line that passes `db_pool_size=`;
  - in `check_connection_budget`, the subscript lookup of the key, and the first line of the error message, which names the key as `database.pool_size`;
  - in `describe_pools`, the subscript lookup on the `database` line.
- `docs/configuration.md`: 4 lines:
  - the key's row in the `database` table;
  - the line of the paragraph below that table that names `database.pool_size`;
  - the key's line in the fenced YAML example below that paragraph (the file for a one-off reporting job);
  - the `APP_DB_POOL_SIZE` item in the list of environment variables, which names `database.pool_size`.
- `tests/test_settings.py`: 5 lines:
  - the key under `database:` in three YAML snippets embedded as strings: the base file that the `config_dir` fixture writes, the production file in `test_environment_file_overrides_base`, and the snippet in `test_connection_budget_rejects_an_oversized_pool`;
  - the regular expression that `test_connection_budget_rejects_an_oversized_pool` passes as `match=`, which names the key after an escaped dot;
  - the key of the `database` dictionary in the `cfg` literal of `test_connection_budget_allows_the_production_pool`.

On each of these lines, replace that one `pool_size` with `max_connections` and change nothing else. Keep the indentation, quotes, colon, value, and any spaces and comment after the value exactly as they are; comments are not realigned, and keys keep their order. Some of these lines also contain a longer name that stays: `APP_DB_POOL_SIZE` on the `ENV_OVERRIDES` line and in the docs list item, and the keyword `db_pool_size` on the `build_settings` line. In the regular expression, keep the backslash before the dot. A line in `docs/configuration.md` or `src/parcelhub/settings.py` that gets longer is not rewrapped.

One exception to "change nothing else": the tables in `docs/configuration.md` are padded with spaces so that their `|` separators line up, and the `Key` column of the `database` table is already wide enough for the new name. In the renamed row, the key cell keeps its width: the new name is six characters longer, so remove six of the spaces between the cell's closing backtick and the next `|`. That `|` and the rest of the row stay exactly where they are, and no other row of any table changes.

Look-alikes that stay exactly as they are:
- The `pool_size` keys of the `cache` and `http` sections, everywhere: in the YAML files; in their `ENV_OVERRIDES` entries, `.get(...)` calls, and `describe_pools` lines in `src/parcelhub/settings.py`; in the `cache` and `http` tables, the fenced YAML example for the cache, and their items in the environment variable list in the docs; and in the test YAML snippets and the dictionaries of `test_merge_keeps_keys_the_override_leaves_out`. Some test snippets contain a line identical to a changed one, but under `http:` or `cache:`; those lines stay.
- The other keys of the `database` section, such as `url`, `pool_timeout`, and `statement_timeout_ms`, everywhere they appear.
- The `Settings` field `db_pool_size` keeps its name everywhere: its declaration, the `build_settings` keyword, the attribute reads in `src/parcelhub/db.py`, the assertions in the tests, and the Python example in the docs. So do `cache_pool_size` and `http_pool_size`.
- The environment variable names `APP_DB_POOL_SIZE`, `APP_CACHE_POOL_SIZE`, and `APP_HTTP_POOL_SIZE`, in code, docs, YAML comments, and `README.md`.
- Test function names that contain `pool_size`.
- The words "pool size", "pool", and "connections" in English text: YAML comments, Python comments and docstrings, and Markdown prose and table cells. The error message in `check_connection_budget` changes only in the key name. In `describe_pools`, only the subscript changes, so the text that the function returns, and the expected lines in `test_describe_pools_lists_every_pool`, stay the same.
- The `"pool_size"` subscripts in `pool_usage` in `src/parcelhub/db.py`: they read the connection pool library's own statistics, not the configuration.

`config/staging.yaml` stays unchanged: it overrides only the cache's `pool_size`, and it mentions the database pool size only in a comment. `src/parcelhub/db.py` and `README.md` stay unchanged too. Do not keep `pool_size` as a fallback or alias for the old key (no second lookup, no compatibility shim), and do not add a changelog entry, tests, or docs.

`deploy/windows/service.yaml` uses Windows CRLF line endings on every line; keep CRLF on the line you change. Every other file uses LF line endings. Every file ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
