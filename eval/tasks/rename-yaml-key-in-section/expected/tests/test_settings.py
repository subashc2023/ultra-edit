import textwrap

import pytest
import yaml

from parcelhub import settings as settings_module
from parcelhub.settings import (
    ConfigError,
    build_settings,
    check_connection_budget,
    describe_pools,
    load_config,
    load_settings,
    merge,
)


def write_yaml(path, text):
    path.write_text(textwrap.dedent(text), encoding="utf-8")
    return path


@pytest.fixture
def config_dir(tmp_path, monkeypatch):
    write_yaml(
        tmp_path / "base.yaml",
        """\
        service:
          workers: 4
        database:
          url: postgresql://localhost/parcelhub
          max_connections: 10
          pool_timeout: 5.0
        cache:
          url: redis://localhost:6379/0
          pool_size: 4
        http:
          pool_size: 20
        """,
    )
    monkeypatch.setattr(settings_module, "CONFIG_DIR", tmp_path)
    return tmp_path


def test_environment_file_overrides_base(config_dir):
    write_yaml(
        config_dir / "production.yaml",
        """\
        service:
          workers: 8
        database:
          max_connections: 25
        cache:
          pool_size: 8
        """,
    )
    settings = load_settings("production", environ={})
    assert settings.workers == 8
    assert settings.db_pool_size == 25
    assert settings.db_pool_timeout == 5.0
    assert settings.cache_pool_size == 8
    assert settings.http_pool_size == 20


def test_staging_keeps_the_base_database_pool(config_dir):
    write_yaml(
        config_dir / "staging.yaml",
        """\
        database:
          url: postgresql://db.staging.internal/parcelhub_staging
        cache:
          pool_size: 6
        """,
    )
    settings = load_settings("staging", environ={})
    assert settings.db_url.endswith("/parcelhub_staging")
    assert settings.db_pool_size == 10
    assert settings.cache_pool_size == 6


def test_extra_file_is_merged_last(config_dir, tmp_path_factory):
    extra = write_yaml(
        tmp_path_factory.mktemp("deploy") / "service.yaml",
        """\
        http:
          pool_size: 10
          proxy: http://proxy.corp.example:3128
        """,
    )
    settings = load_settings("windows", extra=extra, environ={})
    assert settings.http_pool_size == 10
    assert settings.http_proxy == "http://proxy.corp.example:3128"
    assert settings.db_pool_size == 10


def test_database_pool_size_env_override(config_dir):
    settings = load_settings("production", environ={"APP_DB_POOL_SIZE": "3"})
    assert settings.db_pool_size == 3
    assert settings.cache_pool_size == 4


def test_describe_pools_lists_every_pool(config_dir):
    assert describe_pools(load_config("production", environ={})) == [
        "database: 10 connections per worker",
        "cache: 4 connections per worker",
        "http: 20 connections per worker",
    ]


def test_missing_database_pool_size_uses_default():
    settings = build_settings({"database": {"url": "postgresql://localhost/x"}}, "dev")
    assert settings.db_pool_size == 10
    assert settings.cache_pool_size == 4


def test_missing_database_url_is_an_error():
    with pytest.raises(ConfigError, match="database.url is required"):
        build_settings({"database": {}}, "dev")


def test_merge_keeps_keys_the_override_leaves_out():
    base = {"cache": {"pool_size": 4, "default_ttl": 300}, "logging": {"level": "INFO"}}
    override = {"cache": {"pool_size": 8}}
    assert merge(base, override) == {
        "cache": {"pool_size": 8, "default_ttl": 300},
        "logging": {"level": "INFO"},
    }
    assert base["cache"]["pool_size"] == 4


def test_connection_budget_rejects_an_oversized_pool():
    cfg = yaml.safe_load(
        textwrap.dedent(
            """\
            service:
              workers: 8
            database:
              max_connections: 40
            """
        )
    )
    with pytest.raises(ConfigError, match=r"database\.max_connections \(40\)"):
        check_connection_budget(cfg, server_limit=240)


def test_connection_budget_allows_the_production_pool():
    cfg = {"service": {"workers": 8}, "database": {"max_connections": 25}}
    check_connection_budget(cfg, server_limit=240)
