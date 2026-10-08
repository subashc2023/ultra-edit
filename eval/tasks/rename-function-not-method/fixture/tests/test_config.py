import json

from relay.config import Settings, load, reload


def write_settings(tmp_path, text):
    path = tmp_path / "settings.json"
    path.write_text(text, encoding="utf-8")
    return path


def test_missing_file_gives_defaults(tmp_path):
    settings = load(tmp_path / "absent.json", environ={})
    assert settings == Settings()


def test_environment_overrides_file(tmp_path):
    path = write_settings(tmp_path, json.dumps({"listen_port": 9000, "plugins": ["audit"]}))
    loaded = load(path, environ={"RELAY_LISTEN_PORT": "9100", "RELAY_PLUGINS": "audit, metrics"})
    assert loaded.listen_port == 9100
    assert loaded.plugins == ("audit", "metrics")
    assert loaded.extra == {}


def test_reload_keeps_current_settings_when_file_is_invalid(tmp_path):
    path = write_settings(tmp_path, "{not json")
    current = Settings(listen_port=9000)
    assert reload(current, path) is current
