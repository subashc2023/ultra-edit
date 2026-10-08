Rename the settings loader `load` in `src/relay/config.py` to `load_config`, and update everything that refers to it. The bare name `load` has become ambiguous in this project: `src/relay/plugins.py` has its own module-level `load` that imports a plugin, the `Cache` class in `src/relay/service.py` has a `load` method, and `RelayService.health` uses a local variable named `load` for the system load average. This request does not list the lines to change, so read each file to find them.

Rename the function `load` defined at module level in `src/relay/config.py`, together with every code reference that resolves to that function:
- its `def` line, and the one call to it inside `src/relay/config.py`;
- every import of it, absolute or relative;
- every call to it, whether through the imported name or as an attribute of the `config` module;
- the dotted target string that a test passes to `mock.patch` to replace it;
- the code in `docs/configuration.md` that refers to it: one inline code span, and one line of the fenced Python example.

There are eleven such lines, in these six files: `src/relay/config.py`, `src/relay/service.py`, `src/relay/cli.py`, `tests/test_config.py`, `tests/test_cli.py`, and `docs/configuration.md`. On each of these lines, replace that one `load` with `load_config` and change nothing else: the other names imported on the same line stay, in the same order, and a line in `docs/configuration.md` that gets longer is not rewrapped. `src/relay/plugins.py` stays unchanged.

Everything else that is named `load`, or contains `load`, stays as it is:
- `load` in `src/relay/plugins.py` and every use of it: the calls through the `plugins` module, the `mock.patch` target string that names it, and `plugins.load()` in the docs.
- The `load` method of `Cache`, every call to it, and `Cache.load()` in the docs.
- `json.load` and `yaml.safe_load`.
- Local variables named `load` or `loaded`, and the `"load"` key in the health report.
- Every other name that merely contains `load`: `reload` in `src/relay/config.py` keeps its name where it is defined, imported, and called, and in `config.reload()` in the docs; `load_all` in `src/relay/plugins.py`, the `max_load` attribute, and the test function names stay too.
- The word "load" in English text, in any form ("Load", "loads", "loaded", "Loading"): in comments, docstrings, string literals such as help text and log messages, and Markdown prose, headings, and table cells. Only code references to the function change.

Do not keep `load` as an alias of the new name, and do not add a deprecation shim, a changelog entry, or new tests or docs. Every file uses LF line endings and ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
