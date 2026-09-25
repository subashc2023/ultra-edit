from httpkit.client import fetch
from httpkit.settings import MAX_RETRIES


def test_gives_up_after_max_retries(flaky_session):
    flaky_session.fail_next(MAX_RETRIES + 1)
    try:
        fetch(flaky_session, "https://example.test/")
    except RuntimeError:
        pass
    else:
        raise AssertionError("expected RuntimeError")
