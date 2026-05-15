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
HOST="$(clang -print-target-triple)"
OUT_DIR="$PKG_ROOT/src/lib/$HOST"
mkdir -p "$OUT_DIR"

OBJ="$(mktemp -t tiny_artifact.XXXXXX.o)"
trap 'rm -f "$OBJ"' EXIT
clang -O2 -c "$HERE/tiny_artifact.c" -o "$OBJ"
ar rcs "$OUT_DIR/libtiny_artifact.a" "$OBJ"

echo "built $OUT_DIR/libtiny_artifact.a (host: $HOST)"
echo "ensure '$HOST' appears in [link].triples of Cplus.toml — current value:"
grep -A 10 '^\[link\]' "$PKG_ROOT/Cplus.toml" || true
