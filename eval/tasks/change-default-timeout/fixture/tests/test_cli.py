import pytest

from wayfetch import cli
from wayfetch.defaults import (
    DEFAULT_BACKOFF_MAX,
    DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_MAX_RETRIES,
    DEFAULT_READ_TIMEOUT,
)

URL = "https://api.example.com/v1/status"


@pytest.fixture(autouse=True)
def no_user_config(monkeypatch, tmp_path):
    # Keep a config file in the developer's home directory out of the tests.
    monkeypatch.setattr(cli, "CONFIG_PATH", str(tmp_path / "absent.toml"))


def settings_for(*argv):
    return cli.resolve_settings(cli.build_parser().parse_args([*argv, URL]))


def write_config(tmp_path, text):
    path = tmp_path / "wayfetch.toml"
    path.write_text(text, encoding="utf-8")
    return path


def test_settings_fall_back_to_defaults():
    settings = settings_for()
    assert settings["connect_timeout"] == DEFAULT_CONNECT_TIMEOUT
    assert settings["read_timeout"] == DEFAULT_READ_TIMEOUT
    assert settings["max_retries"] == DEFAULT_MAX_RETRIES
    assert settings["backoff_max"] == DEFAULT_BACKOFF_MAX
    assert settings["proxy"] is None


def test_config_file_overrides_defaults(tmp_path):
    path = write_config(tmp_path, "[http]\nconnect_timeout = 2.5\nread_timeout = 10.5\n")
    settings = settings_for("--config", str(path))
    assert settings["connect_timeout"] == 2.5
    assert settings["read_timeout"] == 10.5
    assert settings["max_retries"] == DEFAULT_MAX_RETRIES


def test_flags_override_config_file(tmp_path):
    path = write_config(tmp_path, '[http]\nconnect_timeout = 2.5\n\n[proxy]\nurl = "http://127.0.0.1:10080"\n')
    settings = settings_for("--config", str(path), "--connect-timeout", "10", "--proxy", "http://proxy.internal:3128")
    assert settings["connect_timeout"] == 10.0
    assert settings["proxy"] == "http://proxy.internal:3128"


def test_unknown_config_key_is_an_error(tmp_path, capsys):
    path = write_config(tmp_path, "[http]\nconnect_timout = 3\n")
    assert cli.main(["--config", str(path), URL]) == 2
    assert "unknown setting [http] connect_timout" in capsys.readouterr().err


def test_missing_config_file_named_on_the_command_line_is_an_error(tmp_path, capsys):
    assert cli.main(["--config", str(tmp_path / "nope.toml"), URL]) == 2
    assert "nope.toml" in capsys.readouterr().err


def test_help_lists_every_setting(capsys):
    with pytest.raises(SystemExit):
        cli.main(["--help"])
    out = capsys.readouterr().out
    for flag in ("--connect-timeout", "--read-timeout", "--retries", "--backoff-max", "--max-connections", "--proxy"):
        assert flag in out
