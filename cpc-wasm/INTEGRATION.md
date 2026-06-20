# cpc-wasm — website integration handoff

This document is the contract between the `cpc-wasm` blob and the website
(cplus-lang.dev). It is written for the agent/engineer wiring the playground UI;
**no website code lives in this repo** — only the WASM module and this spec.

The module is **fully client-side and zero-infra**: a static `pkg/` directory
served as assets. No server, no API, no toolchain on the host.

---

## 1. Build the artifact

```sh
# one-time
cargo install wasm-pack
rustup target add wasm32-unknown-unknown

# build (re-run on every cpc-wasm change)
wasm-pack build cpc-wasm --target web --release
# → cpc-wasm/pkg/{cpc_wasm.js, cpc_wasm_bg.wasm, package.json, ...}

# optional: shrink the blob
wasm-opt -Oz cpc-wasm/pkg/cpc_wasm_bg.wasm -o cpc-wasm/pkg/cpc_wasm_bg.wasm
```

Ship the contents of `pkg/` as static assets. Import with the `web` target's
ES-module glue (see §3). A reference page that exercises the whole flow lives at
[`demo/index.html`](demo/index.html) — copy its logic, not its styling.

---

## 2. The two entry points

After `await init()` the module exports three functions. All take a single C+
source string; all are pure (no I/O, no globals) and safe to call on every
keystroke (debounce for cost, not correctness).

| Function | Returns | Use for |
|---|---|---|
| `cplus_version()` | `string` (e.g. `"0.0.24"`) | show the toolchain version |
| `cplus_compile(src)` | JSON string (see §2.1) | the **check + IR** view |
| `cplus_run(src)` | JSON string (see §2.2) | the **run** view (execute + output) |

The playground is **single-file**: `import` (modules / stdlib / vendor) is
rejected with an `E0000` diagnostic, because there is no resolver/filesystem in
the browser. Surface that message as-is.

### 2.1 `cplus_compile` — check + LLVM IR

```ts
type CompileResult = {
  ok: boolean;            // true iff no error-severity diagnostics
  diagnostics: Diagnostic[];
  ir: string | null;      // pre-optimization LLVM IR when ok, else null
};
```

### 2.2 `cplus_run` — execute the program

```ts
type RunResult = {
  ok: boolean;            // true iff the program is in the runnable subset
  diagnostics: Diagnostic[];
  wat: string | null;     // emitted WebAssembly TEXT, for display, when ok
  wasm: number[] | null;  // assembled module BYTES, ready to instantiate, when ok
};
```

`wasm` is the module **already assembled in-process** (the `wat2wasm` assembler
is compiled into this blob). You do **not** need wabt or any other download —
just instantiate the bytes (§4).

---

## 3. Loading the module

```js
import init, { cplus_compile, cplus_run, cplus_version } from "./pkg/cpc_wasm.js";
await init();                       // fetches + instantiates cpc_wasm_bg.wasm once
const version = cplus_version();    // "0.0.24"
```

`init()` must complete before any call. It is idempotent-ish but call it once at
startup and gate the UI on it.

---

## 4. Running a program (the critical bit)

```js
const res = JSON.parse(cplus_run(source));
if (!res.ok) {
  renderDiagnostics(res.diagnostics);     // see §5
  return;
}
showWat(res.wat);                          // optional "view WebAssembly" panel

const bytes = new Uint8Array(res.wasm);
const { instance } = await WebAssembly.instantiate(bytes, {
  env: {
    // The ONLY host import the slice needs. Called once per `#println(i32)`.
    println_i32: (n) => appendToConsole(String(n) + "\n"),
  },
});
const exitCode = instance.exports.main();  // i32 — the program's return value
```

**Import contract (stable):** the emitted module imports exactly
`env.println_i32(i32) -> ()` and exports `main() -> i32` (and exports `memory`,
currently unused). As the backend grows (§6) more `env.*` imports will appear —
treat unknown imports as a versioning signal and keep this list in one place.

There is no stdout stream: capture output purely through the host import
callbacks. Programs are sandboxed by the browser's own wasm engine; a runaway
loop hangs the worker, so **run wasm in a Web Worker** with a kill switch
(terminate + restart) for a timeout. The compile (`cplus_run`) step itself is
bounded and can stay on the main thread (debounced).

---

## 5. Diagnostics

`Diagnostic` is exactly `cplus_core::diagnostics::Diagnostic` serialized — the
same shape `cpc --emit=json` emits, identical between `cplus_compile` and
`cplus_run`:

```ts
type Diagnostic = {
  severity: "error" | "warning" | "note";
  code: string;                 // e.g. "E1900", "E0510", "E0000"
  message: string;
  primary: Span;                // the main location
  labels?: { span: Span; message: string }[];
  notes?: string[];
  suggestions?: { description: string; span: Span; replacement: string;
                  applicability: string }[];
};
type Span = {
  file: string;                 // always "playground.cplus"
  start: { line: number; col: number; byte: number };  // 1-based line/col
  end:   { line: number; col: number; byte: number };
};
```

Render notes/labels/suggestions if you can; at minimum show
`severity[code] line:col: message`. Use `byte` offsets for editor markers.

### Codes worth special-casing in the UI
- **`E0000`** — single-file refusal (`import` used). Show the message; it tells
  the user the playground can't do modules/stdlib.
- **`E1900`** — "not in the wasm playground yet" (the run path hit a construct
  outside the current runnable subset, §6). The `notes` field explains. This is
  expected for many valid programs today; present it as a capability limit, not
  a bug in the user's code. (`cplus_compile` will still type-check and show IR
  for these.)

---

## 6. Current runnable subset (and how it grows)

`cplus_run` currently runs the **i32 core**: integer arithmetic, comparisons,
bitwise ops, `if`/`while`/`loop`, `break`/`continue`/`return`, function calls,
and `#println(i32)`. Anything else (floats, structs/enums, `Text`/strings, the
heap, FFI/`unsafe`, threads) returns `E1900`.

This subset **expands over time** as the wasm backend grows (see
`plans/plan.wasm-playground.md`). Design the UI so growth is transparent:
- Don't hardcode "i32 only" copy; read capabilities from behavior (a program
  either runs or returns `E1900` with an explanatory note).
- New host imports may appear under `env.*` — keep the import object extensible.
- `cplus_version()` changes when the toolchain (and thus the subset) changes;
  use it as a cache-bust key for the `pkg/` assets.

`cplus_compile` (check + IR) already covers the **whole** language regardless of
the run subset — so a program that can't run yet still type-checks and shows IR.
A good UI offers both views.

---

## 7. Versioning / caching

- Treat `pkg/` as immutable per `cplus_version()`. Cache aggressively; bust on
  version change.
- The JSON contracts in §2 are additive-only going forward (new optional
  fields). `ok` / `diagnostics` / `ir` / `wat` / `wasm` are stable keys.
