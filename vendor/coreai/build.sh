#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

mkdir -p bridge/build

if ! xcrun --find swiftc >/dev/null 2>&1; then
  echo "error: swiftc not found via xcrun" >&2
  exit 1
fi

SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"

if [ ! -d "$SDK_PATH/System/Library/Frameworks/CoreAI.framework" ]; then
  echo "error: CoreAI.framework not found in SDK: $SDK_PATH" >&2
  echo "Core AI currently requires an SDK that ships CoreAI.framework (Apple's coreai-models repo lists Xcode 27.0+)." >&2
  exit 1
fi

# CoreAI's `AIModel` / `InferenceFunction` are `@available(macOS 27.0, *)`, so
# the bridge's minimum deployment target must be 27.0 — the whole library is
# unusable below that anyway (the framework isn't present). Pinning it here
# avoids scattering `@available` annotations across the Swift source.
ARCH="$(uname -m)"
DEPLOY_TARGET="${ARCH}-apple-macos27.0"

xcrun swiftc \
  -sdk "$SDK_PATH" \
  -target "$DEPLOY_TARGET" \
  -parse-as-library \
  -emit-library \
  -emit-module \
  -module-name CPlusCoreAIBridge \
  -framework CoreAI \
  -framework Foundation \
  bridge/CoreAIBridge.swift \
  -o bridge/build/libcplus_coreai_bridge.dylib

echo "built bridge/build/libcplus_coreai_bridge.dylib"
