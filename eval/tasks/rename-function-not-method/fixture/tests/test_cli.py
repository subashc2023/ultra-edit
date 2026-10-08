from unittest import mock

from relay import cli
from relay.config import Settings


@mock.patch("relay.cli.RelayService")
@mock.patch("relay.plugins.load")
@mock.patch("relay.config.load")
def test_main_sets_up_each_plugin_in_order(settings_mock, plugin_mock, service_mock):
    settings_mock.return_value = Settings(plugins=("audit",))
    assert cli.main(["--config", "relay.json", "--plugin", "metrics"]) == 0
    settings_mock.assert_called_once_with("relay.json")
    assert [call.args for call in plugin_mock.call_args_list] == [("audit",), ("metrics",)]
    service_mock.assert_called_once_with(settings_mock.return_value, "relay.json")
    service_mock.return_value.run.assert_called_once_with()


def test_check_reports_invalid_settings(tmp_path, capsys):
    path = tmp_path / "broken.json"
    path.write_text("{not json", encoding="utf-8")
    assert cli.main(["--config", str(path), "--check"]) == 2
    assert "cannot read settings from" in capsys.readouterr().err
