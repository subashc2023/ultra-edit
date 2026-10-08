"""Pull batches from a source, decode them, and write the records to a sink.

A source is any object whose ``batches()`` method yields ``Batch`` objects,
such as the HTTP receiver or the spool-directory reader. A sink is any object
with ``write(records)`` and ``flush()`` methods.
"""

import itertools
import logging
import queue
import threading
import time
from dataclasses import dataclass, field

from . import flags
from .decode import DecodeError, decode_batch

log = logging.getLogger(__name__)

# Records handed to the sink per write. Warehouse inserts are fastest around
# this size; much larger slices make a single failed insert expensive to retry.
WRITE_SLICE = 500


@dataclass
class Batch:
    """One payload as received from an agent."""

    batch_id: str
    payload: object  # binary file-like object, positioned at the first byte
    received_at: float = field(default_factory=time.time)


@dataclass
class IngestStats:
    batches: int = 0
    records: int = 0
    rejected: int = 0
    seconds: float = 0.0


class BackgroundWriter:
    """Write records to a sink from one background thread.

    ``submit`` returns as soon as the records are queued, and blocks only when
    ``max_pending`` writes are already waiting. ``close`` waits for the queue to
    drain, flushes the sink, and re-raises the first error the thread hit.
    """

    def __init__(self, sink, max_pending=64):
        self.sink = sink
        self._queue = queue.Queue(maxsize=max_pending)
        self._error = None
        self._thread = threading.Thread(target=self._run, name="sluice-writer", daemon=True)
        self._thread.start()

    def submit(self, records):
        self._queue.put(records)

    def close(self):
        self._queue.put(None)
        self._thread.join()
        if self._error is not None:
            raise self._error
        self.sink.flush()

    def _run(self):
        while True:
            records = self._queue.get()
            if records is None:
                return
            if self._error is not None:
                continue
            try:
                self.sink.write(records)
            except Exception as exc:  # reported by close()
                self._error = exc


def _slices(records, size):
    """Yield lists of at most size items from the iterable records."""
    iterator = iter(records)
    while True:
        chunk = list(itertools.islice(iterator, size))
        if not chunk:
            return
        yield chunk


def ingest(source, sink, schema):
    """Decode every batch from source and write its records to sink.

    A batch that fails to decode is logged and counted as rejected, and the
    batches after it are still ingested. Returns an ``IngestStats``.
    """
    stats = IngestStats()
    started = time.monotonic()
    if flags.USE_ASYNC_WRITER:
        # The writer thread owns the sink until it is closed below.
        writer = BackgroundWriter(sink)
        write = writer.submit
    else:
        writer = None
        write = sink.write

    # Batches are streamed from the source one at a time. Never collect them
    # into a list: a spool directory can hold hours of backlog.
    for batch in source.batches():
        stats.batches += 1
        try:
            if flags.USE_STREAMING_DECODER:
                # Records are written in slices as they are decoded, so a large
                # batch is never held in memory whole.
                records = decode_batch(batch.payload, schema)
                for chunk in _slices(records, WRITE_SLICE):
                    write(chunk)
                    stats.records += len(chunk)
            else:
                # The whole batch is decoded before any of it is written.
                records = decode_batch(batch.payload, schema)
                write(records)
                stats.records += len(records)
        except DecodeError as exc:
            stats.rejected += 1
            log.warning("rejected batch %s: %s", batch.batch_id, exc)

    if writer is not None:
        writer.close()
    else:
        sink.flush()
    stats.seconds = time.monotonic() - started
    return stats
