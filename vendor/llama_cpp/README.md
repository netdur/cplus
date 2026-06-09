# llama_cpp package

C+ bindings for upstream `llama.cpp`.

The package name is `llama_cpp` because C+ dependency names are lowercase
identifiers. Use it like this:

```toml
[dependencies]
llama_cpp = "*"
```

```cplus
import "llama_cpp/llama_cpp" as llama;
```

## Architecture

This package binds upstream C APIs, not C++ internals:

```text
upstream/llama_cplus.h -> cpc-bindgen -> src/raw.cplus
upstream/mtmd_cplus.h  -> cpc-bindgen -> src/mtmd_raw.cplus
src/llama_cpp.cplus    -> small C+ facade
```

The curated headers intentionally use ABI-equivalent `void *` handles for
opaque llama.cpp / ggml / mtmd objects. That keeps the generated C+ surface
small while still linking directly to the real `llama_*` and `mtmd_*` symbols.

The MVP facade supports:

- backend init/free
- load a GGUF model into a session using `llama_model_load_from_file`
- greedy generation into a caller-provided byte buffer
- tokenization into a caller-provided token buffer
- manual decode/sample/token-to-piece calls for lower-level loops
- basic mtmd handles for multimodal capability detection, context init, and
  RGB bitmap creation

## Build

First build upstream `llama.cpp` as libraries. One source-build shape:

```bash
git clone https://github.com/ggml-org/llama.cpp.git
cmake -S llama.cpp -B llama.cpp/build \
  -DBUILD_SHARED_LIBS=ON \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TESTS=OFF \
  -DGGML_METAL=ON
cmake --build llama.cpp/build -j
```

Then build this bridge:
Then make sure [vendor/llama_cpp/Cplus.toml](Cplus.toml) points
`[link].search-paths` at the directory containing `libllama.dylib` and
`libmtmd.dylib`.

Regenerate raw bindings after changing the curated headers or `cpc-bindgen`:

```bash
cd vendor/llama_cpp
./build.sh
```

## XCFramework note

Upstream releases publish an Apple XCFramework. C+ manifests currently support
`-L` library search paths but not `-F` framework search paths, so the simplest
route for C+ is still to extract or build normal `libllama` / `libmtmd`
libraries and point `[link].search-paths` at them. A future compiler/linker
polish could add framework search paths and make the XCFramework flow direct.
