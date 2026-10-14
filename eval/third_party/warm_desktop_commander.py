#!/usr/bin/env python3
"""Start Desktop Commander once so it downloads Chrome into its cache.

    python3 eval/third_party/warm_desktop_commander.py INSTALL_DIR

Desktop Commander checks for Chrome after the MCP handshake and downloads it in
the background when none is found. This script starts the server as the
desktop-commander arm in arms.json does, with its environment, performs the
handshake, then waits until the download finishes or ten minutes pass.
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run_eval import DEFAULT_THIRD_PARTY_ARMS, load_arm_registry, mcp_config  # noqa: E402

TIMEOUT_S = 600
ARM = "desktop-commander"


def main() -> int:
    install = Path(sys.argv[1]).resolve()
    arm = load_arm_registry(DEFAULT_THIRD_PARTY_ARMS).arms[ARM]
    server = mcp_config(arm, install, install)["mcpServers"][arm.server_name]
    command = shutil.which(server["command"]) or server["command"]
    process = subprocess.Popen(
        [command, *server.get("args", [])],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env={**os.environ, **server.get("env", {})},
        cwd=str(install),
    )
    assert process.stdin is not None and process.stderr is not None
    # A thread reads stderr, since a pipe cannot be made non-blocking on every platform.
    chunks: queue.Queue[bytes] = queue.Queue()
    stderr = process.stderr

    def pump() -> None:
        for line in iter(stderr.readline, b""):
            chunks.put(line)

    threading.Thread(target=pump, daemon=True).start()
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
    while time.monotonic() < deadline:
        try:
            log += chunks.get(timeout=1)
        except queue.Empty:
            pass
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
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
    print(f"Desktop Commander Chrome: {status}")
    return 0 if status in ("downloaded", "already available") else 1


if __name__ == "__main__":
    sys.exit(main())
