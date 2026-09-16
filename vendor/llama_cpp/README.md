# llama_cpp package

C+ bindings for upstream [`llama.cpp`](https://github.com/ggml-org/llama.cpp).

The package name is `llama_cpp` because C+ dependency names are lowercase
identifiers. Use it like this:

```toml
[dependencies]
llama_cpp = "*"
```

```cplus
import "llama_cpp/llama_cpp" as llama;
```

```cplus
llama::backend_init();
guard let result::Result[llama::Session, llama::LlamaError]::Ok(loaded) =
    llama::load("model.gguf", context_size: 4096 as u32) else { return 1; };
var session: llama::Session = loaded;

guard let result::Result[text::Text, llama::LlamaError]::Ok(answer) =
    session.generate("The capital of France is", max_new_tokens: 32) else { return 1; };
io::println(answer.view());
```

## Architecture

This package binds upstream's C APIs, not its C++ internals:

```text
llama.cpp include/llama.h    --cpc-bindgen-->  src/raw.cplus
llama.cpp tools/mtmd/mtmd.h  --cpc-bindgen-->  src/mtmd_raw.cplus
                                              src/llama_cpp.cplus  (the facade)
```

`src/raw.cplus` and `src/mtmd_raw.cplus` are **generated — do not hand-edit
them.** Struct layouts come from clang's own record layout of the real header,
so they are correct by construction, and opaque upstream handles arrive as
`*u8` with no ggml types to model.

**There is deliberately no curated copy of the headers in this package.** There
used to be one (`upstream/llama_cplus.h`), and it is the reason this package
needed regenerating: it still declared `llama_model_params::use_mmap`,
`use_direct_io` and `use_mlock` long after upstream deleted them, and had never
heard of `load_mode`, `lazy_mode` or `load_mtp`. Every field past
`n_gpu_layers` was reading the wrong bytes. A hand-written ABI mirror is a
second copy of a layout that nothing checks; regenerating from the real header
removes the whole class of bug.

The facade surface:

- `backend_init` / `backend_free`
- `load` — a GGUF into a ready-to-run `Session`, with labeled defaults for
  context size, GPU layers, threads, and the sampler chain
- `Session::generate` — prompt to an owned `Text`, greedy or sampled
- `Session::tokens_for` / `tokenize` / `token_piece` / `decode` / `sample` —
  the lower-level loop, for callers that want their own control flow
- `Session::describe`, `context_size`, `train_context_size`, `model_size`,
  `parameter_count`, `embedding_size`, `vocabulary_size`, `reset`
- `mtmd_caps_from_file`, `mtmd_init`, `MtmdContext`, `bitmap_rgb`,
  `bitmap_audio` — the multimodal C API

## Build

First build upstream `llama.cpp` as shared libraries:

```bash
git clone https://github.com/ggml-org/llama.cpp.git
cmake -S llama.cpp -B llama.cpp/build \
  -DBUILD_SHARED_LIBS=ON \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_SERVER=OFF \
  -DGGML_METAL=ON
cmake --build llama.cpp/build -j
```

`LLAMA_BUILD_TOOLS` must stay ON (its default) — `libmtmd` is built under
`tools/mtmd`, and this package links it.

Then point cpc at the directory holding `libllama.dylib` and `libmtmd.dylib`:

```bash
export LLAMA_CPP_LIB="$HOME/Workspace/llama.cpp/build/bin"
```

That directory is used as both `-L` and `-rpath`, which is how `libllama` finds
its own `@rpath/libggml*.dylib` siblings at run time. The ggml libraries are
therefore not listed in `[link].libs`.

## Regenerating after a llama.cpp update

```bash
cargo build --release -p cpc-bindgen
cd vendor/llama_cpp
LLAMA_CPP_SRC=/path/to/llama.cpp ./build.sh
```

`LLAMA_CPP_SRC` defaults to `$HOME/Workspace/llama.cpp`. The output records the
upstream revision it was generated against in a header comment, and
regeneration is byte-stable: the same checkout produces the same file.

Then run the suite — an upstream API rename shows up as a compile error in the
facade, and an upstream *layout* change shows up only in the e2e tests below.

## Testing

```bash
cd vendor/llama_cpp

export LLAMA_CPP_LIB=/path/to/llama.cpp/build/bin
export LLAMA_CPP_TEST_MODEL=/path/to/some-model.gguf      # any small GGUF
export LLAMA_CPP_TEST_MMPROJ=/path/to/mmproj-....gguf     # optional, for mtmd
cpc test
```

The suite has two halves:

- `src/llama_cpp.cplus` — unit and negative tests. Every one runs with null
  handles and never reaches a `llama_*` symbol, so they prove the guard logic
  and **nothing about the linked library**.
- `src/test_main.cplus` — the e2e half: loads a real GGUF and drives real
  inference. This is the only half that can catch an ABI drift between this
  package and the llama.cpp it was generated against, because a wrong struct
  layout compiles, links, and then hands libllama garbage. (Verified: putting
  the old stale `llama_model_params` back kills the test binary with SIGSEGV.)

Without `LLAMA_CPP_TEST_MODEL` the e2e half prints a `SKIPPED <test>: ...` line
per test and passes. **A green run with SKIPPED lines is not coverage of the
linked library** — set the variable before believing it.

## XCFramework note

Upstream releases publish an Apple XCFramework. C+ manifests support `-L`
library search paths but not `-F` framework search paths, so the simplest route
for C+ is still to extract or build normal `libllama` / `libmtmd` libraries and
point `[link].search-paths` at them. A future compiler/linker polish could add
framework search paths and make the XCFramework flow direct.
