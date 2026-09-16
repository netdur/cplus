# llama_cpp smoke recipe

Loads a GGUF through `vendor/llama_cpp`, prints what the model is, and
generates a short continuation.

```bash
export LLAMA_CPP_LIB="$HOME/Workspace/llama.cpp/build/bin"
cpc run -- /path/to/model.gguf
# or: LLAMA_CPP_MODEL=/path/to/model.gguf cpc run
```

Expected output:

```text
model:  llama 256M Q8_0
ctx:    2048
params: 162974016

The capital of France is Paris. The capital of France is Paris. The capital
```

## The one piece of wiring

llama.cpp has no standard install location, so `vendor/llama_cpp/Cplus.toml`
expands `${LLAMA_CPP_LIB}` into its `[link].search-paths`. Point it at the
directory holding `libllama.dylib` and `libmtmd.dylib`:

```bash
export LLAMA_CPP_LIB="$HOME/Workspace/llama.cpp/build/bin"
```

If it is unset, `cpc build` stops with E0865 naming the variable, rather than
letting the linker fail with an opaque "library not found". That directory is
used as both `-L` and `-rpath`, so `libllama` finds its own `libggml*.dylib`
siblings at run time.

See [vendor/llama_cpp/README.md](../../../../vendor/llama_cpp/README.md) for
building upstream llama.cpp and for regenerating the bindings after an update.
