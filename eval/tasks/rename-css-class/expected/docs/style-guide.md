# Button style guide

Every button on a Fernhill page starts from the `.btn` base class in
`static/css/buttons.css` and adds exactly one variant class. The variant says
how important the action is; the colors come from the theme.

## Variants

- `btn-accent` marks the primary button: the one main action on a page or in
  a group of buttons, such as "Reserve this copy". Use at most one per group.
- `btn-primary-outline` is for secondary actions next to a primary button,
  such as "Back to results" or "My account". It uses the primary color for
  its border and text and has a transparent background.
- `btn-link` looks like a link and is for low-priority actions such as
  "Advanced search".

`btn-large` and `btn-small` change only the size and can be added to any
variant.

Write the base class first, then the variant, then any size class:

```html
<button type="submit" class="btn btn-accent">Reserve this copy</button>
<a class="btn btn-primary-outline" href="/catalog">Back to results</a>
```

## Colors

The primary color comes from custom properties on `:root`, so a theme can
change it without repeating the button rules:

- `--btn-primary-bg` is the background of a primary button, and the border and
  text color of an outline button.
- `--btn-primary-bg-hover` replaces it on hover and keyboard focus.
- `--btn-primary-fg` is the text color on a primary button.

The dark color scheme sets all three again in a `prefers-color-scheme` block.

## States

`.btn-accent:hover` and `.btn-accent:focus-visible` share one rule. A busy
or disabled primary button (`.btn-accent.is-busy`, `.btn-accent[disabled]`)
is dimmed and shows a progress cursor; `setBusy()` in `static/js/buttons.js`
adds the busy class for you.

## Help text and labels

If a primary button needs an explanation, put it in a paragraph with the id
`btn-primary-help` and point the button at it with `aria-describedby`. A form
can name the label of its primary button in `data-btn-primary-label`;
`static/js/reserve.js` puts that label back once the form is valid.

## Scripts

Use `promote(button)` and `demote(button)` from `static/js/buttons.js` instead
of editing `classList` yourself: `promote` adds `btn-accent` and demotes any
other primary button in the same group, so a group never shows two.
