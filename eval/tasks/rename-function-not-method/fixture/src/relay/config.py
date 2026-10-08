"""Runtime settings for the relay service.

Settings come from a JSON or YAML file that is read once at startup.
Environment variables that start with RELAY_ override values from the file,
so every container can load the same file and still differ per deployment.
"""

import json
import os
from dataclasses import dataclass, field, fields
from pathlib import Path

import yaml

DEFAULT_PATH = Path("/etc/relay/settings.json")
ENV_PREFIX = "RELAY_"


@dataclass(frozen=True)
class Settings:
    listen_host: str = "127.0.0.1"
    listen_port: int = 8080
    cache_dir: str = "/var/cache/relay"
    plugins: tuple = ()
    retry_limit: int = 3
    extra: dict = field(default_factory=dict)


def _read_file(path):
    with path.open(encoding="utf-8") as handle:
        if path.suffix in (".yaml", ".yml"):
            return yaml.safe_load(handle) or {}
        return json.load(handle)


def _env_overrides(environ):
    overrides = {}
    for key, value in environ.items():
        if key.startswith(ENV_PREFIX):
            overrides[key[len(ENV_PREFIX):].lower()] = value
    return overrides


def _coerce(name, value):
    if name in ("listen_port", "retry_limit"):
        return int(value)
    if name == "plugins":
        return tuple(part.strip() for part in value.split(",") if part.strip())
    return value


def load(path=None, environ=None):
    """Load settings from path, then apply environment overrides.

    A missing file is not an error: the defaults apply, so a fresh checkout
    can load settings without any setup.
    """
    path = DEFAULT_PATH if path is None else Path(path)
    environ = os.environ if environ is None else environ
    loaded = _read_file(path) if path.exists() else {}
    for name, value in _env_overrides(environ).items():
        loaded[name] = _coerce(name, value)
    names = {item.name for item in fields(Settings)} - {"extra"}
    known = {name: loaded.pop(name) for name in list(loaded) if name in names}
    if "plugins" in known:
        known["plugins"] = tuple(known["plugins"])
    return Settings(**known, extra=loaded)


def reload(current, path=None):
    """Load the file again, keeping the current settings if it is now invalid."""
    try:
        return load(path)
    except (OSError, ValueError, yaml.YAMLError):
        return current
