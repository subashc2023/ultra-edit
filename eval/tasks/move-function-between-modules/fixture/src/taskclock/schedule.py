"""Job definitions and the arithmetic that decides when each job runs next.

A job file is a JSON object with a "jobs" list. Each entry names the job,
gives its command as a list of arguments, and says how often it runs.
"""

import json
from dataclasses import dataclass, replace
from datetime import timedelta

from .utils import jittered, parse_duration, slugify

DEFAULT_JITTER = 0.1


@dataclass(frozen=True)
class Job:
    """One scheduled command, as read from a job file."""

    name: str
    command: tuple
    interval: timedelta
    jitter: float = DEFAULT_JITTER
    hooks: tuple = ()

    @property
    def slug(self):
        """The job's name as an identifier, used in state files and by --only."""
        return slugify(self.name)

    def with_interval(self, interval):
        """Return a copy of the job that runs every interval instead."""
        return replace(self, interval=interval)

    def next_run(self, last_run, rng=None):
        """Return when the job should run next, given when it last ran.

        Without rng the interval is used exactly, which keeps tests and the
        "next" command repeatable; the runner passes its own generator.
        """
        if rng is None:
            return last_run + self.interval
        return last_run + jittered(self.interval, self.jitter, rng)


def job_from_dict(spec):
    """Build a Job from one entry of a job file, checking every field."""
    name = spec.get("name")
    if not isinstance(name, str) or not name.strip():
        raise ValueError(f"job without a name: {spec!r}")
    command = spec.get("command")
    if not isinstance(command, list) or not command:
        raise ValueError(f"job {name!r}: command must be a non-empty list of arguments")
    if "every" not in spec:
        raise ValueError(f"job {name!r}: missing the every interval")
    try:
        interval = parse_duration(str(spec["every"]))
    except ValueError as exc:
        raise ValueError(f"job {name!r}: {exc}") from None
    if interval <= timedelta(0):
        raise ValueError(f"job {name!r}: the interval must be longer than zero")
    jitter = float(spec.get("jitter", DEFAULT_JITTER))
    hooks = tuple(tuple(hook) for hook in spec.get("hooks", ()))
    return Job(name, tuple(command), interval, jitter, hooks)


def load_jobs(path):
    """Read a job file and return its jobs, refusing duplicate names."""
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
    jobs = [job_from_dict(spec) for spec in data.get("jobs", [])]
    seen = set()
    for job in jobs:
        if job.slug in seen:
            raise ValueError(f"two jobs share the name {job.name!r} once slugified")
        seen.add(job.slug)
    return jobs
