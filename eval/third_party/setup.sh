#!/usr/bin/env bash
# Install the pinned third-party MCP edit servers used by the comparison arms in
# arms.json. Usage: eval/third_party/setup.sh [INSTALL_DIR]
# The default INSTALL_DIR is eval/third_party/install, which run_eval.py uses
# unless --third-party-dir says otherwise. Needs Node.js 18+, npm, and Python 3.10+
# (uv is used when present).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
dest="${1:-$here/install}"
mkdir -p "$dest"
dest="$(cd "$dest" && pwd)"

echo "installing Node servers into $dest"
cp "$here/package.json" "$here/package-lock.json" "$dest/"
# --ignore-scripts skips desktop-commander's install-tracking postinstall; its
# ripgrep binary comes from a platform package and needs no install script.
(cd "$dest" && npm ci --ignore-scripts --no-audit --no-fund)

echo "installing Python servers into $dest/venv"
if command -v uv >/dev/null 2>&1; then
  uv venv --quiet --allow-existing "$dest/venv"
  uv pip install --quiet --python "$dest/venv/bin/python" -r "$here/requirements.txt"
else
  python3 -m venv "$dest/venv"
  "$dest/venv/bin/python" -m pip install --quiet -r "$here/requirements.txt"
fi

# Desktop Commander keeps its state under $HOME/.claude-server-commander. The
# arm points HOME at dc-home so runs never read or change the user's own config.
# Telemetry is also disabled by DESKTOP_COMMANDER_DISABLE_TELEMETRY in arms.json;
# the welcome-onboarding flags are cleared so no onboarding text enters tool results.
dc_config="$dest/dc-home/.claude-server-commander"
mkdir -p "$dc_config"
python3 - "$dc_config/config.json" <<'PY'
import json, sys
path = sys.argv[1]
try:
    with open(path, encoding="utf-8") as handle:
        config = json.load(handle)
except FileNotFoundError:
    config = {}
config.update({
    "telemetryEnabled": False,
    "pendingWelcomeOnboarding": False,
    "welcomeOnboardingEligible": False,
})
with open(path, "w", encoding="utf-8") as handle:
    json.dump(config, handle, indent=2)
    handle.write("\n")
PY

# Desktop Commander downloads Chrome (for PDF tools) in the background whenever it
# starts and finds none. Download it once now, so parallel evaluation runs don't
# each start the same download into the shared cache.
if [[ "${ULTRA_EDIT_EVAL_SKIP_CHROME:-0}" != "1" ]]; then
  echo "warming Desktop Commander's Chrome cache (set ULTRA_EDIT_EVAL_SKIP_CHROME=1 to skip)"
  python3 "$here/warm_desktop_commander.py" "$dest"
fi

echo "third-party servers installed in $dest"
