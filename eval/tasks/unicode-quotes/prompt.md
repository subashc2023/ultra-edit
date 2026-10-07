Update Trailhead's copy in four files. Every file is UTF-8 without a byte order mark and uses LF line endings. Several files contain non-ASCII and invisible characters; each one below is identified by its Unicode code point, and every character you do not explicitly change must keep its exact code point and encoding.

- `docs/guide.md` uses typographic punctuation: curly double quotes U+201C `“` and U+201D `”`, curly single quotes U+2018 `‘` and U+2019 `’`, the em dash U+2014 `—`, the en dash U+2013 `–`, and the ellipsis U+2026 `…`. Between each number and its unit (for example `10 km` and `500 m`) is U+00A0 NO-BREAK SPACE, not an ordinary space. Do not convert any of these to ASCII.
- `i18n/fr.json` stores some accented letters as literal UTF-8 characters (such as `é`, `à`, `ç`, `œ`) and others as six-character JSON escapes such as `\u00e9` and `\u2019` (a backslash, the letter `u`, and four hex digits). Keep every entry in the form it is in now: do not decode escapes, do not add escapes, and do not re-serialize the JSON. The `poi.viewpoint` value contains U+202F NARROW NO-BREAK SPACE and U+00A0 NO-BREAK SPACE; leave it unchanged.
- `README.md` uses straight ASCII double quotes U+0022 `"`. They must stay straight; do not turn them into curly quotes.

1. `docs/guide.md`
   - Replace `the “Plan a route” screen` with `the “Plan a hike” screen`. Keep the U+201C and U+201D quotes.
   - Replace `limit is 10 km` with `limit is 12 km`. The character between the number and `km` is U+00A0 NO-BREAK SPACE before and after the change. Leave `25 km` and the `10 km` in the sentence about the ‘short’ badge unchanged.
   - Replace `“steep” — the score uses both` with `“steep” — the rating uses both`. The dash is U+2014 EM DASH with an ordinary space (U+0020) on each side.
   - Replace `such as 08:00–18:00; trails` with `such as 07:30–19:00; trails`. The dash between the times is U+2013 EN DASH. The `08:00–18:00` in `CHANGELOG.md` stays unchanged.
2. `i18n/fr.json`
   - Replace `"route.difficulty.moderate": "Modérée",` with `"route.difficulty.moderate": "Intermédiaire",`. Both `é` characters in the old value and the `é` in the new value are literal UTF-8 characters (U+00E9), not escapes. Leave the `"legend.moderate"` entry, which spells the same word with `\u00e9` escapes, unchanged.
   - Replace `"poi.heart": "Au cœur du parc national",` with `"poi.heart": "Au cœur de la réserve naturelle",`. `œ` (U+0153) and `é` (U+00E9) are literal UTF-8 characters.
3. `src/messages.py`
   - In the `"ja"` table, replace `"route_saved": "ルートを保存しました",` with `"route_saved": "ハイキングルートを保存しました",`.
   - In the `"de"` table, replace `"street_hint": "Startpunkt an der Hauptstraße wählen",` with `"street_hint": "Startpunkt an der Talstraße wählen",`. Keep `ß` (U+00DF) and `ä` (U+00E4) as literal characters.
4. `README.md`
   - Replace `trailhead plan --region "Black Forest" --max-km 10` with `trailhead plan --region "Vosges du Nord" --max-km 12`.
   - Replace `the CLI prints "No route found"` with `the CLI prints "No matching route"`.
   - Leave `"Plan a route"`, `"metric"`, and the apostrophes in `won't` and `didn't` exactly as they are.

`i18n/de.json`, `src/units.py`, `src/__init__.py`, and `CHANGELOG.md` stay exactly as they are, including their own `Hauptstraße`, curly quotes, and `\u00a0`-style escapes.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
