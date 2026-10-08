Remove the deprecated `--legacy-tls` option from logship. It was deprecated in 2.6.0 and is now due for removal: from now on logship always requires TLS 1.2 or newer. Remove the option completely, together with the `legacy_tls` field of `ShipperConfig` and everything that exists only to support, document, or test them. This request does not quote the lines to remove, so read each file to find them.

Six files change:

- `src/logship/cli.py`: in `build_parser`, remove the option's `parser.add_argument(...)` call together with the deprecation comment line directly above it. In `config_from_args`, remove the keyword argument that passes the option into the config. In `main`, remove the `if` block that issues the deprecation warning.
- `src/logship/config.py`: remove the `legacy_tls` field from the `ShipperConfig` dataclass.
- `src/logship/transport.py`: in `build_ssl_context`, remove the `if` block that lowers the minimum protocol version and weakens the ciphers. The line above it that sets TLS 1.2 as the minimum stays, and so does the client certificate handling below it.
- `tests/test_cli.py`: remove the two test functions that only test this option (one checks the deprecation warning, the other the lowered protocol version), and remove the assertion about the field from `test_defaults`. The other assertions in `test_defaults` stay.
- `docs/options.md`: remove the option's row from the options table. Leave every other row exactly as it is. The removed row is not the widest in any column, so the table stays aligned without any other change.
- `README.md`: remove the subsection about connecting to old collectors, from its heading through the end of its paragraph. The rest of the TLS section stays as it is.

Imports: if a removal leaves an import with no remaining use in its file, remove that import line too. This happens to exactly one import, in `src/logship/cli.py`. Every other import in every file is still used and stays.

Blank lines (a blank line is an empty line with no spaces):
- A single removed line (the import, the keyword argument, the dataclass field, the assertion, the table row) goes on its own. The lines around it, blank or not, stay as they are.
- A removed block of several lines goes whole, including any blank lines inside it, together with every blank line directly above it: the one blank line above each of the two `if` blocks and above the README subsection, and the two blank lines above each removed test function. The `add_argument` call has its comment directly above it and no blank line above that, so only the comment and the call go. Never remove a blank line that comes after a removed block.
- As a result, the remaining code keeps exactly one blank line where statements inside a function were separated by one, keeps two blank lines between top-level functions, and `tests/test_cli.py` ends with the last line of its remaining last test followed by a single newline.

Look-alikes that stay: `--legacy-output` and its `legacy_output` config field are a different, supported option. Keep every line about them, including their parser call, keyword argument, dataclass field, table row, the note below the table, the README section, and the tests that use them. `--tls-server-name` and the other TLS options also stay.

`CHANGELOG.md` stays unchanged even though it mentions `--legacy-tls`; do not add an entry for this removal. After the change, `CHANGELOG.md` is the only file that still mentions `legacy-tls` or `legacy_tls`.

Every file uses LF line endings and ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
