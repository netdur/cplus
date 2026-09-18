#!/usr/bin/env bash
# Build libtiny_artifact.a for the host triple. Run once per host the
# package needs to support; commit (or cache) the resulting `.a`.
#
# A real upstream maintainer would run this in CI for each supported
# triple and ship the resulting binaries. The smoke test runs it
# on-the-fly because the host triple isn't known until install time.

set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PKG_ROOT="$(cd "$HERE/.." && pwd)"
RAW_HOST="$(clang -print-target-triple)"
# Match cpc's stable artifact-triple spelling: Apple clang includes the OS
# version and spells aarch64 as arm64, neither of which belongs in the package
# slice directory.
HOST="$(printf '%s\n' "$RAW_HOST" | sed -E \
  -e 's/^arm64-/aarch64-/' \
  -e 's/^((aarch64|x86_64)-apple-darwin)[0-9.]*$/\1/')"
OUT_DIR="$PKG_ROOT/lib/$HOST"
mkdir -p "$OUT_DIR"

OBJ="$(mktemp -t tiny_artifact.XXXXXX.o)"
trap 'rm -f "$OBJ"' EXIT
clang -O2 -c "$HERE/tiny_artifact.c" -o "$OBJ"
ar rcs "$OUT_DIR/libtiny_artifact.a" "$OBJ"

echo "built $OUT_DIR/libtiny_artifact.a (host: $HOST)"
