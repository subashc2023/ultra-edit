# sluice

sluice is the ingest service behind the metrics warehouse. Agents send batches
of newline-delimited JSON records; sluice decodes each batch as a stream,
checks every record against the schema, and writes the records to the
warehouse sink in slices.

## Install

    pip install -e .

## Payload format

One JSON object per line, encoded as UTF-8. Agents older than 2.0 send a single
header line such as `#sluice/1 host=web-3 count=120` before the records; sluice
reads the header's fields and does not store the header as a record.

A line that is not valid JSON, or that lacks a required field, stops its batch,
and the batch is counted as rejected. The error names the line number within
the batch, counting the header.

## Feature flags

Behavior that is still being rolled out sits behind a feature flag. The flags,
their environment variables, and their defaults are listed in
[docs/feature-flags.md](docs/feature-flags.md).

## Tests

    pytest

Run the tests with no `SLUICE_*` variables set, so that every flag has its
default value.
