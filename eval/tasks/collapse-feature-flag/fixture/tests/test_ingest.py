import io
import json
from unittest import mock

from sluice import flags
from sluice.decode import Schema
from sluice.ingest import WRITE_SLICE, Batch, ingest

SCHEMA = Schema(required=("ts", "host"), optional=("msg",))


def make_payload(count, start=0):
    lines = (json.dumps({"ts": start + n, "host": "web-1"}) for n in range(count))
    return io.BytesIO("".join(line + "\n" for line in lines).encode("utf-8"))


class ListSource:
    def __init__(self, *payloads):
        self.payloads = payloads

    def batches(self):
        for number, payload in enumerate(self.payloads, start=1):
            yield Batch(f"batch-{number}", payload)


class ListSink:
    def __init__(self):
        self.writes = []
        self.flushes = 0

    def write(self, records):
        self.writes.append(list(records))

    def flush(self):
        self.flushes += 1

    @property
    def records(self):
        return [record for write in self.writes for record in write]


def test_ingest_counts_batches_and_records():
    sink = ListSink()
    stats = ingest(ListSource(make_payload(3), make_payload(2)), sink, SCHEMA)
    assert (stats.batches, stats.records, stats.rejected) == (2, 5, 0)
    assert [record["ts"] for record in sink.records] == [0, 1, 2, 0, 1]
    assert sink.flushes == 1


def test_bad_batch_is_rejected_and_the_rest_are_ingested():
    sink = ListSink()
    bad = io.BytesIO(b"{not json}\n")
    stats = ingest(ListSource(bad, make_payload(2)), sink, SCHEMA)
    assert (stats.batches, stats.records, stats.rejected) == (2, 2, 1)


def test_large_batch_is_written_in_slices():
    sink = ListSink()
    ingest(ListSource(make_payload(WRITE_SLICE * 2 + 1)), sink, SCHEMA)
    assert [len(write) for write in sink.writes] == [WRITE_SLICE, WRITE_SLICE, 1]


@mock.patch.object(flags, "USE_ASYNC_WRITER", True)
def test_async_writer_delivers_every_record_before_returning():
    sink = ListSink()
    stats = ingest(ListSource(make_payload(4), make_payload(4, start=4)), sink, SCHEMA)
    assert stats.records == 8
    assert [record["ts"] for record in sink.records] == list(range(8))
    assert sink.flushes == 1


def test_inline_writer_flushes_once():
    with mock.patch.object(flags, "USE_ASYNC_WRITER", False):
        sink = ListSink()
        ingest(ListSource(make_payload(1), make_payload(1)), sink, SCHEMA)
        assert sink.flushes == 1
