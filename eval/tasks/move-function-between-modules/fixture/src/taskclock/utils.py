"""Small helpers shared across taskclock.

Nothing here knows about jobs or schedules: these functions work on plain
strings, numbers, and iterables, so any module can import them without
creating an import cycle.
"""

import random
import re
import unicodedata
from datetime import timedelta

MAX_SLUG_LENGTH = 48
_DURATION_RE = re.compile(r"(\d+)([dhms])")


def slugify(text, separator="-"):
    """Turn a job name into a lowercase identifier that is safe in file names.

    Accented letters lose their accents, every run of other characters
    becomes one separator, and the result is cut to MAX_SLUG_LENGTH.
    """
    normalized = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    words = []
    current = []
    for char in normalized.lower():
        if char.isalnum():
            current.append(char)
        elif current:
            words.append("".join(current))
            current = []
    if current:
        words.append("".join(current))
    return separator.join(words)[:MAX_SLUG_LENGTH].rstrip(separator)


def parse_duration(text):
    """Parse a duration such as "90s", "15m", or "1h30m" into a timedelta.

    The text is one or more parts, each a whole number followed by a unit:
    d for days, h for hours, m for minutes, and s for seconds. Parts are
    written from the largest unit to the smallest, with nothing between
    them, and no unit may appear twice. Case and surrounding whitespace
    are ignored.

    Raises ValueError if the text is not a duration in this form.
    """
    spec = text.strip().lower()
    parts = _DURATION_RE.findall(spec)
    if not parts or "".join(value + unit for value, unit in parts) != spec:
        raise ValueError(f"not a duration: {text!r}")
    units = [unit for _, unit in parts]
    if units != sorted(set(units), key="dhms".index):
        raise ValueError(f"duration units repeated or out of order: {text!r}")
    unit_seconds = {"d": 86400, "h": 3600, "m": 60, "s": 1}
    return timedelta(seconds=sum(int(value) * unit_seconds[unit] for value, unit in parts))


def chunked(items, size):
    """Split items into lists of at most size elements, keeping their order."""
    if size < 1:
        raise ValueError("size must be at least 1")
    chunk = []
    for item in items:
        chunk.append(item)
        if len(chunk) == size:
            yield chunk
            chunk = []
    if chunk:
        yield chunk


def clamp(value, low, high):
    """Return value limited to the closed range from low to high."""
    return max(low, min(value, high))


def jittered(delay, fraction=0.1, rng=random):
    """Return delay moved by up to fraction of itself, earlier or later.

    Spreading start times keeps jobs that share an interval from all
    starting in the same second. The result is never negative.
    """
    fraction = clamp(fraction, 0.0, 1.0)
    offset = rng.uniform(-fraction, fraction) * delay.total_seconds()
    return max(delay + timedelta(seconds=offset), timedelta(0))
