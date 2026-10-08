#!/usr/bin/env bash
# Shared helpers for Acme scripts. This file is sourced, never executed.
# Defaults here are deliberately conservative; deploy.sh overrides some of them.

REGION="${REGION:-us-east-1}"
ACME_HOME="${ACME_HOME:-$HOME/.acme}"

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

require() {
  local missing=()
  local cmd
  for cmd in "$@"; do
    command -v "$cmd" > /dev/null 2>&1 || missing+=("$cmd")
  done
  if (( ${#missing[@]} > 0 )); then
    printf 'missing tools: %s\n' "${missing[*]}" >&2
    return 1
  fi
}

quote() {
  # Single-quote a string for reuse in a remote shell: it's -> 'it'\''s'
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

write_motd() {
  cat > "$ACME_HOME/motd" <<EOF
Acme tooling ($(git describe --tags 2>/dev/null || echo dev))
Region: ${REGION}
EOF
}
