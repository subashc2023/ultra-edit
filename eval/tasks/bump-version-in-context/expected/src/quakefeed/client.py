"""HTTP client for the Quakefeed earthquake event API."""

from __future__ import annotations

import hashlib
import json
import time
from dataclasses import dataclass
from pathlib import Path

import backoff
import httpx

API_URL = "https://api.quakefeed.example/v1"

# Kept as a literal so this module never imports the quakefeed package itself.
# Update it together with quakefeed.__version__ (tests/test_client.py checks).
USER_AGENT = "quakefeed/1.5.0 (+https://github.com/quakefeed/quakefeed)"

# Layout 2 adds the stored_at timestamp used for expiry. Caches written by
# 1.4.2 and earlier use layout 1, which never expired, and are discarded.
CACHE_LAYOUT = 2
CACHE_TTL_SECONDS = 5 * 60


class QuakefeedError(Exception):
    """The API rejected a request."""


@dataclass(frozen=True)
class Quake:
    time: str
    magnitude: float
    place: str


def cache_key(path: str, params: dict) -> str:
    raw = json.dumps([path, params], sort_keys=True).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def _is_retryable(error: Exception) -> bool:
    if isinstance(error, httpx.TransportError):
        return True
    return isinstance(error, httpx.HTTPStatusError) and error.response.status_code >= 500


class Client:
    def __init__(self, api_key, *, cache_dir=None, transport=None, timeout=10.0):
        self._cache_dir = Path(cache_dir) if cache_dir else None
        self._http = httpx.Client(
            base_url=API_URL,
            headers={"User-Agent": USER_AGENT, "Authorization": f"Bearer {api_key}"},
            transport=transport,
            timeout=timeout,
        )

    def __enter__(self):
        return self

    def __exit__(self, *exc_info):
        self.close()

    def close(self):
        self._http.close()

    def ping(self):
        """Return the API server's banner, such as "quakefeed-api/2.3.1"."""
        return self._get("/ping")["server"]

    def recent(self, *, min_magnitude=2.5, hours=24):
        params = {"min_magnitude": min_magnitude, "hours": hours}
        payload = self._cached("/events", params)
        return [Quake(row["time"], float(row["mag"]), row["place"]) for row in payload["events"]]

    def _cached(self, path, params):
        if self._cache_dir is None:
            return self._get(path, params)
        entry_path = self._cache_dir / f"{cache_key(path, params)}.json"
        entry = self._read_cache(entry_path)
        if entry is not None:
            return entry["payload"]
        payload = self._get(path, params)
        entry = {
            "layout": CACHE_LAYOUT,
            "client": USER_AGENT,
            "stored_at": time.time(),
            "payload": payload,
        }
        self._cache_dir.mkdir(parents=True, exist_ok=True)
        entry_path.write_text(json.dumps(entry), encoding="utf-8")
        return payload

    @staticmethod
    def _read_cache(entry_path):
        try:
            entry = json.loads(entry_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        if entry.get("layout") != CACHE_LAYOUT:
            return None
        if time.time() - entry["stored_at"] > CACHE_TTL_SECONDS:
            return None
        return entry

    @backoff.on_exception(
        backoff.expo,
        httpx.HTTPError,
        max_tries=4,
        giveup=lambda error: not _is_retryable(error),
    )
    def _get(self, path, params=None):
        response = self._http.get(path, params=params)
        if response.status_code >= 500:
            response.raise_for_status()
        if response.is_error:
            raise QuakefeedError(f"{response.status_code}: {response.text}")
        return response.json()
