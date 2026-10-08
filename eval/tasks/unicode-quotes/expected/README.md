# Trailhead

Trailhead turns imported trail data into day-hike suggestions. It ships with
English, French, German, and Japanese strings.

## Quick start

    pip install trailhead
    trailhead plan --region "Vosges du Nord" --max-km 12

Open the "Plan a route" screen in the app to see the same suggestions. Set
`TRAILHEAD_UNITS="metric"` to force kilometres; the CLI prints "No matching route"
when nothing fits, and it won't guess a region you didn't import.

## Translations

French strings live in `i18n/fr.json` and German strings in `i18n/de.json`.
Some older French entries are stored with `\u00e9`-style escapes; both forms
load identically, so don't rewrite them.
