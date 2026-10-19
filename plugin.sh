#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-web.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-web" ;;
    *) rel="x86_64-apple-darwin/gray-web" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-web" ;;
    *) rel="x86_64-unknown-linux-musl/gray-web" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-web.exe" "$here/target/release/gray-web" "$here/target/debug/gray-web.exe" "$here/target/debug/gray-web"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-web binary not found in $here" >&2
exit 1
