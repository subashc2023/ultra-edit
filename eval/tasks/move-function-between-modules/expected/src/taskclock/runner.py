"""Run jobs when they are due and call their hooks after each run."""

import logging
import random
import subprocess
import time
from datetime import datetime

from .timeparse import parse_duration

log = logging.getLogger("taskclock.runner")

DEFAULT_GRACE = "30s"


def parse_duration_ms(text):
    """Parse a hook timeout such as "500ms" or "2s" into whole milliseconds.

    Hook timeouts are often shorter than a second, so besides everything
    parse_duration accepts they may use the ms suffix.
    """
    spec = text.strip().lower()
    if spec.endswith("ms") and spec[:-2].isdigit():
        return int(spec[:-2])
    return int(parse_duration(spec).total_seconds() * 1000)


class Runner:
    """Runs a list of jobs, each on its own interval, until interrupted.

    A job may run for its whole interval plus a grace period before it is
    stopped; a hook gets hook_timeout_ms milliseconds.
    """

    def __init__(
        self,
        jobs,
        hook_timeout_ms=2000,
        grace=DEFAULT_GRACE,
        run=subprocess.run,
        clock=time.monotonic,
    ):
        self.jobs = list(jobs)
        self.hook_timeout_ms = hook_timeout_ms
        self.grace = parse_duration(grace)
        self.run = run
        self.clock = clock
        self.rng = random.Random()

    def run_job(self, job):
        """Run one job once, then its hooks; return the job's exit status.

        The status is None when the job was stopped for running too long.
        """
        timeout = (job.interval + self.grace).total_seconds()
        started = self.clock()
        try:
            status = self.run(list(job.command), timeout=timeout).returncode
        except subprocess.TimeoutExpired:
            status = None
        duration = self.clock() - started
        log.info("job %s finished with status %s in %.1fs", job.name, status, duration)
        for hook in job.hooks:
            self.call_hook(hook, job, status)
        return status

    def call_hook(self, hook, job, status):
        """Run one hook with the job's name and status as extra arguments."""
        try:
            self.run([*hook, job.name, str(status)], timeout=self.hook_timeout_ms / 1000)
        except subprocess.TimeoutExpired:
            log.warning(
                "hook %s for job %s timed out after %d ms", hook[0], job.name, self.hook_timeout_ms
            )

    def run_forever(self, sleep=time.sleep):
        """Run every job whenever it is due. Only an interrupt stops this."""
        if not self.jobs:
            raise ValueError("no jobs to run")
        now = datetime.now()
        due = {job.slug: job.next_run(now, self.rng) for job in self.jobs}
        while True:
            job = min(self.jobs, key=lambda item: due[item.slug])
            wait = (due[job.slug] - datetime.now()).total_seconds()
            if wait > 0:
                sleep(wait)
            self.run_job(job)
            due[job.slug] = job.next_run(datetime.now(), self.rng)
