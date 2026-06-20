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
| **Running** the i32 core (`cplus_run`) | ✅ | |
| **Running** the full language (FFI, heap, stdlib) | | ✅ |

There are two front ends here:

- [`cplus_compile`] — the original "does it compile, and what does it lower
  to" path: full diagnostics + pre-optimization LLVM IR, client-side.
- [`cplus_run`] — the *run* path
  (`plans/plan.wasm-playground.md`). For the **i32 core** of the language
  (arithmetic, `if`/`while`/`loop`, `#println`) it emits WebAssembly via
  `cplus_core::wasm_emit` **and assembles it to runnable bytes in-process**, so
  the page can execute the program with no `wat2wasm` download and no server.
  Richer programs (floats, structs, `Text`, the heap, FFI) report an `E1900`
  diagnostic — those need the full wasm backend (and, for FFI/stdlib, more than
  the slice). See `demo/index.html` for a complete client-side run.

`cpc` turns LLVM IR into a *native* binary by shelling out to `clang`, which
can't run in a browser — so executing the **full** language from the web still
needs the full wasm32 backend or a server-side runner. The slice proves the
client-side run path end-to-end on the i32 core.

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

### Running (the i32 slice)

```js
import init, { cplus_run } from "./cpc_wasm.js";
await init();

const { ok, diagnostics, wat, wasm } = JSON.parse(cplus_run(sourceString));
// ok:    boolean — true iff the program is in the runnable i32 subset
// wat:   string | null — the emitted WebAssembly text (for display)
// wasm:  number[] | null — the assembled module bytes, ready to instantiate

if (ok) {
  const { instance } = await WebAssembly.instantiate(new Uint8Array(wasm), {
    env: { println_i32: (n) => append(n + "\n") }, // #println(i32) → your page
  });
  const ret = instance.exports.main();             // run it
}
```

No `wat2wasm`/wabt download: the assembler (the pure-Rust `wat` crate) is
compiled into this blob, so `cplus_run` hands back instantiable bytes directly.

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
