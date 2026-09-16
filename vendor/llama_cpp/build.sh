#!/usr/bin/env bash
# Regenerate src/raw.cplus and src/mtmd_raw.cplus from upstream llama.cpp's
# own public headers.
#
# There is no hand-curated ABI mirror in this package on purpose. A mirror is a
# second copy of a struct layout that nothing checks, and it drifts silently:
# the version of this package written against llama.cpp b6100 still declared
# llama_model_params::use_mmap/use_mlock (deleted upstream) and had never heard
# of load_mode/lazy_mode/load_mtp — every field after n_gpu_layers was reading
# the wrong bytes. cpc-bindgen takes the layout from clang's record layout of
# the real header, so that class of bug cannot recur.
#
#   LLAMA_CPP_SRC=/path/to/llama.cpp ./build.sh
set -euo pipefail

cd "$(dirname "$0")"
REPO_ROOT="$(cd ../.. && pwd)"

LLAMA_CPP_SRC="${LLAMA_CPP_SRC:-$HOME/Workspace/llama.cpp}"
CPC_BINDGEN="${CPC_BINDGEN:-$REPO_ROOT/target/release/cpc-bindgen}"

if [ ! -x "$CPC_BINDGEN" ]; then
  echo "error: cpc-bindgen not found at $CPC_BINDGEN" >&2
  echo "run: cargo build --release -p cpc-bindgen" >&2
  exit 1
fi

if [ ! -f "$LLAMA_CPP_SRC/include/llama.h" ]; then
  echo "error: no llama.cpp checkout at $LLAMA_CPP_SRC" >&2
  echo "set LLAMA_CPP_SRC=/path/to/llama.cpp" >&2
  exit 1
fi

UPSTREAM_REV="$(git -C "$LLAMA_CPP_SRC" rev-parse --short HEAD 2>/dev/null || echo unknown)"
UPSTREAM_DESC="$(git -C "$LLAMA_CPP_SRC" describe --tags --always 2>/dev/null || echo unknown)"

# Rewrite bindgen's absolute "Source header:" line into the upstream-relative
# path plus the revision it came from, so regeneration on another machine is
# byte-identical and the file records what it was generated against.
emit() {
  local header_rel="$1" out="$2"; shift 2
  "$CPC_BINDGEN" "$LLAMA_CPP_SRC/$header_rel" -- "$@" \
    | sed -e "s|^// Source header: .*|// Source header: llama.cpp $header_rel|" \
          -e "s|^// Every declaration|// Upstream revision: $UPSTREAM_DESC ($UPSTREAM_REV)\n//\n// Every declaration|" \
    > "$out"
  echo "regenerated vendor/llama_cpp/$out  <- llama.cpp $header_rel @ $UPSTREAM_DESC"
}

emit include/llama.h        src/raw.cplus \
  -I"$LLAMA_CPP_SRC/ggml/include"

emit tools/mtmd/mtmd.h      src/mtmd_raw.cplus \
  -I"$LLAMA_CPP_SRC/ggml/include" -I"$LLAMA_CPP_SRC/include" -I"$LLAMA_CPP_SRC/tools/mtmd"
