//! Differential parity for the wasm playground slice
//! (`plans/plan.wasm-playground.md`, task T6): the SAME C+ program, built two
//! ways, must produce the SAME output.
//!
//!   - native: `cpc src.cplus -o bin` → run → stdout  (the `#println(i32)`
//!     intrinsic lowers to `printf("%d\n", n)`),
//!   - wasm:   `wasm_emit::generate_wat` → `wat` → `wasmi` → captured ints,
//!     rendered as `"{n}\n"`.
//!
//! This is the guardrail that the wasm backend's semantics match the real
//! compiler — overflow/wrapping especially. It needs `clang` (the native
//! half); when clang is absent the test skips rather than fails, so a
//! toolchain-less CI stays green.

use cplus_core::diagnostics::Severity;
use cplus_core::{attrs, borrowck, lexer, lower, parser, sema, wasm_emit};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// Skip (don't fail) when there's no clang for the native half.
fn clang_available() -> bool {
    Command::new("clang")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Front end → WAT, exactly as the playground would run it.
fn compile_to_wat(src: &str) -> String {
    let path = PathBuf::from("playground.cplus");
    let toks = lexer::tokenize(src).expect("lex");
    let mut prog = parser::parse(toks).expect("parse");
    let mut diags = attrs::check(&prog, path.clone(), src);
    diags.extend(lower::lower(&mut prog, &path, src));
    let (sema_diags, mono) =
        sema::check_multi_with_value_types(&prog, path.clone(), src, BTreeMap::new());
    diags.extend(sema_diags);
    diags.extend(borrowck::check(&prog, &path, src));
    assert!(
        !diags.iter().any(|d| d.severity == Severity::Error),
        "frontend errors: {:?}",
        diags.iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>()
    );
    wasm_emit::generate_wat(&prog, &path, src, &mono.value_types).expect("emit WAT")
}

/// Run the wasm path; render `#println(i32)` output as the native binary would
/// print it (`"{n}\n"` per call), and return `(main_ret, rendered_stdout)`.
fn run_wasm(src: &str) -> (i32, String) {
    use wasmi::{Caller, Engine, Linker, Module, Store};

    let wat = compile_to_wat(src);
    let wasm = wat::parse_str(&wat).expect("assemble WAT");
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm[..]).expect("module");
    let mut store: Store<Vec<i32>> = Store::new(&engine, Vec::new());
    let mut linker = <Linker<Vec<i32>>>::new(&engine);
    linker
        .func_wrap("env", "println_i32", |mut caller: Caller<'_, Vec<i32>>, n: i32| {
            caller.data_mut().push(n);
        })
        .expect("link");
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("instantiate")
        .start(&mut store)
        .expect("start");
    let main = instance.get_typed_func::<(), i32>(&store, "main").expect("main");
    let ret = main.call(&mut store, ()).expect("run");
    let rendered = store.data().iter().map(|n| format!("{n}\n")).collect::<String>();
    (ret, rendered)
}

/// Build + run natively, returning `(exit_code, stdout)`.
fn run_native(src: &str) -> (i32, String) {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempfile::tempdir().expect("tempdir");
    let srcp = dir.path().join("prog.cplus");
    std::fs::write(&srcp, src).expect("write src");
    let bin = dir.path().join("prog");
    let status = Command::new(cpc)
        .arg(&srcp)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "native build failed");
    let out = Command::new(&bin).output().expect("run native");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The core assertion: native and wasm agree on output and return value.
fn assert_parity(src: &str) {
    if !clang_available() {
        eprintln!("skipping wasm differential test: no clang for the native half");
        return;
    }
    let (wret, wout) = run_wasm(src);
    let (nret, nout) = run_native(src);
    assert_eq!(nout, wout, "stdout diverged (native vs wasm)");
    assert_eq!(nret, wret, "return value diverged (native vs wasm)");
}

#[test]
fn hello_world_parity() {
    assert_parity(
        "fn main() -> i32 {\n    var i: i32 = 0;\n    while i < 3 {\n        #println(i);\n        i = i +% 1;\n    }\n    return 0;\n}\n",
    );
}

#[test]
fn arithmetic_and_calls_parity() {
    assert_parity(
        "fn sq(x: i32) -> i32 {\n    return x *% x;\n}\nfn main() -> i32 {\n    var i: i32 = 1;\n    while i <= 5 {\n        #println(sq(i));\n        i = i +% 1;\n    }\n    return sq(3);\n}\n",
    );
}

#[test]
fn wrapping_overflow_parity() {
    // The semantics most likely to diverge: i32::MAX +% 1. Both must wrap to
    // i32::MIN identically.
    assert_parity(
        "fn main() -> i32 {\n    var x: i32 = 2147483647;\n    x = x +% 1;\n    #println(x);\n    return 0;\n}\n",
    );
}

#[test]
fn i64_arithmetic_parity() {
    // 64-bit multiply then narrow to i32 to print: native (LLVM i64) and wasm
    // (i64.mul + i32.wrap_i64) must agree.
    assert_parity(
        "fn main() -> i32 {\n    var x: i64 = 100000;\n    x = x *% 3;\n    #println(x as i32);\n    return 0;\n}\n",
    );
}

#[test]
fn float_div_and_trunc_parity() {
    // f64 divide then truncate to i32: pins float arithmetic + float→int cast.
    assert_parity(
        "fn main() -> i32 {\n    var a: f64 = 7.0;\n    a = a / 2.0;\n    #println(a as i32);\n    return 0;\n}\n",
    );
}

#[test]
fn unsigned_division_parity() {
    // div_u vs div_s actually matters once the dividend's high bit is set.
    assert_parity(
        "fn main() -> i32 {\n    var x: u32 = 4000000000;\n    var y: u32 = 7;\n    let q: u32 = x / y;\n    #println(q as i32);\n    return 0;\n}\n",
    );
}
