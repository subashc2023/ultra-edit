# Installation

The current release is quakefeed 1.5.0. It supports Python 3.9 through 3.13.

## From PyPI

```console
$ python -m pip install quakefeed==1.5.0
$ python -c "import quakefeed; print(quakefeed.__version__)"
1.5.0
```

## Minimum dependency versions

quakefeed is tested against the newest releases of its dependencies and, in a
separate CI job, against the oldest ones it supports. To reproduce that job,
save these pins as `constraints-min.txt`:

```text
httpx==0.25.0
backoff==1.4.2
```

and install with them:

```console
$ python -m pip install quakefeed -c constraints-min.txt
```

## Behind a proxy

httpx reads the standard proxy variables, so requests made by quakefeed go
through the proxy named in `HTTPS_PROXY` without any quakefeed setting:

```console
$ export HTTPS_PROXY=http://10.11.4.2:3128
$ python fetch_quakes.py
```

## Upgrading from 1.4.2

Starting with 1.5.0, cached responses expire after five minutes (cache layout
2). Cache files written by quakefeed 1.4.2 and earlier never expired; they are
discarded and refetched on first use, so the first run after upgrading is
slower. To reclaim the space right away, delete the directory you pass as
`cache_dir`.

Client errors (HTTP 4xx) are no longer retried, so a bad `min_magnitude` fails
immediately with `QuakefeedError` instead of after four attempts.
