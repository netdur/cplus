# cpc-wasm — C+ front end for the web playground

Compiles the C+ front end (lex → parse → attrs → lower → sema → borrowck →
monomorphize → LLVM IR codegen) to WebAssembly so [cplus-lang.dev](https://cplus-lang.dev)
can check C+ and show its LLVM IR entirely client-side — no server round-trip,
no `clang`, no filesystem.

## What it does / doesn't do

| | In the browser (this crate) | Needs a server |
|---|---|---|
| Diagnostics (errors/warnings, full spans) | ✅ | |
| Pre-optimization LLVM IR | ✅ | |
| **Running** the compiled program | | ✅ |

`cpc` turns LLVM IR into a native binary by shelling out to `clang`. `clang`
can't run in a browser, so *executing* a C+ program from the web needs a
server-side runner (compile the IR with a hosted toolchain and run the binary
sandboxed) or a bundled wasm LLVM. This crate is the "does it compile, and what
does it lower to" half — which is most of a playground's value.

The playground is **single-file**: `import` (modules / stdlib / vendor) needs
the resolver + filesystem and is rejected with a clear diagnostic.

## JS API

After bundling (below), the module exports:

```js
import init, { cplus_compile, cplus_version } from "./cpc_wasm.js";

await init();
const json = cplus_compile(sourceString);
const { ok, diagnostics, ir } = JSON.parse(json);
// ok:           boolean — true iff no error-severity diagnostics
// diagnostics:  Array<{ severity: "error"|"warning"|"note",
//                       code: string,            // e.g. "E0510"
//                       message: string,
//                       primary: { file, start:{line,col,byte}, end:{...} },
//                       labels?, notes?, suggestions? }>
// ir:           string | null — LLVM IR when ok, else null

cplus_version(); // "0.0.13" — toolchain version this build came from
```

The diagnostic JSON is exactly what `cplus_core::diagnostics::Diagnostic`
serializes to, so it matches `cpc --emit=json` output field-for-field.

## Building the browser bundle

The raw `cargo build --target wasm32-unknown-unknown` output still needs
wasm-bindgen post-processing to generate the JS glue. Easiest path:

```sh
cargo install wasm-pack          # one-time
wasm-pack build cpc-wasm --target web --release
# → cpc-wasm/pkg/{cpc_wasm.js, cpc_wasm_bg.wasm, ...}
```

Or by hand:

```sh
rustup target add wasm32-unknown-unknown        # one-time
cargo install wasm-bindgen-cli                   # one-time, version-matched to the wasm-bindgen dep
cargo build -p cpc-wasm --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/cpc_wasm.wasm \
  --out-dir cpc-wasm/pkg --target web
wasm-opt -Oz cpc-wasm/pkg/cpc_wasm_bg.wasm -o cpc-wasm/pkg/cpc_wasm_bg.wasm  # optional, shrinks ~1.9M
```

Serve `pkg/` as a static asset from the website and `import` it as shown above.
