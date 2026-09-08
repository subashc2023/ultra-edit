#!/bin/sh
set -eu

launcher_name=${0##*/}
case "$launcher_name" in
    ultra-edit|ultra-edit-mcp)
        ;;
    *)
        printf '%s\n' "ultra-edit: unsupported launcher name: $launcher_name" >&2
        exit 64
        ;;
esac

system=$(uname -s)
machine=$(uname -m)
case "$system:$machine" in
    Linux:x86_64|Linux:amd64)
        target=x86_64-unknown-linux-musl
        ;;
    Linux:aarch64|Linux:arm64)
        target=aarch64-unknown-linux-musl
        ;;
    Darwin:x86_64|Darwin:amd64)
        target=x86_64-apple-darwin
        ;;
    Darwin:arm64|Darwin:aarch64)
        target=aarch64-apple-darwin
        ;;
    *)
        printf '%s\n' "ultra-edit: unsupported platform: $system $machine" >&2
        exit 64
        ;;
esac

case "$0" in
    */*) launcher_path=$0 ;;
    *) launcher_path=$(command -v "$0") ;;
esac
case "$launcher_path" in
    */*) launcher_dir=${launcher_path%/*} ;;
    *) launcher_dir=. ;;
esac
case "$launcher_dir" in
    -*) launcher_dir=./$launcher_dir ;;
esac
launcher_dir=$(CDPATH= cd -P "$launcher_dir" && pwd)
executable=$launcher_dir/targets/$target/$launcher_name

if [ ! -x "$executable" ]; then
    printf '%s\n' "ultra-edit: packaged executable is missing or not executable: $executable" >&2
    exit 126
fi

exec "$executable" "$@"
