"""Command-line entry point: taskclock {check,next,run} JOBFILE [options]."""

import argparse
import logging
import sys
from datetime import datetime

from . import timeparse, utils
from .runner import Runner, parse_duration_ms
from .schedule import load_jobs

log = logging.getLogger("taskclock.cli")


def build_parser():
    parser = argparse.ArgumentParser(prog="taskclock")
    parser.add_argument("command", choices=("check", "next", "run"))
    parser.add_argument("jobfile", help="JSON file that defines the jobs")
    parser.add_argument("--only", metavar="NAME", help="act on this job only, matched by its slug")
    parser.add_argument("--every", metavar="DURATION", help="override every job's interval, e.g. 10s")
    parser.add_argument("--start", metavar="DATE", help="for next: count from midnight on DATE")
    parser.add_argument(
        "--hook-timeout",
        default="2s",
        metavar="DURATION",
        help="how long each hook may run, e.g. 500ms or 2s",
    )
    return parser


def select_jobs(jobs, only):
    """Return the jobs to act on: all of them, or the one whose slug matches."""
    if only is None:
        return jobs
    wanted = utils.slugify(only)
    selected = [job for job in jobs if job.slug == wanted]
    if not selected:
        raise ValueError(f"no job is named {only!r}")
    return selected


def show_next(jobs, start):
    for job in jobs:
        every = timeparse.format_duration(job.interval)
        print(f"{job.name}: every {every}, next at {job.next_run(start):%Y-%m-%d %H:%M:%S}")


def main(argv=None):
    args = build_parser().parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s: %(message)s")
    try:
        jobs = select_jobs(load_jobs(args.jobfile), args.only)
    except (OSError, ValueError) as exc:
        print(f"taskclock: cannot load {args.jobfile}: {exc}", file=sys.stderr)
        return 2
    if args.every is not None:
        try:
            interval = timeparse.parse_duration(args.every)
        except ValueError as exc:
            print(f"taskclock: cannot parse a duration from --every: {exc}", file=sys.stderr)
            return 2
        jobs = [job.with_interval(interval) for job in jobs]
    if args.command == "check":
        print(f"taskclock: {len(jobs)} job(s) in {args.jobfile} are valid")
        return 0
    if args.command == "next":
        start = datetime.now()
        if args.start is not None:
            start = datetime.combine(timeparse.parse_date(args.start), datetime.min.time())
        show_next(jobs, start)
        return 0
    runner = Runner(jobs, hook_timeout_ms=parse_duration_ms(args.hook_timeout))
    try:
        runner.run_forever()
    except KeyboardInterrupt:
        log.info("interrupted; stopping")
    return 0


if __name__ == "__main__":
    sys.exit(main())
