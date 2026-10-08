Retire the `USE_STREAMING_DECODER` feature flag of sluice. It has been on in every deployment since 0.9, so the streaming decoder becomes the only decoder: keep the code that runs when the flag is on, and remove the flag together with the code, tests, and docs that exist only for running with it off. This request does not quote the lines to change, so read each file to find them.

Five files change:

- `src/sluice/flags.py`: remove the flag's assignment together with the two comment lines directly above it. The module docstring, `_env_flag`, and the other flags stay as they are.
- `src/sluice/decode.py`: three changes.
  - In `decode_batch`, the three lines after the docstring (the `if` statement that tests the flag, the `return` inside it, and the `return` after it that calls `_decode_legacy`) become one line: the `return` from inside the `if`, moved four spaces left to the indentation of the function body and otherwise unchanged. The docstring of `decode_batch` stays as it is.
  - Remove the function `_decode_legacy`, which nothing calls any more: its `def` line, its docstring, and its body.
  - Remove the import that only `_decode_legacy` used (see Imports below).
- `src/sluice/ingest.py`: in `ingest`, inside the `try` block of the `for` loop over the batches, replace the `if`/`else` statement that tests the flag with the body of its `if` branch. The `if` line, the `else:` line, and everything in the `else` branch (its comment line and its three statements) go. Every line of the `if` branch stays, in the same order, including its two comment lines and the two lines in the body of its nested `for` loop, and each of these lines moves exactly four spaces left. Nothing else on those lines changes.
- `tests/test_decode.py`: remove the two test functions that run `decode_batch` with the flag patched to `False`. One patches it with a `with` block; the other patches it with a decorator, which goes with its function. In the one test that patches the flag to `True`, remove the `with mock.patch.object(...)` line and move every line of its body four spaces left, including the nested `with pytest.raises(...)` line and the line inside it. That test keeps its name and its statements, in the same order.
- `docs/feature-flags.md`: remove the flag's row from the table of current flags. Leave every other row exactly as it is. The removed row is not wider than the remaining rows in any column, so the table stays aligned without any other change.

Imports: if a removal leaves an import with no remaining use in its file, remove that import line too. This happens to exactly one import: a standard-library module imported on a line of its own in `src/sluice/decode.py`, which only `_decode_legacy` used. Every other import in every file is still used and stays, including `from . import flags` in both `src/sluice/decode.py` and `src/sluice/ingest.py`, and the imports of `mock` and `flags` in `tests/test_decode.py`.

Blank lines (a blank line is an empty line with no spaces):
- The removed import line and the removed table row go on their own. The lines around them, blank or not, stay as they are.
- A removed block of several lines goes whole, together with every blank line directly above it: the one blank line above the flag's comment in `src/sluice/flags.py`, the two blank lines above `_decode_legacy`, and the two blank lines above each of the two removed test functions (above its decorator, for the decorated one). Never remove a blank line that comes after a removed block.
- Collapsing the `if` in `decode_batch`, the `if`/`else` in `ingest`, and the `with` block in the test adds and removes no blank lines; none of them contains one.
- As a result, the remaining flags in `src/sluice/flags.py` stay separated by one blank line, and top-level functions and classes stay separated by two.

Look-alikes that stay:
- The other two flags, `REJECT_UNKNOWN_FIELDS` and `USE_ASYNC_WRITER`, with their comments, their environment variables, their table rows, and every use of them. In particular, the `if`/`else` statement on `USE_ASYNC_WRITER` near the top of `ingest` has the same shape as the one that goes, and stays exactly as it is. So do the `if` on `REJECT_UNKNOWN_FIELDS` in `_parse_record`, and every test that patches either flag with `mock.patch.object`, as a decorator or a `with` block.
- `_decode_legacy_header` and `LEGACY_HEADER_PREFIX` parse the header line that agents older than 2.0 still send, and `StreamingDecoder` still calls `_decode_legacy_header`. Keep them and the two tests of `_decode_legacy_header`, even though the function's name starts with `_decode_legacy`.
- `StreamingDecoder`, `CHUNK_SIZE`, `WRITE_SLICE`, `_slices`, the three `test_streaming_decoder_...` tests, and every comment, docstring, and paragraph that talks about streaming in general, such as the comment above `CHUNK_SIZE` and the comment above the `for` loop in `ingest`.
- Docstrings and comments outside the removed lines stay word for word, and the rest of `docs/feature-flags.md`, including its section on retiring a flag, stays as it is.

`tests/test_ingest.py`, `README.md`, and `CHANGELOG.md` stay unchanged: `tests/test_ingest.py` patches only `USE_ASYNC_WRITER`, and `CHANGELOG.md` keeps its entries that mention the flag; do not add an entry for this change. After the change, `CHANGELOG.md` is the only file that still mentions `USE_STREAMING_DECODER` or `SLUICE_STREAMING_DECODER`, and `_decode_legacy` appears only as part of the name `_decode_legacy_header`.

Every file uses LF line endings and ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
