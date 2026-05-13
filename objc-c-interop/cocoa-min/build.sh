#!/bin/sh
# Build the C+ Cocoa hello-world.
#
# Strategy: emit LLVM IR via cpc, then hand off to clang with -framework Cocoa
# for AppKit and -lobjc for the runtime. cpc itself doesn't know about
# Apple framework search paths or the ObjC runtime library — those are
# linker-level concerns we delegate to clang.

set -e

cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"
CPC="$ROOT/target/debug/cpc"

# Build the compiler if needed.
if [ ! -x "$CPC" ]; then
    (cd "$ROOT" && cargo build --quiet --bin cpc)
fi

# Emit IR.
"$CPC" --emit-ll hello_appkit.cplus > hello_appkit.ll

# Link against Cocoa.
clang hello_appkit.ll \
    -framework Cocoa \
    -lobjc \
    -Wno-override-module \
    -o hello_appkit

echo "Built: ./hello_appkit"
echo "Run:   ./hello_appkit"
