# httpkit

A tiny HTTP helper.

## Retries

`fetch` retries a failed request up to `MAX_RETRIES` times, sleeping
`BACKOFF_SECONDS` longer before each new attempt. Each host is further limited
by `MAX_RETRIES_PER_HOST`; see [configuration](docs/configuration.md).

Change `MAX_RETRIES` in `src/httpkit/settings.py`.
