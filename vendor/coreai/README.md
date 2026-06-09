# CoreAI package

This package is a C+ facade over Apple's Swift-first Core AI runtime.

Core AI loads `.aimodel` / `.aimodelc` assets and specializes them for Apple
silicon. Apple's current `coreai-models` repo lists Xcode 27.0+ and macOS/iOS
27.0+ as requirements for running and app integration.

## Shape

- `src/coreai.cplus` exposes opaque C+ handles: `Model`, `Function`, `NDArray`.
- `bridge/CoreAIBridge.swift` owns the real Swift/CoreAI values.
- `bridge/coreai_bridge.h` documents the C ABI between both sides.
- `build.sh` compiles the Swift bridge into `bridge/build/libcplus_coreai_bridge.dylib`.

## MVP API

```cplus
import "coreai/coreai" as coreai;

let model = coreai::load_model("/path/to/model.aimodel");
let function = coreai::load_function(model, "main");
let input = coreai::ndarray_f32(shape, values);
let output = coreai::run1_f32(function, "input", input, "output");
```

The first version intentionally supports only f32 tensors and one named input
to one named output. That is enough to prove the bridge with a small exported
model before adding descriptors, tokenizers, state views, and language-model
session helpers.

## Build

```bash
cd vendor/coreai
./build.sh
```

On machines without CoreAI in the active SDK this fails early with:

```text
error: CoreAI.framework not found in SDK: ...
```

That is expected on Xcode versions before CoreAI ships.
