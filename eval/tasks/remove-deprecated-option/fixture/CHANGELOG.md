# Changelog

## 2.6.0 - 2026-03-02

- Deprecated `--legacy-tls`. It now prints a `DeprecationWarning` and will be
  removed in 3.0; upgrade collectors to TLS 1.2 or newer.
- Added `--tls-server-name` for collectors behind a load balancer whose
  certificate names a different host.

## 2.5.0 - 2025-11-17

- Added `--legacy-output` for scripts that parse the 1.x one-line summary.
- `--batch-size` now defaults to 500 instead of 100.

## 2.0.0 - 2025-04-08

- The summary printed on exit is now JSON.
- TLS 1.2 is required by default. Pass `--legacy-tls` to reach collectors that
  only support TLS 1.0 or 1.1.
