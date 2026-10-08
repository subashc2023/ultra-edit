# Changelog

## Unreleased

- Cached responses now expire after five minutes (cache layout 2). Caches
  written by 1.4.2 and earlier are discarded on first read.
- New `Client.ping()` returns the API server's banner.
- Client errors (HTTP 4xx) are no longer retried.

## 1.4.2 - 2026-08-14

- Require `backoff` 1.4.2 or newer.
- Fix parsing of events whose `mag` field is sent as a string.

## 1.4.1 - 2026-07-02

- Send a `User-Agent` header with every request.

## 1.4.0 - 2026-05-19

- Add the on-disk response cache (layout 1).
- Drop support for Python 3.8.
