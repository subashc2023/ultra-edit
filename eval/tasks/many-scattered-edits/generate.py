#!/usr/bin/env python3
"""Regenerate this task's `config/services.toml` fixture and expected bytes.

    python eval/tasks/many-scattered-edits/generate.py

The committed files are the source of truth for the evaluation; this generator
recreates `config/services.toml` in both `fixture/` and `expected/`
deterministically. The other files in the task are small and committed as is.
"""

from __future__ import annotations

from pathlib import Path

TASK_DIR = Path(__file__).resolve().parent
RELATIVE_PATH = "config/services.toml"

TEAMS = (
    "auth",
    "billing",
    "catalog",
    "checkout",
    "inventory",
    "ledger",
    "notify",
    "orders",
    "payments",
    "pricing",
    "profile",
    "reports",
    "search",
    "shipping",
    "tax",
)
ROLES = (
    "api",
    "admin",
    "cache",
    "export",
    "gateway",
    "indexer",
    "scheduler",
    "stream",
    "sync",
    "webhooks",
    "worker",
)

HEADER = (
    "# Service registry for the shop platform.\n"
    "#\n"
    "# One [services.<name>] table per deployable service. Keys that are left out\n"
    "# fall back to config/defaults.toml. Keep tables grouped by owning team.\n"
    "\n"
    "schema_version = 3\n"
)

# Edits applied to the expected file, keyed by service name. Each value maps a
# key to its new raw TOML value, or holds the special entries "+after" (insert a
# new line after a key) and "-" (delete a key's line).
OVERRIDES: dict[str, dict[str, object]] = {
    "auth-gateway": {"port": "8081"},
    "billing-scheduler": {"timeout_ms": "7500"},
    "billing-stream": {"enabled": "false"},
    "catalog-indexer": {"memory": '"1Gi"'},
    "checkout-api": {"tags": '["public", "http", "pci"]'},
    "checkout-admin": {"replicas": "1"},
    "inventory-sync": {"+after": ("timeout_ms", "connect_timeout_ms", "750")},
    "ledger-export": {"region": '"us-west-2"'},
    "orders-worker": {"timeout_ms": "12000"},
    "payments-api": {"health_check": '"/readyz"'},
    "pricing-api": {"enabled": "true"},
    "pricing-cache": {"-": "log_level"},
    "profile-gateway": {"cpu": '"2000m"'},
    "reports-export": {"log_level": '"debug"'},
    "search-indexer": {"owner": '"team-discovery"'},
    "shipping-worker": {"port": "9190"},
    "tax-api": {"retries": "4"},
}


def services() -> list[tuple[str, str, str]]:
    return [(f"{team}-{role}", team, role) for team in TEAMS for role in ROLES]


def table_rows(index: int, team: str, role: str) -> list[tuple[str, str]]:
    mix = (index * 37 + 11) % 97
    if role in ("api", "admin", "gateway", "webhooks"):
        port, protocol, health = 8080, "http", "/healthz"
    elif role == "cache":
        port, protocol, health = 6379, "tcp", "/livez"
    elif role == "stream":
        port, protocol, health = 9092, "tcp", "/livez"
    else:
        port, protocol, health = 9090, "grpc", "/healthz"
    if role in ("api", "gateway", "webhooks"):
        tags = '["public", "http"]'
    elif role == "admin":
        tags = '["internal", "http"]'
    else:
        tags = '["internal", "batch"]'
    return [
        ("description", f'"{team.capitalize()} {role} service"'),
        ("port", str(port)),
        ("protocol", f'"{protocol}"'),
        ("replicas", str((2, 3, 2, 4, 2, 3)[mix % 6])),
        ("enabled", "false" if mix % 9 == 4 else "true"),
        ("timeout_ms", str((3000, 5000, 3000, 10000, 5000)[mix % 5])),
        ("retries", str((2, 3, 2, 1)[mix % 4])),
        ("region", '"{}"'.format(("us-east-1", "eu-west-1", "us-east-1", "ap-southeast-2")[mix % 4])),
        ("owner", f'"team-{team}"'),
        ("log_level", '"{}"'.format(("info", "info", "warn", "info")[(mix // 4) % 4])),
        ("health_check", f'"{health}"'),
        ("cpu", '"{}"'.format(("250m", "500m", "500m", "1000m")[(mix // 3) % 4])),
        ("memory", '"{}"'.format(("256Mi", "512Mi", "512Mi", "1Gi")[(mix // 2) % 4])),
        ("tags", tags),
    ]


def table(index: int, name: str, team: str, role: str, edits: dict[str, object]) -> str:
    lines = [f"\n[services.{name}]\n"]
    for key, value in table_rows(index, team, role):
        if edits.get("-") == key:
            continue
        lines.append(f"{key} = {edits.get(key, value)}\n")
        after = edits.get("+after")
        if isinstance(after, tuple) and after[0] == key:
            lines.append(f"{after[1]} = {after[2]}\n")
    return "".join(lines)


def render(overrides: dict[str, dict[str, object]]) -> bytes:
    blocks = (
        table(index, name, team, role, overrides.get(name, {}))
        for index, (name, team, role) in enumerate(services())
    )
    return (HEADER + "".join(blocks)).encode("utf-8")


def fixture_bytes() -> bytes:
    return render({})


def expected_bytes() -> bytes:
    return render(OVERRIDES)


def main() -> None:
    for kind, data in (("fixture", fixture_bytes()), ("expected", expected_bytes())):
        path = TASK_DIR / kind / RELATIVE_PATH
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        lines = data.count(b"\n")
        print(f"wrote {path} ({lines} lines)")


if __name__ == "__main__":
    main()
