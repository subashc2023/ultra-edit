"""Thin wrappers around local files and the object store."""

from __future__ import annotations

from pathlib import Path
from typing import IO

BUCKET_URL = "https://objects.internal.example/worker-exports"


def open_output(path: str | Path) -> IO[str]:
    return Path(path).open("r", encoding="utf-8", newline="")


def upload(path: Path, *, content_type: str) -> str:
    # The real implementation streams to the object store; tests patch this.
    return f"{BUCKET_URL}/{path.name}?type={content_type}"
