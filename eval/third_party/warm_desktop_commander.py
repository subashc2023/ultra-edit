#!/usr/bin/env python3
"""Start Desktop Commander once so it downloads Chrome into its cache.

    python3 eval/third_party/warm_desktop_commander.py INSTALL_DIR

Desktop Commander checks for Chrome after the MCP handshake and downloads it in
the background when none is found. This script performs the handshake with the
arm's environment, then waits until the download finishes or ten minutes pass.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

TIMEOUT_S = 600


def main() -> int:
    install = Path(sys.argv[1]).resolve()
    executable = install / "node_modules" / ".bin" / "desktop-commander"
    home = install / "dc-home"
    env = {**os.environ, "HOME": str(home), "DESKTOP_COMMANDER_DISABLE_TELEMETRY": "1"}
    process = subprocess.Popen(
        [str(executable)],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env=env,
        cwd=str(install),
    )
    assert process.stdin is not None and process.stderr is not None
    handshake = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "ultra-edit-eval-setup", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
    ]
    for message in handshake:
        process.stdin.write((json.dumps(message) + "\n").encode())
        process.stdin.flush()
    deadline = time.monotonic() + TIMEOUT_S
    status = "timed out"
    log = b""
    os.set_blocking(process.stderr.fileno(), False)
    while time.monotonic() < deadline:
        chunk = process.stderr.read() or b""
        log += chunk
        if b"Chrome download complete" in log:
            status = "downloaded"
            break
        if b"Failed to install Chrome" in log:
            status = "failed"
            break
        # No download message after the handshake settles: Chrome was already found.
        if b"Downloading Chrome" not in log and time.monotonic() > deadline - TIMEOUT_S + 20:
            status = "already available"
            break
        time.sleep(1)
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
    print(f"Desktop Commander Chrome: {status}")
    return 0 if status in ("downloaded", "already available") else 1


if __name__ == "__main__":
    sys.exit(main())
