import pytest

from app import handlers, registry
from app.models import Job, JobStatus


def test_every_enabled_handler_has_a_queue_or_default():
    for kind in registry.ENABLED_HANDLERS:
        assert registry.queue_for(kind)


def test_retry_policy_backoff_is_capped():
    policy = handlers.RetryPolicy(max_attempts=10, base_delay_s=2.0, max_delay_s=20.0)
    assert [policy.delay_for(n) for n in range(1, 6)] == [2.0, 4.0, 8.0, 16.0, 20.0]


def test_retry_policy_does_not_retry_value_errors():
    assert not handlers.RetryPolicy().should_retry(1, ValueError("bad input"))


@pytest.mark.skipif(not hasattr(handlers, "legacy_export"), reason="legacy_export removed")
def test_legacy_export_warns(tmp_path, monkeypatch):
    monkeypatch.setattr(handlers, "_load_rows", lambda job: [])
    with pytest.deprecated_call():
        handlers.legacy_export(Job("1", "legacy-export"))


def test_run_job_reports_failure_after_last_attempt(monkeypatch):
    def boom(job):
        raise RuntimeError("disk full")

    monkeypatch.setitem(handlers._HANDLERS, "boom", (boom, handlers.RetryPolicy(max_attempts=2)))
    result = handlers.run_job(Job("7", "boom"), sleep=lambda seconds: None)
    assert result.status is JobStatus.FAILED
