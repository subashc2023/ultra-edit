"""Thin HTTP helpers shared by the client and the CLI."""

from __future__ import annotations

import json
import time
import urllib.request

DEFAULT_HEADERS = {"Accept": "application/json", "User-Agent": "tidepool/0.9"}


def _request(url, timeout_s=30.0):
    req = urllib.request.Request(url, headers=DEFAULT_HEADERS)
    with urllib.request.urlopen(req, timeout=timeout_s) as resp:
        return resp.read()


def fetch_text(url):
    """Fetch ``url`` once and decode the body as UTF-8 text."""
    return _request(url).decode("utf-8")


def fetch_json(url, *, retries=3):
    """Fetch ``url`` and decode the JSON body, retrying on OSError."""
    last_error = None
    for attempt in range(retries):
        try:
            return json.loads(_request(url))
        except OSError as error:
            last_error = error
            time.sleep(0.25 * (attempt + 1))
    raise last_error


def fetch_json_cached(url, cache, *, retries=3):
    """Like fetch_json(url, retries=retries), but returns cache[url] when present."""
    if url not in cache:
        cache[url] = fetch_json(url, retries=retries)
    return cache[url]
