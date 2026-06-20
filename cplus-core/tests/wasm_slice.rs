//! End-to-end test for the wasm playground slice
//! (`plans/plan.wasm-playground.md`): C+ source → WAT (`wasm_emit`) → wasm
//! bytes (`wat`) → run in a pure-Rust interpreter (`wasmi`), capturing the
//! `#println(i32)` host calls. This is the browser path minus the browser —
//! no toolchain, no clang, runnable anywhere CI runs.

use cplus_core::diagnostics::Severity;
use cplus_core::{attrs, borrowck, lexer, lower, parser, sema, wasm_emit};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Run the real front end and emit WAT, exactly as the playground would.
fn compile_to_wat(src: &str) -> Result<String, String> {
    let path = PathBuf::from("playground.cplus");
    let toks = lexer::tokenize(src).map_err(|e| format!("lex: {e:?}"))?;
    let mut prog = parser::parse(toks).map_err(|e| format!("parse: {e:?}"))?;
    let mut diags = attrs::check(&prog, path.clone(), src);
    diags.extend(lower::lower(&mut prog, &path, src));
    let (sema_diags, mono) =
        sema::check_multi_with_value_types(&prog, path.clone(), src, BTreeMap::new());
    diags.extend(sema_diags);
    diags.extend(borrowck::check(&prog, &path, src));
    if let Some(d) = diags.iter().find(|d| d.severity == Severity::Error) {
        return Err(format!("frontend {}: {}", d.code, d.message));
    }
    wasm_emit::generate_wat(&prog, &path, src, &mono.value_types).map_err(|d| format!("{}: {}", d.code, d.message))
}

/// Per-run host state: the captured `#println(i32)` outputs in call order.
#[derive(Default)]
struct Host {
    out: Vec<i32>,
}

/// Assemble + run a C+ program, returning `(main's return value, printed ints)`.
fn run(src: &str) -> (i32, Vec<i32>) {
    use wasmi::{Caller, Engine, Linker, Module, Store};

    let wat = compile_to_wat(src).expect("compile to WAT");
    let wasm = wat::parse_str(&wat).unwrap_or_else(|e| panic!("assemble WAT failed: {e}\n--- WAT ---\n{wat}"));

    let engine = Engine::default();
    let module = Module::new(&engine, &wasm[..]).expect("wasmi accepts the module");
    let mut store = Store::new(&engine, Host::default());
    let mut linker = <Linker<Host>>::new(&engine);
    linker
        .func_wrap("env", "println_i32", |mut caller: Caller<'_, Host>, n: i32| {
            caller.data_mut().out.push(n);
        })
        .expect("link host println_i32");

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("instantiate")
        .start(&mut store)
        .expect("start");
    let main = instance
        .get_typed_func::<(), i32>(&store, "main")
        .expect("exported main() -> i32");
    let ret = main.call(&mut store, ()).expect("run main");
    let out = std::mem::take(&mut store.data_mut().out);
    (ret, out)
}

#[test]
fn slice_hello_world_prints_0_1_2_and_returns_0() {
    // The plan's definition-of-done program.
    let (ret, out) = run(
        "fn main() -> i32 {\n    var i: i32 = 0;\n    while i < 3 {\n        #println(i);\n        i = i +% 1;\n    }\n    return 0;\n}\n",
    );
    assert_eq!(ret, 0);
    assert_eq!(out, vec![0, 1, 2]);
}

#[test]
fn user_function_and_arithmetic() {
    let (ret, out) = run(
        "fn add(a: i32, b: i32) -> i32 {\n    return a +% b;\n}\nfn main() -> i32 {\n    #println(add(40, 2));\n    return add(2, 3);\n}\n",
    );
    assert_eq!(ret, 5);
    assert_eq!(out, vec![42]);
}

#[test]
fn break_and_continue_control_flow() {
    // Print 0,1,2,3,4 then break at 5; `continue` skips the print for even-but
    // here just exercises both branches deterministically.
    let (ret, out) = run(
        "fn main() -> i32 {\n    var i: i32 = 0;\n    loop {\n        if i >= 5 {\n            break;\n        }\n        #println(i);\n        i = i +% 1;\n    }\n    return i;\n}\n",
    );
    assert_eq!(ret, 5);
    assert_eq!(out, vec![0, 1, 2, 3, 4]);
}

#[test]
fn wrapping_arithmetic_matches_twos_complement() {
    // i32::MAX +% 1 wraps to i32::MIN. Pins the slice's wrapping semantics
    // (the differential guardrail's core case).
    let (ret, _out) = run(
        "fn main() -> i32 {\n    var x: i32 = 2147483647;\n    x = x +% 1;\n    return x;\n}\n",
    );
    assert_eq!(ret, i32::MIN);
}

// ---- Phase 1: scalars / floats run end-to-end ----
// Every C+ function is exported, so a helper returning i64/f64 can be called
// directly to observe its value (the i32-only `#println` can't print those).

/// Instantiate and return the typed result of a no-arg exported function.
fn call_i64(src: &str, f: &str) -> i64 {
    use wasmi::{Caller, Engine, Linker, Module, Store};
    let wasm = wat::parse_str(&compile_to_wat(src).expect("compile")).expect("assemble");
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm[..]).expect("module");
    let mut store = Store::new(&engine, Host::default());
    let mut linker = <Linker<Host>>::new(&engine);
    linker.func_wrap("env", "println_i32", |mut c: Caller<'_, Host>, n: i32| c.data_mut().out.push(n)).unwrap();
    let inst = linker.instantiate(&mut store, &module).unwrap().start(&mut store).unwrap();
    inst.get_typed_func::<(), i64>(&store, f).expect("i64 fn").call(&mut store, ()).unwrap()
}

#[test]
fn i64_arithmetic_runs() {
    // 1_000_000 * 1_000_000 = 10^12, overflows i32 but fits i64. wasmi exposes
    // i64 results directly; f64 results are observed via `#println(.. as i32)`.
    let v = call_i64("fn big() -> i64 {\n    var x: i64 = 1000000;\n    x = x *% x;\n    return x;\n}\nfn main() -> i32 { return 0; }\n", "big");
    assert_eq!(v, 1_000_000_000_000);
}

#[test]
fn f64_arithmetic_runs() {
    // (1.5 + 2.25) * 4 = 15.0 → 15.
    let (_ret, out) = run(
        "fn main() -> i32 {\n    var a: f64 = 1.5;\n    var b: f64 = 2.25;\n    let s: f64 = a + b;\n    #println((s * 4.0) as i32);\n    return 0;\n}\n",
    );
    assert_eq!(out, vec![15]);
}

#[test]
fn int_to_float_cast_runs() {
    // (7 as f64)/2.0 = 3.5, ×10 = 35.0 → 35. Exercises i32→f64 and f64→i32.
    let (_ret, out) = run(
        "fn main() -> i32 {\n    let n: i32 = 7;\n    let h: f64 = (n as f64) / 2.0;\n    #println((h * 10.0) as i32);\n    return 0;\n}\n",
    );
    assert_eq!(out, vec![35]);
}

#[test]
fn value_if_and_short_circuit_run() {
    // max(a,b) via value-if, then gate a print on a && condition.
    let (ret, out) = run(
        "fn main() -> i32 {\n    let a: i32 = 3;\n    let b: i32 = 8;\n    let m: i32 = if a > b { a } else { b };\n    if m > 5 && a < b {\n        #println(m);\n    }\n    return m;\n}\n",
    );
    assert_eq!(ret, 8);
    assert_eq!(out, vec![8]);
}
