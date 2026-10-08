"""Command-line entry point for logship."""

import argparse
import json
import sys

from logship.config import ShipperConfig
from logship.transport import ship


def build_parser():
    parser = argparse.ArgumentParser(
        prog="logship",
        description="Ship the lines of local log files to a central collector.",
    )
    parser.add_argument("paths", nargs="+", metavar="PATH", help="log files to read")
    parser.add_argument(
        "--endpoint",
        required=True,
        metavar="URL",
        help="collector to send to, for example https://logs.example.com:6514",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=500,
        metavar="N",
        help="lines sent per request (default: 500)",
    )
    parser.add_argument(
        "--ca-file",
        metavar="PEM",
        help="trust only the CA certificates in this bundle",
    )
    parser.add_argument(
        "--client-cert",
        metavar="PEM",
        help="present this certificate and key to the collector",
    )
    parser.add_argument(
        "--tls-server-name",
        metavar="NAME",
        help="expect this name in the collector's certificate",
    )
    parser.add_argument(
        "--legacy-output",
        action="store_true",
        help="print the 1.x one-line summary instead of JSON",
    )
    return parser


def config_from_args(args):
    return ShipperConfig(
        endpoint=args.endpoint,
        batch_size=args.batch_size,
        ca_file=args.ca_file,
        client_cert=args.client_cert,
        tls_server_name=args.tls_server_name,
        legacy_output=args.legacy_output,
    )


def format_summary(result, legacy_output=False):
    if legacy_output:
        return f"shipped={result.shipped} failed={result.failed}"
    return json.dumps({"shipped": result.shipped, "failed": result.failed})


def main(argv=None, ship_fn=ship):
    args = build_parser().parse_args(argv)
    config = config_from_args(args)

    result = ship_fn(args.paths, config)
    print(format_summary(result, config.legacy_output))
    return 0 if result.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
