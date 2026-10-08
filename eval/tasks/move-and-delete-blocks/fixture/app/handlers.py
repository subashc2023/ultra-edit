"""Job handlers for the worker service.

Each handler is registered with ``@register(name, retry=...)`` and receives a
``Job``. A handler returns a ``HandlerResult``; raising marks the attempt as
failed, and ``run_job`` consults the handler's ``RetryPolicy`` to decide
whether the job is retried.
"""

from __future__ import annotations

import csv
import datetime as dt
import functools
import json
import logging
import time
import warnings
from typing import Callable, Iterable

from app.models import HandlerResult, Job, JobStatus
from app.audit import record_event
from app.config import settings
from app.storage import open_output, upload

__all__ = [
    "RetryPolicy",
    "export_csv",
    "export_json",
    "get_handler",
    "import_jobs",
    "legacy_export",
    "purge_expired",
    "register",
    "reindex",
    "run_job",
    "send_digest",
]

logger = logging.getLogger(__name__)


DEFAULT_TIMEOUT_S = 30
EXPORT_COLUMNS = ("id", "name", "status", "owner", "created_at", "finished_at")
LEGACY_EXPORT_COLUMNS = ("id", "name", "status", "created")
DIGEST_MAX_ITEMS = 50

Handler = Callable[[Job], HandlerResult]
_HANDLERS: dict[str, tuple[Handler, "RetryPolicy"]] = {}


def register(name: str, *, retry: "RetryPolicy") -> Callable[[Handler], Handler]:
    """Register ``func`` as the handler for jobs of kind ``name``."""

    def decorator(func: Handler) -> Handler:
        if name in _HANDLERS:
            raise ValueError(f"handler {name!r} is already registered")
        _HANDLERS[name] = (func, retry)
        return func

    return decorator


def deprecated(message: str) -> Callable[[Handler], Handler]:
    """Emit a ``DeprecationWarning`` with ``message`` each time the handler runs."""

    def decorator(func: Handler) -> Handler:
        @functools.wraps(func)
        def wrapper(job: Job) -> HandlerResult:
            warnings.warn(message, DeprecationWarning, stacklevel=2)
            return func(job)

        return wrapper

    return decorator


def get_handler(name: str) -> tuple[Handler, "RetryPolicy"]:
    try:
        return _HANDLERS[name]
    except KeyError:
        raise LookupError(f"no handler registered for {name!r}") from None


def _load_rows(job: Job) -> list[dict[str, str]]:
    """Return the rows selected by ``job.params["query"]`` as dictionaries."""
    query = job.params.get("query", "")
    with open_output(settings.snapshot_dir / "jobs.jsonl") as handle:
        rows = [json.loads(line) for line in handle if line.strip()]
    if query:
        rows = [row for row in rows if query.lower() in row.get("name", "").lower()]
    return rows


@register("export-csv", retry=RetryPolicy(max_attempts=3))
def export_csv(job: Job) -> HandlerResult:
    """Write the selected rows as CSV with the current six columns."""
    rows = _load_rows(job)
    path = settings.export_dir / f"{job.id}.csv"
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=EXPORT_COLUMNS, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)
    url = upload(path, content_type="text/csv")
    record_event("export", job_id=job.id, rows=len(rows), format="csv")
    return HandlerResult(status=JobStatus.SUCCEEDED, output=url)


@register("export-json", retry=RetryPolicy(max_attempts=3))
def export_json(job: Job) -> HandlerResult:
    """Write the selected rows as a JSON array."""
    rows = _load_rows(job)
    path = settings.export_dir / f"{job.id}.json"
    payload = [{column: row.get(column) for column in EXPORT_COLUMNS} for row in rows]
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    url = upload(path, content_type="application/json")
    record_event("export", job_id=job.id, rows=len(rows), format="json")
    return HandlerResult(status=JobStatus.SUCCEEDED, output=url)


# Deprecated since 2.0: use export-csv. Scheduled for removal in 3.0.
@deprecated("legacy-export is deprecated; use export-csv")
@register("legacy-export", retry=RetryPolicy(max_attempts=1))
def legacy_export(job: Job) -> HandlerResult:
    """Write the pre-2.0 four-column CSV (no owner, no finished_at)."""
    rows = _load_rows(job)
    path = settings.export_dir / f"{job.id}-legacy.csv"
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(LEGACY_EXPORT_COLUMNS)

        for row in rows:
            writer.writerow([row.get(column, "") for column in LEGACY_EXPORT_COLUMNS])
    logger.warning("legacy-export used by job %s", job.id)
    url = upload(path, content_type="text/csv")
    return HandlerResult(status=JobStatus.SUCCEEDED, output=url)


@register("import-jobs", retry=RetryPolicy(max_attempts=5, base_delay_s=10.0))
def import_jobs(job: Job) -> HandlerResult:
    """Import rows from an uploaded CSV in either the current or the legacy layout."""
    source = job.params["source"]
    imported = 0
    skipped = 0
    with open_output(source) as handle:
        reader = csv.reader(handle)
        header = next(reader, [])
        if tuple(header) == LEGACY_EXPORT_COLUMNS:
            columns = LEGACY_EXPORT_COLUMNS
        elif tuple(header) == EXPORT_COLUMNS:
            columns = EXPORT_COLUMNS
        else:
            return HandlerResult(status=JobStatus.FAILED, output=f"unrecognized header: {header!r}")
        for values in reader:
            if len(values) != len(columns):
                skipped += 1
                continue
            record_event("import-row", job_id=job.id, row=dict(zip(columns, values)))
            imported += 1
    record_event("import", job_id=job.id, imported=imported, skipped=skipped)
    return HandlerResult(status=JobStatus.SUCCEEDED, output=f"{imported} imported, {skipped} skipped")


@register("purge-expired", retry=RetryPolicy(max_attempts=2))
def purge_expired(job: Job) -> HandlerResult:
    """Delete exports older than ``settings.export_ttl_days``."""
    cutoff = time.time() - settings.export_ttl_days * 86400
    removed = 0
    for path in sorted(settings.export_dir.glob("*")):
        if path.is_file() and path.stat().st_mtime < cutoff:
            path.unlink()
            removed += 1
    logger.info("purged %d expired exports", removed)
    return HandlerResult(status=JobStatus.SUCCEEDED, output=f"{removed} removed")


@register("reindex", retry=RetryPolicy(max_attempts=4, max_delay_s=60.0))
def reindex(job: Job) -> HandlerResult:
    """Rebuild the search index for the rows selected by the job's query."""
    rows = _load_rows(job)
    batch_size = int(job.params.get("batch_size", 500))
    batches = 0
    for start in range(0, len(rows), batch_size):
        batch = rows[start : start + batch_size]
        record_event("reindex-batch", job_id=job.id, size=len(batch))
        batches += 1
    return HandlerResult(status=JobStatus.SUCCEEDED, output=f"{len(rows)} rows in {batches} batches")


def _digest_lines(rows: Iterable[dict[str, str]]) -> list[str]:
    lines = []
    for row in rows:
        status = row.get("status", "unknown")
        lines.append(f"- {row.get('name', '?')} [{status}]")
        if len(lines) >= DIGEST_MAX_ITEMS:
            lines.append(f"- ... and more (showing the first {DIGEST_MAX_ITEMS})")
            break
    return lines


@register("send-digest", retry=RetryPolicy(max_attempts=3, base_delay_s=30.0))
def send_digest(job: Job) -> HandlerResult:
    """E-mail a summary of the selected rows to ``job.params["to"]``."""
    rows = _load_rows(job)
    if not rows:
        return HandlerResult(status=JobStatus.SKIPPED, output="nothing to send")
    today = dt.date.today().isoformat()
    body = "\n".join([f"Job digest for {today}", "", *_digest_lines(rows)]) + "\n"
    record_event("digest", job_id=job.id, to=job.params["to"], rows=len(rows))
    return HandlerResult(status=JobStatus.SUCCEEDED, output=body)


def run_job(job: Job, *, sleep: Callable[[float], None] = time.sleep) -> HandlerResult:
    """Run ``job`` with its handler, retrying according to the handler's policy."""
    handler, policy = get_handler(job.kind)
    attempt = 1
    while True:
        started = time.monotonic()
        try:
            result = handler(job)
        except Exception as error:  # noqa: BLE001 - every failure is retried or reported
            elapsed = time.monotonic() - started
            logger.warning("job %s attempt %d failed after %.1fs: %s", job.id, attempt, elapsed, error)
            if not policy.should_retry(attempt, error):
                record_event("job-failed", job_id=job.id, attempts=attempt, error=str(error))
                return HandlerResult(status=JobStatus.FAILED, output=str(error))
            sleep(policy.delay_for(attempt))
            attempt += 1
            continue
        record_event("job-finished", job_id=job.id, attempts=attempt, status=result.status.value)
        return result


class RetryPolicy:
    """Exponential backoff for failed handler attempts.

    ``delay_for(attempt)`` returns the seconds to wait after the given 1-based
    attempt, doubling from ``base_delay_s`` and capped at ``max_delay_s``.
    """

    def __init__(self, *, max_attempts: int = 5, base_delay_s: float = 2.0, max_delay_s: float = 300.0) -> None:
        if max_attempts < 1:
            raise ValueError("max_attempts must be at least 1")
        self.max_attempts = max_attempts
        self.base_delay_s = base_delay_s
        self.max_delay_s = max_delay_s

    def should_retry(self, attempt: int, error: BaseException) -> bool:
        if isinstance(error, (ValueError, PermissionError)):
            return False
        return attempt < self.max_attempts

    def delay_for(self, attempt: int) -> float:
        return min(self.base_delay_s * 2 ** (attempt - 1), self.max_delay_s)

    def __repr__(self) -> str:
        return (
            f"RetryPolicy(max_attempts={self.max_attempts}, "
            f"base_delay_s={self.base_delay_s}, max_delay_s={self.max_delay_s})"
        )
