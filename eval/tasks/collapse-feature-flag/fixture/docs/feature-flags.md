# Feature flags

A feature flag switches between an established code path and its replacement
while the replacement is rolled out. The flags live in `src/sluice/flags.py`.
Each one is read from an environment variable when the service starts, so an
operator can turn a new code path off again without a release.

## Current flags

| Flag                    | Environment variable           | Default | Effect when on                                       |
| ----------------------- | ------------------------------ | ------- | ---------------------------------------------------- |
| `REJECT_UNKNOWN_FIELDS` | `SLUICE_REJECT_UNKNOWN_FIELDS` | off     | Reject records with fields the schema does not list. |
| `USE_STREAMING_DECODER` | `SLUICE_STREAMING_DECODER`     | on      | Decode each payload line by line as it arrives.      |
| `USE_ASYNC_WRITER`      | `SLUICE_ASYNC_WRITER`          | off     | Write records to the sink from a background thread.  |

The values `1`, `true`, `yes`, and `on` turn a flag on, in any letter case. Any
other non-empty value turns it off, and an unset or empty variable keeps the
default.

## Reading a flag in code

Read a flag through the module, as in `flags.USE_ASYNC_WRITER`, and never with
`from sluice.flags import ...`. Tests switch a flag with
`mock.patch.object(flags, "NAME", value)`, which only affects code that looks
the attribute up each time it runs.

## Retiring a flag

Once a flag has been on in every deployment for a full release, retire it: keep
the code path it turns on, delete the path it replaced together with the tests
that cover only that path, and remove its row from the table above. Leave the
changelog entries that mention it.
