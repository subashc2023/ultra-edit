from httpkit.settings import BACKOFF_SECONDS, RETRY_LIMIT


def fetch(session, url):
    """Fetch url, retrying up to RETRY_LIMIT times."""
    for attempt in range(RETRY_LIMIT + 1):
        response = session.get(url)
        if response.ok:
            return response
        session.sleep(BACKOFF_SECONDS * (attempt + 1))
    raise RuntimeError(f"gave up after {RETRY_LIMIT} retries: {url}")
