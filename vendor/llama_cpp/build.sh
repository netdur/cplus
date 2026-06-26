#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."

CPC_BINDGEN="${CPC_BINDGEN:-./target/debug/cpc-bindgen}"

if [ ! -x "$CPC_BINDGEN" ]; then
  echo "error: cpc-bindgen not found at $CPC_BINDGEN" >&2
  echo "run: cargo build -p cpc-bindgen" >&2
  exit 1
fi

"$CPC_BINDGEN" vendor/llama_cpp/upstream/llama_cplus.h > vendor/llama_cpp/src/raw.cplus
"$CPC_BINDGEN" vendor/llama_cpp/upstream/mtmd_cplus.h > vendor/llama_cpp/src/mtmd_raw.cplus

echo "regenerated vendor/llama_cpp/src/raw.cplus"
echo "regenerated vendor/llama_cpp/src/mtmd_raw.cplus"
