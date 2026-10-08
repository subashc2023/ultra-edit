"""Command-line entry point: ``python -m pkg.cli product SKU``."""

from __future__ import annotations

import argparse
import json
import sys

from pkg.client import CatalogClient
from pkg.net import fetch_json


def build_parser():
    parser = argparse.ArgumentParser(prog="tidepool")
    parser.add_argument("--base-url", default="https://api.tidepool.example/v2")
    parser.add_argument(
        "--retries",
        type=int,
        default=3,
        help="attempts per request (default: 3)",
    )
    parser.add_argument("command", choices=["product", "raw"])
    parser.add_argument("target")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    if args.command == "raw":
        data = fetch_json(args.target, retries=args.retries)
    else:
        client = CatalogClient(args.base_url, retries=args.retries)
        data = client.product(args.target)
    json.dump(data, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
