Order IDs are moving to a new format (`ORD-` or `RET-` followed by 8 to 10 digits) and phone numbers are now displayed as `+1 415-555-0100`. Update the validation patterns, their documentation, and the edge configuration. Make the 12 changes below in four files: `src/validate/patterns.py`, `web/src/validate.ts`, `docs/patterns.md`, and `deploy/nginx/site.conf`.

Each code block below shows one complete line exactly as it appears in the file, including its leading spaces. Every character is literal: `\`, `\\`, `|`, `\|`, `$1`, `$&`, `&`, `/`, `.`, `*`, `?`, `+`, `^`, `[`, `]`, `{`, and `}` all mean themselves, not regular-expression or replacement syntax, and nothing is escaped by this prompt. A `\\` in a code block is two backslash characters in the file. All four files use Unix LF line endings, and `deploy/nginx/site.conf` does not end with a newline.

## `src/validate/patterns.py`

1. Replace the raw-string line

```
ORDER_ID = re.compile(r"^ORD-\d{6,8}$")
```

with

```
ORDER_ID = re.compile(r"^(?:ORD|RET)-\d{8,10}$")
```

2. Replace the normal-string line (every backslash in it is doubled, and stays doubled)

```
TAG_LIST = re.compile("^\\w+(?:\\|\\w+)*$")
```

with

```
TAG_LIST = re.compile("^[\\w.-]+(?:\\|[\\w.-]+){0,15}$")
```

3. Replace the line

```
PHONE_SUB = (r"^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$", r"(\1) \2-\3")
```

with

```
PHONE_SUB = (r"^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$", r"+1 \1-\2-\3")
```

`LEGACY_ORDER_ID = re.compile("^ORD-\\d{6,8}$")`, `UNIX_PATH`, `DUP_SLASH_SUB`, and every other line stay unchanged.

## `web/src/validate.ts`

4. Replace the line

```
export const UNIX_PATH = /^(?:\/[^/]+)+\/?$/;
```

with

```
export const UNIX_PATH = /^(?:\/[^/\0]+){1,32}\/?$/;
```

5. Replace the line

```
export const ORDER_ID = /^ORD-\d{6,8}$/;
```

with

```
export const ORDER_ID = /^(?:ORD|RET)-\d{8,10}$/;
```

6. In `formatPhone`, replace the line (indented with two spaces)

```
  return value.replace(/^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$/, "($1) $2-$3");
```

with (still indented with two spaces)

```
  return value.replace(/^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$/, "+1 $1-$2-$3");
```

7. In `highlightMatches`, replace the line (indented with two spaces)

```
  return text.replace(term, "<mark>$&</mark>");
```

with (still indented with two spaces; the new text contains two backslash-escaped double quotes)

```
  return text.replace(term, "<mark class=\"hit\">$&</mark>");
```

`DIGITS`, `TAG_SPLIT`, `WIN_DRIVE`, `collapseSlashes`, `splitTags`, and every other line stay unchanged.

## `docs/patterns.md`

8. Replace the table row

```
| Order ID | `^ORD-\d{6,8}$` | `ORD-1234567` |
```

with

```
| Order ID | `^(?:ORD\|RET)-\d{8,10}$` | `RET-12345678` |
```

9. Replace the table row

```
| Tag list | `^\w+(?:\|\w+)*$` | `api\|web` |
```

with

```
| Tag list | `^[\w.-]+(?:\|[\w.-]+){0,15}$` | `api\|web-v2.1` |
```

10. In the "Phone numbers" section, replace the line

```
and rewrites the number to `(\1) \2-\3`, so `+1 415.555.0100` becomes `(415) 555-0100`.
```

with

```
and rewrites the number to `+1 \1-\2-\3`, so `(415) 555.0100` becomes `+1 415-555-0100`.
```

The "Legacy v1 patterns" section still mentions `^ORD-\d{6,8}$` and `^\w+(?:\|\w+)*$`; leave that section and every other table row unchanged.

## `deploy/nginx/site.conf`

11. Replace the line (indented with four spaces)

```
    location ~* \.(png|jpe?g|gif|svg|ico)$ {
```

with (still indented with four spaces)

```
    location ~* \.(png|jpe?g|gif|svg|ico|webp|avif)$ {
```

12. Replace the line (indented with four spaces)

```
    rewrite ^/old/(.*)$ /new/$1 permanent;
```

with (still indented with four spaces)

```
    rewrite ^/old/(.*)$ /archive/$1?from=old&v=2 permanent;
```

The `location ~* \.(css|js)$ {` block, the other `rewrite` lines, and the final `}` line (which has no newline after it) stay unchanged.

`src/validate/compat.py` (which has look-alike v1 patterns), `web/src/format.ts`, `README.md`, and every other file stay exactly as they are.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
