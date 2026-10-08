# quakefeed

A small Python client for the Quakefeed earthquake event API. It fetches recent
events above a magnitude threshold, retries transient failures with exponential
backoff, and caches responses on disk for a few minutes.

## Install

```console
$ pip install quakefeed
```

To pin the current release in a requirements file:

```text
quakefeed==1.4.2
```

quakefeed needs Python 3.9 or newer, `httpx` 0.25 or newer, and `backoff` 1.4.2
or newer.

## Usage

```python
from quakefeed import Client

with Client(api_key="...", cache_dir=".quakefeed-cache") as client:
    for quake in client.recent(min_magnitude=4.5, hours=24):
        print(quake.time, quake.magnitude, quake.place)
```

Every request identifies itself with a `User-Agent` header that names the
quakefeed version, so API operators can tell old clients apart.

See [docs/installation.md](docs/installation.md) for pinning, proxies, and
upgrade notes, and [CHANGELOG.md](CHANGELOG.md) for the release history.
