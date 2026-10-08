"""Sanity checks for config/services.toml."""

import sys
import tomllib
from pathlib import Path

KNOWN_REGIONS = {"us-east-1", "us-west-2", "eu-west-1", "ap-southeast-2"}


def main() -> int:
    path = Path(__file__).resolve().parents[1] / "config" / "services.toml"
    services = tomllib.loads(path.read_text(encoding="utf-8"))["services"]
    problems = []
    for name, table in services.items():
        if not table.get("owner", "").startswith("team-"):
            problems.append(f"{name}: owner must start with team-")
        if table.get("region", "us-east-1") not in KNOWN_REGIONS:
            problems.append(f"{name}: unknown region {table['region']}")
    for problem in problems:
        print(problem)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
