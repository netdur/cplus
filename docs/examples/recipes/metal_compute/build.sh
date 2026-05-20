#!/usr/bin/env bash
# build.sh — compile the Metal kernel and the C+ host together.
#
# Three stages:
#   1. xcrun metal:    shaders/double.metal -> shaders/double.air
#      xcrun metallib: shaders/double.air   -> shaders/double.metallib
#   2. Substitute the resulting .metallib byte size into a temp copy of
#      src/main.cplus (the source's `__SHADER_LEN__` placeholder).
#   3. cpc --emit-ll + clang:  IR -> metal_compute,
#      linked with -framework Metal -framework Foundation -lobjc.
#
# Step 2 is a workaround for the absence of an `include_str!` companion
# to `include_bytes!` in v0.0.6. When that ships, the host source will
# read the size via a complementary compile-time embed instead of via
# build-time sed.
#
# macOS-only. Prereq:
#   xcodebuild -downloadComponent MetalToolchain   # one-time, ~1.5 GB

set -euo pipefail
cd "$(dirname "$0")"

# 1) Compile the Metal shader to a .metallib.
xcrun -sdk macosx metal    -c shaders/double.metal -o shaders/double.air
xcrun -sdk macosx metallib    shaders/double.air   -o shaders/double.metallib

# 2) Determine the .metallib's byte size and patch the host source.
SHADER_LEN=$(stat -f%z shaders/double.metallib 2>/dev/null || stat -c%s shaders/double.metallib)
echo "shader size: ${SHADER_LEN} bytes"

mkdir -p build
sed "s/__SHADER_LEN__/${SHADER_LEN}/g" src/main.cplus > build/main.cplus

# 3) cpc -> LLVM IR -> clang link.
CPC="${CPC:-cpc}"
"$CPC" --emit-ll build/main.cplus > build/main.ll
clang build/main.ll \
    -framework Metal \
    -framework Foundation \
    -lobjc \
    -Wno-override-module \
    -o metal_compute

echo "built: ./metal_compute"
