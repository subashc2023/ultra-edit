"""Client for the Quakefeed earthquake event API."""

from quakefeed.client import Client, Quake, QuakefeedError

__version__ = "1.4.2"

__all__ = ["Client", "Quake", "QuakefeedError", "__version__"]
