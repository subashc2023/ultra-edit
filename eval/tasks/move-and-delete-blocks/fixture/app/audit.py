"""Structured audit events."""

from __future__ import annotations

import json
import logging
import time

audit_logger = logging.getLogger("worker.audit")


def record_event(event: str, **fields: object) -> None:
    audit_logger.info(json.dumps({"event": event, "ts": time.time(), **fields}, default=str))
