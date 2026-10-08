"""Parsing and formatting of the dates, times, and durations in job files.

Job files give start dates as ISO dates ("2024-03-01"), times of day on a
24-hour clock ("07:30"), and weekdays as three-letter names ("mon,wed").
The helpers here turn that text into standard library objects and back,
and raise ValueError with the offending text when it cannot be read.
"""

import re
from datetime import date, timedelta

_DATE_RE = re.compile(r"(\d{4})-(\d{2})-(\d{2})")
_TIME_RE = re.compile(r"([01]?\d|2[0-3]):([0-5]\d)")
WEEKDAYS = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")


def parse_date(text):
    """Parse an ISO date such as "2024-03-01" into a date."""
    match = _DATE_RE.fullmatch(text.strip())
    if match is None:
        raise ValueError(f"not a date: {text!r}")
    year, month, day = (int(group) for group in match.groups())
    return date(year, month, day)


def parse_time_of_day(text):
    """Parse a 24-hour time such as "07:30" into its offset from midnight.

    The result is a timedelta, so it can be added to the midnight of any
    date to get the moment a daily job is due.
    """
    match = _TIME_RE.fullmatch(text.strip())
    if match is None:
        raise ValueError(f"not a time of day: {text!r}")
    return timedelta(hours=int(match.group(1)), minutes=int(match.group(2)))


def parse_weekdays(text):
    """Parse a comma-separated list such as "mon,wed,fri" into weekday numbers.

    Monday is 0, as in date.weekday(). The result is sorted and has no
    duplicates.
    """
    days = set()
    for name in text.split(","):
        name = name.strip().lower()
        if name not in WEEKDAYS:
            raise ValueError(f"unknown weekday {name!r} in {text!r}")
        days.add(WEEKDAYS.index(name))
    return sorted(days)


def format_duration(delta):
    """Format a timedelta compactly, for example as "1h30m" or "45s".

    Zero units are left out, so the output can be read back by
    parse_duration. A zero timedelta is formatted as "0s".
    """
    total = int(delta.total_seconds())
    if total < 0:
        raise ValueError("cannot format a negative duration")
    parts = []
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60), ("s", 1)):
        value, total = divmod(total, size)
        if value:
            parts.append(f"{value}{unit}")
    return "".join(parts) or "0s"
