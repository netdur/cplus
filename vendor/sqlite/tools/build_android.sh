#!/bin/sh
# Builds lib/aarch64-linux-android/libsqlite3.a — the SQLite an Android app
# links, because Android gives it none (see Cplus.toml, `[android.link]`).
#
# The archive is COMMITTED, like camera.dex: a consumer links it without an NDK
# toolchain step of its own. Rerun this to move to a new SQLite; bump VERSION,
# YEAR and SHA3 together from https://www.sqlite.org/download.html (the
# PRODUCT line gives all three).
#
#   ANDROID_SDK_ROOT=~/Library/Android/sdk sh tools/build_android.sh
set -e
cd "$(dirname "$0")/.."

VERSION=3530400
YEAR=2026
SHA3=628a44cfe82c66aed1ccbbe85a562d2e33ebe64b3288981ed76285612227934e

SDK="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
BIN="$SDK/ndk/29.0.13846066/toolchains/llvm/prebuilt/darwin-x86_64/bin"
OUT=lib/aarch64-linux-android
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

curl -sfL "https://www.sqlite.org/$YEAR/sqlite-amalgamation-$VERSION.zip" -o "$WORK/a.zip"
GOT=$(python3 -I -c "import hashlib,sys;print(hashlib.sha3_256(open(sys.argv[1],'rb').read()).hexdigest())" "$WORK/a.zip")
if [ "$GOT" != "$SHA3" ]; then echo "sha3 mismatch: $GOT" >&2; exit 1; fi
(cd "$WORK" && unzip -q a.zip)

# THE OPTIONS, and why each one:
#   ENABLE_SNAPSHOT / STMT_SCANSTATUS / NORMALIZE / COLUMN_METADATA /
#   UNLOCK_NOTIFY / PREUPDATE_HOOK — src/raw.cplus binds these APIs, generated
#   from Apple's header; without them the symbols do not exist.
#   ENABLE_FTS5 / RTREE / MATH_FUNCTIONS — Apple's system SQLite has them, so
#   a query that runs on iOS runs here.
#   TEMP_STORE=3 — temp tables and sorts in memory. An app has no writable
#   /tmp, so a temp FILE (a big ORDER BY, VACUUM) would fail; AOSP's own
#   build sets the same.
"$BIN/clang" -target aarch64-linux-android24 -O2 -fPIC -c \
    "$WORK/sqlite-amalgamation-$VERSION/sqlite3.c" -o "$WORK/sqlite3.o" \
    -DSQLITE_THREADSAFE=1 -DHAVE_USLEEP=1 -DSQLITE_TEMP_STORE=3 \
    -DSQLITE_ENABLE_SNAPSHOT -DSQLITE_ENABLE_STMT_SCANSTATUS \
    -DSQLITE_ENABLE_NORMALIZE -DSQLITE_ENABLE_COLUMN_METADATA \
    -DSQLITE_ENABLE_UNLOCK_NOTIFY -DSQLITE_ENABLE_PREUPDATE_HOOK \
    -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_RTREE -DSQLITE_ENABLE_MATH_FUNCTIONS

mkdir -p "$OUT"
rm -f "$OUT/libsqlite3.a"
"$BIN/llvm-ar" rcs "$OUT/libsqlite3.a" "$WORK/sqlite3.o"

# Every symbol the binding names must be in the archive, or a consumer's
# `--no-undefined` link finds out instead of this script.
grep -o 'link_name = "sqlite3_[a-z0-9_]*"\|extern fn sqlite3_[a-z0-9_]*' src/raw.cplus \
    | grep -o 'sqlite3_[a-z0-9_]*' | sort -u > "$WORK/bound"
"$BIN/llvm-nm" -g --defined-only "$OUT/libsqlite3.a" 2>/dev/null \
    | awk 'NF==3 {print $3}' | sort -u > "$WORK/defined"
MISSING=$(comm -23 "$WORK/bound" "$WORK/defined")
if [ -n "$MISSING" ]; then echo "bound but not built:" >&2; echo "$MISSING" >&2; exit 1; fi
ls -l "$OUT/libsqlite3.a"
