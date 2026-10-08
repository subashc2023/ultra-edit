"""Decode batch payloads into records that match a schema.

A payload is UTF-8 text with one JSON object per line. Agents older than 2.0
send one header line before the records, for example
``#sluice/1 host=web-3 count=120``. The decoder keeps the header's fields
apart and never treats the header as a record.
"""

import json
from dataclasses import dataclass

from . import flags

# Bytes read from a payload at a time. When streaming, the decoder holds at most
# one chunk plus one unfinished line in memory, however large the batch is.
CHUNK_SIZE = 64 * 1024

LEGACY_HEADER_PREFIX = "#sluice/1 "


class DecodeError(ValueError):
    """A line of the payload is not a valid record.

    ``line`` is the 1-based line number within the payload, counting the agent
    header if there is one.
    """

    def __init__(self, line, reason):
        super().__init__(f"line {line}: {reason}")
        self.line = line
        self.reason = reason


@dataclass(frozen=True)
class Schema:
    """The fields a record must have and the extra fields it may have."""

    required: tuple[str, ...]
    optional: tuple[str, ...] = ()

    @property
    def fields(self):
        return frozenset(self.required) | frozenset(self.optional)


def _decode_legacy_header(line):
    """Parse the header line that agents older than 2.0 send before the records.

    Returns the header's ``key=value`` pairs as a dict, or None when line is
    not a header.
    """
    if not line.startswith(LEGACY_HEADER_PREFIX):
        return None
    header = {}
    for item in line[len(LEGACY_HEADER_PREFIX):].split():
        key, _, value = item.partition("=")
        header[key] = value
    return header


def _parse_record(line, number, schema):
    """Parse one line of JSON and check it against schema."""
    try:
        record = json.loads(line)
    except json.JSONDecodeError as exc:
        raise DecodeError(number, f"invalid JSON ({exc.msg})") from None
    if not isinstance(record, dict):
        raise DecodeError(number, "not a JSON object")
    missing = [name for name in schema.required if name not in record]
    if missing:
        raise DecodeError(number, "missing " + ", ".join(missing))
    if flags.REJECT_UNKNOWN_FIELDS:
        unknown = sorted(set(record) - schema.fields)
        if unknown:
            raise DecodeError(number, "unknown field(s) " + ", ".join(unknown))
    return record


class StreamingDecoder:
    """Decode a payload incrementally, yielding each record once its line ends.

    Memory use is bounded by the chunk size and the longest line rather than by
    the size of the payload. UTF-8 never uses the newline byte inside a
    multi-byte character, so each complete line can be decoded on its own.
    """

    def __init__(self, schema, chunk_size=CHUNK_SIZE):
        self.schema = schema
        self.chunk_size = chunk_size
        self.header = None

    def records(self, payload):
        """Yield the records of payload, a binary file-like object, in order."""
        for number, raw in enumerate(self._lines(payload), start=1):
            try:
                line = raw.decode("utf-8")
            except UnicodeDecodeError:
                raise DecodeError(number, "not valid UTF-8") from None
            if number == 1:
                self.header = _decode_legacy_header(line)
                if self.header is not None:
                    continue
            if line.strip():
                yield _parse_record(line, number, self.schema)

    def _lines(self, payload):
        pending = b""
        while True:
            chunk = payload.read(self.chunk_size)
            if not chunk:
                break
            *complete, pending = (pending + chunk).split(b"\n")
            yield from complete
        if pending:
            yield pending


def decode_batch(payload, schema):
    """Decode a batch payload, a binary file-like object, into records.

    Records come back as an iterable of dicts in payload order. A line that is
    not valid JSON, or does not match schema, raises DecodeError naming the line.
    """
    return StreamingDecoder(schema).records(payload)
