# Fernhill Library web front end

Static pages, styles, and scripts for the Fernhill Library catalog site. The
server renders nothing: every page in `templates/` is served as is, and the
scripts in `static/js/` add behavior in the browser.

## Layout

- `templates/` holds the HTML pages. `reserve.html` is maintained by branch
  staff on Windows and keeps its CRLF line endings.
- `static/css/buttons.css` defines the button classes.
- `static/js/buttons.js` has helpers shared by every page, and
  `static/js/reserve.js` runs the reservation form.
- `tests/` checks the markup that the scripts rely on.
- `docs/style-guide.md` explains when to use each kind of button.

## Running the tests

The tests need only Python 3.9 or newer:

    python -m unittest discover -s tests

## Theming

Branch microsites recolor the buttons by overriding `--btn-primary-bg`,
`--btn-primary-bg-hover`, and `--btn-primary-fg` on `:root` in their own
stylesheet, loaded after `buttons.css`. Keep the contrast between each
background and its text at 4.5:1 or more.
