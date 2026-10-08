"""Runtime settings, read once from the environment."""

from __future__ import annotations

import dataclasses
import os
from pathlib import Path


@dataclasses.dataclass(frozen=True)
class Settings:
    export_dir: Path
    snapshot_dir: Path
    export_ttl_days: int


def load_settings() -> Settings:
    root = Path(os.environ.get("WORKER_DATA", "/var/lib/worker"))
    return Settings(
        export_dir=root / "exports",
        snapshot_dir=root / "snapshots",
        export_ttl_days=int(os.environ.get("WORKER_EXPORT_TTL_DAYS", "14")),
    )


settings = load_settings()
