Rename the CSS class `btn-primary` to `btn-accent` in the Fernhill Library front end. Several pages now use the class for buttons that are not the main action, so it is getting a name that describes how the button looks. This request does not list the lines to change, so read each file to find them.

Rename the class wherever `btn-primary` is the whole class name, as a name of its own:
- in the `class` attributes of the HTML templates, where it sits among other classes; the other classes stay, in the same order;
- in CSS selectors, including where it is combined with a pseudo-class, an attribute selector, another class, or a descendant or sibling combinator;
- in JavaScript, in the class names passed to `classList` methods and in the selector strings passed to `querySelector` and `querySelectorAll`;
- in the test, in the string literals that name the class, including the selector text the test expects to find in the stylesheet;
- in the style guide, in every inline code span that names the class, alone or as part of a selector, and in the one line of the fenced HTML example that uses it.

There are 29 such lines, in these seven files:
- `templates/index.html`: 2 lines.
- `templates/reserve.html`: 2 lines.
- `static/css/buttons.css`: 7 lines.
- `static/js/buttons.js`: 5 lines.
- `static/js/reserve.js`: 2 lines.
- `tests/test_markup.py`: 6 lines; one of them names the class twice.
- `docs/style-guide.md`: 5 lines; two of them name the class twice.

On each of these lines, replace every occurrence of the class name `btn-primary` with `btn-accent` and change nothing else. Some of these lines also contain one of the longer names below; those stay. No comment, docstring, or prose sentence names the class outside a code span, so none of them changes. The changed lines get shorter; do not rewrap or realign anything.

Every longer hyphenated name that merely contains `btn-primary` stays exactly as it is, including on the lines that change:
- The class `btn-primary-outline`, which is a different class: in `class` attributes, in CSS selectors (with or without `:hover` and `:focus-visible`), in `classList` calls and selector strings, in the test, and in the style guide.
- The CSS custom properties `--btn-primary-bg`, `--btn-primary-bg-hover`, and `--btn-primary-fg` keep their names: where they are defined (in `:root` and again in the dark color scheme block), where they are read with `var()`, and where the test, the style guide, and `README.md` name them.
- The `data-btn-primary-label` attribute: on the form in `templates/reserve.html`, in the `getAttribute` call that reads it, in the test, and in the style guide.
- The id `btn-primary-help`: in its `id` attribute, in the `aria-describedby` attribute that refers to it, in the `#btn-primary-help` CSS rule, in the `getElementById` call, in the test, and in the style guide.

Other names and prose stay too:
- The JavaScript variable `btnPrimary` keeps its name, and so do the functions `promote`, `demote`, and `isPrimary`, and the test classes and test functions.
- The word "primary" in English text, in any form ("primary button", "Primary button", "primary color"): in CSS, JavaScript, and HTML comments, in docstrings, and in Markdown prose.

`README.md` stays unchanged: it names only the custom properties. Do not keep `btn-primary` as an alias (no extra selector, no second class in the markup), and do not add a changelog entry, tests, or docs.

`templates/reserve.html` uses Windows CRLF line endings on every line; keep CRLF on the lines you change. Every other file uses LF line endings. Every file ends with a single newline.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
