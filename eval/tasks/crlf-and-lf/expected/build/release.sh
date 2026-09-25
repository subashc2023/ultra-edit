#!/bin/sh
set -eu
APP_VERSION=2.4.0
OUT_DIR=dist/unix
SIGN_BUILD=1
echo "Building ${APP_VERSION} into ${OUT_DIR}"
mkdir -p "$OUT_DIR"
printf 'compiled\n' > "$OUT_DIR/build.txt"
