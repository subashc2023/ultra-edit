`config/services.toml` is a long service registry (about 2,650 lines) with one `[services.<name>]` table per service. Every table has the same keys in the same order, and most values are shared by many tables, so each line you change below also appears in many other tables. Change each line only inside the table named for it. Line numbers are the line of the table's `[services.<name>]` header in the original file, before any edit.

1. `config/services.toml`
   1. `[services.auth-gateway]` (line 72): replace `port = 8080` with `port = 8081`.
   2. `[services.billing-scheduler]` (line 280): replace `timeout_ms = 10000` with `timeout_ms = 7500`.
   3. `[services.billing-stream]` (line 296): replace `enabled = true` with `enabled = false`.
   4. `[services.catalog-indexer]` (line 440): replace `memory = "256Mi"` with `memory = "1Gi"`.
   5. `[services.checkout-api]` (line 536): replace `tags = ["public", "http"]` with `tags = ["public", "http", "pci"]`.
   6. `[services.checkout-admin]` (line 552): replace `replicas = 2` with `replicas = 1`.
   7. `[services.inventory-sync]` (line 840): insert a new line `connect_timeout_ms = 750` directly after its line `timeout_ms = 3000`. The new line has no indentation, like every other key line.
   8. `[services.ledger-export]` (line 936): replace `region = "ap-southeast-2"` with `region = "us-west-2"`.
   9. `[services.orders-worker]` (line 1400): replace `timeout_ms = 5000` with `timeout_ms = 12000`.
   10. `[services.payments-api]` (line 1416): replace `health_check = "/healthz"` with `health_check = "/readyz"`.
   11. `[services.pricing-api]` (line 1592): replace `enabled = false` with `enabled = true`.
   12. `[services.pricing-cache]` (line 1624): delete its line `log_level = "info"` entirely (the whole line including its newline). No blank line is added or removed.
   13. `[services.profile-gateway]` (line 1832): replace `cpu = "1000m"` with `cpu = "2000m"`.
   14. `[services.reports-export]` (line 1992): replace `log_level = "warn"` with `log_level = "debug"`.
   15. `[services.search-indexer]` (line 2200): replace `owner = "team-search"` with `owner = "team-discovery"`.
   16. `[services.shipping-worker]` (line 2456): replace `port = 9090` with `port = 9190`.
   17. `[services.tax-api]` (line 2472): replace `retries = 1` with `retries = 4`.
2. `docs/services.md`
   - Replace the row
     `| auth-gateway      | 8080 | http     | team-auth      | us-east-1      |`
     with
     `| auth-gateway      | 8081 | http     | team-auth      | us-east-1      |`
   - Replace the row
     `| search-indexer    | 9090 | grpc     | team-search    | us-east-1      |`
     with
     `| search-indexer    | 9090 | grpc     | team-discovery | us-east-1      |`

   Both new rows keep the existing column widths, so no other row changes. The other `team-search` services and the `team-search` mention in the closing paragraph stay as they are.

Every other table in `config/services.toml` keeps its values, and `config/defaults.toml` and `README.md` stay unchanged even though they contain the same key lines.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
