"""Command-line entry point: wayfetch [options] URL.

Settings come from three places, in order of precedence: flags, the TOML config
file, and the built-in defaults in wayfetch.defaults. The flags therefore
default to None here, so that a flag that was not given does not hide a value
from the config file.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path

from wayfetch.client import Client, FetchError
from wayfetch.defaults import (
    CONFIG_PATH,
    DEFAULT_BACKOFF_MAX,
    DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_MAX_CONNECTIONS,
    DEFAULT_MAX_RETRIES,
    DEFAULT_READ_TIMEOUT,
)

# Where each setting lives in the config file: (table, key) -> setting name.
FILE_KEYS = {
    ("http", "connect_timeout"): "connect_timeout",
    ("http", "read_timeout"): "read_timeout",
    ("retry", "max_retries"): "max_retries",
    ("retry", "backoff_max"): "backoff_max",
    ("pool", "max_connections"): "max_connections",
    ("proxy", "url"): "proxy",
}

EPILOG = """\
examples:
  wayfetch https://api.example.com/v1/status
  wayfetch -o report.csv --read-timeout 60 https://reports.example.com/daily.csv
  wayfetch --proxy http://127.0.0.1:10080 https://api.example.com/v1/status
"""


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="wayfetch",
        description="Fetch a URL and write the response body to stdout or a file.",
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("url", help="absolute http:// or https:// URL to fetch")
    parser.add_argument("-o", "--output", metavar="FILE", help="write the body to FILE instead of stdout")
    parser.add_argument(
        "--config",
        metavar="FILE",
        help=f"read settings from FILE (default: {CONFIG_PATH})",
    )
    parser.add_argument(
        "--connect-timeout",
        type=float,
        metavar="SECONDS",
        help="give up on a connection attempt after SECONDS (default: 10)",
    )
    parser.add_argument(
        "--read-timeout",
        type=float,
        metavar="SECONDS",
        help="give up when no data arrives for SECONDS (default: 10)",
    )
    parser.add_argument(
        "--retries",
        type=int,
        metavar="N",
        help="retry a failed request up to N times (default: 3)",
    )
    parser.add_argument(
        "--backoff-max",
        type=float,
        metavar="SECONDS",
        help="never pause longer than SECONDS between attempts (default: 10)",
    )
    parser.add_argument(
        "--max-connections",
        type=int,
        metavar="N",
        help="keep at most N connections open (default: 100)",
    )
    parser.add_argument("--proxy", metavar="URL", help="send every request through this proxy")
    return parser


def load_config(path: Path) -> dict:
    """Read a TOML config file and return its settings by name.

    Unknown tables and keys are an error, so a typo such as connect_timout does
    not silently fall back to the default.
    """
    with path.open("rb") as handle:
        data = tomllib.load(handle)
    settings = {}
    for table, values in data.items():
        if not isinstance(values, dict):
            raise ValueError(f"{path}: {table} must be a table")
        for key, value in values.items():
            if (table, key) not in FILE_KEYS:
                raise ValueError(f"{path}: unknown setting [{table}] {key}")
            settings[FILE_KEYS[table, key]] = value
    return settings


def resolve_settings(args: argparse.Namespace) -> dict:
    """Merge the flags over the config file over the built-in defaults."""
    settings = {
        "connect_timeout": DEFAULT_CONNECT_TIMEOUT,
        "read_timeout": DEFAULT_READ_TIMEOUT,
        "max_retries": DEFAULT_MAX_RETRIES,
        "backoff_max": DEFAULT_BACKOFF_MAX,
        "max_connections": DEFAULT_MAX_CONNECTIONS,
        "proxy": None,
    }
    path = Path(args.config or CONFIG_PATH).expanduser()
    # A missing file at the default path is fine; a missing --config file is not.
    if args.config or path.exists():
        settings.update(load_config(path))
    flags = {
        "connect_timeout": args.connect_timeout,
        "read_timeout": args.read_timeout,
        "max_retries": args.retries,
        "backoff_max": args.backoff_max,
        "max_connections": args.max_connections,
        "proxy": args.proxy,
    }
    settings.update({name: value for name, value in flags.items() if value is not None})
    return settings


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        settings = resolve_settings(args)
    except (OSError, ValueError) as exc:
        print(f"wayfetch: {exc}", file=sys.stderr)
        return 2
    try:
        with Client(**settings) as client:
            response = client.get(args.url)
    except FetchError as exc:
        print(f"wayfetch: {exc}", file=sys.stderr)
        return 1
    if args.output:
        Path(args.output).write_bytes(response.content)
    else:
        sys.stdout.buffer.write(response.content)
    return 0 if response.is_success else 1


if __name__ == "__main__":
    sys.exit(main())
