"""Feature flags.

Each flag is a module attribute holding a bool. It is read once, when this
module is first imported, from the environment variable named beside it, so an
operator can switch a flag without a release. Code reads a flag as
``flags.NAME`` at the point of use, which also lets a test switch it with
``mock.patch.object``.
"""

import os

_TRUE_VALUES = frozenset({"1", "true", "yes", "on"})


def _env_flag(name, default):
    """Return the flag set in environment variable name, or default if unset."""
    value = os.environ.get(name, "").strip()
    if not value:
        return default
    return value.lower() in _TRUE_VALUES


# Reject records that carry fields the schema does not list, instead of storing
# them as they are. Off until every agent sends schema 3 records.
REJECT_UNKNOWN_FIELDS = _env_flag("SLUICE_REJECT_UNKNOWN_FIELDS", default=False)

# Write records to the sink from a background thread instead of inline. Off
# until the writer's queue limit has been load tested.
USE_ASYNC_WRITER = _env_flag("SLUICE_ASYNC_WRITER", default=False)
