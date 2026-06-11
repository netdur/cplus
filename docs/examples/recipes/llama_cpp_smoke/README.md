# llama_cpp smoke recipe

This is a wiring template for `vendor/llama_cpp`.

Before building it:

1. Build upstream `llama.cpp` with `libllama` and `libmtmd`.
2. Point cpc at that library directory via the `LLAMA_CPP_LIB` environment
   variable — `vendor/llama_cpp/Cplus.toml` expands `${LLAMA_CPP_LIB}` into its
   `[link].search-paths`:

   ```bash
   export LLAMA_CPP_LIB="$HOME/Workspace/llama.cpp/build/bin"
   ```

   If it is unset, `cpc build` stops with E0865 naming the variable, rather
   than letting the linker fail with an opaque "library not found".
3. Vendor or symlink the package into this recipe:

   ```bash
   mkdir -p docs/examples/recipes/llama_cpp_smoke/vendor
   ln -s ../../../../../vendor/llama_cpp docs/examples/recipes/llama_cpp_smoke/vendor/llama_cpp
   ```

4. Replace `/tmp/model.gguf` with a real GGUF path.
