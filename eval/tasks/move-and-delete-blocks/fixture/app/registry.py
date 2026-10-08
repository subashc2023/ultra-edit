"""Which handlers the scheduler may enqueue, and on which queue.

The scheduler reads ENABLED_HANDLERS at startup; a job whose kind is not listed
here is rejected with HTTP 422.
"""

from __future__ import annotations

ENABLED_HANDLERS = [
    "export-csv",
    "export-json",
    "legacy-export",
    "import-jobs",
    "legacy-import",  # replaced by import-jobs in 2.4
    "purge-expired",
    "reindex",  # nightly at 02:00 UTC
    "legacy-reindex",
    "send-digest",
]

# Kept for the 3.0 migration report. Do not edit by hand.
REMOVED_IN_3_0 = [
    "legacy-export",
    "legacy-import",
    "legacy-reindex",
]

HANDLER_QUEUES = {
    "export-csv": "exports",
    "export-json": "exports",
    "legacy-export": "exports",
    "import-jobs": "imports",
    "purge-expired": "maintenance",
    "reindex": "maintenance",
    "send-digest": "notifications",
}


def queue_for(kind: str) -> str:
    if kind not in ENABLED_HANDLERS:
        raise LookupError(f"handler {kind!r} is not enabled")
    return HANDLER_QUEUES.get(kind, "default")
