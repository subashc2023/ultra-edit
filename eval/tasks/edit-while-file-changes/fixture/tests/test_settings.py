from orders import settings


def test_timeouts_are_ordered():
    assert settings.CONNECT_TIMEOUT_S < settings.READ_TIMEOUT_S
    assert settings.POOL_TIMEOUT_S > 0


def test_backoff_is_bounded():
    assert settings.RETRY_BACKOFF_S < settings.RETRY_BACKOFF_MAX_S
    assert settings.RETRY_LIMIT >= 1


def test_as_dict_lists_only_settings():
    values = settings.as_dict()
    assert "SERVICE_NAME" in values
    assert "annotations" not in values
