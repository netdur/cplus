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

let model = coreai::Model::load("/path/to/model.aimodel");
let function = model.function(named: "main");
let input = coreai::NDArray::f32(shape, values);
let output = function.run(input, named: "input", to: "output");
output.copy_into(dest);
```

The surface follows `naming_guideline.md`: the raw bridge handle is hidden
behind `_raw`, construction takes its content (`Model::load`, `NDArray::f32`),
labeled parameters read as phrases (`function(named:)`, `run(input, named:, to:)`),
and the last bridge error is read as `coreai::last_error() -> Option[Text]`.

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
