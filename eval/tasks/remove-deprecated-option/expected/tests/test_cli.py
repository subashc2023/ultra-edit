import json
import ssl

import pytest

from logship.cli import build_parser, config_from_args, format_summary, main
from logship.config import ShipperConfig
from logship.transport import ShipResult, build_ssl_context

ENDPOINT = "https://logs.example.test:6514"


def parse(*options):
    return build_parser().parse_args(["--endpoint", ENDPOINT, *options, "app.log"])


def test_defaults():
    config = config_from_args(parse())
    assert config.endpoint == ENDPOINT
    assert config.batch_size == 500
    assert config.ca_file is None
    assert config.legacy_output is False


def test_endpoint_is_required():
    with pytest.raises(SystemExit):
        build_parser().parse_args(["app.log"])


def test_legacy_output_prints_one_line_summary():
    result = ShipResult(shipped=12, failed=1)
    assert format_summary(result, legacy_output=True) == "shipped=12 failed=1"
    assert json.loads(format_summary(result)) == {"shipped": 12, "failed": 1}


def test_main_exits_nonzero_when_lines_fail(capsys):
    def fake_ship(paths, config):
        return ShipResult(shipped=2, failed=5)

    code = main(["--endpoint", ENDPOINT, "--legacy-output", "app.log"], ship_fn=fake_ship)
    assert code == 1
    assert capsys.readouterr().out == "shipped=2 failed=5\n"


def test_context_requires_tls12_by_default():
    context = build_ssl_context(ShipperConfig(endpoint=ENDPOINT))
    assert context.minimum_version == ssl.TLSVersion.TLSv1_2
    assert context.verify_mode == ssl.CERT_REQUIRED
