# taskclock

taskclock runs commands on fixed intervals. It is a small stand-in for cron
that reads its jobs from one JSON file and keeps each job's interval exact
instead of tying it to wall-clock minutes, which suits containers and
development machines where a full cron daemon is more than you need.

## Job files

```json
{
  "jobs": [
    {"name": "Nightly backup", "command": ["backup", "--all"], "every": "1d"},
    {"name": "Sync mirrors", "command": ["sync-mirrors"], "every": "15m", "jitter": 0.2,
     "hooks": [["notify", "--channel", "ops"]]}
  ]
}
```

Every job needs a `name`, a `command` (a list of arguments, not a shell
string), and an `every` interval. The interval is a duration made of whole
numbers and the units `d`, `h`, `m`, and `s`, largest first: `90s`, `15m`,
`1h30m`, `2d`. taskclock checks that it can parse a duration for every job
before it starts any of them, so a typo stops the whole file from loading
rather than leaving one job that silently never runs.

`jitter` (default 0.1) moves each run earlier or later by up to that fraction
of the interval, so jobs that share an interval do not all start at once.

Hooks are commands run after the job finishes, with the job name and its exit
status appended as two extra arguments. Each hook gets two seconds by default;
change that with `--hook-timeout`, which also accepts milliseconds (`500ms`).

## Commands

    taskclock check jobs.json      # load the file and report problems
    taskclock next jobs.json       # show when each job runs next
    taskclock run jobs.json        # run the jobs until interrupted

`--only NAME` limits any command to one job, matched by its slug, and
`--every DURATION` overrides the interval of every selected job, which is
handy for trying out a job file: `taskclock run jobs.json --every 10s`.

See [docs/api.md](docs/api.md) for the Python API.
