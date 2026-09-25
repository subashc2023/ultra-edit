Update the contact details and the release sign-off.

In both Markdown files, some lines end with two spaces, which Markdown renders as a line break. Every line that ends with two spaces now must still end with exactly two spaces afterwards, including the lines you edit. Lines without trailing spaces must not gain any.

1. `docs/contact.md`
   - `400 Harbor Street, Suite 12` becomes `410 Harbor Street, Suite 200`
   - `Phone: +1 503 555 0100` becomes `Phone: +1 503 555 0199`
   - `Hours: 9:00-17:00 Pacific` becomes `Hours: 8:00-18:00 Pacific`
2. `docs/release-notes.md`
   - `Dana Whitfield` becomes `Priya Raman`

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
