from datetime import timedelta

import pytest

from taskclock.utils import slugify
from taskclock.timeparse import parse_duration


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("90s", timedelta(seconds=90)),
        ("15m", timedelta(minutes=15)),
        ("1h30m", timedelta(hours=1, minutes=30)),
        ("2d", timedelta(days=2)),
        (" 1H5S ", timedelta(hours=1, seconds=5)),
    ],
)
def test_parse_duration_accepts_compact_forms(text, expected):
    assert parse_duration(text) == expected


@pytest.mark.parametrize("text", ["", "15", "m", "1h 30m", "30m1h", "1h1h", "1.5h"])
def test_parse_duration_rejects_other_text(text):
    with pytest.raises(ValueError):
        parse_duration(text)


def test_parse_duration_adds_up_every_unit():
    duration = parse_duration("1d2h3m4s")
    assert duration.total_seconds() == 93784


def test_slugify_drops_punctuation_and_case():
    assert slugify("Nightly Backup (EU)") == "nightly-backup-eu"
    assert slugify("  --sync   mirrors--  ") == "sync-mirrors"


def test_slugify_keeps_digits():
    assert slugify("Rotate logs, 2nd pass") == "rotate-logs-2nd-pass"


def test_slugify_respects_separator_and_length():
    assert slugify("Rotate logs", separator="_") == "rotate_logs"
    assert len(slugify("x" * 30 + " " + "y" * 30)) == 48
