# CoreAI smoke recipe

This is a wiring template for `vendor/coreai`, not a runnable example on the
current checked-in SDK.

Before building it:

1. Use Xcode/SDK with `CoreAI.framework`.
2. Build the bridge with `cd vendor/coreai && ./build.sh`.
3. Vendor or symlink the package into this recipe:

   ```bash
   mkdir -p docs/examples/recipes/coreai_smoke/vendor
   ln -s ../../../../../vendor/coreai docs/examples/recipes/coreai_smoke/vendor/coreai
   ```

4. Replace `/tmp/model.aimodel`, `"input"`, and `"output"` with the real model
   path and function signature.
