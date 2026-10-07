"""Formatting helpers for distances and opening hours."""

NBSP = "\u00a0"
EN_DASH = "\u2013"


def format_km(value):
    """Return ``"10\u00a0km"`` for 10: a no-break space keeps the unit attached."""
    return f"{value:g}{NBSP}km"


def format_hours(opens, closes):
    return f"{opens}{EN_DASH}{closes}"
