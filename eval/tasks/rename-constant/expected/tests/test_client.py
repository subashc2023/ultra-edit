from httpkit.client import fetch
from httpkit.settings import RETRY_LIMIT


def test_gives_up_after_max_retries(flaky_session):
    flaky_session.fail_next(RETRY_LIMIT + 1)
    try:
        fetch(flaky_session, "https://example.test/")
    except RuntimeError:
        pass
    else:
        raise AssertionError("expected RuntimeError")
