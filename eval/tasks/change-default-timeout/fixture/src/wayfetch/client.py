"""A small HTTP client with separate connect and read timeouts and retries."""

from __future__ import annotations

import time
from typing import Callable

import httpx

from wayfetch.defaults import (
    DEFAULT_BACKOFF_BASE,
    DEFAULT_BACKOFF_MAX,
    DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_MAX_CONNECTIONS,
    DEFAULT_MAX_RETRIES,
    DEFAULT_READ_TIMEOUT,
    RETRY_STATUSES,
    USER_AGENT,
)


class FetchError(Exception):
    """A request failed on every attempt."""


class Client:
    """Send HTTP requests, retrying connection errors and busy servers.

    connect_timeout and read_timeout are in seconds. connect_timeout bounds DNS
    resolution, the TCP handshake, and TLS; read_timeout bounds each wait for
    response data. Every keyword argument defaults to the matching value in
    wayfetch.defaults.

    base_url may be empty, in which case every request needs an absolute URL.
    """

    def __init__(
        self,
        base_url: str = "",
        *,
        connect_timeout: float = DEFAULT_CONNECT_TIMEOUT,
        read_timeout: float = DEFAULT_READ_TIMEOUT,
        max_retries: int = DEFAULT_MAX_RETRIES,
        backoff_max: float = DEFAULT_BACKOFF_MAX,
        max_connections: int = DEFAULT_MAX_CONNECTIONS,
        proxy: str | None = None,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        if connect_timeout <= 0 or read_timeout <= 0:
            raise ValueError("timeouts must be positive")
        self.connect_timeout = connect_timeout
        self.read_timeout = read_timeout
        self.max_retries = max_retries
        self.backoff_max = backoff_max
        self._sleep = sleep
        self._http = httpx.Client(
            base_url=base_url,
            # Writes and waits for a pooled connection share the read limit.
            timeout=httpx.Timeout(read_timeout, connect=connect_timeout),
            limits=httpx.Limits(max_connections=max_connections),
            proxy=proxy,
            transport=transport,
            headers={"User-Agent": USER_AGENT},
            # The proxy comes only from the settings, never from HTTPS_PROXY.
            trust_env=False,
        )

    def __enter__(self) -> Client:
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def close(self) -> None:
        self._http.close()

    def backoff_delay(self, retry: int) -> float:
        """Seconds to wait before the given retry (1 for the first retry)."""
        return min(self.backoff_max, DEFAULT_BACKOFF_BASE * 2**retry)

    def request(self, method: str, url: str, **kwargs) -> httpx.Response:
        """Send a request, retrying until it succeeds or the retries run out.

        Connection errors and timeouts raise FetchError once every attempt has
        failed. A response with a status in RETRY_STATUSES is retried too, but
        the last such response is returned rather than raised, so the caller
        can still read its body.
        """
        retry = 0
        while True:
            try:
                response = self._http.request(method, url, **kwargs)
            except (httpx.ConnectError, httpx.TimeoutException) as exc:
                if retry >= self.max_retries:
                    raise FetchError(f"{method} {url}: {exc}") from exc
            else:
                if response.status_code not in RETRY_STATUSES or retry >= self.max_retries:
                    return response
                response.close()
            retry += 1
            self._sleep(self.backoff_delay(retry))

    def get(self, url: str, **kwargs) -> httpx.Response:
        return self.request("GET", url, **kwargs)

    def head(self, url: str, **kwargs) -> httpx.Response:
        return self.request("HEAD", url, **kwargs)
