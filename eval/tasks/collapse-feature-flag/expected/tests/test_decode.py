import io
import json
from unittest import mock

import pytest

from sluice import flags
from sluice.decode import (
    DecodeError,
    Schema,
    StreamingDecoder,
    _decode_legacy_header,
    decode_batch,
)

SCHEMA = Schema(required=("ts", "host"), optional=("msg", "level"))


def payload(*lines):
    return io.BytesIO("".join(line + "\n" for line in lines).encode("utf-8"))


def record(**fields):
    return json.dumps({"ts": 1700000000, "host": "web-1", **fields})


def test_streaming_decoder_yields_records_in_order():
    decoder = StreamingDecoder(SCHEMA, chunk_size=7)
    records = decoder.records(payload(record(msg="first"), record(msg="second")))
    assert [r["msg"] for r in records] == ["first", "second"]


def test_streaming_decoder_reads_a_last_line_without_newline():
    decoder = StreamingDecoder(SCHEMA)
    data = io.BytesIO((record(msg="a") + "\n" + record(msg="b")).encode("utf-8"))
    assert [r["msg"] for r in decoder.records(data)] == ["a", "b"]


def test_streaming_decoder_keeps_the_agent_header():
    decoder = StreamingDecoder(SCHEMA)
    records = list(decoder.records(payload("#sluice/1 host=web-3 count=1", record())))
    assert decoder.header == {"host": "web-3", "count": "1"}
    assert len(records) == 1


def test_decode_legacy_header_reads_key_value_pairs():
    header = _decode_legacy_header("#sluice/1 host=web-3 count=120")
    assert header == {"host": "web-3", "count": "120"}


def test_decode_legacy_header_ignores_records():
    assert _decode_legacy_header(record()) is None


def test_decode_batch_yields_records_before_a_bad_line():
    records = decode_batch(payload(record(msg="ok"), "{not json}"), SCHEMA)
    assert next(records)["msg"] == "ok"
    with pytest.raises(DecodeError, match="line 2"):
        next(records)


def test_missing_field_names_the_line():
    with pytest.raises(DecodeError, match="line 3: missing host"):
        list(decode_batch(payload(record(), record(), '{"ts": 1}'), SCHEMA))


def test_invalid_utf8_is_a_decode_error():
    with pytest.raises(DecodeError, match="not valid UTF-8"):
        list(decode_batch(io.BytesIO(b'{"ts": 1, "host": "\xff"}\n'), SCHEMA))


def test_unknown_fields_are_kept_by_default():
    records = list(decode_batch(payload(record(color="red")), SCHEMA))
    assert records[0]["color"] == "red"


def test_unknown_fields_are_rejected_when_the_flag_is_on():
    with mock.patch.object(flags, "REJECT_UNKNOWN_FIELDS", True):
        records = decode_batch(payload(record(color="red")), SCHEMA)
        with pytest.raises(DecodeError, match="unknown field"):
            list(records)
