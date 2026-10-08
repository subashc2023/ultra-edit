"""Small helpers shared across taskclock.

Nothing here knows about jobs or schedules: these functions work on plain
strings, numbers, and iterables, so any module can import them without
creating an import cycle.
"""

import random
import unicodedata
from datetime import timedelta

MAX_SLUG_LENGTH = 48


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
