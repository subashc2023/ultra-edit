Prepare release 2.4.0.

`build/release.cmd` uses Windows CRLF line endings. `build/release.sh` and `VERSION` use Unix LF line endings, and `VERSION` has no newline at the end. Keep each file's line endings exactly as they are, including on the lines you add.

1. In `VERSION`, `build/release.cmd`, and `build/release.sh`, change the version `2.3.1` to `2.4.0`. It appears once in each file.
2. In `build/release.cmd`, insert a new line `set SIGN_BUILD=1` directly after the line `set OUT_DIR=dist\win`. The new line ends with CRLF like the rest of the file.
3. In `build/release.sh`, insert a new line `SIGN_BUILD=1` directly after the line `OUT_DIR=dist/unix`.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
