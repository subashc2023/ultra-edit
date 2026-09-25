from httpkit.settings import BACKOFF_SECONDS, MAX_RETRIES


def fetch(session, url):
    """Fetch url, retrying up to MAX_RETRIES times."""
    for attempt in range(MAX_RETRIES + 1):
        response = session.get(url)
        if response.ok:
            return response
        session.sleep(BACKOFF_SECONDS * (attempt + 1))
    raise RuntimeError(f"gave up after {MAX_RETRIES} retries: {url}")
