# shop-platform-config

Deployment configuration for the shop platform services.

- `config/services.toml`: one `[services.<name>]` table per service, grouped by
  owning team. Unset keys fall back to `config/defaults.toml`.
- `docs/services.md`: the human-readable catalog of the services that serve
  traffic directly.
- `tools/lint_services.py`: checks that every table has an `owner` and a known
  `region`.

Typical changes are one-line edits, for example `timeout_ms = 3000` to
`timeout_ms = 5000` or `enabled = true` to `enabled = false` for a single
service. Update `docs/services.md` whenever a port, owner, or region in the
catalog changes.
