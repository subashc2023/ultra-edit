# wayfetch

wayfetch is a small HTTP client for Python and a command-line tool built on it.
It keeps separate connect and read timeouts, retries transient failures with
exponential backoff, and reuses connections through a pool.

## Install

```console
$ pip install wayfetch
```

wayfetch needs Python 3.11 or newer (for `tomllib`) and `httpx` 0.26 or newer.

## Command line

```console
$ wayfetch https://api.example.com/v1/status
$ wayfetch -o report.csv --read-timeout 60 https://reports.example.com/daily.csv
$ wayfetch --connect-timeout 3 --retries 0 https://flaky.example.com/health
```

`wayfetch --help` lists every option. The same settings can be kept in
`~/.config/wayfetch/config.toml`; [examples/wayfetch.toml](examples/wayfetch.toml)
shows the format.

## Library

```python
from wayfetch.client import Client

with Client("https://api.example.com", read_timeout=30) as client:
    response = client.get("/v1/status")
    print(response.json())
```

## Timeouts and retries

wayfetch gives up on a connection attempt after 15 seconds, and on a response
that sends no data for 10 seconds. A failed request is retried up to 3 times,
with pauses that double from 1 second to at most 10 seconds. Responses with
status 429, 502, 503, or 504 are retried too; other responses are returned as
they are. See [docs/configuration.md](docs/configuration.md) to change any of
these limits.

## License

MIT. See [CHANGELOG.md](CHANGELOG.md) for the release history.
