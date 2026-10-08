"""The relay service: receives webhook events and forwards them to targets."""

import json
import os
import signal
import time
from pathlib import Path

from .config import DEFAULT_PATH, load_config, reload


class Cache:
    """A small on-disk JSON cache that keeps one file per key."""

    def __init__(self, directory):
        self.directory = Path(directory)

    def _path(self, key):
        return self.directory / f"{key}.json"

    def load(self, key, default=None):
        """Load the value stored under key, or return default if there is none."""
        path = self._path(key)
        if not path.exists():
            return default
        with path.open(encoding="utf-8") as handle:
            return json.load(handle)

    def store(self, key, value):
        self.directory.mkdir(parents=True, exist_ok=True)
        with self._path(key).open("w", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True)


class RelayService:
    max_load = 4.0

    def __init__(self, settings, path=DEFAULT_PATH):
        self.settings = settings
        self.path = path
        self.cache = Cache(settings.cache_dir)
        # Routes survive restarts, so load them from the cache before serving.
        self.routes = self.cache.load("routes", default={})
        self.started = time.monotonic()

    @classmethod
    def from_file(cls, path=DEFAULT_PATH):
        return cls(load_config(path), path)

    def add_route(self, event, target):
        self.routes.setdefault(event, []).append(target)
        self.cache.store("routes", self.routes)

    def targets(self, event):
        return list(self.routes.get(event, ()))

    def refresh(self):
        self.settings = reload(self.settings, self.path)

    def health(self):
        load = os.getloadavg()[0]
        return {
            "status": "ok" if load < self.max_load else "busy",
            "load": round(load, 2),
            "uptime": int(time.monotonic() - self.started),
            "routes": len(self.routes),
        }

    def run(self):
        """Block until interrupted, reading the settings file again on SIGHUP."""
        signal.signal(signal.SIGHUP, lambda signum, frame: self.refresh())
        while True:
            signal.pause()
