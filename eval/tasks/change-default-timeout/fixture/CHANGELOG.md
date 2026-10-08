# Changelog

## 2.3.0 - 2026-08-21

- Added `--max-connections` and the `[pool]` table. The pool keeps up to 100
  connections open by default.
- `backoff_max` can now be set in the config file. Its default stays 10
  seconds.

## 2.2.1 - 2026-05-02

- Fractional timeouts such as `--read-timeout 10.5` are no longer rounded down
  to whole seconds.

## 2.2.0 - 2026-03-14

- Added `--proxy` and the `[proxy]` table. `HTTPS_PROXY` is no longer read.
- Unknown keys in the config file are now an error instead of being ignored.

## 2.0.0 - 2025-10-30

- Raised the default connect timeout from 5 to 10 seconds. TLS handshakes to
  servers on other continents often took longer than 5 seconds on mobile
  networks.
- Split the single `timeout` setting into `connect_timeout` and `read_timeout`.
- Settings can now be read from a TOML config file.
