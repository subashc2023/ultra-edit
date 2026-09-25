#!/usr/bin/env python3
"""Regenerate this task's fixture and expected bytes deterministically.

    python eval/tasks/large-file-two-regions/generate.py

The committed files are the source of truth for the evaluation; the unit tests
check that they still equal this generator's output.
"""

from __future__ import annotations

from pathlib import Path

TASK_DIR = Path(__file__).resolve().parent
RELATIVE_PATH = "src/routes.py"
FUNCTION_COUNT = 285
HEADER = '"""Route handlers for the inventory service."""\n\nfrom inventory.dispatch import dispatch\n'


def function_block(
    index: int, *, api: str = "v1", timeout_ms: int | None = None, retries: int | None = None
) -> str:
    timeout_ms = 1000 + (index % 5) * 500 if timeout_ms is None else timeout_ms
    retries = 1 + index % 3 if retries is None else retries
    return (
        "\n\n"
        f"def handle_route_{index:04d}(request):\n"
        f'    """Serve /api/{api}/items/{index}."""\n'
        f"    timeout_ms = {timeout_ms}\n"
        f"    retries = {retries}\n"
        f'    return dispatch(request, "/api/{api}/items/{index}", timeout_ms, retries)\n'
    )


def fixture_bytes() -> bytes:
    blocks = (function_block(index) for index in range(1, FUNCTION_COUNT + 1))
    return (HEADER + "".join(blocks)).encode("utf-8")


def expected_bytes() -> bytes:
    overrides = {
        21: {"timeout_ms": 2500},
        268: {"api": "v2", "retries": 4},
    }
    blocks = (function_block(index, **overrides.get(index, {})) for index in range(1, FUNCTION_COUNT + 1))
    return (HEADER + "".join(blocks)).encode("utf-8")


def main() -> None:
    for kind, data in (("fixture", fixture_bytes()), ("expected", expected_bytes())):
        path = TASK_DIR / kind / RELATIVE_PATH
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        lines = data.count(b"\n")
        print(f"wrote {path} ({lines} lines)")


if __name__ == "__main__":
    main()
