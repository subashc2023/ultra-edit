We are releasing quakefeed 1.5.0. Bump the package's own version from 1.4.2 to 1.5.0 everywhere this repository declares it or shows it to users as the current release. The places are not listed here, so read the files to find them.

A mention of 1.4.2 changes only when it says which version of quakefeed this checkout is:
- the version declared in the package metadata and in the package's `__version__`;
- the version the client reports about itself at runtime, including the one written into its HTTP user-agent string;
- the version the documentation states as the current release, tells users to install or pin as the current release, or shows as the output of printing `quakefeed.__version__`.

There are seven such mentions, in these five files: `pyproject.toml`, `src/quakefeed/__init__.py`, `src/quakefeed/client.py`, `README.md`, and `docs/installation.md`. In each one, replace the version number 1.4.2 with 1.5.0 and change nothing else on the line.

Leave every other mention unchanged, even where it is the same number:
- Versions of other software, such as the `backoff` and `httpx` dependency requirements and pins, and the API server's version.
- Mentions of 1.4.2 as an earlier release that this one is compared with: notes about caches written by 1.4.2 and earlier, the upgrade section for users coming from 1.4.2 (its heading included), and test data that imitates what 1.4.2 wrote.
- Numbers that merely contain 1.4.2 inside a longer number, such as 1.4.20, or the 11.4.2 at the end of an IP address.
- Mentions of 1.5.0 that are already present.

`tests/test_client.py` stays unchanged: it compares the user-agent with `quakefeed.__version__` rather than a literal, so it needs no edit. `CHANGELOG.md` stays unchanged too, including its Unreleased section and its 1.4.2 entry; the release script adds the 1.5.0 heading later. Do not add or remove lines.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
