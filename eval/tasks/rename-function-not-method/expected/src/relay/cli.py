"""Command-line entry point: relay [--config PATH] [--plugin NAME] [--check]."""

import argparse
import logging
import sys

from relay import config, plugins
from relay.service import RelayService

log = logging.getLogger("relay.cli")


def build_parser():
    parser = argparse.ArgumentParser(prog="relay")
    parser.add_argument(
        "--config",
        default=str(config.DEFAULT_PATH),
        help="settings file to load (JSON or YAML)",
    )
    parser.add_argument(
        "--plugin",
        action="append",
        default=[],
        help="extra plugin to load; may be repeated",
    )
    parser.add_argument("--check", action="store_true", help="validate the settings and exit")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        settings = config.load_config(args.config)
    except (OSError, ValueError) as exc:
        print(f"relay: cannot read settings from {args.config}: {exc}", file=sys.stderr)
        return 2
    if args.check:
        print(f"relay: settings in {args.config} are valid")
        return 0
    for name in list(settings.plugins) + args.plugin:
        plugins.load(name).setup(settings)
        log.info("loaded plugin %s", name)
    RelayService(settings, args.config).run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
