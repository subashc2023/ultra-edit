# httpkit

A tiny HTTP helper.

## Retries

`fetch` retries a failed request up to `RETRY_LIMIT` times, sleeping
`BACKOFF_SECONDS` longer before each new attempt. Each host is further limited
by `MAX_RETRIES_PER_HOST`; see [configuration](docs/configuration.md).

Change `RETRY_LIMIT` in `src/httpkit/settings.py`.
