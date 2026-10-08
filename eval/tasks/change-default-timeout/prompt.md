Raise wayfetch's default connect timeout from 10 to 15 seconds. The connect timeout limits how long wayfetch spends resolving the host name and completing the TCP and TLS handshakes. Update every place in this repository that defines its default, or states or shows that default as the current value. This request does not quote the lines to change, so read each file to find them.

There are seven such places, in these six files. In each one, change the number 10 to 15 and change nothing else on the line; where the value is written as `10.0`, it becomes `15.0`.

- `src/wayfetch/defaults.py`: the constant that defines the default connect timeout (one line).
- `src/wayfetch/cli.py`: the help text of the `--connect-timeout` option, which states its default (one line).
- `docs/configuration.md`: the Default cell of the `connect_timeout` row in the settings table, and the sentence in the "Timeouts" section that gives the connect timeout's default (two lines).
- `README.md`: in the "Timeouts and retries" section, the number of seconds after which wayfetch gives up on a connection attempt (one line).
- `examples/wayfetch.toml`: the `connect_timeout` value. Every value in this file is the built-in default, so it must show the new default (one line).
- `tests/test_client.py`: the connect timeout assertion in the test that builds a `Client` with no settings and checks its defaults (one line).

Leave every other mention unchanged, even where it is the same number:
- The read timeout, whose default is also 10 and stays 10: its constant, its `--read-timeout` help text, its row in the settings table, its sentence in the "Timeouts" section (worded the same as the connect timeout's), its number in the same README sentence as the connect timeout's (on the next line), its value in `examples/wayfetch.toml`, and its assertion in the client defaults test.
- The retry backoff cap `backoff_max`, whose default is also 10: its constant, its `--backoff-max` help text, its row in the settings table, the sentences about retry pauses in `docs/configuration.md` and `README.md`, its value in `examples/wayfetch.toml`, its assertion in the client defaults test, and the expected list of backoff delays in `tests/test_client.py`.
- Numbers that merely contain 10: the connection pool size of 100, the proxy port 10080, the read timeout of 10.5 that a CLI test writes into its config file, and the 1010-byte response body in a client test.
- History: the note in `docs/configuration.md` saying that version 2.0 raised the default connect timeout from 5 to 10 seconds stays as written.
- Connect timeouts of 10 that a test passes explicitly instead of relying on the default: the `connect_timeout=10` keyword argument and the assertion about it in `tests/test_client.py`, and the `--connect-timeout` flag value and the assertion about it in `tests/test_cli.py`.

`src/wayfetch/client.py` stays unchanged: it takes its defaults from the constants in `src/wayfetch/defaults.py` and repeats none of their values. `tests/test_cli.py` stays unchanged: its defaults test compares with the constants rather than with literals. `CHANGELOG.md` stays unchanged too, including its 2.0.0 entry; do not add a changelog entry, a "Changed in" note, or any other text about this change. Do not add or remove lines, and do not rewrap or realign anything: 15 is as wide as 10, so the settings table stays aligned and wrapped paragraphs keep their line breaks.

Every file uses LF line endings and ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
