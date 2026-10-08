import httpx
import pytest

from wayfetch.client import Client, FetchError
from wayfetch.defaults import USER_AGENT

BASE_URL = "https://api.example.com"


def make_client(handler, **kwargs):
    kwargs.setdefault("sleep", lambda seconds: None)
    return Client(BASE_URL, transport=httpx.MockTransport(handler), **kwargs)


def test_defaults_match_the_documentation():
    with Client(BASE_URL) as client:
        assert client.connect_timeout == 15.0
        assert client.read_timeout == 10.0
        assert client.max_retries == 3
        assert client.backoff_max == 10.0


def test_explicit_timeouts_reach_the_request():
    seen = {}

    def handler(request):
        seen.update(request.extensions["timeout"])
        return httpx.Response(204)

    with make_client(handler, connect_timeout=10, read_timeout=2.5) as client:
        client.get("/v1/ping")
    assert seen["connect"] == 10
    assert seen["read"] == 2.5


def test_timeouts_must_be_positive():
    with pytest.raises(ValueError, match="timeouts must be positive"):
        Client(BASE_URL, connect_timeout=0)


def test_requests_send_user_agent():
    seen = {}

    def handler(request):
        seen["user_agent"] = request.headers["User-Agent"]
        return httpx.Response(204)

    with make_client(handler) as client:
        client.get("/v1/ping")
    assert seen["user_agent"] == USER_AGENT


def test_body_is_returned_whole():
    with make_client(lambda request: httpx.Response(200, content=b"x" * 1010)) as client:
        response = client.get("/v1/export")
    assert len(response.content) == 1010


def test_backoff_doubles_up_to_the_cap():
    with Client(BASE_URL) as client:
        assert [client.backoff_delay(n) for n in range(1, 7)] == [1.0, 2.0, 4.0, 8.0, 10.0, 10.0]


def test_busy_server_is_retried():
    statuses = iter([503, 429, 200])
    pauses = []
    with make_client(lambda request: httpx.Response(next(statuses)), sleep=pauses.append) as client:
        response = client.get("/v1/status")
    assert response.status_code == 200
    assert pauses == [1.0, 2.0]


def test_connect_timeouts_give_up_after_max_retries():
    calls = []

    def handler(request):
        calls.append(request)
        raise httpx.ConnectTimeout("timed out", request=request)

    with make_client(handler, max_retries=2) as client, pytest.raises(FetchError):
        client.get("/v1/status")
    assert len(calls) == 3


def test_client_errors_are_not_retried():
    calls = []

    def handler(request):
        calls.append(request)
        return httpx.Response(404, text="no such report")

    with make_client(handler) as client:
        response = client.get("/v1/reports/1999-13-01")
    assert response.status_code == 404
    assert len(calls) == 1
