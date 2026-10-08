# Validation patterns

Both `src/validate/patterns.py` and `web/src/validate.ts` implement these
patterns. Update this table whenever either file changes.

| Name | Pattern | Example match |
|------|---------|---------------|
| Slug | `^[a-z0-9]+(?:-[a-z0-9]+)*$` | `release-notes` |
| Order ID | `^(?:ORD\|RET)-\d{8,10}$` | `RET-12345678` |
| Hex color | `^#(?:[0-9a-fA-F]{3}){1,2}$` | `#1a2B3c` |
| Tag list | `^[\w.-]+(?:\|[\w.-]+){0,15}$` | `api\|web-v2.1` |
| Unix path | `^(?:/[^/\0]+){1,32}/?$` | `/var/log/acme/` |

Inside the table, a literal pipe is written as `\|` so that Markdown does not
start a new cell. Outside tables, write `|` as usual, for example `api|web`.

## Phone numbers

`normalize_phone()` matches `^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$`
and rewrites the number to `+1 \1-\2-\3`, so `(415) 555.0100` becomes `+1 415-555-0100`.

## Legacy v1 patterns

The v1 API still accepts `^ORD-\d{6,8}$` and `^\w+(?:\|\w+)*$`; see
`src/validate/compat.py`.
