import json

import httpx
import pytest

import quakefeed
from quakefeed.client import CACHE_LAYOUT, USER_AGENT, Client, QuakefeedError, cache_key

FEED = {
    "events": [
        {"time": "2026-10-08T04:12:09Z", "mag": 5.1, "place": "34 km SW of Hualien City, Taiwan"},
        {"time": "2026-10-08T10:31:44Z", "mag": 4.6, "place": "Kermadec Islands region"},
    ]
}


def make_client(handler, **kwargs):
    return Client("test-key", transport=httpx.MockTransport(handler), **kwargs)


def test_user_agent_matches_package_version():
    assert USER_AGENT.startswith(f"quakefeed/{quakefeed.__version__} ")


def test_requests_send_user_agent():
    seen = {}

    def handler(request):
        seen["user_agent"] = request.headers["User-Agent"]
        return httpx.Response(200, json={"server": "quakefeed-api/1.4.20"})

    with make_client(handler) as client:
        assert client.ping() == "quakefeed-api/1.4.20"
    assert seen["user_agent"] == USER_AGENT


def test_events_are_parsed():
    with make_client(lambda request: httpx.Response(200, json=FEED)) as client:
        quakes = client.recent(min_magnitude=4.5)
    assert [quake.magnitude for quake in quakes] == [5.1, 4.6]
    assert quakes[1].place == "Kermadec Islands region"


def test_client_errors_are_not_retried():
    calls = []

    def handler(request):
        calls.append(request)
        return httpx.Response(400, text="min_magnitude must be between 0 and 10")

    with make_client(handler) as client, pytest.raises(QuakefeedError):
        client.recent(min_magnitude=12)
    assert len(calls) == 1


def test_cache_written_by_older_release_is_discarded(tmp_path):
    # A layout-1 entry exactly as quakefeed 1.4.2 wrote it: no stored_at.
    old_entry = {
        "layout": 1,
        "client": "quakefeed/1.4.2 (+https://github.com/quakefeed/quakefeed)",
        "payload": {"events": []},
    }
    entry_path = tmp_path / f"{cache_key('/events', {'min_magnitude': 2.5, 'hours': 24})}.json"
    entry_path.write_text(json.dumps(old_entry), encoding="utf-8")

    with make_client(lambda request: httpx.Response(200, json=FEED), cache_dir=tmp_path) as client:
        quakes = client.recent()

    assert len(quakes) == 2
    rewritten = json.loads(entry_path.read_text(encoding="utf-8"))
    assert rewritten["layout"] == CACHE_LAYOUT
    assert rewritten["client"] == USER_AGENT
