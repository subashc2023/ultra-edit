# Python API

The `taskclock` command covers most uses, but the modules below are stable
and can be imported by scripts that generate or check job files. Anything
not listed here, including names that start with an underscore, may change
between releases.

## taskclock.utils

General helpers with no knowledge of jobs or schedules.

<a id="taskclock.utils.slugify"></a>
### `slugify(text, separator="-")`

Return a lowercase ASCII identifier for `text`, made of its letters and
digits with one `separator` between words, at most 48 characters long. Job
slugs name state files and are what `--only` matches.

```python
>>> from taskclock.utils import slugify
>>> slugify("Nightly Backup (EU)")
'nightly-backup-eu'
```

<a id="taskclock.utils.parse_duration"></a>
### `parse_duration(text)`

Parse a duration such as `90s`, `15m`, or `1h30m` and return a
`datetime.timedelta`. The units are `d`, `h`, `m`, and `s`, written from the
largest to the smallest with nothing between them. Raises `ValueError` for
anything else, including an empty string and `1h 30m`.

```python
>>> from taskclock.utils import parse_duration
>>> parse_duration("1h30m")
datetime.timedelta(seconds=5400)
```

<a id="taskclock.utils.chunked"></a>
### `chunked(items, size)`

Yield lists of at most `size` items from `items`, in order. Only the last list
may be shorter. Raises `ValueError` if `size` is less than 1.

<a id="taskclock.utils.jittered"></a>
### `jittered(delay, fraction=0.1, rng=random)`

Return the timedelta `delay` moved earlier or later by a random amount of up
to `fraction` of itself, never below zero. Pass a seeded `random.Random` as
`rng` for repeatable results.

## taskclock.timeparse

Parsing and formatting of the dates, times, and durations in job files.

<a id="taskclock.timeparse.parse_date"></a>
### `parse_date(text)`

Parse an ISO date such as `2024-03-01` into a `datetime.date`.

<a id="taskclock.timeparse.parse_time_of_day"></a>
### `parse_time_of_day(text)`

Parse a 24-hour time such as `07:30` and return its offset from midnight as a
`datetime.timedelta`.

<a id="taskclock.timeparse.parse_weekdays"></a>
### `parse_weekdays(text)`

Parse a comma-separated list of three-letter weekday names, such as
`mon,wed,fri`, into a sorted list of weekday numbers with Monday as 0.

<a id="taskclock.timeparse.format_duration"></a>
### `format_duration(delta)`

Format a `datetime.timedelta` the way [`parse_duration`](#taskclock.utils.parse_duration)
reads it, leaving out zero units. A zero duration is `0s`.

```python
>>> from datetime import timedelta
>>> from taskclock.timeparse import format_duration
>>> format_duration(timedelta(minutes=90))
'1h30m'
```

## taskclock.runner

<a id="taskclock.runner.parse_duration_ms"></a>
### `parse_duration_ms(text)`

Like [`parse_duration`](#taskclock.utils.parse_duration), but returns whole
milliseconds as an `int` and also accepts the `ms` suffix, as in `500ms`.
`--hook-timeout` is read with this function.

<a id="taskclock.runner.Runner"></a>
### `Runner(jobs, hook_timeout_ms=2000, grace="30s")`

Runs `jobs` until interrupted. `run_forever()` starts each job when it is due
and gives it its interval plus `grace` to finish; `run_job(job)` runs one job
once, calls its hooks, and returns its exit status.
