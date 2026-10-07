# Job handlers

The worker runs one handler per job kind. Every handler has a retry policy;
see [Retry policy](#retry-policy).

| Kind | Queue | Max attempts |
| --- | --- | --- |
| `export-csv` | exports | 3 |
| `export-json` | exports | 3 |
| `legacy-export` | exports | 1 |
| `import-jobs` | imports | 5 |
| `purge-expired` | maintenance | 2 |
| `reindex` | maintenance | 4 |
| `send-digest` | notifications | 3 |

## Export CSV

`export-csv` writes the selected rows with six columns: `id`, `name`,
`status`, `owner`, `created_at`, and `finished_at`.

```sh
worker enqueue export-csv --param query=nightly
```

## Legacy export

`legacy-export` writes the pre-2.0 four-column CSV (`id`, `name`, `status`,
`created`). It logs a warning on every run and is never retried.

```sh
worker enqueue legacy-export --param query=nightly
```

### Migrating

Replace `legacy-export` with `export-csv`. Consumers that read columns by
position must skip the `owner` column.

## Retry policy

A failed attempt is retried after `base_delay_s * 2 ** (attempt - 1)` seconds,
capped at `max_delay_s`. `ValueError` and `PermissionError` are never retried.

## Import jobs

`import-jobs` accepts files in the current six-column layout and in the legacy
four-column layout written by older releases.
