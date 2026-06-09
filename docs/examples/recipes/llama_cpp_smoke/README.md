# llama_cpp smoke recipe

This is a wiring template for `vendor/llama_cpp`.

Before building it:

1. Build upstream `llama.cpp` with `libllama` and `libmtmd`.
2. Ensure `vendor/llama_cpp/Cplus.toml` points `[link].search-paths` at that
   library directory.
3. Vendor or symlink the package into this recipe:

   ```bash
   mkdir -p docs/examples/recipes/llama_cpp_smoke/vendor
   ln -s ../../../../../vendor/llama_cpp docs/examples/recipes/llama_cpp_smoke/vendor/llama_cpp
   ```

4. Replace `/tmp/model.gguf` with a real GGUF path.
