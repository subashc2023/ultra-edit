"""Layered settings for the parcelhub service.

Settings come from YAML files merged in order: config/base.yaml, then the file
for the current environment (config/<environment>.yaml) if there is one, then
an optional extra file passed with --config. The environment variables listed
in ENV_OVERRIDES are applied last, so an operator can change one value without
editing or redeploying any file.
"""

import os
from dataclasses import dataclass
from pathlib import Path

import yaml

CONFIG_DIR = Path(__file__).resolve().parents[2] / "config"

# The PostgreSQL server accepts 250 connections. Keep 10 of them free for
# migrations and maintenance sessions.
SERVER_CONNECTION_LIMIT = 240

# Environment variables that override a single key: name -> (section, key, type).
ENV_OVERRIDES = {
    "APP_DB_URL": ("database", "url", str),
    "APP_DB_POOL_SIZE": ("database", "max_connections", int),
    "APP_CACHE_POOL_SIZE": ("cache", "pool_size", int),
    "APP_HTTP_POOL_SIZE": ("http", "pool_size", int),
    "APP_LOG_LEVEL": ("logging", "level", str),
}


class ConfigError(ValueError):
    """The merged settings are incomplete or contradict each other."""


@dataclass(frozen=True)
class Settings:
    """Typed, read-only view of the merged configuration.

    Field names carry a prefix for their section (db_, cache_, http_), so code
    that receives a Settings object never has to know the YAML layout.
    """

    environment: str
    workers: int
    listen: str
    db_url: str
    db_pool_size: int
    db_pool_timeout: float
    db_statement_timeout_ms: int
    cache_url: str
    cache_pool_size: int
    cache_default_ttl: int
    http_pool_size: int
    http_timeout: float
    http_retries: int
    http_proxy: str | None
    log_level: str


def merge(base, override):
    """Return base with override merged into it, recursing into nested mappings.

    Neither argument is modified. A key that override leaves out keeps its
    value from base, so an environment file only has to list what differs.
    """
    merged = dict(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = merge(merged[key], value)
        else:
            merged[key] = value
    return merged


def _read_yaml(path):
    with open(path, encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    if not isinstance(data, dict):
        raise ConfigError(f"{path}: expected a mapping at the top level")
    return data


def _apply_env(cfg, environ):
    for name, (section, key, cast) in ENV_OVERRIDES.items():
        if name in environ:
            cfg[section] = {**cfg.get(section, {}), key: cast(environ[name])}
    return cfg


def load_config(environment, extra=None, environ=None):
    """Return the merged configuration mapping for environment.

    config/base.yaml must exist. config/<environment>.yaml and extra are both
    optional; without them the base values apply.
    """
    environ = os.environ if environ is None else environ
    cfg = _read_yaml(CONFIG_DIR / "base.yaml")
    env_file = CONFIG_DIR / f"{environment}.yaml"
    if env_file.exists():
        cfg = merge(cfg, _read_yaml(env_file))
    if extra is not None:
        cfg = merge(cfg, _read_yaml(extra))
    return _apply_env(cfg, environ)


def build_settings(cfg, environment):
    """Convert the merged mapping into Settings, with a default for each missing key."""
    service = cfg.get("service", {})
    database = cfg.get("database", {})
    cache = cfg.get("cache", {})
    http = cfg.get("http", {})
    if "url" not in database:
        raise ConfigError("database.url is required")
    return Settings(
        environment=environment,
        workers=service.get("workers", 4),
        listen=service.get("listen", "0.0.0.0:8080"),
        db_url=database["url"],
        db_pool_size=database.get("max_connections", 10),
        db_pool_timeout=database.get("pool_timeout", 5.0),
        db_statement_timeout_ms=database.get("statement_timeout_ms", 15000),
        cache_url=cache.get("url", "memory://"),
        cache_pool_size=cache.get("pool_size", 4),
        cache_default_ttl=cache.get("default_ttl", 300),
        http_pool_size=http.get("pool_size", 20),
        http_timeout=http.get("timeout", 10.0),
        http_retries=http.get("retries", 3),
        http_proxy=http.get("proxy"),
        log_level=cfg.get("logging", {}).get("level", "INFO"),
    )


def check_connection_budget(cfg, server_limit=SERVER_CONNECTION_LIMIT):
    """Fail early if the workers' database pools together exceed server_limit.

    Each worker process opens its own pool, so the server sees the pool size
    times the number of workers. cfg is the merged mapping; config/base.yaml
    supplies both values when no other file does.
    """
    workers = cfg["service"]["workers"]
    per_worker = cfg["database"]["max_connections"]
    if workers * per_worker > server_limit:
        raise ConfigError(
            f"database.max_connections ({per_worker}) x service.workers ({workers}) "
            f"needs {workers * per_worker} connections; the limit is {server_limit}"
        )


def describe_pools(cfg):
    """Return one line per connection pool, for the startup log."""
    return [
        f"database: {cfg['database']['max_connections']} connections per worker",
        f"cache: {cfg['cache']['pool_size']} connections per worker",
        f"http: {cfg['http']['pool_size']} connections per worker",
    ]


def load_settings(environment, extra=None, environ=None):
    """Load, check, and convert the configuration for environment."""
    cfg = load_config(environment, extra, environ)
    check_connection_budget(cfg)
    return build_settings(cfg, environment)
