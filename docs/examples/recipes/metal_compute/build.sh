#!/usr/bin/env bash
# build.sh — compile the Metal kernel and the C+ host together.
#
# Two stages:
#   1. xcrun metal:    shaders/double.metal -> shaders/double.air
#      xcrun metallib: shaders/double.air   -> shaders/double.metallib
#      Then write the resulting byte count to shaders/double.metallib.size
#      so `include_str!` can pick it up at cpc-build time.
#   2. cpc build:      reads ./Cplus.toml, walks imports, links against
#                      -framework Metal / Foundation / -lobjc declared
#                      in [[bin]].
#
# v0.0.7 Phase 4.1: dropped the v0.0.6 `--emit-ll | clang` two-step and
# the sed `__SHADER_LEN__` patch. Plain `cpc build` now does the link;
# the metallib size flows through `include_str!` instead of through
# build-time source substitution.
#
# macOS-only. Prereq:
#   xcodebuild -downloadComponent MetalToolchain   # one-time, ~1.5 GB

set -euo pipefail
cd "$(dirname "$0")"

# 1) Compile the Metal shader to a .metallib and pin its byte count.
xcrun -sdk macosx metal    -c shaders/double.metal -o shaders/double.air
xcrun -sdk macosx metallib    shaders/double.air   -o shaders/double.metallib

SHADER_LEN=$(stat -f%z shaders/double.metallib 2>/dev/null || stat -c%s shaders/double.metallib)
echo "shader size: ${SHADER_LEN} bytes"
printf '%s' "${SHADER_LEN}" > shaders/double.metallib.size

# 2) Build the host binary via plain cpc build (manifest-driven link).
CPC="${CPC:-cpc}"
"$CPC" build -o metal_compute

echo "built: ./metal_compute"
