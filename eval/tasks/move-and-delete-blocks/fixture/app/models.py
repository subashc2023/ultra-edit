"""Data models shared by the scheduler and the worker."""

from __future__ import annotations

import dataclasses
import enum
from typing import Any


class JobStatus(enum.Enum):
    QUEUED = "queued"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    SKIPPED = "skipped"


@dataclasses.dataclass(frozen=True)
class Job:
    id: str
    kind: str
    params: dict[str, Any] = dataclasses.field(default_factory=dict)
    owner: str = "system"


@dataclasses.dataclass(frozen=True)
class HandlerResult:
    status: JobStatus
    output: str = ""


@dataclasses.dataclass(frozen=True)
class RetryPolicyConfig:
    """Retry settings as stored in ``worker.toml``; ``app.handlers.RetryPolicy`` applies them."""

    max_attempts: int = 5
    base_delay_s: float = 2.0
    max_delay_s: float = 300.0
