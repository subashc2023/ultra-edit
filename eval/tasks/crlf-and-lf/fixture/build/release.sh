#!/bin/sh
set -eu
APP_VERSION=2.3.1
OUT_DIR=dist/unix
echo "Building ${APP_VERSION} into ${OUT_DIR}"
mkdir -p "$OUT_DIR"
printf 'compiled\n' > "$OUT_DIR/build.txt"
