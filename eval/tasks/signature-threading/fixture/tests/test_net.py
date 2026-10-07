from unittest import mock

import pytest

from pkg import net


def test_fetch_text_decodes_utf8():
    with mock.patch.object(net, "_request", return_value="caf\u00e9".encode()) as request:
        assert net.fetch_text("https://x.test/t") == "caf\u00e9"
    request.assert_called_once_with("https://x.test/t")


def test_fetch_json_decodes_body():
    with mock.patch.object(net, "_request", return_value=b'{"ok": true}') as request:
        assert net.fetch_json("https://x.test/a") == {"ok": True}
    request.assert_called_once_with("https://x.test/a")


def test_fetch_json_retries_then_raises():
    with mock.patch.object(net, "_request", side_effect=OSError("boom")) as request, \
            mock.patch.object(net.time, "sleep"):
        with pytest.raises(OSError):
            net.fetch_json("https://x.test/b", retries=2)
    assert request.call_args_list == [mock.call("https://x.test/b")] * 2


def test_fetch_json_cached_skips_network_on_hit():
    cache = {"https://x.test/c": {"cached": True}}
    with mock.patch.object(net, "_request") as request:
        assert net.fetch_json_cached("https://x.test/c", cache) == {"cached": True}
    request.assert_not_called()
