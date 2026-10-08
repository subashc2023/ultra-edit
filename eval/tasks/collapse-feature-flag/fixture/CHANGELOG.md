# Changelog

## 0.10.0 - 2026-08-19

- Added `USE_ASYNC_WRITER` (`SLUICE_ASYNC_WRITER`), off by default, to write
  records to the sink from a background thread.
- Decode errors now name the line number within the batch.

## 0.9.0 - 2026-06-02

- `USE_STREAMING_DECODER` is now on by default. Set
  `SLUICE_STREAMING_DECODER=0` to go back to the in-memory decoder.
- Records are written to the sink in slices of 500.

## 0.8.0 - 2026-04-14

- Added `USE_STREAMING_DECODER` (`SLUICE_STREAMING_DECODER`), off by default,
  to decode payloads line by line instead of reading them into memory first.
- Added `REJECT_UNKNOWN_FIELDS` (`SLUICE_REJECT_UNKNOWN_FIELDS`), off by
  default, to reject records with fields the schema does not list.
