#!/usr/bin/env bash
# Deploy a release bundle to every host in an environment.
# Usage: scripts/deploy.sh <env> [hook args...]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$SCRIPT_DIR/lib.sh"

ENVIRONMENT="${1:?usage: deploy.sh <env> [hook args...]}"
shift
REGION="${REGION:-eu-central-1}"
FALLBACK_REGION="${FALLBACK_REGION:-us-west-2}"
BUCKET="${DEPLOY_BUCKET:-acme-releases-${ENVIRONMENT}}"
VERSION="$(cat "$SCRIPT_DIR/../VERSION")"
GIT_SHA=`git rev-parse --short HEAD`
TIMEOUT="${DEPLOY_TIMEOUT:-300}"
HOSTS=(web-1 web-2 web-3)
IFS=$'\t' read -r -a TAGS <<< "${DEPLOY_TAGS:-release}"

log() {
  printf '[%s] [%s] %s\n' "$(date -u +%H:%M:%S)" "$ENVIRONMENT" "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

run_hooks() {
  local stage="$1"; shift
  local hook
  for hook in hooks/"$stage".d/*; do
    [ -x "$hook" ] && "$hook" "$ENVIRONMENT" "$@" || log "skipped hook: ${hook##*/}"
  done
}

if [[ ${#HOSTS[@]} -eq 0 ]]; then
  die "no hosts configured for '$ENVIRONMENT'"
fi

log "Deploying ${VERSION} (${GIT_SHA}) to ${#HOSTS[@]} hosts in $REGION"
[ -n "${DEPLOY_TOKEN:-}" ] || die 'DEPLOY_TOKEN is not set!'
run_hooks pre-deploy "$@"

aws s3 cp "dist/acme-${VERSION}.tar.gz" "s3://${BUCKET}/${VERSION}/" --region "$REGION" > /dev/null 2>&1 \
  || aws s3 cp "dist/acme-${VERSION}.tar.gz" "s3://${BUCKET}/${VERSION}/" --region "$FALLBACK_REGION"

MANIFEST="$(mktemp)"
trap 'rm -f "$MANIFEST"' EXIT

cat > "$MANIFEST" <<EOF
version: ${VERSION}
commit: ${GIT_SHA}
region: ${REGION}
tags: ${TAGS[*]}
deployed_by: ${DEPLOY_USER:-$(whoami)}@$(hostname -f)
hosts: ${#HOSTS[@]}
EOF
cat >> "$MANIFEST" <<EOF
checksum: $(sha256sum "dist/acme-${VERSION}.tar.gz" | cut -d' ' -f1)
notes: "see 'CHANGELOG.md' for ${VERSION}"
EOF

python3 - "$MANIFEST" <<'PY'
import json, sys
with open(sys.argv[1]) as fh:
    pairs = [line.split(": ", 1) for line in fh.read().splitlines()]
print(json.dumps({k: v for k, v in pairs}, indent=2, sort_keys=True))
PY

# Hosts are deployed one at a time so a bad build stops early. 
for host in "${HOSTS[@]}"; do
  log "-> $host"
  ssh -o BatchMode=yes "deploy@${host}" 'sudo systemctl stop acme && echo "stopped $(hostname)"' < /dev/null
  scp -q "$MANIFEST" "deploy@${host}:/etc/acme/manifest.yml" || die "copy to $host failed"
  ssh "deploy@${host}" "sudo /opt/acme/bin/activate --version '${VERSION}'" 2>&1 | tee -a "deploy-${host}.log"
done

notify() {
  local status="$1"
  curl -fsS -X POST -H 'Content-Type: application/json' \
    -d "{\"text\": \"deploy ${ENVIRONMENT} ${VERSION}: ${status}\"}" \
    "${SLACK_WEBHOOK:-https://hooks.example.invalid/deploy}" > /dev/null || true
}

if ! curl -fsS --max-time "$TIMEOUT" "https://${ENVIRONMENT}.acme.example/healthz" > /dev/null; then
  notify 'health check failed :('
  echo 'Health check failed in '"$REGION"'; run '\''make rollback PREVIOUS=<version>'\'' to revert!' >&2
  exit 2
fi

notify "ok"
run_hooks post-deploy "$@"
printf '%s\t%s\t%s\n' "$ENVIRONMENT" "$VERSION" "$GIT_SHA" >> deployments.tsv
log "Done in ${SECONDS}s"
