import subprocess
from datetime import timedelta

import pytest

from taskclock.runner import Runner, parse_duration_ms
from taskclock.schedule import Job


def make_job(**changes):
    fields = {"name": "Sync mirrors", "command": ("sync-mirrors",), "interval": timedelta(minutes=15)}
    fields.update(changes)
    return Job(**fields)


@pytest.mark.parametrize(
    ("text", "expected"),
    [("500ms", 500), ("2s", 2000), ("1m", 60000), (" 250MS ", 250)],
)
def test_parse_duration_ms(text, expected):
    assert parse_duration_ms(text) == expected


def test_parse_duration_ms_rejects_fractions():
    with pytest.raises(ValueError):
        parse_duration_ms("1.5s")


def test_run_job_passes_name_and_status_to_each_hook():
    calls = []

    def fake_run(args, timeout):
        calls.append((args, timeout))
        return subprocess.CompletedProcess(args, 3 if args[0] == "sync-mirrors" else 0)

    runner = Runner([], hook_timeout_ms=500, run=fake_run)
    job = make_job(hooks=(("notify", "--channel", "ops"),))
    assert runner.run_job(job) == 3
    assert calls == [
        (["sync-mirrors"], 930.0),
        (["notify", "--channel", "ops", "Sync mirrors", "3"], 0.5),
    ]


def test_hook_timeout_is_logged_not_raised(caplog):
    def fake_run(args, timeout):
        if args[0] == "notify":
            raise subprocess.TimeoutExpired(args, timeout)
        return subprocess.CompletedProcess(args, 0)

    runner = Runner([], run=fake_run)
    assert runner.run_job(make_job(hooks=(("notify",),))) == 0
    assert "timed out after 2000 ms" in caplog.text
