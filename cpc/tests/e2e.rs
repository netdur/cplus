use std::path::PathBuf;
use std::process::Command;

#[test]
fn hello_world_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bin = dir.join("hello");

    let compile = Command::new(cpc)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc exited non-zero: {compile}");

    let run = Command::new(&bin).output().expect("run produced binary");
    assert!(
        run.status.success(),
        "binary exited non-zero: {}",
        run.status
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "hello, world\n");
    assert!(run.stderr.is_empty(), "unexpected stderr: {:?}", run.stderr);
}

#[test]
fn emit_ir_prints_module() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--emit-ir")
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("define i32 @main()"), "missing main: {s}");
    assert!(s.contains("hello, world"), "missing greeting: {s}");
}

/// v0.0.16: owned locals declared in a loop body are dropped at the end of each
/// iteration (and on break/continue) — previously they leaked, because the
/// back-edge branch was emitted before the scope-exit drop hooks. A Drop counts
/// into a static; with a fresh owned value per iteration the total must equal the
/// iteration count across while / for / loop-with-break.
#[test]
fn loop_body_locals_drop_each_iteration() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("loopdrop.cplus");
    std::fs::write(
        &src,
        "static mut FREES: i32 = 0;\n\
         struct B { opaque data: *u8 }\n\
         impl B { fn drop(mut self) { unsafe { FREES = FREES + 1; } return; } }\n\
         fn work() {\n\
             let mut i: i32 = 0;\n\
             while i < 3 { let b: B = B { data: unsafe { 0 as *u8 } }; i = i + 1; }\n\
             for j in 0..2 { let c: B = B { data: unsafe { 0 as *u8 } }; }\n\
             let mut k: i32 = 0;\n\
             loop { let d: B = B { data: unsafe { 0 as *u8 } }; if k == 1 { break; } k = k + 1; }\n\
             return;\n\
         }\n\
         fn main() -> i32 { work(); return unsafe { FREES }; }\n",
    )
    .unwrap();
    let bin = dir.join("loopdrop");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "loop-drop program must compile");
    let run = Command::new(&bin).status().expect("run loopdrop");
    // while: 3, for: 2, loop (break on k==1 → k=0,1): 2  ⇒ 7 drops total.
    assert_eq!(
        run.code(),
        Some(7),
        "loop-body locals must drop each iteration; got {:?}",
        run.code()
    );
}

#[test]
fn diagnostics_json_emits_ndjson() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_path = dir.join("bad.cplus");
    std::fs::write(&src_path, "fn main() -> i32 { 1 < 2 < 3 }").unwrap();

    let out = Command::new(cpc)
        .arg("--diagnostics=json")
        .arg("--ast")
        .arg(&src_path)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure on bad source");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().next().expect("at least one diagnostic");
    let v: serde_json::Value = serde_json::from_str(line).expect("stderr line is valid JSON");
    assert_eq!(v["severity"], "error");
    assert_eq!(v["code"], "E0102");
    assert!(v["primary"]["file"]
        .as_str()
        .unwrap()
        .ends_with("bad.cplus"));
    assert!(
        v["message"].as_str().unwrap().contains("non-chainable")
            || v["message"].as_str().unwrap().contains("comparison")
    );
}

#[test]
fn diagnostics_short_format() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_path = dir.join("bad.cplus");
    std::fs::write(&src_path, "fn main() -> i32 { let x = 1 0 }").unwrap();

    let out = Command::new(cpc)
        .arg("--diagnostics=short")
        .arg("--ast")
        .arg(&src_path)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[E0100]"),
        "expected E0100 in stderr: {stderr}"
    );
    assert!(
        stderr.contains("bad.cplus:"),
        "expected file path in stderr: {stderr}"
    );
}

// ---- Phase 1 end-to-end: each sample program compiles, runs, prints expected output ----

fn compile_and_run(sample: &str) -> std::process::Output {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::copy(
        format!("{}/../docs/examples/{sample}", env!("CARGO_MANIFEST_DIR")),
        &src,
    )
    .expect("copy sample");
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc failed to compile {sample}");
    Command::new(&bin).output().expect("run produced binary")
}

#[test]
fn factorial_runs() {
    let out = compile_and_run("factorial.cplus");
    assert!(out.status.success(), "factorial exited non-zero");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3628800\n");
}

#[test]
fn fibonacci_runs() {
    let out = compile_and_run("fibonacci.cplus");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6765\n");
}

#[test]
fn sum_range_runs() {
    let out = compile_and_run("sum_range.cplus");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5050\n");
}

#[test]
fn c_for_runs() {
    let out = compile_and_run("c_for.cplus");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "45\n");
}

// Phase 2 slice 1: full primitive types + casts.

#[test]
fn mixed_ints_runs() {
    let out = compile_and_run("mixed_ints.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 1_000_000_000 + 1_000_000_000 = 2_000_000_000 (fits in i64 cleanly).
    // Truncated to i32: bit pattern of 2_000_000_000 in i32 is still 2_000_000_000.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "2000000000\n");
}

#[test]
fn float_arith_runs() {
    let out = compile_and_run("float_arith.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 3*3 + 4*4 = 25
    assert_eq!(String::from_utf8_lossy(&out.stdout), "25\n");
}

#[test]
fn unsigned_runs() {
    let out = compile_and_run("unsigned.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 1 + 2 + ... + 10 = 55
    assert_eq!(String::from_utf8_lossy(&out.stdout), "55\n");
}

// Phase 2 slice 2A: plain enums + path expressions

#[test]
fn direction_runs() {
    let out = compile_and_run("direction.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // opposite(North) == South, which has variant index 1
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
}

// Phase 2 slice 2B: structs (no methods)

#[test]
fn point_runs() {
    let out = compile_and_run("point.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // distance_squared((0,0), (3,4)) = 9 + 16 = 25
    assert_eq!(String::from_utf8_lossy(&out.stdout), "25\n");
}

#[test]
fn mutable_struct_runs() {
    let out = compile_and_run("mutable_struct.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "10\n");
}

#[test]
fn nested_struct_runs() {
    let out = compile_and_run("nested.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // line.to.x + line.to.y = 5 + 12 = 17
    assert_eq!(String::from_utf8_lossy(&out.stdout), "17\n");
}

// Phase 2 slice 2C: methods + impl blocks

#[test]
fn methods_runs() {
    let out = compile_and_run("methods.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // Point::new(3,4); translate(1,1) → (4,5); magnitude → 16 + 25 = 41
    assert_eq!(String::from_utf8_lossy(&out.stdout), "41\n");
}

// Phase 2 slice 2D: fixed-size arrays

#[test]
fn array_sum_runs() {
    let out = compile_and_run("array_sum.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 1+2+3+4+5 = 15
    assert_eq!(String::from_utf8_lossy(&out.stdout), "15\n");
}

#[test]
fn array_struct_runs() {
    let out = compile_and_run("array_struct.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 100 + 200 + 50 = 350
    assert_eq!(String::from_utf8_lossy(&out.stdout), "350\n");
}

#[test]
fn array_out_of_bounds_traps() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("oob.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let xs: [i32; 3] = [1, 2, 3]; return xs[10 as usize]; }",
    )
    .unwrap();
    let bin = dir.join("oob");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run");
    assert!(
        !run.status.success(),
        "expected trap on out-of-bounds index"
    );
}

// Phase 3 slice 3B: wrapping operators `+% -% *%`

#[test]
fn wrap_arith_runs() {
    let out = compile_and_run("wrap_arith.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 255u8 +% 1u8 = 0; 127i8 +% 1i8 = -128; 200u8 *% 2u8 = 144; 0u8 -% 1u8 = 255
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n-128\n144\n255\n");
}

#[test]
fn zero_initialized_static_aggregate_cross_lang_g033() {
    // v0.0.12 G-033 (llama.cplus G-032): cpc-defined aggregate globals
    // initialized with `#zero::[T]()` link cleanly into a C TU that
    // declares them `extern T name;`. Validates the flip-ownership
    // story end-to-end for arrays + #[repr(C)] structs: C reads from
    // and writes to cpc-owned BSS storage, cpc reads the C-side
    // writes back through the same symbol.
    //
    // Coincidentally also exercises a regression-prone codegen
    // ordering bug — pre-fix the struct type was declared *after* the
    // static that used it as a zeroinitializer operand, and clang
    // rejected it with "invalid type for null constant".
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let cplus_src = dir.join("g033.cplus");
    let c_src = dir.join("c_user.c");
    let cplus_obj = dir.join("g033.o");
    let c_obj = dir.join("c_user.o");
    let bin = dir.join("g033_bin");
    std::fs::write(
        &cplus_src,
        "#[repr(C)] struct S { a: i32, b: i64, opaque c: *u8 }\n\
         pub static mut MUT_I32_TABLE: [i32; 16] = #zero::[[i32; 16]]();\n\
         pub static mut MUT_STRUCT:    S         = #zero::[S]();\n\
         extern fn c_set_table(idx: i32, val: i32);\n\
         extern fn c_set_struct(a: i32, b: i64);\n\
         fn main() -> i32 {\n\
             // initial: cpc-owned, both zero\n\
             let v0: i32 = unsafe { MUT_I32_TABLE[5] };\n\
             if v0 != (0 as i32) { return 1; }\n\
             if unsafe { MUT_STRUCT.a } != (0 as i32) { return 2; }\n\
             // C writes through extern decl, cpc reads same storage\n\
             unsafe { c_set_table(5 as i32, 42 as i32); }\n\
             unsafe { c_set_struct(7 as i32, 99 as i64); }\n\
             if unsafe { MUT_I32_TABLE[5] } != (42 as i32) { return 3; }\n\
             if unsafe { MUT_STRUCT.a } != (7 as i32) { return 4; }\n\
             if unsafe { MUT_STRUCT.b } != (99 as i64) { return 5; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    std::fs::write(
        &c_src,
        // C+ `i64` is `int64_t` (`long long`), not `long`: `long` is 32-bit on
        // Windows (LLP64), which would mismatch the C+ field layout + ABI.
        "#include <stdint.h>\n\
         extern int32_t MUT_I32_TABLE[16];\n\
         extern struct S { int a; int64_t b; void* c; } MUT_STRUCT;\n\
         void c_set_table(int idx, int val) { MUT_I32_TABLE[idx] = val; }\n\
         void c_set_struct(int a, int64_t b) { MUT_STRUCT.a = a; MUT_STRUCT.b = b; }\n",
    )
    .unwrap();
    let clang_c = Command::new("clang")
        .args(["-c", "-o"])
        .arg(&c_obj)
        .arg(&c_src)
        .status()
        .expect("invoke clang for C side");
    assert!(clang_c.success(), "clang -c failed for C side");
    let cpc_emit = Command::new(cpc)
        .arg("--emit-obj")
        .arg(&cplus_src)
        .arg("-o")
        .arg(&cplus_obj)
        .status()
        .expect("invoke cpc --emit-obj");
    assert!(cpc_emit.success(), "cpc --emit-obj failed");
    let link = Command::new("clang")
        .arg(&cplus_obj)
        .arg(&c_obj)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke clang link");
    assert!(link.success(), "clang link failed");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "expected exit 0, got {:?} (cross-language aggregate-global regression?)",
        run.status
    );
}

#[test]
fn atomic_thread_fence_runtime_g030() {
    // v0.0.12 G-030 (llama.cplus G-029): standalone memory fence
    // through `stdlib/atomic`. The fence is correctness-irrelevant on
    // a single thread (no other writes to order), but the program must
    // compile and run without trapping. IR check confirms LLVM emits
    // `fence seq_cst`/etc. for the non-Relaxed orderings; Relaxed is
    // elided.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor")
        .join("stdlib");
    symlink_dir(&stdlib, &dir.join("vendor").join("stdlib"));
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"f\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"f\"\npath = \"src/main.cplus\"\n\
         [dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/atomic\" as atomic;\n\
         fn main() -> i32 {\n\
             atomic::atomic_thread_fence(atomic::Ordering::SeqCst);\n\
             atomic::atomic_thread_fence(atomic::Ordering::Acquire);\n\
             atomic::atomic_thread_fence(atomic::Ordering::Release);\n\
             atomic::atomic_thread_fence(atomic::Ordering::AcqRel);\n\
             atomic::atomic_thread_fence(atomic::Ordering::Relaxed);\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "atomic_thread_fence must compile under cpc build");
    let run = Command::new(dir.join("target/debug/f"))
        .output()
        .expect("run");
    assert!(run.status.success(), "fence program returned non-zero");
}

#[test]
fn cpu_relax_runtime_g031() {
    // v0.0.12 G-031 (llama.cplus G-030): spin-loop hint. Correctness-
    // irrelevant; check the program compiles + runs and the expected
    // architecture intrinsic appears in the IR.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("relax.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let mut i: i32 = 0;\n\
             while i < 4 { #cpu_relax(); i = i +% 1; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("relax");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#cpu_relax() must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success());

    // IR-level check: aarch64 → llvm.aarch64.hint; x86_64 → llvm.x86.sse2.pause
    let ll = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    let ir = String::from_utf8_lossy(&ll.stdout);
    if cfg!(target_arch = "aarch64") {
        assert!(
            ir.contains("llvm.aarch64.hint"),
            "aarch64 build must emit llvm.aarch64.hint, got:\n{ir}"
        );
    } else if cfg!(target_arch = "x86_64") {
        assert!(
            ir.contains("llvm.x86.sse2.pause"),
            "x86_64 build must emit llvm.x86.sse2.pause, got:\n{ir}"
        );
    }
}

#[test]
fn inline_asm_tier1_runtime() {
    // v0.0.14 inline-asm Tier 1: a bare-template `#asm` compiles, links, runs,
    // and emits an operand-free side-effecting asm call. `nop` is valid on
    // every target, so the IR check is arch-independent.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("asm.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             unsafe { #asm(\"nop\"); }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("asm");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#asm Tier 1 must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success());

    let ll = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    let ir = String::from_utf8_lossy(&ll.stdout);
    assert!(
        ir.contains("call void asm sideeffect \"nop\", \"\"()"),
        "expected operand-free sideeffect asm call, got:\n{ir}"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn inline_asm_tier2_operands_run_aarch64() {
    // v0.0.14 inline-asm Tier 2: `in`/`out`/`inout` operands compile, link, and
    // produce correct results on arm64. add(40,2)=42, inc(7)=8, sum=50.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("asm2.cplus");
    std::fs::write(
        &src,
        "fn add(a: i64, b: i64) -> i64 {\n\
             let mut s: i64 = 0;\n\
             unsafe { #asm(\"add {s}, {a}, {b}\", s = out(reg) s, a = in(reg) a, b = in(reg) b); }\n\
             return s;\n\
         }\n\
         fn inc(x: i64) -> i64 {\n\
             let mut v: i64 = x;\n\
             unsafe { #asm(\"add {v}, {v}, #1\", v = inout(reg) v); }\n\
             return v;\n\
         }\n\
         fn main() -> i32 {\n\
             let s: i64 = add(40 as i64, 2 as i64);\n\
             let t: i64 = inc(7 as i64);\n\
             if s != (42 as i64) { return 1; }\n\
             if t != (8 as i64) { return 2; }\n\
             return (s +% t) as i32;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("asm2");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#asm Tier 2 must compile");
    let run = Command::new(&bin).status().expect("run asm2");
    assert_eq!(run.code(), Some(50), "expected 50, got {:?}", run.code());
}

#[test]
#[cfg(target_arch = "aarch64")]
fn inline_asm_tier3_naked_fn_runs_aarch64() {
    // v0.0.14 inline-asm Tier 3: a `#[naked]` function — no prologue/epilogue,
    // body is inline asm reading args from ABI registers (x0/x1) and returning
    // via x0. raw_add(40, 2) = 42.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("naked.cplus");
    std::fs::write(
        &src,
        "#[naked]\n\
         fn raw_add(a: i64, b: i64) -> i64 {\n\
             unsafe { #asm(\"add x0, x0, x1\\nret\"); }\n\
         }\n\
         fn main() -> i32 {\n\
             let r: i64 = raw_add(40 as i64, 2 as i64);\n\
             return r as i32;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("naked");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#[naked] must compile");
    let run = Command::new(&bin).status().expect("run naked");
    assert_eq!(run.code(), Some(42), "expected 42, got {:?}", run.code());

    // IR: the function carries `naked noinline`, no param prologue, ends in
    // `unreachable` (the asm performs the return).
    let ll = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    let ir = String::from_utf8_lossy(&ll.stdout);
    assert!(
        ir.contains("@raw_add") && ir.contains("naked noinline"),
        "expected naked attribute on raw_add, got:\n{ir}"
    );
}

#[test]
fn inline_asm_outside_unsafe_rejected_e0801() {
    // Negative: `#asm` is unsafe; using it outside an `unsafe` block fails.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("asm_unsafe.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             #asm(\"nop\");\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "#asm outside unsafe must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0801"), "expected E0801, got:\n{stderr}");
}

#[test]
fn cross_module_unknown_item_reports_e0405_g030() {
    // v0.0.12 G-030 bonus: pre-fix, the resolver lumped "name doesn't
    // exist in module X" into PrivateAccess (E0403) with the misleading
    // "mark it `pub` ..." message. New variant E0405 fires for the
    // genuinely-missing case; E0403 stays for "exists but not pub".
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn real_fn() -> i32 { return 0; }\n\
         fn hidden_fn() -> i32 { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/missing.cplus"),
        "import \"./lib\" as lib;\nfn main() -> i32 { return lib::nope(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/private.cplus"),
        "import \"./lib\" as lib;\nfn main() -> i32 { return lib::hidden_fn(); }\n",
    )
    .unwrap();
    let missing = Command::new(cpc)
        .arg("check")
        .arg(dir.join("src/missing.cplus"))
        .output()
        .expect("invoke cpc check (missing)");
    assert!(!missing.status.success());
    let missing_err = String::from_utf8_lossy(&missing.stderr);
    assert!(
        missing_err.contains("E0405") && missing_err.contains("no item named"),
        "missing item must report E0405, got:\n{missing_err}"
    );
    assert!(
        !missing_err.contains("is private"),
        "missing item must NOT say `is private`, got:\n{missing_err}"
    );

    let private = Command::new(cpc)
        .arg("check")
        .arg(dir.join("src/private.cplus"))
        .output()
        .expect("invoke cpc check (private)");
    assert!(!private.status.success());
    let private_err = String::from_utf8_lossy(&private.stderr);
    assert!(
        private_err.contains("E0403") && private_err.contains("is private"),
        "genuinely-private item must still report E0403, got:\n{private_err}"
    );
}

#[test]
fn emit_obj_auto_detects_cplus_toml_g029() {
    // v0.0.12 G-029 (llama.cplus G-028): `cpc --emit-obj src/foo.cplus`
    // (the CMake `add_custom_command` shape) used to bypass `Cplus.toml`
    // entirely — so `import "stdlib/atomic"` fired E0852 even when the
    // file lived under a project that declared `stdlib = "*"`. The fix
    // walks up from the file's directory looking for `Cplus.toml`; if
    // found, the resolver gets the project's deps list. Three checks:
    //   (a) imports resolve when run from the project root
    //   (b) imports resolve when invoked from a different cwd (CMake's
    //       build/ directory)
    //   (c) single-file mode with no reachable manifest still rejects
    //       bare imports — backward-compat preserved.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor")
        .join("stdlib");
    symlink_dir(&stdlib, &dir.join("vendor").join("stdlib"));
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"g029\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"g029\"\npath = \"src/main.cplus\"\n\
         [dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/_probe.cplus"),
        "import \"stdlib/atomic\" as atomic;\n\
         fn touch() -> i32 { return 0; }\n",
    )
    .unwrap();

    // (a) from project root
    let obj_a = dir.join("probe_a.o");
    let a = Command::new(cpc)
        .arg("--emit-obj")
        .arg(dir.join("src/_probe.cplus"))
        .arg("-o")
        .arg(&obj_a)
        .current_dir(&dir)
        .output()
        .expect("invoke cpc --emit-obj from project root");
    assert!(
        a.status.success(),
        "(a) --emit-obj from project root must resolve stdlib import: {}",
        String::from_utf8_lossy(&a.stderr)
    );
    assert!(obj_a.exists(), "(a) .o not produced");

    // (b) from a different cwd (simulates CMake build dir)
    let cmake_dir = tempdir();
    let obj_b = cmake_dir.join("probe_b.o");
    let b = Command::new(cpc)
        .arg("--emit-obj")
        .arg(dir.join("src/_probe.cplus"))
        .arg("-o")
        .arg(&obj_b)
        .current_dir(&cmake_dir)
        .output()
        .expect("invoke cpc --emit-obj from external cwd");
    assert!(
        b.status.success(),
        "(b) --emit-obj from external cwd must auto-detect Cplus.toml: {}",
        String::from_utf8_lossy(&b.stderr)
    );
    assert!(obj_b.exists(), "(b) .o not produced");

    // (c) no manifest reachable — bare import still fails with E0852
    let bare_dir = tempdir();
    std::fs::write(
        bare_dir.join("bare.cplus"),
        "import \"stdlib/atomic\" as atomic;\nfn f() -> i32 { return 0; }\n",
    )
    .unwrap();
    let obj_c = bare_dir.join("bare.o");
    let c = Command::new(cpc)
        .arg("--emit-obj")
        .arg(bare_dir.join("bare.cplus"))
        .arg("-o")
        .arg(&obj_c)
        .output()
        .expect("invoke cpc --emit-obj on no-manifest file");
    assert!(
        !c.status.success(),
        "(c) bare-import without manifest must still fail"
    );
    let stderr_c = String::from_utf8_lossy(&c.stderr);
    assert!(
        stderr_c.contains("E0852"),
        "(c) expected E0852 for bare import without manifest, got: {stderr_c}"
    );
}

#[test]
fn zero_intrinsic_and_write_zeroed_runtime_g028() {
    // v0.0.12 G-028 (llama.cplus G-026): `#zero::[T]()` returns a
    // zeroed T; `*T.write_zeroed()` zeroes T-many bytes through a
    // raw pointer. Closes the C99 partial-init silent-garbage gap
    // that caught a real bug in ggml_dyn_tallocr_new.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g028.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         #[repr(C)]\n\
         struct Chunk { offset: usize, size: usize, opaque next: *u8, pad: i64 }\n\
         fn main() -> i32 {\n\
             // #zero::[T]() — stack value, all bytes zeroed.\n\
             let mut c: Chunk = #zero::[Chunk]();\n\
             if c.offset != (0 as usize) { return 1; }\n\
             if c.size   != (0 as usize) { return 2; }\n\
             c.size = 64 as usize;\n\
             if c.size != (64 as usize) { return 3; }\n\
             // *T.write_zeroed() — heap pointer, T-many bytes zeroed.\n\
             let p: *Chunk = unsafe { malloc(#size_of::[Chunk]()) as *Chunk };\n\
             unsafe { p.write_zeroed(); }\n\
             let d: Chunk = unsafe { *p };\n\
             if d.offset != (0 as usize) { return 4; }\n\
             if d.size   != (0 as usize) { return 5; }\n\
             unsafe { free(p as *u8); }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("g028");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#zero / write_zeroed must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "expected exit 0, got {:?}", run.status);
}

#[test]
fn extern_struct_return_sret_cross_language_g027() {
    // v0.0.12 G-027: cpc was emitting `declare %T @f(...)` + a direct
    // struct-return call for any extern fn returning >16B aggregate.
    // The AArch64-Darwin (and x86_64-sysv) C ABI requires sret — a
    // hidden `ptr sret(%T)` first arg. Mismatch → caller wrote args
    // into x0 where the callee expected the sret pointer → SIGSEGV.
    //
    // This test compiles a C side returning a 24B struct, a C+ side
    // importing it via `extern fn`, links them, and runs. Exit 0 means
    // the ABI agrees end-to-end. Pre-fix: SIGSEGV (139). Post-fix: 0.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let c_src = dir.join("c_side.c");
    let c_obj = dir.join("c_side.o");
    let cplus_src = dir.join("main.cplus");
    let ll = dir.join("main.ll");
    let bin = dir.join("g027");
    std::fs::write(
        &c_src,
        // NB: C+ `i64` is `long long`/`int64_t`, NOT `long` — `long` is only
        // 64-bit on LP64 (macOS/Linux); on Windows (LLP64) it is 32-bit, so a
        // `long`-based struct would mismatch the C+ `i64` layout and ABI.
        "typedef struct { long long a; long long b; long long c; } Big24;\n\
         Big24 make_big(long long x) {\n\
             Big24 r = { x + 1, x + 2, x + 3 };\n\
             return r;\n\
         }\n",
    ).unwrap();
    std::fs::write(
        &cplus_src,
        "#[repr(C)]\n\
         struct Big24 { a: i64, b: i64, c: i64 }\n\
         extern fn make_big(x: i64) -> Big24;\n\
         fn main() -> i32 {\n\
             let r: Big24 = unsafe { make_big(10 as i64) };\n\
             if r.a != (11 as i64) { return 1; }\n\
             if r.b != (12 as i64) { return 2; }\n\
             if r.c != (13 as i64) { return 3; }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    let clang_c = Command::new("clang")
        .args(["-c", "-o"])
        .arg(&c_obj)
        .arg(&c_src)
        .status()
        .expect("invoke clang for C side");
    assert!(clang_c.success(), "clang -c failed for C side");
    let ll_out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&cplus_src)
        .output()
        .expect("invoke cpc --emit-ll");
    assert!(ll_out.status.success(), "cpc --emit-ll failed");
    std::fs::write(&ll, &ll_out.stdout).unwrap();
    let link = Command::new("clang")
        .arg("-Wno-override-module")
        .arg(&ll)
        .arg(&c_obj)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke clang to link");
    assert!(link.success(), "clang link failed");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "expected exit 0, got {:?} (ABI regression — sret no longer emitted on extern import?)",
        run.status
    );
}

#[test]
fn extern_struct_param_abi_cross_language_g034() {
    // v0.0.12 G-034 (llama.cplus G-033): call-site mirror of G-027 on
    // the param side. cpc's *declaration* of an extern fn taking a
    // struct-by-value param classified it correctly per the AArch64-
    // Darwin C ABI (≤8B → coerce i64, ≤16B → coerce [2 x i64], >16B →
    // ptr indirect). The *call site* passed the raw `%T` aggregate
    // instead, silently mismatching → SIGSEGV on the first call.
    //
    // Drive all three size buckets through one cross-language binary.
    // Exit 0 means the ABI agrees end-to-end for each. Pre-fix:
    // SIGSEGV on the first call (exit 139).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let c_src = dir.join("c_side.c");
    let c_obj = dir.join("c_side.o");
    let cplus_src = dir.join("main.cplus");
    let ll = dir.join("main.ll");
    let bin = dir.join("g034");
    std::fs::write(
        &c_src,
        "#include <stdbool.h>\n\
         #include <stdint.h>\n\
         #include <stddef.h>\n\
         struct S8  { int64_t a; };\n\
         struct S16 { int64_t a; int64_t b; };\n\
         struct S24 { size_t  a; void *  b; bool    c; };\n\
         int64_t take_s8(struct S8 s)   { return s.a; }\n\
         int64_t take_s16(struct S16 s) { return s.a * 10 + s.b; }\n\
         int64_t take_s24(struct S24 s) { return (int64_t)s.a + (s.c ? 1000 : 0); }\n",
    )
    .unwrap();
    std::fs::write(
        &cplus_src,
        "#[repr(C)]\n\
         struct S8 { a: i64 }\n\
         #[repr(C)]\n\
         struct S16 { a: i64, b: i64 }\n\
         #[repr(C)]\n\
         struct S24 { a: usize, opaque b: *u8, c: bool }\n\
         extern fn take_s8(s: S8) -> i64;\n\
         extern fn take_s16(s: S16) -> i64;\n\
         extern fn take_s24(s: S24) -> i64;\n\
         fn main() -> i32 {\n\
             let v8: S8 = S8 { a: 1 as i64 };\n\
             let r8: i64 = unsafe { take_s8(v8) };\n\
             if r8 != (1 as i64) { return 1; }\n\
             let v16: S16 = S16 { a: 1 as i64, b: 2 as i64 };\n\
             let r16: i64 = unsafe { take_s16(v16) };\n\
             if r16 != (12 as i64) { return 2; }\n\
             let v24: S24 = S24 { a: 1 as usize, b: unsafe { 0 as *u8 }, c: true };\n\
             let r24: i64 = unsafe { take_s24(v24) };\n\
             if r24 != (1001 as i64) { return 3; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let clang_c = Command::new("clang")
        .args(["-c", "-o"])
        .arg(&c_obj)
        .arg(&c_src)
        .status()
        .expect("invoke clang for C side");
    assert!(clang_c.success(), "clang -c failed for C side");
    let ll_out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&cplus_src)
        .output()
        .expect("invoke cpc --emit-ll");
    assert!(ll_out.status.success(), "cpc --emit-ll failed");
    std::fs::write(&ll, &ll_out.stdout).unwrap();
    let link = Command::new("clang")
        .arg("-Wno-override-module")
        .arg(&ll)
        .arg(&c_obj)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke clang to link");
    assert!(link.success(), "clang link failed");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "expected exit 0, got {:?} (ABI regression — call-site struct-by-value coercion lost?)",
        run.status
    );
}

#[test]
fn unit_type_in_turbofish_runtime_g026() {
    // v0.0.12 G-026: `()` parses as the unit type in turbofish slots
    // and explicit return positions. Drives a generic fn through both
    // and confirms it executes.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("unit_type_g026.cplus");
    std::fs::write(
        &src,
        "fn run[T]() -> () { return; }\n\
         fn main() -> i32 {\n\
             run::[i32]();\n\
             run::[()]();\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("unit_type_g026");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "unit-type turbofish must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success());
}

#[test]
fn parse_error_in_entry_file_has_real_span_g026() {
    // v0.0.12 G-026 (span half): parse errors on the entry file in
    // project mode previously rendered with a `1:1` fallback span.
    // The fix registers each file's source into the loader BEFORE
    // attempting parse, so the diagnostic gets the real span back.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor")
        .join("stdlib");
    symlink_dir(&stdlib, &dir.join("vendor").join("stdlib"));
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sp\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"sp\"\npath = \"src/main.cplus\"\n\
         [dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/io\" as io;\n\nfn main() -> i32 {\n    let x: ( = 5;\n    return 0;\n}\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "bad syntax must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("4:14") || stderr.contains("main.cplus:4:"),
        "expected real span on line 4, got: {stderr}"
    );
    assert!(
        !stderr.contains("main.cplus:1:1"),
        "regression — span fell back to 1:1: {stderr}"
    );
}

#[test]
fn is_null_methods_runtime_g024() {
    // v0.0.12 G-024: `is_null()` / `is_not_null()` are builtin methods
    // on raw pointers; lower to a single `icmp eq/ne ptr %p, null`.
    // No unsafe required (no memory access).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("is_null_g024.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             let p: *u8 = unsafe { malloc(64 as usize) };\n\
             if p.is_null() { return 1; }\n\
             let nilp: *u8 = unsafe { 0 as *u8 };\n\
             if nilp.is_not_null() { return 2; }\n\
             if !nilp.is_null() { return 3; }\n\
             unsafe { free(p); }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("is_null_g024");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "is_null methods must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "is_null program returned non-zero: {:?}", run.status);
}

#[test]
fn addr_of_field_through_pointer_runtime_g025() {
    // v0.0.12 G-025: `#addr_of((*p).field)` is the pattern that blocked
    // the llama.cplus gallocr port — `ggml_hash_set_free(&galloc->hash_set)`
    // shaped calls. Codegen reuses `gen_place`, which walks Deref →
    // field-GEP on the pointed-to struct.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("addr_of_g025.cplus");
    std::fs::write(
        &src,
        "struct HashSet { count: i32, capacity: i32 }\n\
         struct Galloc  { id: i32, hash_set: HashSet, extra: i64 }\n\
         fn use_hs(hs: *HashSet) -> i32 { return unsafe { (*hs).count }; }\n\
         fn main() -> i32 {\n\
             let g: Galloc = Galloc { id: 7, hash_set: HashSet { count: 99, capacity: 256 }, extra: 1000 as i64 };\n\
             let gp: *Galloc = unsafe { #addr_of(g) };\n\
             let hsp: *HashSet = unsafe { #addr_of((*gp).hash_set) };\n\
             let a: [i32; 4] = [10, 20, 30, 40];\n\
             let aip: *i32 = unsafe { #addr_of(a[2]) };\n\
             let third: i32 = unsafe { *aip };\n\
             return (use_hs(hsp) - 99) + (third - 30);\n\
         }",
    )
    .unwrap();
    let bin = dir.join("addr_of_g025");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#addr_of place-expression must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "expected exit 0, got {:?}", run.status);
}

#[test]
fn neg_lit_with_lhs_type_runtime_g023() {
    // v0.0.12 G-023: `let x: i64 = -100;` must work end-to-end. Pre-fix,
    // sema rejected this with E0302 because the i64 expected-type wasn't
    // propagated into unary-minus' operand; codegen then emitted `sub i32`
    // into an i64 store. Covers multiple widths in one binary so a future
    // regression in any of them surfaces here.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("neg_lit_g023.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let a: i64 = -100;\n\
             let b: i64 = -2_147_483_649;\n\
             let c: i16 = -32768;\n\
             let d: i8  = -1;\n\
             let e: f32 = -1.5f32;\n\
             let f: f64 = -3.14;\n\
             let _a = a; let _b = b; let _c = c; let _d = d;\n\
             let _e = e; let _f = f;\n\
             if a >= (0 as i64) { return 1; }\n\
             if b >= (0 as i64) { return 2; }\n\
             if c >= (0 as i16) { return 3; }\n\
             if d >= (0 as i8)  { return 4; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let bin = dir.join("neg_lit_g023");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "neg-literal G-023 must compile");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "neg-literal program returned non-zero");
}

#[test]
fn wrapping_add_does_not_trap_in_debug() {
    // Plain `+` would trap; the wrapping form must NOT trap.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("wrap_no_trap.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let x: i32 = 2147483647; let y: i32 = x +% 1; #println(y); return 0; }",
    )
    .unwrap();
    let bin = dir.join("wrap_no_trap");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "compile failed");
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "wrapping add must not trap in debug");
    // 2147483647 +% 1 wraps to -2147483648
    assert_eq!(String::from_utf8_lossy(&run.stdout), "-2147483648\n");
}

// Phase 3 slice 3A: ownership surface syntax + move tracking

#[test]
fn ownership_runs() {
    let out = compile_and_run("ownership.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // `mut self` mutates buf to all 7s; checksum sums them (4 * 7 = 28);
    // first reads the first element (7). Order: sum, then first.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "28\n7\n");
}

// Phase 3 slice 3F: revived from slice 3A. The destructor-as-non-Copy idiom
// (an empty `fn drop(mut self) {}`) makes B non-Copy, restoring move
// consumption and re-firing E0335.

#[test]
fn use_after_move_rejected_at_compile_time() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("uaf.cplus");
    std::fs::write(
        &src,
        "struct B { x: i32 }\n\
         impl B { fn drop(mut self) {} fn consume(move self) -> i32 { return self.x; } }\n\
         fn main() -> i32 {\n\
           let b: B = B { x: 7 };\n\
           let s: i32 = b.consume();\n\
           return s + b.x;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("uaf");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for use-after-move"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0335"),
        "expected E0335 in stderr, got: {stderr}"
    );
}

#[test]
fn move_param_use_after_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("uam.cplus");
    std::fs::write(
        &src,
        "struct B { x: i32 }\n\
         impl B { fn drop(mut self) {} }\n\
         fn take(move b: B) -> i32 { return b.x; }\n\
         fn main() -> i32 {\n\
           let b: B = B { x: 3 };\n\
           let a: i32 = take(b);\n\
           return a + take(b);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("uam");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for double-consume"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0335"),
        "expected E0335 in stderr, got: {stderr}"
    );
}

// Phase 3 slice 3C: Copy auto-derive

#[test]
fn copy_struct_runs() {
    let out = compile_and_run("copy_struct.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // distance_squared = 3*3 + 4*4 = 25, then p.x = 3, p.y = 4.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "25\n3\n4\n");
}

// Phase 3 slice 3F: Drop (destructors)

#[test]
fn drop_basic_runs() {
    let out = compile_and_run("drop_basic.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // Tracker::new prints 1 then 2. Scope exit drops in reverse: -2 then -1.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n2\n-2\n-1\n");
}

#[test]
fn drop_move_runs() {
    let out = compile_and_run("drop_move.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // h is moved into take, where drop fires (prints -7). main's drop is
    // suppressed (flag was flipped on move). Then main prints the returned id.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "-7\n7\n");
}

// Phase 3 slice 3I: tagged unions + match

#[test]
fn maybe_runs() {
    let out = compile_and_run("maybe.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // unwrap_or(Some(7), -1) → 7; unwrap_or(None, -1) → -1.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "7\n-1\n");
}

#[test]
fn shape_runs() {
    let out = compile_and_run("shape.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // Circle(2)→48, Rect(3,5)→60, Square(4)→64, Empty→0.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "48\n60\n64\n0\n");
}

#[test]
fn uninit_init_runs() {
    let out = compile_and_run("uninit_init.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n-1\n0\n");
}

#[test]
fn loops_runs() {
    let out = compile_and_run("loops.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // sum_with_loop(5)=15; skip_evens_under(6)=9; drain_with_while_let()=10.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "15\n9\n10\n");
}

/// `break` outside a loop is E0353.
#[test]
fn break_outside_loop_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "fn main() -> i32 { break; return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure on bare `break`"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0353"), "expected E0353, got: {stderr}");
}

/// `continue` outside a loop is E0353.
#[test]
fn continue_outside_loop_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "fn main() -> i32 { continue; return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure on bare `continue`"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0353"), "expected E0353, got: {stderr}");
}

/// Phase 5 slice 5BC.4 — Rule E3 multi-parameter elision. Originally
/// asserted E0372 (move-while-borrowed) under the v0.0.9 default that
/// `x: T` (non-Copy) means borrow. Under v0.0.10 Phase 5 default-move,
/// `longest(a, b)` consumes both inputs at the call site, so the
/// subsequent `drain(a)` is detected as a plain use-after-move (E0335)
/// before the borrow-region machinery is reached. Same bug detected,
/// different error code.
#[test]
fn longest_move_either_input_while_borrowed_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn longest(a: B, b: B) -> B {
    if a.x > b.x {
        return a;
    }
    return b;
}
fn drain(move b: B) { return; }
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = longest(a, b);
    drain(a);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move-while-multi-source-borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Under v0.0.10 Phase 5 the same bug surfaces as E0335 (use-after-move)
    // rather than E0372 (move-while-borrowed) — `longest(a, b)` already
    // consumed `a` by the time `drain(a)` runs.
    assert!(
        stderr.contains("E0335") || stderr.contains("E0372"),
        "expected E0335 or E0372, got: {stderr}"
    );
}

/// Phase 5 slice 5BC.3b: originally asserted E0372 (move while a
/// Rule-E1 return-borrow is still live). Under v0.0.10 Phase 5
/// default-move, `passthrough(x)` consumes `x`, so the subsequent
/// `drain(x)` is a plain E0335 (use-after-move) — same bug detected,
/// different code.
#[test]
fn move_while_return_borrow_live_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(move b: B) { return; }
fn main() -> i32 {
    let x: B = B { x: 1 };
    let r: B = passthrough(x);
    drain(x);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move-while-borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0335") || stderr.contains("E0372"),
        "expected E0335 or E0372, got: {stderr}"
    );
}

/// Phase 5 slice 5BC.2a: originally asserted E0370 (move + shared-borrow
/// in same call). Under v0.0.10 Phase 5 default-move, the first arg
/// `peek(y)` already consumed `y`, so the second arg `y` is a plain
/// use-after-move (E0335).
#[test]
fn move_and_borrow_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn drain(n: i32, move b: B) { return; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    drain(peek(y), y);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move-and-borrow conflict"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0335") || stderr.contains("E0370"),
        "expected E0335 or E0370, got: {stderr}"
    );
}

#[test]
fn uninit_read_before_assign_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ua.cplus");
    std::fs::write(&src, "fn main() -> i32 { let x: i32; return x; }\n").unwrap();
    let bin = dir.join("ua");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure on read-before-assign"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0345"),
        "expected E0345 in stderr, got: {stderr}"
    );
}

#[test]
fn non_exhaustive_match_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nonex.cplus");
    std::fs::write(
        &src,
        "enum M { A, B }\n\
         fn main() -> i32 {\n\
           let m: M = M::A;\n\
           return match m { M::A => 0 };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("nonex");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for non-exhaustive match"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0340"),
        "expected E0340 in stderr, got: {stderr}"
    );
}

// Phase 3 slice 3G: defer

#[test]
fn defer_basic_runs() {
    let out = compile_and_run("defer_basic.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 1, 5 print in order; defers fire LIFO at scope exit (4, 3, 2).
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n5\n4\n3\n2\n");
}

#[test]
fn defer_drop_interleave_runs() {
    let out = compile_and_run("defer_drop.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // Construction: 1, 2. Scope exit reverses the registration stack:
    //   defer #println(200) -> 200
    //   Drop(b)            -> -2
    //   defer #println(100) -> 100
    //   Drop(a)            -> -1
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1\n2\n200\n-2\n100\n-1\n"
    );
}

// ---- runtime trap behavior for overflow + divide-by-zero ----

const OVERFLOW_PROGRAM: &str =
    "fn main() -> i32 { let mut x: i32 = 2147483647; x = x + 1; #println(x); return 0; }";

const DIV_ZERO_PROGRAM: &str =
    "fn main() -> i32 { let x: i32 = 10; let y: i32 = 0; return x / y; }";

fn compile_program(src: &str, release: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let path = dir.join("prog.cplus");
    std::fs::write(&path, src).unwrap();
    let bin = dir.join("prog");
    let mut cmd = Command::new(cpc);
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .arg(&path)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "compile failed");
    (dir, bin)
}

#[test]
fn debug_mode_traps_on_overflow() {
    let (_dir, bin) = compile_program(OVERFLOW_PROGRAM, false);
    let run = Command::new(&bin).output().expect("run");
    assert!(
        !run.status.success(),
        "expected trap on overflow in debug; got success with stdout={:?}",
        String::from_utf8_lossy(&run.stdout)
    );
    // Trap aborts before reaching `println`, so stdout should be empty.
    assert!(run.stdout.is_empty());
}

#[test]
fn release_mode_wraps_on_overflow() {
    let (_dir, bin) = compile_program(OVERFLOW_PROGRAM, true);
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "expected release wrap to succeed; status={:?} stderr={:?}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    // INT_MAX + 1 wraps to INT_MIN.
    assert_eq!(String::from_utf8_lossy(&run.stdout), "-2147483648\n");
}

#[test]
fn divide_by_zero_traps_in_debug() {
    let (_dir, bin) = compile_program(DIV_ZERO_PROGRAM, false);
    let run = Command::new(&bin).output().expect("run");
    assert!(!run.status.success(), "expected div-by-zero trap in debug");
}

#[test]
fn divide_by_zero_traps_in_release() {
    let (_dir, bin) = compile_program(DIV_ZERO_PROGRAM, true);
    let run = Command::new(&bin).output().expect("run");
    assert!(
        !run.status.success(),
        "div-by-zero must trap in release too (per plan §2.3)"
    );
}

#[test]
fn sema_error_in_compile_emits_diagnostic() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "fn main() -> i32 { let x = 1; x = 2; 0 }").unwrap();
    let bin = dir.join("bad");
    let result = Command::new(cpc)
        .arg("--diagnostics=short")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !result.status.success(),
        "expected sema failure to fail compilation"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("E0305"),
        "expected E0305 (immutable assign), got: {stderr}"
    );
}

// ---- Phase 4 slice 4A.5: `if let` / `guard let` ----

#[test]
fn if_let_basic_runs() {
    let out = compile_and_run("if_let_basic.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // unwrap_or(Some(42), 0) → 42; unwrap_or(None, 7) → 7.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n7\n");
}

#[test]
fn guard_let_chain_runs() {
    let out = compile_and_run("guard_let_chain.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // pipeline(10) → 24 (10→20→25→24); pipeline(-5) → -1 (step_a fails).
    assert_eq!(String::from_utf8_lossy(&out.stdout), "24\n-1\n");
}

#[test]
fn guard_let_complement_runs() {
    let out = compile_and_run("guard_let_complement.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // run(5) → Ok(105) → 105; run(-3) → Err(-4) → wrapped → 4.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "105\n4\n");
}

#[test]
fn irrefutable_if_let_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { if let x = 7 { return x; } return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure on irrefutable if-let"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0347"),
        "expected E0347 in stderr, got: {stderr}"
    );
}

#[test]
fn non_diverging_guard_let_else_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        r#"enum M { S(i32), N }
fn main() -> i32 {
    let m: M = M::S(1);
    guard let M::S(v) = m else { let x: i32 = 1; };
    return v;
}
"#,
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure on non-diverging guard-let else"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0348"),
        "expected E0348 in stderr, got: {stderr}"
    );
}

// ---- Phase 4 slice 4A: multi-file projects via `cpc build` ----

/// Copy the in-tree `hello_mods` sample to a tempdir and run `cpc build`
/// from inside it; the produced binary should print `49`.
#[test]
fn hello_mods_project_builds_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    let manifest = include_str!("../../docs/examples/projects/hello_mods/Cplus.toml");
    let main_src = include_str!("../../docs/examples/projects/hello_mods/src/main.cplus");
    let math_src = include_str!("../../docs/examples/projects/hello_mods/src/math.cplus");
    std::fs::write(dir.join("Cplus.toml"), manifest).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.cplus"), main_src).unwrap();
    std::fs::write(dir.join("src/math.cplus"), math_src).unwrap();

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");

    let bin = dir.join("target/debug/hello_mods");
    assert!(bin.is_file(), "expected binary at {}", bin.display());
    let out = Command::new(&bin).output().expect("run binary");
    assert!(
        out.status.success(),
        "binary exited non-zero: {}",
        out.status
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "49\n");
}

#[test]
fn public_type_alias_facade_reexports_struct_literals_and_methods() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"alias_facade\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/types.cplus"),
        r#"
pub struct Point {
    pub x: i32,
}

impl Point {
    pub fn new(x: i32) -> Point {
        return Point { x: x };
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("src/facade.cplus"),
        r#"
import "./types" as types;

pub type Point = types::Point;
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        r#"
import "./facade" as facade;

fn main() -> i32 {
    let a = facade::Point { x: 20 };
    let b = facade::Point::new(22);
    return a.x + b.x;
}
"#,
    )
    .unwrap();

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");

    let out = Command::new(dir.join("target/debug/alias_facade"))
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(42));
}

/// v0.0.2 AppKit-via-Cplus.toml: a manifest declaring `frameworks` and
/// `libs` produces a binary linked against those frameworks/libraries.
///
/// Test strategy: build a tiny project that uses `objc_getClass` from
/// libobjc (a Darwin-stable symbol). Without `libs = ["objc"]` the link
/// fails; with it, the link succeeds and the binary runs. Skipped on
/// non-macOS because `-lobjc` only resolves on Apple platforms.
#[test]
#[cfg(target_os = "macos")]
fn manifest_libs_links_libobjc() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"objc_smoke\"\n\n[[bin]]\nname = \"objc_smoke\"\npath = \"src/main.cplus\"\nlibs = [\"objc\"]\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "extern fn objc_getClass(name: *u8) -> *u8;\n\
         fn main() -> i32 {\n\
           let cstr: str = \"NSObject\";\n\
           let p: *u8 = unsafe { #str_ptr(cstr) };\n\
           let cls: *u8 = unsafe { objc_getClass(p) };\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build with libs failed: {status}");
    let bin = dir.join("target/debug/objc_smoke");
    assert!(bin.is_file(), "expected binary at {}", bin.display());
}

/// v0.0.2 AppKit-via-Cplus.toml: `frameworks` flows to `clang -framework`.
/// Build a manifest that asks for Foundation; the build must succeed.
#[test]
#[cfg(target_os = "macos")]
fn manifest_frameworks_passes_dash_framework() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"fw\"\n\n[[bin]]\nname = \"fw\"\npath = \"src/main.cplus\"\nframeworks = [\"Foundation\"]\nlibs = [\"objc\"]\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // The body doesn't have to use Foundation — we only need to prove the
    // -framework flag is accepted (linker would silently ignore an unused
    // framework, but a typo or unknown framework name will fail link).
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "cpc build with frameworks failed: {status}"
    );
}

/// `cpc build` without a `Cplus.toml` in cwd should fail with a manifest
/// error (not a panic, not a generic crash).
#[test]
fn cpc_build_without_manifest_errors_cleanly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(!out.status.success(), "expected failure without manifest");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Cplus.toml") || stderr.contains("manifest"),
        "stderr should mention manifest: {stderr}"
    );
}

/// Slice 4B: a cross-file call to a non-`pub` function should fail with E0403.
#[test]
fn cross_file_private_fn_emits_e0403() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/math.cplus"),
        "fn square(n: i32) -> i32 { return n * n; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./math\" as math;\nfn main() -> i32 { return math::square(7); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0403"), "expected E0403, got: {stderr}");
}

/// Slice 4C: a sema diagnostic whose error site sits in an *imported*
/// file should render with that file's path + a line/col that maps into
/// the imported file's source — not the entry file's. Pre-4C, all
/// diagnostics rendered against the entry file's line-map regardless of
/// origin, so a cross-file error would show wrong (or out-of-range)
/// coordinates.
#[test]
fn cross_file_sema_error_renders_in_imported_file() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // Imported file: well-formed structure but a sema error inside —
    // `square` is declared `-> i32` but returns a float. The E0302
    // points into math.cplus, NOT main.cplus.
    std::fs::write(
        dir.join("src/math.cplus"),
        "pub fn square(n: i32) -> i32 { return 1.5; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./math\" as math;\nfn main() -> i32 { return math::square(7); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--diagnostics=short")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected sema failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The error's file path should end in math.cplus, not main.cplus.
    let line = stderr.lines().next().expect("at least one diagnostic");
    assert!(
        line.contains("math.cplus:"),
        "diagnostic should be attributed to math.cplus, got: {line}"
    );
    assert!(line.contains("E0302"), "expected E0302, got: {line}");
}

/// Slice 4C: reading a non-`pub` field across a file boundary should
/// fail with E0403.
#[test]
fn cross_file_private_field_read_emits_e0403() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/geom.cplus"),
        // Struct is pub; first field is pub, second isn't.
        "pub struct Point { pub x: i32, y: i32 }\nimpl Point { pub fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nfn main() -> i32 { let p: g::Point = g::Point::new(1, 2); return p.y; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected E0403 from private-field read"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0403"),
        "expected E0403 in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("private"),
        "expected diagnostic to mention 'private': {stderr}"
    );
}

/// Slice 4C: reading a `pub` field across a file boundary works.
#[test]
fn cross_file_public_field_read_works() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/geom.cplus"),
        "pub struct Point { pub x: i32, pub y: i32 }\nimpl Point { pub fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nfn main() -> i32 { let p: g::Point = g::Point::new(3, 4); return p.x; }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "expected build to succeed: {status}");
    let bin = dir.join("target/debug/x");
    let out = Command::new(&bin).output().expect("run");
    // p.x = 3 → exit code 3.
    assert_eq!(out.status.code(), Some(3));
}

/// Slice 4C: cross-file struct literal binding a private field is E0403.
#[test]
fn cross_file_struct_literal_private_field_emits_e0403() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/geom.cplus"),
        "pub struct Point { pub x: i32, y: i32 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nfn main() -> i32 { let p = g::Point { x: 1, y: 2 }; return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected E0403 from private-field bind"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0403"),
        "expected E0403 in stderr, got: {stderr}"
    );
}

/// Slice 4C: same-file private field access is unaffected.
#[test]
fn same_file_private_field_access_works() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"sf2\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        // No `pub` anywhere — same-file references read/construct freely.
        "struct Point { x: i32, y: i32 }\nfn main() -> i32 { let p = Point { x: 5, y: 7 }; return p.x; }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "build failed: {status}");
    let bin = dir.join("target/debug/sf2");
    let out = Command::new(&bin).output().expect("run");
    assert_eq!(out.status.code(), Some(5));
}

/// Slice 4B: same-file references ignore `pub`, including unmarked
/// items. Sanity: a project that uses private items only inside their
/// declaring file builds cleanly.
#[test]
fn same_file_private_access_builds() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"sf\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn helper(n: i32) -> i32 { return n + 1; }\nfn main() -> i32 { return helper(41); }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "build failed: {status}");
    let bin = dir.join("target/debug/sf");
    let out = Command::new(&bin).output().expect("run binary");
    // helper(41) → 42; main returns it as the exit code.
    assert_eq!(out.status.code(), Some(42));
}

/// Phase 4 exit criterion: a project split across 5+ `.cplus` files
/// with a `Cplus.toml` manifest builds. `calc` exercises `pub`-gated
/// cross-file functions, a cross-file `pub enum`, cross-file variant
/// patterns in a `match`, and `import "..." as N` for both type and
/// function references.
#[test]
fn calc_5file_project_builds_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    // Mirror the in-tree calc project verbatim into the tempdir so the
    // build is fully self-contained (and we don't write to the source
    // tree from a test).
    let proj_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs/examples/projects/calc");
    let manifest = std::fs::read_to_string(proj_root.join("Cplus.toml")).unwrap();
    std::fs::write(dir.join("Cplus.toml"), manifest).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for f in [
        "main.cplus",
        "eval.cplus",
        "util.cplus",
        "expr.cplus",
        "ops.cplus",
    ] {
        let src = std::fs::read_to_string(proj_root.join("src").join(f)).unwrap();
        std::fs::write(dir.join("src").join(f), src).unwrap();
    }

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");

    let bin = dir.join("target/debug/calc");
    assert!(bin.is_file(), "expected binary at {}", bin.display());
    let out = Command::new(&bin).output().expect("run binary");
    assert!(
        out.status.success(),
        "binary exited non-zero: {}",
        out.status
    );
    // (3 + 4) * (-2) = -14.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "-14\n");
}

/// Slice 4C-tail: resolver/manifest diagnostics flow through
/// `--diagnostics=json` and emit a single NDJSON line with the expected
/// shape (code, severity, primary.file).
#[test]
fn e0401_json_shape() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./missing\" as m;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--diagnostics=json")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .next()
        .expect("expected at least one diagnostic line");
    let v: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stderr line not valid JSON: {e}\nline: {line}"));
    assert_eq!(v["severity"], "error");
    assert_eq!(v["code"], "E0401");
    let primary_file = v["primary"]["file"].as_str().expect("primary.file");
    assert!(
        primary_file.ends_with("main.cplus"),
        "primary file should be the importing file, got: {primary_file}"
    );
}

/// Slice 4C-tail: did-you-mean suggestion for E0401 picks the closest
/// existing `.cplus` filename within edit distance ≤ 2.
#[test]
fn e0401_did_you_mean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // "math.cplus" exists; the typo "maths.cplus" is one edit away.
    std::fs::write(
        dir.join("src/math.cplus"),
        "pub fn square(n: i32) -> i32 { return n * n; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./maths\" as m;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--diagnostics=json")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().next().unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let sugg = v["suggestions"][0]["replacement"].as_str();
    assert!(
        sugg.map(|s| s.contains("math.cplus")).unwrap_or(false),
        "expected suggestion to reference math.cplus, got: {sugg:?}"
    );
}

/// Slice 4C-tail: manifest errors render as structured diagnostics too.
#[test]
fn malformed_manifest_emits_e0406_json() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[[[ not valid toml").unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--diagnostics=json")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().next().unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["code"], "E0406");
    assert_eq!(v["severity"], "error");
}

/// An `import` pointing at a non-existent file should fail with E0401.
#[test]
fn import_not_found_emits_e0401() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./nope\" as nope;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0401"), "expected E0401, got: {stderr}");
}

/// A cyclic import chain should be rejected with E0404.
#[test]
fn cyclic_imports_emit_e0404() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/a.cplus"),
        "import \"./b\" as b;\nfn from_a() -> i32 { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/b.cplus"),
        "import \"./a\" as a;\nfn from_b() -> i32 { return 2; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./a\" as a;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0404"), "expected E0404, got: {stderr}");
}

// ---- Phase 4 slice 4D: `cpc fmt` ----

/// Stdin → stdout: an ugly input should come out canonical.
#[test]
fn fmt_stdin_normalizes() {
    use std::io::Write;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let mut child = Command::new(cpc)
        .arg("fmt")
        .arg("--stdin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn cpc fmt --stdin");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"fn  f( x:i32 )->i32{return x+1;}\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "fn f(x: i32) -> i32 { return x + 1; }\n"
    );
}

/// `cpc fmt --check PATH/` over the in-tree samples must succeed with
/// no diff. This is the load-bearing test: the samples are the
/// formatter's de facto spec.
#[test]
fn fmt_check_all_samples_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs/examples");
    let out = Command::new(cpc)
        .arg("fmt")
        .arg("--check")
        .arg(&root)
        .output()
        .expect("invoke cpc fmt --check");
    assert!(
        out.status.success(),
        "cpc fmt --check found drift in samples:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

/// `cpc fmt --check` on a known-unformatted file exits non-zero and
/// prints a diff to stderr.
#[test]
fn fmt_check_reports_diff() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let f = dir.join("bad.cplus");
    std::fs::write(&f, "fn  main()->i32{return 0;}\n").unwrap();
    let out = Command::new(cpc)
        .arg("fmt")
        .arg("--check")
        .arg(&f)
        .output()
        .expect("invoke cpc fmt --check");
    assert!(
        !out.status.success(),
        "expected non-zero exit on dirty file"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad.cplus"),
        "expected file path in diff header, got: {stderr}"
    );
    assert!(
        stderr.contains("-fn"),
        "expected `-` lines in diff, got: {stderr}"
    );
    assert!(
        stderr.contains("+fn"),
        "expected `+` lines in diff, got: {stderr}"
    );
}

/// Default mode rewrites in place.
#[test]
fn fmt_rewrites_in_place() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let f = dir.join("ugly.cplus");
    std::fs::write(&f, "fn  main()->i32{return 0;}\n").unwrap();
    let status = Command::new(cpc)
        .arg("fmt")
        .arg(&f)
        .status()
        .expect("invoke cpc fmt");
    assert!(status.success());
    let after = std::fs::read_to_string(&f).unwrap();
    assert_eq!(after, "fn main() -> i32 { return 0; }\n");
}

/// `--emit` prints to stdout and leaves the source file unchanged.
#[test]
fn fmt_emit_leaves_file_alone() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let f = dir.join("ugly.cplus");
    let orig = "fn  main()->i32{return 0;}\n";
    std::fs::write(&f, orig).unwrap();
    let out = Command::new(cpc)
        .arg("fmt")
        .arg("--emit")
        .arg(&f)
        .output()
        .expect("invoke cpc fmt --emit");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "fn main() -> i32 { return 0; }\n"
    );
    // File on disk untouched.
    let after = std::fs::read_to_string(&f).unwrap();
    assert_eq!(after, orig);
}

/// `cpc fmt` is idempotent end-to-end: format, then format again, equal.
#[test]
fn fmt_idempotent_in_place() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let f = dir.join("u.cplus");
    std::fs::write(&f, "fn  main()->i32{let x:i32=1+2;return x;}\n").unwrap();
    let once = Command::new(cpc)
        .arg("fmt")
        .arg(&f)
        .status()
        .expect("invoke");
    assert!(once.success());
    let first = std::fs::read_to_string(&f).unwrap();
    let twice = Command::new(cpc)
        .arg("fmt")
        .arg(&f)
        .status()
        .expect("invoke");
    assert!(twice.success());
    let second = std::fs::read_to_string(&f).unwrap();
    assert_eq!(first, second, "fmt(fmt(x)) must equal fmt(x)");
}

/// Phase 5 slice 5BC.codegen: `mut x: T` on a non-Copy struct must propagate
/// the callee's writes back to the caller's place — the §2.9 exclusive-borrow
/// ABI. The runtime regression: before this slice, codegen passed by value,
/// so `bump(x)` would observe x.v = 10 (not 11) even though the spec says
/// `mut t: Tag` is an exclusive borrow.
#[test]
fn mut_param_noncopy_struct_mutation_propagates() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "\
struct Tag { v: i32 }
impl Tag { fn drop(mut self) { return; } }
fn bump(mut t: Tag) {
    t.v = t.v + 1;
    return;
}
fn main() -> i32 {
    let mut x: Tag = Tag { v: 10 };
    bump(x);
    #println(x.v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(
        run.status.success(),
        "binary exited non-zero: {}",
        run.status
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "11\n");
}

/// Phase 5 slice 5BC.codegen: `mut p: P` on a Copy struct is local
/// mutability per §2.9, NOT an exclusive borrow. The callee's writes must
/// stay local — caller observes the original value. Negative complement of
/// the test above: documents the spec line that "mut on Copy" ≠ "borrow".
#[test]
fn mut_param_copy_struct_does_not_propagate() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "\
struct P { v: i32 }
fn bump(mut p: P) {
    p.v = p.v + 1;
    return;
}
fn main() -> i32 {
    let q: P = P { v: 10 };
    bump(q);
    #println(q.v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(run.status.success());
    // Copy semantics: caller's q.v is unchanged.
    assert_eq!(String::from_utf8_lossy(&run.stdout), "10\n");
}

/// Phase 5 slice 5BC.codegen: a non-Copy `mut x: T` parameter must produce
/// exactly one `drop` call (in the caller's scope), not two. Regression
/// guard: if codegen ever re-registers the param for drop in the callee,
/// this test catches the double-free at runtime by counting drop emissions
/// through observable side effects.
#[test]
fn mut_param_noncopy_struct_no_double_drop_at_runtime() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    // The drop body prints -id; one Tracker means one drop must print "-7"
    // exactly once. If the callee double-dropped we'd see "-7" twice.
    std::fs::write(
        &src,
        "\
struct Tracker { id: i32 }
impl Tracker {
    fn drop(mut self) {
        #println(0 -% self.id);
        return;
    }
}
fn bump(mut t: Tracker) {
    t.id = t.id + 1;
    return;
}
fn main() -> i32 {
    let mut x: Tracker = Tracker { id: 6 };
    bump(x);
    #println(x.id);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(run.status.success());
    // Expected: 7 (bumped value) then -7 (single drop). One drop only.
    assert_eq!(String::from_utf8_lossy(&run.stdout), "7\n-7\n");
}

/// Phase 5 slice 5ATTR.1 — attribute parser + validator wired into the
/// driver pipeline. A misspelled attribute fires E0354 with a did-you-mean
/// suggestion before sema runs, so the user sees the attribute error
/// rather than a downstream complaint about an unknown name.
#[test]
fn unknown_attribute_rejected_e0354() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "#[tset]\nfn f() { return; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for unknown attribute"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0354"), "expected E0354, got: {stderr}");
}

/// Phase 5 slice 5ATTR.1 — attribute on the wrong target fires E0356.
/// `#[test]` is only valid on free functions in Phase 5.
#[test]
fn test_attribute_on_struct_rejected_e0356() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "#[test]\nstruct P { v: i32 }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for misplaced #[test]"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0356"), "expected E0356, got: {stderr}");
}

/// Phase 5 slice 5ATTR.2 — sema rejects a `#[test]` function with the wrong
/// signature. The two accepted shapes are `fn()` and `fn() -> i32`; anything
/// else is E0358. Drives the full pipeline through `cpc build`.
#[test]
fn test_attribute_bad_signature_rejected_e0358() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "#[test] fn t(n: i32) { return; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for bad test signature"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0358"), "expected E0358, got: {stderr}");
}

/// Phase 5 slice 5ATTR.2 — sema rejects `pub` on a `#[test]` function. Tests
/// are project-internal helpers; exposing them as part of the API surface
/// breaks the runner's discovery contract.
#[test]
fn test_attribute_pub_rejected_e0359() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "#[test] pub fn t() { return; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for pub on #[test]"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0359"), "expected E0359, got: {stderr}");
}

/// Phase 5 slice 5ATTR.3 — `assert` with a true condition lets the program
/// run to completion. Pins both the codegen (conditional branch + trap on
/// the false path; ok branch flows through) and the no-effect-at-runtime
/// behavior when the assertion holds.
#[test]
fn assert_true_runs_to_completion() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(&src, "fn main() -> i32 {\n  assert 1 == 1;\n  assert 2 + 2 == 4;\n  #println(42);\n  return 0;\n}\n").unwrap();
    let bin = dir.join("ok");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "expected clean compile, stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(
        run.status.success(),
        "binary exited non-zero: {}",
        run.status
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

/// Phase 5 slice 5ATTR.3 — `assert` with a false condition traps at runtime.
/// On Darwin the trap surfaces as SIGILL; on Linux it's SIGABRT. Either way
/// the exit status is non-zero and the program never reaches code after
/// the assertion. Phase-5 behavior; slice 5ATTR.4 replaces the trap with a
/// per-test failure-flag write inside test-driver builds.
#[test]
fn assert_false_traps_at_runtime() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n  assert 1 == 2;\n  #println(999);\n  return 0;\n}\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "expected clean compile, stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(
        !run.status.success(),
        "expected non-zero exit on trap, got: {}",
        run.status
    );
    // The `#println(999)` after the failing assertion must not have run.
    assert!(
        !String::from_utf8_lossy(&run.stdout).contains("999"),
        "code after failing assert ran: {:?}",
        run.stdout
    );
}

/// Phase 5 slice 5ATTR.3 — `assert` with a non-bool expression is rejected
/// at sema (E0302), same code as every other "wrong type for this position"
/// case.
#[test]
fn assert_non_bool_rejected_e0302() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "fn main() -> i32 { assert 42; return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected sema rejection of non-bool assert"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0302"), "expected E0302, got: {stderr}");
}

/// Phase 5 slice 5ATTR.1 — `#[test]` parses, validates, and a program
/// carrying it still compiles to a binary (no consumer yet — that's slice
/// 5ATTR.2 / 5ATTR.4). For now the attribute is data on the AST that doesn't
/// alter codegen, so the test function is emitted like any other.
#[test]
fn test_attribute_clean_compile() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn t1() { return; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected clean compile, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run produced binary");
    assert!(
        run.status.success(),
        "binary exited non-zero: {}",
        run.status
    );
}

// ---- Phase 5 slice 5ATTR.4 — `cpc test` subcommand ----

#[test]
fn cpc_test_runs_passing_tests() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn passes() { assert 1 + 1 == 2; }\n\
         #[test]\nfn also_passes() { assert true; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected all-pass, stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("test passes ... ok"));
    assert!(stdout.contains("test also_passes ... ok"));
    assert!(stdout.contains("2 passed; 0 failed"));
}

#[test]
fn cpc_test_reports_failing_test() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn passes() { assert true; }\n\
         #[test]\nfn fails() { assert false; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected non-zero exit on failing test"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("test passes ... ok"));
    assert!(stdout.contains("test fails ... FAILED"));
    assert!(stdout.contains("1 passed; 1 failed"));
}

#[test]
fn cpc_test_json_output() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn ok1() { assert 1 == 1; }\n\
         #[test]\nfn bad() { assert 1 == 2; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .arg("--json")
        .output()
        .expect("invoke cpc");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "expected 3 lines (2 tests + 1 summary): {stdout}"
    );
    // Each line must be valid JSON.
    let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("line 0 JSON");
    let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("line 1 JSON");
    let v2: serde_json::Value = serde_json::from_str(lines[2]).expect("line 2 JSON");
    assert_eq!(v0["name"], "ok1");
    assert_eq!(v0["result"], "pass");
    assert_eq!(v1["name"], "bad");
    assert_eq!(v1["result"], "fail");
    assert_eq!(v2["passed"], 1);
    assert_eq!(v2["failed"], 1);
}

#[test]
fn cpc_test_no_tests_zero_exit() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success(), "no tests should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 passed; 0 failed"),
        "got stdout: {stdout}"
    );
}

#[test]
fn cpc_test_i32_return_form() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn zero_ok() -> i32 { return 0; }\n\
         #[test]\nfn nonzero_fails() -> i32 { return 7; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failing exit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("test zero_ok ... ok"));
    assert!(stdout.contains("test nonzero_fails ... FAILED"));
}

#[test]
fn cpc_test_calls_helper_functions() {
    // Ensures helpers (non-test fns) are still emitted and callable from tests.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "fn double(n: i32) -> i32 { return n + n; }\n\
         #[test]\nfn doubles_correctly() { assert double(3) == 6; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected pass, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cpc_test_skips_user_main() {
    // A `fn main` in the source must be skipped (the test driver replaces
    // it). If the project's `main` were still emitted, LLVM would error on
    // duplicate `@main` symbols.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { return 42; }\n\
         #[test]\nfn t() { assert true; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected pass, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The driver should return the failed-count (0), not the user's 42.
    assert_eq!(out.status.code(), Some(0));
}

// ---- Phase 6 slice 6BC.1 — intra-call exclusive-borrow conflicts ----

#[test]
fn e0380_two_mut_borrows_of_same_binding_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn modify_both(mut a: B, mut b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    modify_both(y, y);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for two mut borrows"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0380"), "expected E0380, got: {stderr}");
}

#[test]
fn e0381_mut_and_shared_borrow_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn write_thing(mut a: B, n: i32) { return; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    write_thing(y, peek(y));
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for mut+shared"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0381"), "expected E0381, got: {stderr}");
}

#[test]
fn e0382_mut_and_move_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn write_and_take(mut a: B, move b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    write_and_take(y, y);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for mut+move"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0382"), "expected E0382, got: {stderr}");
    // E0370 must NOT fire for the same pair — E0382 is the more specific
    // diagnostic and suppresses cascading errors.
    assert!(
        !stderr.contains("E0370"),
        "E0370 should be suppressed for mut+move pair, got: {stderr}"
    );
}

#[test]
fn mut_borrows_of_different_bindings_accepted() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn modify_both(mut a: B, mut b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    let z: B = B { x: 2 };
    modify_both(y, z);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "two mut borrows of distinct places should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn mut_borrows_of_copy_type_accepted() {
    // `mut x: i32` is local-mutability on Copy types, not a borrow. Two
    // such args should compile without E0380 / E0381.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
fn modify_both(mut a: i32, mut b: i32) { return; }
fn main() -> i32 {
    let y: i32 = 1;
    modify_both(y, y);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Copy mut args should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 6 exit criterion — iterator invalidation rejected ----

#[test]
fn phase6_exit_iterator_invalidation_rejected() {
    // The Phase-6 exit demo: a VecI32 with a `cursor` (shared borrow
    // of self) and a `push` (mut self / exclusive borrow). Calling
    // push while a cursor is alive must be a compile-time error.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("vec_invalid.cplus");
    std::fs::write(
        &src,
        "\
struct VecI32 { data: [i32; 8], len: usize }
impl VecI32 {
    fn drop(mut self) { return; }
    fn cursor(self) -> VecI32 { return self; }
    fn push(mut self, x: i32) { return; }
}
fn main() -> i32 {
    let mut v: VecI32 = VecI32 { data: [0, 0, 0, 0, 0, 0, 0, 0], len: 0 };
    let cur: VecI32 = v.cursor();
    v.push(42);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bin");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "Phase-6 exit: iterator invalidation must reject; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0381"),
        "expected E0381 on iterator-invalidation; got: {stderr}"
    );
}

#[test]
fn phase6_exit_sequential_pushes_accepted() {
    // Positive: pushes without an outstanding cursor compile fine.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("vec_ok.cplus");
    std::fs::write(
        &src,
        "\
struct VecI32 { data: [i32; 8], len: usize }
impl VecI32 {
    fn drop(mut self) { return; }
    fn push(mut self, x: i32) { return; }
}
fn main() -> i32 {
    let mut v: VecI32 = VecI32 { data: [0, 0, 0, 0, 0, 0, 0, 0], len: 0 };
    v.push(1);
    v.push(2);
    v.push(3);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bin");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "sequential pushes should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 6 slice 6BC.opt — static drop-flag specialization ----

#[test]
fn never_moved_drop_binding_elides_flag() {
    // A let-bound Drop binding that's never moved should emit an
    // unconditional drop call at scope exit — no flag alloca, no
    // flag store, no flag load, no conditional branch.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn main() -> i32 {
    let x: B = B { x: 7 };
    return x.x;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        !ir.contains("%x.drop_flag"),
        "drop flag should be elided when binding is never moved; got: {ir}"
    );
    // Direct unconditional drop call must still appear. Slice 1F changed
    // the call to use `preserve_nonecc` to match the cold-path CC on the
    // drop method's `define` line.
    assert!(
        ir.contains("call preserve_nonecc void @B.drop(ptr %x"),
        "expected unconditional drop call (preserve_nonecc); got: {ir}"
    );
}

#[test]
fn moved_drop_binding_keeps_runtime_flag() {
    // When a binding IS moved somewhere in the function, the
    // runtime flag mechanism stays — flag alloca, init store,
    // flip-on-move store, load-and-branch at scope exit.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn consume(move b: B) { return; }
fn main() -> i32 {
    let x: B = B { x: 7 };
    consume(x);
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    // v0.0.14: drop-flag names carry a uniquifying suffix (`%x.drop_flagN`), so
    // match the prefix rather than an exact `= alloca i1`.
    assert!(
        ir.contains("%x.drop_flag"),
        "drop flag should remain for moved binding; got: {ir}"
    );
    assert!(
        ir.contains("alloca i1"),
        "drop flag is an i1 alloca; got: {ir}"
    );
    assert!(
        ir.contains("load i1, ptr %x.drop_flag"),
        "flag load should remain at scope exit; got: {ir}"
    );
}

#[test]
fn never_moved_drop_runtime_behavior_unchanged() {
    // The Phase-3 drop_basic sample expects output `1\n2\n-2\n-1\n`.
    // Confirm that 6BC.opt's optimization doesn't change the runtime
    // behavior: the drop calls still fire in the right order.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_path = dir.join("drop_basic.cplus");
    let sample = format!(
        "{}/../docs/examples/drop_basic.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::copy(&sample, &src_path).expect("copy sample");
    let bin = dir.join("drop_basic");
    let compile = Command::new(cpc)
        .arg(&src_path)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        stdout, "1\n2\n-2\n-1\n",
        "drop_basic output changed after 6BC.opt optimization; got: {stdout:?}"
    );
}

// ---- Phase 6 slice 6BC.codegen — noalias / readonly param attributes ----

#[test]
fn mut_param_tagged_noalias_in_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn bump(mut b: B) -> i32 { b.x = b.x + 1; return b.x; }
fn main() -> i32 {
    let mut v: B = B { x: 1 };
    return bump(v);
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected clean emit; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("i32 @bump(ptr noalias "),
        "expected `mut b: B` to lower to `ptr noalias`; got: {ir}"
    );
}

#[test]
fn shared_param_tagged_readonly_in_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 7 };
    return peek(v);
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected clean emit; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("i32 @peek(ptr readonly "),
        "expected shared borrow `borrow b: B` to lower to `ptr readonly`; got: {ir}"
    );
    // And NOT `noalias` — shared borrows can alias per §2.9.
    assert!(
        !ir.contains("@peek(ptr noalias"),
        "shared borrow must not get `noalias`; got: {ir}"
    );
}

#[test]
fn bare_noncopy_param_move_forwarded_no_double_free() {
    // v0.0.12 regression: a bare `x: T` non-Copy param that is forwarded back
    // out (`fn forward(x: T) -> T { return x; }`) used to lower as a shared
    // borrow — the caller dropped its binding unconditionally AND the returned
    // value's new owner dropped it, double-freeing the same heap allocation.
    // macOS libmalloc aborts on the second free, so a regression makes the
    // program exit non-zero. The fix moves the value (caller drop-flag flip +
    // callee-owned drop), so it frees exactly once and exits 0.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Owned { ptr: *u8 }
impl Owned {
    fn make() -> Owned { return Owned { ptr: unsafe { malloc(16 as usize) } }; }
    fn drop(mut self) { unsafe { free(self.ptr); } return; }
}
fn forward(x: Owned) -> Owned { return x; }
fn main() -> i32 {
    let b: Owned = Owned::make();
    let c: Owned = forward(b);
    return 0;
}
",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "expected clean compile; stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert!(
        run.status.success(),
        "forwarded move double-freed (non-zero exit); stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn partial_move_out_of_drop_type_rejected_e0509() {
    // v0.0.12 fix (E0509): moving a non-Copy field out of a value whose type
    // implements `drop` is rejected. The owning destructor frees its fields by
    // hand (docs/design/phase3-drop.md §5), so stealing a field would
    // double-free it. Both the `let`-binding and `return` move positions are
    // guarded.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Owned { ptr: *u8 }
impl Owned {
    fn make() -> Owned { return Owned { ptr: unsafe { malloc(16 as usize) } }; }
    fn drop(mut self) { unsafe { free(self.ptr); } return; }
}
struct Pair { a: Owned, b: Owned }
impl Pair {
    fn drop(mut self) { unsafe { free(self.a.ptr); } unsafe { free(self.b.ptr); } return; }
}
fn main() -> i32 {
    let p: Pair = Pair { a: Owned::make(), b: Owned::make() };
    let q: Owned = p.a;
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected E0509 rejection, but compile succeeded"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0509"),
        "expected E0509, got: {stderr}"
    );
}

#[test]
fn field_move_out_of_auto_drop_aggregate_rejected_e0509() {
    // v0.0.14 auto field-drop: a struct holding `Drop` fields is now itself
    // drop-carrying, so moving a field out of it is E0509 — otherwise the
    // struct's synthesized field-drop would free the moved-out field a second
    // time at scope exit. (Pre-v0.0.14 this compiled, because structs did not
    // auto-drop their fields.)
    let (ok, stderr) = try_compile_snippet(
        "\
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Owned { ptr: *u8 }
impl Owned {
    fn make() -> Owned { return Owned { ptr: unsafe { malloc(16 as usize) } }; }
    fn drop(mut self) { unsafe { free(self.ptr); } return; }
}
struct Pair { a: Owned, b: Owned }
fn main() -> i32 {
    let p: Pair = Pair { a: Owned::make(), b: Owned::make() };
    let q: Owned = p.a;
    return 0;
}
",
    );
    assert!(
        !ok,
        "moving a field out of an auto-drop aggregate must be rejected"
    );
    assert!(stderr.contains("E0509"), "expected E0509, got: {stderr}");
}

#[test]
fn field_extract_from_copy_aggregate_allowed() {
    // A struct whose fields are all Copy is not drop-carrying, so pulling a
    // field out is a copy (not a move) and stays legal.
    let (ok, stderr) = try_compile_snippet(
        "\
struct Point { x: i32, y: i32 }
fn main() -> i32 {
    let p: Point = Point { x: 3, y: 4 };
    let q: i32 = p.x;
    return q -% 3;
}
",
    );
    assert!(
        ok,
        "field extract from a Copy aggregate must compile; stderr: {stderr}"
    );
}

#[test]
fn enum_multi_payload_large_first_value_layout() {
    // v0.0.14: a tagged-enum variant whose first payload exceeds 8 bytes (a
    // `string`) must place the second payload *after* it, not overlapping. The
    // old slot-index GEP read the second value from inside the first's bytes.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
struct P { x: i32 }
enum R { Both(string, P), None }
fn mk() -> R { return R::Both(\"hello\".to_string(), P { x: 9 }); }
fn main() -> i32 {
    let r: R = mk();
    let out: i32 = match r {
        R::Both(s, p) => { let kept: string = s; kept.len() as i32 +% p.x }
        R::None => { 0 }
    };
    return out -% 14;
}
",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "multi-payload enum must compile");
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(
        run.status.code(),
        Some(0),
        "second payload must read at its real offset, no double-free"
    );
}

#[test]
fn auto_field_drop_no_double_free_runtime() {
    // v0.0.14 auto field-drop, end to end: `Holder` has no `drop` but owns a
    // `Res` (which does). Moving a Holder into `consume` must run Res::drop
    // exactly once per iteration. A double-free would abort the process; 100
    // iterations exiting 0 proves the field destructor runs once, no more.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Res { p: *u8 }
impl Res {
    fn make() -> Res { return Res { p: unsafe { malloc(16 as usize) } }; }
    fn drop(mut self) { unsafe { free(self.p); } return; }
}
struct Holder { r: Res }
fn consume(move h: Holder) -> i32 { return 0; }
fn main() -> i32 {
    let mut i: i32 = 0;
    let mut acc: i32 = 0;
    while i < 100 {
        let h: Holder = Holder { r: Res::make() };
        acc = acc +% consume(h);
        i = i +% 1;
    }
    return acc;
}
",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "auto field-drop program must compile; stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "auto field-drop must not double-free (status {:?})",
        run.status
    );
}

/// Helper: compile a snippet with `--emit-ll`, return (success, stderr).
fn try_compile_snippet(src_text: &str) -> (bool, String) {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, src_text).unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn return_region_undeclared_rejected_e0511() {
    // v0.0.12 (#2): a return type naming a borrow region (`-> borrow Z str`)
    // must tie that region to a parameter. An undeclared region is inert —
    // reject it rather than silently accept (was the deferred "future polish").
    let (ok, stderr) = try_compile_snippet(
        "fn f(a: borrow A str) -> borrow Z str { return a; }\n\
         fn main() -> i32 { return #str_len(f(\"x\")) as i32; }\n",
    );
    assert!(!ok, "expected E0511 rejection, compiled instead");
    assert!(stderr.contains("E0511"), "expected E0511, got: {stderr}");
}

#[test]
fn return_region_mismatch_rejected_e0512() {
    // v0.0.12 (#2): returning a borrow from a different region than the
    // signature declares is rejected — regions are now meaningful.
    let (ok, stderr) = try_compile_snippet(
        "fn weird(a: borrow A str, b: borrow B str) -> borrow A str { return b; }\n\
         fn main() -> i32 { return #str_len(weird(\"x\", \"y\")) as i32; }\n",
    );
    assert!(!ok, "expected E0512 rejection, compiled instead");
    assert!(stderr.contains("E0512"), "expected E0512, got: {stderr}");
}

#[test]
fn return_region_matching_compiles() {
    // v0.0.12 (#2) positive: a region-annotated return that borrows a
    // same-region parameter is valid and must keep compiling.
    let (ok, stderr) = try_compile_snippet(
        "fn pick(a: borrow A str, b: borrow A str) -> borrow A str {\n\
             if #str_len(a) > #str_len(b) { return a; }\n\
             return b;\n\
         }\n\
         fn main() -> i32 { return #str_len(pick(\"hello\", \"worldlong\")) as i32; }\n",
    );
    assert!(ok, "valid same-region return must compile; stderr: {stderr}");
}

#[test]
fn return_borrow_of_local_owned_rejected_e0513() {
    // v0.0.12 (#3): returning a `str` view into a function-local owned value
    // (a `string`, which drops at function exit) dangles — reject it.
    let (ok, stderr) = try_compile_snippet(
        "fn bad() -> str {\n\
             let s: string = \"heap\".to_string();\n\
             return s.as_str();\n\
         }\n\
         fn main() -> i32 { return #str_len(bad()) as i32; }\n",
    );
    assert!(!ok, "expected E0513 rejection, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn return_borrow_alias_of_local_owned_rejected_e0513() {
    // Returning an alias to `s.as_str()` is the same dangling view as
    // returning `s.as_str()` directly.
    let (ok, stderr) = try_compile_snippet(
        "fn bad() -> str {\n\
             let s: string = \"heap\".to_string();\n\
             let view: str = s.as_str();\n\
             return view;\n\
         }\n\
         fn main() -> i32 { return #str_len(bad()) as i32; }\n",
    );
    assert!(!ok, "expected E0513 rejection, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn return_borrow_branch_alias_of_local_owned_rejected_e0513() {
    // Flow merging must keep the unsafe branch provenance even when another
    // branch assigns a literal-backed view.
    let (ok, stderr) = try_compile_snippet(
        "fn bad(flag: bool) -> str {\n\
             let s: string = \"heap\".to_string();\n\
             let mut view: str;\n\
             if flag { view = s.as_str(); } else { view = \"static\"; }\n\
             return view;\n\
         }\n\
         fn main() -> i32 { return #str_len(bad(true)) as i32; }\n",
    );
    assert!(!ok, "expected E0513 rejection, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn return_literal_str_view_compiles() {
    // v0.0.12 (#3) positive: a `str` bound to a string literal is `'static`;
    // returning it is sound and must keep compiling (no false positive).
    let (ok, stderr) = try_compile_snippet(
        "fn ok() -> str { let s: str = \"literal\"; return s; }\n\
         fn main() -> i32 { return #str_len(ok()) as i32; }\n",
    );
    assert!(ok, "returning a literal-backed str must compile; stderr: {stderr}");
}

#[test]
fn return_slice_of_param_compiles() {
    // v0.0.12 (#3) positive: returning a view borrowed from a parameter is
    // caller-tied and sound — must not be flagged as a dangling local.
    let (ok, stderr) = try_compile_snippet(
        "fn first(borrow s: str) -> str { return s; }\n\
         fn main() -> i32 { return #str_len(first(\"x\")) as i32; }\n",
    );
    assert!(ok, "returning a borrow of a parameter must compile; stderr: {stderr}");
}

#[test]
fn escaping_view_in_returned_struct_rejected_e0513() {
    // v0.0.13 (Tier 1): the dangle hidden inside a returned aggregate. The
    // view borrows local `s`, which drops at return — so the struct carries a
    // dangling view. E0513 even though the return *type* is a struct, not a view.
    let (ok, stderr) = try_compile_snippet(
        "struct Holder { view: str }\n\
         fn keep() -> Holder {\n\
             let s: string = \"heap\".to_string();\n\
             return Holder { view: s.as_str() };\n\
         }\n\
         fn main() -> i32 { let h: Holder = keep(); return 0; }\n",
    );
    assert!(!ok, "expected E0513 on the escaping view, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn move_owned_field_into_returned_struct_compiles() {
    // v0.0.13 (Tier 1) negative-guard: moving an *owned* `string` into a
    // returned struct is a normal ownership transfer — must NOT be flagged.
    let (ok, stderr) = try_compile_snippet(
        "struct Owner { s: string }\n\
         fn mk() -> Owner {\n\
             let s: string = \"heap\".to_string();\n\
             return Owner { s: s };\n\
         }\n\
         fn main() -> i32 { let o: Owner = mk(); return 0; }\n",
    );
    assert!(ok, "moving an owned string into a returned struct must compile; stderr: {stderr}");
}

#[test]
fn param_rooted_view_in_returned_struct_compiles() {
    // v0.0.13 (Tier 1) negative-guard: a view borrowed from a *parameter* is
    // caller-tied (the source outlives the call), so storing it in a returned
    // struct is sound — must not be flagged as a dangling local.
    let (ok, stderr) = try_compile_snippet(
        "struct Holder { view: str }\n\
         fn wrap(borrow s: string) -> Holder { return Holder { view: s.as_str() }; }\n\
         fn main() -> i32 { return 0; }\n",
    );
    assert!(ok, "param-rooted view in a returned struct must compile; stderr: {stderr}");
}

#[test]
fn let_str_eq_if_expression_compiles_and_runs() {
    // v0.0.12 regression: `let v: str = if cond { "a" } else { "b" };` crashed
    // codegen ("let init produces a value") because `expr_value_ty` didn't
    // handle string literals, so the if-expr got no result slot. The struct
    // case was already fixed; `str` / fat-pointer arms were the residual.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
fn pick(c: bool) -> str {
    let v: str = if c { \"aaa\" } else { \"bb\" };
    return v;
}
fn main() -> i32 { return #str_len(pick(true)) as i32; }
",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "str-typed let-if must compile, not panic; stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(3), "expected #str_len(\"aaa\") == 3");
}

#[test]
fn musttail_wrong_return_type_and_nested_expr_compile() {
    // bench-cplus handoff #3 regression: the tail-call detector used to
    // over-mark `return CALL(...)` shapes as `musttail`, so `return
    // dot(d,n) > 0.0f32;` (return type differs from the callee) and
    // `return sub(v, scale(...))` (callee result feeds another call, not
    // a tail position) tripped LLVM's musttail verifier. Both must now
    // emit a plain `call` and compile clean. (The detector only marks a
    // literal `return CALL(args);` whose return type matches the callee.)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
struct V { x: f32, y: f32, z: f32 }
fn v_make(x: f32, y: f32, z: f32) -> V { return V { x: x, y: y, z: z }; }
fn dot(a: V, b: V) -> f32 { return a.x * b.x + a.y * b.y + a.z * b.z; }
fn scale(v: V, s: f32) -> V { return v_make(v.x * s, v.y * s, v.z * s); }
fn sub(a: V, b: V) -> V { return v_make(a.x - b.x, a.y - b.y, a.z - b.z); }
fn check(d: V, n: V) -> bool { return dot(d, n) > 0.0f32; }
fn reflect(v: V, n: V) -> V { return sub(v, scale(n, 2.0f32 * dot(v, n))); }
fn main() -> i32 {
    let a: V = v_make(1.0f32, 2.0f32, 3.0f32);
    let b: V = v_make(4.0f32, 5.0f32, 6.0f32);
    let r: V = reflect(a, b);
    if check(a, b) {
        if r.x < 0.0f32 { return 0; }
    }
    return 1;
}
",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg("--release")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "musttail-shaped returns must compile (no verifier reject); stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    // dot(a,b)=32>0 so check is true; reflect = a - 64*b, r.x = -255 < 0.
    assert_eq!(run.status.code(), Some(0), "expected the happy-path exit 0");
}

#[test]
fn let_struct_eq_if_else_with_block_arm_compiles() {
    // bench-cplus handoff #4 regression: `let R: STRUCT = if c { call } else
    // { lets...; tail }` used to panic codegen for the struct-valued case.
    // This is the handoff's exact repro (a struct result, an else arm that
    // binds locals before its tail expression).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
struct V { x: f32, y: f32 }
fn v_make(x: f32, y: f32) -> V { return V { x: x, y: y }; }
fn main() -> i32 {
    let cond: bool = true;
    let dir: V = v_make(1.0f32, 2.0f32);
    let result: V = if cond {
        v_make(3.0f32, 4.0f32)
    } else {
        let r_perp: V = dir;
        let mut k: f32 = 1.0f32 - r_perp.x;
        if k < 0.0f32 { k = 0.0f32; }
        r_perp
    };
    return result.x as i32;
}
",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "struct-typed let-if-else must compile, not panic; stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(3), "expected result.x == 3.0 → 3");
}

#[test]
fn copy_struct_param_stays_by_value_no_attr() {
    // `mut p: Point` on a Copy struct is local-mutability, not a
    // borrow. Stays struct-by-value, no LLVM attribute.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct Point { x: i32, y: i32 }
fn shift(mut p: Point) -> i32 { p.x = p.x + 1; return p.x; }
fn main() -> i32 {
    let v: Point = Point { x: 1, y: 2 };
    return shift(v);
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("i32 @shift(%Point "),
        "Copy struct should stay struct-by-value; got: {ir}"
    );
}

// ---- Phase 6 slice 6BC.5 — explicit `borrow REGION T` syntax ----

#[test]
fn borrow_region_annotation_compiles_and_links() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > 0 { return a; }
    return b;
}
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = merge(a, b);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "annotated function should compile and link; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn borrow_region_annotation_establishes_multi_source_borrow() {
    // Verifies that the annotation flows through to call-site borrow
    // tracking: moving either source while the result is alive fires
    // E0372.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > 0 { return a; }
    return b;
}
fn drain(move b: B) { return; }
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = merge(a, b);
    drain(a);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move-while-multi-borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
}

#[test]
fn borrow_region_with_mut_marker_is_exclusive() {
    // `mut x: borrow A T` is an exclusive borrow in region A. The
    // return inherits the Exclusive flavor; reading the source
    // while the result is alive fires E0383.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn cursor(mut buf: borrow A B) -> borrow A B { return buf; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 1 };
    let cur: B = cursor(v);
    let n: i32 = peek(v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for read while exclusively borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn move_with_borrow_annotation_rejected_at_parse() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
fn take(move x: borrow A B) { return; }
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move+borrow"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Parser error — E0100 with text about region annotations.
    assert!(
        stderr.contains("E0100") || stderr.contains("borrow"),
        "expected parse error mentioning borrow, got: {stderr}"
    );
}

#[test]
fn explicit_annotation_fixes_e0384() {
    // The original E0384 case (Phase 6 slice 6BC.4) becomes
    // compilable once the user adds explicit annotations.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > 0 { return a; }
    return B { x: 0 };
}
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "explicit annotation should suppress E0384; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 6 slice 6BC.4 — Rule E3-mut + E0384 ----

#[test]
fn e3_mut_longest_pattern_compiles_cleanly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn longest_mut(mut a: B, mut b: B) -> B {
    if a.x > b.x { return a; }
    return b;
}
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = longest_mut(a, b);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "E3-mut should admit the longest-mut pattern; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn e3_mut_move_of_either_source_while_borrowed_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn longest_mut(mut a: B, mut b: B) -> B {
    if a.x > b.x { return a; }
    return b;
}
fn drain(move b: B) { return; }
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = longest_mut(a, b);
    drain(a);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move-while-multi-borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
    assert!(
        stderr.contains("exclusively borrowed"),
        "E0372 should report exclusive flavor under E3-mut; got: {stderr}"
    );
}

#[test]
fn e0384_mixed_rooting_requires_annotation() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: B, b: B) -> B {
    if a.x > 0 { return a; }
    return B { x: 0 };
}
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for ambiguous elision"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0384"), "expected E0384, got: {stderr}");
    assert!(
        stderr.contains("borrow REGION T"),
        "E0384 suggestion should reference `borrow REGION T`; got: {stderr}"
    );
}

#[test]
fn e0384_does_not_fire_on_fresh_value_returns() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn fresh(a: B, b: B) -> B { return B { x: 0 }; }
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "always-fresh returns should not trigger E0384; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 6 slice 6BC.3 — partial-place activation ----

#[test]
fn disjoint_subfield_borrows_accepted_in_one_call() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct Inner { v: i32 }
impl Inner { fn drop(mut self) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(mut self) { return; } }
fn modify_both(mut a: Inner, mut b: Inner) { return; }
fn main() -> i32 {
    let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
    modify_both(p.left, p.right);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "disjoint sub-places should admit; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn e0374_parent_and_subfield_in_one_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct Inner { v: i32 }
impl Inner { fn drop(mut self) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(mut self) { return; } }
fn write_pair(mut a: Pair, b: Inner) { return; }
fn main() -> i32 {
    let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
    write_pair(p, p.left);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for parent+sub-place"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Rejected either as a parent+subfield borrow conflict (E0374) or, with
    // v0.0.14's stricter value-passing, as a partial move of `p.left` out of a
    // Drop aggregate (E0337). Both are correct refusals of `write_pair(p, p.left)`.
    assert!(
        stderr.contains("E0374") || stderr.contains("E0337"),
        "expected E0374 or E0337, got: {stderr}"
    );
}

#[test]
fn e0374_cross_statement_subfield_borrow_blocks_parent_read() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct Inner { v: i32 }
impl Inner { fn drop(mut self) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(mut self) { return; } }
fn cursor(mut i: Inner) -> Inner { return i; }
fn peek_pair(borrow p: Pair) -> i32 { return 0; }
fn main() -> i32 {
    let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
    let cur: Inner = cursor(p.left);
    let n: i32 = peek_pair(p);
    return n;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for read of parent while sub-place borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0374"), "expected E0374, got: {stderr}");
}

#[test]
fn disjoint_subfield_cross_statement_accepted() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct Inner { v: i32 }
impl Inner { fn drop(mut self) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(mut self) { return; } }
fn cursor(mut i: Inner) -> Inner { return i; }
fn peek(borrow i: Inner) -> i32 { return i.v; }
fn main() -> i32 {
    let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
    let cur: Inner = cursor(p.left);
    let n: i32 = peek(p.right);
    return n;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "disjoint sub-places should admit cross-statement; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 6 slice 6BC.2 — cross-statement exclusive-borrow tracking ----

#[test]
fn e0383_read_of_exclusively_borrowed_place_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn cursor(mut b: B) -> B { return b; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 1 };
    let cur: B = cursor(v);
    let n: i32 = peek(v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for read of exclusively-borrowed place"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn e0383_does_not_fire_when_borrower_consumed_first() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn cursor(mut b: B) -> B { return b; }
fn drain(move c: B) { return; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 1 };
    let cur: B = cursor(v);
    drain(cur);
    let n: i32 = peek(v);
    return n;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "moving the exclusive borrower should release the borrow; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn e0372_message_refined_when_borrow_is_exclusive() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn cursor(mut b: B) -> B { return b; }
fn drain(move b: B) { return; }
fn main() -> i32 {
    let v: B = B { x: 1 };
    let cur: B = cursor(v);
    drain(v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for move while exclusively borrowed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
    assert!(
        stderr.contains("exclusively borrowed"),
        "E0372 should report 'exclusively borrowed' for the mut-borrow case; got: {stderr}"
    );
    // E0383 must NOT fire for the same conflict.
    assert!(
        !stderr.contains("E0383"),
        "E0383 should be suppressed for move-while-exclusive; got: {stderr}"
    );
}

#[test]
fn e2_mut_method_call_establishes_exclusive_borrow() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B {
    fn drop(mut self) { return; }
    fn cursor(mut self) -> B { return self; }
}
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let mut v: B = B { x: 1 };
    let cur: B = v.cursor();
    let n: i32 = peek(v);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure for read while mut-self method's return is alive"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn reading_the_exclusive_borrower_itself_accepted() {
    // Reading the borrower itself is fine — it owns the borrow.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn cursor(mut b: B) -> B { return b; }
fn peek(borrow b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 1 };
    let cur: B = cursor(v);
    let n: i32 = peek(cur);
    return n;
}
",
    )
    .unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "reading the borrower itself should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Phase 5 slice 5DOC — doctest extraction ----

#[test]
fn doctest_extracts_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// ```\n\
         /// assert 1 + 1 == 2;\n\
         /// ```\n\
         fn helper() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expected pass, stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("DOC_TEST::helper::0 ... ok"),
        "got: {stdout}"
    );
}

#[test]
fn doctest_failure_reports_doc_test_name() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// ```\n\
         /// assert false;\n\
         /// ```\n\
         fn bad() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failing exit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("DOC_TEST::bad::0 ... FAILED"),
        "got: {stdout}"
    );
}

#[test]
fn doctest_can_call_documented_item() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// ```\n\
         /// assert square(3) == 9;\n\
         /// ```\n\
         fn square(n: i32) -> i32 { return n * n; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn doctest_multiple_fences_get_distinct_names() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// ```\n\
         /// assert true;\n\
         /// ```\n\
         /// some prose\n\
         /// ```\n\
         /// assert 1 == 1;\n\
         /// ```\n\
         fn item() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DOC_TEST::item::0 ... ok"), "got: {stdout}");
    assert!(stdout.contains("DOC_TEST::item::1 ... ok"), "got: {stdout}");
}

#[test]
fn doctest_unchanged_for_source_without_fences() {
    // A `///` block with no fence is documentation — it should NOT
    // synthesize a test fn.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// Plain doc comment, no example.\n\
         fn f() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 passed; 0 failed"),
        "no tests should be discovered, got: {stdout}"
    );
}

#[test]
fn doctest_does_not_interfere_with_cpc_build() {
    // Building a file with `///` fences must succeed (synthesized
    // `#[test]` fns compile but aren't called by user's main).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "/// ```\n\
         /// assert true;\n\
         /// ```\n\
         fn helper() -> i32 { return 7; }\n\
         fn main() -> i32 { return helper(); }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "build with doctests failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "user's main should produce 7");
}

// ---- Phase 7 slice 7GEN.4: generics + interface validation ----

#[test]
fn phase7_generic_decls_and_impl_interface_clean() {
    // Parses + sema-checks a file exercising generic fns, generic types,
    // an interface decl, and an `impl Interface for Type` block with a
    // matching method signature. Pre-monomorphization (7GEN.5) the
    // generic items are codegen-skipped; the concrete `main` runs.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7.cplus");
    std::fs::write(
        &src,
        // Slice 7GEN.6: `Ord` is now blessed; the interface body in
        // this test uses a different name to avoid the collision.
        "interface Compare { fn compare(self, other: i32) -> i32; }\n\
         struct Pair[A, B] { first: A, second: B }\n\
         enum Maybe[T] { Some(T), None }\n\
         struct Point { x: i32, y: i32 }\n\
         impl Compare for Point { fn compare(self, other: i32) -> i32 { return 0; } }\n\
         fn identity[T](x: T) -> T { return x; }\n\
         fn main() -> i32 { return 7; }\n",
    )
    .unwrap();
    let bin = dir.join("p7");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "phase 7 syntax should sema-clean: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "main returns 7");
}

#[test]
fn phase7_impl_interface_missing_method_rejected_e0503() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7_miss.cplus");
    std::fs::write(
        &src,
        "interface Two { fn a(self) -> i32; fn b(self) -> i32; }\n\
         struct P { x: i32 }\n\
         impl Two for P { fn a(self) -> i32 { return 0; } }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "missing method should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0503"),
        "expected E0503 in stderr: {stderr}"
    );
}

#[test]
fn phase7_generic_fn_inferred_call_runs() {
    // Slice 7GEN.5a: monomorphization lands an `identity[T]` call that
    // sema infers (T = i32) and codegen emits as `identity__i32`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7gen5.cplus");
    std::fs::write(
        &src,
        "fn identity[T](x: T) -> T { return x; }\n\
         fn main() -> i32 {\n\
             let a: i32 = identity(7);\n\
             let b: i32 = identity(35);\n\
             return a + b;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7gen5");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic fn should build cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "identity(7) + identity(35) should yield 42"
    );
}

#[test]
fn phase7_generic_fn_distinct_instantiations_emit_distinct_symbols() {
    // Calling `id` with i32 and again with i64 should emit two
    // distinct monomorphizations in the IR: `id__i32` and `id__i64`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7gen5_distinct.cplus");
    std::fs::write(
        &src,
        "fn id[T](x: T) -> T { return x; }\n\
         fn main() -> i32 {\n\
             let a: i32 = id(7);\n\
             let b: i64 = id(99i64);\n\
             return a;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "build failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("@id__i32"), "missing id__i32 in IR: {ir}");
    assert!(ir.contains("@id__i64"), "missing id__i64 in IR: {ir}");
}

#[test]
fn phase7_turbofish_explicit_type_args_runs() {
    // Slice 7GEN.5b: `identity::[i32](7)` substitutes the explicit type
    // arg instead of inferring. End-to-end compile + run.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7tb.cplus");
    std::fs::write(
        &src,
        "fn identity[T](x: T) -> T { return x; }\n\
         fn main() -> i32 {\n\
             let a: i32 = identity::[i32](7);\n\
             let b: i32 = identity::[i32](35);\n\
             return a + b;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7tb");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "turbofish call should build cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "identity::[i32](7) + identity::[i32](35) should yield 42"
    );
}

#[test]
fn phase7_turbofish_arity_mismatch_rejected_e0501() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7tb_bad.cplus");
    std::fs::write(
        &src,
        "fn id[T](x: T) -> T { return x; }\n\
         fn main() -> i32 { let a: i32 = id::[i32, bool](7); return a; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "arity mismatch should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0501"),
        "expected E0501 in stderr: {stderr}"
    );
}

#[test]
fn phase7_generic_struct_instantiation_runs() {
    // Slice 7GEN.5c: a generic struct can be instantiated at type position
    // and in a struct literal. Distinct instantiations emit distinct
    // mangled structs and run end-to-end.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7c.cplus");
    std::fs::write(
        &src,
        "struct Pair[A, B] { first: A, second: B }\n\
         fn use_int(p: Pair[i32, i32]) -> i32 { return p.first + p.second; }\n\
         fn use_mixed(p: Pair[bool, i32]) -> i32 { return p.second; }\n\
         fn main() -> i32 {\n\
             let a: i32 = use_int(Pair[i32, i32] { first: 10, second: 20 });\n\
             let b: i32 = use_mixed(Pair[bool, i32] { first: true, second: 12 });\n\
             return a + b;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7c");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic struct should build cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "use_int(Pair{{10,20}}) + use_mixed(Pair{{true,12}}) = 30 + 12 = 42"
    );
}

#[test]
fn phase7_generic_struct_emits_distinct_mangled_types() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7c_ir.cplus");
    std::fs::write(
        &src,
        "struct Pair[A, B] { first: A, second: B }\n\
         fn f(p: Pair[i32, i32]) -> i32 { return p.first; }\n\
         fn g(p: Pair[bool, i32]) -> i32 { return p.second; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "build failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("%Pair__i32__i32"),
        "missing %Pair__i32__i32 in IR: {ir}"
    );
    assert!(
        ir.contains("%Pair__bool__i32"),
        "missing %Pair__bool__i32 in IR: {ir}"
    );
}

#[test]
fn phase7_generic_enum_option_runs() {
    // Slices 7GEN.5d + 7GEN.5e together: `Option[T]::Some(v)` at both
    // value-site *and* pattern-site (slice 7GEN.5e closed the
    // mangled-name leak; users no longer have to type `Option__i32`).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7d.cplus");
    std::fs::write(
        &src,
        "enum Option[T] { Some(T), None }\n\
         fn unwrap_or(o: Option[i32], default: i32) -> i32 {\n\
             return match o {\n\
                 Option[i32]::Some(v) => v,\n\
                 Option[i32]::None => default,\n\
             };\n\
         }\n\
         fn main() -> i32 {\n\
             let a: Option[i32] = Option[i32]::Some(35);\n\
             let b: Option[i32] = Option[i32]::None;\n\
             return unwrap_or(a, 0) + unwrap_or(b, 7);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7d");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Option[T] should build cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "Some(35) + None|7 = 42");
}

#[test]
fn phase7_generic_typed_impl_mut_self_runs() {
    // Slice 7GEN.5e step 3: mut self on generic-typed impl method,
    // and method that takes T as a param.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_genimpl_mut.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] {\n\
             fn get(self) -> T { return self.value; }\n\
             fn set(mut self, v: T) { self.value = v; }\n\
         }\n\
         fn main() -> i32 {\n\
             let mut b: Box[i32] = Box[i32] { value: 0 };\n\
             b.set(42);\n\
             return b.get();\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_genimpl_mut");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "mut-self generic-typed impl should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected Box.set(42).get() → 42");
}

#[test]
fn phase7_exit_demo_runs() {
    // Phase-7 exit criterion: docs/examples/phase7_generics.cplus
    // exercises every Phase-7 feature in one program and returns 42.
    // (Growable Vec[T] is deferred to slice 7HEAP — separate phase.)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bin = dir.join("p7demo");
    let src = std::path::PathBuf::from("../docs/examples/phase7_generics.cplus");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Phase-7 exit demo should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "Phase-7 exit demo should return 42");
}

// ---- Phase 10 slice 10.FFI.1: extern fn + raw pointers ----

#[test]
fn phase10_extern_fn_abs_runs() {
    // Slice 10.FFI.1a: extern fn declaration links against libc `abs`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10a.cplus");
    std::fs::write(
        &src,
        "extern fn abs(x: i32) -> i32;\n\
         fn main() -> i32 {\n\
             return unsafe { abs(0 -% 42) };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p10a");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "extern fn abs should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "abs(-42) → 42");
}

#[test]
fn phase10_extern_fn_emits_declare_not_define() {
    // Slice 10.FFI.1c: IR uses `declare` (no body) for extern fns.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10b.cplus");
    std::fs::write(
        &src,
        "extern fn abs(x: i32) -> i32;\n\
         fn main() -> i32 { return unsafe { abs(7) }; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "extern fn should emit IR cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("declare i32 @abs(i32)"),
        "expected `declare i32 @abs(i32)`, got IR:\n{ir}"
    );
    assert!(
        !ir.contains("define i32 @abs(") && !ir.contains("define internal i32 @abs("),
        "extern fn must not emit a body, got IR:\n{ir}"
    );
    // Call site uses the literal symbol name (no module prefix).
    assert!(
        ir.contains("call i32 @abs(i32"),
        "expected call to literal `@abs`, got IR:\n{ir}"
    );
}

#[test]
fn phase10_exit_demo_runs() {
    // Phase-10 exit demo: docs/examples/phase10_ffi.cplus exercises
    // every Phase-10 feature (extern fn + raw pointers + unsafe +
    // varargs + repr(C)) and exits 42. Stdout: "sum=42 count=3".
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bin = dir.join("p10exit");
    let src = std::path::PathBuf::from("../docs/examples/phase10_ffi.cplus");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Phase-10 exit demo should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42), "Phase-10 exit demo exit code");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, "sum=42 count=3\n");
}

#[test]
fn phase10_repr_c_struct_runs() {
    // Slice 10.FFI.5: `#[repr(C)]` accepted on struct decls; codegen
    // produces a binary that runs (the attribute is a marker — our
    // default layout already matches C on x86_64 for primitive fields).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10rc.cplus");
    std::fs::write(
        &src,
        "#[repr(C)]\n\
         struct Point { x: i32, y: i32 }\n\
         fn main() -> i32 {\n\
             let p: Point = Point { x: 7, y: 35 };\n\
             return p.x + p.y;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p10rc");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "#[repr(C)] struct should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase10_varargs_printf_runs() {
    // Slice 10.FFI.4: extern fn printf(fmt: *u8, ...) -> i32; works.
    // Prints "answer = 42\n" and returns the byte count (12).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10va.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         fn main() -> i32 {\n\
             let fmt: str = \"answer = %d\\n\";\n\
             return unsafe { printf(#str_ptr(fmt), 42) };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p10va");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "varargs printf should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(
        run.status.code(),
        Some(12),
        "printf returns bytes written = 12"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, "answer = 42\n");
}

#[test]
fn phase10_owned_string_sample_runs() {
    // The Phase-8 + 10.FFI exit demo at docs/examples/owned_string.cplus:
    // an owned, growable string type built entirely at user-level via
    // `extern fn malloc/free/memcpy` + `*u8` pointer operations +
    // `str_ptr` / `str_len` / `str_from_raw_parts` intrinsics. Prints
    // "Hello, world!" and exits with code 13 (the byte length).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bin = dir.join("p10os");
    let src = std::path::PathBuf::from("../docs/examples/owned_string.cplus");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "owned-string sample should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(13), "len(`Hello, world!`) = 13");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim_end(), "Hello, world!");
}

#[test]
fn phase10_pointer_roundtrip_via_malloc_runs() {
    // Slice 10.FFI.2: malloc → store-through-deref → load-through-deref → free.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10rt.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             return unsafe {\n\
                 let p: *u8 = malloc(1 as usize);\n\
                 *p = 42 as u8;\n\
                 let b: u8 = *p;\n\
                 free(p);\n\
                 b as i32\n\
             };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p10rt");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "pointer roundtrip should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "malloc + store + load roundtrips → 42"
    );
}

#[test]
fn phase10_pointer_index_and_arithmetic_runs() {
    // Slice 10.FFI.2: p[i] and `p + n` both work on raw pointers.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10ia.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             return unsafe {\n\
                 let p: *u8 = malloc(4 as usize);\n\
                 p[0] = 10 as u8;\n\
                 p[1] = 20 as u8;\n\
                 p[2] = 12 as u8;\n\
                 let q: *u8 = p + 1 as usize;\n\
                 let a: u8 = *q;\n\
                 let b: u8 = *(q + 1 as usize);\n\
                 free(p);\n\
                 (a + b) as i32\n\
             };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p10ia");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "pointer index+arith should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(32), "20 + 12 = 32 via pointer index+arith");
}

#[test]
fn phase10_raw_pointer_in_extern_signature_compiles() {
    // Slice 10.FFI.1b: `*u8` in an extern fn signature parses, sema-clean,
    // and emits as LLVM `ptr` in the declaration.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p10c.cplus");
    std::fs::write(
        &src,
        "extern fn strlen(s: *u8) -> usize;\n\
         extern fn abs(x: i32) -> i32;\n\
         fn main() -> i32 { return unsafe { abs(0 -% 5) }; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "raw pointer in extern signature should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("declare i64 @strlen(ptr)"),
        "expected `declare i64 @strlen(ptr)`, got IR:\n{ir}"
    );
}

#[test]
fn phase8_println_str_runs() {
    // Slice 8.STR.2: `#println(str)` prints a literal and exits.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p8s.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n    #println(\"Hello, C+!\");\n    return 0;\n}\n",
    )
    .unwrap();
    let bin = dir.join("p8s");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "#println(str) should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim_end(), "Hello, C+!");
}

#[test]
fn phase8_str_equality_runs() {
    // Slice 8.STR.3: byte-level `==` on `str` values via memcmp.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p8e.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let a: str = \"hello\";\n\
             let b: str = \"hello\";\n\
             let c: str = \"world\";\n\
             if a == b {\n\
                 if a != c {\n\
                     return 42;\n\
                 }\n\
             }\n\
             return 1;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p8e");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "str equality should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "expected a==b && a!=c to take us to 42"
    );
}

#[test]
fn phase8_fizzbuzz_exit_demo_runs() {
    // Phase-8 exit demo: FizzBuzz with real strings via #println(str).
    // The full output (alternating "Fizz"/"Buzz"/"FizzBuzz"/numbers) is
    // verified by checking three key lines, not the whole transcript —
    // brittle full-output checks add no value over the structural ones.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bin = dir.join("p8fb");
    let src = std::path::PathBuf::from("../docs/examples/fizzbuzz.cplus");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Phase-8 FizzBuzz exit demo should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        15,
        "expected 15 lines, got {}: {:?}",
        lines.len(),
        lines
    );
    assert_eq!(lines[0], "1");
    assert_eq!(lines[2], "Fizz"); // i=3
    assert_eq!(lines[4], "Buzz"); // i=5
    assert_eq!(lines[14], "FizzBuzz"); // i=15
}

#[test]
fn phase7_bound_satisfied_runs() {
    // Slice 7GEN.5e step 4 + 7GEN.6: bound-satisfied path runs end-to-end.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_bound.cplus");
    std::fs::write(
        &src,
        "fn pick[T: Copy](a: T, b: T) -> T { return a; }\n\
         fn main() -> i32 { return pick(42, 0); }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_bound");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "bound-satisfied call should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected pick(42, 0) → 42");
}

#[test]
fn phase7_bound_violated_rejected_e0502() {
    // Slice 7GEN.5e step 4: bound-violated call is rejected.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_bound_bad.cplus");
    std::fs::write(
        &src,
        "fn max[T: Ord](a: T, b: T) -> T { return a; }\n\
         struct Point { x: i32 }\n\
         fn main() -> i32 {\n\
             let p: Point = Point { x: 0 };\n\
             let r: Point = max(p, p);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "bound violation should fail compilation"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0502"),
        "expected E0502 in stderr, got: {}",
        stderr
    );
}

#[test]
fn phase7_generic_typed_impl_runs() {
    // Slice 7GEN.5e step 3: `impl Box[T] { fn get(self) -> T }` —
    // generic-typed impl. The Phase-7 exit-demo shape.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_genimpl.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] {\n\
             fn get(self) -> T { return self.value; }\n\
         }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = Box[i32] { value: 42 };\n\
             return b.get();\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_genimpl");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic-typed impl should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected Box[i32]::get() → 42");
}

#[test]
fn phase7_generic_method_with_turbofish_runs() {
    // Slice 7GEN.5e: generic method on a concrete-typed impl, called
    // with explicit turbofish.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_meth.cplus");
    std::fs::write(
        &src,
        "struct P { x: i32 }\n\
         impl P {\n\
             fn cast[T](self, value: T) -> T { return value; }\n\
         }\n\
         fn main() -> i32 {\n\
             let p: P = P { x: 0 };\n\
             return p.cast::[i32](42);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_meth");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic method with turbofish should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected cast::[i32](42) → 42");
}

#[test]
fn phase7_generic_assoc_call_with_turbofish_runs() {
    // Slice 7GEN.5e: generic associated function with turbofish.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_assoc.cplus");
    std::fs::write(
        &src,
        "struct P { x: i32 }\n\
         impl P {\n\
             fn ident[T](value: T) -> T { return value; }\n\
         }\n\
         fn main() -> i32 {\n\
             return P::ident::[i32](42);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_assoc");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic assoc call with turbofish should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected P::ident::[i32](42) → 42");
}

#[test]
fn phase7_generic_enum_unqualified_pattern_runs() {
    // Slice 7GEN.5e: unqualified `Option::Some(v)` against an
    // `Option[i32]` scrutinee — type-directed pattern resolution.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7e_unqual.cplus");
    std::fs::write(
        &src,
        "enum Option[T] { Some(T), None }\n\
         fn unwrap_or(o: Option[i32], default: i32) -> i32 {\n\
             return match o {\n\
                 Option::Some(v) => v,\n\
                 Option::None => default,\n\
             };\n\
         }\n\
         fn main() -> i32 {\n\
             let a: Option[i32] = Option[i32]::Some(35);\n\
             let b: Option[i32] = Option[i32]::None;\n\
             return unwrap_or(a, 0) + unwrap_or(b, 7);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p7e_unqual");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "unqualified Option pattern should build cleanly: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(42),
        "Some(35) + None|7 = 42 (unqualified pattern)"
    );
}

#[test]
fn phase7_generic_enum_emits_distinct_types() {
    // Two distinct enum instantiations should produce two distinct
    // LLVM enum types (`%enum.0` and `%enum.1`). The source-level
    // mangled name `Option__i32` doesn't appear in IR — codegen
    // names tagged enums by sequential ID (pre-Phase-7 lowering).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7d_ir.cplus");
    std::fs::write(
        &src,
        "enum Option[T] { Some(T), None }\n\
         fn use_i32(o: Option[i32]) -> i32 { return 0; }\n\
         fn use_bool(o: Option[bool]) -> i32 { return 0; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "build failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    // Two enum types declared in the IR preamble.
    assert!(ir.contains("%enum.0 = type"), "missing %enum.0: {ir}");
    assert!(ir.contains("%enum.1 = type"), "missing %enum.1: {ir}");
}

#[test]
fn phase7_self_outside_impl_rejected_e0508() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7_self.cplus");
    std::fs::write(
        &src,
        "fn loose(x: Self) -> i32 { return 0; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "Self outside impl/interface should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0508"),
        "expected E0508 in stderr: {stderr}"
    );
}

// Phase 11 cocoa-min — full ObjC interop integration test.

#[cfg(target_os = "macos")]
#[test]
fn phase11_cocoa_min_compiles_and_links() {
    // Verify the cocoa-min sample compiles + links against Cocoa.
    // The binary launches a GUI window when run; we don't exercise that
    // here (would need a GUI sandbox), but the compile + link is itself
    // a meaningful end-to-end test of all four Phase-11 ObjC slices:
    // 11.LINKNAME (msgSend aliases), 11.INTPTR (0 as *u8), 11.FN_PTR
    // (IMP callback), plus Phase 10 #[repr(C)] / extern fn / unsafe.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = format!(
        "{}/../objc-c-interop/cocoa-min/hello_appkit.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    let ll = dir.join("hello_appkit.ll");
    // Emit IR.
    let emit = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        emit.status.success(),
        "cpc --emit-ll failed: {}",
        String::from_utf8_lossy(&emit.stderr)
    );
    std::fs::write(&ll, &emit.stdout).unwrap();
    // Link with Cocoa.
    let bin = dir.join("hello_appkit");
    let link = Command::new("clang")
        .arg(&ll)
        .arg("-framework")
        .arg("Cocoa")
        .arg("-lobjc")
        .arg("-Wno-override-module")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke clang");
    assert!(link.success(), "clang link failed");
    assert!(bin.exists(), "binary not created");
}

// Phase 11 reference library: Allocator interface + VecI32 demo.

#[test]
fn phase11_vec_allocator_demo_runs() {
    // Builds VecI32 with CMalloc, pushes 1..=8 (exercising realloc-on-grow),
    // sums via indexed read, prints + exits 36.
    let out = compile_and_run("phase11_vec_allocator.cplus");
    assert_eq!(out.status.code(), Some(36), "vec_allocator should exit 36");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "36\n", "should print sum to stdout");
}

#[test]
fn phase11_raw_ptr_reinterpret_cast_in_unsafe_compiles() {
    // The `*u8 as *T` reinterpretation cast. Required for allocator-style
    // code that treats a byte buffer as a typed pointer.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ptr_reinterpret.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         fn main() -> i32 {\n\
             let p: *u8 = unsafe { malloc(4 as usize) };\n\
             let q: *i32 = unsafe { p as *i32 };\n\
             unsafe { *q = 42; }\n\
             return unsafe { *q };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ptr_reinterpret");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42));
}

#[test]
fn phase11_raw_ptr_reinterpret_outside_unsafe_rejected_e0801() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ptr_reinterpret_unsafe.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         fn main() -> i32 {\n\
             let p: *u8 = unsafe { malloc(4 as usize) };\n\
             let q: *i32 = p as *i32;\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "ptr-to-ptr reinterpret outside unsafe should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0801"),
        "expected E0801 in stderr: {stderr}"
    );
}

#[test]
fn phase11_if_expr_with_usize_arms_compiles() {
    // Pre-existing codegen bug: expr_value_ty didn't recognize Cast,
    // so `if c { 8 as usize } else { 16 as usize }` failed at codegen.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("if_usize.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let x: usize = if 1 == 1 { 8 as usize } else { 16 as usize };\n\
             return x as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("if_usize");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(8));
}

// Phase 11 slice 11.FN_PTR: function pointer types and values.

#[test]
fn phase11_fn_pointer_demo_runs() {
    let out = compile_and_run("phase11_fn_pointers.cplus");
    // Exit 42 = handle_click(0) + handle_hover(0) = 35 + 7.
    assert_eq!(
        out.status.code(),
        Some(42),
        "phase11_fn_pointers should exit 42"
    );
}

#[test]
fn phase11_fn_pointer_indirect_call_via_local_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fnptr_local.cplus");
    std::fs::write(
        &src,
        "fn double(x: i32) -> i32 { return x +% x; }\n\
         fn main() -> i32 {\n\
             let f: fn(i32) -> i32 = double;\n\
             return f(21);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("fnptr_local");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42));
}

#[test]
fn phase11_fn_pointer_struct_field_runs() {
    // The headline struct-of-callbacks pattern. Indirect call through
    // a struct field of FnPtr type.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fnptr_struct.cplus");
    std::fs::write(
        &src,
        "struct Actions { on_click: fn(i32) -> i32 }\n\
         fn handler(x: i32) -> i32 { return x +% 35; }\n\
         fn main() -> i32 {\n\
             let a: Actions = Actions { on_click: handler };\n\
             return a.on_click(7);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("fnptr_struct");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42));
}

#[test]
fn phase11_fn_pointer_to_libc_atexit_runs() {
    // Cross-language fn-pointer FFI: pass a C+ fn to libc's atexit,
    // verify the C runtime calls our fn back during program teardown.
    // This is the headline ObjC-interop-style use case.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fnptr_atexit.cplus");
    std::fs::write(
        &src,
        "extern fn atexit(cb: fn()) -> i32;\n\
         fn cleanup() { #println(42); }\n\
         fn main() -> i32 { unsafe { atexit(cleanup); } return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("fnptr_atexit");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "fn pointer to atexit should compile");
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, "42\n", "cleanup should print 42 from atexit");
}

#[test]
fn phase11_fn_pointer_signature_mismatch_rejected_e0302() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fnptr_mismatch.cplus");
    std::fs::write(
        &src,
        "fn double(x: i32) -> i32 { return x +% x; }\n\
         fn main() -> i32 { let f: fn(bool) -> i32 = double; return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0302"),
        "expected E0302 in stderr: {stderr}"
    );
}

// Phase 11 / P3 from null design: integer-to-raw-pointer cast.
// `0 as *T` inside `unsafe { }` is how C+ expresses FFI null without
// adding a `null` keyword to the language.

#[test]
fn phase11_int_to_ptr_cast_inside_unsafe_compiles() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("int_to_ptr.cplus");
    std::fs::write(
        &src,
        "extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             let null_ptr: *u8 = unsafe { 0 as *u8 };\n\
             unsafe { free(null_ptr); }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("int_to_ptr");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "0 as *u8 inside unsafe should compile");
    // libc's free(NULL) is a no-op per POSIX, so the binary should exit 0.
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
}

#[test]
fn phase11_int_to_ptr_cast_outside_unsafe_rejected_e0801() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("int_to_ptr_unsafe.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let p: *u8 = 0 as *u8; return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "0 as *u8 outside unsafe should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0801"),
        "expected E0801 in stderr: {stderr}"
    );
}

// Phase 11 / ObjC interop: `#[link_name = "..."]` attribute.

#[test]
fn phase11_link_name_aliases_symbol_runs() {
    // Declare libc's `abs` under a different C+ name via #[link_name].
    // Verifies the linker resolution: the C+ source calls `my_abs` but
    // the LLVM IR's `declare`/`call` use `@abs`, which links against libc.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("link_name_abs.cplus");
    std::fs::write(
        &src,
        "#[link_name = \"abs\"] extern fn my_abs(x: i32) -> i32;\n\
         fn main() -> i32 { return unsafe { my_abs(0 -% 42) }; }\n",
    )
    .unwrap();
    let bin = dir.join("link_name_abs");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "link_name extern fn should compile");
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42), "abs(-42) should return 42");
}

#[test]
fn phase11_link_name_emits_alias_in_ir() {
    // Verify the IR shape: `declare i32 @abs(i32)` even though the source
    // declared `my_abs`. The call site also uses `@abs`, not `@my_abs`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("link_name_ir.cplus");
    std::fs::write(
        &src,
        "#[link_name = \"abs\"] extern fn my_abs(x: i32) -> i32;\n\
         fn main() -> i32 { return unsafe { my_abs(0 -% 7) }; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success(), "compile should succeed");
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("declare i32 @abs("),
        "expected `declare i32 @abs(...)` in IR: {ir}"
    );
    assert!(
        ir.contains("@abs(i32"),
        "expected call to use `@abs` not `@my_abs`: {ir}"
    );
    assert!(
        !ir.contains("@my_abs"),
        "should NOT emit `@my_abs` anywhere: {ir}"
    );
}

#[test]
fn phase11_link_name_dedups_multiple_decls() {
    // Two `extern fn`s aliasing the same symbol must emit only one `declare`.
    // This is the headline ObjC use case: many typed signatures, one symbol.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("link_name_dedup.cplus");
    std::fs::write(
        &src,
        "#[link_name = \"abs\"] extern fn abs_i32(x: i32) -> i32;\n\
         #[link_name = \"abs\"] extern fn abs_again(x: i32) -> i32;\n\
         fn main() -> i32 { return unsafe { abs_i32(0 -% 7) + abs_again(0 -% 35) }; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "two link_name aliases for same symbol should compile"
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    let declare_count = ir.matches("declare i32 @abs(").count();
    assert_eq!(
        declare_count, 1,
        "expected exactly one `declare @abs`, got {declare_count}: {ir}"
    );
    // And the binary still runs.
    let bin = dir.join("link_name_dedup");
    let _ = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(
        run.status.code(),
        Some(42),
        "abs(-7) + abs(-35) should be 42"
    );
}

#[test]
fn phase11_link_name_on_non_extern_fn_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("link_name_local.cplus");
    std::fs::write(
        &src,
        "#[link_name = \"foo\"] fn local(x: i32) -> i32 { return x; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "link_name on non-extern fn should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0356"),
        "expected E0356 in stderr: {stderr}"
    );
}

// Phase 11 slice 11.LAYOUT: size_of[T]() / align_of[T]() intrinsics.

#[test]
fn phase11_size_of_align_of_demo_runs() {
    // Exit demo: prints primitive sizes/aligns + Point size, exits with size_of[Point].
    // Locks the layout numbers: i32=4, i64=8, *u8=8 on the supported 64-bit targets,
    // Point (two i32s) = 8 bytes.
    let out = compile_and_run("phase11_size_of.cplus");
    // Exit code is the size of Point (deliberately non-zero) — don't assert .success().
    let stdout = String::from_utf8_lossy(&out.stdout);
    // 6 primitive-layout lines (s_i8, s_i32, s_i64, a_i8, a_i32, a_i64) + 1 aggregate (s_point).
    let expected = "1\n4\n8\n1\n4\n8\n8\n";
    assert_eq!(stdout, expected, "stdout mismatch");
    assert_eq!(
        out.status.code(),
        Some(8),
        "exit code should be size_of[Point] = 8"
    );
}

#[test]
fn phase11_size_of_inside_generic_fn_runs() {
    // #size_of::[T]() inside a generic fn body — monomorphize must substitute
    // T to the concrete type via subst_type_ast in the call's type_args, or
    // codegen panics on Ty::Param. This pins that substitution.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("size_of_generic.cplus");
    std::fs::write(
        &src,
        "fn typed_size[T]() -> usize { return #size_of::[T](); }\n\
         fn main() -> i32 { let n: usize = typed_size::[i32](); return n as i32; }\n",
    )
    .unwrap();
    let bin = dir.join("size_of_generic");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        compile.success(),
        "size_of inside generic fn should compile cleanly"
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(
        run.status.code(),
        Some(4),
        "typed_size::[i32]() should return 4"
    );
}

#[test]
fn phase11_size_of_no_type_arg_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad_size_of.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let n: usize = #size_of(); return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "#size_of() with no type arg should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0501"),
        "expected E0501 in stderr: {stderr}"
    );
}

// Slice 7GEN.5c carry-forward (closed 2026-05-13): a generic fn whose
// declared return type names a generic struct must substitute T at the
// call site. Previously failed with "expected struct, found struct" because
// `subst_ty` didn't recurse through nested generic instantiations.

#[test]
fn phase7_generic_fn_returning_generic_struct_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g_ret.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         fn boxed[T](v: T) -> Box[T] { return Box[T] { value: v }; }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = boxed::[i32](42);\n\
             return b.value;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("g_ret");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic fn returning Box[T] should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase7_generic_fn_returning_generic_struct_inferred_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g_ret_inf.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         fn boxed[T](v: T) -> Box[T] { return Box[T] { value: v }; }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = boxed(7);\n\
             return b.value * 6;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("g_ret_inf");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic fn returning Box[T] via inference should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase7_generic_fn_returning_nested_generic_struct_runs() {
    // Nested case: fn -> Pair[Box[T], i32]. Requires recursive subst_ty
    // through two levels of generic instantiation.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g_nested.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         struct Pair[A, B] { first: A, second: B }\n\
         fn wrap[T](v: T, tag: i32) -> Pair[Box[T], i32] {\n\
             return Pair[Box[T], i32] { first: Box[T] { value: v }, second: tag };\n\
         }\n\
         fn main() -> i32 {\n\
             let p: Pair[Box[i32], i32] = wrap::[i32](20, 22);\n\
             return p.first.value + p.second;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("g_nested");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic fn returning nested generic should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

// Slice 7GEN.5c carry-forward (closed 2026-05-13): `Type[args]::assoc_fn(...)`
// — calling an associated function on an instantiated generic type — was
// rejected. Parser emits `GenericEnumCall`; sema now routes through the
// struct path when the name resolves to a generic struct template.

#[test]
fn phase7_generic_type_assoc_fn_call_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g_assoc.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] {\n\
             fn new(v: T) -> Box[T] { return Box[T] { value: v }; }\n\
         }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = Box[i32]::new(42);\n\
             return b.value;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("g_assoc");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Box[i32]::new should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase7_generic_type_assoc_fn_multi_args_runs() {
    // Two type args; calls a method that doesn't return Self.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g_assoc_multi.cplus");
    std::fs::write(
        &src,
        "struct Pair[A, B] { first: A, second: B }\n\
         impl Pair[A, B] {\n\
             fn make(a: A, b: B) -> Pair[A, B] { return Pair[A, B] { first: a, second: b }; }\n\
             fn sum_first_and_b(self) -> i32 { return self.first; }\n\
         }\n\
         fn main() -> i32 {\n\
             let p: Pair[i32, bool] = Pair[i32, bool]::make(42, true);\n\
             return p.sum_first_and_b();\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("g_assoc_multi");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Pair[i32,bool]::make should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_vec_generic_demo_runs() {
    // The fully-generic `Vec[T, A: Allocator]` sample, unblocked by the
    // two Phase-7 generics carry-forwards landing in the same session
    // (return-type substitution + Type[args]::assoc_fn).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = format!(
        "{}/../docs/examples/phase11_vec_generic.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    let bin = dir.join("vec_generic");
    let out = Command::new(cpc)
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "Vec[T, A] sample should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(
        run.status.code(),
        Some(36),
        "Vec generic demo should exit with sum 1..=8 = 36; stdout={}",
        String::from_utf8_lossy(&run.stdout)
    );
}

// Phase 11 polish (2026-05-13): `type Foo = Bar;` aliases.
// Parked from the Phase-9 rejection; this is independent work that
// landed because a real use case surfaced (renaming verbose generic
// instantiations for readability).

#[test]
fn phase11_type_alias_primitive_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_prim.cplus");
    std::fs::write(
        &src,
        "type Byte = i32;\n\
         fn main() -> i32 { let n: Byte = 42; return n; }\n",
    )
    .unwrap();
    let bin = dir.join("alias_prim");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "type alias should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_type_alias_struct_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_struct.cplus");
    std::fs::write(
        &src,
        "struct Point { x: i32, y: i32 }\n\
         type Coord = Point;\n\
         fn main() -> i32 {\n\
             let p: Coord = Point { x: 20, y: 22 };\n\
             return p.x + p.y;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("alias_struct");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "struct alias should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_type_alias_chained_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_chain.cplus");
    std::fs::write(
        &src,
        "type A = i32;\n\
         type B = A;\n\
         type C = B;\n\
         fn main() -> i32 { let n: C = 42; return n; }\n",
    )
    .unwrap();
    let bin = dir.join("alias_chain");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "chained alias should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_type_alias_cycle_rejected_e0510() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_cycle.cplus");
    std::fs::write(
        &src,
        "type A = B;\n\
         type B = A;\n\
         fn main() -> i32 { let x: A = 0; return x; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "cyclic alias should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0510"),
        "expected E0510 in stderr: {stderr}"
    );
}

#[test]
fn phase11_type_alias_duplicate_rejected_e0301() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_dup.cplus");
    std::fs::write(
        &src,
        "struct Foo { x: i32 }\n\
         type Foo = i32;\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "duplicate type definition should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0301"),
        "expected E0301 in stderr: {stderr}"
    );
}

#[test]
fn phase11_type_alias_in_fn_signature_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("alias_fn.cplus");
    std::fs::write(
        &src,
        "type Bytes = usize;\n\
         fn measure(n: Bytes) -> Bytes { return n; }\n\
         fn main() -> i32 { let n: Bytes = 42 as usize; return measure(n) as i32; }\n",
    )
    .unwrap();
    let bin = dir.join("alias_fn");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "alias in fn signature should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

// Phase 8 — owned `string` + interpolation. Three slices landed together:
// 8.STR.3 (owned string type), 8.STR.6 (blessed ToString), 8.STR.B
// (interpolation parser + codegen).

#[test]
fn phase8_string_new_and_methods_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("s.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let s: string = string::with_capacity(64 as usize);\n\
             let empty: bool = s.is_empty();\n\
             let view: str = s.as_str();\n\
             let n: i32 = s.len() as i32;\n\
             if empty { return 42; }\n\
             return n;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("s");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "string methods should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase8_to_string_on_primitives_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ts.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let n: i32 = -1234;\n\
             let s: string = n.to_string();\n\
             #println(s.as_str());\n\
             return s.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ts");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "to_string should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(5), "len of \"-1234\" is 5");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("-1234"), "stdout: {stdout}");
}

#[test]
fn phase8_interp_simple_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ip.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let n: i32 = 42;\n\
             let name: str = \"world\";\n\
             let g: string = \"hello ${name}, n is ${n}\";\n\
             #println(g.as_str());\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ip");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "interpolation should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("hello world, n is 42"), "stdout: {stdout}");
}

#[test]
fn phase8_interp_expressions_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ipe.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let n: i32 = 7;\n\
             let s: string = \"sum: ${n +% 3}, doubled: ${n *% 2}\";\n\
             #println(s.as_str());\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ipe");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "expr-inside-interp should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("sum: 10, doubled: 14"), "stdout: {stdout}");
}

#[test]
fn phase8_interp_double_dollar_escape_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dd.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let s: str = \"price: $$5\";\n\
             #println(s);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("dd");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "$$ escape should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("price: $5"), "stdout: {stdout}");
}

#[test]
fn phase8_interp_non_tostring_type_rejected_e0612() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nots.cplus");
    std::fs::write(
        &src,
        "struct Point { x: i32, y: i32 }\n\
         fn main() -> i32 {\n\
             let p: Point = Point { x: 1, y: 2 };\n\
             let s: string = \"point: ${p}\";\n\
             return s.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "non-ToString type should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0612"),
        "expected E0612 in stderr: {stderr}"
    );
}

#[test]
fn phase8_interp_demo_sample_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = format!(
        "{}/../docs/examples/phase8_interpolation.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    let bin = dir.join("interp_demo");
    let out = Command::new(cpc)
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "interpolation demo should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
}

// Phase 11 polish (2026-05-13): `-g` emits DWARF debug metadata.
// v1 ships function-level info only — verified via IR shape and via
// `nm -a` on the linked binary (macOS debug map).

#[test]
fn phase11_debuginfo_g_emits_di_metadata() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dbg.cplus");
    std::fs::write(
        &src,
        "fn helper(x: i32) -> i32 { return x +% 1; }\n\
         fn main() -> i32 { return helper(41); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("-g")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "-g should emit IR: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("!llvm.module.flags"),
        "missing module flags: {ir}"
    );
    assert!(ir.contains("!DICompileUnit"), "missing DICompileUnit: {ir}");
    assert!(ir.contains("!DIFile"), "missing DIFile: {ir}");
    assert!(
        ir.contains("!DISubprogram(name: \"main\""),
        "missing DISubprogram for main: {ir}"
    );
    assert!(
        ir.contains("!DISubprogram(name: \"helper\""),
        "missing DISubprogram for helper: {ir}"
    );
    assert!(ir.contains("!DILocation"), "missing DILocation: {ir}");
    // define lines should reference !dbg.
    assert!(
        ir.contains("i32 @main()") && ir.contains("!dbg "),
        "main define should carry !dbg: {ir}"
    );
}

#[test]
fn phase11_debuginfo_g_binary_links() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dbg_bin.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 42; }\n").unwrap();
    let bin = dir.join("dbg_bin");
    let out = Command::new(cpc)
        .arg("-g")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "cpc -g should link the binary: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_debuginfo_off_by_default_no_di() {
    // Sanity: without -g, no DI metadata.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nodbg.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        !ir.contains("!DICompileUnit"),
        "DI should be absent without -g: {ir}"
    );
}

// Phase 11 polish (2026-05-13): sanitizer flags. `--asan` / `--ubsan` /
// `--tsan` / `--msan` plumb through to clang and attach the matching
// `sanitize_*` function attribute to every `define` in cpc-emitted IR
// (clang's sanitizer passes skip functions without these attributes
// when consuming a `.ll` — the C frontend auto-attaches them).

#[test]
fn phase11_asan_attaches_function_attr() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--asan")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("i32 @main() sanitize_address"),
        "main should carry sanitize_address attr: {ir}"
    );
}

#[test]
fn phase11_ubsan_no_function_attr() {
    // UBSan doesn't gate on a function attribute; we just forward
    // -fsanitize=undefined to clang. Verify the IR is unchanged.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("u.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--ubsan")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        !ir.contains("sanitize_"),
        "UBSan should not attach a sanitize_ attr: {ir}"
    );
}

#[test]
fn phase11_sanitizer_exclusive_combo_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("x.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("x");
    let out = Command::new(cpc)
        .arg("--asan")
        .arg("--tsan")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "asan + tsan should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mutually exclusive"), "stderr: {stderr}");
}

#[test]
fn phase11_asan_catches_heap_overflow() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("oob.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         fn main() -> i32 {\n\
             let p: *u8 = unsafe { malloc(8 as usize) };\n\
             let mut i: usize = 0 as usize;\n\
             while i < 100 as usize {\n\
                 unsafe { *(p + i) = 42 as u8; }\n\
                 i = i +% 1 as usize;\n\
             }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("oob");
    let out = Command::new(cpc)
        .arg("--asan")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "asan build should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    // ASan exits non-zero and prints "AddressSanitizer:" on stderr.
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("AddressSanitizer"),
        "ASan didn't fire on heap overflow; stderr={stderr}, status={:?}",
        run.status
    );
}

// Phase 11 polish (2026-05-13): borrow-conflict diagnostics surface a
// secondary "borrowed here" / "moved here" / "sibling read of X here"
// span so users see both ends of the conflict.

#[test]
fn phase11_borrow_diagnostic_includes_secondary_label() {
    // v0.0.10 Phase 5: rewritten to use explicit `borrow A B` region
    // annotations. Under default-move semantics, plain `a: B` would
    // consume at the call site and the secondary-label E0372 path
    // wouldn't fire — explicit borrow annotations preserve the path.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bdiag.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn longest(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > b.x { return a; }
    return b;
}
fn drain(move b: B) { return; }
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = longest(a, b);
    drain(a);
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "stderr: {stderr}");
    assert!(
        stderr.contains("note: `r` borrows `a` here"),
        "secondary label missing; stderr: {stderr}"
    );
}

#[test]
fn phase11_borrow_diagnostic_json_carries_labels_field() {
    // v0.0.10 Phase 5: see sibling test for the rewrite rationale.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bjson.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn longest(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > b.x { return a; }
    return b;
}
fn drain(move b: B) { return; }
fn main() -> i32 {
    let a: B = B { x: 1 };
    let b: B = B { x: 2 };
    let r: B = longest(a, b);
    drain(a);
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--diagnostics=json")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("\"labels\""),
        "JSON output should carry a labels field; stderr: {stderr}"
    );
    assert!(stderr.contains("borrows `a` here"), "stderr: {stderr}");
}

// Phase 11 polish (2026-05-14): CLI niceties.

#[test]
fn phase11_cli_version_flag_works() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    for flag in &["--version", "-V"] {
        let out = Command::new(cpc).arg(flag).output().expect("invoke cpc");
        assert!(out.status.success(), "{flag} should succeed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with("cpc "), "{flag} stdout: {stdout}");
    }
}

#[test]
fn phase11_cli_check_subcommand_on_clean_file_exits_zero() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("clean.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "check on clean file should exit 0: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn phase11_cli_check_subcommand_on_broken_file_exits_nonzero() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("broken.cplus");
    std::fs::write(&src, "fn main() -> i32 { return foo; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "check on broken file should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0300"), "stderr: {stderr}");
}

#[test]
fn phase11_cli_check_subcommand_no_codegen_artifact() {
    // `cpc check` should never produce a binary even when the source
    // compiles cleanly. Verify by giving it a file that would produce
    // `a.out` if it ran through the full pipeline.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let cwd = dir.clone();
    let out = Command::new(cpc)
        .current_dir(&cwd)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let aout = cwd.join("a.out");
    assert!(!aout.exists(), "`check` should not create a.out");
}

#[test]
fn phase11_cli_subcommand_help_returns_only_relevant_slice() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("test")
        .arg("--help")
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("cpc test"),
        "`cpc test --help` should print only the test usage: {stdout}"
    );
    assert!(
        !stdout.contains("cpc build"),
        "subcommand help should NOT include other subcommands: {stdout}"
    );
}

#[test]
fn phase11_cli_help_documents_sanitizer_and_debuginfo_flags() {
    // Regression — these landed earlier but weren't in --help until
    // the CLI polish pass.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--help")
        .output()
        .expect("invoke cpc");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for flag in &[
        "--asan",
        "--ubsan",
        "--tsan",
        "--msan",
        "-g",
        "--debug-info",
    ] {
        assert!(
            stdout.contains(flag),
            "--help should document {flag}: {stdout}"
        );
    }
    assert!(
        stdout.contains("cpc check"),
        "--help should document `check`: {stdout}"
    );
}

// Phase 11 polish (2026-05-14): doc generator.

#[test]
fn phase11_doc_generator_writes_markdown() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("demo.cplus");
    std::fs::write(
        &src,
        "\
/// A point in 2D space.
pub struct Point { pub x: i32, pub y: i32 }

/// Sum two integers, wrapping on overflow.
pub fn add(a: i32, b: i32) -> i32 { return a +% b; }

/// Internal helper — not documented (and not pub).
fn private(n: i32) -> i32 { return n; }
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .current_dir(&dir)
        .arg("doc")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "doc should succeed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let md_path_rel = stdout.trim();
    assert!(md_path_rel.ends_with("demo.md"), "stdout: {stdout}");
    let md_path = dir.join(md_path_rel);
    let md = std::fs::read_to_string(&md_path).expect("read generated md");
    assert!(md.contains("# `demo.cplus`"));
    assert!(md.contains("`struct Point`"));
    assert!(md.contains("`fn add`"));
    assert!(
        !md.contains("private"),
        "private item should not appear: {md}"
    );
}

#[test]
fn phase11_doc_generator_preserves_fenced_doctests() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("d.cplus");
    std::fs::write(
        &src,
        "\
/// Adds two integers.
///
/// ```
/// assert add(2, 3) == 5;
/// ```
pub fn add(a: i32, b: i32) -> i32 { return a +% b; }
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .current_dir(&dir)
        .arg("doc")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let md_path_rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let md = std::fs::read_to_string(dir.join(&md_path_rel)).expect("read md");
    assert!(
        md.contains("assert add(2, 3) == 5"),
        "fenced doctest body should appear in output: {md}"
    );
}

#[test]
fn phase11_doc_generator_no_arg_errors() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc).arg("doc").output().expect("invoke cpc");
    assert!(!out.status.success(), "no-arg `doc` should error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("requires a FILE"), "stderr: {stderr}");
}

#[test]
fn phase11_doc_help_in_subcommand_help() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("doc")
        .arg("--help")
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("cpc doc FILE"),
        "subcommand help should be doc-specific: {stdout}"
    );
}

// Phase 11 polish (2026-05-14): owned `string` Drop integration.
// Strings allocated via `string::with_capacity` / `s.clone()` /
// `to_string()` / interpolation literals get freed at scope exit.
// Verified via ASan — without Drop, the runtime would report leaks.
// (LeakSanitizer is part of `-fsanitize=address` on macOS/Linux.)

#[test]
fn phase11_string_drop_no_leaks_under_asan() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nl.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let s: string = string::with_capacity(64 as usize);\n\
             let n: i32 = 42;\n\
             let g: string = \"n is ${n}\";\n\
             let t: string = n.to_string();\n\
             return s.len() as i32 +% t.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("nl");
    let out = Command::new(cpc)
        .arg("--asan")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "asan build should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    let stderr = String::from_utf8_lossy(&run.stderr);
    // ASan reports leaks on exit. If Drop is wired, stderr is clean.
    assert!(
        !stderr.contains("LeakSanitizer"),
        "ASan reported a leak — string Drop not freeing: stderr={stderr}"
    );
    assert!(
        !stderr.contains("AddressSanitizer"),
        "ASan reported a bug: stderr={stderr}"
    );
}

#[test]
fn phase11_string_drop_handles_empty_string_new_safely() {
    // `string::new()` stores ptr=null. free(null) is a libc no-op so
    // Drop on an empty string must not crash.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("en.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let s: string = string::new();\n\
             return s.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("en");
    let out = Command::new(cpc)
        .arg("--asan")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
}

// Phase 11 polish (2026-05-14): slice types `T[]`. Fat-pointer view
// of a contiguous run; same { ptr, len } shape as `str` but with the
// element type tracked at sema level. Construction via
// `slice_from_raw_parts` (unsafe); access via `slice_ptr` / `slice_len`.

#[test]
fn phase11_slice_type_parse_and_use_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("sl.cplus");
    std::fs::write(
        &src,
        "\
extern fn malloc(n: usize) -> *u8;

fn sum_i32(xs: i32[]) -> i32 {
    let n: usize = #slice_len(xs);
    let p: *i32 = #slice_ptr(xs);
    let mut acc: i32 = 0;
    let mut i: usize = 0 as usize;
    while i < n {
        acc = acc +% unsafe { *(p + i) };
        i = i +% 1 as usize;
    }
    return acc;
}

fn main() -> i32 {
    let buf: *u8 = unsafe { malloc(16 as usize) };
    let p: *i32 = unsafe { buf as *i32 };
    unsafe {
        *(p + 0 as usize) = 10;
        *(p + 1 as usize) = 20;
        *(p + 2 as usize) = 12;
    }
    let xs: i32[] = unsafe { #slice_from_raw_parts(p, 3 as usize) };
    return sum_i32(xs);
}
",
    )
    .unwrap();
    let bin = dir.join("sl");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "slice sample should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "sum of [10,20,12] = 42");
}

#[test]
fn phase11_slice_from_raw_parts_outside_unsafe_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nu.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let p: *i32 = unsafe { 0 as *i32 };
    let xs: i32[] = #slice_from_raw_parts(p, 0 as usize);
    return #slice_len(xs) as i32;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "slice_from_raw_parts outside unsafe should reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0801"),
        "expected E0801 in stderr: {stderr}"
    );
}

#[test]
fn phase11_slice_ptr_on_non_slice_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ns.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let n: i32 = 42;
    let p: *i32 = #slice_ptr(n);
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0302"),
        "expected E0302 in stderr: {stderr}"
    );
    assert!(
        stderr.contains("slice"),
        "stderr should mention 'slice': {stderr}"
    );
}

#[test]
fn phase11_slice_type_distinct_element_types() {
    // u8[] vs i32[] should NOT be assignment-compatible: tests that
    // the element type is type-checked, not erased.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dt.cplus");
    std::fs::write(
        &src,
        "\
fn takes_i32_slice(xs: i32[]) -> i32 { return #slice_len(xs) as i32; }
fn main() -> i32 {
    let p: *u8 = unsafe { 0 as *u8 };
    let bytes: u8[] = unsafe { #slice_from_raw_parts(p, 0 as usize) };
    return takes_i32_slice(bytes);
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "u8[] to i32[] should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0302"),
        "expected E0302 in stderr: {stderr}"
    );
}

// ---- Phase v0.0.2 Slice 1G: --emit-ll-opt and --emit-asm ----
//
// These flags pipe cpc's IR through clang to inspect post-optimization IR
// (for validating !range / !alias.scope survives -O2) or native assembly
// (for spot-checking hot-loop bounds-check elision). They are supporting
// infrastructure for slices 1B/1C — without them those slices cannot be
// validated, only emitted.

#[test]
fn emit_ll_opt_prints_post_pass_ir() {
    // The post-pass IR should still contain a `define` for main and should
    // carry attribute markup that LLVM adds during -O0 (e.g.
    // `local_unnamed_addr`, `target triple`).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 42; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll-opt")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "cpc --emit-ll-opt exited non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("define"), "missing define in post-pass IR: {s}");
    assert!(s.contains("@main"), "missing @main: {s}");
    // The clang round-trip always inserts a `target triple` line, which is
    // a reliable marker that we passed through `-S -emit-llvm` rather than
    // bypassing it.
    assert!(s.contains("target triple"), "missing target triple: {s}");
}

#[test]
fn emit_ll_opt_release_runs_optimization() {
    // At -O2 LLVM constant-folds `1+2+3` into a literal `ret i32 6`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fold.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 1 + 2 + 3; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--release")
        .arg("--emit-ll-opt")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("ret i32 6"),
        "expected constant-folded `ret i32 6` at -O2, got:\n{s}"
    );
}

#[test]
fn emit_asm_prints_assembly() {
    // Native assembly should contain a label for `main` (with target-
    // dependent leading underscore on macOS).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 42; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-asm")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "cpc --emit-asm exited non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    // Either `_main:` (Mach-O) or `main:` (ELF). Both contain `main:`.
    assert!(
        s.contains("main:") || s.contains("main "),
        "missing main label in asm: {s}"
    );
}

#[test]
fn emit_ll_opt_without_file_arg_fails() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--emit-ll-opt")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure without FILE arg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--emit-ll-opt requires a FILE argument"),
        "missing diagnostic, got: {stderr}"
    );
}

#[test]
fn emit_asm_without_file_arg_fails() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--emit-asm")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure without FILE arg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--emit-asm requires a FILE argument"),
        "missing diagnostic, got: {stderr}"
    );
}

#[test]
fn emit_ll_opt_propagates_sema_errors() {
    // Negative: bad source still fails at sema, before clang is invoked.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "fn main() -> i32 { return \"not an int\"; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll-opt")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected sema failure to propagate");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0302") || stderr.contains("error"),
        "expected sema diagnostic, got: {stderr}"
    );
}

#[test]
fn emit_ll_opt_preserves_slice_1a_attrs() {
    // End-to-end check that Slice 1A's `noundef` survives the clang round
    // trip. (LLVM keeps the attribute in `define` lines even at -O0.)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("attr.cplus");
    std::fs::write(
        &src,
        "fn double(x: i32) -> i32 { return x + x; }\n\
         fn main() -> i32 { return double(21); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll-opt")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("noundef"),
        "expected `noundef` attr to survive clang round-trip, got:\n{s}"
    );
}

// ---- Phase 2 Slices 2A/2B: package system MVP ----
//
// Manifest declares `[dependencies]`; resolver routes `<dep>/<module>`
// imports under `vendor/<dep>/src/`. Bare paths and stale `.cplus`
// extensions fail with structured E08xx diagnostics.

#[test]
fn vendor_import_round_trips_end_to_end() {
    // Smoke test the full Slice 2A+2B path: consumer declares a dep,
    // resolver routes `utils/math` to `vendor/utils/src/math.cplus`,
    // and the resulting binary returns the right value.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\nutils = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/utils/src")).unwrap();
    std::fs::write(
        dir.join("vendor/utils/Cplus.toml"),
        "[package]\nname = \"utils\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/utils/src/math.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"utils/math\" as math;\n\
         fn main() -> i32 { return math::add(20, 22); }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/app");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected 42 from math::add(20, 22)");
}

#[test]
fn undeclared_vendor_package_emits_e0852() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"nope/foo\" as f;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0852"), "expected E0852, got: {stderr}");
    assert!(
        stderr.contains("not a declared dependency"),
        "diagnostic should explain the cause: {stderr}"
    );
}

#[test]
fn stale_cplus_extension_in_import_emits_e0858() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nutils = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/utils/src")).unwrap();
    std::fs::write(
        dir.join("vendor/utils/Cplus.toml"),
        "[package]\nname = \"utils\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/utils/src/math.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"utils/math.cplus\" as math;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0858"), "expected E0858, got: {stderr}");
}

#[test]
fn vendor_escape_emits_e0859() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nutils = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/utils/src")).unwrap();
    std::fs::write(
        dir.join("vendor/utils/Cplus.toml"),
        "[package]\nname = \"utils\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/utils/src/math.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"utils/../escape\" as e;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0859"), "expected E0859, got: {stderr}");
}

#[test]
fn bare_import_emits_e0853() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nutils = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/utils/src")).unwrap();
    std::fs::write(
        dir.join("vendor/utils/Cplus.toml"),
        "[package]\nname = \"utils\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/utils/src/math.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"bare\" as b;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0853"), "expected E0853, got: {stderr}");
}

#[test]
fn local_relative_imports_still_work_with_deps_declared() {
    // Regression guard: declaring a `[dependencies]` entry must not
    // break existing local-relative imports inside the consumer.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nutils = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/utils/src")).unwrap();
    std::fs::write(
        dir.join("vendor/utils/Cplus.toml"),
        "[package]\nname = \"utils\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/utils/src/_dummy.cplus"),
        "pub fn unused() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helper.cplus"),
        "pub fn local() -> i32 { return 7; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./helper\" as helper;\n\
         fn main() -> i32 { return helper::local(); }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "local import broke after introducing deps");
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(7));
}

// ---- Phase 2 Slice 2C: build driver dep walk + bundled-binary verification ----
//
// `cpc build` walks the consumer's `[dependencies]`, loads each vendor's
// `Cplus.toml`, verifies the manifest-is-truth contract, and splices each
// dep's `[link]` contributions into the clang link line. Misuse fires
// distinct E08xx diagnostics with no graceful-degradation fallbacks.

/// Helper: ask the same `clang -print-target-triple` that cpc asks. Tests
/// that probe bundled-binary paths need to match cpc's host triple lookup
/// exactly — falsehood about the host is the difference between exercising
/// E0860 (file missing on host) and E0862 (host unsupported).
fn host_triple_for_test() -> String {
    let out = Command::new("clang")
        .arg("-print-target-triple")
        .output()
        .expect("invoke clang -print-target-triple");
    assert!(out.status.success(), "clang -print-target-triple failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn dep_link_table_libs_flow_through_to_linker() {
    // Vendor declares `[link] libs = [...]`; the consumer's binary should link
    // against that lib via the dep walk. Use a pure-source vendor package so
    // we don't need a bundled artifact. The example lib must actually exist on
    // the host linker's search path: libm (`m`) on Unix, but Windows has no
    // separate `m.lib` (math is in the UCRT), so use `kernel32` there.
    let lib_name = if cfg!(windows) { "kernel32" } else { "m" };
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\nmathy = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/mathy/src")).unwrap();
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        format!("[package]\nname = \"mathy\"\n\n[link]\nlibs = [\"{lib_name}\"]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/api.cplus"),
        "pub fn answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"mathy/api\" as m;\nfn main() -> i32 { return m::answer(); }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "dep with [link].libs should still build");
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn dep_walk_links_bundled_static_lib_end_to_end() {
    // Full bundled-artifact path: vendor ships a real `.a` at
    // `src/lib/<host>/libtiny.a`; consumer's C+ source declares an extern
    // fn matching the C symbol, calls it, and the dep walk wires the
    // archive into the link line.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();

    // 1. Build a tiny static archive from C, deposit at the vendor path.
    let lib_dir = dir.join("vendor/tiny/src/lib").join(&host);
    std::fs::create_dir_all(&lib_dir).unwrap();
    let c_src = dir.join("tiny_src.c");
    std::fs::write(&c_src, "int tiny_double(int n) { return n * 2; }\n").unwrap();
    let obj = dir.join("tiny.o");
    let cc = Command::new("clang")
        .arg("-c")
        .arg(&c_src)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("invoke clang -c");
    assert!(cc.success(), "clang -c on tiny.c failed");
    let archive = lib_dir.join("libtiny.a");
    let ar = Command::new(ar_prog())
        .arg("rcs")
        .arg(&archive)
        .arg(&obj)
        .status()
        .expect("invoke ar");
    assert!(ar.success(), "ar rcs failed");
    let _ = std::fs::remove_file(&obj);
    let _ = std::fs::remove_file(&c_src);

    // 2. Vendor manifest declares the artifact.
    std::fs::write(
        dir.join("vendor/tiny/Cplus.toml"),
        format!(
            "[package]\nname = \"tiny\"\n\n[link]\nbundled = [\"libtiny.a\"]\ntriples = [\"{host}\"]\n"
        ),
    ).unwrap();
    std::fs::create_dir_all(dir.join("vendor/tiny/src")).unwrap();
    std::fs::write(
        dir.join("vendor/tiny/src/api.cplus"),
        "pub fn double(n: i32) -> i32 { return unsafe { tiny_double(n) }; }\n\
         extern fn tiny_double(n: i32) -> i32;\n",
    )
    .unwrap();

    // 3. Consumer.
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\ntiny = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"tiny/api\" as tiny;\nfn main() -> i32 { return tiny::double(21); }\n",
    )
    .unwrap();

    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "bundled-artifact build failed");
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(42), "expected tiny::double(21) == 42");
}

#[test]
fn missing_vendor_manifest_emits_e0854() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nghost = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // vendor/ghost/ exists as a dir but no Cplus.toml inside.
    std::fs::create_dir_all(dir.join("vendor/ghost/src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0854"), "expected E0854, got: {stderr}");
    assert!(
        stderr.contains("is missing `Cplus.toml`"),
        "diagnostic should explain: {stderr}"
    );
}

#[test]
fn vendor_name_dir_mismatch_emits_e0855() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    // Vendor lives in vendor/foo/ but its Cplus.toml claims name = "bar".
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"bar\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0855"), "expected E0855, got: {stderr}");
    assert!(
        stderr.contains("must match its directory name"),
        "diagnostic should explain: {stderr}"
    );
}

#[test]
fn bundled_declared_but_file_missing_emits_e0860() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    // The triples list includes the host so we route past the E0862
    // check; the file at the expected path is absent → E0860.
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        format!("[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libmissing.a\"]\ntriples = [\"{host}\"]\n"),
    ).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0860"), "expected E0860, got: {stderr}");
    assert!(
        stderr.contains("libmissing.a"),
        "diagnostic should name the file: {stderr}"
    );
}

// ---- v0.0.3 Slice 1A: stdlib/io end-to-end ----

/// A project that declares `stdlib = "*"` and imports `stdlib/io` can call
/// `io::print` / `io::println` / `io::eprintln`. Verifies the new bodies in
/// vendor/stdlib/src/io.cplus produce the expected bytes on stdout/stderr.
#[test]
fn stdlib_io_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"io_smoke\"\n\n[[bin]]\nname = \"io_smoke\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let io_src = include_str!("../../vendor/stdlib/src/io.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/io.cplus"), io_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/io\" as io;\n\
         fn main() -> i32 {\n\
             io::print(\"hello \");\n\
             io::println(\"world\");\n\
             io::eprintln(\"to stderr\");\n\
             return 0;\n\
         }\n",
    )
    .unwrap();

    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/io_smoke");
    let out = Command::new(&bin).output().expect("run io_smoke");
    assert!(
        out.status.success(),
        "binary exited non-zero: {}",
        out.status
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hello world\n",
        "stdout mismatch"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "to stderr\n",
        "stderr mismatch"
    );
}

/// v0.0.3 Phase 2 (CWE-377 regression): two concurrent `cpc` invocations
/// on identical input must not collide on a predictable temp path. Before
/// the tempfile migration both invocations wrote to `cpc-<pid>.ll` — if
/// the PIDs happened to match (across containers, or on a wraparound),
/// one would overwrite the other's IR mid-compile. With tempfile-crate
/// random suffixes, paths are statistically unique even under collision.
#[test]
fn concurrent_cpc_invocations_no_temp_collision() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("a.cplus"), "fn main() -> i32 { return 7; }\n").unwrap();
    std::fs::write(dir.join("b.cplus"), "fn main() -> i32 { return 11; }\n").unwrap();

    let cpc_a = cpc.to_string();
    let dir_a = dir.clone();
    let h_a = std::thread::spawn(move || {
        let out = dir_a.join("a.out");
        let st = Command::new(&cpc_a)
            .arg(dir_a.join("a.cplus"))
            .arg("-o")
            .arg(&out)
            .status()
            .expect("invoke cpc a");
        assert!(st.success(), "cpc a failed");
        let run = Command::new(&out).status().expect("run a");
        assert_eq!(run.code(), Some(7), "a should exit 7");
    });
    let cpc_b = cpc.to_string();
    let dir_b = dir.clone();
    let h_b = std::thread::spawn(move || {
        let out = dir_b.join("b.out");
        let st = Command::new(&cpc_b)
            .arg(dir_b.join("b.cplus"))
            .arg("-o")
            .arg(&out)
            .status()
            .expect("invoke cpc b");
        assert!(st.success(), "cpc b failed");
        let run = Command::new(&out).status().expect("run b");
        assert_eq!(run.code(), Some(11), "b should exit 11");
    });
    h_a.join().expect("thread a");
    h_b.join().expect("thread b");
}

/// v0.0.3 Slice 1E: stdlib/env reads the PATH variable (universally set).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_env_var_into() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"envt\"\n\n[[bin]]\nname = \"envt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["vec", "env", "iterator", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/env\" as env;\n\
         import \"stdlib/vec\" as vec;\n\
         fn main() -> i32 {\n\
             let buf: vec::Vec[u8] = vec::new::[u8]();\n\
             if !env::var_into(\"PATH\", buf) { return 1; }\n\
             if !env::has_var(\"PATH\") { return 2; }\n\
             if env::argc() < (1 as usize) { return 3; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/envt");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "env smoke failed");
}

/// v0.0.3 Phase 4: cpc-bindgen reads a small C header and emits a
/// `.cplus` file that (a) parses through cpc cleanly and (b) links
/// against the original C source's compiled object. Round-trips
/// scalars, raw pointers, fixed-width integers via stdint.h aliases.
#[test]
#[cfg(target_os = "macos")]
fn cpc_bindgen_round_trips_via_c_library() {
    // cpc-bindgen is a sibling workspace crate; locate its binary
    // relative to this test's deps/ directory.
    let exe = std::env::current_exe().expect("current_exe");
    let mut target_dir = exe.parent().unwrap(); // .../deps
    target_dir = target_dir.parent().unwrap(); // .../<mode>
    let bindgen = target_dir.join("cpc-bindgen");
    assert!(
        bindgen.is_file(),
        "cpc-bindgen binary not built at {}",
        bindgen.display()
    );
    let bindgen = bindgen.to_string_lossy().to_string();
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    // Tiny C library: 4 fns covering scalar return, scalar args, pointer
    // args, and a double round-trip.
    let header = dir.join("api.h");
    std::fs::write(
        &header,
        "int add_ints(int a, int b);\n\
         unsigned int max_u32(unsigned int a, unsigned int b);\n\
         long count_bytes(const char *s);\n\
         double area_of_rect(double w, double h);\n",
    )
    .unwrap();
    let c_src = dir.join("api.c");
    std::fs::write(
        &c_src,
        "#include \"api.h\"\n\
         int add_ints(int a, int b) { return a + b; }\n\
         unsigned int max_u32(unsigned int a, unsigned int b) { return a > b ? a : b; }\n\
         long count_bytes(const char *s) { long n = 0; while (s[n]) n++; return n; }\n\
         double area_of_rect(double w, double h) { return w * h; }\n",
    )
    .unwrap();
    // Compile + archive the C source into libtiny.a.
    let c_obj = dir.join("api.o");
    let st = Command::new("clang")
        .arg("-c")
        .arg(&c_src)
        .arg("-o")
        .arg(&c_obj)
        .status()
        .expect("invoke clang");
    assert!(st.success(), "clang -c failed");
    let lib = dir.join("libtiny.a");
    let st = Command::new(ar_prog())
        .arg("rcs")
        .arg(&lib)
        .arg(&c_obj)
        .status()
        .expect("invoke ar");
    assert!(st.success(), "ar failed");

    // Run cpc-bindgen to produce the C+ bindings.
    let bg_out = Command::new(bindgen)
        .arg(&header)
        .output()
        .expect("invoke cpc-bindgen");
    assert!(
        bg_out.status.success(),
        "cpc-bindgen failed: {}",
        String::from_utf8_lossy(&bg_out.stderr)
    );
    let bindings = String::from_utf8_lossy(&bg_out.stdout);
    assert!(bindings.contains("extern fn add_ints(a: i32, b: i32) -> i32;"));
    assert!(bindings.contains("extern fn max_u32(a: u32, b: u32) -> u32;"));
    assert!(bindings.contains("extern fn count_bytes(s: *i8) -> i64;"));
    assert!(bindings.contains("extern fn area_of_rect(w: f64, h: f64) -> f64;"));

    // Write a `.cplus` driver that uses the bindings and asserts results.
    let cplus = dir.join("main.cplus");
    let driver = format!(
        "{bindings}\n\
         fn main() -> i32 {{\n\
             let s: str = \"hello\\0\";\n\
             let n: i64 = unsafe {{ count_bytes(#str_ptr(s) as *i8) }};\n\
             if n != (5 as i64) {{ return 1; }}\n\
             let sum: i32 = unsafe {{ add_ints(20 as i32, 22 as i32) }};\n\
             if sum != (42 as i32) {{ return 2; }}\n\
             let m: u32 = unsafe {{ max_u32(7 as u32, 11 as u32) }};\n\
             if m != (11 as u32) {{ return 3; }}\n\
             let a: f64 = unsafe {{ area_of_rect(3.0 as f64, 4.0 as f64) }};\n\
             if a != (12.0 as f64) {{ return 4; }}\n\
             return 0;\n\
         }}\n"
    );
    std::fs::write(&cplus, driver).unwrap();

    // cpc → .o, then clang to link with libtiny.a.
    let cplus_obj = dir.join("main.o");
    let st = Command::new(cpc)
        .arg("--emit-obj")
        .arg(&cplus)
        .arg("-o")
        .arg(&cplus_obj)
        .status()
        .expect("invoke cpc --emit-obj");
    assert!(st.success(), "cpc --emit-obj failed");
    let bin = dir.join("smoke");
    let st = Command::new("clang")
        .arg(&cplus_obj)
        .arg(&lib)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success(), "clang link failed");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "bindgen round-trip should exit 0");
}

/// v0.0.3 Slice 3A: compound assignment operators run correctly. Tests
/// every variant: arithmetic (+= -= *= /= %=), bitwise (&= |= ^=), and
/// shifts (<<= >>=) on both signed and unsigned integers.
#[test]
fn compound_assigns_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ca.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let mut x: i32 = 10 as i32;\n\
             x += 5 as i32;            // 15\n\
             x -= 2 as i32;            // 13\n\
             x *= 2 as i32;            // 26\n\
             x /= 3 as i32;            // 8\n\
             x %= 5 as i32;            // 3\n\
             let mut b: u32 = 0xff as u32;\n\
             b &= 0x0f as u32;         // 0x0f\n\
             b |= 0xa0 as u32;         // 0xaf\n\
             b ^= 0x20 as u32;         // 0x8f\n\
             b <<= 1 as u32;           // 0x11e\n\
             b >>= 2 as u32;           // 0x47 = 71\n\
             return x +% (b as i32);   // 3 + 71 = 74\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ca");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(74), "compound-assigns should produce 74");
}

/// v0.0.3 Slice 1D': stdlib/hash_map StrIntMap — insert + get + overwrite + miss.
#[test]
fn stdlib_hash_map_str_int() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hm\"\n\n[[bin]]\nname = \"hm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["result", "hash_map"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/hash_map\" as map;\n\
         import \"stdlib/result\" as result;\n\
         fn main() -> i32 {\n\
             let mut m: map::HashMap[str, i32] = map::new_str_int_map();\n\
             m.insert(\"apple\",  1 as i32);\n\
             m.insert(\"banana\", 2 as i32);\n\
             m.insert(\"cherry\", 3 as i32);\n\
             m.insert(\"apple\",  10 as i32);\n\
             let mut fails: i32 = 0 as i32;\n\
             guard let result::Result[i32, result::IoError]::Ok(v1) = m.get(\"apple\")\n\
                 else { return 50; };\n\
             if v1 != (10 as i32) { fails = fails +% (1 as i32); }\n\
             guard let result::Result[i32, result::IoError]::Ok(v2) = m.get(\"banana\")\n\
                 else { return 51; };\n\
             if v2 != (2 as i32) { fails = fails +% (1 as i32); }\n\
             if m.contains_key(\"grape\") { fails = fails +% (1 as i32); }\n\
             if m.len() != (3 as usize) { fails = fails +% (1 as i32); }\n\
             return fails;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/hm");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "hash_map round-trip failed");
}

/// v0.0.4 Phase 3 Slice 3B.5: generic HashMap[K, V] exercised over
/// integer keys (K=i32) and over str keys with overwrite + miss +
/// 100-entry grow path. Validates: (a) blessed `k.hash()` + `k.eq()`
/// dispatch through monomorphization; (b) two-type-parameter generic
/// struct shape; (c) doubling-on-load-factor still re-inserts every
/// live entry correctly.
#[test]
fn stdlib_hash_map_generic_k_v() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hmg\"\n\n[[bin]]\nname = \"hmg\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let hm_src = include_str!("../../vendor/stdlib/src/hash_map.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/hash_map.cplus"), hm_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/hash_map\" as hm;\n\
         import \"stdlib/result\" as result;\n\
         fn main() -> i32 {\n\
             // K = i32, V = i32 with overwrite + miss.\n\
             let mut m1: hm::HashMap[i32, i32] = hm::new::[i32, i32]();\n\
             m1.insert(1 as i32, 10 as i32);\n\
             m1.insert(2 as i32, 20 as i32);\n\
             m1.insert(1 as i32, 100 as i32);  // overwrite\n\
             if m1.len() != (2 as usize) { return 1 as i32; }\n\
             guard let result::Result[i32, result::IoError]::Ok(v1) = m1.get(1 as i32)\n\
                 else { return 2 as i32; };\n\
             if v1 != (100 as i32) { return 3 as i32; }\n\
             match m1.get(99 as i32) {\n\
                 result::Result[i32, result::IoError]::Ok(_) => { return 4 as i32; }\n\
                 result::Result[i32, result::IoError]::Err(_) => { }\n\
             }\n\
             // K = str, V = i32.\n\
             let mut m2: hm::HashMap[str, i32] = hm::new::[str, i32]();\n\
             m2.insert(\"apple\", 1 as i32);\n\
             m2.insert(\"banana\", 2 as i32);\n\
             m2.insert(\"cherry\", 3 as i32);\n\
             if m2.len() != (3 as usize) { return 5 as i32; }\n\
             guard let result::Result[i32, result::IoError]::Ok(v2) = m2.get(\"banana\")\n\
                 else { return 6 as i32; };\n\
             if v2 != (2 as i32) { return 7 as i32; }\n\
             if !m2.contains_key(\"apple\") { return 8 as i32; }\n\
             if m2.contains_key(\"grape\") { return 9 as i32; }\n\
             // Stress: 100 entries exercises grow_to (16 → 32 → 64 → 128).\n\
             let mut m3: hm::HashMap[i32, i32] = hm::new::[i32, i32]();\n\
             let mut i: i32 = 0;\n\
             while i < (100 as i32) {\n\
                 m3.insert(i, i *% (10 as i32));\n\
                 i = i +% (1 as i32);\n\
             }\n\
             if m3.len() != (100 as usize) { return 10 as i32; }\n\
             let mut sum: i32 = 0;\n\
             let mut j: i32 = 0;\n\
             while j < (100 as i32) {\n\
                 guard let result::Result[i32, result::IoError]::Ok(v) = m3.get(j)\n\
                     else { return 11 as i32; };\n\
                 sum = sum +% v;\n\
                 j = j +% (1 as i32);\n\
             }\n\
             // sum over j of j*10 for j in 0..100 = 10 * 99 * 100 / 2 = 49500.\n\
             if sum != (49500 as i32) { return 12 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (generic HashMap)");
    let bin = dir.join("target/debug/hmg");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "generic HashMap round-trip failed");
}

/// v0.0.3 Slice 1C: stdlib/net round-trip — fork() a server, parent acts
/// as client, send "HELLO" (5 bytes), receive echo, assert len.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_net_tcp_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"netrt\"\n\n[[bin]]\nname = \"netrt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // v0.0.4 Phase 3 Slice 3A.3: net.cplus now imports stdlib/reactor for
    // the async I/O wrappers; its async fns also implicitly need
    // stdlib/future for the `Future[T]` shape. Stage both alongside net.
    for name in &[
        "result", "vec", "net", "netsys", "io", "reactor", "future", "iterator", "option",
    ] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    // On Linux the resolver loads the `*_linux.cplus` overrides (epoll reactor,
    // Linux syscall constants) in place of their base files; stage them so the
    // fixture links on Linux too. macOS uses the base files copied above.
    for over in &["netsys_linux", "reactor_linux"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{over}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{over}.cplus")), src).unwrap();
    }
    // Pick a port that's almost certainly unused on the test runner.
    // Using a per-test-pid offset keeps parallel test runs from colliding.
    let port: u16 = 41000 + (std::process::id() as u16 & 0x0fff);
    std::fs::write(
        dir.join("src/main.cplus"),
        format!(
            "import \"stdlib/net\" as net;\n\
             import \"stdlib/vec\" as vec;\n\
             import \"stdlib/result\" as result;\n\
             extern fn fork() -> i32;\n\
             extern fn waitpid(pid: i32, status: *i32, options: i32) -> i32;\n\
             extern fn sleep(secs: u32) -> u32;\n\
             extern fn _exit(code: i32);\n\
             fn run_server() -> i32 {{\n\
                 guard let result::Result[net::TcpListener, result::IoError]::Ok(lis) = net::listen_tcp({port} as u16)\n\
                     else {{ return 1; }};\n\
                 let mut listener: net::TcpListener = lis;\n\
                 guard let result::Result[net::TcpStream, result::IoError]::Ok(client) = listener.accept()\n\
                     else {{ return 2; }};\n\
                 let mut stream: net::TcpStream = client;\n\
                 guard let result::Result[vec::Vec[u8], result::IoError]::Ok(data) = stream.read_to_end()\n\
                     else {{ return 3; }};\n\
                 guard let result::Result[usize, result::IoError]::Ok(w) = stream.write_all(data)\n\
                     else {{ return 4; }};\n\
                 if w == (0 as usize) {{ return 5; }}\n\
                 return 0;\n\
             }}\n\
             fn run_client() -> usize {{\n\
                 unsafe {{ sleep(1 as u32); }}\n\
                 guard let result::Result[net::TcpStream, result::IoError]::Ok(s) = net::connect_tcp(\"127.0.0.1\", {port} as u16)\n\
                     else {{ return 0 as usize; }};\n\
                 let mut stream: net::TcpStream = s;\n\
                 let mut payload: vec::Vec[u8] = vec::new::[u8]();\n\
                 payload.push(72 as u8); payload.push(73 as u8);\n\
                 guard let result::Result[usize, result::IoError]::Ok(w) = stream.write_all(payload)\n\
                     else {{ return 0 as usize; }};\n\
                 if w == (0 as usize) {{ return 0 as usize; }}\n\
                 stream.shutdown_write();\n\
                 guard let result::Result[vec::Vec[u8], result::IoError]::Ok(got) = stream.read_to_end()\n\
                     else {{ return 0 as usize; }};\n\
                 return got.len();\n\
             }}\n\
             fn main() -> i32 {{\n\
                 let pid: i32 = unsafe {{ fork() }};\n\
                 if pid < (0 as i32) {{ return 9; }}\n\
                 if pid == (0 as i32) {{\n\
                     let rc: i32 = run_server();\n\
                     unsafe {{ _exit(rc); }}\n\
                     return rc;\n\
                 }}\n\
                 let n: usize = run_client();\n\
                 let null_status: *i32 = unsafe {{ 0 as *i32 }};\n\
                 unsafe {{ waitpid(pid, null_status, 0 as i32); }}\n\
                 if n != (2 as usize) {{ return 1; }}\n\
                 return 0;\n\
             }}\n"
        ),
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/netrt");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "tcp round-trip failed");
}

/// v0.0.3 drop-tracking: a non-Copy aggregate (Vec[u8]) wrapped in a
/// Result and returned across a module boundary must not double-free its
/// heap allocation. Five compiler fixes coordinate to make this work:
/// (1) `scan_moves` recognizes `return v;`, `let v = src;`, and Path-callee
/// args as moves; (2) `mark_moved` fires at each of those codegen sites;
/// (3) enum `payload_slots` is computed from byte size, not type count;
/// (4) `return_passes_by_sret_widened` covers non-Copy structs + enums;
/// (5) method signatures use sret when the return type qualifies.
#[test]
fn cross_module_vec_in_result_no_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"dtrk\"\n\n[[bin]]\nname = \"dtrk\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["vec", "result", "iterator", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    // helper module that constructs the Vec + wraps in Result, lives in
    // its own file so the move crosses a module boundary.
    std::fs::write(
        dir.join("vendor/stdlib/src/maker.cplus"),
        "import \"./vec\" as vec;\n\
         import \"./result\" as result;\n\
         pub fn make_three_bytes() -> result::Result[vec::Vec[u8], result::IoError] {\n\
             let mut v: vec::Vec[u8] = vec::new::[u8]();\n\
             v.push(7 as u8);\n\
             v.push(8 as u8);\n\
             v.push(9 as u8);\n\
             return result::io_ok::[vec::Vec[u8]](v);\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/result\" as result;\n\
         import \"stdlib/maker\" as maker;\n\
         fn main() -> i32 {\n\
             guard let result::Result[vec::Vec[u8], result::IoError]::Ok(got) =\n\
                 maker::make_three_bytes()\n\
                 else {{ return 1; }};\n\
             return got.len() as i32;\n\
         }\n"
        .replace("{{ return 1; }}", "{ return 1; }")
        .as_str(),
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/dtrk");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(3),
        "Vec[u8] len after cross-module Result move must be 3"
    );
}

/// v0.0.3 Slice 1B: stdlib/fs round-trip — write 3 bytes via fs::create +
/// File::write_all; read them back via fs::open_read + File::read_to_end;
/// verify the byte count matches.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_fs_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"fsrt\"\n\n[[bin]]\nname = \"fsrt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // v0.0.5 Phase 4 Slice 4C: fs.cplus now imports net + reactor +
    // future (for File::read_async). Stage them too.
    for name in &[
        "result", "vec", "fs", "io", "iterator", "option", "net", "netsys", "reactor", "future",
    ] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    // On Linux the resolver loads the `*_linux.cplus` overrides in place of
    // their base files; stage them so the fixture links on Linux too.
    for over in &["netsys_linux", "reactor_linux"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{over}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{over}.cplus")), src).unwrap();
    }
    let tmp_file = dir.join("fsrt.txt");
    let tmp_path = tmp_file.to_string_lossy().to_string();
    std::fs::write(
        dir.join("src/main.cplus"),
        format!(
            "import \"stdlib/fs\" as fs;\n\
             import \"stdlib/vec\" as vec;\n\
             import \"stdlib/result\" as result;\n\
             fn write_data(path: str) -> bool {{\n\
                 let mut data: vec::Vec[u8] = vec::new::[u8]();\n\
                 data.push(72 as u8);\n\
                 data.push(73 as u8);\n\
                 data.push(33 as u8);\n\
                 guard let result::Result[fs::File, result::IoError]::Ok(w) = fs::create(path)\n\
                     else {{ return false; }};\n\
                 let mut writer: fs::File = w;\n\
                 guard let result::Result[usize, result::IoError]::Ok(wrote) = writer.write_all(data)\n\
                     else {{ return false; }};\n\
                 if wrote == (0 as usize) {{ return false; }}\n\
                 writer.close();\n\
                 return true;\n\
             }}\n\
             fn read_len(path: str) -> usize {{\n\
                 guard let result::Result[fs::File, result::IoError]::Ok(r) = fs::open_read(path)\n\
                     else {{ return 0 as usize; }};\n\
                 let mut reader: fs::File = r;\n\
                 guard let result::Result[vec::Vec[u8], result::IoError]::Ok(got) = reader.read_to_end()\n\
                     else {{ return 0 as usize; }};\n\
                 return got.len();\n\
             }}\n\
             fn main() -> i32 {{\n\
                 let path: str = \"{tmp_path}\";\n\
                 if !write_data(path) {{ return 1; }}\n\
                 let n: usize = read_len(path);\n\
                 if n != (3 as usize) {{ return 2; }}\n\
                 return 0;\n\
             }}\n"
        ),
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/fsrt");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "fs round-trip failed");
}

/// v0.0.3 Slice 1P.3: turbofish call to a generic free function in another
/// module with a qualified type-arg (`mod::other::T`). Before the fix,
/// Call's type_args weren't rewritten by the resolver, so cross-module
/// turbofish failed at sema with "unknown type `other::T`".
#[test]
fn stdlib_cross_module_turbofish_with_qualified_type_arg() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"tbf\"\n\n[[bin]]\nname = \"tbf\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/result\" as result;\n\
         fn main() -> i32 {\n\
             let r: result::Result[i32, result::IoError] =\n\
                 result::ok::[i32, result::IoError](42 as i32);\n\
             return match r {\n\
                 result::Result[i32, result::IoError]::Ok(v) => v,\n\
                 result::Result[i32, result::IoError]::Err(_) => 0 -% 1 as i32,\n\
             };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/tbf");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected 42 from Ok branch");
}

/// v0.0.3 Slice 1P.2: a method defined in `impl Vec[T] { fn push(...) }`
/// inside `stdlib/vec` is reachable on a `Vec[u8]` constructed from a
/// consumer that imports both `stdlib/vec` and an unrelated module
/// `stdlib/other`. Before the two-phase collect_methods fix, importing a
/// downstream module whose impl methods returned `Vec[u8]` caused method
/// table population to race with instantiation, leaving Vec[u8] methodless.
#[test]
fn stdlib_cross_module_generic_method_propagation() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"xmm\"\n\n[[bin]]\nname = \"xmm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    // v0.0.5 Phase 3 Slice 3A: vec.cplus imports stdlib/iterator (for
    // Vec::iter's `gen fn` return wrap → Iterator[T]); iterator.cplus
    // imports stdlib/option. Stage both alongside vec.cplus so sema's
    // signature collection resolves cleanly.
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    // `other` module uses `vec::Vec[u8]` in its method's return type —
    // this is what triggered the pre-fix bug.
    std::fs::write(
        dir.join("vendor/stdlib/src/other.cplus"),
        "import \"./vec\" as vec;\n\
         pub struct Maker { _x: i32 }\n\
         pub fn make_maker() -> Maker { return Maker { _x: 0 as i32 }; }\n\
         impl Maker {\n\
             pub fn make_buf(self) -> vec::Vec[u8] {\n\
                 let mut buf: vec::Vec[u8] = vec::new::[u8]();\n\
                 buf.push(7 as u8);\n\
                 return buf;\n\
             }\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/other\" as other;\n\
         fn main() -> i32 {\n\
             let mut v: vec::Vec[u8] = vec::new::[u8]();\n\
             v.push(1 as u8);\n\
             v.push(2 as u8);\n\
             return v.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/xmm");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(2), "expected v.len() = 2");
}

/// v0.0.4 Phase 1A: regression for musttail+sret ABI mismatch.
///
/// A consumer module receives a `Vec[u8]` from a producer module whose
/// constructor `make_empty_buf()` tail-returns `vec::new::[u8]()`. Both
/// wrapper and callee use sret (Vec[u8] is non-Copy, 24-byte). Before the
/// fix, the musttail call site forwarded the caller's sret slot as bare
/// `ptr %0` while the callee declared `ptr sret(%Vec__u8) ...`. LLVM's
/// musttail verifier rejected with "mismatched ABI impacting function
/// attributes". The fix mirrors the callee's sret attribute string on the
/// call site.
#[test]
fn musttail_sret_cross_module_vec_return_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mts\"\n\n[[bin]]\nname = \"mts\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    // v0.0.5 Phase 3 Slice 3A: vec.cplus imports stdlib/iterator (for
    // Vec::iter's `gen fn` return wrap → Iterator[T]); iterator.cplus
    // imports stdlib/option. Stage both alongside vec.cplus so sema's
    // signature collection resolves cleanly.
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    // Producer wrapper: tail-calls vec::new[u8]. Both sites are sret.
    std::fs::write(
        dir.join("src/maker.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         pub fn make_empty_buf() -> vec::Vec[u8] {\n\
             return vec::new::[u8]();\n\
         }\n",
    )
    .unwrap();
    // Consumer pushes onto the producer's returned Vec.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./maker\" as maker;\n\
         fn main() -> i32 {\n\
             let mut buf = maker::make_empty_buf();\n\
             buf.push(7 as u8);\n\
             buf.push(8 as u8);\n\
             buf.push(9 as u8);\n\
             return buf.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (musttail+sret regression?)");
    let bin = dir.join("target/debug/mts");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(3), "expected buf.len() = 3");
}

/// v0.0.4 Phase 1B: generic-fn return-type T-substitution + transitive
/// generic-fn instantiation propagation.
///
/// `fn make_buf[T]() -> vec::Vec[T] { return vec::new::[T](); }` exercises:
///   1. A user-written generic fn that returns a stdlib generic struct.
///   2. The body's inner generic call (`vec::new::[T]`) uses the outer
///      fn's type-param T.
///   3. A consumer calls `make_buf::[i32]()` and gets back `vec::Vec[i32]`.
///
/// Before the fix, monomorphize only saw sema's `fn_instantiations`,
/// which (for the inner call inside the generic body) recorded
/// `(vec::new, [Ty::Param("T")])` — not a real concrete instantiation.
/// `vec_new__i32` was never synthesized; codegen panicked looking up the
/// un-mangled name.
///
/// Fix: monomorphize propagates instantiations to a fixed point by
/// walking each instantiation's template body, reading the AST
/// turbofish type-args, and substituting through the outer subst.
#[test]
fn generic_fn_returning_generic_struct_transitive_instantiation() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"gpb\"\n\n[[bin]]\nname = \"gpb\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let io_src = include_str!("../../vendor/stdlib/src/io.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/io.cplus"), io_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/io\" as io;\n\
         \n\
         fn make_buf[T]() -> vec::Vec[T] {\n\
             return vec::new::[T]();\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
             let mut b = make_buf::[i32]();\n\
             b.push(7);\n\
             b.push(8);\n\
             b.push(9);\n\
             io::println(\"ok\");\n\
             return b.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (Phase 1B regression?)");
    let bin = dir.join("target/debug/gpb");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(3), "expected b.len() = 3");
}

/// v0.0.4 Phase 1C: `Type[args]::name(...)` resolves to a same-module
/// free generic fn when no impl-block method matches.
///
/// `vec::Vec[i32]::with_capacity(16)` desugars to a call of the free fn
/// `vec::with_capacity::[i32](16)`. Mirrors the Rust UFCS shape
/// `Vec::<i32>::with_capacity(16)` despite C+ stdlib having
/// `with_capacity` as a module-level free fn rather than an impl-block
/// associated fn.
#[test]
fn assoc_free_fn_dispatch_via_type_brackets() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"ats\"\n\n[[bin]]\nname = \"ats\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let io_src = include_str!("../../vendor/stdlib/src/io.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/io.cplus"), io_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/io\" as io;\n\
         \n\
         fn main() -> i32 {\n\
             let mut b = vec::Vec[i32]::with_capacity(16);\n\
             b.push(7);\n\
             b.push(8);\n\
             io::println(\"ok\");\n\
             return b.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (Phase 1C regression?)");
    let bin = dir.join("target/debug/ats");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(2), "expected b.len() = 2");
}

/// v0.0.4 Phase 1E: non-Copy `O` for `thread::spawn` + `JoinHandle::join`.
///
/// Worker fn returns `string` via sret; the trampoline forwards its sret
/// slot into the heap ctx so the value lands at the offset `join` reads
/// from. join's aggregate load lifts the 24-byte struct back to the
/// parent. ASan-clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_join_non_copy_string() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"tsj\"\n\n[[bin]]\nname = \"tsj\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn produce() -> string { return \"hello from worker\".to_string(); }\n\
         fn main() -> i32 {\n\
             let h: thread::JoinHandle[string] = thread::spawn::[string](produce);\n\
             let s: string = h.join();\n\
             return s.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1E thread sret regression?)"
    );
    let bin = dir.join("target/debug/tsj");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(17),
        "expected len(\"hello from worker\") = 17, got {:?}",
        run.code()
    );
}

/// v0.0.4 Phase 1E: `async fn` returning non-Copy `T`.
///
/// Pre-fix, the coroutine prologue passed `ptr null` as the promise to
/// `llvm.coro.id` but later wrote a value via `coro.promise`. For Copy
/// scalars the OOB writes landed in frame slack and "worked" by luck; for
/// `string` (24 B) they overflowed (ASan caught it). Fix: allocate
/// `%.coro.promise = alloca <T>` and pass it through `coro.id` so the
/// promise slot is part of the frame at a known offset.
#[test]
fn async_fn_returning_string_through_block_on() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"asr\"\n\n[[bin]]\nname = \"asr\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    // v0.0.4 Phase 3 Slice 3A.1: executor.cplus now imports reactor.
    let __reactor_for_executor = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor.cplus"),
        __reactor_for_executor,
    )
    .unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/executor\" as executor;\n\
         async fn inner() -> string {\n\
             return \"hello from coro\".to_string();\n\
         }\n\
         async fn outer() -> string {\n\
             let s = await inner();\n\
             return s;\n\
         }\n\
         fn main() -> i32 {\n\
             let f: future::Future[string] = outer();\n\
             let s: string = executor::block_on::[string](f);\n\
             return s.len() as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1E async sret regression?)"
    );
    let bin = dir.join("target/debug/asr");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(15),
        "expected len(\"hello from coro\") = 15, got {:?}",
        run.code()
    );
}

/// v0.0.4 Phase 1F: recursive `mangle_o_for_tramp` — raw pointer O.
///
/// `thread::spawn::[*u8](worker)` previously fell into the
/// "unsupported" arm of the mangler and crashed at runtime. The
/// recursive mangler matches sema's `mangle_ty_for_name` so
/// `JoinHandle__ptr_u8` lookups land.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_join_raw_pointer_o() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"tsp\"\n\n[[bin]]\nname = \"tsp\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn produce() -> *u8 { return unsafe { malloc(64 as usize) }; }\n\
         fn main() -> i32 {\n\
             let h: thread::JoinHandle[*u8] = thread::spawn::[*u8](produce);\n\
             let p: *u8 = h.join();\n\
             unsafe { free(p); }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1F raw-pointer mangler regression?)"
    );
    let bin = dir.join("target/debug/tsp");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "expected clean round-trip");
}

/// v0.0.4 Phase 1F: fn-pointer O round-trip. Mangler emits `fn_ret_i32`
/// (matches sema's `mangle_ty_for_name` shape).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_join_fn_pointer_o() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"tsf\"\n\n[[bin]]\nname = \"tsf\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn pick_42() -> i32 { return 42; }\n\
         fn produce_fn() -> fn() -> i32 { return pick_42; }\n\
         fn main() -> i32 {\n\
             let h: thread::JoinHandle[fn() -> i32] = thread::spawn::[fn() -> i32](produce_fn);\n\
             let f: fn() -> i32 = h.join();\n\
             return f();\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1F fn-pointer mangler regression?)"
    );
    let bin = dir.join("target/debug/tsf");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected pick_42() = 42");
}

/// v0.0.4 Phase 1G: generic `async fn` end-to-end across multiple
/// instantiations.
///
/// Sema threads `is_async` through `subst_type_ast` already (v0.0.3
/// Slice 5E groundwork); monomorphize's `synthesize_fn` preserves
/// `is_async` when cloning the template. This pins the property by
/// driving 3 concrete instantiations (`id::[i32]`, `id::[i64]`,
/// `id::[bool]`) through `block_on` and verifying each round-trip.
#[test]
fn generic_async_fn_multi_instantiation_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"gar\"\n\n[[bin]]\nname = \"gar\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    // v0.0.4 Phase 3 Slice 3A.1: executor.cplus now imports reactor.
    let __reactor_for_executor = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor.cplus"),
        __reactor_for_executor,
    )
    .unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/executor\" as executor;\n\
         async fn id[T](x: T) -> T { return x; }\n\
         fn main() -> i32 {\n\
             let f1: future::Future[i32] = id::[i32](42);\n\
             let n: i32 = executor::block_on::[i32](f1);\n\
             if n != 42 { return 1; }\n\
             let f2: future::Future[i64] = id::[i64](99 as i64);\n\
             let m: i64 = executor::block_on::[i64](f2);\n\
             if m != (99 as i64) { return 2; }\n\
             let f3: future::Future[bool] = id::[bool](true);\n\
             let b: bool = executor::block_on::[bool](f3);\n\
             if !b { return 3; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1G generic async fn regression?)"
    );
    let bin = dir.join("target/debug/gar");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected all generic async instantiations to round-trip clean"
    );
}

/// v0.0.4 Phase 2 Slice 2B: `Box[T]` — single heap-allocated owned value.
///
/// Exercises:
///   - i32 round-trip (`new(42).get() == 42`).
///   - `set` mutation followed by `get` reads the new value.
///   - `unwrap(move self)` consumes the box and the function-exit Drop
///     frees the heap slot — no manual free, or we'd double-free.
///   - non-Copy `string` round-trip via `move v` param.
///   - ASan-clean.
#[test]
fn stdlib_box_round_trip_copy_and_non_copy() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"boxr\"\n\n[[bin]]\nname = \"boxr\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let box_src = include_str!("../../vendor/stdlib/src/box.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/box.cplus"), box_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/box\" as box;\n\
         fn main() -> i32 {\n\
             let mut b = box::new::[i32](7);\n\
             if b.get() != 7 { return 1; }\n\
             b.set(100);\n\
             if b.get() != 100 { return 2; }\n\
             if b.unwrap() != 100 { return 3; }\n\
             let s = \"boxed-string\".to_string();\n\
             let b2 = box::new::[string](s);\n\
             let recovered: string = b2.unwrap();\n\
             if recovered.len() != (12 as usize) { return 4; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (Phase 2B Box regression?)");
    let bin = dir.join("target/debug/boxr");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "expected all Box checks to pass");
}

/// v0.0.4 Phase 2 Slice 2C: `Arc[T]` — atomically refcounted shared
/// ownership. Two worker threads each hold a clone; parent drops last.
/// TSan + ASan clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_arc_cross_thread_share() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"arct\"\n\n[[bin]]\nname = \"arct\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let arc_src = include_str!("../../vendor/stdlib/src/arc.cplus");
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/arc.cplus"), arc_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/arc\" as arc;\n\
         import \"stdlib/thread\" as thread;\n\
         fn worker(move handle: arc::Arc[i32]) -> i32 {\n\
             return handle.get();\n\
         }\n\
         fn main() -> i32 {\n\
             let root = arc::new::[i32](7);\n\
             let c1 = root.clone();\n\
             let c2 = root.clone();\n\
             let h1: thread::JoinHandle[i32] = thread::spawn_with::[arc::Arc[i32], i32](c1, worker);\n\
             let h2: thread::JoinHandle[i32] = thread::spawn_with::[arc::Arc[i32], i32](c2, worker);\n\
             let r1: i32 = h1.join();\n\
             let r2: i32 = h2.join();\n\
             if r1 != 7 { return 1; }\n\
             if r2 != 7 { return 2; }\n\
             if root.get() != 7 { return 3; }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    // Build under both ASan + TSan to verify the refcount machinery
    // has no double-frees or races.
    for sanitizer in &["", "--asan", "--tsan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        let st = cmd.status().expect("invoke cpc");
        assert!(st.success(), "cpc build failed with {}", sanitizer);
        let bin = dir.join("target/debug/arct");
        let run = Command::new(&bin).output().expect("run");
        assert!(
            run.status.success(),
            "arct exit non-zero with {}: code={:?} stderr={}",
            sanitizer,
            run.status.code(),
            String::from_utf8_lossy(&run.stderr),
        );
    }
}

/// v0.0.4 Phase 2 Slice 2D: `Rc[T]` — single-threaded refcounted
/// shared ownership. Same shape as `Arc[T]`, non-atomic refcount.
/// 3-deep clone chain rounds-trips ASan-clean.
#[test]
fn stdlib_rc_clone_chain_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"rcr\"\n\n[[bin]]\nname = \"rcr\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let rc_src = include_str!("../../vendor/stdlib/src/rc.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/rc.cplus"), rc_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/rc\" as rc;\n\
         fn main() -> i32 {\n\
             let a = rc::new::[i32](42);\n\
             if a.get() != 42 { return 1; }\n\
             if a.strong_count() != (1 as u64) { return 2; }\n\
             let b = a.clone();\n\
             if a.strong_count() != (2 as u64) { return 3; }\n\
             let c = b.clone();\n\
             if c.strong_count() != (3 as u64) { return 4; }\n\
             if c.get() != 42 { return 5; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (Phase 2D Rc regression?)");
    let bin = dir.join("target/debug/rcr");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "expected Rc round-trip to pass");
}

/// v0.0.4 Phase 2 Slice 2E: `Mutex[T]` — pthread-backed mutual
/// exclusion with an internal refcount. Two worker threads each
/// acquire the lock, increment, drop; parent verifies final value =
/// initial + 2. TSan + ASan clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_mutex_cross_thread_increment() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mux\"\n\n[[bin]]\nname = \"mux\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let mutex_src = include_str!("../../vendor/stdlib/src/mutex.cplus");
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/mutex.cplus"), mutex_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/mutex\" as mutex;\n\
         import \"stdlib/thread\" as thread;\n\
         fn worker(move m: mutex::Mutex[i32]) -> i32 {\n\
             let mut g = m.lock();\n\
             let cur: i32 = g.get();\n\
             g.set(cur + 1);\n\
             return 0;\n\
         }\n\
         fn main() -> i32 {\n\
             let root = mutex::new::[i32](10);\n\
             let c1 = root.clone();\n\
             let c2 = root.clone();\n\
             let h1: thread::JoinHandle[i32] = thread::spawn_with::[mutex::Mutex[i32], i32](c1, worker);\n\
             let h2: thread::JoinHandle[i32] = thread::spawn_with::[mutex::Mutex[i32], i32](c2, worker);\n\
             let _r1: i32 = h1.join();\n\
             let _r2: i32 = h2.join();\n\
             let final_val: i32 = {\n\
                 let g = root.lock();\n\
                 g.get()\n\
             };\n\
             if final_val != 12 { return 1; }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    for sanitizer in &["", "--asan", "--tsan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        let st = cmd.status().expect("invoke cpc");
        assert!(st.success(), "cpc build failed with {}", sanitizer);
        let bin = dir.join("target/debug/mux");
        let run = Command::new(&bin).output().expect("run");
        assert!(
            run.status.success(),
            "mux exit non-zero with {}: code={:?} stderr={}",
            sanitizer,
            run.status.code(),
            String::from_utf8_lossy(&run.stderr),
        );
    }
}

/// v0.0.4 Phase 2 Slice 2F: `Channel[T]` — MPMC FIFO between threads.
///
/// Two producers each push 100 values; two consumers drain until Closed.
/// Verifies the channel under genuine multi-producer / multi-consumer
/// contention. Runs ASan + TSan clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_channel_mpmc_stress() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"ch\"\n\n[[bin]]\nname = \"ch\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let channel_src = include_str!("../../vendor/stdlib/src/channel.cplus");
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/channel.cplus"), channel_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/channel\" as channel;\n\
         import \"stdlib/thread\" as thread;\n\
         fn producer(move ch: channel::Channel[i32]) -> i32 {\n\
             let mut i: i32 = 0;\n\
             while i < 100 {\n\
                 ch.send(i);\n\
                 i = i +% 1;\n\
             }\n\
             return 0;\n\
         }\n\
         fn consumer(move ch: channel::Channel[i32]) -> i32 {\n\
             let mut count: i32 = 0;\n\
             let mut done: bool = false;\n\
             while !done {\n\
                 match ch.recv() {\n\
                     channel::RecvResult[i32]::Value(_v) => { count = count +% 1; },\n\
                     channel::RecvResult[i32]::Closed => { done = true; },\n\
                 }\n\
             }\n\
             return count;\n\
         }\n\
         fn main() -> i32 {\n\
             let root = channel::new::[i32]();\n\
             let p1 = root.clone();\n\
             let p2 = root.clone();\n\
             let c1 = root.clone();\n\
             let c2 = root.clone();\n\
             let hp1: thread::JoinHandle[i32] = thread::spawn_with::[channel::Channel[i32], i32](p1, producer);\n\
             let hp2: thread::JoinHandle[i32] = thread::spawn_with::[channel::Channel[i32], i32](p2, producer);\n\
             let hc1: thread::JoinHandle[i32] = thread::spawn_with::[channel::Channel[i32], i32](c1, consumer);\n\
             let hc2: thread::JoinHandle[i32] = thread::spawn_with::[channel::Channel[i32], i32](c2, consumer);\n\
             let _r1: i32 = hp1.join();\n\
             let _r2: i32 = hp2.join();\n\
             root.close();\n\
             let cnt1: i32 = hc1.join();\n\
             let cnt2: i32 = hc2.join();\n\
             let total: i32 = cnt1 +% cnt2;\n\
             if total != 200 { return 1; }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    for sanitizer in &["", "--asan", "--tsan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        let st = cmd.status().expect("invoke cpc");
        assert!(st.success(), "cpc build failed with {}", sanitizer);
        let bin = dir.join("target/debug/ch");
        let run = Command::new(&bin).output().expect("run");
        assert!(
            run.status.success(),
            "channel test exit non-zero with {}: code={:?} stderr={}",
            sanitizer,
            run.status.code(),
            String::from_utf8_lossy(&run.stderr),
        );
    }
}

/// v0.0.4 Phase 2 Slice 2G: `CowStr` — clone-on-write string wrapper.
///
/// Two variants: View(str) borrows caller's bytes; Owned(string) owns
/// a heap buffer. `into_owned(move c)` allocates+copies on the View
/// path; hands over the buffer on the Owned path. ASan-clean.
#[test]
fn stdlib_cow_str_view_and_owned_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"cowr\"\n\n[[bin]]\nname = \"cowr\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let cow_src = include_str!("../../vendor/stdlib/src/cow.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/cow.cplus"), cow_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/cow\" as cow;\n\
         fn main() -> i32 {\n\
             let c1 = cow::from_view(\"hello\");\n\
             if cow::is_owned(c1) { return 1; }\n\
             if cow::len(c1) != (5 as usize) { return 2; }\n\
             let initial = \"world\".to_string();\n\
             let c2 = cow::from_owned(initial);\n\
             if !cow::is_owned(c2) { return 3; }\n\
             if cow::len(c2) != (5 as usize) { return 4; }\n\
             let c3 = cow::from_view(\"abc\");\n\
             let s3 = cow::into_owned(c3);\n\
             if s3.len() != (3 as usize) { return 5; }\n\
             let init2 = \"xyzpq\".to_string();\n\
             let c4 = cow::from_owned(init2);\n\
             let s4 = cow::into_owned(c4);\n\
             if s4.len() != (5 as usize) { return 6; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 2G CowStr regression?)"
    );
    let bin = dir.join("target/debug/cowr");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "expected all CowStr checks to pass");
}

/// v0.0.4 Phase 2 Slice 2H: JoinHandle::drop is non-blocking. Spawn a
/// worker that runs for ~200ms; drop the handle immediately; verify the
/// parent returns from the dropping scope in well under that. Sleep at
/// the end so the worker has time to finish cleanly under ASan.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_drop_is_non_blocking() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"detach_fast\"\n\n[[bin]]\nname = \"detach_fast\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    // Worker spins for a measurable amount of time (~200ms on this machine);
    // parent drops the handle immediately and reports elapsed ms. With
    // fire-and-forget detach the drop returns in microseconds — well below
    // any sane threshold. With the old blocking-join Drop, this would
    // return ~200ms.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         extern fn usleep(us: u32) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         #[repr(C)]\n\
         struct Ts { sec: i64, ns: i64 }\n\
         extern fn clock_gettime(clk: i32, ts: *Ts) -> i32;\n\
         fn now_ns() -> i64 {\n\
             let raw: *u8 = unsafe { malloc(16 as usize) };\n\
             let p: *Ts = unsafe { raw as *Ts };\n\
             let _r: i32 = unsafe { clock_gettime(6 as i32, p) };\n\
             let s: i64 = unsafe { p[0].sec };\n\
             let n: i64 = unsafe { p[0].ns };\n\
             unsafe { free(raw); }\n\
             return s *% (1000000000 as i64) +% n;\n\
         }\n\
         fn slow_worker() -> i32 {\n\
             let _r: i32 = unsafe { usleep(200000 as u32) };\n\
             return 0 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let t0: i64 = now_ns();\n\
             {\n\
                 let h: thread::JoinHandle[i32] = thread::spawn::[i32](slow_worker);\n\
                 // h goes out of scope here — Drop should NOT block on the worker.\n\
             }\n\
             let t1: i64 = now_ns();\n\
             let elapsed_us: i64 = (t1 -% t0) / (1000 as i64);\n\
             // Give the worker time to finish cleanly so ASan doesn't see\n\
             // the process exit with a still-running thread.\n\
             let _r: i32 = unsafe { usleep(250000 as u32) };\n\
             // Return 0 if drop was non-blocking (< 50ms), else the\n\
             // elapsed ms clamped to i32.\n\
             if elapsed_us > (50000 as i64) {\n\
                 return (elapsed_us / (1000 as i64)) as i32;\n\
             }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build --asan failed");
    let bin = dir.join("target/debug/detach_fast");
    let run = Command::new(&bin).output().expect("run");
    let code = run.status.code();
    assert_eq!(
        code,
        Some(0),
        "drop blocked for {:?} ms (expected non-blocking < 50ms); stderr={}",
        code,
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stderr.contains("AddressSanitizer"),
        "expected ASan-clean run, got:\n{stderr}"
    );
}

/// v0.0.4 Phase 3 Slice 3A.2: executor::yield_now round-trips through
/// v0.0.4 Phase 4 Slice 4A/4B/4C: `gen fn` + `Iterator[T]::next()` +
/// `for x in iter { ... }` round-trip. The generator coroutine yields
/// values 1..=5; the for-in lowering walks the iterator inline (no
/// per-iteration Option allocation), summing into `total`. Validates
/// every Phase 4 surface in one shot.
#[test]
fn phase4_gen_fn_for_in_round_trips() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"genf\"\n\n[[bin]]\nname = \"genf\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/iterator\" as iter;\n\
         import \"stdlib/option\" as option;\n\
         gen fn count_up(n: i32) -> i32 {\n\
             let mut i: i32 = 1;\n\
             while i <= n {\n\
                 yield i;\n\
                 i = i +% (1 as i32);\n\
             }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             // Path 1: `for x in iter` desugar.\n\
             let mut sum: i32 = 0;\n\
             for x in count_up(5 as i32) {\n\
                 sum = sum +% x;\n\
             }\n\
             if sum != (15 as i32) { return 1 as i32; }\n\
             // Path 2: explicit `it.next()` pull-style consumption.\n\
             let mut it: iter::Iterator[i32] = count_up(3 as i32);\n\
             let mut pulled: i32 = 0;\n\
             let mut loops: i32 = 0;\n\
             while loops < (10 as i32) {\n\
                 match it.next() {\n\
                     option::Option[i32]::Some(v) => { pulled = pulled +% v; }\n\
                     option::Option[i32]::None => {\n\
                         if pulled != (6 as i32) { return 2 as i32; }\n\
                         return 0 as i32;\n\
                     }\n\
                 }\n\
                 loops = loops +% (1 as i32);\n\
             }\n\
             return 3 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (gen fn / for-in)");
    let bin = dir.join("target/debug/genf");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "gen fn + for-in round-trip mismatched");
}

/// the reactor's pending queue. Each `yield_now()` enqueues self and
/// suspends; block_on's drain step resumes us. Counts to N to prove
/// the loop actually advances.
#[test]
fn stdlib_executor_yield_now_round_trips() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"yt\"\n\n[[bin]]\nname = \"yt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         async fn count_with_yields() -> i32 {\n\
             let mut i: i32 = 0;\n\
             while i < 5 {\n\
                 executor::yield_now();\n\
                 i = i +% 1;\n\
             }\n\
             return i;\n\
         }\n\
         fn main() -> i32 {\n\
             let f: future::Future[i32] = count_with_yields();\n\
             return executor::block_on::[i32](f);\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (yield_now regression?)");
    let bin = dir.join("target/debug/yt");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(5), "expected 5 yield round-trips");
}

/// v0.0.4 Phase 3 Slice 3A.1: reactor wait-fd-readable. Open a pipe,
/// write a byte to the write end, then await `wait_read` on the read
/// end. The reactor's kevent_wait should return immediately (fd is
/// already readable), resume the coroutine, and we read the byte.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_reactor_wait_fd_readable_kqueue_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"rwf\"\n\n[[bin]]\nname = \"rwf\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         extern fn pipe(fds: *u8) -> i32;\n\
         extern fn read(fd: i32, buf: *u8, count: usize) -> isize;\n\
         extern fn write(fd: i32, buf: *u8, count: usize) -> isize;\n\
         extern fn close(fd: i32) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         async fn await_and_read(rfd: i32) -> i32 {\n\
             unsafe { __cplus_reactor_wait_read(rfd); }\n\
             let buf: *u8 = unsafe { malloc(1 as usize) };\n\
             let n: isize = unsafe { read(rfd, buf, 1 as usize) };\n\
             let v: u8 = unsafe { *buf };\n\
             unsafe { free(buf); }\n\
             if n != (1 as isize) { return -1 as i32; }\n\
             return v as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = unsafe { malloc(8 as usize) };\n\
             let _r: i32 = unsafe { pipe(fds_buf) };\n\
             let fds_i32: *i32 = unsafe { fds_buf as *i32 };\n\
             let rfd: i32 = unsafe { *fds_i32 };\n\
             let wfd_p: *i32 = unsafe { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = unsafe { *wfd_p };\n\
             let payload: *u8 = unsafe { malloc(1 as usize) };\n\
             unsafe { *payload = 42 as u8; }\n\
             let _w: isize = unsafe { write(wfd, payload, 1 as usize) };\n\
             unsafe { free(payload); }\n\
             let f: future::Future[i32] = await_and_read(rfd);\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             unsafe { close(rfd); }\n\
             unsafe { close(wfd); }\n\
             unsafe { free(fds_buf); }\n\
             return got;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (reactor wait_read regression?)"
    );
    let bin = dir.join("target/debug/rwf");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(42),
        "expected reactor to wake + read byte 42"
    );
}

/// v0.0.5 Phase 3 Slice 3D: `File::lines()` end-to-end. Writes a small
/// multi-line file via raw libc, then iterates via the gen method:
///   `for line in f.lines() { ... }`
/// Validates the chunk-and-carry newline scanner: line A ('a'), line B
/// ('bc'), final fragment 'd' (no trailing \n at EOF) all yielded as
/// owned `string` values.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_fs_file_lines_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"flt\"\n\n[[bin]]\nname = \"flt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let fs_src = include_str!("../../vendor/stdlib/src/fs.cplus");
    let net_src = include_str!("../../vendor/stdlib/src/net.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/fs.cplus"), fs_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
    // net.cplus imports stdlib/netsys for platform syscall constants; the
    // resolver loads netsys_linux.cplus on Linux. Stage both so the fixture
    // resolves on either OS.
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys.cplus"),
        include_str!("../../vendor/stdlib/src/netsys.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys_linux.cplus"),
        include_str!("../../vendor/stdlib/src/netsys_linux.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    // Each test gets its own temp file to avoid cross-test interference.
    let test_file = dir.join("input.txt");
    std::fs::write(&test_file, "alpha\nbeta beta\ngamma").unwrap();
    let test_file_str = test_file.to_str().unwrap();
    let main = format!(
        "import \"stdlib/fs\" as fs;\n\
         import \"stdlib/result\" as result;\n\
         fn main() -> i32 {{\n\
             guard let result::Result[fs::File, result::IoError]::Ok(f) = fs::open_read(\"{test_file_str}\")\n\
                 else {{ return 1 as i32; }};\n\
             let mut count: i32 = 0;\n\
             let mut total_len: i32 = 0;\n\
             for line in f.lines() {{\n\
                 count = count +% (1 as i32);\n\
                 total_len = total_len +% (line.len() as i32);\n\
             }}\n\
             // 3 lines: \"alpha\"(5), \"beta beta\"(9), \"gamma\"(5) = 19 bytes total.\n\
             if count != (3 as i32) {{ return 2 as i32; }}\n\
             if total_len != (19 as i32) {{ return 3 as i32; }}\n\
             return 0 as i32;\n\
         }}\n",
    );
    std::fs::write(dir.join("src/main.cplus"), main).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 3 Slice 3D regression?)"
    );
    let bin = dir.join("target/debug/flt");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "expected 3 lines totaling 19 bytes");
}

/// v0.0.5 Phase 4 Slice 4C: `File::read_async` round-trip. Same EAGAIN-
/// suspend/resume shape as `read_fd_async` but accessed through the
/// method form. Uses a pipe stand-in (kqueue doesn't fire EVFILT_READ
/// on regular-file fds — they're always immediately "ready") wrapped
/// in a `File { fd }`-shaped harness so the method dispatch + reactor
/// integration are both exercised.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_fs_file_read_async_compiles() {
    // The fs::File constructor (`open_read`) requires a real path; pipe
    // fds can't be wrapped without a public `File { fd }` constructor
    // (the field is private). For now, smoke-test that the method form
    // compiles cleanly — runtime exercise lives in
    // `stdlib_net_read_fd_async_eagain_round_trip` for the free fn.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"fra\"\n\n[[bin]]\nname = \"fra\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let fs_src = include_str!("../../vendor/stdlib/src/fs.cplus");
    let net_src = include_str!("../../vendor/stdlib/src/net.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/fs.cplus"), fs_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
    // net.cplus imports stdlib/netsys for platform syscall constants; the
    // resolver loads netsys_linux.cplus on Linux. Stage both so the fixture
    // resolves on either OS.
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys.cplus"),
        include_str!("../../vendor/stdlib/src/netsys.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys_linux.cplus"),
        include_str!("../../vendor/stdlib/src/netsys_linux.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    let test_file = dir.join("input.txt");
    std::fs::write(&test_file, "x").unwrap();
    let test_file_str = test_file.to_str().unwrap();
    let main = format!(
        "import \"stdlib/fs\" as fs;\n\
         import \"stdlib/result\" as result;\n\
         import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         async fn read_first(move f: fs::File) -> i32 {{\n\
             // Re-bind locally so the body has a `mut` handle without\n\
             // tripping the E0900 (mut-pointer-pass + await) guard.\n\
             let mut f: fs::File = f;\n\
             let _nb: i32 = f.make_nonblocking();\n\
             let buf: *u8 = unsafe {{ malloc(1 as usize) }};\n\
             let n: isize = await f.read_async(buf, 1 as usize);\n\
             let v: u8 = unsafe {{ *buf }};\n\
             unsafe {{ free(buf); }}\n\
             if n != (1 as isize) {{ return 0 -% 1 as i32; }}\n\
             return v as i32;\n\
         }}\n\
         fn main() -> i32 {{\n\
             guard let result::Result[fs::File, result::IoError]::Ok(f) = fs::open_read(\"{test_file_str}\")\n\
                 else {{ return 1 as i32; }};\n\
             let fut: future::Future[i32] = read_first(f);\n\
             let got: i32 = executor::block_on::[i32](fut);\n\
             if got != (0x78 as i32) {{ return 2 as i32; }}\n\
             return 0 as i32;\n\
         }}\n",
    );
    std::fs::write(dir.join("src/main.cplus"), main).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 4 Slice 4C regression?)"
    );
    let bin = dir.join("target/debug/fra");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected to read 'x' (0x78) asynchronously"
    );
}

/// v0.0.5 Phase 3 Slice 3B: tuple types end-to-end. Exercises
///   - Tuple type in fn return position: `fn make_pair(...) -> (i32, i32)`
///   - Tuple literal expression: `(x, y)`
///   - Numeric field projection: `pair.0`, `pair.1`
///   - 3-tuples (arity > 2)
///   - Mixed element types: `(i32, bool)`
///
/// Tuples lower to synthesized concrete structs (`__tuple_<t1>_<t2>_...`)
/// at sema time; codegen reconstructs the matching struct from element
/// types and emits the same insertvalue/load shape as a struct literal.
#[test]
fn phase3b_tuple_construct_projection_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"tup\"\n\n[[bin]]\nname = \"tup\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn make_pair(x: i32, y: i32) -> (i32, i32) {\n\
             return (x, y);\n\
         }\n\
         fn main() -> i32 {\n\
             // 2-tuple round-trip: construct via fn return, project via .0/.1.\n\
             let p: (i32, i32) = make_pair(7 as i32, 35 as i32);\n\
             let sum: i32 = p.0 +% p.1;\n\
             if sum != (42 as i32) { return 1 as i32; }\n\
             // 3-tuple, inline literal.\n\
             let t: (i32, i32, i32) = (1 as i32, 2 as i32, 3 as i32);\n\
             let s: i32 = t.0 +% t.1 +% t.2;\n\
             if s != (6 as i32) { return 2 as i32; }\n\
             // Mixed element types — exercises the per-element type\n\
             // mangling path in tuple_struct_name.\n\
             let mixed: (i32, bool) = (99 as i32, true);\n\
             if !mixed.1 { return 3 as i32; }\n\
             if mixed.0 != (99 as i32) { return 4 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 3 Slice 3B regression?)"
    );
    let bin = dir.join("target/debug/tup");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "tuple construct + project should round-trip"
    );
}

/// v0.0.5 Phase 4 Slice 4F: concurrent-async stress. Spawns N
/// `time::sleep(50)` futures eagerly (each runs to its first
/// wait_timer + suspends), then awaits each in sequence. With the
/// awaiter-notification fix, all N timers run concurrently — total
/// wall time is ~max(individual delay), not Σ.
///
/// Without 4F, this hangs: the outer's `await futs[i]` suspends, the
/// inner sleep's timer fires and inner completes, but the outer never
/// gets re-resumed (only the timer's coro was resumed by
/// `poll_one_event`, not its awaiter).
///
/// Stores `Future[i32]` handles as raw `*u8` in a malloc'd array to
/// work around the nested-generic `Vec[Future[i32]]` limitation
/// (sema's ty_to_source_name renders inner struct types as
/// `<concrete>`); re-wraps as `Future[i32] { handle: h }` at await
/// time via the struct's `pub handle` field.
#[test]
#[cfg(target_os = "macos")]
fn phase4f_concurrent_n_sleeps_stress() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"cns\"\n\n[[bin]]\nname = \"cns\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let time_src = include_str!("../../vendor/stdlib/src/time.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/time.cplus"), time_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/time\" as time;\n\
         import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         extern fn gettimeofday(tv: *u8, tz: *u8) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn now_ms() -> u64 {\n\
             let buf: *u8 = unsafe { malloc(16 as usize) };\n\
             let _rc: i32 = unsafe { gettimeofday(buf, 0 as *u8) };\n\
             let sec: i64 = unsafe { *(buf as *i64) };\n\
             let usec: i64 = unsafe { *((buf + (8 as usize)) as *i64) };\n\
             unsafe { free(buf); }\n\
             return ((sec *% (1000 as i64)) +% (usec / (1000 as i64))) as u64;\n\
         }\n\
         async fn unit_sleep() -> i32 {\n\
             await time::sleep(50 as u64);\n\
             return 0 as i32;\n\
         }\n\
         async fn stress(n: i32) -> i32 {\n\
             let bytes: usize = (n as usize) *% (8 as usize);\n\
             let buf: *u8 = unsafe { malloc(bytes) };\n\
             let hdls: **u8 = unsafe { buf as **u8 };\n\
             let mut i: i32 = 0;\n\
             while i < n {\n\
                 let f: future::Future[i32] = unit_sleep();\n\
                 let slot: **u8 = unsafe { hdls + (i as usize) };\n\
                 unsafe { *slot = f.handle; }\n\
                 i = i +% (1 as i32);\n\
             }\n\
             let mut j: i32 = 0;\n\
             while j < n {\n\
                 let slot: **u8 = unsafe { hdls + (j as usize) };\n\
                 let h: *u8 = unsafe { *slot };\n\
                 let f: future::Future[i32] = future::Future[i32] { handle: h };\n\
                 let _r: i32 = await f;\n\
                 j = j +% (1 as i32);\n\
             }\n\
             unsafe { free(buf); }\n\
             return 0 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let t0: u64 = now_ms();\n\
             let _r: i32 = executor::block_on::[i32](stress(50 as i32));\n\
             let t1: u64 = now_ms();\n\
             let elapsed: u64 = t1 -% t0;\n\
             // Concurrent: ~50ms + overhead. Sequential would be 50*50 = 2500ms.\n\
             if elapsed < (40 as u64) { return 1 as i32; }\n\
             if elapsed > (500 as u64) { return 2 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 4 Slice 4F regression?)"
    );
    let bin = dir.join("target/debug/cns");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected 50 concurrent sleeps to complete in ~50ms (not sequential ~2500ms)"
    );
}

/// v0.0.5 Phase 4 Slice 4B: async method form on a user-defined struct.
/// Exercises the new `gen_async_method` codegen path end-to-end:
/// `mut self` is pointer-passed (not consumed), the method body runs
/// inside an LLVM coroutine that returns `Future[T]`, and `block_on`
/// drives it through the reactor just like a free async fn would.
/// Mirror of the existing `stdlib_net_read_fd_async_eagain_round_trip`
/// shape, but threading the read through a method call instead of a
/// free-fn call.
#[test]
#[cfg(target_os = "macos")]
fn async_method_on_user_struct_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"asm\"\n\n[[bin]]\nname = \"asm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let net_src = include_str!("../../vendor/stdlib/src/net.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
    // net.cplus imports stdlib/netsys for platform syscall constants; the
    // resolver loads netsys_linux.cplus on Linux. Stage both so the fixture
    // resolves on either OS.
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys.cplus"),
        include_str!("../../vendor/stdlib/src/netsys.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys_linux.cplus"),
        include_str!("../../vendor/stdlib/src/netsys_linux.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         import \"stdlib/net\" as net;\n\
         extern fn pipe(fds: *u8) -> i32;\n\
         extern fn write(fd: i32, buf: *u8, count: usize) -> isize;\n\
         extern fn close(fd: i32) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         struct PipeReader { fd: i32 }\n\
         impl PipeReader {\n\
             pub async fn read_byte(mut self) -> i32 {\n\
                 let buf: *u8 = unsafe { malloc(1 as usize) };\n\
                 let n: isize = await net::read_fd_async(self.fd, buf, 1 as usize);\n\
                 let v: u8 = unsafe { *buf };\n\
                 unsafe { free(buf); }\n\
                 if n != (1 as isize) { return -1 as i32; }\n\
                 return v as i32;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = unsafe { malloc(8 as usize) };\n\
             let _r: i32 = unsafe { pipe(fds_buf) };\n\
             let fds_i32: *i32 = unsafe { fds_buf as *i32 };\n\
             let rfd: i32 = unsafe { *fds_i32 };\n\
             let wfd_p: *i32 = unsafe { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = unsafe { *wfd_p };\n\
             let nb: i32 = net::set_nonblocking(rfd);\n\
             if nb != (0 as i32) { return 90 as i32; }\n\
             let mut reader: PipeReader = PipeReader { fd: rfd };\n\
             let f: future::Future[i32] = reader.read_byte();\n\
             let payload: *u8 = unsafe { malloc(1 as usize) };\n\
             unsafe { *payload = 42 as u8; }\n\
             let _w: isize = unsafe { write(wfd, payload, 1 as usize) };\n\
             unsafe { free(payload); }\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             let _c1: i32 = unsafe { close(rfd) };\n\
             let _c2: i32 = unsafe { close(wfd) };\n\
             unsafe { free(fds_buf); }\n\
             return got;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 4 Slice 4B regression?)"
    );
    let bin = dir.join("target/debug/asm");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(42),
        "expected async method call to drive reactor + return read byte 42"
    );
}

/// v0.0.5 Phase 4 Slice 4A: `time::sleep(ms)` round-trip via kqueue
/// EVFILT_TIMER. Drives the reactor's timer path end-to-end:
///   - `time::sleep(80ms)` translates to `__cplus_reactor_wait_timer(80)`
///     inside an `async fn`.
///   - Codegen emits `stdlib_reactor_register_timer_v1(80, %.coro.hdl)`
///     then suspends self via `llvm.coro.suspend`.
///   - Reactor submits an EVFILT_TIMER one-shot kevent with ident set
///     to the handle pointer.
///   - `block_on`'s drive loop sees `waiter_count() > 0` (n_timers > 0),
///     calls `poll_one_event` which blocks in kevent until the timer
///     fires, reads ident back as the handle, resumes the coroutine.
/// Verifies elapsed wall-clock time is bounded loosely (70..500 ms),
/// proving the suspend really blocked rather than busy-looping.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_time_sleep_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"slp\"\n\n[[bin]]\nname = \"slp\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let time_src = include_str!("../../vendor/stdlib/src/time.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/time.cplus"), time_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/time\" as time;\n\
         import \"stdlib/executor\" as executor;\n\
         extern fn gettimeofday(tv: *u8, tz: *u8) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn now_ms() -> u64 {\n\
             let buf: *u8 = unsafe { malloc(16 as usize) };\n\
             let _rc: i32 = unsafe { gettimeofday(buf, 0 as *u8) };\n\
             let sec: i64 = unsafe { *(buf as *i64) };\n\
             let usec: i64 = unsafe { *((buf + (8 as usize)) as *i64) };\n\
             unsafe { free(buf); }\n\
             return ((sec *% (1000 as i64)) +% (usec / (1000 as i64))) as u64;\n\
         }\n\
         async fn do_sleep(ms: u64) -> i32 {\n\
             await time::sleep(ms);\n\
             return 0 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let t0: u64 = now_ms();\n\
             let _r: i32 = executor::block_on::[i32](do_sleep(80 as u64));\n\
             let t1: u64 = now_ms();\n\
             let elapsed: u64 = t1 -% t0;\n\
             if elapsed < (70 as u64) { return 1 as i32; }\n\
             if elapsed > (500 as u64) { return 2 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 4 Slice 4A regression?)"
    );
    let bin = dir.join("target/debug/slp");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected ~80ms sleep to complete within bounds"
    );
}

/// v0.0.4 Phase 3 Slice 3A.3: stdlib `net::read_fd_async` round-trip.
/// Exercises the full async-wrapper EAGAIN path:
///   - `set_nonblocking(rfd)` flips O_NONBLOCK via fcntl.
///   - `read_fd_async(rfd, buf, 1)` syscalls, gets EAGAIN, registers
///     with the reactor's wait_read filter, suspends the coroutine.
///   - block_on's drive loop runs drain_pending (writer task pushes
///     the byte synchronously into the pipe), then poll_one_event
///     fires kevent_wait, which returns immediately because the pipe
///     became readable. Reader is resumed, retries the read, returns 1.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_net_read_fd_async_eagain_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"rfa\"\n\n[[bin]]\nname = \"rfa\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let net_src = include_str!("../../vendor/stdlib/src/net.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
    // net.cplus imports stdlib/netsys for platform syscall constants; the
    // resolver loads netsys_linux.cplus on Linux. Stage both so the fixture
    // resolves on either OS.
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys.cplus"),
        include_str!("../../vendor/stdlib/src/netsys.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys_linux.cplus"),
        include_str!("../../vendor/stdlib/src/netsys_linux.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         import \"stdlib/net\" as net;\n\
         extern fn pipe(fds: *u8) -> i32;\n\
         extern fn write(fd: i32, buf: *u8, count: usize) -> isize;\n\
         extern fn close(fd: i32) -> i32;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         async fn reader(rfd: i32) -> i32 {\n\
             let buf: *u8 = unsafe { malloc(1 as usize) };\n\
             let n: isize = await net::read_fd_async(rfd, buf, 1 as usize);\n\
             let v: u8 = unsafe { *buf };\n\
             unsafe { free(buf); }\n\
             if n != (1 as isize) { return -1 as i32; }\n\
             return v as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = unsafe { malloc(8 as usize) };\n\
             let _r: i32 = unsafe { pipe(fds_buf) };\n\
             let fds_i32: *i32 = unsafe { fds_buf as *i32 };\n\
             let rfd: i32 = unsafe { *fds_i32 };\n\
             let wfd_p: *i32 = unsafe { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = unsafe { *wfd_p };\n\
             let nb: i32 = net::set_nonblocking(rfd);\n\
             if nb != (0 as i32) { return 90 as i32; }\n\
             // Start the reader coroutine; reactor body runs eagerly,\n\
             // hits EAGAIN on the empty pipe, registers a waiter, suspends.\n\
             let f: future::Future[i32] = reader(rfd);\n\
             // Now write the byte synchronously. kqueue's EVFILT_READ on\n\
             // rfd will fire when block_on calls kevent_wait below.\n\
             let payload: *u8 = unsafe { malloc(1 as usize) };\n\
             unsafe { *payload = 42 as u8; }\n\
             let _w: isize = unsafe { write(wfd, payload, 1 as usize) };\n\
             unsafe { free(payload); }\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             let _c1: i32 = unsafe { close(rfd) };\n\
             let _c2: i32 = unsafe { close(wfd) };\n\
             unsafe { free(fds_buf); }\n\
             return got;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (net::read_fd_async)");
    let bin = dir.join("target/debug/rfa");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(42),
        "expected reactor EAGAIN→wait_read→resume to yield byte 42"
    );
}

/// v0.0.3 Slice 1P.1: cross-module generic enum construction
/// `result::Result[i32, i32]::Ok(42)` and the matching pattern
/// `result::Result[i32, i32]::Ok(v)` work end-to-end.
#[test]
fn stdlib_qualified_generic_enum_construct_and_match() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"qge\"\n\n[[bin]]\nname = \"qge\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/result\" as result;\n\
         fn main() -> i32 {\n\
             let r: result::Result[i32, i32] = result::Result[i32, i32]::Ok(42 as i32);\n\
             return match r {\n\
                 result::Result[i32, i32]::Ok(v) => v,\n\
                 result::Result[i32, i32]::Err(_) => 0 -% 1 as i32,\n\
             };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/qge");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected 42");
}

/// A project that depends on `stdlib` can `import "stdlib/vec"` and use the
/// v0.0.5 Phase 1B: block-tail `Ident(name)` of a non-Copy binding moves
/// the value out of the block instead of dropping it twice. The bug:
/// `let f: string = { let inner: string = ...; inner };` would free
/// `inner`'s heap at the block's scope exit, then dangle into `f`'s
/// slot, then double-free at `f`'s scope exit. Fix: pre-mark the
/// tail Ident as moved (Runtime drop disposition), then flip the
/// flag in `gen_block_expr` before the inner scope tears down.
/// v0.0.5 Slice 1A: `fn echo(x: string) -> string { return x; }` was the
/// long-open double-free footgun documented in plan.md. The caller's `s`
/// flowed into `echo` as a value-passed aggregate (heap pointer shared
/// with the caller); `return x` lifted that pointer into the caller's
/// result binding `t`; at scope exit, both `s` and `t` Dropped the same
/// heap → SIGTRAP (exit 133 on darwin).
///
/// The fix (codegen-side auto-clone): when `StmtKind::Return` sees a
/// bare-Ident return of a non-`move` `string` parameter, emit a deep
/// copy into the result slot. Both ends now own independent heaps.

#[test]
/// v0.0.5: `fn max[T: Ord](a, b) -> T` can now be written with the
/// canonical `a.cmp(b)` body. The bound-method dispatch (added to
/// `check_method_call`) resolves `.cmp` against the active `T: Ord`
/// bound's interface signature, so the call type-checks at sema time
/// instead of failing as "no method `cmp` on type `type-param`".
/// Monomorphization then substitutes T → concrete type and the call
/// dispatches to that type's `impl Ord for T` method.
fn generic_max_with_ord_bound_calls_cmp_in_body() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("max.cplus");
    std::fs::write(
        &src,
        "\
struct Point { x: i32, y: i32 }
impl Ord for Point {
    fn cmp(self, other: Point) -> i32 {
        if self.x < other.x { return 0 -% 1; }
        if self.x > other.x { return 1; }
        return 0;
    }
}
fn max[T: Ord](a: T, b: T) -> T {
    if a.cmp(b) < 0 { return b; }
    return a;
}
fn main() -> i32 {
    let p: Point = Point { x: 1, y: 2 };
    let q: Point = Point { x: 3, y: 4 };
    let r: Point = max(p, q);
    return r.x;
}
",
    )
    .unwrap();
    let bin = dir.join("max");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for max[T: Ord] with cmp");
    let run = Command::new(&bin).status().expect("run max");
    assert_eq!(run.code(), Some(3), "max(p, q).x should be 3 (q's x)");
}

#[test]
/// v0.0.5: `<` / `<=` / `>` / `>=` on a generic-parameter operand is
/// rejected at sema time with E0302 and a helpful message pointing at
/// the `.cmp()` idiom. Before this lint, sema let the comparison
/// through (because Ty::Param bodies aren't fully sema-checked), and
/// codegen happily produced `icmp slt %StructTy` — LLVM rejected the
/// IR with the cryptic "icmp requires integer operands" when the user
/// instantiated with a non-numeric type. C+ has no operator
/// overloading (SKILL.md §2.6), so the only correct shape is to call
/// the bound's `cmp(other)` method and compare the i32 result.
fn ordered_compare_on_generic_param_rejected_e0302() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("badmax.cplus");
    std::fs::write(
        &src,
        "\
fn max_lt[T: Ord](a: T, b: T) -> T {
    if a < b { return b; }
    return a;
}
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("badmax");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "cpc should reject `<` on T: Ord");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0302"),
        "expected E0302 in stderr; got: {stderr}"
    );
    assert!(
        stderr.contains("cmp")
            && (stderr.contains("§2.6") || stderr.contains("operator overloading")),
        "diagnostic should point at .cmp() and the §2.6 no-overloading policy; got: {stderr}"
    );
}

#[test]
fn echo_string_param_does_not_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("echo.cplus");
    std::fs::write(
        &src,
        "\
fn echo(x: string) -> string {
    return x;
}
fn main() -> i32 {
    let s: string = \"hello\".to_string();
    let t: string = echo(s);
    if t.len() != (5 as usize) { return 1 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let bin = dir.join("echo");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for echo-double-free regression"
    );
    let run = Command::new(&bin).status().expect("run echo");
    assert_eq!(
        run.code(),
        Some(0),
        "echo(x: string) returning x should not double-free; got exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B: `f32x4` SIMD dot product end-to-end.
#[test]
fn simd_f32x4_dot_product_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dot.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);
    let b: f32x4 = f32x4::new(5.0f32, 6.0f32, 7.0f32, 8.0f32);
    let p: f32x4 = a.mul(b);
    let s: f32 = p.lane(0 as u32) + p.lane(1 as u32) + p.lane(2 as u32) + p.lane(3 as u32);
    if s != 70.0f32 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("dot");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD dot-product e2e");
    let run = Command::new(&bin).status().expect("run dot");
    assert_eq!(
        run.code(),
        Some(0),
        "f32x4 dot product expected 70.0; exit {:?}",
        run.code()
    );
}

/// v0.0.7 Slice 2.2 audit: `u64x2` — the 1B gap among 128-bit 8-byte-lane
/// widths (only `i64x2` shipped). Exercises arithmetic, the
/// umin/umax intrinsics that were just declared, and lane round-trip.
#[test]
fn simd_u64x2_min_max_and_arithmetic_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("u64x2.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: u64x2 = u64x2::new(10 as u64, 5 as u64);
    let b: u64x2 = u64x2::new(3 as u64, 20 as u64);
    let lo: u64x2 = a.min(b);
    let hi: u64x2 = a.max(b);
    if lo.lane(0 as u32) != (3 as u64)  { return 1; }
    if lo.lane(1 as u32) != (5 as u64)  { return 2; }
    if hi.lane(0 as u32) != (10 as u64) { return 3; }
    if hi.lane(1 as u32) != (20 as u64) { return 4; }
    let sum: u64x2 = a.add(b);
    if sum.lane(0 as u32) != (13 as u64) { return 5; }
    if sum.lane(1 as u32) != (25 as u64) { return 6; }
    let mask: u64x2 = a.and(u64x2::splat(0xFF as u64));
    if mask.lane(0 as u32) != (10 as u64) { return 7; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("u64x2");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for u64x2 e2e");
    let run = Command::new(&bin).status().expect("run u64x2");
    assert_eq!(
        run.code(),
        Some(0),
        "u64x2 min/max/arithmetic failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B: `f32x4::fma` + `sqrt` + `to_array` round-trip.
#[test]
fn simd_f32x4_fma_sqrt_and_to_array() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fma.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f32x4 = f32x4::splat(2.0f32);
    let b: f32x4 = f32x4::splat(3.0f32);
    let c: f32x4 = f32x4::splat(1.0f32);
    let r: f32x4 = a.fma(b, c);
    let s: f32x4 = r.sqrt();
    let arr: [f32; 4] = s.to_array();
    if arr[0] < 2.6f32 { return 1; }
    if arr[0] > 2.7f32 { return 2; }
    if arr[3] < 2.6f32 { return 3; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("fma");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD fma+sqrt e2e");
    let run = Command::new(&bin).status().expect("run fma");
    assert_eq!(
        run.code(),
        Some(0),
        "fma+sqrt round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: `f64x2` end-to-end (dot product + fma + sqrt).
#[test]
fn simd_f64x2_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("f64x2.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f64x2 = f64x2::new(3.0, 4.0);
    let b: f64x2 = f64x2::splat(2.0);
    let p: f64x2 = a.mul(b);
    let dot: f64 = p.lane(0 as u32) + p.lane(1 as u32);
    if dot != 14.0 { return 1; }
    let s: f64x2 = a.mul(a).fma(b, b).sqrt();
    if s.lane(0 as u32) < 4.4 { return 2; }
    if s.lane(0 as u32) > 4.5 { return 3; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("f64x2");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for f64x2 e2e");
    let run = Command::new(&bin).status().expect("run f64x2");
    assert_eq!(
        run.code(),
        Some(0),
        "f64x2 dot/fma/sqrt round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: `i32x4` end-to-end (add/sub/mul/abs lanes).
#[test]
fn simd_i32x4_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("i32x4.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: i32x4 = i32x4::new(1, 2, 3, 4);
    let b: i32x4 = i32x4::splat(10);
    let c: i32x4 = a.mul(b);
    let d: i32x4 = c.sub(i32x4::splat(25));
    let f: i32x4 = d.abs();
    let s: i32 = f.lane(0 as u32) + f.lane(1 as u32) + f.lane(2 as u32) + f.lane(3 as u32);
    // |(10-25)| + |(20-25)| + |(30-25)| + |(40-25)| = 15+5+5+15 = 40
    if s != 40 { return 1; }
    let arr: [i32; 4] = f.to_array();
    let g: i32x4 = i32x4::from_array(arr);
    if g.lane(2 as u32) != 5 { return 2; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("i32x4");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for i32x4 e2e");
    let run = Command::new(&bin).status().expect("run i32x4");
    assert_eq!(
        run.code(),
        Some(0),
        "i32x4 add/sub/mul/abs round-trip failed; exit {:?}",
        run.code()
    );
}

/// SIMD Tier-1 (G-037 reinterpret, G-038a int↔float convert): lane-type
/// bitcast and lane-wise int/float conversion, end to end. Covers signed and
/// unsigned source conversion and a 64-bit-lane round trip.
#[test]
fn simd_reinterpret_and_int_float_convert_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("conv.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    // signed int -> float -> int round trip (sitofp / fptosi)
    let a: i32x4 = i32x4::new(0 - 5, 7, 100, 3);
    let back: i32x4 = i32x4::from_float(f32x4::from_int(a));
    if back.lane(0 as u32) != (0 - 5) { return 1; }
    if back.lane(3 as u32) != 3 { return 2; }
    // unsigned -> float -> unsigned: a big u32 stays positive (uitofp/fptoui)
    let u: u32x4 = u32x4::splat(4000000000u32);
    let ui: u32x4 = u32x4::from_float(f32x4::from_int(u));
    if ui.lane(0 as u32) < (2000000000u32) { return 3; }
    // 64-bit lanes (sitofp/fptosi on <2 x i64>/<2 x double>)
    let l: i64x2 = i64x2::new((0 as i64) - (42 as i64), 99 as i64);
    let lb: i64x2 = i64x2::from_float(f64x2::from_int(l));
    if lb.lane(0 as u32) != ((0 as i64) - (42 as i64)) { return 4; }
    // reinterpret: u8 lanes as i8 (no-op width), then i8x16 as i16x8 (bitcast)
    let bytes: u8x16 = u8x16::splat(255u8);
    let signed: i8x16 = i8x16::reinterpret(bytes);
    let shorts: i16x8 = i16x8::reinterpret(signed);
    // 0xFFFF as i16 == -1; first lane must be -1
    if shorts.lane(0 as u32) != ((0 as i16) - (1 as i16)) { return 5; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("conv");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD convert/reinterpret e2e");
    let run = Command::new(&bin).status().expect("run conv");
    assert_eq!(run.code(), Some(0), "SIMD convert/reinterpret failed; exit {:?}", run.code());
}

/// Negative: the SIMD Tier-1 conversions reject shape mismatches with E0324.
#[test]
fn simd_convert_rejects_shape_mismatches() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let cases: &[(&str, &str)] = &[
        // from_int needs an int source of the same lane width
        ("from_int_lane_width", "let a: i16x8 = i16x8::splat(1i16); let _b: f32x4 = f32x4::from_int(a);"),
        // from_int target must be float
        ("from_int_int_target", "let a: i32x4 = i32x4::splat(1); let _b: i32x4 = i32x4::from_int(a);"),
        // from_float target must be int
        ("from_float_float_target", "let a: f32x4 = f32x4::splat(1.0f32); let _b: f32x4 = f32x4::from_float(a);"),
        // reinterpret needs equal total width (128 vs 256 bits)
        ("reinterpret_width", "let a: f64x4 = f64x4::splat(1.0f64); let _b: i8x16 = i8x16::reinterpret(a);"),
    ];
    for (label, body) in cases {
        let src = dir.join(format!("{label}.cplus"));
        std::fs::write(&src, format!("fn main() -> i32 {{ {body} return 0; }}\n")).unwrap();
        let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
        assert!(!out.status.success(), "{label}: expected rejection");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("E0324"), "{label}: expected E0324, got:\n{stderr}");
    }
}

/// SIMD Tier-1 (G-039a/b, G-038b): 64-bit lane types plus the bridges that
/// produce and consume them — low/high (split), combine (join), widen
/// (sext/zext, double lane width), narrow (trunc, half lane width).
#[test]
fn simd_low_high_combine_widen_narrow_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("halves.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let v: i8x16 = i8x16::new(1i8,2i8,3i8,4i8,5i8,6i8,7i8,8i8,
                              9i8,10i8,11i8,12i8,13i8,14i8,15i8,16i8);
    let lo: i8x8 = v.low();
    let hi: i8x8 = v.high();
    let rejoined: i8x16 = lo.combine(hi);
    if rejoined.lane(0 as u32) != 1i8 { return 1; }
    if rejoined.lane(15 as u32) != 16i8 { return 2; }
    if lo.lane(7 as u32) != 8i8 { return 3; }
    if hi.lane(0 as u32) != 9i8 { return 4; }
    // widen i8x8 -> i16x8 sign-extends: -1 stays -1
    let w: i16x8 = i8x8::splat(0i8 - 1i8).widen();
    if w.lane(0 as u32) != (0i16 - 1i16) { return 5; }
    // widen u8x8 -> u16x8 zero-extends: 255 stays positive
    let uw: u16x8 = u8x8::splat(255u8).widen();
    if uw.lane(0 as u32) != 255u16 { return 6; }
    // narrow i16x8 -> i8x8 truncates: 0x1FF -> 0xFF == -1
    let n: i8x8 = i16x8::splat(511i16).narrow();
    if n.lane(0 as u32) != (0i8 - 1i8) { return 7; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("halves");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD low/high/combine/widen/narrow");
    let run = Command::new(&bin).status().expect("run halves");
    assert_eq!(run.code(), Some(0), "SIMD half/widen/narrow failed; exit {:?}", run.code());
}

/// G-036 keystone: a widening integer dot product is now *composable* from
/// Tier-1 primitives (widen + low/high + arithmetic), with no dedicated
/// compiler builtin — and it computes the correct non-wrapping result where a
/// naive `i8.mul` would overflow.
#[test]
fn simd_widening_dot_product_composes() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("qdot.cplus");
    std::fs::write(
        &src,
        "\
fn dot8(a: i8x8, b: i8x8) -> i32 {
    let aw: i16x8 = a.widen();
    let bw: i16x8 = b.widen();
    let prod: i16x8 = aw.mul(bw);
    let plo: i32x4 = prod.low().widen();
    let phi: i32x4 = prod.high().widen();
    return plo.add(phi).sum();
}
fn main() -> i32 {
    // 50 * 3 = 150 overflows i8; the widening path keeps it correct.
    // 8 lanes * 150 = 1200.
    if dot8(i8x8::splat(50i8), i8x8::splat(3i8)) != 1200 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("qdot");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for widening dot product");
    let run = Command::new(&bin).status().expect("run qdot");
    assert_eq!(run.code(), Some(0), "widening dot product wrong; exit {:?}", run.code());
}

/// SIMD Tier-1 (G-040): data-dependent byte table lookup (`vqtbl1q`).
/// `tbl.table(idx)` gathers `tbl[idx[i]]` per lane; out-of-range indices
/// yield 0. The one runtime-index shuffle (swizzle needs literal indices).
#[test]
fn simd_table_lookup_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("tbl.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let t: u8x16 = u8x16::new(10u8,20u8,30u8,40u8,50u8,60u8,70u8,80u8,
                              90u8,100u8,110u8,120u8,130u8,140u8,150u8,160u8);
    // lanes 0,2,15 in range; lane 3 index 200 is out of range -> 0.
    let idx: u8x16 = u8x16::new(0u8,2u8,15u8,200u8, 0u8,0u8,0u8,0u8,
                               0u8,0u8,0u8,0u8, 0u8,0u8,0u8,0u8);
    let r: u8x16 = t.table(idx);
    if r.lane(0 as u32) != 10u8 { return 1; }   // t[0]
    if r.lane(1 as u32) != 30u8 { return 2; }   // t[2]
    if r.lane(2 as u32) != 160u8 { return 3; }  // t[15]
    if r.lane(3 as u32) != 0u8 { return 4; }    // out of range
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("tbl");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD table lookup");
    let run = Command::new(&bin).status().expect("run tbl");
    assert_eq!(run.code(), Some(0), "SIMD table lookup wrong; exit {:?}", run.code());
}

/// W0001 lint: a horizontal `sum`/`product` over narrow integer lanes
/// (the `i8x16.mul().sum()` quant footgun) warns but still compiles — the
/// correct path is `.widen()` first or `simd/integer::dot_i32`. The
/// widening `dot_i32` pipeline (sums i32x4) must stay warning-free.
#[test]
fn simd_narrow_int_sum_warns_but_compiles() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("foot.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
         let a: i8x16 = i8x16::splat(50i8);\n\
         let prod: i8x16 = a.mul(i8x16::splat(50i8));\n\
         return prod.sum() as i32;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(), "W0001 is a warning — must not fail the build");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("W0001"), "expected W0001 warning, got:\n{stderr}");

    // The correct widening sum (i32x4) must NOT warn.
    let ok = dir.join("ok.cplus");
    std::fs::write(
        &ok,
        "fn main() -> i32 { let a: i32x4 = i32x4::splat(5); return a.sum(); }\n",
    )
    .unwrap();
    let out2 = Command::new(cpc).arg("check").arg(&ok).output().expect("invoke cpc");
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(!stderr2.contains("W0001"), "i32x4 sum must not warn, got:\n{stderr2}");
}

/// Negative: `table` requires a 16-byte SIMD table.
#[test]
fn simd_table_rejects_non_byte16() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
         let t: i32x4 = i32x4::splat(1);\n\
         let i: u8x16 = u8x16::splat(0u8);\n\
         let _r = t.table(i);\n\
         return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "table on i32x4 must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0324"), "expected E0324, got:\n{stderr}");
}

/// Negative: widen/narrow reject lane types with no wider/narrower step.
#[test]
fn simd_widen_narrow_reject_invalid() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let cases: &[(&str, &str)] = &[
        ("widen_float", "let a: f32x4 = f32x4::splat(1.0f32); let _b = a.widen();"),
        ("widen_64bit_lane", "let a: i64x2 = i64x2::splat(1i64); let _b = a.widen();"),
        ("narrow_byte_lane", "let a: i8x16 = i8x16::splat(1i8); let _b = a.narrow();"),
    ];
    for (label, body) in cases {
        let src = dir.join(format!("{label}.cplus"));
        std::fs::write(&src, format!("fn main() -> i32 {{ {body} return 0; }}\n")).unwrap();
        let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
        assert!(!out.status.success(), "{label}: expected rejection");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("E0324"), "{label}: expected E0324, got:\n{stderr}");
    }
}

/// v0.0.6 Slice 1B expansion: byte and short SIMD widths
/// (`i8x16`, `i16x8`, `u8x16`, `u16x8`) — completes the 128-bit family.
#[test]
fn simd_byte_and_short_widths_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bs.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    // u8x16: ASCII case-shift idiom.
    let upper: u8x16 = u8x16::splat(65 as u8);
    let delta: u8x16 = u8x16::splat(32 as u8);
    if upper.add(delta).lane(7 as u32) != (97 as u8) { return 1; }
    // i8x16: signed clamp to non-negative.
    let neg: i8x16 = i8x16::splat(-5 as i8);
    if neg.max(i8x16::splat(0 as i8)).lane(15 as u32) != (0 as i8) { return 2; }
    // i16x8: abs + lane reduction shape.
    let mixed: i16x8 = i16x8::new(
        10 as i16, -20 as i16, 30 as i16, -40 as i16,
        5 as i16, -5 as i16, 1 as i16, -1 as i16,
    );
    if mixed.abs().lane(3 as u32) != (40 as i16) { return 3; }
    // u16x8: bit-shift + mask round-trip.
    let v: u16x8 = u16x8::splat(0xABCD as u16);
    if v.shr(8 as u32).lane(0 as u32) != (0x00AB as u16) { return 4; }
    if v.and(u16x8::splat(0x00FF as u16)).lane(0 as u32) != (0x00CD as u16) { return 5; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bs");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for byte/short SIMD e2e");
    let run = Command::new(&bin).status().expect("run bs");
    assert_eq!(
        run.code(),
        Some(0),
        "byte/short SIMD round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: integer SIMD widths beyond i32x4
/// (`i64x2`, `u32x4`) and bitwise/shift ops on integer SIMD.
#[test]
fn simd_integer_widths_and_bitwise_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bits.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: i32x4 = i32x4::new(255, 240, 15, 85);
    let mask: i32x4 = i32x4::splat(15);
    if a.and(mask).lane(0 as u32) != 15 { return 1; }
    if a.or(mask).lane(2 as u32) != 15 { return 2; }
    if a.xor(mask).lane(1 as u32) != 255 { return 3; }
    let inv: i32x4 = mask.not();
    if inv.lane(0 as u32) != (0 -% 16) { return 4; }
    if a.shl(4 as u32).lane(2 as u32) != 240 { return 5; }
    if a.shr(4 as u32).lane(3 as u32) != 5 { return 6; }
    let big: i64x2 = i64x2::new(100 as i64, -50 as i64);
    if big.abs().lane(1 as u32) != (50 as i64) { return 7; }
    if big.shl(2 as u32).lane(0 as u32) != (400 as i64) { return 8; }
    let unsi: u32x4 = u32x4::new(10 as u32, 20 as u32, 30 as u32, 40 as u32);
    let other: u32x4 = u32x4::splat(25 as u32);
    if unsi.min(other).lane(0 as u32) != (10 as u32) { return 9; }
    if unsi.max(other).lane(0 as u32) != (25 as u32) { return 10; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("bits");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD bitwise e2e");
    let run = Command::new(&bin).status().expect("run bits");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD bitwise + new widths round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: SIMD `load` / `store` round-trip through a
/// `malloc`'d buffer. Exercises both raw-pointer interop and the
/// `unsafe { ... }` requirement.
#[test]
fn simd_load_store_through_malloc_buffer() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ls.cplus");
    std::fs::write(
        &src,
        "\
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);

fn main() -> i32 {
    let buf: *u8 = unsafe { malloc(16 as usize) };
    let fp: *f32 = unsafe { buf as *f32 };
    let v: f32x4 = f32x4::new(2.0f32, 4.0f32, 6.0f32, 8.0f32);
    unsafe { v.store(fp); }
    let r: f32x4 = unsafe { f32x4::load(fp) };
    let s: f32 = r.lane(0 as u32) + r.lane(1 as u32) + r.lane(2 as u32) + r.lane(3 as u32);
    unsafe { free(buf); }
    if s != 20.0f32 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("ls");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD load/store e2e");
    let run = Command::new(&bin).status().expect("run ls");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD load/store round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: `min` / `max` across float + signed-int SIMD.
#[test]
fn simd_min_max_across_widths_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("minmax.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f32x4 = f32x4::new(1.0f32, -2.0f32, 3.0f32, -4.0f32);
    let b: f32x4 = f32x4::new(0.0f32, -1.0f32, 5.0f32, -3.0f32);
    if a.min(b).lane(1 as u32) != -2.0f32 { return 1; }
    if a.max(b).lane(2 as u32) != 5.0f32 { return 2; }
    let ia: i32x4 = i32x4::new(1, 2, 3, 4);
    let ib: i32x4 = i32x4::new(5, 1, 10, 0);
    if ia.min(ib).lane(0 as u32) != 1 { return 3; }
    if ia.max(ib).lane(2 as u32) != 10 { return 4; }
    if ia.min(ib).lane(3 as u32) != 0 { return 5; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("minmax");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD min/max e2e");
    let run = Command::new(&bin).status().expect("run minmax");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD min/max round-trip failed; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1B expansion: i32x4 IR shape (`<4 x i32>`) + integer `mul`.
#[test]
fn simd_i32x4_emits_integer_vector_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("i32x4vir.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: i32x4 = i32x4::splat(3);
    let b: i32x4 = i32x4::splat(7);
    let c: i32x4 = a.mul(b);
    if c.lane(0 as u32) != 21 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    assert!(
        out.status.success(),
        "cpc --emit-ll failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("<4 x i32>"),
        "expected `<4 x i32>` in IR; got:\n{ir}"
    );
    // Integer mul has no `contract` flag (that's float-only).
    assert!(
        ir.contains("mul <4 x i32>")
            || ir.contains("mul nsw <4 x i32>")
            || ir.contains("mul nuw <4 x i32>"),
        "expected vector `mul <4 x i32>` in IR; got:\n{ir}"
    );
}

/// v0.0.6 Slice 1B expansion: f64x2 IR shape (`<2 x double>`).
#[test]
fn simd_f64x2_emits_vector_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("f64x2vir.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f64x2 = f64x2::splat(1.0);
    let b: f64x2 = f64x2::splat(2.0);
    let c: f64x2 = a.mul(b);
    if c.lane(0 as u32) != 2.0 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    assert!(
        out.status.success(),
        "cpc --emit-ll failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("<2 x double>"),
        "expected `<2 x double>` in IR; got:\n{ir}"
    );
    assert!(
        ir.contains("fmul contract <2 x double>"),
        "expected `fmul contract <2 x double>` in IR; got:\n{ir}"
    );
}

/// v0.0.6 Slice 1B: verify codegen emits `<4 x float>` vector IR.
#[test]
fn simd_f32x4_emits_vector_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("vir.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let a: f32x4 = f32x4::splat(1.0f32);
    let b: f32x4 = f32x4::splat(2.0f32);
    let c: f32x4 = a.mul(b);
    if c.lane(0 as u32) != 2.0f32 { return 1; }
    return 0;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    assert!(
        out.status.success(),
        "cpc --emit-ll failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("<4 x float>"),
        "expected `<4 x float>` in IR; got:\n{ir}"
    );
    assert!(
        ir.contains("fmul contract <4 x float>"),
        "expected `fmul contract <4 x float>` in IR; got:\n{ir}"
    );
}

/// v0.0.6 Slice 1A: `include_bytes!` end-to-end.
/// Embeds a 6-byte asset at compile time, asserts each byte at runtime.
#[test]
fn include_bytes_embeds_file_and_reads_bytes_back() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let asset = dir.join("hello.bin");
    std::fs::write(&asset, b"hello\n").unwrap();
    let src = dir.join("ib.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let p: *[u8; 6] = #include_bytes(\"hello.bin\");
    let bytes: *u8 = unsafe { p as *u8 };
    let b0: u8 = unsafe { bytes[0 as usize] };
    let b1: u8 = unsafe { bytes[1 as usize] };
    let b4: u8 = unsafe { bytes[4 as usize] };
    let b5: u8 = unsafe { bytes[5 as usize] };
    if b0 != (104 as u8) { return 1 as i32; }
    if b1 != (101 as u8) { return 2 as i32; }
    if b4 != (111 as u8) { return 3 as i32; }
    if b5 != (10  as u8) { return 4 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let bin = dir.join("ib");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for include_bytes! e2e");
    let run = Command::new(&bin).status().expect("run ib");
    assert_eq!(
        run.code(),
        Some(0),
        "include_bytes! bytes did not round-trip; exit {:?}",
        run.code()
    );
}

/// v0.0.6 Slice 1A: two `include_bytes!` calls on the same path emit one
/// shared `@.bytes.N` global. Inspect emitted IR via `cpc emit-llvm` to
/// verify only one `private unnamed_addr constant` is generated.
#[test]
fn include_bytes_dedupes_repeated_path() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("a.bin"), b"abc").unwrap();
    let src = dir.join("dup.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let p1: *[u8; 3] = #include_bytes(\"a.bin\");
    let p2: *[u8; 3] = #include_bytes(\"a.bin\");
    let b1: *u8 = unsafe { p1 as *u8 };
    let b2: *u8 = unsafe { p2 as *u8 };
    let v1: u8 = unsafe { b1[0 as usize] };
    let v2: u8 = unsafe { b2[0 as usize] };
    if v1 != v2 { return 1 as i32; }
    if v1 != (97 as u8) { return 2 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-llvm");
    assert!(
        out.status.success(),
        "cpc emit-llvm failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    // Count `@.bytes.` global *definitions* only: one line per
    // `private unnamed_addr constant`. References at use sites also
    // contain the symbol, but they don't have `= private`.
    let bytes_defs = ir
        .lines()
        .filter(|l| l.contains("@.bytes.") && l.contains("= private"))
        .count();
    assert_eq!(
        bytes_defs, 1,
        "expected exactly one `@.bytes.N` definition (dedup), saw {bytes_defs}; IR:\n{ir}"
    );
}

/// v0.0.7 Slice 3.1: `include_str!` end-to-end.
/// Embeds a UTF-8 file at compile time and round-trips length + bytes.
#[test]
fn include_str_embeds_utf8_file_and_reads_back() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let asset = dir.join("greet.txt");
    // ASCII payload so we can compare individual bytes by code point
    // without dragging in a UTF-8 multibyte boundary fixture.
    std::fs::write(&asset, b"hi!").unwrap();
    let src = dir.join("is.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let s: str = #include_str(\"greet.txt\");
    if #str_len(s) != (3 as usize) { return 1 as i32; }
    let p: *u8 = #str_ptr(s);
    let b0: u8 = unsafe { p[0 as usize] };
    let b1: u8 = unsafe { p[1 as usize] };
    let b2: u8 = unsafe { p[2 as usize] };
    if b0 != (104 as u8) { return 2 as i32; }
    if b1 != (105 as u8) { return 3 as i32; }
    if b2 != (33 as u8)  { return 4 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let bin = dir.join("is");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for include_str! e2e");
    let run = Command::new(&bin).status().expect("run is");
    assert_eq!(
        run.code(),
        Some(0),
        "include_str! bytes did not round-trip; exit {:?}",
        run.code()
    );
}

/// v0.0.7 Slice 3.1: a `.cplus` file that calls `include_str!` on a
/// file containing a stray 0xFF byte must fail to build, reporting E0875.
#[test]
fn include_str_rejects_non_utf8_file_with_e0875() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("bad.bin"), [b'o', b'k', 0xFF, b'!']).unwrap();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let s: str = #include_str(\"bad.bin\");
    return 0 as i32;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(dir.join("bad"))
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected build failure for non-UTF-8 include_str! input"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0875"),
        "expected E0875 in stderr; got:\n{stderr}"
    );
}

/// v0.0.7 Slice 3.1: include_str! + include_bytes! on the same path
/// share one underlying `[N x i8]` global (dedup keyed by abs_path).
#[test]
fn include_str_and_include_bytes_share_global() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(dir.join("shared.txt"), b"abc").unwrap();
    let src = dir.join("share.cplus");
    std::fs::write(
        &src,
        "\
fn main() -> i32 {
    let s: str = #include_str(\"shared.txt\");
    let b: *[u8; 3] = #include_bytes(\"shared.txt\");
    if #str_len(s) != (3 as usize) { return 1 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-llvm");
    assert!(
        out.status.success(),
        "cpc emit-llvm failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    let bytes_defs = ir
        .lines()
        .filter(|l| l.contains("@.bytes.") && l.contains("= private"))
        .count();
    assert_eq!(
        bytes_defs, 1,
        "expected exactly one shared `@.bytes.N` definition across \
         include_str! + include_bytes! on the same path, saw {bytes_defs}; IR:\n{ir}"
    );
}

/// v0.0.8 bench-gap finding 3: `let X: STRUCT = if cond { call() } else
/// { ...; struct_literal };` used to panic at codegen.rs:5902 because
/// `expr_value_ty_with_bindings` didn't recognize `Call` or `StructLit`
/// as value-producing — `gen_if` returned None and the `let` panicked
/// on the missing value. Fixed in v0.0.8 by extending the helper to
/// resolve Call return types via `self.sigs` and struct literals via
/// `self.types.struct_by_name`.
#[test]
fn mixed_if_arm_with_call_and_struct_literal_does_not_panic() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("mixed.cplus");
    std::fs::write(&src, "\
struct V { x: f32, y: f32, z: f32 }
fn v_make(x: f32, y: f32) -> V { return V { x: x, y: y, z: 0.0f32 }; }

fn refract(dir: V, n: V, cond: bool) -> V {
    let result: V = if cond {
        v_make(3.0f32, 4.0f32)
    } else {
        let r_perp: V = V { x: dir.x + n.x, y: dir.y + n.y, z: 0.0f32 };
        let mut k: f32 = 1.0f32 - r_perp.x;
        if k < 0.0f32 { k = 0.0f32; }
        V { x: r_perp.x + r_perp.x, y: r_perp.y + k, z: 0.0f32 }
    };
    return result;
}

fn main() -> i32 {
    let d: V = V { x: 1.0f32, y: 2.0f32, z: 0.0f32 };
    let n: V = V { x: 0.0f32, y: 1.0f32, z: 0.0f32 };
    let r_true: V = refract(d, n, true);
    if r_true.x != 3.0f32 { return 1; }
    if r_true.y != 4.0f32 { return 2; }
    let r_false: V = refract(d, n, false);
    if r_false.x != 2.0f32 { return 3; }
    return 0;
}
").unwrap();
    let bin = dir.join("mixed");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for mixed-if-arm reproducer (regression)");
    let run = Command::new(&bin).status().expect("run mixed");
    assert_eq!(run.code(), Some(0),
        "mixed-if-arm reproducer expected exit 0; got {:?}", run.code());
}

#[test]
fn block_tail_ident_non_copy_does_not_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("blkmv.cplus");
    std::fs::write(
        &src,
        "\
extern fn printf(fmt: *u8, ...) -> i32;
fn main() -> i32 {
    // Block-tail rebind.
    let f: string = {
        let inner: string = \"inside\".to_string();
        inner
    };
    if f.len() != (6 as usize) { return 1 as i32; }
    // Nested block-tail rebind.
    let g: string = {
        let outer: string = {
            let deep: string = \"deep\".to_string();
            deep
        };
        outer
    };
    if g.len() != (4 as usize) { return 2 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let bin = dir.join("blkmv");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for block-tail-rebind regression"
    );
    let run = Command::new(&bin).status().expect("run blkmv");
    assert_eq!(
        run.code(),
        Some(0),
        "block-tail rebind should not double-free"
    );
}

/// v0.0.5 Phase 1C: container `drop` invokes inner-T Drop via the
/// `__cplus_drop_in_place::[T]` intrinsic. Without this fix, every
/// container that holds a Drop type leaked the inner resources on
/// container teardown — `Box[string]`, `Vec[string]`, `Arc[string]`,
/// `HashMap[str, string]` all bled bytes per-instance.
///
/// We can't easily detect leaks portably (LSan needs Linux), but we
/// CAN verify the new drop path runs without crashing for every
/// container that v0.0.4 shipped. A crash here means the inner-T Drop
/// machinery is firing on bad pointers (e.g. uninitialized refcount
/// path or wrong field offset).
#[test]
fn phase1c_container_inner_drop_runs_without_crash() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"idrop\"\n\n[[bin]]\nname = \"idrop\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &[
        "box", "vec", "arc", "rc", "hash_map", "atomic", "result", "iterator", "option",
    ] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/box\" as box;\n\
         import \"stdlib/vec\" as vec;\n\
         import \"stdlib/arc\" as arc;\n\
         import \"stdlib/rc\" as rc;\n\
         import \"stdlib/hash_map\" as hm;\n\
         fn box_scope() { let _b: box::Box[string] = box::new::[string](\"hello\".to_string()); return; }\n\
         fn vec_scope() {\n\
             let mut v: vec::Vec[string] = vec::new::[string]();\n\
             v.push(\"one\".to_string());\n\
             v.push(\"two\".to_string());\n\
             v.push(\"three\".to_string());\n\
             return;\n\
         }\n\
         fn arc_scope() {\n\
             let a: arc::Arc[string] = arc::new::[string](\"arc-value\".to_string());\n\
             let _c: u64 = a.strong_count();\n\
             return;\n\
         }\n\
         fn rc_scope() {\n\
             let r: rc::Rc[string] = rc::new::[string](\"rc-value\".to_string());\n\
             let _c: u64 = r.strong_count();\n\
             return;\n\
         }\n\
         fn hm_scope() {\n\
             let mut m: hm::HashMap[str, i32] = hm::new::[str, i32]();\n\
             m.insert(\"apple\", 1 as i32);\n\
             m.insert(\"banana\", 2 as i32);\n\
             m.insert(\"cherry\", 3 as i32);\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             box_scope();\n\
             vec_scope();\n\
             arc_scope();\n\
             rc_scope();\n\
             hm_scope();\n\
             return 0 as i32;\n\
         }\n",
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 1C inner-Drop regression?)"
    );
    let bin = dir.join("target/debug/idrop");
    let run = Command::new(&bin).status().expect("run idrop");
    assert_eq!(
        run.code(),
        Some(0),
        "inner-T Drop sites should all run cleanly"
    );
}

/// v0.0.5 Phase 1D: async fns drive cleanly under `--asan`. The
/// Phase-1E note in plan-0.0.4 flagged that scalar `i32` async fns
/// returned 0 instead of the expected value under `--asan`; that
/// regression was incidentally cured by Phase 1E's promise-alloca fix
/// (passing `alloca <T>` to `coro.id` instead of `ptr null`) but was
/// never tested. This regression locks the fix in: scalar primitive
/// returns, chained awaits across two coroutines, and the generic
/// async-fn instantiation matrix (i32/i64/bool) all build and run
/// cleanly under ASan.
#[test]
#[cfg(target_os = "macos")]
fn phase1d_async_runs_clean_under_asan() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"asanasync\"\n\n[[bin]]\nname = \"asanasync\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/executor\" as executor;\n\
         async fn id[T](x: T) -> T { return x; }\n\
         async fn inner(x: i32) -> i32 { return x +% (10 as i32); }\n\
         async fn outer(x: i32) -> i32 {\n\
             let v: i32 = await inner(x);\n\
             return v +% (100 as i32);\n\
         }\n\
         fn main() -> i32 {\n\
             // Scalar primitive return.\n\
             let f0: future::Future[i32] = id::[i32](42);\n\
             if executor::block_on::[i32](f0) != (42 as i32) { return 1; }\n\
             // Two more generic instantiations to exercise the\n\
             // monomorphized promise alloca for different sizes.\n\
             let f1: future::Future[i64] = id::[i64](99 as i64);\n\
             if executor::block_on::[i64](f1) != (99 as i64) { return 2; }\n\
             let f2: future::Future[bool] = id::[bool](true);\n\
             if !executor::block_on::[bool](f2) { return 3; }\n\
             // Chained await — two coroutine frames live concurrently.\n\
             let f3: future::Future[i32] = outer(5 as i32);\n\
             if executor::block_on::[i32](f3) != (115 as i32) { return 4; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc --asan");
    assert!(
        st.success(),
        "cpc build --asan failed (Phase 1D async-under-ASan regression?)"
    );
    let bin = dir.join("target/debug/asanasync");
    let run = Command::new(&bin).status().expect("run asanasync");
    assert_eq!(
        run.code(),
        Some(0),
        "async fns under --asan should return their declared values"
    );
}

/// v0.0.5 Phase 2B: `pub gen fn iter(self) -> T` on a user struct.
/// Mirror of Phase 4's `gen fn` lowering, threaded through the method
/// path (`check_method` + `gen_gen_method`). Verifies:
///   - sema wraps return T → Iterator[T] at the method-sig site
///   - codegen emits a coroutine returning Iterator[T] with the
///     receiver as the first parameter
///   - `for x in obj.iter()` desugar walks the iterator inline
#[test]
fn phase2b_gen_method_on_struct() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"genm\"\n\n[[bin]]\nname = \"genm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/iterator\" as iterator;\n\
         pub struct Counter { n: i32 }\n\
         impl Counter {\n\
             pub gen fn iter(self) -> i32 {\n\
                 let mut i: i32 = 0;\n\
                 while i < self.n {\n\
                     yield i;\n\
                     i = i +% (1 as i32);\n\
                 }\n\
                 return;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let c: Counter = Counter { n: 5 as i32 };\n\
             let mut sum: i32 = 0;\n\
             for x in c.iter() {\n\
                 sum = sum +% x;\n\
             }\n\
             // 0+1+2+3+4 = 10\n\
             if sum != (10 as i32) { return 1 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 2B gen-method regression?)"
    );
    let bin = dir.join("target/debug/genm");
    let run = Command::new(&bin).status().expect("run genm");
    assert_eq!(
        run.code(),
        Some(0),
        "gen-method + for-in should sum 0..5 to 10"
    );
}

/// v0.0.5 Phase 2C: `impl EnumName { fn ... }` on a non-generic enum.
/// Lifts the v0.0.4 E0325 restriction for concrete enum types. Generic
/// enum impls (`impl Option[T]`) still pending — the monomorphize-side
/// `synthesize_generic_typed_impls` analog for enum templates needs the
/// same `mono.enum_instantiations` walk and is a separate slice.
///
/// Verifies:
///   - Plain enums (Tag::Yes/No): both methods dispatch through the
///     enum's pointer-passed receiver.
///   - Tagged enums (Shape::Circle(i32)/Square(i32)): method body's
///     `match self { ... }` reads through the receiver correctly.
#[test]
fn phase2c_enum_impl_methods_dispatch() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("enumimpl.cplus");
    std::fs::write(
        &src,
        "\
extern fn printf(fmt: *u8, ...) -> i32;
pub enum Tag { Yes, No }
impl Tag {
    pub fn flip(self) -> Tag {
        return match self {
            Tag::Yes => Tag::No,
            Tag::No => Tag::Yes,
        };
    }
    pub fn is_yes(self) -> bool {
        return match self {
            Tag::Yes => true,
            Tag::No => false,
        };
    }
}
pub enum Shape { Circle(i32), Square(i32) }
impl Shape {
    pub fn area(self) -> i32 {
        return match self {
            Shape::Circle(r) => r *% r *% (3 as i32),
            Shape::Square(s) => s *% s,
        };
    }
}
fn main() -> i32 {
    let y: Tag = Tag::Yes;
    let n: Tag = y.flip();
    if y.is_yes() != true { return 1 as i32; }
    if n.is_yes() != false { return 2 as i32; }
    let c: Shape = Shape::Circle(2 as i32);
    let s: Shape = Shape::Square(3 as i32);
    if c.area() != (12 as i32) { return 3 as i32; }
    if s.area() != (9 as i32) { return 4 as i32; }
    return 0 as i32;
}
",
    )
    .unwrap();
    let bin = dir.join("enumimpl");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 2C enum impl regression?)"
    );
    let run = Command::new(&bin).status().expect("run enumimpl");
    assert_eq!(
        run.code(),
        Some(0),
        "enum impl methods should dispatch correctly"
    );
}

/// free-function constructors `vec::new::[T]()` + `vec::with_capacity::[T](n)`.
/// Exercises push, len, get, drop end-to-end.
#[test]
fn stdlib_vec_push_and_get() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vec_smoke\"\n\n[[bin]]\nname = \"vec_smoke\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    // v0.0.5 Phase 3 Slice 3A: vec.cplus imports stdlib/iterator (for
    // Vec::iter's `gen fn` return wrap → Iterator[T]); iterator.cplus
    // imports stdlib/option. Stage both alongside vec.cplus so sema's
    // signature collection resolves cleanly.
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         fn main() -> i32 {\n\
             let mut v: vec::Vec[i32] = vec::new::[i32]();\n\
             let mut i: i32 = 1;\n\
             while i <= 8 {\n\
                 v.push(i);\n\
                 i = i +% 1;\n\
             }\n\
             let mut total: i32 = 0;\n\
             let mut j: usize = 0 as usize;\n\
             while j < v.len() {\n\
                 total = total +% vec::at_copy::[i32](v, j);\n\
                 j = j +% (1 as usize);\n\
             }\n\
             return total;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/vec_smoke");
    let run = Command::new(&bin).status().expect("run");
    // 1+2+3+4+5+6+7+8 = 36.
    assert_eq!(run.code(), Some(36), "expected sum of 1..=8 = 36");
}

/// v0.0.5 Phase 3 Slice 3A: `Vec[T]::iter()` is the first stdlib
/// gen-method, exercised end-to-end via for-in. Validates Phase 2B's
/// gen-method machinery on a generic struct's instantiation (`Vec[i32]`).
#[test]
fn stdlib_vec_iter_for_in() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vec_iter\"\n\n[[bin]]\nname = \"vec_iter\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         fn main() -> i32 {\n\
             let mut v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(10 as i32);\n\
             v.push(20 as i32);\n\
             v.push(30 as i32);\n\
             let mut sum: i32 = 0;\n\
             for x in v.iter() {\n\
                 sum = sum +% x;\n\
             }\n\
             if sum != (60 as i32) { return 1 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 3 Slice 3A regression?)"
    );
    let bin = dir.join("target/debug/vec_iter");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "Vec::iter for-in sum should be 60");
}

/// v0.0.5 Phase 2C follow-on: generic-enum impl synthesis. `impl
/// Option[T] { fn is_some(self) -> bool }` style — methods on a
/// generic enum template now compile + dispatch correctly at each
/// instantiation. Mirror of the struct-side `synthesize_generic_typed_impls`
/// path; sema's `instantiate_enum_from_arg_tys` populates the
/// synthesized concrete enum's methods table from the generic impl
/// template, and monomorphize emits the concrete ImplBlock per
/// instantiation.
#[test]
fn phase2c_generic_enum_impl_synthesis() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"gei\"\n\n[[bin]]\nname = \"gei\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "enum Maybe[T] { Some(T), None }\n\
         impl Maybe[T] {\n\
             pub fn is_some(self) -> bool {\n\
                 return match self {\n\
                     Maybe[T]::Some(_) => true,\n\
                     Maybe[T]::None => false,\n\
                 };\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let s: Maybe[i32] = Maybe[i32]::Some(7 as i32);\n\
             let n: Maybe[i32] = Maybe[i32]::None;\n\
             if !s.is_some() { return 1 as i32; }\n\
             if n.is_some() { return 2 as i32; }\n\
             // Second instantiation: Maybe[bool] exercises the per-arg\n\
             // synthesis path independently.\n\
             let sb: Maybe[bool] = Maybe[bool]::Some(true);\n\
             if !sb.is_some() { return 3 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (generic-enum impl synthesis regression?)"
    );
    let bin = dir.join("target/debug/gei");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected generic-enum methods to dispatch correctly"
    );
}

/// v0.0.5 Phase 3 Slice 3C follow-on: `vec::collect[T]` drains an
/// Iterator[T] into a Vec[T]. Free fn (not an `impl Iterator[T]`
/// method) to avoid the iterator↔vec circular import. Exercises
/// chained `.iter().filter(...)` consumption.
#[test]
fn stdlib_vec_collect_drains_iterator() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"col\"\n\n[[bin]]\nname = \"col\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         fn is_pos(x: i32) -> bool { return x > (0 as i32); }\n\
         fn main() -> i32 {\n\
             let mut src: vec::Vec[i32] = vec::new::[i32]();\n\
             src.push(0 -% (1 as i32));\n\
             src.push(2 as i32);\n\
             src.push(0 -% (3 as i32));\n\
             src.push(4 as i32);\n\
             src.push(5 as i32);\n\
             let positives: vec::Vec[i32] = vec::collect::[i32](src.iter().filter(is_pos));\n\
             if positives.len() != (3 as usize) { return 1 as i32; }\n\
             let mut sum: i32 = 0;\n\
             let mut i: usize = 0 as usize;\n\
             while i < positives.len() {\n\
                 sum = sum +% vec::at_copy::[i32](positives, i);\n\
                 i = i +% (1 as usize);\n\
             }\n\
             if sum != (11 as i32) { return 2 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (collect adapter regression?)"
    );
    let bin = dir.join("target/debug/col");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "expected collected positives to total 11"
    );
}

/// v0.0.5 Phase 3 Slice 3C: iterator adapters end-to-end. Exercises
/// `Iterator[i32]::filter`, `Iterator[i32]::take`, and the free
/// `iterator::map::[i32, i32]` — all of which match on `Option[T]`
/// inside generic-impl-method / generic-fn bodies. Sema's
/// `propagate_pattern_instantiations` is what registers `Option[i32]`
/// from those pattern positions; without it, codegen would panic in
/// `lty(Ty::Enum(EnumId(0)))` synthesizing the adapter's `match
/// self.next() { ... }` lowering.
#[test]
fn stdlib_iterator_adapters_filter_take_map() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"itad\"\n\n[[bin]]\nname = \"itad\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/iterator\" as iterator;\n\
         fn is_even(x: i32) -> bool { return (x % (2 as i32)) == (0 as i32); }\n\
         fn double(x: i32) -> i32 { return x *% (2 as i32); }\n\
         fn main() -> i32 {\n\
             let mut v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(1 as i32);\n\
             v.push(2 as i32);\n\
             v.push(3 as i32);\n\
             v.push(4 as i32);\n\
             v.push(5 as i32);\n\
             v.push(6 as i32);\n\
             // filter: keep even — sum 2+4+6 = 12\n\
             let mut sum: i32 = 0;\n\
             for x in v.iter().filter(is_even) {\n\
                 sum = sum +% x;\n\
             }\n\
             if sum != (12 as i32) { return 1 as i32; }\n\
             // take(3): count exactly three elements\n\
             let mut count: i32 = 0;\n\
             for _x in v.iter().take(3 as usize) {\n\
                 count = count +% (1 as i32);\n\
             }\n\
             if count != (3 as i32) { return 2 as i32; }\n\
             // map: double every element — sum 2+4+6+8+10+12 = 42\n\
             let mut sum2: i32 = 0;\n\
             for x in iterator::map::[i32, i32](v.iter(), double) {\n\
                 sum2 = sum2 +% x;\n\
             }\n\
             if sum2 != (42 as i32) { return 3 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed (Phase 3 Slice 3C regression?)"
    );
    let bin = dir.join("target/debug/itad");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "iterator adapters round-trip should exit 0"
    );
}

/// v0.0.4 Phase 3 Slice 3B.3: `Vec[T]::extend_from_slice(s: T[])` —
/// slice-typed wrapper over `extend_from_raw`. Single realloc + single
/// memcpy regardless of T. This test exercises both element type kinds
/// where T is a scalar primitive (i32) — the `T[]` slice shape carries
/// the count, so the caller doesn't have to compute it separately.
#[test]
fn stdlib_vec_extend_from_slice_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vex\"\n\n[[bin]]\nname = \"vex\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    // v0.0.5 Phase 3 Slice 3A: vec.cplus imports stdlib/iterator (for
    // Vec::iter's `gen fn` return wrap → Iterator[T]); iterator.cplus
    // imports stdlib/option. Stage both alongside vec.cplus so sema's
    // signature collection resolves cleanly.
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             // Build a source Vec with [10, 20, 30, 40, 50] then expose a slice.\n\
             let mut src_vec: vec::Vec[i32] = vec::new::[i32]();\n\
             src_vec.push(10 as i32);\n\
             src_vec.push(20 as i32);\n\
             src_vec.push(30 as i32);\n\
             src_vec.push(40 as i32);\n\
             src_vec.push(50 as i32);\n\
             let slice: i32[] = src_vec.as_slice();\n\
             // Extend a fresh Vec; assert total + count.\n\
             let mut dst: vec::Vec[i32] = vec::new::[i32]();\n\
             dst.push(1 as i32);\n\
             vec::extend_from_slice::[i32](dst, slice);\n\
             dst.push(2 as i32);\n\
             // dst = [1, 10, 20, 30, 40, 50, 2]; len = 7, sum = 153.\n\
             let mut sum: i32 = 0;\n\
             let mut i: usize = 0 as usize;\n\
             while i < dst.len() {\n\
                 sum = sum +% vec::at_copy::[i32](dst, i);\n\
                 i = i +% (1 as usize);\n\
             }\n\
             if dst.len() != (7 as usize) { return 90 as i32; }\n\
             if sum != (153 as i32) { return 91 as i32; }\n\
             return 0 as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/vex");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "extend_from_slice round-trip mismatched"
    );
}

/// v0.0.3 Phase 5 Slice 5A: stdlib/atomic end-to-end.
///
/// Exercises load / store / fetch_add / fetch_sub / fetch_and / fetch_or
/// / fetch_xor / compare_exchange (both success and failure paths) on
/// `u64` and `i32`. Each op is a `match`-dispatch in the stdlib wrapper
/// that maps `Ordering::*` to the per-ordering compiler intrinsic
/// (`__cplus_atomic_<op>_<ty>_<ord>`). The binary exits non-zero on the
/// first round-trip mismatch, so a clean exit is the assertion.
#[test]
fn stdlib_atomic_round_trips() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"atomic_smoke\"\n\n[[bin]]\nname = \"atomic_smoke\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/atomic\" as atomic;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             let p64: *u64 = unsafe { malloc(8 as usize) as *u64 };\n\
             atomic::atomic_store_u64(p64, 0 as u64, atomic::Ordering::SeqCst);\n\
             let prev: u64 = atomic::atomic_fetch_add_u64(p64, 10 as u64, atomic::Ordering::SeqCst);\n\
             if prev != (0 as u64) { unsafe { free(p64 as *u8); } return 1; }\n\
             let cur: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if cur != (10 as u64) { unsafe { free(p64 as *u8); } return 2; }\n\
             let _s: u64 = atomic::atomic_fetch_sub_u64(p64, 3 as u64, atomic::Ordering::SeqCst);\n\
             let after_sub: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_sub != (7 as u64) { unsafe { free(p64 as *u8); } return 3; }\n\
             let cx: u64 = atomic::atomic_compare_exchange_u64(p64, 7 as u64, 42 as u64, atomic::Ordering::SeqCst);\n\
             if cx != (7 as u64) { unsafe { free(p64 as *u8); } return 4; }\n\
             let after_cx: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_cx != (42 as u64) { unsafe { free(p64 as *u8); } return 5; }\n\
             let cx_fail: u64 = atomic::atomic_compare_exchange_u64(p64, 0 as u64, 99 as u64, atomic::Ordering::SeqCst);\n\
             if cx_fail != (42 as u64) { unsafe { free(p64 as *u8); } return 6; }\n\
             let after_fail: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_fail != (42 as u64) { unsafe { free(p64 as *u8); } return 7; }\n\
             unsafe { free(p64 as *u8); }\n\
             let p32: *i32 = unsafe { malloc(4 as usize) as *i32 };\n\
             atomic::atomic_store_i32(p32, 0xF0 as i32, atomic::Ordering::SeqCst);\n\
             let _o: i32 = atomic::atomic_fetch_or_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let or_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if or_val != (0xFF as i32) { unsafe { free(p32 as *u8); } return 8; }\n\
             let _a: i32 = atomic::atomic_fetch_and_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let and_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if and_val != (0x0F as i32) { unsafe { free(p32 as *u8); } return 9; }\n\
             let _x: i32 = atomic::atomic_fetch_xor_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let xor_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if xor_val != (0 as i32) { unsafe { free(p32 as *u8); } return 10; }\n\
             unsafe { free(p32 as *u8); }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/atomic_smoke");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "atomic_smoke exited non-zero: {:?} stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// v0.0.3 Phase 5 Slice 5A: every atomic ordering keyword reaches LLVM.
/// Compiles a program that uses all five `Ordering::*` variants and
/// inspects the emitted IR via `--emit-llvm-ir`. This complements the
/// in-tree codegen unit tests by checking the full stdlib-wrapper +
/// match-dispatch path actually produces every ordering keyword.
#[test]
fn stdlib_atomic_ir_contains_every_ordering() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"atomic_ir\"\n\n[[bin]]\nname = \"atomic_ir\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    // Three calls — one with relaxed, one with acquire, one with seqcst
    // — together cover monotonic+acquire+seq_cst keywords. The wrapper
    // body's match arms cover release and acq_rel under the hood for
    // every op, so we don't need to call them all here to assert
    // their presence in the emitted IR.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/atomic\" as atomic;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         fn main() -> i32 {\n\
             let p: *u64 = unsafe { malloc(8 as usize) as *u64 };\n\
             atomic::atomic_store_u64(p, 0 as u64, atomic::Ordering::Relaxed);\n\
             let _a: u64 = atomic::atomic_fetch_add_u64(p, 1 as u64, atomic::Ordering::Acquire);\n\
             let _b: u64 = atomic::atomic_fetch_add_u64(p, 1 as u64, atomic::Ordering::SeqCst);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll-project")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "cpc --emit-ll-project failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The wrapper module's match arms instantiate every per-ordering
    // intrinsic, so the linked IR must mention every LLVM ordering
    // keyword even with only three call sites in main.
    let ll = String::from_utf8_lossy(&out.stdout).into_owned();
    for kw in ["monotonic", "acquire", "release", "acq_rel", "seq_cst"] {
        assert!(ll.contains(kw), "expected ordering keyword `{kw}` in IR");
    }
    assert!(ll.contains("atomicrmw add"), "expected atomicrmw add in IR");
    assert!(ll.contains("store atomic"), "expected store atomic in IR");
}

/// v0.0.3 Phase 5 Slice 5B: spawn an OS thread and round-trip a value back
/// through `JoinHandle::join`. Verifies the full surface: thread::spawn[O]
/// → pthread_create → trampoline runs user fn → result lands in heap ctx →
/// join blocks until worker exits → join reads + frees → owned value
/// returned to the parent.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_join_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"thread_smoke\"\n\n[[bin]]\nname = \"thread_smoke\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn lo() -> i64 { return 100 as i64; }\n\
         fn hi() -> i64 { return 200 as i64; }\n\
         fn answer_i32() -> i32 { return 42 as i32; }\n\
         fn answer_u64() -> u64 { return 99 as u64; }\n\
         fn answer_bool() -> bool { return true; }\n\
         fn main() -> i32 {\n\
             let h1: thread::JoinHandle[i64] = thread::spawn::[i64](lo);\n\
             let h2: thread::JoinHandle[i64] = thread::spawn::[i64](hi);\n\
             let total: i64 = h1.join() +% h2.join();\n\
             if total != (300 as i64) { return 1; }\n\
             let h32: thread::JoinHandle[i32] = thread::spawn::[i32](answer_i32);\n\
             if h32.join() != (42 as i32) { return 2; }\n\
             let hu: thread::JoinHandle[u64] = thread::spawn::[u64](answer_u64);\n\
             if hu.join() != (99 as u64) { return 3; }\n\
             let hb: thread::JoinHandle[bool] = thread::spawn::[bool](answer_bool);\n\
             if hb.join() != true { return 4; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/thread_smoke");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "thread_smoke exited non-zero: {:?} stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// v0.0.3 Phase 5 Slice 5C: spawn_with end-to-end. Two threads each
/// receive a `Range` struct argument (Copy struct, 16 bytes); each
/// computes the partial sum and the parent adds the joined results.
/// Also covers non-Copy input via `string` — the worker takes
/// ownership and returns the byte length.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_with_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sw\"\n\n[[bin]]\nname = \"sw\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         struct Range { start: i64, end: i64 }\n\
         fn sum_range(r: Range) -> i64 {\n\
             let mut total: i64 = 0 as i64;\n\
             let mut i: i64 = r.start;\n\
             while i < r.end {\n\
                 total = total +% i;\n\
                 i = i +% (1 as i64);\n\
             }\n\
             return total;\n\
         }\n\
         fn measure(move s: string) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let left:  Range = Range { start: 1 as i64,   end: 501 as i64  };\n\
             let right: Range = Range { start: 501 as i64, end: 1001 as i64 };\n\
             let h1: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](left, sum_range);\n\
             let h2: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](right, sum_range);\n\
             let total: i64 = h1.join() +% h2.join();\n\
             if total != (500500 as i64) { return 1; }\n\
             let s: string = \"hello, threaded world\".to_string();\n\
             let hs: thread::JoinHandle[i64] = thread::spawn_with::[string, i64](s, measure);\n\
             if hs.join() != (21 as i64) { return 2; }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let bin = dir.join("target/debug/sw");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "spawn_with test exited non-zero: {:?} stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// v0.0.3 Phase 5 Slice 5C: ASan-clean run of the spawn_with path with
/// a moved `string` input. The worker takes ownership and drops it
/// when the start function exits; the heap buffer must not leak.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_spawn_with_string_input_asan_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sw_asan\"\n\n[[bin]]\nname = \"sw_asan\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn measure(move s: string) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let s: string = \"hello, threaded world\".to_string();\n\
             let h: thread::JoinHandle[i64] = thread::spawn_with::[string, i64](s, measure);\n\
             let n: i64 = h.join();\n\
             if n != (21 as i64) { return 1; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "cpc build --asan failed");
    let run = Command::new(dir.join("target/debug/sw_asan"))
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "exited non-zero: {:?} stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stderr.contains("LeakSanitizer"),
        "leak detected:\n{stderr}"
    );
    assert!(
        !stderr.contains("AddressSanitizer"),
        "ASan error:\n{stderr}"
    );
}

/// v0.0.3 Phase 5 Slice 5C borrow-check negative: post-move use of a
/// non-Copy `string` input rejected by sema with `E0335 use of moved
/// value`. The `move` annotation on `spawn_with[I, O]`'s input
/// argument transfers ownership at the call site; the parent loses
/// access to the string immediately.
#[test]
fn stdlib_thread_spawn_with_post_move_use_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sw_neg\"\n\n[[bin]]\nname = \"sw_neg\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn measure(move s: string) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let s: string = \"hi\".to_string();\n\
             let h: thread::JoinHandle[i64] = thread::spawn_with::[string, i64](s, measure);\n\
             // Post-move use: borrow checker rejects.\n\
             let n: i64 = s.len() as i64;\n\
             let _r: i64 = h.join();\n\
             return n as i32;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected build to fail on post-move use"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0335") || stderr.contains("use of moved value"),
        "expected E0335 (use of moved value), got:\n{stderr}"
    );
}

/// v0.0.4 Phase 2 Slice 2H — true fire-and-forget thread detach. Drop
/// a `JoinHandle` without calling `join`. The Drop impl in
/// `stdlib/thread` now calls `pthread_detach` + atomically decrements
/// the ctx refcount (no blocking). The worker's trampoline also
/// decrements after writing the result; whichever thread observes
/// prev==1 frees the ctx. Run under ASan to verify the refcount
/// handshake doesn't leak the ctx. The spin loop ensures the worker
/// has time to finish before main exits (so its dec actually runs).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_thread_drop_detaches_unjoined_handle() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"thread_detach\"\n\n[[bin]]\nname = \"thread_detach\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         fn worker() -> i32 { return 7 as i32; }\n\
         fn main() -> i32 {\n\
             {\n\
                 let h: thread::JoinHandle[i32] = thread::spawn::[i32](worker);\n\
                 // h falls out of scope here: Drop runs pthread_detach + free.\n\
             }\n\
             // Spin briefly so the worker can finish before main exits.\n\
             let mut i: i64 = 0 as i64;\n\
             while i < (5000000 as i64) { i = i +% (1 as i64); }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build --asan failed");
    let bin = dir.join("target/debug/thread_detach");
    let run = Command::new(&bin).output().expect("run");
    assert!(
        run.status.success(),
        "detach test exited non-zero: {:?} stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    // ASan would have written its leak report to stderr if anything leaked.
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stderr.contains("LeakSanitizer"),
        "expected no leaks under ASan, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("AddressSanitizer"),
        "expected no ASan errors, got:\n{stderr}"
    );
}

#[test]
fn orphan_static_lib_emits_e0861() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    // Vendor declares NO `[link]` at all but has an .a file sitting under
    // src/lib/<host>/ — orphan, manifest-is-truth violation.
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n",
    )
    .unwrap();
    let lib_dir = dir.join("vendor/foo/src/lib").join(&host);
    std::fs::create_dir_all(&lib_dir).unwrap();
    // The orphan-detection is filesystem-presence only, no content read.
    std::fs::write(lib_dir.join("liborphan.a"), b"not a real archive").unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0861"), "expected E0861, got: {stderr}");
    assert!(
        stderr.contains("liborphan.a"),
        "diagnostic should name the file: {stderr}"
    );
}

#[test]
fn host_triple_unsupported_emits_e0862() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    // Package only supports an alien triple. (`not-a-real-triple` is
    // deliberately nonsensical so this test stays host-agnostic — both
    // x86 and arm CI machines run it correctly.)
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libfoo.a\"]\ntriples = [\"not-a-real-triple\"]\n",
    ).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0862"), "expected E0862, got: {stderr}");
    assert!(
        stderr.contains("not-a-real-triple"),
        "diagnostic should list the package's supported triples: {stderr}"
    );
}

#[test]
fn bundled_without_triples_emits_e0863_via_build() {
    // E0863 is enforced at manifest-parse time, but a `cpc build` that
    // touches a malformed vendor manifest must still surface it through
    // the dep walk — this test pins the integration path so future
    // refactors can't silently swallow the diagnostic.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libfoo.a\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0863"), "expected E0863, got: {stderr}");
}

// ---- Phase 5 Slice 5.A: library targets + object emission ----
//
// `[lib]` in Cplus.toml produces `.a` and `.dylib`/`.so` instead of an
// executable. A C consumer can `#include` a hand-written header, link
// against the artifact, and call any C-callable function. The e2e tests
// here build a tiny library, link it from C, and verify the runtime
// answer — the same shape as the AppKit-via-Cplus.toml slice's tests.

#[test]
fn lib_target_produces_staticlib() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mathlib\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed: {st}");
    let a_path = dir.join("target/debug/libmathlib.a");
    assert!(
        a_path.is_file(),
        "expected libmathlib.a at {}",
        a_path.display()
    );
}

#[test]
fn lib_target_produces_dylib_or_so() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mathlib\"\n\n[lib]\ncrate-type = \"cdylib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed: {st}");
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let dyn_path = dir.join(format!("target/debug/libmathlib.{ext}"));
    assert!(
        dyn_path.is_file(),
        "expected libmathlib.{ext} at {}",
        dyn_path.display()
    );
}

#[test]
fn lib_target_both_produces_a_and_dylib() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mathlib\"\n\n[lib]\ncrate-type = \"both\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    assert!(dir.join("target/debug/libmathlib.a").is_file());
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    assert!(dir.join(format!("target/debug/libmathlib.{ext}")).is_file());
}

#[test]
fn lib_target_exposes_pub_symbols_unmangled() {
    // The key property for C-consumability: `pub fn add` in src/lib.cplus
    // ends up as the bare `_add` (Mach-O) / `add` (ELF) symbol — not the
    // path-mangled `_src.lib.add` that the resolver normally produces.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mathlib\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let nm = Command::new(nm_prog())
        .arg("-g")
        .arg(dir.join("target/debug/libmathlib.a"))
        .output()
        .expect("invoke nm");
    let out = String::from_utf8_lossy(&nm.stdout);
    let has_bare = out.contains(" _add") || out.contains(" T add");
    assert!(
        has_bare,
        "expected unmangled `add` in libmathlib.a; got:\n{out}"
    );
    // And the mangled form must NOT appear.
    assert!(
        !out.contains("src.lib.add"),
        "expected `pub fn add` to skip path-mangling; got mangled form in:\n{out}"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn c_consumer_links_static_and_dynamic() {
    // Full round-trip: build a C+ lib, write a C consumer, link both
    // statically and dynamically, run, check exit code matches the
    // arithmetic. The most important end-to-end signal that the slice
    // really delivers C-callable libraries.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mathlib\"\n\n[lib]\ncrate-type = \"both\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         pub fn sub(a: i32, b: i32) -> i32 { return a - b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         extern int32_t add(int32_t, int32_t);\n\
         extern int32_t sub(int32_t, int32_t);\n\
         int main(void) { return add(2, 3) - sub(10, 4); /* 5 - 6 = -1 → 255 */ }\n",
    )
    .unwrap();

    // Static link.
    let static_bin = dir.join("c_user_static");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/debug"))
        .arg("-lmathlib")
        .arg("-o")
        .arg(&static_bin)
        .status()
        .expect("clang static link");
    assert!(st.success(), "static link failed");
    let run = Command::new(&static_bin)
        .status()
        .expect("run static-linked");
    assert_eq!(
        run.code(),
        Some(255),
        "5 - 6 = -1 → 255 (u8) from static link"
    );

    // Dynamic link.
    let dyn_bin = dir.join("c_user_dyn");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/debug"))
        .arg("-lmathlib")
        .arg("-Wl,-rpath,@executable_path/target/debug")
        .arg("-o")
        .arg(&dyn_bin)
        .status()
        .expect("clang dynamic link");
    assert!(st.success(), "dynamic link failed");
    let run = Command::new(&dyn_bin)
        .current_dir(&dir)
        .status()
        .expect("run dynamic-linked");
    assert_eq!(
        run.code(),
        Some(255),
        "5 - 6 = -1 → 255 (u8) from dynamic link"
    );
}

#[test]
fn lib_target_rejects_fn_main_with_e0409() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"badlib\"\n\n[lib]\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure on lib + fn main");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0409"), "expected E0409, got: {stderr}");
}

#[test]
fn bin_and_lib_in_one_manifest_emit_e0408() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"both\"\n\n[[bin]]\nname = \"exe\"\n\n[lib]\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure on bin+lib");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0408"), "expected E0408, got: {stderr}");
}

#[test]
fn emit_obj_produces_relocatable_object() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("foo.cplus");
    std::fs::write(
        &src,
        "pub fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let out = dir.join("foo.o");
    let st = Command::new(cpc)
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc --emit-obj failed: {st}");
    assert!(out.is_file(), "expected {}", out.display());
    // File magic: 0xfeedfacf on Mach-O 64, ELF starts with 0x7f 'E' 'L' 'F',
    // a Windows COFF object starts with the 2-byte machine type
    // (0x8664 little-endian -> 0x64 0x86 for x86_64, 0xaa64 for arm64).
    let bytes = std::fs::read(&out).unwrap();
    let is_macho = bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe]);
    let is_elf = bytes.starts_with(&[0x7f, b'E', b'L', b'F']);
    let is_coff = bytes.starts_with(&[0x64, 0x86]) || bytes.starts_with(&[0x64, 0xaa]);
    assert!(
        is_macho || is_elf || is_coff,
        "expected Mach-O, ELF, or COFF object; first bytes: {:?}",
        &bytes[..4.min(bytes.len())]
    );
}

#[test]
fn lib_target_non_pub_fns_get_internal_linkage() {
    // Phase 5 Slice 5.B: only `pub` items expose external symbols. A
    // private helper called by a pub fn must NOT appear in `nm -g` output
    // of the resulting `.a`. `-O2` may inline it away entirely, which is
    // also fine (the assertion accepts either absent or internal).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"linkage\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub fn pub_api(x: i32) -> i32 { return helper(x); }\n\
         fn helper(x: i32) -> i32 { return x +% (1 as i32); }\n",
    )
    .unwrap();
    // Use release so -O2 + internal-linkage lets LTO fold helper away.
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let nm = Command::new(nm_prog())
        .arg("-g")
        .arg(dir.join("target/release/liblinkage.a"))
        .output()
        .expect("invoke nm");
    let out = String::from_utf8_lossy(&nm.stdout);
    // `pub_api` must be exported.
    assert!(
        out.contains(" _pub_api") || out.contains(" T pub_api"),
        "expected `pub_api` in nm -g output:\n{out}"
    );
    // `helper` must NOT be a globally-visible symbol — either inlined
    // away by LTO or carrying internal linkage.
    assert!(
        !out.contains(" _helper") && !out.contains(" T helper"),
        "private `helper` leaked into nm -g output:\n{out}"
    );
}

#[test]
fn lib_target_non_pub_methods_get_internal_linkage() {
    // Same property for `impl` block methods: only `pub fn` exposes
    // external symbols. Private methods used by pub ones stay internal.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"meth\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub struct Counter { v: i32 }\n\
         impl Counter {\n\
           pub fn make() -> Counter { return Counter { v: 0 }; }\n\
           pub fn value(self) -> i32 { return self.v; }\n\
           fn priv_bump(mut self) -> Counter { return Counter { v: self.v +% (1 as i32) }; }\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");
    let nm = Command::new(nm_prog())
        .arg("-g")
        .arg(dir.join("target/release/libmeth.a"))
        .output()
        .expect("invoke nm");
    let out = String::from_utf8_lossy(&nm.stdout);
    assert!(
        !out.contains("priv_bump"),
        "private method `priv_bump` leaked into nm -g output:\n{out}"
    );
}

// ---- Phase 5 Slice 5.F: reference example + design note ----

/// Drive the full `docs/examples/c_consumer/` workflow as a single CI test:
/// build the C+ library, compile + link the C consumer, run it, expect
/// `0 failure(s)` exit code. This is the closing-arc verification that
/// the whole user-facing story (5.A → 5.E) holds together.
#[test]
#[cfg(target_os = "macos")]
fn c_consumer_reference_example_runs_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR for this crate is `cpc/`. The reference example
    // lives at `<repo>/docs/examples/c_consumer/`.
    let example_root = manifest_dir
        .parent()
        .unwrap()
        .join("docs/examples/c_consumer");
    let mathlib_dir = example_root.join("mathlib");
    let c_user_dir = example_root.join("c_user");
    assert!(
        mathlib_dir.is_dir(),
        "expected reference mathlib at {}",
        mathlib_dir.display()
    );
    assert!(
        c_user_dir.is_dir(),
        "expected reference c_user at {}",
        c_user_dir.display()
    );

    // Clean any leftover artifacts so the test is hermetic.
    let _ = std::fs::remove_dir_all(mathlib_dir.join("target"));
    let _ = std::fs::remove_file(c_user_dir.join("c_user"));
    let _ = std::fs::remove_file(c_user_dir.join("c_user_dyn"));

    // 1. Build the library via cpc.
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&mathlib_dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of reference mathlib failed");

    // The build must have written all three artifacts: .a, .dylib, .h.
    let release_dir = mathlib_dir.join("target/release");
    assert!(
        release_dir.join("libmathlib.a").is_file(),
        "missing libmathlib.a"
    );
    assert!(
        release_dir.join("libmathlib.dylib").is_file(),
        "missing libmathlib.dylib"
    );
    assert!(release_dir.join("mathlib.h").is_file(), "missing mathlib.h");

    // 2. Compile + link the C consumer against the static lib.
    let c_user_bin = c_user_dir.join("c_user");
    let st = Command::new("clang")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-I")
        .arg(&release_dir)
        .arg(c_user_dir.join("c_user.c"))
        .arg(release_dir.join("libmathlib.a"))
        .arg("-o")
        .arg(&c_user_bin)
        .status()
        .expect("clang link");
    assert!(
        st.success(),
        "linking C consumer against libmathlib.a failed"
    );

    // 3. Run it. The binary returns the number of failures; expect 0.
    let run = Command::new(&c_user_bin).output().expect("run c_user");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("0 failure(s)"),
        "reference example reported failures:\nstdout=\n{stdout}\nstderr=\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(0), "c_user exited non-zero");

    // 4. Also try the dynamic-link path for parity.
    let c_user_dyn = c_user_dir.join("c_user_dyn");
    let st = Command::new("clang")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-I")
        .arg(&release_dir)
        .arg(c_user_dir.join("c_user.c"))
        .arg("-L")
        .arg(&release_dir)
        .arg("-lmathlib")
        .arg(format!("-Wl,-rpath,{}", release_dir.display()))
        .arg("-o")
        .arg(&c_user_dyn)
        .status()
        .expect("clang link dynamic");
    assert!(
        st.success(),
        "linking C consumer against libmathlib.dylib failed"
    );
    let run = Command::new(&c_user_dyn).status().expect("run c_user_dyn");
    assert_eq!(run.code(), Some(0));

    // 5. Leave the directory clean — keeps CI re-runs deterministic.
    let _ = std::fs::remove_file(&c_user_bin);
    let _ = std::fs::remove_file(&c_user_dyn);
    let _ = std::fs::remove_dir_all(mathlib_dir.join("target"));
}

// ---- Phase 5 Slice 5.E: --emit-header for auto-generated C declarations ----

#[test]
fn emit_header_basic_round_trip() {
    // The generated header must parse as valid C, contain a prototype
    // for each pub fn, and use the right C type names for primitives.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         pub extern fn noop() { return; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "--emit-header failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(h.contains("#pragma once"));
    assert!(h.contains("#include <stdint.h>"));
    assert!(
        h.contains("int32_t add(int32_t a, int32_t b);"),
        "missing add prototype in:\n{h}"
    );
    assert!(
        h.contains("void noop(void);"),
        "missing noop prototype in:\n{h}"
    );
}

#[test]
fn emit_header_renders_repr_c_struct() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "#[repr(C)]\n\
         pub struct Point { pub x: i32, pub y: i32 }\n\
         pub extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(h.contains("typedef struct Point"));
    assert!(h.contains("int32_t x;"));
    assert!(h.contains("int32_t y;"));
    assert!(h.contains("} Point;"));
    assert!(h.contains("int32_t square(Point p);"));
}

#[test]
fn emit_header_renders_plain_enum() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "pub enum Color { Red, Green, Blue }\n\
         pub extern fn first() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(h.contains("enum Color"), "missing enum in:\n{h}");
    assert!(h.contains("Color_Red = 0"));
    assert!(h.contains("Color_Green = 1"));
    assert!(h.contains("Color_Blue = 2"));
}

#[test]
fn emit_header_skips_non_pub_items() {
    // Non-`pub` fns must not appear in the header.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "pub extern fn pub_api(x: i32) -> i32 { return helper(x); }\n\
         fn helper(x: i32) -> i32 { return x +% (1 as i32); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(h.contains("int32_t pub_api(int32_t x);"));
    assert!(
        !h.contains("helper("),
        "non-pub `helper` leaked into header:\n{h}"
    );
}

#[test]
fn emit_header_skips_extern_import_declarations() {
    // `extern fn foo(...);` is an import (not an export). It should
    // not appear in the generated header — the header is what THIS
    // library exposes, not what it imports.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         pub extern fn my_alloc(n: usize) -> *u8 { return unsafe { malloc(n) }; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(
        h.contains("uint8_t * my_alloc(size_t n);"),
        "missing my_alloc; got:\n{h}"
    );
    assert!(
        !h.contains("uint8_t * malloc"),
        "import `malloc` leaked into header:\n{h}"
    );
}

#[test]
fn emit_header_passes_clang_syntax_check() {
    // Round-trip: the generated header must compile cleanly through
    // clang's syntax check (`-fsyntax-only`). Catches typos in the
    // type-mapping table.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("lib.cplus");
    std::fs::write(
        &src,
        "#[repr(C)]\n\
         pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }\n\
         pub enum Shape { Circle, Square, Triangle }\n\
         pub extern fn norm(v: Vec3) -> f32 {\n\
           return v.x * v.x + v.y * v.y + v.z * v.z;\n\
         }\n\
         pub extern fn area(s: Shape, side: f64) -> f64 { return side; }\n\
         pub extern fn buf_ptr(n: usize) -> *u8 { unsafe { return 0 as *u8; } }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-header")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let h_path = dir.join("lib.h");
    std::fs::write(&h_path, &out.stdout).unwrap();

    // Wrap the header in a translation unit and ask clang to parse it.
    let tu_path = dir.join("tu.c");
    std::fs::write(&tu_path, format!("#include \"{}\"\n", h_path.display())).unwrap();
    let clang = Command::new("clang")
        .arg("-fsyntax-only")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-x")
        .arg("c")
        .arg(&tu_path)
        .output()
        .expect("invoke clang");
    assert!(
        clang.status.success(),
        "clang rejected generated header:\nheader=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&clang.stderr),
    );
}

#[test]
fn lib_build_writes_libname_h_alongside_artifacts() {
    // `cpc build` on a [lib] manifest emits target/<mode>/<libname>.h
    // alongside the .a / .dylib so consumers can `#include` it directly.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hdrgen\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let h_path = dir.join("target/debug/hdrgen.h");
    assert!(
        h_path.is_file(),
        "expected generated header at {}",
        h_path.display()
    );
    let h = std::fs::read_to_string(&h_path).unwrap();
    assert!(
        h.contains("int32_t add(int32_t a, int32_t b);"),
        "header missing add prototype:\n{h}"
    );
}

#[test]
fn emit_header_requires_file_argument() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--emit-header")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure without FILE");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires a FILE argument"),
        "missing diagnostic, got: {stderr}"
    );
}

// ---- Phase 5 Slice 5.D: aggregate ABI coercion at the C boundary ----

#[test]
#[cfg(target_os = "macos")]
fn aggregate_param_8_bytes_round_trips() {
    // 8-byte struct (Point) — aarch64 PCS passes in a single GPR (i64).
    // Before 5.D, calling `square({3,4})` from C returned garbage; after,
    // it returns 25.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"abi8\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Point { x: i32, y: i32 }\n\
         pub extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         typedef struct { int32_t x; int32_t y; } Point;\n\
         extern int32_t square(Point);\n\
         int main(void) { Point p = {3, 4}; return square(p); /* 9 + 16 = 25 */ }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-labi8")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(25), "expected 25 = 3^2 + 4^2");
}

#[test]
#[cfg(target_os = "macos")]
fn aggregate_param_16_bytes_round_trips() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"abi16\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Pair { a: i64, b: i64 }\n\
         pub extern fn sum_pair(p: Pair) -> i64 { return p.a + p.b; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         typedef struct { int64_t a; int64_t b; } Pair;\n\
         extern int64_t sum_pair(Pair);\n\
         int main(void) { Pair p = {10, 20}; return (int)sum_pair(p); /* 30 */ }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-labi16")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(30));
}

#[test]
#[cfg(target_os = "macos")]
fn aggregate_param_24_bytes_indirect_round_trips() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"abi24\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
         pub extern fn sum_triple(t: Triple) -> i64 { return t.a + t.b + t.c; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         typedef struct { int64_t a; int64_t b; int64_t c; } Triple;\n\
         extern int64_t sum_triple(Triple);\n\
         int main(void) { Triple t = {100, 200, 300}; return (int)sum_triple(t); /* 600 */ }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-labi24")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(600 - 256 - 256)); // u8 truncation of 600 → 88
}

#[test]
#[cfg(target_os = "macos")]
fn aggregate_return_8_bytes_coerces() {
    // 8-byte struct return: aarch64 PCS packs into a single i64 register.
    // Verified by C caller reconstructing the struct from the returned bits.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"retc8\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Point { x: i32, y: i32 }\n\
         pub extern fn make_point(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         typedef struct { int32_t x; int32_t y; } Point;\n\
         extern Point make_point(int32_t, int32_t);\n\
         int main(void) {\n\
           Point p = make_point(7, 11);\n\
           if (p.x != 7) return 1;\n\
           if (p.y != 11) return 2;\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-lretc8")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0));
}

#[test]
#[cfg(target_os = "macos")]
fn aggregate_return_24_bytes_sret() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"retc24\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
         pub extern fn make_triple() -> Triple { return Triple { a: 11 as i64, b: 22 as i64, c: 33 as i64 }; }\n",
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success());

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         typedef struct { int64_t a; int64_t b; int64_t c; } Triple;\n\
         extern Triple make_triple(void);\n\
         int main(void) {\n\
           Triple t = make_triple();\n\
           if (t.a != 11) return 1;\n\
           if (t.b != 22) return 2;\n\
           if (t.c != 33) return 3;\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-lretc24")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0));
}

// ---- Phase 5 Slice 5.C: `pub extern fn body` C-callable exports ----

#[test]
#[cfg(target_os = "macos")]
fn pub_extern_fn_round_trips_through_c() {
    // Full end-to-end: build a C+ lib that exports `pub extern fn` definitions,
    // link from C, run, check return value.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"cexport\"\n\n[lib]\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "pub extern fn cab_add(a: i32, b: i32) -> i32 { return a + b; }\n\
         pub extern fn cab_neg(x: i32) -> i32 { return -x; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         extern int32_t cab_add(int32_t, int32_t);\n\
         extern int32_t cab_neg(int32_t);\n\
         int main(void) {\n\
           int r = cab_add(20, 22);  /* 42 */\n\
           if (cab_neg(r) != -42) return 1;\n\
           return r;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(dir.join("target/release"))
        .arg("-lcexport")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success(), "C link against pub extern fn lib failed");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected 42 from cab_add(20, 22)");
}

#[test]
fn pub_extern_fn_with_str_param_is_rejected_e0410() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "pub extern fn echo(s: str) -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected sema failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0410"), "expected E0410, got: {stderr}");
    assert!(
        stderr.contains("fat pointer"),
        "diagnostic should mention the fat-pointer reason: {stderr}"
    );
}

#[test]
fn exec_target_linkage_unchanged_by_5b() {
    // Regression guard: 5.B's `internal` linkage rule is gated on lib
    // mode. An executable build must not change symbol visibility for
    // non-pub helpers — the change is opt-in via `[lib]`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("exe.cplus");
    std::fs::write(
        &src,
        "fn double(x: i32) -> i32 { return x +% x; }\n\
         fn main() -> i32 { return double(21); }\n",
    )
    .unwrap();
    let bin = dir.join("exe");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42));
    // v0.0.3 Slice 3D: non-pub fns now get `internal` linkage in
    // executable builds too (was lib-only in Slice 5.B). LTO can strip
    // unused helpers from the final binary.
    let ll_out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll");
    let ir = String::from_utf8_lossy(&ll_out.stdout);
    // v0.0.8 fix C: non-pub fn → `internal fastcc`.
    assert!(
        ir.contains("define internal fastcc i32 @double("),
        "non-pub `double` must get `internal fastcc` linkage+cc in exe mode (3D + fix C); got:\n{ir}"
    );
}

#[test]
fn emit_obj_requires_output_path() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("foo.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--emit-obj")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected failure without `-o`");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires `-o"),
        "missing diagnostic, got: {stderr}"
    );
}

// ---- Phase 3A: bitshifts, bitwise ops, byte-swap intrinsics ----
//
// End-to-end smoke tests. The compiler emits IR; clang produces a binary;
// the runtime answer is byte-checked. Catches LLVM-rejected IR (mismatched
// shift widths, etc.) that pure codegen unit tests don't.

#[test]
fn bitshifts_and_bitwise_run_correctly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bits.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
           let port: u16 = 8080 as u16;\n\
           let hi: u16 = (port >> 8) & (0xff as u16);\n\
           let lo: u16 = port & (0xff as u16);\n\
           if hi != (31 as u16) { return 10; }\n\
           if lo != (144 as u16) { return 11; }\n\
           let xor: i32 = 0xf0 ^ 0x0f;\n\
           if xor != 0xff { return 12; }\n\
           let mask: u32 = ~(0 as u32);\n\
           if mask != (0xffffffff as u32) { return 13; }\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("bits");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "compile failed");
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(
        run.code(),
        Some(0),
        "binary returned {}, expected 0",
        run.code().unwrap_or(-1)
    );
}

#[test]
fn htons_round_trips_to_bswap() {
    // #htons(0x1234) on LE → 0x3412. Verify the binary's runtime answer.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("hs.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
           let p: u16 = 0x1234 as u16;\n\
           let s: u16 = #htons(p);\n\
           if s != (0x3412 as u16) { return 1; }\n\
           // round-trip: #htons(#htons(x)) == x.\n\
           let r: u16 = #htons(s);\n\
           if r != p { return 2; }\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("hs");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn bswap32_byte_reverses_correctly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bs.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
           let p: u32 = 0x12345678 as u32;\n\
           let s: u32 = #bswap32(p);\n\
           if s != (0x78563412 as u32) { return 1; }\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("bs");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn shift_count_widths_compose() {
    // i64 << u8 generated zext'd shift count. Verify runtime answer to
    // catch any IR-level type mismatches that LLVM would reject.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("sh.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
           let x: i64 = 1 as i64;\n\
           let n: u8 = 8 as u8;\n\
           let y: i64 = x << n;\n\
           if y != (256 as i64) { return 1; }\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("sh");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success());
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(0));
}

// ---- Phase 3B: reference programs smoke tests ----
//
// Each recipe under `docs/examples/recipes/<name>/` is a tiny `cpc build`
// project. The tests below copy each recipe to a tempdir (so we don't
// pollute the source tree with `target/` directories), build it, and
// exercise the resulting binary against a representative input. Recipes
// that use macOS-only APIs (argv via `_NSGetArgv`, etc.) are
// `#[cfg(target_os = "macos")]`-gated; the simpler recipes run cross-
// platform.
//
// For network recipes, we either use 127.0.0.1 with a short-lived
// netcat-style helper or skip the runtime check and verify compile-only.

#[cfg(test)]
fn copy_recipe_to_tempdir(name: &str) -> std::path::PathBuf {
    let dir = tempdir();
    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs/examples/recipes")
        .join(name);
    let src_dir = manifest_path.join("src");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::copy(manifest_path.join("Cplus.toml"), dir.join("Cplus.toml")).unwrap();
    for entry in std::fs::read_dir(&src_dir).expect("recipe src/ exists") {
        let e = entry.unwrap();
        let dest = dir.join("src").join(e.file_name());
        std::fs::copy(e.path(), dest).unwrap();
    }
    dir
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_env_var_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("env_var");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "env_var build failed");
    let out = Command::new(dir.join("target/debug/env_var"))
        .env("HOME", "/tmp/recipe-test")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("HOME=/tmp/recipe-test"), "got: {stdout}");
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_argv_parse_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("argv_parse");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "argv_parse build failed");
    let out = Command::new(dir.join("target/debug/argv_parse"))
        .args(["alpha", "beta", "gamma"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // argv[0] is the binary path; check the three custom args appear.
    assert!(stdout.contains("alpha\n"), "got: {stdout}");
    assert!(stdout.contains("beta\n"), "got: {stdout}");
    assert!(stdout.contains("gamma\n"), "got: {stdout}");
}

#[test]
fn recipe_stdin_lines_runs() {
    use std::io::Write;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("stdin_lines");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "stdin_lines build failed");
    let mut child = std::process::Command::new(dir.join("target/debug/stdin_lines"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"alpha\nbeta\ngamma\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "1: alpha\n2: beta\n3: gamma\n", "got: {stdout}");
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_file_read_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("file_read");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "file_read build failed");
    let test_file = dir.join("payload.txt");
    std::fs::write(&test_file, "the quick brown fox\n").unwrap();
    let out = Command::new(dir.join("target/debug/file_read"))
        .arg(&test_file)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "the quick brown fox\n", "got: {stdout}");
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_file_write_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("file_write");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "file_write build failed");
    let test_file = dir.join("out.txt");
    let st = Command::new(dir.join("target/debug/file_write"))
        .arg(&test_file)
        .arg("written by file_write")
        .status()
        .expect("run");
    assert!(st.success(), "file_write exited non-zero");
    let contents = std::fs::read_to_string(&test_file).expect("output exists");
    assert_eq!(contents, "written by file_write");
}

#[test]
fn recipe_hash_table_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("hash_table");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "hash_table build failed");
    let out = Command::new(dir.join("target/debug/hash_table"))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("4/4 ok"), "expected 4/4 ok, got: {stdout}");
}

#[test]
fn recipe_json_parse_runs() {
    use std::io::Write;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("json_parse");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "json_parse build failed");
    let mut child = std::process::Command::new(dir.join("target/debug/json_parse"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(br#"{"k":[1,true,null]}"#)
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("obj\n"), "got: {stdout}");
    assert!(stdout.contains("key \"k\"\n"), "got: {stdout}");
    assert!(stdout.contains("arr\n"), "got: {stdout}");
    assert!(stdout.contains("num 1\n"), "got: {stdout}");
    assert!(stdout.contains("bool true\n"), "got: {stdout}");
    assert!(stdout.contains("null\n"), "got: {stdout}");
}

#[test]
fn recipe_json_parse_rejects_malformed() {
    use std::io::Write;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("json_parse");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success());
    let mut child = std::process::Command::new(dir.join("target/debug/json_parse"))
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.as_mut().unwrap().write_all(b"{bad:1}").unwrap();
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_tcp_client_compiles() {
    // Compile-only: a full round-trip would need a server up — covered
    // by the tcp_server recipe below. This guards the build path.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("tcp_client");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "tcp_client build failed");
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_tcp_server_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    // Build both server and client into the same workflow.
    let server_dir = copy_recipe_to_tempdir("tcp_server");
    let client_dir = copy_recipe_to_tempdir("tcp_client");
    assert!(Command::new(cpc)
        .arg("build")
        .current_dir(&server_dir)
        .status()
        .unwrap()
        .success());
    assert!(Command::new(cpc)
        .arg("build")
        .current_dir(&client_dir)
        .status()
        .unwrap()
        .success());

    // Pick a high-numbered ephemeral port — collisions are unlikely
    // across parallel test runs, and the test exits even on failure
    // so a stuck server only leaks for the kernel-cleanup window.
    let port = 19200 + (std::process::id() % 2000);
    let server_bin = server_dir.join("target/debug/tcp_server");
    let client_bin = client_dir.join("target/debug/tcp_client");
    let mut server = Command::new(&server_bin)
        .arg(port.to_string())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let out = Command::new(&client_bin)
        .args(["127.0.0.1", &port.to_string(), "hello, server!"])
        .output()
        .expect("run client");
    let _ = server.wait();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "hello, server!", "got: {stdout}");
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_http_get_compiles() {
    // Compile-only — DNS / network reachability not assumed in CI.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("http_get");
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "http_get build failed");
}

/// v0.0.3 Phase 5 Slice 5D reference recipe: concurrent counter. Two
/// threads share a `*u64`; each performs 100_000 atomic increments.
/// The final value must be exactly 200_000 — atomic fetch_add ensures
/// no torn updates regardless of how the kernel schedules them.
#[test]
#[cfg(target_os = "macos")]
fn recipe_concurrent_counter_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("concurrent_counter");
    // Vendor-link both stdlib modules the recipe imports.
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "concurrent_counter build failed");
    let out = Command::new(dir.join("target/debug/concurrent_counter"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "concurrent_counter exited non-zero: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.0.3 Phase 5 Slice 5D ASan + TSan: real instrumentation. Builds
/// the concurrent_counter recipe with `--tsan` (then `--asan`) and
/// confirms ThreadSanitizer / AddressSanitizer reports clean. The
/// recipe is the canonical "shared mutable state via atomics" pattern
/// — exactly the case TSan was built to police. A regression that
/// broke atomic lowering (or introduced a non-atomic access on the
/// shared pointer) would surface here as a TSan data-race warning.
///
/// Implicit pre-condition: `cpc build` actually forwards
/// `--asan`/`--tsan` through to clang. v0.0.3 Slice 5D follow-up wired
/// this; before the fix, the flag was silently dropped and the binary
/// linked without sanitizer runtimes.
#[test]
#[cfg(target_os = "macos")]
fn recipe_concurrent_counter_tsan_and_asan_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    for san in ["--tsan", "--asan"] {
        let dir = copy_recipe_to_tempdir("concurrent_counter");
        std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
        std::fs::write(
            dir.join("vendor/stdlib/Cplus.toml"),
            "[package]\nname = \"stdlib\"\n",
        )
        .unwrap();
        let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
        let atomic_src = include_str!("../../vendor/stdlib/src/atomic.cplus");
        std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
        std::fs::write(dir.join("vendor/stdlib/src/atomic.cplus"), atomic_src).unwrap();
        let st = Command::new(cpc)
            .arg("build")
            .arg(san)
            .current_dir(&dir)
            .status()
            .expect("build");
        assert!(st.success(), "concurrent_counter build {san} failed");
        let out = Command::new(dir.join("target/debug/concurrent_counter"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "concurrent_counter under {san} exited non-zero: {:?} stderr={}",
            out.status.code(),
            stderr
        );
        assert!(
            !stderr.contains("WARNING: ThreadSanitizer"),
            "TSan flagged a race under {san}:\n{stderr}"
        );
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged an error under {san}:\n{stderr}"
        );
        assert!(
            !stderr.contains("LeakSanitizer"),
            "LSan flagged a leak under {san}:\n{stderr}"
        );
    }
}

/// v0.0.3 Phase 5 Slice 5D follow-up: confirm that swapping atomic
/// fetch_add for a non-atomic `*p +%= 1` makes TSan actually
/// fail. This is the "sanitizer is on" canary — without it, a future
/// regression that silently disabled `--tsan` propagation would leave
/// the previous test vacuously passing.
#[test]
#[cfg(target_os = "macos")]
fn racy_counter_provokes_tsan_warning() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"racy\"\n\n[[bin]]\nname = \"racy\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/thread\" as thread;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         fn bump_racy(counter: *u64) -> i32 {\n\
             let mut i: i32 = 0 as i32;\n\
             while i < (100000 as i32) {\n\
                 unsafe { *counter = *counter +% (1 as u64); }\n\
                 i = i +% (1 as i32);\n\
             }\n\
             return 0 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let counter: *u64 = unsafe { malloc(8 as usize) as *u64 };\n\
             unsafe { *counter = 0 as u64; }\n\
             let h1: thread::JoinHandle[i32] = thread::spawn_with::[*u64, i32](counter, bump_racy);\n\
             let h2: thread::JoinHandle[i32] = thread::spawn_with::[*u64, i32](counter, bump_racy);\n\
             let _r1: i32 = h1.join();\n\
             let _r2: i32 = h2.join();\n\
             unsafe { free(counter as *u8); }\n\
             return 0;\n\
         }\n",
    ).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--tsan")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "racy build under --tsan failed");
    let out = Command::new(dir.join("target/debug/racy"))
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("WARNING: ThreadSanitizer"),
        "expected TSan to flag the deliberate race; got:\n{stderr}"
    );
}

/// v0.0.3 Phase 5 Slice 5E reference recipe: async_compute. Chained
/// `async fn` + `await` + `executor::block_on` driving three nested
/// coroutines to completion. Validates the full async-syntax surface
/// + LLVM coroutine codegen + the stdlib executor's poll loop in one
/// shot.
/// v0.0.5 Phase 4 Slice 4E: async_fetch recipe round-trip. Exercises
/// method-form async TCP (`stream.write_all_async`, `stream.read_async`,
/// `stream.make_nonblocking`) end-to-end against a real localhost
/// echo server running in a sidecar Rust thread. The C+ client uses
/// `block_on` on a single async fn that connects, sends 'A', reads
/// the echoed byte. Validates 4B's method form drives the reactor
/// correctly through multi-level awaits inside the outer future.
///
/// **Concurrency note:** 4E's original 1000-task stress is blocked
/// on an executor improvement — nested awaits in `spawn_local`'d
/// futures don't get re-resumed when their awaitee completes (only
/// the *outer* future passed to `block_on` is re-driven on each loop
/// pass). Forward-pointed to Phase 5.
#[test]
#[cfg(target_os = "macos")]
fn recipe_async_fetch_runs() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("async_fetch");
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // Stage stdlib modules the recipe imports + their transitive deps.
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    let net_src = include_str!("../../vendor/stdlib/src/net.cplus");
    let result_src = include_str!("../../vendor/stdlib/src/result.cplus");
    let vec_src = include_str!("../../vendor/stdlib/src/vec.cplus");
    let iterator_src = include_str!("../../vendor/stdlib/src/iterator.cplus");
    let option_src = include_str!("../../vendor/stdlib/src/option.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
    // net.cplus imports stdlib/netsys for platform syscall constants; the
    // resolver loads netsys_linux.cplus on Linux. Stage both so the fixture
    // resolves on either OS.
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys.cplus"),
        include_str!("../../vendor/stdlib/src/netsys.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/netsys_linux.cplus"),
        include_str!("../../vendor/stdlib/src/netsys_linux.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "async_fetch build failed");
    // Bind to a free port on 127.0.0.1, accept one connection, echo
    // back whatever byte the client writes. Sidecar Rust thread does
    // the synchronous accept/read/write; the C+ binary is the async
    // client.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 1];
        conn.read_exact(&mut buf).expect("read");
        conn.write_all(&buf).expect("echo");
        // Hold the connection open briefly so the client's read
        // doesn't EOF instead of returning the byte. (TCP buffers
        // mean this typically isn't needed, but cheap insurance.)
        std::thread::sleep(std::time::Duration::from_millis(20));
        drop(conn);
    });
    let out = Command::new(dir.join("target/debug/async_fetch"))
        .env("FETCH_PORT", port.to_string())
        .output()
        .expect("run");
    server.join().expect("server thread");
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(
        code,
        0x41,
        "expected echoed 'A' (0x41); got code={code} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_async_yield_demo_runs() {
    // v0.0.4 Phase 3 Slice 3A.5: cooperative-multitasking recipe.
    // Three tasks each yield 4 times via spawn_local + yield_now;
    // verifies reactor-driven interleaving works end-to-end.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("async_yield_demo");
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    let reactor_src = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "async_yield_demo build failed");
    let out = Command::new(dir.join("target/debug/async_yield_demo"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "async_yield_demo exited non-zero: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[cfg(target_os = "macos")]
fn recipe_async_compute_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("async_compute");
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let future_src = include_str!("../../vendor/stdlib/src/future.cplus");
    let executor_src = include_str!("../../vendor/stdlib/src/executor.cplus");
    // v0.0.4 Phase 3 Slice 3A.1: executor.cplus now imports reactor.
    let __reactor_for_executor = include_str!("../../vendor/stdlib/src/reactor.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor.cplus"),
        __reactor_for_executor,
    )
    .unwrap();
    // On Linux the resolver loads reactor_linux.cplus (epoll) in place of
    // reactor.cplus (kqueue); stage it alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_linux.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_linux.cplus"),
    )
    .unwrap();
    // On Windows the resolver loads reactor_windows.cplus (Win32 timer +
    // pending/awaiter queues) in place of reactor.cplus (kqueue); stage it
    // alongside so the fixture links.
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor_windows.cplus"),
        include_str!("../../vendor/stdlib/src/reactor_windows.cplus"),
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/executor.cplus"), executor_src).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "async_compute build failed");
    let out = Command::new(dir.join("target/debug/async_compute"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "async_compute exited non-zero: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.0.3 Phase 5 Slice 5B reference recipe: parallel sum. Two threads
/// each compute half of `sum(1..=1000)`; parent joins both and adds the
/// partial results. Validates the cornerstone `thread::spawn[O]` +
/// `JoinHandle[O]::join(move self) -> O` flow under a real build.
#[test]
#[cfg(target_os = "macos")]
fn recipe_parallel_sum_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = copy_recipe_to_tempdir("parallel_sum");
    // Recipe uses stdlib/thread — link the stdlib vendor tree into the
    // tempdir before building. (`copy_recipe_to_tempdir` only ships
    // the recipe's own src + manifest.)
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let thread_src = include_str!("../../vendor/stdlib/src/thread.cplus");
    // v0.0.4 Phase 2 Slice 2H: thread.cplus now imports stdlib/atomic
    // for the refcounted-ctx dec on Drop. Stage atomic.cplus too.
    let __atomic_for_thread = include_str!("../../vendor/stdlib/src/atomic.cplus");
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        __atomic_for_thread,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/thread.cplus"), thread_src).unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("build");
    assert!(st.success(), "parallel_sum build failed");
    let out = Command::new(dir.join("target/debug/parallel_sum"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "parallel_sum exited non-zero: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.0.3 Phase 2 (CWE-377 hardening): use `tempfile::TempDir` so each
/// test gets a cryptographically random directory with secure mode bits,
/// not the predictable `cpc-test-<pid>-<nanos>-<counter>` shape. The
/// TempDir auto-cleans on drop, but we leak it via `Box::leak` so the
/// returned `PathBuf` stays valid for the rest of the test (matches the
/// pre-fix contract that returned a plain `PathBuf`).
/// v0.0.3 Slice 3E: CI lint that scans every `.cplus` source under
/// `docs/examples/projects/`, `docs/examples/recipes/`, and
/// `proves/benchmark/programs/<n>/cplus*/` for `import "..."` statements
/// and verifies each path follows v0.0.2 Slice 2B's rules:
///   - `./foo` or `../foo` → file-relative (always OK)
///   - `<dep>/<rest>` where `<dep>` is declared in the project's Cplus.toml
///   - no bare unqualified paths, no stale `.cplus` extension
///
/// Catches drift before it surfaces as user-build failures.
#[test]
fn ci_lint_imports_match_declared_deps() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let project_roots: Vec<std::path::PathBuf> = {
        let mut roots = Vec::new();
        // Project-mode trees we care about.
        let candidate_parents = [
            root.join("docs/examples/projects"),
            root.join("docs/examples/recipes"),
            root.join("proves/benchmark/programs"),
        ];
        for parent in candidate_parents {
            if !parent.is_dir() {
                continue;
            }
            // Walk one level: each immediate subdirectory MAY be a project.
            // For proves/benchmark/programs/<N>/, projects sit one level
            // deeper (e.g. `04-curl-lite/cplus`, `04-curl-lite/cplus-stdlib`).
            for entry in std::fs::read_dir(&parent).unwrap().flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                if p.join("Cplus.toml").is_file() {
                    roots.push(p.clone());
                    continue;
                }
                // Recurse one level for proves-style trees.
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for sub in rd.flatten() {
                        let sp = sub.path();
                        if sp.is_dir() && sp.join("Cplus.toml").is_file() {
                            roots.push(sp);
                        }
                    }
                }
            }
        }
        roots
    };

    let mut errors: Vec<String> = Vec::new();
    for proj in &project_roots {
        let manifest = std::fs::read_to_string(proj.join("Cplus.toml")).unwrap();
        // Cheap parse: gather `[dependencies]` table names.
        let mut declared_deps: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut in_deps = false;
        for line in manifest.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_deps = t == "[dependencies]";
                continue;
            }
            if in_deps {
                if let Some((name, _)) = t.split_once('=') {
                    let name = name.trim();
                    if !name.is_empty() && !name.starts_with('#') {
                        declared_deps.insert(name.to_string());
                    }
                }
            }
        }
        // Walk every .cplus under this project's src/.
        let src_dir = proj.join("src");
        if !src_dir.is_dir() {
            continue;
        }
        let mut stack = vec![src_dir];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("cplus") {
                    continue;
                }
                let body = std::fs::read_to_string(&p).unwrap();
                for (lineno, line) in body.lines().enumerate() {
                    let t = line.trim();
                    if !t.starts_with("import ") {
                        continue;
                    }
                    // Pull the quoted path out: import "..." as ...;
                    let Some(start) = t.find('"') else {
                        continue;
                    };
                    let after = &t[start + 1..];
                    let Some(end) = after.find('"') else {
                        continue;
                    };
                    let path = &after[..end];
                    if path.ends_with(".cplus") {
                        errors.push(format!(
                            "{}:{}: stale `.cplus` extension in `import \"{path}\"` (drop it)",
                            p.display(),
                            lineno + 1
                        ));
                        continue;
                    }
                    if path.starts_with("./") || path.starts_with("../") {
                        // file-relative, always OK
                        continue;
                    }
                    if let Some(slash) = path.find('/') {
                        let first = &path[..slash];
                        if !declared_deps.contains(first) {
                            errors.push(format!(
                                "{}:{}: bare import `\"{path}\"` first segment `{first}` not in [dependencies] of {}",
                                p.display(), lineno + 1, proj.join("Cplus.toml").display(),
                            ));
                        }
                    } else if !declared_deps.contains(path) {
                        errors.push(format!(
                            "{}:{}: bare unqualified import `\"{path}\"` — add `./` for file-relative or declare it as a dependency",
                            p.display(), lineno + 1,
                        ));
                    }
                }
            }
        }
    }
    if !errors.is_empty() {
        panic!(
            "CI lint found {} import drift(s):\n{}",
            errors.len(),
            errors.join("\n")
        );
    }
}

/// v0.0.6 Phase 2B: `vendor/appkit/src/convert.cplus` C+/ObjC data
/// bridge. Verifies the string + NSData round-trippers actually work
/// against a real autorelease pool. Smaller than the full appkit
/// smoke test because it touches Foundation only — no AppKit widgets,
/// no main thread requirements.
#[test]
#[cfg(target_os = "macos")]
fn appkit_bridge_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bridge_rt\"\n\n[[bin]]\nname = \"bridge_rt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\nappkit = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();

    // Symlink the in-tree vendor packages so the build picks up the
    // current convert.cplus + runtime.cplus.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/stdlib"), dir.join("vendor/stdlib")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/appkit"), dir.join("vendor/appkit")).unwrap();

    std::fs::write(
        dir.join("src/main.cplus"),
        r#"
import "appkit/convert" as bridge;
import "appkit/application" as application;
import "stdlib/vec" as vec;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();

    // string -> NSString -> string round-trip preserves content + length.
    let original: string = "hello, world".to_string();
    let ns: *u8 = bridge::cplus_string_to_nsstring(original);
    let back: string = bridge::nsstring_to_cplus_string(ns);
    if back.len() != (12 as usize) { return 1; }
    if back.as_str() != "hello, world" { return 2; }

    // str literal path.
    let ns2: *u8 = bridge::cplus_str_to_nsstring("bridge");
    let s2: string = bridge::nsstring_to_cplus_string(ns2);
    if s2.as_str() != "bridge" { return 3; }

    // Empty string is a corner the encoding-aware length path must handle.
    let ns3: *u8 = bridge::cplus_str_to_nsstring("");
    let s3: string = bridge::nsstring_to_cplus_string(ns3);
    if s3.len() != (0 as usize) { return 4; }

    // Vec[u8] -> NSData -> Vec[u8] copy round-trip.
    let mut bytes: vec::Vec[u8] = vec::Vec[u8]::with_capacity(4 as usize);
    bytes.push(10 as u8);
    bytes.push(20 as u8);
    bytes.push(30 as u8);
    bytes.push(40 as u8);
    let data: *u8 = bridge::vec_u8_to_nsdata(bytes);
    let back_bytes: vec::Vec[u8] = bridge::nsdata_to_vec_u8(data);
    if back_bytes.len() != (4 as usize) { return 5; }
    if vec::at_copy::[u8](back_bytes, 0 as usize) != (10 as u8) { return 6; }
    if vec::at_copy::[u8](back_bytes, 3 as usize) != (40 as u8) { return 7; }

    pool.drain();
    return 0;
}
"#,
    ).unwrap();

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build for bridge round-trip failed: {status}");

    let bin = dir.join("target/debug/bridge_rt");
    assert!(bin.is_file(), "expected binary at {}", bin.display());

    let run = Command::new(bin).status().expect("run bridge_rt");
    assert!(run.success(), "bridge_rt exited non-zero: {run}");
}

/// Helper for the AppKit runtime round-trip tests below: stand up a tempdir
/// project that depends on the in-tree `vendor/{stdlib,appkit}` (via symlink so
/// edits are picked up), build it, run it, and assert exit 0. The program is
/// expected to use distinct non-zero return codes per failed assertion.
#[cfg(target_os = "macos")]
fn appkit_run_program(pkg: &str, program: &str) {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        format!("[package]\nname = \"{pkg}\"\n\n[[bin]]\nname = \"{pkg}\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\nappkit = \"*\"\n"),
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/stdlib"), dir.join("vendor/stdlib")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/appkit"), dir.join("vendor/appkit")).unwrap();
    std::fs::write(dir.join("src/main.cplus"), program).unwrap();

    let status = Command::new(cpc).arg("build").current_dir(&dir).status().expect("invoke cpc build");
    assert!(status.success(), "cpc build for {pkg} failed: {status}");
    let bin = dir.join(format!("target/debug/{pkg}"));
    assert!(bin.is_file(), "expected binary at {}", bin.display());
    let run = Command::new(bin).status().expect("run program");
    assert!(run.success(), "{pkg} exited non-zero: {run}");
}

/// v0.0.16 AppKit ownership/Drop model (plan.appkit.md §2): the `rt::retain` /
/// `rt::release` / `rt::retain_count` primitives behave, and an owned wrapper
/// (`Alert`, created `new` = +1) releases its object in `drop` — so building
/// many in a loop neither leaks nor over-releases (a crash). Foundation-only,
/// so it needs no window server.
#[test]
#[cfg(target_os = "macos")]
fn appkit_ownership_round_trip() {
    appkit_run_program(
        "ak_own",
        r#"
import "appkit/runtime" as rt;
import "appkit/dialogs" as dialogs;

fn main() -> i32 {
    let cls: *u8 = rt::get_class(#str_ptr("NSObject\0"));
    let obj: *u8 = rt::msg_id(cls, rt::sel(#str_ptr("new\0")));   // +1
    if rt::retain_count(obj) != (1 as i64) { return 1; }
    let _r: *u8 = rt::retain(obj);                                // +2
    if rt::retain_count(obj) != (2 as i64) { return 2; }
    rt::release(obj);                                             // +1
    if rt::retain_count(obj) != (1 as i64) { return 3; }
    rt::release(obj);                                             // 0 -> dealloc

    // Owned wrapper: each Alert drops (releases) at end of iteration.
    let mut i: i32 = 0;
    loop {
        if i >= (500 as i32) { break; }
        let a: dialogs::Alert = dialogs::Alert::new();
        a.set_message_text(#str_ptr("hi\0"));
        i = i +% (1 as i32);
    }
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit "+1 normal form" ownership (plan.appkit.md §2): an owned
/// widget wrapper (`Button`, +1) added to a parent survives its wrapper's `drop`
/// — the parent retained it, and `drop` releases only the wrapper's +1. We add a
/// button inside a helper (so its wrapper drops on return), then confirm the
/// content view still holds exactly one live subview (messaging it doesn't trap).
#[test]
#[cfg(target_os = "macos")]
fn appkit_owned_widget_survives_wrapper_drop() {
    appkit_run_program(
        "ak_own2",
        r#"
import "appkit/application" as application;
import "appkit/window" as window;
import "appkit/controls" as controls;
import "appkit/runtime" as rt;

fn add_button(content: *u8) {
    let b: controls::Button = controls::Button::new(
        rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 80.0, height: 24.0 } });
    b.set_title(#str_ptr("Hi\0"));
    rt::msg_void_id(content, rt::sel(#str_ptr("addSubview:\0")), b.obj);
    return;   // b drops here -> release; content retained it, so it survives
}

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 200.0, height: 200.0 } };
    let win = window::Window::new(f, 15 as u64, 2 as u64, 0 as i8);
    let content: *u8 = win.content_view();
    add_button(content);

    let subs: *u8 = rt::msg_id(content, rt::sel(#str_ptr("subviews\0")));
    if rt::msg_i64(subs, rt::sel(#str_ptr("count\0"))) != (1 as i64) { return 1; }
    let btn: *u8 = rt::msg_id(subs, rt::sel(#str_ptr("firstObject\0")));
    let _tag: i64 = rt::msg_i64(btn, rt::sel(#str_ptr("tag\0")));   // traps if freed
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit `pasteboard.cplus` (plan.appkit.md §4): the system clipboard
/// round-trips a string (write -> read -> compare), twice, proving clear/
/// set_string/string_ns and the `opaque` (non-owned singleton) handling.
#[test]
#[cfg(target_os = "macos")]
fn appkit_pasteboard_round_trip() {
    appkit_run_program(
        "ak_pb",
        r#"
import "appkit/application" as application;
import "appkit/pasteboard" as pb;
import "appkit/convert" as conv;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let board: pb::Pasteboard = pb::Pasteboard::general();
    let _cc: i64 = board.clear();
    if board.set_string(#str_ptr("clip-test-123\0")) != (1 as i8) { return 1; }
    let got_ns: *u8 = board.string_ns();
    if got_ns == unsafe { 0 as *u8 } { return 2; }
    if conv::nsstring_to_cplus_string(got_ns).as_str() != "clip-test-123" { return 3; }
    let _cc2: i64 = board.clear();
    let _ok2: i8 = board.set_string(#str_ptr("second\0"));
    if conv::nsstring_to_cplus_string(board.string_ns()).as_str() != "second" { return 4; }
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit `layout.cplus` (plan.appkit.md §4, Auto Layout): anchor
/// constraints build, activate, read their constant back, and deactivate.
/// NSView + constraints need no run loop, so this is headless-safe.
#[test]
#[cfg(target_os = "macos")]
fn appkit_autolayout_constraints() {
    appkit_run_program(
        "ak_layout",
        r#"
import "appkit/application" as application;
import "appkit/view" as view;
import "appkit/layout" as layout;
import "appkit/runtime" as rt;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let pf = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 400.0, height: 400.0 } };
    let cf = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 10.0, height: 10.0 } };
    let parent: view::View = view::View::new(pf);
    let child: view::View = view::View::new(cf);
    parent.add_subview(child.obj);
    layout::use_constraints(child.obj);
    let c1: *u8 = layout::activate(layout::equal(layout::leading(child.obj), layout::leading(parent.obj)));
    let c2: *u8 = layout::activate(layout::equal_const(layout::width(child.obj), 200.0));
    if layout::is_active(c1) != (1 as i8) { return 1; }
    if layout::is_active(c2) != (1 as i8) { return 2; }
    let w: f64 = rt::msg_f64(c2, rt::sel(#str_ptr("constant\0")));
    if w < 199.5 { return 3; }
    if w > 200.5 { return 4; }
    let _d: *u8 = layout::deactivate(c2);
    if layout::is_active(c2) != (0 as i8) { return 5; }
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit `notifications.cplus` (plan.appkit.md §3): NSNotificationCenter
/// subscribe -> post (callback fires) -> drop the Observer (unsubscribe) -> post
/// (callback no longer fires). Exercises the synthesized observer class, the
/// associated-object callback dispatch, and the Observer's removeObserver+release
/// drop. Foundation-only, so no window server needed.
#[test]
#[cfg(target_os = "macos")]
fn appkit_notification_subscribe_and_unsubscribe() {
    appkit_run_program(
        "ak_notify",
        r#"
import "appkit/application" as application;
import "appkit/notifications" as notify;

static mut COUNT: i32 = 0;

fn on_note(note: *u8) {
    unsafe { COUNT = COUNT +% (1 as i32); };
    return;
}

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let center: notify::NotificationCenter = notify::NotificationCenter::default();
    {
        let obs: notify::Observer = center.add_observer(#str_ptr("CPlusTestNote\0"), on_note);
        center.post(#str_ptr("CPlusTestNote\0"));
        center.post(#str_ptr("CPlusTestNote\0"));
    }
    // Two posts while subscribed.
    if unsafe { COUNT } != (2 as i32) { return 1; }
    // Observer dropped above -> unsubscribed; this post must not fire.
    center.post(#str_ptr("CPlusTestNote\0"));
    if unsafe { COUNT } != (2 as i32) { return 2; }
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit delegate/data-source synthesis (plan.appkit.md §3):
/// `data::create_table_data_source` builds an `NSTableViewDataSource` from two
/// C+ method implementations. We invoke the synthesized methods directly
/// (`numberOfRowsInTableView:` and `tableView:objectValueForTableColumn:row:`)
/// and assert the C+-computed return values — no real table or run loop needed.
#[test]
#[cfg(target_os = "macos")]
fn appkit_table_data_source_synthesis() {
    appkit_run_program(
        "ak_ds",
        r#"
import "appkit/application" as application;
import "appkit/data" as data;
import "appkit/runtime" as rt;
import "appkit/convert" as conv;

fn ds_row_count(self_obj: *u8, _cmd: *u8, table: *u8) -> i64 {
    return 3 as i64;
}

fn ds_value(self_obj: *u8, _cmd: *u8, table: *u8, column: *u8, row: i64) -> *u8 {
    if row == (0 as i64) { return rt::ns_string(#str_ptr("row-0\0")); }
    if row == (1 as i64) { return rt::ns_string(#str_ptr("row-1\0")); }
    return rt::ns_string(#str_ptr("row-2\0"));
}

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let ds: *u8 = data::create_table_data_source(ds_row_count, ds_value);

    let n: i64 = rt::msg_i64_id(ds, rt::sel(#str_ptr("numberOfRowsInTableView:\0")), unsafe { 0 as *u8 });
    if n != (3 as i64) { return 1; }

    let nil: *u8 = unsafe { 0 as *u8 };
    let sel_v: *u8 = rt::sel(#str_ptr("tableView:objectValueForTableColumn:row:\0"));
    if conv::nsstring_to_cplus_string(rt::msg_id_id_id_i64(ds, sel_v, nil, nil, 0 as i64)).as_str() != "row-0" { return 2; }
    if conv::nsstring_to_cplus_string(rt::msg_id_id_id_i64(ds, sel_v, nil, nil, 2 as i64)).as_str() != "row-2" { return 3; }

    rt::release(ds);
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit window + table-selection delegate synthesis (plan.appkit.md
/// §3): `window::create_window_delegate` and `data::create_table_delegate` build
/// delegate objects from C+ method bodies. We invoke the synthesized methods
/// directly — `windowShouldClose:` returns the C+ value, `windowWillClose:` and
/// `tableViewSelectionDidChange:` fire their handlers — and check
/// `TableView::selected_row()` reads -1 on a fresh table. No run loop needed.
#[test]
#[cfg(target_os = "macos")]
fn appkit_window_and_table_delegates() {
    appkit_run_program(
        "ak_deleg",
        r#"
import "appkit/application" as application;
import "appkit/window" as window;
import "appkit/data" as data;
import "appkit/runtime" as rt;

static mut WILL_CLOSE: i32 = 0;
static mut SEL_CHANGED: i32 = 0;

fn should_close(self_obj: *u8, _cmd: *u8, sender: *u8) -> i8 { return 1 as i8; }
fn will_close(self_obj: *u8, _cmd: *u8, note: *u8) { unsafe { WILL_CLOSE = 1 as i32; }; return; }
fn sel_changed(self_obj: *u8, _cmd: *u8, note: *u8) { unsafe { SEL_CHANGED = 1 as i32; }; return; }

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let nil: *u8 = unsafe { 0 as *u8 };

    let wd: *u8 = window::create_window_delegate(should_close, will_close);
    if rt::msg_i8_id(wd, rt::sel(#str_ptr("windowShouldClose:\0")), nil) != (1 as i8) { return 1; }
    rt::msg_void_id(wd, rt::sel(#str_ptr("windowWillClose:\0")), nil);
    if unsafe { WILL_CLOSE } != (1 as i32) { return 2; }

    let td: *u8 = data::create_table_delegate(sel_changed);
    rt::msg_void_id(td, rt::sel(#str_ptr("tableViewSelectionDidChange:\0")), nil);
    if unsafe { SEL_CHANGED } != (1 as i32) { return 3; }

    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 10.0, height: 10.0 } };
    let table: data::TableView = data::TableView::new(f);
    if table.selected_row() != (0 as i64) -% (1 as i64) { return 4; }

    rt::release(wd);
    rt::release(td);
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit `BezierPath` (plan.appkit.md §4, custom drawing): our wrapper
/// marshals the NSPoint args for moveToPoint:/lineToPoint: and reads elementCount
/// back. A path is a data object, so this needs no drawing context.
#[test]
#[cfg(target_os = "macos")]
fn appkit_bezier_path_build() {
    appkit_run_program(
        "ak_path",
        r#"
import "appkit/application" as application;
import "appkit/graphics" as graphics;
import "appkit/runtime" as rt;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let path: graphics::BezierPath = graphics::BezierPath::new();
    path.move_to(0.0, 0.0);
    path.line_to(10.0, 10.0);
    path.line_to(20.0, 0.0);
    if path.element_count() != (3 as i64) { return 1; }
    path.set_line_width(2.0);
    path.close();
    let r = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 5.0, height: 5.0 } };
    let path2: graphics::BezierPath = graphics::BezierPath::new();
    path2.append_rect(r);
    if path2.element_count() < (4 as i64) { return 2; }
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit custom view (plan.appkit.md §4, custom drawing):
/// `view::create_custom_view` synthesizes an NSView subclass whose `drawRect:`
/// is a C+ function. We invoke `drawRect:` directly with a known NSRect and
/// assert it round-trips by value into the IMP (the hard part — a struct arg in
/// a synthesized method). No display/run loop needed.
#[test]
#[cfg(target_os = "macos")]
fn appkit_custom_view_draw_rect() {
    appkit_run_program(
        "ak_draw",
        r#"
import "appkit/application" as application;
import "appkit/view" as view;
import "appkit/runtime" as rt;

static mut DREW: i32 = 0;
static mut DRAW_W: f64 = 0.0;

fn my_draw(self_obj: *u8, _cmd: *u8, dirty: rt::Rect) {
    unsafe { DREW = 1 as i32; };
    unsafe { DRAW_W = dirty.size.width; };
    return;
}

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 100.0, height: 100.0 } };
    let v: view::View = view::create_custom_view(f, my_draw);
    let dirty = rt::Rect { origin: rt::Point { x: 1.0, y: 2.0 }, size: rt::Size { width: 42.0, height: 7.0 } };
    rt::msg_void_rect(v.obj, rt::sel(#str_ptr("drawRect:\0")), dirty);
    if unsafe { DREW } != (1 as i32) { return 1; }
    if unsafe { DRAW_W } != 42.0 { return 2; }
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit Auto Layout depth (plan.appkit.md §4): constraint priorities
/// (NSLayoutPriority `float`) and NSLayoutGuide. Our wrapper sets/reads a
/// priority and constrains against a layout guide. Constraints/guides are data
/// objects — no run loop needed.
#[test]
#[cfg(target_os = "macos")]
fn appkit_layout_priority_and_guide() {
    appkit_run_program(
        "ak_lp",
        r#"
import "appkit/application" as application;
import "appkit/view" as view;
import "appkit/layout" as layout;
import "appkit/runtime" as rt;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 400.0, height: 400.0 } };
    let parent: view::View = view::View::new(f);
    let child: view::View = view::View::new(f);
    parent.add_subview(child.obj);
    layout::use_constraints(child.obj);

    let c: *u8 = layout::equal_const(layout::width(child.obj), 50.0);
    let _p: *u8 = layout::set_priority(c, layout::priority_high());
    if layout::priority(c) != (750.0 as f32) { return 1; }
    let _a: *u8 = layout::activate(c);
    if layout::is_active(c) != (1 as i8) { return 2; }

    let guide: *u8 = layout::add_guide(parent.obj);
    let c2: *u8 = layout::activate(layout::equal(layout::leading(child.obj), layout::leading(guide)));
    if layout::is_active(c2) != (1 as i8) { return 3; }

    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit drag-and-drop destination (plan.appkit.md §4):
/// `drag::create_drag_destination_view` synthesizes an NSView that accepts
/// drops. We register it and invoke the synthesized `NSDraggingDestination`
/// methods directly — `draggingEntered:` returns the accepted operation,
/// `performDragOperation:` returns success and fires its handler. The live drag
/// session (NSDraggingInfo) isn't exercised headlessly; this verifies our
/// synthesis + return marshaling.
#[test]
#[cfg(target_os = "macos")]
fn appkit_drag_destination() {
    appkit_run_program(
        "ak_drag",
        r#"
import "appkit/application" as application;
import "appkit/drag" as drag;
import "appkit/runtime" as rt;

static mut DROPPED: i32 = 0;

fn on_entered(self_obj: *u8, _cmd: *u8, info: *u8) -> i64 { return drag::drag_op_copy(); }
fn on_perform(self_obj: *u8, _cmd: *u8, info: *u8) -> i8 { unsafe { DROPPED = 1 as i32; }; return 1 as i8; }

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 100.0, height: 100.0 } };
    let v: *u8 = drag::create_drag_destination_view(f, on_entered, on_perform);
    drag::register_for_string_drops(v);

    let nil: *u8 = unsafe { 0 as *u8 };
    if rt::msg_i64_id(v, rt::sel(#str_ptr("draggingEntered:\0")), nil) != drag::drag_op_copy() { return 1; }
    if rt::msg_i8_id(v, rt::sel(#str_ptr("performDragOperation:\0")), nil) != (1 as i8) { return 2; }
    if unsafe { DROPPED } != (1 as i32) { return 3; }

    rt::release(v);
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit drag source (plan.appkit.md §4):
/// `drag::create_drag_source_view` synthesizes an NSView that is an
/// NSDraggingSource + drag initiation. We invoke the source view's
/// `draggingSession:sourceOperationMaskForDraggingContext:` directly and assert
/// the C+ op mask, build a DraggingItem with a real frame + image (the
/// setDraggingFrame:contents: rect+id call — exercises the HFA struct-arg ABI),
/// and register a `mouseDragged:` handler that calls begin_string_drag (the live
/// drag itself needs a real NSEvent — confirmed by hand, see the recipe).
#[test]
#[cfg(target_os = "macos")]
fn appkit_drag_source() {
    appkit_run_program(
        "ak_dsrc",
        r#"
import "appkit/application" as application;
import "appkit/drag" as drag;
import "appkit/runtime" as rt;

fn src_mask(self_obj: *u8, _cmd: *u8, session: *u8, context: i64) -> i64 {
    return drag::drag_op_copy();
}

// -mouseDragged: — fired by AppKit on a real drag gesture (not headless). The
// live NSEvent flows straight into begin_string_drag; registering this links the
// whole initiation path.
fn src_dragged(self_obj: *u8, _cmd: *u8, event: *u8) {
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 60.0, height: 20.0 } };
    drag::begin_string_drag(self_obj, event, #str_ptr("payload\0"), f, unsafe { 0 as *u8 });
    return;
}

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let f = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 100.0, height: 100.0 } };
    let v: *u8 = drag::create_drag_source_view(f, src_mask, src_dragged);
    let nil: *u8 = unsafe { 0 as *u8 };
    let m: i64 = rt::msg_i64_id_i64(v, rt::sel(#str_ptr("draggingSession:sourceOperationMaskForDraggingContext:\0")), nil, 0 as i64);
    if m != drag::drag_op_copy() { return 1; }

    // DraggingItem + setDraggingFrame:contents: (rect+id) — headless-safe now
    // that struct args pass correctly. Use a real NSImage as contents.
    let img: *u8 = rt::alloc_init(#str_ptr("NSImage\0"));
    let item = drag::DraggingItem::from_string(#str_ptr("hi\0"));
    item.set_dragging_frame_contents(f, img);
    if item.obj == nil { return 2; }

    rt::release(v);
    pool.drain();
    return 0;
}
"#,
    );
}

/// v0.0.16 AppKit master/detail milestone app (plan.appkit.md §5): the
/// `appkit_list_detail` recipe ties the binding surface together (table data
/// source + selection delegate, menu, controls, app delegate). It's a GUI app
/// (`app.run()` blocks), so this is compile + link validation only — it builds
/// the real recipe source against the in-tree vendor packages.
#[test]
#[cfg(target_os = "macos")]
fn appkit_list_detail_recipe_builds() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let recipe = root.join("docs/examples/recipes/appkit_list_detail");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::copy(recipe.join("Cplus.toml"), dir.join("Cplus.toml")).unwrap();
    std::fs::copy(recipe.join("src/main.cplus"), dir.join("src/main.cplus")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/stdlib"), dir.join("vendor/stdlib")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/appkit"), dir.join("vendor/appkit")).unwrap();

    let status = Command::new(cpc).arg("build").current_dir(&dir).status().expect("invoke cpc build");
    assert!(status.success(), "appkit_list_detail recipe failed to build");
    assert!(
        dir.join("target/debug/appkit_list_detail").is_file(),
        "expected the list_detail binary"
    );
}

/// The `appkit_drag_drop` recipe — a runnable drag SOURCE (mouseDragged: ->
/// begin_string_drag) + DESTINATION (performDragOperation:) demo. GUI app
/// (`app.run()` blocks), so compile + link validation only; the live drag
/// gesture is a manual test.
#[test]
#[cfg(target_os = "macos")]
fn appkit_drag_drop_recipe_builds() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let recipe = root.join("docs/examples/recipes/appkit_drag_drop");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::copy(recipe.join("Cplus.toml"), dir.join("Cplus.toml")).unwrap();
    std::fs::copy(recipe.join("src/main.cplus"), dir.join("src/main.cplus")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/stdlib"), dir.join("vendor/stdlib")).unwrap();
    std::os::unix::fs::symlink(root.join("vendor/appkit"), dir.join("vendor/appkit")).unwrap();

    let status = Command::new(cpc).arg("build").current_dir(&dir).status().expect("invoke cpc build");
    assert!(status.success(), "appkit_drag_drop recipe failed to build");
    assert!(
        dir.join("target/debug/appkit_drag_drop").is_file(),
        "expected the drag_drop binary"
    );
}

/// vendor/appkit `controls` coverage: construct + configure every control type
/// and read back the value-bearing ones. AppKit object construction + property
/// setters are headless-safe (no window server), so this exercises the wrapper
/// msgSends end to end (incl. the owned TextField/Button Drop path and one
/// `TextField::new_label` must produce a *static* label — non-editable,
/// non-bezeled — not the default editable NSTextField (which renders as an input
/// box and silently accepts dropped/typed text). Regression for the drag-drop
/// demo where instruction "labels" were swallowing the dragged payload.
#[test]
#[cfg(target_os = "macos")]
fn appkit_new_label_is_static() {
    appkit_run_program(
        "ak_label",
        r#"
import "appkit/runtime" as rt;
import "appkit/controls" as controls;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 120.0, height: 24.0 } };
    let label = controls::TextField::new_label(f);
    if rt::msg_i8(label.obj, rt::sel(#str_ptr("isEditable\0"))) != (0 as i8) { return 1; }
    if rt::msg_i8(label.obj, rt::sel(#str_ptr("isBezeled\0"))) != (0 as i8) { return 2; }
    if rt::msg_i8(label.obj, rt::sel(#str_ptr("isSelectable\0"))) != (0 as i8) { return 3; }
    // An input field stays editable (the contrast case).
    let field = controls::TextField::new_input_field(f);
    if rt::msg_i8(field.obj, rt::sel(#str_ptr("isEditable\0"))) != (1 as i8) { return 4; }
    return 0;
}
"#,
    );
}

/// `attach_callback`). Scope is the vendor wrappers, not Apple's widget behavior.
#[test]
#[cfg(target_os = "macos")]
fn appkit_controls_construct_and_configure() {
    appkit_run_program(
        "ak_controls",
        r#"
import "appkit/runtime" as rt;
import "appkit/controls" as controls;

fn on_action(sender: *u8) { return; }

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 120.0, height: 24.0 } };

    let label = controls::TextField::new_label(f);
    label.set_string_value(#str_ptr("hello\0"));
    label.set_bezeled(0 as i8);
    label.set_editable(0 as i8);

    let field = controls::TextField::new_input_field(f);
    field.set_placeholder_string(#str_ptr("name\0"));
    field.set_string_value(#str_ptr("abc\0"));

    let btn = controls::Button::new(f);
    btn.set_title(#str_ptr("OK\0"));
    btn.set_enabled(1 as i8);
    btn.set_state(1 as i64);
    if btn.state() != (1 as i64) { return 1; }
    btn.set_on_click(on_action);

    let slider = controls::Slider::new(f);
    slider.set_min_value(0.0);
    slider.set_max_value(10.0);
    slider.set_double_value(5.0);
    if slider.double_value() < (4.0) { return 2; }

    let pi = controls::ProgressIndicator::new(f);
    pi.set_indeterminate(0 as i8);
    pi.set_double_value(0.5);

    let popup = controls::PopUpButton::new(f, 0 as i8);
    popup.add_item(#str_ptr("A\0"));
    popup.add_item(#str_ptr("B\0"));
    popup.select_item_at_index(1 as i64);
    if popup.index_of_selected_item() != (1 as i64) { return 3; }

    let stepper = controls::Stepper::new(f);
    stepper.set_min_value(0.0);
    stepper.set_max_value(9.0);
    stepper.set_double_value(2.0);
    if stepper.double_value() < (1.0) { return 4; }

    let sw = controls::Switch::new(f);
    sw.set_state(1 as i64);
    if sw.state() != (1 as i64) { return 5; }

    let seg = controls::SegmentedControl::new(f);
    seg.set_segment_count(2 as i64);
    seg.set_label_for_segment(#str_ptr("L\0"), 0 as i64);
    seg.set_selected_for_segment(1 as i8, 0 as i64);

    let dp = controls::DatePicker::new(f);
    dp.set_date_picker_style(0 as i64);

    let cw = controls::ColorWell::new(f);
    cw.deactivate();

    let li = controls::LevelIndicator::new(f);
    li.set_max_value(5.0);
    li.set_double_value(3.0);
    if li.double_value() < (2.0) { return 6; }

    let pc = controls::PathControl::new(f);
    pc.set_path_style(0 as i64);

    return 0;
}
"#,
    );
}

/// v0.0.16: the value controls (Slider, Stepper, …) are now owned (+1 normal
/// form) — `new` is `alloc/init` (+1), `drop` releases once. This stresses that:
/// (1) building+dropping 200 in a loop neither leaks nor double-frees (a double
/// release would trap on a reused address), and (2) after a wrapper is added to
/// a parent (which retains) and then dropped, the control survives via the
/// parent (subview count stays 1) — the +1 normal form, not an over-release.
#[test]
#[cfg(target_os = "macos")]
fn appkit_owned_controls_drop_balanced() {
    appkit_run_program(
        "ak_ctl_drop",
        r#"
import "appkit/runtime" as rt;
import "appkit/controls" as controls;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 80.0, height: 20.0 } };

    let mut i: i32 = 0;
    while i < 200 {
        let s = controls::Slider::new(f);
        s.set_double_value(1.0);
        let st = controls::Stepper::new(f);
        st.set_max_value(5.0);
        i = i + 1;
    }

    let parent: *u8 = rt::alloc_init_with_frame(#str_ptr("NSView\0"), f);
    {
        let s = controls::Slider::new(f);
        s.set_double_value(7.0);
        rt::msg_void_id(parent, rt::sel(#str_ptr("addSubview:\0")), s.obj);
    }
    let subs: *u8 = rt::msg_id(parent, rt::sel(#str_ptr("subviews\0")));
    if rt::msg_u64(subs, rt::sel(#str_ptr("count\0"))) != (1 as u64) { return 1; }

    return 0;
}
"#,
    );
}

/// v0.0.16: the base views (View, StackView, Box, Scroller) are now owned (+1
/// normal form), like the controls. Same balance check: build+drop 200 in a loop
/// (no double-free/leak), and a child added to a parent survives the wrapper's
/// drop (subview count stays 1).
#[test]
#[cfg(target_os = "macos")]
fn appkit_owned_views_drop_balanced() {
    appkit_run_program(
        "ak_view_drop",
        r#"
import "appkit/runtime" as rt;
import "appkit/view" as view;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 50.0, height: 50.0 } };

    let mut i: i32 = 0;
    while i < 200 {
        let v = view::View::new(f);
        let sv = view::StackView::new(f);
        let bx = view::Box::new(f);
        i = i + 1;
    }

    let parent: *u8 = rt::alloc_init_with_frame(#str_ptr("NSView\0"), f);
    {
        let child = view::View::new(f);
        rt::msg_void_id(parent, rt::sel(#str_ptr("addSubview:\0")), child.obj);
    }
    let subs: *u8 = rt::msg_id(parent, rt::sel(#str_ptr("subviews\0")));
    if rt::msg_u64(subs, rt::sel(#str_ptr("count\0"))) != (1 as u64) { return 1; }

    return 0;
}
"#,
    );
}

/// vendor/appkit `text` coverage: construct + configure the text-entry widgets
/// (TextView, SecureTextField, SearchField, TokenField, ComboBox, Form) and read
/// back string/selection state. Headless-safe object construction + setters.
#[test]
#[cfg(target_os = "macos")]
fn appkit_text_widgets_construct_and_configure() {
    appkit_run_program(
        "ak_text",
        r#"
import "appkit/runtime" as rt;
import "appkit/text" as text;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 200.0, height: 40.0 } };

    let tv = text::TextView::new(f);
    tv.set_string(#str_ptr("hello world\0"));
    tv.set_editable(1 as i8);
    tv.set_rich_text(0 as i8);
    if tv.string() == unsafe { 0 as *u8 } { return 1; }

    let secure = text::SecureTextField::new(f);
    secure.set_placeholder_string(#str_ptr("password\0"));
    secure.set_string_value(#str_ptr("pw\0"));
    if secure.string_value() == unsafe { 0 as *u8 } { return 2; }

    let search = text::SearchField::new(f);
    search.set_placeholder_string(#str_ptr("search\0"));
    search.set_string_value(#str_ptr("q\0"));

    let tokens = text::TokenField::new(f);
    tokens.set_string_value(#str_ptr("a,b\0"));
    if tokens.string_value() == unsafe { 0 as *u8 } { return 3; }

    let combo = text::ComboBox::new(f);
    combo.add_item(#str_ptr("one\0"));
    combo.add_item(#str_ptr("two\0"));
    combo.select_item_at_index(1 as i64);
    if combo.index_of_selected_item() != (1 as i64) { return 4; }

    let form = text::Form::new(f);
    let _entry: *u8 = form.add_entry(#str_ptr("Name\0"));
    form.set_interline_spacing(4.0);

    return 0;
}
"#,
    );
}

/// vendor/appkit `containers` coverage: construct + configure the layout
/// container views (SplitView, TabView + TabViewItem, VisualEffectView, GridView,
/// Browser, Matrix, ClipView, RulerView, Popover), including the cross-object
/// wiring (add an arranged subview, a tab item, a document view). Headless-safe.
#[test]
#[cfg(target_os = "macos")]
fn appkit_containers_construct_and_configure() {
    appkit_run_program(
        "ak_containers",
        r#"
import "appkit/runtime" as rt;
import "appkit/containers" as containers;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 200.0, height: 200.0 } };
    let v: *u8 = rt::alloc_init_with_frame(#str_ptr("NSView\0"), f);

    let split = containers::SplitView::new(f);
    split.set_vertical(1 as i8);
    split.set_divider_style(1 as i64);
    split.add_arranged_subview(v);

    let tab = containers::TabView::new(f);
    let item = containers::TabViewItem::new(#str_ptr("id1\0"));
    item.set_label(#str_ptr("Tab 1\0"));
    item.set_view(v);
    tab.add_tab_view_item(item.obj);
    tab.select_tab_view_item_at_index(0 as i64);

    let vfx = containers::VisualEffectView::new(f);
    vfx.set_material(0 as i64);
    vfx.set_blending_mode(0 as i64);
    vfx.set_state(1 as i64);

    let grid = containers::GridView::new(f);
    grid.set_row_spacing(4.0);
    grid.set_column_spacing(6.0);

    let browser = containers::Browser::new(f);
    browser.reload_column(0 as i64);

    let matrix = containers::Matrix::new(f);
    matrix.set_mode(0 as i64);

    let clip = containers::ClipView::new(f);
    clip.set_document_view(v);

    let ruler = containers::RulerView::new(f);
    ruler.set_orientation(0 as i64);

    let pop = containers::Popover::new();
    pop.set_behavior(1 as i64);

    return 0;
}
"#,
    );
}

/// vendor/appkit `toolbar` coverage: Toolbar, ToolbarItem, ToolbarItemGroup,
/// TouchBar/TouchBarItem, and the status-bar trio (StatusBar -> StatusItem ->
/// StatusBarButton). The system status bar + a status item are real (the item
/// flow is guarded on a non-null button). Headless-safe.
#[test]
#[cfg(target_os = "macos")]
fn appkit_toolbar_construct_and_configure() {
    appkit_run_program(
        "ak_toolbar",
        r#"
import "appkit/runtime" as rt;
import "appkit/toolbar" as toolbar;

fn main() -> i32 {
    let tb = toolbar::Toolbar::new(#str_ptr("main\0"));
    tb.set_display_mode(1 as i64);
    tb.set_allows_user_customization(1 as i8);

    let ti = toolbar::ToolbarItem::new(#str_ptr("item1\0"));
    ti.set_label(#str_ptr("Item\0"));
    ti.set_palette_label(#str_ptr("Item\0"));
    ti.set_tool_tip(#str_ptr("tip\0"));

    let tg = toolbar::ToolbarItemGroup::new(#str_ptr("group1\0"));

    let bar = toolbar::StatusBar::system();
    let item_obj: *u8 = bar.status_item_with_length(-1.0);
    if item_obj == unsafe { 0 as *u8 } { return 1; }
    let si = toolbar::StatusItem::from_obj(item_obj);
    si.set_length(24.0);
    let btn_obj: *u8 = si.button();
    if btn_obj != unsafe { 0 as *u8 } {
        let sbb = toolbar::StatusBarButton::from_obj(btn_obj);
        sbb.set_title(#str_ptr("S\0"));
    }

    let touch = toolbar::TouchBar::new();
    let touch_item = toolbar::TouchBarItem::new(#str_ptr("ti1\0"));

    return 0;
}
"#,
    );
}

/// vendor/appkit `panels` coverage: NSPanel + the shared file/print panels
/// (SavePanel, OpenPanel, PageLayout, PrintPanel) — construct + configure.
/// `run_modal()` / `make_key_and_order_front:` are intentionally NOT called:
/// they block on a modal dialog and would hang a headless run.
#[test]
#[cfg(target_os = "macos")]
fn appkit_panels_construct_and_configure() {
    appkit_run_program(
        "ak_panels",
        r#"
import "appkit/runtime" as rt;
import "appkit/panels" as panels;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 300.0, height: 200.0 } };

    let p = panels::Panel::new(f, 1 as u64, 2 as u64, 0 as i8);
    p.set_title(#str_ptr("Panel\0"));

    let save = panels::SavePanel::shared();
    save.set_title(#str_ptr("Save\0"));
    save.set_prompt(#str_ptr("Save\0"));
    save.set_message(#str_ptr("Choose a location\0"));
    save.set_name_field_string_value(#str_ptr("file.txt\0"));

    let open = panels::OpenPanel::shared();
    open.set_can_choose_files(1 as i8);
    open.set_can_choose_directories(0 as i8);
    open.set_allows_multiple_selection(1 as i8);

    let pl = panels::PageLayout::shared();
    let pp = panels::PrintPanel::shared();

    return 0;
}
"#,
    );
}

/// vendor/appkit `controllers` coverage: ViewController, WindowController,
/// TabViewController, SplitViewController, ArrayController, ObjectController —
/// construct + the headless-safe setters/getters (a view controller's view round
/// trips; the array/object controllers take content). `show_window:` is skipped
/// (it presents UI).
#[test]
#[cfg(target_os = "macos")]
fn appkit_controllers_construct_and_configure() {
    appkit_run_program(
        "ak_controllers",
        r#"
import "appkit/runtime" as rt;
import "appkit/controllers" as controllers;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 100.0, height: 100.0 } };
    let v: *u8 = rt::alloc_init_with_frame(#str_ptr("NSView\0"), f);

    let vc = controllers::ViewController::new();
    vc.set_view(v);
    if vc.view() == unsafe { 0 as *u8 } { return 1; }

    let wc = controllers::WindowController::new();
    let _w: *u8 = wc.window();

    let tvc = controllers::TabViewController::new();
    let svc = controllers::SplitViewController::new();

    let arr: *u8 = rt::msg_id(rt::get_class(#str_ptr("NSArray\0")), rt::sel(#str_ptr("array\0")));
    let ac = controllers::ArrayController::new();
    ac.set_content(arr);

    let obj: *u8 = rt::msg_id(rt::get_class(#str_ptr("NSObject\0")), rt::sel(#str_ptr("new\0")));
    let oc = controllers::ObjectController::new();
    oc.set_content(obj);

    return 0;
}
"#,
    );
}

/// vendor/appkit `menu` coverage: the owned Menu + MenuItem wrappers and their
/// Drop interplay — a menu retains items on `add_item`, an item retains its
/// submenu, and a separator factory item is added. Building + dropping the whole
/// tree must neither leak nor over-release.
#[test]
#[cfg(target_os = "macos")]
fn appkit_menu_build_tree() {
    appkit_run_program(
        "ak_menu",
        r#"
import "appkit/menu" as menu;

fn main() -> i32 {
    let m = menu::Menu::new(#str_ptr("App\0"));
    m.set_autoenables_items(0 as i8);

    let quit = menu::MenuItem::new(#str_ptr("Quit\0"), #str_ptr("terminate:\0"), #str_ptr("q\0"));
    quit.set_enabled(1 as i8);
    quit.set_key_equivalent_modifier_mask(1048576 as u64);
    m.add_item(quit.obj);

    let sep: *u8 = menu::MenuItem::separator();
    m.add_item(sep);

    let submenu = menu::Menu::new(#str_ptr("More\0"));
    let parent = menu::MenuItem::new(#str_ptr("More\0"), unsafe { 0 as *u8 }, #str_ptr("\0"));
    parent.set_submenu(submenu.obj);
    m.add_item(parent.obj);

    return 0;
}
"#,
    );
}

/// vendor/appkit `events` coverage: the pure NSEvent helpers — the
/// `has_modifier` bitmask predicate and the type/modifier constants. (The
/// `NSEvent*` field accessors need a live AppKit-dispatched event and are
/// exercised only in real handlers, not here.)
#[test]
#[cfg(target_os = "macos")]
fn appkit_event_modifier_helpers() {
    appkit_run_program(
        "ak_events",
        r#"
import "appkit/events" as events;

fn main() -> i32 {
    let combo: u64 = events::mod_command() | events::mod_shift();
    if events::has_modifier(combo, events::mod_command()) != (1 as i8) { return 1; }
    if events::has_modifier(combo, events::mod_shift()) != (1 as i8) { return 2; }
    if events::has_modifier(combo, events::mod_control()) != (0 as i8) { return 3; }
    if events::mod_command() != (1048576 as u64) { return 4; }
    if events::type_key_down() != (10 as i64) { return 5; }
    if events::type_left_mouse_down() != (1 as i64) { return 6; }
    return 0;
}
"#,
    );
}

/// vendor/appkit `convert` coverage: the C+/ObjC data bridge, verified by real
/// round-trips — str↔NSString (content equality), Vec[u8]↔NSData (byte values),
/// and NSArray→Vec[f64] (built from NSNumbers). This is the most checkable
/// module; the assertions confirm bytes/chars survive the boundary, not just
/// that the calls run.
#[test]
#[cfg(target_os = "macos")]
fn appkit_convert_roundtrips() {
    appkit_run_program(
        "ak_convert",
        r#"
import "appkit/runtime" as rt;
import "appkit/convert" as convert;
import "stdlib/vec" as vec;

fn main() -> i32 {
    // str -> NSString -> string, content preserved.
    let ns: *u8 = convert::cplus_str_to_nsstring("hello world");
    let back: string = convert::nsstring_to_cplus_string(ns);
    if back.as_str() != "hello world" { return 1; }

    // Vec[u8] -> NSData -> Vec[u8], bytes preserved.
    let mut v: vec::Vec[u8] = vec::Vec[u8]::new();
    v.push(10 as u8);
    v.push(20 as u8);
    v.push(30 as u8);
    let data: *u8 = convert::vec_u8_to_nsdata(v);
    let bytes: vec::Vec[u8] = convert::nsdata_to_vec_u8(data);
    if bytes.len() != (3 as usize) { return 2; }
    if vec::at_copy::[u8](bytes, 0 as usize) != (10 as u8) { return 3; }
    if vec::at_copy::[u8](bytes, 2 as usize) != (30 as u8) { return 4; }

    // NSArray of NSNumbers -> Vec[f64].
    let marr: *u8 = rt::msg_id(rt::get_class(#str_ptr("NSMutableArray\0")), rt::sel(#str_ptr("array\0")));
    let num_cls: *u8 = rt::get_class(#str_ptr("NSNumber\0"));
    let n1: *u8 = rt::msg_id_f64(num_cls, rt::sel(#str_ptr("numberWithDouble:\0")), 1.5);
    let n2: *u8 = rt::msg_id_f64(num_cls, rt::sel(#str_ptr("numberWithDouble:\0")), 2.5);
    rt::msg_void_id(marr, rt::sel(#str_ptr("addObject:\0")), n1);
    rt::msg_void_id(marr, rt::sel(#str_ptr("addObject:\0")), n2);
    if convert::nsarray_count(marr) != (2 as usize) { return 5; }
    let nums: vec::Vec[f64] = convert::nsarray_to_vec_f64(marr);
    if nums.len() != (2 as usize) { return 6; }
    if vec::at_copy::[f64](nums, 0 as usize) < (1.0) { return 7; }
    if vec::at_copy::[f64](nums, 1 as usize) < (2.0) { return 8; }

    return 0;
}
"#,
    );
}

/// vendor/appkit `graphics` coverage: Color (rgba + named factories), Font
/// (system/bold/label), Image (by_name), ImageView, and BezierPath's data-only
/// accessors (element_count, set_line_width). Font/Color factories are headless-
/// safe; a system image may be nil without an app bundle, so set_image is guarded.
#[test]
#[cfg(target_os = "macos")]
fn appkit_graphics_factories_and_views() {
    appkit_run_program(
        "ak_graphics",
        r#"
import "appkit/runtime" as rt;
import "appkit/graphics" as graphics;

fn main() -> i32 {
    let f: rt::Rect = rt::Rect { origin: rt::Point { x: 0.0, y: 0.0 }, size: rt::Size { width: 64.0, height: 64.0 } };

    if graphics::Color::red() == unsafe { 0 as *u8 } { return 1; }
    if graphics::Color::rgba(0.5, 0.25, 0.75, 1.0) == unsafe { 0 as *u8 } { return 2; }
    let _b = graphics::Color::black();
    let _w = graphics::Color::white();
    let _c = graphics::Color::clear();
    let _g = graphics::Color::gray();
    let _y = graphics::Color::yellow();
    let _gn = graphics::Color::green();
    let _bl = graphics::Color::blue();

    if graphics::Font::system_font_of_size(13.0) == unsafe { 0 as *u8 } { return 3; }
    let _bold = graphics::Font::bold_system_font_of_size(13.0);
    let _label = graphics::Font::label_font_of_size(11.0);

    let named: *u8 = graphics::Image::by_name(#str_ptr("NSApplicationIcon\0"));

    let iv = graphics::ImageView::new(f);
    iv.set_scaling(0 as i64);
    if named != unsafe { 0 as *u8 } { iv.set_image(named); }

    let path = graphics::BezierPath::new();
    path.move_to(0.0, 0.0);
    path.line_to(10.0, 10.0);
    path.set_line_width(2.0);
    if path.element_count() < (2 as i64) { return 4; }

    return 0;
}
"#,
    );
}

/// Regression for the struct-by-value `objc_msgSend` argument ABI: NSPoint
/// (2×f64) and NSRect (4×f64) are Homogeneous Floating-point Aggregates and must
/// be passed in FP registers (d0–d3) per AAPCS64. cpc previously coerced them to
/// integer class / passed NSRect indirectly, so the value never reached the
/// method (garbage geometry). This round-trips both through NSValue: the arg is
/// the HFA (the fixed path), the return reads it back. Pre-fix this returned
/// garbage / (0,0); post-fix the coordinates survive.
#[test]
#[cfg(target_os = "macos")]
fn appkit_struct_arg_abi_hfa_roundtrip() {
    appkit_run_program(
        "ak_hfa",
        r#"
import "appkit/runtime" as rt;

#[link_name = "objc_msgSend"]
extern fn value_with_point(cls: *u8, sel: *u8, p: rt::Point) -> *u8;
#[link_name = "objc_msgSend"]
extern fn value_with_rect(cls: *u8, sel: *u8, r: rt::Rect) -> *u8;
#[link_name = "objc_msgSend"]
extern fn rect_value(v: *u8, sel: *u8) -> rt::Rect;

fn main() -> i32 {
    let nsvalue: *u8 = rt::get_class(#str_ptr("NSValue\0"));

    // 2×f64 HFA (NSPoint) argument.
    let p: rt::Point = rt::Point { x: 12.0, y: 34.0 };
    let vp: *u8 = unsafe { value_with_point(nsvalue, rt::sel(#str_ptr("valueWithPoint:\0")), p) };
    let gp: rt::Point = rt::msg_point(vp, rt::sel(#str_ptr("pointValue\0")));
    if gp.x < 11.0 { return 1; }
    if gp.y < 33.0 { return 2; }

    // 4×f64 HFA (NSRect) argument — passed Indirect before the fix.
    let r: rt::Rect = rt::Rect { origin: rt::Point { x: 5.0, y: 6.0 }, size: rt::Size { width: 7.0, height: 8.0 } };
    let vr: *u8 = unsafe { value_with_rect(nsvalue, rt::sel(#str_ptr("valueWithRect:\0")), r) };
    let gr: rt::Rect = unsafe { rect_value(vr, rt::sel(#str_ptr("rectValue\0"))) };
    if gr.origin.x < 4.0 { return 3; }
    if gr.size.width < 6.0 { return 4; }
    if gr.size.height < 7.0 { return 5; }

    return 0;
}
"#,
    );
}

#[test]
#[cfg(target_os = "macos")]
fn appkit_vendor_package_smoke() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    // Write consumer Cplus.toml
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"smoke_app\"\n\n[[bin]]\nname = \"smoke_app\"\npath = \"src/main.cplus\"\n\n[dependencies]\nappkit = \"*\"\n",
    ).unwrap();

    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/appkit/src")).unwrap();

    // Read and copy our implemented appkit package into the tempdir project.
    let appkit_toml = std::fs::read_to_string("../vendor/appkit/Cplus.toml").unwrap();
    std::fs::write(dir.join("vendor/appkit/Cplus.toml"), appkit_toml).unwrap();
    for entry in std::fs::read_dir("../vendor/appkit/src").unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("cplus") {
            continue;
        }
        let dst = dir
            .join("vendor/appkit/src")
            .join(path.file_name().unwrap());
        std::fs::copy(path, dst).unwrap();
    }

    // Write consumer main.cplus
    std::fs::write(
        dir.join("src/main.cplus"),
        r#"
import "appkit/appkit" as appkit;

fn on_click(sender: *u8) {
    // Click action callback
}

fn main() -> i32 {
    let pool = appkit::AutoreleasePool::new();
    let app = appkit::Application::shared();
    
    let frame = appkit::Rect {
        origin: appkit::Point { x: 0.0, y: 0.0 },
        size: appkit::Size { width: 100.0, height: 100.0 }
    };
    
    let btn = appkit::Button::new(frame);
    btn.set_enabled(1 as i8);
    btn.set_on_click(on_click);
    
    let color = appkit::Color::rgba(1.0, 0.0, 0.0, 1.0);
    let font = appkit::Font::system_font_of_size(12.0);
    let alert = appkit::Alert::new();
    alert.set_message_text(#str_ptr("Smoke\0"));
    alert.add_button(#str_ptr("OK\0"));

    let secure = appkit::SecureTextField::new(frame);
    secure.set_placeholder_string(#str_ptr("Password\0"));
    let search = appkit::SearchField::new(frame);
    search.set_placeholder_string(#str_ptr("Search\0"));
    search.set_on_search(on_click);
    let tokens = appkit::TokenField::new(frame);
    tokens.set_string_value(#str_ptr("one,two\0"));
    let combo = appkit::ComboBox::new(frame);
    combo.add_item(#str_ptr("A\0"));
    let text_view = appkit::TextView::new(frame);
    text_view.set_string(#str_ptr("Body\0"));

    let stepper = appkit::Stepper::new(frame);
    stepper.set_increment(1.0);
    let sw = appkit::Switch::new(frame);
    sw.set_state(1 as i64);
    let segments = appkit::SegmentedControl::new(frame);
    segments.set_segment_count(2 as i64);
    segments.set_label_for_segment(#str_ptr("One\0"), 0 as i64);
    let date_picker = appkit::DatePicker::new(frame);
    date_picker.set_date_picker_style(0 as i64);
    let color_well = appkit::ColorWell::new(frame);
    color_well.set_color(color);
    let level = appkit::LevelIndicator::new(frame);
    level.set_max_value(10.0);
    let path = appkit::PathControl::new(frame);
    path.set_path_style(0 as i64);

    let split = appkit::SplitView::new(frame);
    split.set_vertical(1 as i8);
    let tab_view = appkit::TabView::new(frame);
    let tab_item = appkit::TabViewItem::new(#str_ptr("main\0"));
    tab_item.set_label(#str_ptr("Main\0"));
    tab_view.add_tab_view_item(tab_item.obj);
    let visual = appkit::VisualEffectView::new(frame);
    visual.set_material(0 as i64);
    let grid = appkit::GridView::new(frame);
    grid.set_row_spacing(8.0);
    let browser = appkit::Browser::new(frame);
    browser.reload_column(0 as i64);
    let matrix = appkit::Matrix::new(frame);
    matrix.set_mode(0 as i64);
    let clip = appkit::ClipView::new(frame);
    clip.set_document_view(text_view.obj);
    let ruler = appkit::RulerView::new(frame);
    ruler.set_orientation(0 as i64);
    let popover = appkit::Popover::new();
    popover.set_behavior(1 as i64);

    let table = appkit::TableView::new(frame);
    let col = appkit::TableColumn::new(#str_ptr("name\0"));
    col.set_title(#str_ptr("Name\0"));
    table.add_table_column(col.obj);
    table.reload_data();
    let outline = appkit::OutlineView::new(frame);
    outline.add_table_column(col.obj);
    let cell = appkit::TableCellView::new(frame);
    cell.set_text_field(secure.obj);
    let row = appkit::TableRowView::new(frame);
    let collection = appkit::CollectionView::new(frame);
    let flow = appkit::CollectionViewFlowLayout::new();
    flow.set_item_size(appkit::Size { width: 44.0, height: 44.0 });
    collection.set_collection_view_layout(flow.obj);
    let grid_layout = appkit::CollectionViewGridLayout::new();
    grid_layout.set_minimum_item_size(appkit::Size { width: 20.0, height: 20.0 });
    let rule = appkit::RuleEditor::new(frame);
    rule.reload_criteria();
    let pred = appkit::PredicateEditor::new(frame);
    pred.reload_criteria();

    let toolbar = appkit::Toolbar::new(#str_ptr("main-toolbar\0"));
    toolbar.set_display_mode(1 as i64);
    let toolbar_item = appkit::ToolbarItem::new(#str_ptr("item\0"));
    toolbar_item.set_label(#str_ptr("Item\0"));
    let status_bar = appkit::StatusBar::system();
    let status_item_raw = status_bar.status_item_with_length(24.0);
    let status_item = appkit::StatusItem::from_obj(status_item_raw);
    let status_button = appkit::StatusBarButton::from_obj(status_item.button());
    status_button.set_title(#str_ptr("S\0"));
    let touch_bar = appkit::TouchBar::new();
    let touch_item = appkit::TouchBarItem::new(#str_ptr("touch\0"));

    let vc = appkit::ViewController::new();
    vc.set_view(text_view.obj);
    let wc = appkit::WindowController::new();
    let tabs = appkit::TabViewController::new();
    // NSTabViewController insists on (a) a fresh NSTabViewItem (not
    // one already attached to another tab parent) and (b) the item
    // having a non-nil viewController. The original smoke had neither —
    // it reused tab_item.obj from `tab_view` above. Fix both.
    let tab_item2 = appkit::TabViewItem::new(#str_ptr("controllers\0"));
    tab_item2.set_label(#str_ptr("Controllers\0"));
    let tab_vc = appkit::ViewController::new();
    tab_vc.set_view(visual.obj);
    tab_item2.set_view_controller(tab_vc.obj);
    tabs.add_tab_view_item(tab_item2.obj);
    let array_controller = appkit::ArrayController::new();
    let object_controller = appkit::ObjectController::new();
    
    pool.drain();
    return 42;
}
"#,
    )
    .unwrap();

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "cpc build for appkit smoke failed: {status}"
    );

    let bin = dir.join("target/debug/smoke_app");
    assert!(bin.is_file(), "expected binary at {}", bin.display());

    let run = Command::new(bin).status().expect("run smoke_app");
    // 42 is the sentinel "all widget constructions + method calls
    // completed without an NSException" set at the end of the smoke
    // source. We don't run the event loop, so 0 is unreachable —
    // 42 is the success exit.
    assert_eq!(
        run.code(), Some(42),
        "smoke_app expected exit 42 (all calls completed), got: {run}"
    );
}

#[test]
fn simd_vendor_package_smoke() {
    // v0.0.8 Phase 2: end-to-end check that `vendor/simd` builds, links,
    // and produces correct results across Vec3 / Vec4 / Mat4x4.
    //
    // Math checks (all asserted via integer-cast comparison so we
    // avoid flaky float equality):
    //   - Vec3.dot((1,2,3), (4,5,6))     == 32
    //   - Vec3.cross((1,2,3), (4,5,6))   == (-3, 6, -3)
    //   - Vec3.lerp(0, (10,20,30), 0.5)  == (5, 10, 15)
    //   - Vec4.dot((1,2,3,4), (5,6,7,8)) == 70
    //   - identity * Vec4(1,2,3,4)       == Vec4(1,2,3,4)
    //   - (2 * identity) * Vec4(1,2,3,4) == Vec4(2,4,6,8)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"simd_smoke\"\n\n[[bin]]\nname = \"simd_smoke\"\npath = \"src/main.cplus\"\n\n[dependencies]\nsimd = \"*\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/simd/src")).unwrap();

    // Copy the in-tree simd package into the tempdir project.
    let simd_toml = std::fs::read_to_string("../vendor/simd/Cplus.toml").unwrap();
    std::fs::write(dir.join("vendor/simd/Cplus.toml"), simd_toml).unwrap();
    for entry in std::fs::read_dir("../vendor/simd/src").unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("cplus") {
            continue;
        }
        let dst = dir
            .join("vendor/simd/src")
            .join(path.file_name().unwrap());
        std::fs::copy(path, dst).unwrap();
    }

    std::fs::write(
        dir.join("src/main.cplus"),
        r#"import "simd/vec3" as vec3;
import "simd/vec4" as vec4;
import "simd/mat4x4" as mat;

fn main() -> i32 {
    let a3: vec3::Vec3 = vec3::Vec3::new(1.0f32, 2.0f32, 3.0f32);
    let b3: vec3::Vec3 = vec3::Vec3::new(4.0f32, 5.0f32, 6.0f32);
    if (a3.dot(b3) as i32) != 32 { return 1; }
    let cr: vec3::Vec3 = a3.cross(b3);
    if (cr.x() as i32) != (0 - 3) { return 2; }
    if (cr.y() as i32) != 6 { return 3; }
    if (cr.z() as i32) != (0 - 3) { return 4; }
    let lerped: vec3::Vec3 = vec3::Vec3::zero().lerp(
        vec3::Vec3::new(10.0f32, 20.0f32, 30.0f32), 0.5f32);
    if (lerped.x() as i32) != 5 { return 10; }
    if (lerped.y() as i32) != 10 { return 11; }
    if (lerped.z() as i32) != 15 { return 12; }

    let a4: vec4::Vec4 = vec4::Vec4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);
    let b4: vec4::Vec4 = vec4::Vec4::new(5.0f32, 6.0f32, 7.0f32, 8.0f32);
    if (a4.dot(b4) as i32) != 70 { return 20; }

    let id: mat::Mat4x4 = mat::Mat4x4::identity();
    let mv: vec4::Vec4 = id.mul_vec(a4);
    if (mv.x() as i32) != 1 { return 30; }
    if (mv.y() as i32) != 2 { return 31; }
    if (mv.z() as i32) != 3 { return 32; }
    if (mv.w() as i32) != 4 { return 33; }
    let m2: mat::Mat4x4 = id.scale(2.0f32);
    let mv2: vec4::Vec4 = m2.mul_vec(a4);
    if (mv2.x() as i32) != 2 { return 40; }
    if (mv2.w() as i32) != 8 { return 41; }

    return 0;
}
"#,
    )
    .unwrap();

    let status = Command::new(cpc)
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "cpc build for simd smoke failed: {status}"
    );

    let bin = dir.join("target/release/simd_smoke");
    assert!(bin.is_file(), "expected binary at {}", bin.display());

    let run = Command::new(bin).status().expect("run simd_smoke");
    assert_eq!(
        run.code(),
        Some(0),
        "simd_smoke expected exit 0 (all asserts passed), got: {run}"
    );
}

#[test]
fn env_macro_round_trip_runs() {
    // v0.0.8 Phase 4: `env!("NAME")` reads the env var at compile time
    // and substitutes a `str` value (fat pointer to a `.rodata` global).
    // Verify the end-to-end pipeline: parser → sema → codegen → linked
    // binary correctly carries the value the compiler saw at build.
    std::env::set_var("CPC_E2E_GREETING", "hello-from-env");
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("env_test.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let g: str = #env(\"CPC_E2E_GREETING\");\n\
             // Exit code = length of the env-var value (14 chars for\n\
             // `hello-from-env`). Confirms the str's len field was wired up.\n\
             return #str_len(g) as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("env_test");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .env("CPC_E2E_GREETING", "hello-from-env")
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for env! round-trip");
    let run = Command::new(&bin).status().expect("run env_test");
    assert_eq!(
        run.code(),
        Some(14),
        "expected exit 14 (length of `hello-from-env`), got: {run}"
    );
}

#[test]
fn env_macro_missing_var_errors_e0876() {
    // Negative path: var not set when cpc runs → E0876, build fails.
    std::env::remove_var("CPC_E2E_DEFINITELY_MISSING_VAR_PHASE4");
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("env_missing.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let _x: str = #env(\"CPC_E2E_DEFINITELY_MISSING_VAR_PHASE4\");\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(dir.join("env_missing"))
        .env_remove("CPC_E2E_DEFINITELY_MISSING_VAR_PHASE4")
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected cpc build to fail on missing env var, got success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0876"),
        "expected E0876 in stderr, got:\n{stderr}"
    );
}

// ---- v0.0.9 Phase 3: mixed-if-arm panic regression ----

#[test]
fn mixed_if_arm_field_tail_compiles_and_runs() {
    // Field tail expression in one arm — pre-Phase-3 this panicked
    // "let init produces a value" because `expr_value_ty_with_bindings`
    // didn't handle Field. Now it computes the field's type from the
    // receiver's struct definition.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p.cplus");
    std::fs::write(
        &src,
        "struct V3 { x: f32, y: f32, z: f32 }\n\
         fn main() -> i32 {\n\
             let cond: bool = true;\n\
             let a: V3 = V3 { x: 3.0f32, y: 4.0f32, z: 5.0f32 };\n\
             let b: V3 = V3 { x: 9.0f32, y: 8.0f32, z: 7.0f32 };\n\
             let x: f32 = if cond { a.x } else { b.x };\n\
             #println(x as i32);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("p");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "cpc failed; stderr:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "exited {:?}", run.status);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n");
}

// ---- v0.0.14: if-arm building a payload-carrying enum ctor ----

#[test]
fn if_arm_payload_enum_ctor_value_not_discarded() {
    // An `if`-expression whose branches build a payload-carrying enum
    // constructor (`Out::Hi(7)`, lowered as `Call { callee: Path }`),
    // sitting in a value position (a `match` arm). Pre-fix,
    // `expr_value_ty_with_bindings` didn't recognize the `Call{Path}`
    // enum-ctor shape, so `gen_if` allocated no result slot and the
    // branch value was silently discarded — the consuming `match` then
    // read an uninitialized slot. This was the v0.0.14 json `parse()`
    // miscompile (parsed values read back as Null / spurious Err).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ifarm.cplus");
    std::fs::write(
        &src,
        "\
enum Tag { A, B }
enum Out { Hi(i32), Lo(i32) }

fn pick(t: Tag, flag: bool) -> Out {
    let r: Out = match t {
        Tag::A => { if flag { Out::Hi(7) } else { Out::Lo(8) } }
        Tag::B => Out::Lo(30),
    };
    return r;
}

fn main() -> i32 {
    let o: Out = pick(Tag::A, true);
    let code: i32 = match o {
        Out::Hi(x) => x,
        Out::Lo(_) => 99,
    };
    if code != 7 { return 100 +% code; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("ifarm");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for if-arm enum-ctor reproducer");
    let run = Command::new(&bin).status().expect("run ifarm");
    assert_eq!(
        run.code(),
        Some(0),
        "if-arm enum-ctor value was discarded; expected exit 0, got {:?}",
        run.code()
    );
}

/// v0.0.15: retiring the if-result predictor. An `if`-expression whose arms
/// are *method calls* returning a struct (`p.shift()` / `p.keep()`, lowered as
/// `Call { callee: Field { .. } }`) in value position. The old
/// `expr_value_ty_with_bindings` predictor only typed `Call` callees shaped as
/// `Ident` or `Path`; a `Field` callee fell through to `None`, so `gen_if`
/// allocated no result slot and the branch value was silently discarded —
/// exactly the drift-prone gap the refactor closes. `gen_if` now sizes the
/// slot from the `Ty` `gen_expr` actually returns, so any value-producing
/// arm shape works without the predictor having to enumerate it.
#[test]
fn if_arm_method_call_struct_value_not_discarded() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ifmeth.cplus");
    std::fs::write(
        &src,
        "\
struct P { x: i32, y: i32 }

impl P {
    fn shift(self) -> P { return P { x: self.x +% 1, y: self.y +% 1 }; }
    fn keep(self) -> P { return P { x: self.x, y: self.y }; }
}

fn choose(p: P, flag: bool) -> P {
    let r: P = if flag { p.shift() } else { p.keep() };
    return r;
}

fn main() -> i32 {
    let base: P = P { x: 10, y: 20 };
    let out: P = choose(base, true);
    if out.x != 11 { return 1; }
    if out.y != 21 { return 2; }
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("ifmeth");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for if-arm method-call reproducer");
    let run = Command::new(&bin).status().expect("run ifmeth");
    assert_eq!(
        run.code(),
        Some(0),
        "if-arm method-call struct value was discarded; expected exit 0, got {:?}",
        run.code()
    );
}

/// v0.0.15: module-scope `#asm("...");` → LLVM `module asm "..."`. End-to-end:
/// the directive must survive through codegen, assemble via the integrated
/// assembler, link, and the program still run. A bare `.text` section switch is
/// the most portable benign directive (valid on every target's assembler) and
/// has no runtime effect, so `main` returning 0 proves the whole pipeline.
#[test]
fn module_asm_item_compiles_links_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("modasm.cplus");
    std::fs::write(
        &src,
        "#asm(\".text\");\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();

    // The emitted IR carries the module-level directive verbatim. (`--emit-ll`
    // compiles the given FILE; `--emit-ir` is the frozen Phase-0 demo that
    // ignores its input.)
    let ir = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc --emit-ll");
    assert!(ir.status.success(), "cpc --emit-ll failed: {:?}", ir);
    let ir_text = String::from_utf8_lossy(&ir.stdout);
    assert!(
        ir_text.contains("module asm \".text\""),
        "expected `module asm` directive in IR, got:\n{ir_text}"
    );

    // And it assembles, links, and runs.
    let bin = dir.join("modasm");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for module-asm program");
    let run = Command::new(&bin).status().expect("run modasm");
    assert_eq!(run.code(), Some(0), "module-asm program exit code");
}

/// v0.0.14: container element drop — verify (by count, not just crash-free)
/// that dropping a `Vec[T]` runs each element's `drop` exactly once via the
/// `__cplus_drop_in_place::[T]` loop, including when the Vec is itself an
/// owning field auto-dropped through a wrapper struct.
#[test]
fn vec_element_drop_runs_per_element_by_count() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vd\"\n\n[[bin]]\nname = \"vd\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/Cplus.toml"), "[package]\nname = \"stdlib\"\n").unwrap();
    for name in &["vec", "iterator", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         static mut DROPS: i32 = 0;\n\
         struct Cell { tag: i32 }\n\
         impl Cell { fn drop(mut self) { unsafe { DROPS = DROPS +% 1; }; } }\n\
         struct Wrap { items: vec::Vec[Cell], name: i32 }\n\
         fn direct() {\n\
             let mut v: vec::Vec[Cell] = vec::new::[Cell]();\n\
             v.push(Cell { tag: 1 });\n\
             v.push(Cell { tag: 2 });\n\
             v.push(Cell { tag: 3 });\n\
             return;\n\
         }\n\
         fn nested() {\n\
             let mut v: vec::Vec[Cell] = vec::new::[Cell]();\n\
             v.push(Cell { tag: 1 });\n\
             v.push(Cell { tag: 2 });\n\
             let w: Wrap = Wrap { items: v, name: 9 };\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             direct();\n\
             nested();\n\
             return unsafe { DROPS };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("vd");
    let st = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for vec element-drop count test");
    let run = Command::new(dir.join("target/debug/vd")).status().expect("run vd");
    // 3 (direct) + 2 (nested, auto-dropped through Wrap) = 5 element drops.
    assert_eq!(run.code(), Some(5), "expected 5 element drops, got {:?}", run.code());
}

/// v0.0.14: consumed-enum payload leak fix. Matching an owned enum consumes
/// it; an owning payload binding is now drop-registered, so a binding that is
/// NOT moved out is dropped at arm exit (closing the leak), while every
/// move-out shape (into a call, a re-wrap ctor, or a bare-`Ident` arm value)
/// disarms the drop (no double-free). Verified by an exact drop count.
#[test]
fn consumed_enum_payload_drops_once_per_arm() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ce.cplus");
    std::fs::write(
        &src,
        "\
static mut DROPS: i32 = 0;
struct Res { tag: i32 }
impl Res { fn drop(mut self) { unsafe { DROPS = DROPS +% 1; }; } }
enum Box1 { Some(Res), None }
enum Wrap { W(Res), X }
fn consume(r: Res) -> i32 { return r.tag; }
fn s_not_moved() {
    let b: Box1 = Box1::Some(Res { tag: 1 });
    let _c: i32 = match b { Box1::Some(r) => 1, Box1::None => 0 };
    return;
}
fn s_consumed() {
    let b: Box1 = Box1::Some(Res { tag: 2 });
    let _c: i32 = match b { Box1::Some(r) => consume(r), Box1::None => 0 };
    return;
}
fn s_rewrap() {
    let b: Box1 = Box1::Some(Res { tag: 3 });
    let w: Wrap = match b { Box1::Some(r) => Wrap::W(r), Box1::None => Wrap::X };
    return;
}
fn s_tail() {
    let b: Box1 = Box1::Some(Res { tag: 4 });
    let out: Res = match b { Box1::Some(r) => r, Box1::None => Res { tag: 0 } };
    return;
}
fn main() -> i32 {
    s_not_moved();
    s_consumed();
    s_rewrap();
    s_tail();
    return unsafe { DROPS };
}
",
    )
    .unwrap();
    let bin = dir.join("ce");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for consumed-enum payload test");
    let run = Command::new(&bin).status().expect("run ce");
    // Each scenario drops its payload exactly once: leak fixed (s_not_moved)
    // and no double-free on any move-out path. 4 total.
    assert_eq!(run.code(), Some(4), "expected 4 drops, got {:?}", run.code());
}

/// v0.0.15 double-free fix (vendor/json segfault): a heap-owning ENUM moved by
/// bare-ident into a method-call argument (`elems.push(v)`, where `v` is a
/// `match`-arm payload owning a nested `Vec`). Pre-fix, `effective_move` only
/// covered `Ty::Struct` and the struct-method `MethodInfo` used the raw
/// `move_` flag, so the enum was borrow-copied without `mark_moved`: the
/// caller's scope-exit drop freed heap the callee had already stored into the
/// vector — a use-after-free / double-free on the next read. An exact drop
/// count catches the extra teardown (a buggy build double-runs the leaves'
/// `drop` or crashes outright).
#[test]
fn enum_move_into_method_arg_no_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"df\"\n\n[[bin]]\nname = \"df\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/Cplus.toml"), "[package]\nname = \"stdlib\"\n").unwrap();
    for name in &["vec", "iterator", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/option\" as option;\n\
         static mut DROPS: i32 = 0;\n\
         static mut SUM: i32 = 0;\n\
         struct Leaf { tag: i32 }\n\
         impl Leaf { fn drop(mut self) { unsafe { DROPS = DROPS +% 1; }; } }\n\
         enum Node { One(Leaf), Many(vec::Vec[Node]) }\n\
         enum Parse { Ok(Node, i32), Fail(i32) }\n\
         fn make_inner() -> Parse {\n\
             let mut kids: vec::Vec[Node] = vec::new::[Node]();\n\
             kids.push(Node::One(Leaf { tag: 1 }));\n\
             kids.push(Node::One(Leaf { tag: 2 }));\n\
             return Parse::Ok(Node::Many(kids), 0);\n\
         }\n\
         fn build() -> Node {\n\
             let mut elems: vec::Vec[Node] = vec::new::[Node]();\n\
             let r: Parse = make_inner();\n\
             match r {\n\
                 Parse::Ok(v, rp) => { let _p: i32 = rp; elems.push(v); }\n\
                 Parse::Fail(rp) => { return Node::One(Leaf { tag: rp }); }\n\
             }\n\
             return Node::Many(elems);\n\
         }\n\
         fn count(borrow n: Node) -> i32 {\n\
             return match n {\n\
                 Node::One(l) => l.tag,\n\
                 Node::Many(kids) => {\n\
                     let mut total: i32 = 0;\n\
                     let mut i: usize = 0 as usize;\n\
                     while i < kids.len() { match kids.at(i) { option::Option[*Node]::Some(p) => { total = total +% count(unsafe { *p }); } option::Option[*Node]::None => {} } i = i +% (1 as usize); }\n\
                     total\n\
                 }\n\
             };\n\
         }\n\
         fn run_once() {\n\
             let n: Node = build();\n\
             unsafe { SUM = SUM +% count(n); }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             let mut iter: i32 = 0;\n\
             while iter < 8 { run_once(); iter = iter +% 1; }\n\
             if unsafe { SUM } != 24 { return 100; }\n\
             return unsafe { DROPS };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc).current_dir(&dir).arg("build").status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for enum-move double-free test");
    let run = Command::new(dir.join("target/debug/df")).status().expect("run df");
    // 2 leaves per iter × 8 iters = 16 drops, each exactly once. A double-free
    // (the bug) crashes or yields a different count.
    assert_eq!(run.code(), Some(16), "expected 16 leaf drops (no double-free), got {:?}", run.code());
}

/// v0.0.15 double-free fix (companion): a heap-owning enum payload moved out of
/// a `match` arm via an `if`/`else` branch *tail* (a bare `Ident`), the
/// vendor/json `parse` shape `match r { Ok(v) => if c { … } else { v } }`.
/// `gen_block_into_slot` (the if-branch lowering) did not disarm the bare-ident
/// tail move the way `gen_block_expr` does, so the moved-out value was
/// double-freed. The runtime drop-flag store lands inside the branch block, so
/// the binding still drops correctly on the branch that doesn't move it.
#[test]
fn enum_conditional_branch_tail_move_no_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"cb\"\n\n[[bin]]\nname = \"cb\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/Cplus.toml"), "[package]\nname = \"stdlib\"\n").unwrap();
    for name in &["vec", "iterator", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/option\" as option;\n\
         static mut DROPS: i32 = 0;\n\
         static mut SUM: i32 = 0;\n\
         struct Leaf { tag: i32 }\n\
         impl Leaf { fn drop(mut self) { unsafe { DROPS = DROPS +% 1; }; } }\n\
         enum Node { One(Leaf), Many(vec::Vec[Node]) }\n\
         enum Parse { Ok(Node), Fail }\n\
         fn make() -> Parse {\n\
             let mut kids: vec::Vec[Node] = vec::new::[Node]();\n\
             kids.push(Node::One(Leaf { tag: 1 }));\n\
             kids.push(Node::One(Leaf { tag: 2 }));\n\
             return Parse::Ok(Node::Many(kids));\n\
         }\n\
         fn unwrap_or(flag: bool) -> Node {\n\
             let r: Parse = make();\n\
             return match r {\n\
                 Parse::Ok(v) => { if flag { Node::One(Leaf { tag: 9 }) } else { v } }\n\
                 Parse::Fail => Node::One(Leaf { tag: 0 }),\n\
             };\n\
         }\n\
         fn count(borrow n: Node) -> i32 {\n\
             return match n {\n\
                 Node::One(l) => l.tag,\n\
                 Node::Many(kids) => {\n\
                     let mut total: i32 = 0;\n\
                     let mut i: usize = 0 as usize;\n\
                     while i < kids.len() { match kids.at(i) { option::Option[*Node]::Some(p) => { total = total +% count(unsafe { *p }); } option::Option[*Node]::None => {} } i = i +% (1 as usize); }\n\
                     total\n\
                 }\n\
             };\n\
         }\n\
         fn run_once() {\n\
             let n: Node = unwrap_or(false);\n\
             unsafe { SUM = SUM +% count(n); }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             let mut iter: i32 = 0;\n\
             while iter < 8 { run_once(); iter = iter +% 1; }\n\
             if unsafe { SUM } != 24 { return 100; }\n\
             return unsafe { DROPS };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc).current_dir(&dir).arg("build").status().expect("invoke cpc");
    assert!(st.success(), "cpc build failed for conditional-branch-tail move test");
    let run = Command::new(dir.join("target/debug/cb")).status().expect("run cb");
    assert_eq!(run.code(), Some(16), "expected 16 leaf drops (no double-free), got {:?}", run.code());
}

// ---- v0.0.14: broad raw-ptr !Send rule + `unsafe impl Send/Sync` ----

#[test]
fn unsafe_impl_send_compiles_and_runs_end_to_end() {
    // A raw-ptr-hiding struct is !Send by the structural rule; `unsafe impl
    // Send for Handle {}` re-enables it. Verifies the override flows through
    // parser + sema + codegen and runs (the impl is sema-only — no codegen).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("snd.cplus");
    std::fs::write(
        &src,
        "\
struct Handle { opaque p: *u8 }
unsafe impl Send for Handle {}
fn ship[T: Send](v: T) -> T { return v; }
fn main() -> i32 {
    let h: Handle = Handle { p: unsafe { 7 as *u8 } };
    let q: Handle = ship::[Handle](h);
    return unsafe { q.p as usize as i32 };
}
",
    )
    .unwrap();
    let bin = dir.join("snd");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for unsafe impl Send program");
    let run = Command::new(&bin).status().expect("run snd");
    assert_eq!(run.code(), Some(7), "expected exit 7, got {:?}", run.code());
}

#[test]
fn raw_ptr_struct_without_override_rejected_at_compile_time() {
    // The same program without the `unsafe impl Send` must fail to compile
    // with E0502 (Handle does not satisfy the `Send` bound).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nosend.cplus");
    std::fs::write(
        &src,
        "\
struct Handle { opaque p: *u8 }
fn ship[T: Send](v: T) -> T { return v; }
fn main() -> i32 {
    let h: Handle = Handle { p: unsafe { 0 as *u8 } };
    let _q: Handle = ship::[Handle](h);
    return 0;
}
",
    )
    .unwrap();
    let bin = dir.join("nosend");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0502"),
        "expected E0502 for !Send raw-ptr struct; stderr:\n{stderr}"
    );
}

#[test]
fn no_alloc_drop_glue_rejected_at_compile_time() {
    // A `#[no_alloc]` function with a `string` local: the scope-exit drop
    // frees the buffer (deallocation), so it must fail to compile (E0901)
    // even though no `malloc`/`free` call appears in the body.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("na.cplus");
    std::fs::write(
        &src,
        "\
#[no_alloc]
fn f(s: str) -> i32 {
    let owned: string = s.to_string();
    return 0;
}
fn main() -> i32 { return 0; }
",
    )
    .unwrap();
    let bin = dir.join("na");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0901"),
        "expected E0901 for no_alloc drop glue; stderr:\n{stderr}"
    );
}

// ---- v0.0.9 Phase 2: character literals 'a' ----

#[test]
fn char_literal_basic_runs() {
    let out = compile_and_run("char_literal.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "485\n1\n");
}

#[test]
fn char_literal_rejects_multi_byte_source() {
    // Negative: `'ab'` is a parse-time reject (the lexer surfaces it
    // as UnexpectedChar('b') at the closing-quote check).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let x: u8 = 'ab'; return x as i32; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected cpc to reject 'ab' as a char literal"
    );
}

// ---- v0.0.9 Phase 9 (cpc-gaps G-002 lock-down): generic HashMap[K, V] ----

#[test]
fn hash_map_combos_project_runs() {
    // The `hash_map_combos` project exercises every (K, V) combination
    // the llama port needs: str→i32, str→u64, i32→i32, u64→u32,
    // i64→bool, plus a 100-entry grow workload. Built end-to-end via
    // `cpc build` against the in-tree stdlib.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let proj_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs/examples/projects/hash_map_combos");
    let manifest = std::fs::read_to_string(proj_root.join("Cplus.toml")).unwrap();
    std::fs::write(dir.join("Cplus.toml"), manifest).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let main_src = std::fs::read_to_string(proj_root.join("src/main.cplus")).unwrap();
    std::fs::write(dir.join("src/main.cplus"), main_src).unwrap();
    // The in-tree project uses a symlinked vendor/stdlib; for the
    // tempdir copy we point to the same target through the project's
    // absolute path. cpc's resolver canonicalizes, so an absolute
    // symlink works the same as a relative one.
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib_target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor/stdlib");
    symlink_dir(&stdlib_target, &dir.join("vendor/stdlib"));

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");

    let bin = dir.join("target/debug/hash_map_combos");
    assert!(bin.is_file(), "expected binary at {}", bin.display());
    let out = Command::new(&bin).output().expect("run binary");
    assert!(out.status.success(), "binary exited non-zero: {}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hash_map combos: 6/6 ok\n");
}

// ---- v0.0.9 Phase 8 (cpc-gaps G-001): [link] extra-objects in Cplus.toml ----

#[test]
fn link_extra_objects_e2e_runs() {
    // End-to-end: hand-write a `helper.c`, compile it to `helper.o`
    // with clang, declare it in `[link] extra-objects`, and have the
    // C+ binary call into it via `extern fn`. Pre-G-001 the workflow
    // required a wrapper script that ran `clang` after `cpc build`;
    // now `cpc build` does the link in one step.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    // 1. Write the C helper.
    let c_src = dir.join("helper.c");
    std::fs::write(
        &c_src,
        "#include <stddef.h>\n\
         size_t cplus_ptr_addr(const void *p) { return (size_t)p; }\n\
         int the_answer(void) { return 42; }\n",
    )
    .unwrap();
    // 2. Compile it to a .o.
    let obj = dir.join("helper.o");
    let cc_status = Command::new("clang")
        .arg("-c")
        .arg(&c_src)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("invoke clang");
    assert!(cc_status.success(), "clang -c failed");
    // 3. Lay out a minimal C+ project that links against helper.o.
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(
        src_dir.join("main.cplus"),
        "extern fn the_answer() -> i32;\n\
         fn main() -> i32 {\n\
             #println(unsafe { the_answer() });\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\n\
         name = \"extra-objects-test\"\n\
         \n\
         [[bin]]\n\
         name = \"extra-objects-test\"\n\
         path = \"src/main.cplus\"\n\
         \n\
         [link]\n\
         extra-objects = [\"helper.o\"]\n",
    )
    .unwrap();
    // 4. cpc build.
    let build = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .output()
        .expect("invoke cpc");
    assert!(
        build.status.success(),
        "cpc build failed; stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // 5. Run the produced binary.
    let bin = dir.join("target/debug/extra-objects-test");
    let run = Command::new(&bin).output().expect("run binary");
    assert!(run.status.success(), "exited {:?}", run.status);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn link_extra_objects_missing_file_rejected_e0864() {
    // Negative: the manifest declares an extra-object that doesn't
    // exist on disk. cpc build must fail with E0864 before invoking
    // clang (so the user gets a clean "file not found" diagnostic
    // instead of a linker error).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(
        src_dir.join("main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\n\
         name = \"missing-obj\"\n\
         \n\
         [[bin]]\n\
         name = \"missing-obj\"\n\
         path = \"src/main.cplus\"\n\
         \n\
         [link]\n\
         extra-objects = [\"does-not-exist.o\"]\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected cpc build to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0864"),
        "expected E0864 in stderr, got:\n{stderr}"
    );
}

// ---- v0.0.9 Phase 7 (cpc-gaps G-011): single-file mode follows local imports ----

#[test]
fn single_file_local_import_compiles_and_runs() {
    // Two-file "project" driven through the single-file path (`cpc FILE
    // -o BIN`, no Cplus.toml). The entry imports a sibling file via
    // `./` and calls a function declared there. Pre-G-011 this failed
    // because the single-file pipeline ignored `import` statements.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("helper.cplus"),
        "pub fn answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    let entry = dir.join("main.cplus");
    std::fs::write(
        &entry,
        "import \"./helper\" as h;\n\
         fn main() -> i32 {\n\
             #println(h::answer());\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "cpc failed; stderr:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run");
    assert!(run.status.success(), "binary exited {:?}", run.status);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn single_file_emit_obj_local_import_compiles() {
    // The same two-file project, but via `cpc --emit-obj` (the original
    // motivating shape from cpc-gaps G-011). Produces a `.o` that
    // contains both files' merged IR. We don't link it back here —
    // verifying that the object file is produced is the test.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("util.cplus"),
        "pub fn double(x: i32) -> i32 { return x +% x; }\n",
    )
    .unwrap();
    let entry = dir.join("entry.cplus");
    std::fs::write(
        &entry,
        "import \"./util\" as u;\n\
         pub fn main_shim() -> i32 { return u::double(21); }\n",
    )
    .unwrap();
    let obj = dir.join("entry.o");
    let out = Command::new(cpc)
        .arg("--emit-obj")
        .arg(&entry)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "cpc --emit-obj failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(obj.exists(), "expected .o to exist at {}", obj.display());
    let metadata = std::fs::metadata(&obj).expect("stat obj");
    assert!(metadata.len() > 0, "expected non-empty .o");
}

#[test]
fn single_file_bare_import_rejected() {
    // `import "stdlib/io"` in single-file mode (no Cplus.toml, no
    // declared dependencies) must fail with E0853 — the user needs
    // either a project setup or a `./`-prefixed path.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let entry = dir.join("bad.cplus");
    std::fs::write(
        &entry,
        "import \"stdlib/io\" as io;\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected cpc to reject bare import in single-file mode"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // E0852 fires for a 2+-segment bare import (`stdlib/io`) — the
    // resolver classifies it as a vendor import and reports that
    // `stdlib` isn't a declared dependency. E0853 would fire for a
    // 1-segment bare import (`foo`); both are acceptable rejects
    // from the user's perspective.
    assert!(
        stderr.contains("E0852") || stderr.contains("E0853"),
        "expected E0852 or E0853 in stderr, got:\n{stderr}"
    );
}

// ---- v0.0.9 Phase 6 (cpc-gaps G-016): raw-pointer → integer cast ----

#[test]
fn pointer_to_int_cast_runs() {
    // End-to-end alignment check: malloc(64) returns a 16+-byte-aligned
    // pointer on every libc we care about; `(addr % 16)` is 0.
    let out = compile_and_run("pointer_to_int_cast.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n");
}

#[test]
fn pointer_to_int_cast_emits_ptrtoint() {
    // Pin the codegen choice — sema admits the cast in unsafe, codegen
    // lowers to LLVM `ptrtoint`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let src = format!(
        "{}/../docs/examples/pointer_to_int_cast.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success(), "exited {:?}", out.status);
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("ptrtoint ptr") && ir.contains(" to i64"),
        "expected `ptrtoint ptr ... to i64` in IR; got:\n{ir}"
    );
}

#[test]
fn pointer_to_int_cast_outside_unsafe_rejected() {
    // Negative: ptr-to-int cast outside unsafe must fail with E0801.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         fn main() -> i32 {\n\
             let p: *u8 = unsafe { malloc(8 as usize) };\n\
             let addr: usize = p as usize;\n\
             return addr as i32;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected cpc to reject ptr→int cast outside unsafe"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0801"),
        "expected E0801 in stderr, got:\n{stderr}"
    );
}

// ---- v0.0.9 Phase 4: module-scope `const` and `static` ----

#[test]
fn const_static_basic_runs() {
    // End-to-end: const substitution (200) + immutable static load (100) +
    // static mut load/store under unsafe (255) → 555.
    let out = compile_and_run("const_static_basic.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "555\n");
}

#[test]
fn const_static_emits_expected_globals() {
    // Inspect the emitted IR to pin the load/store routing decision —
    // immutable statics use `constant`, mutable use `global`, const
    // items emit no global at all (substituted in `lower`).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let src = format!(
        "{}/../docs/examples/const_static_basic.cplus",
        env!("CARGO_MANIFEST_DIR")
    );
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success(), "exited {:?}", out.status);
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("@IMMUTABLE_OFFSET = constant i32 50"),
        "expected immutable-static global; ir was:\n{ir}"
    );
    assert!(
        ir.contains("@COUNTER = global i32 5"),
        "expected mutable-static global; ir was:\n{ir}"
    );
    // Const items never become globals — verify ADD_CONST is absent.
    assert!(
        !ir.contains("@ADD_CONST"),
        "const item should be lower-substituted, not emitted as a global; ir was:\n{ir}"
    );
}

#[test]
fn const_static_mut_write_outside_unsafe_rejected() {
    // Negative case: writing to `static mut` without an enclosing
    // `unsafe { ... }` block must fail with E0X34. Pin the diagnostic
    // shape so a future refactor of the unsafe gate doesn't silently
    // relax it.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "static mut COUNTER: i32 = 0;\n\
         fn main() -> i32 { COUNTER = 5; return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected cpc to reject the program, got success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0X34"),
        "expected E0X34 in stderr, got:\n{stderr}"
    );
}

// ---- v0.0.9 follow-up: `static FOO: str = "..."`. Lowers to a
// paired data global (the bytes) + a fat-pointer global (the
// `{ ptr, i64 }` str header). Reads through the regular static-
// load path; closes the cross-cutting "no static str" gap that
// had `vendor/log` allocating ANSI escape sequences per call. ----

#[test]
fn static_str_immutable_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "static GREETING: str = \"hello, world\\n\";\n\
         extern fn write(fd: i32, p: *u8, n: usize) -> isize;\n\
         \n\
         fn main() -> i32 {\n\
             let n: usize = #str_len(GREETING);\n\
             let p: *u8 = #str_ptr(GREETING);\n\
             let _w: isize = unsafe { write(1 as i32, p, n) };\n\
             if n != (13 as usize) { return 1; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc failed to compile static-str program");
    let out = Command::new(&bin).output().expect("run produced binary");
    assert!(out.status.success(), "static str round-trip failed; exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello, world\n");
}

#[test]
fn static_str_with_hex_escape_runs() {
    // Pin the joint case: `\xHH` escape inside a `static str` literal.
    // ANSI escapes are the canonical use case.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "static RESET: str = \"\\x1b[0m\";\n\
         fn main() -> i32 {\n\
             // 4 bytes: ESC, '[', '0', 'm'\n\
             if #str_len(RESET) != (4 as usize) { return 1; }\n\
             let p: *u8 = #str_ptr(RESET);\n\
             if unsafe { *p } != (27 as u8) { return 2; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc failed to compile \\xHH-in-static-str");
    let out = Command::new(&bin).output().expect("run produced binary");
    assert!(
        out.status.success(),
        "\\x1b[0m static-str should be 4 bytes starting with ESC; exited {:?}",
        out.status,
    );
}

// ---- v0.0.9 follow-up: Ty::Mask distinct from Ty::Simd. Compare
// ops on a numeric SIMD now produce a `mask{N}x{M}` value (distinct
// type, identical LLVM `<N x iN>` lowering); `select` / `any` / `all`
// require a mask receiver. End-to-end test: build a mask via `.lt`,
// blend via `.select`, reduce via `.any`. ----

#[test]
fn simd_mask_compare_select_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         \n\
         fn main() -> i32 {\n\
             let a: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);\n\
             let b: f32x4 = f32x4::new(4.0f32, 3.0f32, 2.0f32, 1.0f32);\n\
             // Mask is true where a < b (lanes 0,1) and false where not.\n\
             let m: mask32x4 = a.lt(b);\n\
             // Blend: where mask is set, take a; else take b. Expected lanes\n\
             // are min(a,b) per lane: [1.0, 2.0, 2.0, 1.0].\n\
             let r: f32x4 = m.select(a, b);\n\
             let l0: f32 = r.lane(0 as u32);\n\
             let l1: f32 = r.lane(1 as u32);\n\
             let l2: f32 = r.lane(2 as u32);\n\
             let l3: f32 = r.lane(3 as u32);\n\
             unsafe { printf(#str_ptr(\"%g %g %g %g\\n\\0\"), l0 as f64, l1 as f64, l2 as f64, l3 as f64); }\n\
             // Round-trip: any() should be true (at least lanes 0,1 set);\n\
             // all() should be false (lanes 2,3 not set).\n\
             if !m.any() { return 1; }\n\
             if m.all()  { return 2; }\n\
             // to_bits round-trip: bits.to_mask() should match m.\n\
             let bits: i32x4 = m.to_bits();\n\
             let m2: mask32x4 = bits.to_mask();\n\
             if !m2.any() { return 3; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc failed to compile mask program");
    let out = Command::new(&bin).output().expect("run produced binary");
    assert!(
        out.status.success(),
        "compare → select → any/all round-trip failed; exited {:?}\nstdout: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1 2 2 1\n",
        "blended lanes should be [min(a,b) per lane]"
    );
}

// ---- #addr_of(x) intrinsic: takes the address of a stack local as
// `*T` with zero runtime cost — the alloca pointer is returned
// directly. Closes the "no address-of-local" gap that forced
// vendor/uuid, vendor/log, and vendor/metal to malloc per call. ----

#[test]
fn addr_of_round_trips_through_libc_time() {
    // The canonical addr_of use case: pass a stack local's address to
    // a libc fn that writes through the pointer. `time(#addr_of(t))`
    // both writes `t` and returns the same value — assert they match
    // to prove the addr_of pointer actually aliased the stack slot.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         extern fn time(t: *i64) -> i64;\n\
         \n\
         fn main() -> i32 {\n\
             let mut t: i64 = 0;\n\
             let returned: i64 = unsafe { time(#addr_of(t)) };\n\
             if t == returned { return 0; }\n\
             return 1;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc failed to compile addr_of program");
    let out = Command::new(&bin).output().expect("run produced binary");
    assert!(
        out.status.success(),
        "time(#addr_of(t)) should write t and return the same value; \
         exited {:?}",
        out.status
    );
}

#[test]
fn addr_of_emits_no_alloca_or_load_extras() {
    // Pin codegen: `#addr_of(x)` reuses the existing local alloca with
    // no GEP, no load, no extra store. The IR for `time(#addr_of(t))`
    // should reference `%t` directly as the argument to `@time`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "extern fn time(t: *i64) -> i64;\n\
         fn main() -> i32 {\n\
             let mut t: i64 = 0;\n\
             let _r: i64 = unsafe { time(#addr_of(t)) };\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc --emit-ll");
    assert!(out.status.success(), "--emit-ll failed");
    let ir = String::from_utf8_lossy(&out.stdout);
    // Local `t` lowers to an alloca named with a `t` prefix (e.g.
    // `%t.addr1`). The addr_of result reuses that pointer literally —
    // no GEP, no `inttoptr`, no extra alloca for the pointer itself.
    // Match `@time(ptr %t...)` to allow the suffix the lowering picks.
    let calls_time_with_t_addr = ir
        .lines()
        .any(|l| l.contains("call i64 @time(ptr %t"));
    assert!(
        calls_time_with_t_addr,
        "expected `call i64 @time(ptr %t<suffix>)` — the alloca pointer fed \
         directly with no intermediate; got ir:\n{ir}"
    );
}

// ---- G-023 regression: bare-Ident move into struct-literal field +
// into raw-pointer-store. Pre-fix, the local's scope-exit Drop fired
// even though the value was bitwise-copied into the destination,
// freeing inner heap storage the destination aliased. ----

#[test]
fn g023_struct_literal_field_init_does_not_double_drop() {
    // Repro that motivated the fix: a function builds a non-Copy local
    // (HashMap[str, str]), wraps it in a returned struct (`Wrap { m: m }`),
    // and the caller queries the wrapped map. Pre-fix the local's Drop
    // freed the map's internal table while the field aliased it — the
    // caller saw a zero-length / not-present map even though len()
    // reported 1 (the bitwise-copied count). Post-fix the local's
    // drop_flag flips to false at the struct-literal site so only the
    // wrapper owns the storage.
    //
    // Same root cause hits `Box::new[T]` for non-Copy T, `arena::alloc[T]`
    // for non-Copy T, and any "build a non-Copy local, wrap, return"
    // helper. clap's `ArgMatches` rewrite couldn't ship without this.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\n\
         name    = \"g023_struct_lit\"\n\
         version = \"0.0.1\"\n\
         edition = \"2026\"\n\
         \n\
         [[bin]]\n\
         name = \"g023_struct_lit\"\n\
         path = \"src/main.cplus\"\n\
         \n\
         [dependencies]\n\
         stdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/hash_map\" as map;\n\
         \n\
         pub struct Wrap { pub m: map::HashMap[str, str] }\n\
         \n\
         fn make() -> Wrap {\n\
             let mut m: map::HashMap[str, str] = map::new::[str, str]();\n\
             m.insert(\"name\", \"alice\");\n\
             return Wrap { m: m };\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
             let w: Wrap = make();\n\
             if w.m.contains_key(\"name\") { return 0; }\n\
             return 1;\n\
         }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib_target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor/stdlib");
    symlink_dir(&stdlib_target, &dir.join("vendor/stdlib"));

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed");
    let bin = dir.join("target/debug/g023_struct_lit");
    let out = Command::new(&bin).output().expect("run binary");
    assert!(
        out.status.success(),
        "Wrap{{m: m}} field-init should not drop the local; pre-G-023-fix the \
         field aliased freed HashMap storage and contains_key returned false. \
         exited {:?}",
        out.status
    );
}

#[test]
fn g023_raw_pointer_store_does_not_double_drop() {
    // Repro for the `unsafe { *p = v; }` shape used by `Box::new[T]`
    // and `arena::alloc[T]`. A non-Copy `move v: T` parameter is
    // bitwise-stored into a malloc'd slot; pre-fix, v's scope-exit
    // Drop ran anyway and freed inner heap storage (the Vec's `ptr`
    // buffer) while the slot aliased it. Post-fix, the assign's bare-
    // Ident RHS flips v's drop_flag.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\n\
         name    = \"g023_raw_store\"\n\
         version = \"0.0.1\"\n\
         edition = \"2026\"\n\
         \n\
         [[bin]]\n\
         name = \"g023_raw_store\"\n\
         path = \"src/main.cplus\"\n\
         \n\
         [dependencies]\n\
         stdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         \n\
         extern fn malloc(n: usize) -> *u8;\n\
         \n\
         fn place[T](move val: T) -> *T {\n\
             let raw: *u8 = unsafe { malloc(#size_of::[T]()) };\n\
             let p: *T = unsafe { raw as *T };\n\
             unsafe { *p = val; }\n\
             return p;\n\
         }\n\
         \n\
         fn make_vec() -> vec::Vec[i32] {\n\
             let mut v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(100 as i32);\n\
             v.push(200 as i32);\n\
             return v;\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
             let p: *vec::Vec[i32] = place::[vec::Vec[i32]](make_vec());\n\
             let len: usize = unsafe { (*p).len() };\n\
             let v0: i32 = vec::at_copy::[i32](unsafe { *p }, 0 as usize);\n\
             if len == (2 as usize) && v0 == (100 as i32) { return 0; }\n\
             return 1;\n\
         }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    let stdlib_target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor/stdlib");
    symlink_dir(&stdlib_target, &dir.join("vendor/stdlib"));

    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed");
    let bin = dir.join("target/debug/g023_raw_store");
    let out = Command::new(&bin).output().expect("run binary");
    assert!(
        out.status.success(),
        "place[T](move val) should not Drop val after raw-pointer-store; \
         pre-G-023-fix the slot's Vec.ptr was freed and read-back failed. \
         exited {:?}",
        out.status
    );
}

/// v0.0.12 realtime Phase 8: a `[profile.realtime]` project applies the
/// contract to *local* functions — `cpc check` rejects an allocation in
/// local code with E0901 (and the unknown-extern E0907 from deny_block).
#[test]
fn realtime_profile_rejects_local_allocation() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"f\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"f\"\npath = \"src/main.cplus\"\n\
         [profile.realtime]\ndeny_alloc = true\ndeny_block = true\nstack_limit = 4096\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "extern fn malloc(n: usize) -> *u8;\n\
         fn hot() -> *u8 { return unsafe { malloc(64 as usize) }; }\n\
         fn main() -> i32 { let _p: *u8 = hot(); return 0; }",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc check");
    assert!(!out.status.success(), "profile must reject local allocation");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0901"), "expected E0901, got: {stderr}");
}

/// A clean real-time program (no allocation, no blocking, small frame) passes
/// `cpc check` under an active `[profile.realtime]`.
#[test]
fn realtime_profile_clean_program_passes() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"f\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"f\"\npath = \"src/main.cplus\"\n\
         [profile.realtime]\ndeny_alloc = true\ndeny_block = true\nstack_limit = 4096\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn dsp(x: i32) -> i32 { return x +% 1; }\n\
         fn main() -> i32 { return dsp(41); }",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc check");
    assert!(status.success(), "clean realtime program must pass cpc check");
}

/// v0.0.12 realtime Phase 1 (method-dispatch hole): a `#[no_alloc]` function
/// that reaches an allocating method *through a receiver* (`b.grow()`) used to
/// slip past the checker — only free-fn calls were walked. Now the dispatched
/// method must itself carry the contract.
#[test]
fn no_alloc_rejects_allocating_method_through_receiver() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "extern fn malloc(n: usize) -> *u8;\n\
         struct Bag { ptr: *u8 }\n\
         impl Bag {\n\
             fn grow(mut self) { unsafe { self.ptr = malloc(64 as usize); } return; }\n\
         }\n\
         #[no_alloc]\n\
         fn hot(mut b: Bag) { b.grow(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "allocating method via receiver must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0901"), "expected E0901, got:\n{stderr}");
    assert!(stderr.contains("Bag::grow"), "diagnostic should name the method, got:\n{stderr}");
}

/// Companion positive case: a `#[no_alloc]` function calling a method that is
/// itself `#[no_alloc]` must compile (no false positive). Guards the realtime
/// demo / vendor/rt pattern (e.g. `is_empty` → `self.len()`, both marked).
#[test]
fn no_alloc_allows_marked_method_through_receiver() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "struct Ctr { v: i32 }\n\
         impl Ctr {\n\
             #[no_alloc]\n\
             fn bump(mut self) { self.v = self.v +% 1; return; }\n\
         }\n\
         #[no_alloc]\n\
         fn hot(mut c: Ctr) { c.bump(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(
        out.status.success(),
        "calling a #[no_alloc] method from a #[no_alloc] fn must pass; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `to_string()` allocates an owned `string`; it must be rejected inside a
/// `#[no_alloc]` body (blessed-method allocation, not a user method).
#[test]
fn no_alloc_rejects_to_string() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[no_alloc]\n\
         fn hot(n: i32) { let _s: string = n.to_string(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "to_string in #[no_alloc] must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0901"), "expected E0901, got:\n{stderr}");
}

/// `#[no_block]` mirrors the same dispatch fix: a blocking method reached
/// through a receiver must be rejected when the callee method isn't marked
/// `#[no_block]`.
#[test]
fn no_block_rejects_blocking_method_through_receiver() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "extern fn pthread_mutex_lock(m: *u8) -> i32;\n\
         struct Lock { h: *u8 }\n\
         impl Lock {\n\
             fn take(self) { unsafe { let _r: i32 = pthread_mutex_lock(self.h); } return; }\n\
         }\n\
         #[no_block]\n\
         fn hot(l: Lock) { l.take(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "blocking method via receiver must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0907"), "expected E0907, got:\n{stderr}");
}

#[test]
fn fp_contract_flag_controls_fmuladd_emission() {
    // B-10: `a*b+c` on a float type contracts to `llvm.fmuladd` by default
    // (matching clang's `-ffp-contract=on`). `--fp-contract=off` suppresses
    // the contraction so the IR keeps a separate `fmul` + `fadd`, giving
    // float output bit-identical to a C build compiled with
    // `-ffp-contract=off`. The flag must precede `--emit-ll FILE`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fma.cplus");
    std::fs::write(
        &src,
        "fn compute(a: f32, b: f32, c: f32) -> f32 { return a * b + c; }\n\
         fn main() -> i32 {\n\
         let r: f32 = compute(2.0 as f32, 3.0 as f32, 4.0 as f32);\n\
         return r as i32;\n\
         }\n",
    )
    .unwrap();

    // Default: one fused multiply-add, no separate fmul/fadd in the body.
    let on = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll on");
    assert!(on.status.success());
    let on_ir = String::from_utf8_lossy(&on.stdout);
    assert!(
        on_ir.contains("call contract float @llvm.fmuladd.f32"),
        "default build must contract a*b+c to fmuladd, got:\n{on_ir}"
    );

    // --fp-contract=off: plain fmul + fadd, no fmuladd *call* in the body
    // (the preamble still `declare`s the intrinsic — that's harmless).
    let off = Command::new(cpc)
        .arg("--fp-contract=off")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("emit-ll off");
    assert!(off.status.success());
    let off_ir = String::from_utf8_lossy(&off.stdout);
    assert!(
        !off_ir.contains("call contract float @llvm.fmuladd.f32"),
        "--fp-contract=off must not contract to fmuladd, got:\n{off_ir}"
    );
    assert!(
        off_ir.contains("fmul float") && off_ir.contains("fadd float"),
        "--fp-contract=off must keep separate fmul + fadd, got:\n{off_ir}"
    );
    assert!(
        !off_ir.contains("fmul contract float") && !off_ir.contains("fadd contract float"),
        "--fp-contract=off must drop the `contract` fast-math flag, got:\n{off_ir}"
    );

    // Both modes still build and run to the same (integer-truncated) result.
    for extra in [None, Some("--fp-contract=off")] {
        let bin = dir.join(match extra {
            Some(_) => "fma_off",
            None => "fma_on",
        });
        let mut cmd = Command::new(cpc);
        if let Some(flag) = extra {
            cmd.arg(flag);
        }
        let status = cmd.arg(&src).arg("-o").arg(&bin).status().expect("build");
        assert!(status.success(), "build failed for {extra:?}");
        let run = Command::new(&bin).output().expect("run");
        // 2*3+4 = 10
        assert_eq!(run.status.code(), Some(10), "wrong result for {extra:?}");
    }
}

#[test]
fn fp_contract_rejects_invalid_value() {
    // B-10: an unrecognized `--fp-contract=` value is a usage error.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("x.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--fp-contract=bogus")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "invalid --fp-contract must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--fp-contract expects off|on|fast"),
        "expected usage error, got:\n{stderr}"
    );
}

/// G-044 (llama.cplus): array-literal elements coerce to the annotated element
/// type. `let a: [i64; 4] = [1, 2, 3, 4]` used to build a `[4 x i32]` aggregate
/// and store it into the `[4 x i64]` slot — an LLVM type error at codegen even
/// though `cpc check` passed. Both the explicit-element and fill forms must now
/// compile and produce the right values.
#[test]
fn g044_array_literal_element_coercion() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g044.cplus");
    let bin = dir.join("g044");
    std::fs::write(
        &src,
        "fn elems() -> i64 { let a: [i64; 4] = [1, 2, 3, 4]; return a[3 as usize]; }\n\
         fn fill() -> i64 { let b: [i64; 5] = [7; 5]; return b[4 as usize]; }\n\
         fn main() -> i32 {\n\
             if elems() != (4 as i64) { return 1; }\n\
             if fill() != (7 as i64) { return 2; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "G-044 program must compile: {compile}");
    let run = Command::new(&bin).status().expect("run g044");
    assert!(run.success(), "G-044 program must exit 0, got {run}");
}

/// G-043 (llama.cplus): a `static` array initializer may be an explicit element
/// list (`[10, 20, 30, 40]`), a fill (`[v; N]`), or nested arrays — previously
/// rejected with E0X30 (literal-only). Elements coerce to the declared element
/// type (the static-position analog of G-044).
#[test]
fn g043_static_array_initializer() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g043.cplus");
    let bin = dir.join("g043");
    std::fs::write(
        &src,
        "static T: [i32; 4] = [10, 20, 30, 40];\n\
         static T64: [i64; 5] = [1, 2, 3, 4, 5];\n\
         static NESTED: [[i32; 2]; 2] = [[1, 2], [3, 4]];\n\
         fn main() -> i32 {\n\
             if T[2 as usize] != 30 { return 1; }\n\
             if T64[4 as usize] != (5 as i64) { return 2; }\n\
             if NESTED[1 as usize][0 as usize] != 3 { return 3; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "G-043 program must compile: {compile}");
    let run = Command::new(&bin).status().expect("run g043");
    assert!(run.success(), "G-043 program must exit 0, got {run}");
}

/// G-043 guard: `const` stays literal-only — an array initializer on a `const`
/// is still E0X30 (consts are inlined at use sites; arrays belong in `static`).
#[test]
fn g043_const_array_initializer_still_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g043c.cplus");
    std::fs::write(
        &src,
        "const C: [i32; 2] = [1, 2];\nfn main() -> i32 { return 0; }",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc check");
    assert!(!out.status.success(), "const array initializer must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0X30"), "expected E0X30, got: {stderr}");
}

/// G-034 (llama.cplus): an indexed write to a `pub static mut [T; N]` resolved
/// the static name (was E0300 "undefined name" — only the indexed-write LHS
/// path failed, while indexed read and scalar write worked).
#[test]
fn g034_static_mut_indexed_write() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g034.cplus");
    let bin = dir.join("g034");
    std::fs::write(
        &src,
        "pub static mut TABLE: [i32; 16] = #zero::[[i32; 16]]();\n\
         fn fill() {\n\
             let mut i: usize = 0 as usize;\n\
             while i < (16 as usize) {\n\
                 unsafe { TABLE[i] = (i as i32) *% (2 as i32); };\n\
                 i = i +% (1 as usize);\n\
             }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             fill();\n\
             let v: i32 = unsafe { TABLE[5 as usize] };\n\
             if v != 10 { return 1; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "G-034 program must compile: {compile}");
    let run = Command::new(&bin).status().expect("run g034");
    assert!(run.success(), "G-034 program must exit 0, got {run}");
}

/// G-034 guard: a genuinely undefined name in indexed-write position still
/// reports E0300 (the fix must not swallow real undefined-name errors).
#[test]
fn g034_undefined_indexed_write_still_e0300() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("g034u.cplus");
    std::fs::write(
        &src,
        "fn f() { NOPE[0 as usize] = 1; return; }\nfn main() -> i32 { return 0; }",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc check");
    assert!(!out.status.success(), "undefined indexed write must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0300"), "expected E0300, got: {stderr}");
}

/// G-045 (llama.cplus): native `f16` scalar — `as` conversions (fpext/fptrunc),
/// `from_bits`/`to_bits` (LLVM bitcast), struct/array storage, and arithmetic.
/// This is the enabler for pure-C+ fp16↔fp32 (the "zero-`.c`" headline).
#[test]
fn g045_f16_scalar_end_to_end() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("f16.cplus");
    let bin = dir.join("f16");
    std::fs::write(
        &src,
        "fn fp16_to_fp32(bits: u16) -> f32 { return f16::from_bits(bits) as f32; }\n\
         fn fp32_to_fp16(x: f32) -> u16 { return (x as f16).to_bits(); }\n\
         struct Block { d: f16, n: i32 }\n\
         fn main() -> i32 {\n\
             // `as` round-trip (fptrunc + fpext); 1.5 is exact in f16\n\
             let r: f32 = (1.5f32 as f16) as f32;\n\
             if r < 1.49f32 { return 1; }\n\
             if r > 1.51f32 { return 2; }\n\
             // from_bits: IEEE half 0x3C00 == 1.0\n\
             let one: f32 = fp16_to_fp32(0x3C00 as u16);\n\
             if one < 0.999f32 { return 3; }\n\
             if one > 1.001f32 { return 4; }\n\
             // to_bits/from_bits round-trip through the u16 storage rep\n\
             let back: f32 = fp16_to_fp32(fp32_to_fp16(2.5f32));\n\
             if back < 2.49f32 { return 5; }\n\
             if back > 2.51f32 { return 6; }\n\
             // f64.to_bits bit pattern of 1.0\n\
             if (1.0f64).to_bits() != 0x3FF0000000000000u64 { return 7; }\n\
             // f16 as struct field + array storage\n\
             let b: Block = Block { d: 1.5f32 as f16, n: 0 };\n\
             if (b.d as f32) < 1.49f32 { return 8; }\n\
             let mut arr: [f16; 2] = [0.0f32 as f16, 0.0f32 as f16];\n\
             arr[1] = 3.0f32 as f16;\n\
             if (arr[1] as f32) < 2.99f32 { return 9; }\n\
             // f16 arithmetic (LLVM legalizes) + size_of\n\
             let s: f16 = (2.0f32 as f16) + (3.0f32 as f16);\n\
             if (s as f32) < 4.99f32 { return 10; }\n\
             if #size_of::[f16]() != (2 as usize) { return 11; }\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(compile.success(), "G-045 program must compile: {compile}");
    let run = Command::new(&bin).status().expect("run f16");
    assert!(run.success(), "G-045 program must exit 0, got {run}");
}

/// G-045 guard: `from_bits` is type-checked — `f16::from_bits` wants a `u16`,
/// so passing a float is E0302 (the bitcast is bit-preserving, not a convert).
#[test]
fn g045_from_bits_wrong_arg_type_e0302() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("f16neg.cplus");
    std::fs::write(
        &src,
        "fn f() -> f16 { return f16::from_bits(1.0f32); }\nfn main() -> i32 { return 0; }",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&src).output().expect("invoke cpc check");
    assert!(!out.status.success(), "from_bits with float arg must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0302"), "expected E0302, got: {stderr}");
}

/// Set up a minimal self-contained project (no deps) for the graph tests and
/// return its root directory. The entry defines a struct with a method so the
/// graph has fields, methods, and a `defines` edge to exercise.
fn graph_project() -> std::path::PathBuf {
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"g\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"g\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "struct Point { pub x: i32, pub y: i32 }\n\
         impl Point {\n\
             fn sum(self) -> i32 { return self.x +% self.y; }\n\
         }\n\
         fn main() -> i32 {\n\
             let p: Point = Point { x: 1, y: 2 };\n\
             return p.sum();\n\
         }\n",
    )
    .unwrap();
    dir
}

#[test]
fn graph_emits_nodes_and_edges_json() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    let out = Command::new(cpc)
        .arg("graph")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc graph");
    assert!(out.status.success(), "cpc graph exited non-zero: {}", out.status);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"nodes\""), "missing nodes array: {s}");
    assert!(s.contains("\"edges\""), "missing edges array: {s}");
    assert!(s.contains("\"name\": \"Point\""), "missing Point node: {s}");
    assert!(s.contains("\"name\": \"sum\""), "missing sum method node: {s}");
    assert!(s.contains("\"has_field\""), "missing has_field edge: {s}");
    assert!(s.contains("\"has_method\""), "missing has_method edge: {s}");
    assert!(s.contains("\"defines\""), "missing defines edge: {s}");
}

#[test]
fn query_def_and_members_resolve() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();

    // def by bare name resolves the struct.
    let def = Command::new(cpc)
        .args(["query", "def", "Point"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query def");
    assert!(def.status.success(), "query def Point should find the symbol");
    let s = String::from_utf8_lossy(&def.stdout);
    assert!(s.contains("\"kind\": \"struct\""), "def not a struct: {s}");
    assert!(s.contains("\"name\": \"Point\""), "def wrong name: {s}");

    // members lists fields and methods.
    let mem = Command::new(cpc)
        .args(["query", "members", "Point"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query members");
    assert!(mem.status.success());
    let m = String::from_utf8_lossy(&mem.stdout);
    assert!(m.contains("\"name\": \"x\""), "members missing field x: {m}");
    assert!(m.contains("\"name\": \"sum\""), "members missing method sum: {m}");
}

#[test]
fn query_missing_symbol_exits_nonzero() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    let out = Command::new(cpc)
        .args(["query", "def", "Nonexistent"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query def");
    assert!(!out.status.success(), "not-found must exit non-zero");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
}

#[test]
fn query_unknown_kind_reports_and_fails() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    let out = Command::new(cpc)
        .args(["query", "bogus-kind", "x"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query");
    assert!(!out.status.success(), "unknown kind must exit non-zero");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("unknown query kind"),
        "expected an unknown-kind message, got: {err}"
    );
}

#[test]
fn query_type_at_resolves_a_typed_local() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    // graph_project line 6 is `let p: Point = Point { x: 1, y: 2 };` (the
    // string-continuation `\` strips indentation, so `p` is at column 5).
    let out = Command::new(cpc)
        .args(["query", "type-at", "src/main.cplus:6:5"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query type-at");
    assert!(out.status.success(), "type-at on `p` should resolve");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"type\": \"Point\""), "p is a Point: {s}");
    assert!(s.contains("\"kind\": \"type-at\""));

    // A bad position format exits non-zero.
    let bad = Command::new(cpc)
        .args(["query", "type-at", "src/main.cplus"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query type-at");
    assert!(!bad.status.success(), "malformed position must exit non-zero");
}

#[test]
fn query_callers_and_callees_resolve_method_calls() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    // graph_project's main: `let p: Point = ...; p.sum()` → main calls Point::sum.
    let callers = Command::new(cpc)
        .args(["query", "callers", "sum"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callers");
    assert!(callers.status.success());
    let c = String::from_utf8_lossy(&callers.stdout);
    assert!(c.contains("\"name\": \"main\""), "main should call sum: {c}");
    assert!(c.contains("\"unresolved\""), "callers carries unresolved count: {c}");

    let callees = Command::new(cpc)
        .args(["query", "callees", "main"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callees");
    assert!(callees.status.success());
    let ce = String::from_utf8_lossy(&callees.stdout);
    assert!(ce.contains("\"name\": \"sum\""), "callees of main include sum: {ce}");
}

#[test]
fn query_refs_returns_call_sites_with_locations() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    // main calls Point::sum once → one resolved reference at a real location.
    let out = Command::new(cpc)
        .args(["query", "refs", "sum"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query refs");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"kind\": \"refs\""));
    assert!(s.contains("\"scope\""), "refs states its coverage: {s}");
    assert!(s.contains("\"in_context\""), "a reference carries its enclosing item: {s}");
    assert!(s.contains("\"line\""), "a reference carries a location: {s}");

    // An unknown symbol exits non-zero.
    let u = Command::new(cpc)
        .args(["query", "refs", "does_not_exist"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query refs");
    assert!(!u.status.success(), "unknown symbol must exit non-zero");
}

/// v0.0.13: free-function (and `module::fn` path) calls resolve. The resolver
/// rewrites the callee to its qualified dotted form; the graph now matches that
/// against node ids, so ordinary direct calls produce `Calls` edges instead of
/// landing in `unresolved`. Regression for the under-reporting bug that the
/// method-only fixture above missed.
#[test]
fn query_callers_resolves_free_function_calls() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"g\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"g\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    // `helper` is a free function called twice from `mid`, which `main` calls.
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn helper() -> i32 { return 7; }\n\
         fn mid() -> i32 { return helper() +% helper(); }\n\
         fn main() -> i32 { return mid(); }\n",
    )
    .unwrap();
    // callers(helper) resolves to `mid`, with no unresolved residue.
    let callers = Command::new(cpc)
        .args(["query", "callers", "helper"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callers");
    assert!(callers.status.success());
    let c = String::from_utf8_lossy(&callers.stdout);
    assert!(c.contains("\"name\": \"mid\""), "mid should call helper: {c}");
    assert!(c.contains("\"unresolved\": 0"), "free calls must resolve, not land in unresolved: {c}");
    // refs(helper) finds both call sites.
    let refs = Command::new(cpc)
        .args(["query", "refs", "helper"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query refs");
    let r = String::from_utf8_lossy(&refs.stdout);
    assert_eq!(r.matches("\"line\"").count(), 2, "two call sites of helper: {r}");
}

/// The honest floor: a call *through a function pointer* genuinely can't be
/// named, so it stays in `unresolved` (C+ has no other indirect dispatch).
#[test]
fn query_fn_pointer_call_stays_unresolved() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"g\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"g\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn h(x: i32) -> i32 { return x; }\n\
         fn main() -> i32 { let f: fn(i32) -> i32 = h; return f(5); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .args(["query", "callees", "main"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callees");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    // The indirect `f(5)` call is unresolved; `h` is not a resolved callee.
    assert!(s.contains("\"unresolved\": 1"), "fn-pointer call is the unresolved floor: {s}");
}

#[test]
fn query_context_packs_the_neighborhood() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    // `sum` is called by main → context(sum) has main as a caller; context(main)
    // has sum as a callee. One call, the whole neighborhood.
    let out = Command::new(cpc)
        .args(["query", "context", "main"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query context");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"kind\": \"context\""));
    assert!(s.contains("\"target\""), "context carries the target node: {s}");
    assert!(s.contains("\"callees\""), "context carries callees: {s}");
    assert!(s.contains("\"name\": \"sum\""), "main's callee sum appears: {s}");

    let u = Command::new(cpc)
        .args(["query", "context", "Point"]) // a struct, not a fn → not found
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query context");
    assert!(!u.status.success(), "context of a non-function exits non-zero");
}

#[test]
fn mcp_server_handshake_and_tool_call() {
    use std::io::Write;
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    let mut child = Command::new(cpc)
        .arg("mcp")
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn cpc mcp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        let msgs = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"find_callers","arguments":{"function":"sum"}}}"#,
        ];
        for m in msgs {
            writeln!(stdin, "{m}").expect("write");
        }
    } // dropping stdin closes it → server loop ends
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    // initialize + tools/list + tools/call → 3 responses; the notification got none.
    assert_eq!(lines.len(), 3, "expected 3 responses, got: {s}");

    let init: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "cpc-graph");

    let list: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let names: Vec<String> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"find_callers".to_string()));
    assert!(names.contains(&"code_context".to_string()));

    let call: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"name\": \"main\""), "main calls sum: {text}");
}

#[test]
fn query_call_hierarchy_and_unknown_fn() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = graph_project();
    let h = Command::new(cpc)
        .args(["query", "call-hierarchy", "main", "--depth", "2"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query call-hierarchy");
    assert!(h.status.success());
    assert!(String::from_utf8_lossy(&h.stdout).contains("\"kind\": \"call-hierarchy\""));

    // An unknown function name exits non-zero.
    let u = Command::new(cpc)
        .args(["query", "callers", "does_not_exist"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callers");
    assert!(!u.status.success(), "unknown fn must exit non-zero");
}

#[test]
fn cstring_literal_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("cstr.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         fn main() -> i32 {\n\
             let m: *u8 = c\"hi\\n\";\n\
             unsafe { printf(m); }\n\
             unsafe { printf(c\"n=%d\\n\", 7 as i32); }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("cstr");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "c-string program must compile");
    let run = Command::new(&bin).output().expect("run produced binary");
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "hi\nn=7\n");
}

#[test]
fn f16_literal_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("f16.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         fn main() -> i32 {\n\
             let h: f16 = 0.5f16;\n\
             let x: f32 = h as f32;\n\
             unsafe { printf(c\"%.3f\\n\", x as f64); }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("f16");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "f16-literal program must compile");
    let run = Command::new(&bin).output().expect("run produced binary");
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "0.500\n");
}

// v0.0.13 (G-043 second half): struct-literal statics — the ggml
// `static const sphere_t scene[10] = {...}` port pattern. A scalar struct
// static, a struct-of-struct, and an array-of-struct all read back at runtime.
#[test]
fn struct_literal_static_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("scene.cplus");
    std::fs::write(
        &src,
        "struct Vec3 { x: f32, y: f32, z: f32 }\n\
         struct Sphere { center: Vec3, radius: f32, color: i32, visible: bool }\n\
         static SUN: Sphere = Sphere {\n\
             center: Vec3 { x: 0.0f32, y: 0.0f32, z: 0.0f32 },\n\
             radius: 2.0f32, color: 100, visible: true,\n\
         };\n\
         static SCENE: [Sphere; 3] = [\n\
             Sphere { center: Vec3 { x: 1.0f32, y: 0.0f32, z: 0.0f32 }, radius: 1.0f32, color: 1, visible: true },\n\
             Sphere { center: Vec3 { x: 0.0f32, y: 2.0f32, z: 0.0f32 }, radius: 3.0f32, color: 2, visible: false },\n\
             Sphere { center: Vec3 { x: 0.0f32, y: 0.0f32, z: 5.0f32 }, radius: 4.0f32, color: 3, visible: true },\n\
         ];\n\
         fn main() -> i32 {\n\
             // SUN.color(100) + SUN.radius(2) = 102\n\
             let mut acc: i32 = SUN.color +% (SUN.radius as i32);\n\
             // sum of radii (1+3+4)=8, sum of colors (1+2+3)=6, z of [2]=5\n\
             let mut i: i32 = 0;\n\
             while i < 3 {\n\
                 acc = acc +% (SCENE[i as usize].radius as i32);\n\
                 acc = acc +% SCENE[i as usize].color;\n\
                 i = i +% 1;\n\
             }\n\
             acc = acc +% (SCENE[2].center.z as i32);\n\
             return acc;   // 102 + 8 + 6 + 5 = 121\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("scene");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "struct-literal-static program must compile");
    let run = Command::new(&bin).output().expect("run produced binary");
    assert_eq!(run.status.code(), Some(121), "expected exit 121");
}

// A struct-literal static with a non-literal field value is rejected (E0X30),
// and the generic struct-literal form is excluded.
#[test]
fn struct_literal_static_non_literal_field_rejected_e0x30() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "struct P { x: i32, y: i32 }\n\
         fn f() -> i32 { return 3; }\n\
         static BAD: P = P { x: f(), y: 2 };\n\
         fn main() -> i32 { return BAD.x; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0X30"), "expected E0X30, got: {stderr}");
}

// v0.0.13: const-eval for array lengths — `[T; N]` and `[v; N]` where `N` is a
// non-negative integer `const`. Folds in the lower pass; every later pass sees
// a plain length. Exercises type position (let + param + struct field) and the
// fill-count position.
#[test]
fn const_array_length_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("clen.cplus");
    std::fs::write(
        &src,
        "const CAP: usize = 8;\n\
         const ROWS: u32 = 3;\n\
         struct Grid { cells: [i32; CAP] }\n\
         fn sum(buf: [i32; CAP]) -> i32 {\n\
             let mut s: i32 = 0;\n\
             let mut i: i32 = 0;\n\
             while i < (CAP as i32) { s = s +% buf[i as usize]; i = i +% 1; }\n\
             return s;\n\
         }\n\
         fn main() -> i32 {\n\
             let a: [i32; CAP] = [2; CAP];\n\
             let g: Grid = Grid { cells: [1; CAP] };\n\
             let mut total: i32 = sum(a);\n\
             total = total +% g.cells[0];\n\
             let m: [u8; ROWS] = [0u8; ROWS];\n\
             total = total +% (m[2] as i32);\n\
             return total;   // 2*8 + 1 + 0 = 17\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("clen");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "const-array-length program must compile");
    let run = Command::new(&bin).output().expect("run produced binary");
    assert_eq!(run.status.code(), Some(17), "expected exit 17");
}

// An unknown const-name array length is rejected with E0X36.
#[test]
fn unknown_const_array_length_rejected_e0x36() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("badlen.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let a: [i32; NOPE] = [0; 1]; return a[0]; }\n",
    )
    .unwrap();
    let bin = dir.join("badlen");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0X36"), "expected E0X36, got: {stderr}");
}

// v0.0.13 (topic D): `#[inline(always)]` emits `alwaysinline`, which LLVM honors
// even at debug -O0 — so a marked SIMD/kernel wrapper is inlined away (no `call`
// survives) where an unmarked one stays a real call. This is the lever for hot
// kernels built from vendor/simd Tier-2 wrappers. Verified via --emit-ll-opt.
#[test]
fn inline_always_inlines_at_debug_o0() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("inl.cplus");
    std::fs::write(
        &src,
        "#[inline(always)] fn scale(v: f32x4, k: f32) -> f32x4 { return v.mul(f32x4::splat(k)); }\n\
         fn main() -> i32 {\n\
             let a: f32x4 = f32x4::splat(2.0f32);\n\
             let b: f32x4 = scale(a, 3.0f32);\n\
             return b.lane(0 as u32) as i32;   // 6\n\
         }\n",
    )
    .unwrap();
    // The post-opt debug IR must have no surviving call to @scale.
    let out = Command::new(cpc)
        .arg("--emit-ll-opt")
        .arg(&src)
        .output()
        .expect("invoke cpc --emit-ll-opt");
    assert!(out.status.success(), "emit-ll-opt failed");
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        !ir.contains("call") || !ir.contains("@scale"),
        "alwaysinline fn should be inlined away at -O0; IR:\n{ir}"
    );
    // And it still runs correctly.
    let bin = dir.join("inl");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(run.status.code(), Some(6), "expected exit 6");
}

// v0.0.13 (topic C tail): `--realtime-report` digest of the contract analysis.
// A `[profile.realtime]` project with an allocating function reports the E0901 /
// E0907 violations as JSON and exits non-zero (CI gate + artifact). No deps, so
// no vendor symlink needed.
#[test]
fn realtime_report_json_flags_violations() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"rt\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"rt\"\npath = \"src/main.cplus\"\n\
         [profile.realtime]\ndeny_alloc = true\ndeny_block = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "extern fn malloc(n: usize) -> *u8;\n\
         fn bad() -> i32 { let p: *u8 = unsafe { malloc(8 as usize) }; if p.is_null() { return 1; } return 0; }\n\
         fn main() -> i32 { return bad(); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--realtime-report=json")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc --realtime-report=json");
    // Non-zero: violations present (CI gate).
    assert!(!out.status.success(), "expected non-zero exit on violations");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"kind\": \"realtime-report\""), "stdout:\n{stdout}");
    assert!(stdout.contains("E0901"), "expected a no_alloc violation; stdout:\n{stdout}");
    assert!(stdout.contains("\"clean\": false"), "stdout:\n{stdout}");
    assert!(stdout.contains("\"no_alloc\": 1"), "stdout:\n{stdout}");
}

#[test]
fn realtime_report_clean_exits_zero() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"rt\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\
         [[bin]]\nname = \"rt\"\npath = \"src/main.cplus\"\n\
         [profile.realtime]\ndeny_alloc = true\ndeny_block = true\nstack_limit = 4096\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn good(x: i32) -> i32 { return x +% 1; }\n\
         fn main() -> i32 { return good(41); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--realtime-report")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc --realtime-report");
    assert!(out.status.success(), "clean project must exit zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("clean"), "stdout:\n{stdout}");
    assert!(stdout.contains("functions under contract: 2"), "stdout:\n{stdout}");
}

/// TEXT.1: an `unsafe fn` (free function and method) compiles and runs when
/// every call is inside an `unsafe { ... }` block. The exit code threads the
/// returned values through to prove the bodies actually executed.
#[test]
fn unsafe_fn_compiles_and_runs_when_called_in_unsafe_block() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("unsafe_ok.cplus");
    std::fs::write(
        &src,
        "struct Counter { n: i32 }\n\
         impl Counter { unsafe fn raw_get(self) -> i32 { return self.n; } }\n\
         unsafe fn danger() -> i32 { return 42; }\n\
         fn main() -> i32 {\n\
             let c: Counter = Counter { n: 7 };\n\
             let a: i32 = unsafe { c.raw_get() };\n\
             let b: i32 = unsafe { danger() };\n\
             return a +% b;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("unsafe_ok");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "cpc must compile unsafe-fn program");
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(49), "7 + 42 should reach the exit code");
}

/// TEXT.1 (negative): calling an `unsafe fn` outside an `unsafe { ... }` block
/// is a compile error (E0801) — the program must not build.
#[test]
fn unsafe_fn_call_outside_unsafe_block_fails_to_compile() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("unsafe_bad.cplus");
    std::fs::write(
        &src,
        "unsafe fn danger() -> i32 { return 1; }\n\
         fn main() -> i32 { return danger(); }\n",
    )
    .unwrap();
    let bin = dir.join("unsafe_bad");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "bare unsafe-fn call must fail to compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0801"), "expected E0801, stderr:\n{stderr}");
}

/// TEXT.2: vendor the `stdlib/text` module (and its `option` dep) into a temp
/// project and write `src/main.cplus`. Mirrors the other stdlib e2e setups.
#[cfg(target_os = "macos")]
fn setup_text_project(dir: &std::path::Path, main_src: &str) {
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"textt\"\n\n[[bin]]\nname = \"textt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["text", "option"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    std::fs::write(dir.join("src/main.cplus"), main_src).unwrap();
}

/// TEXT.2: the `Text` stdlib type builds, links, and its core API
/// (from_str / push_str / len / starts_with / ends_with / contains / find /
/// clone / `unsafe` as_str) returns correct results. The exit code is the
/// number of the 7 checks that passed, so a wrong answer is visible.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_core_api_builds_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         import \"stdlib/option\" as option;\n\
         fn main() -> i32 {\n\
             let mut t: text::Text = text::from_str(\"hello\");\n\
             t.push_str(\", world\");\n\
             let mut score: i32 = 0;\n\
             if t.len() == (12 as usize) { score = score +% 1; }\n\
             if t.starts_with(\"hello\") { score = score +% 1; }\n\
             if t.ends_with(\"world\") { score = score +% 1; }\n\
             if t.contains(\"lo, wo\") { score = score +% 1; }\n\
             match t.find(\"world\") {\n\
                 option::Option[usize]::Some(i) => { if i == (7 as usize) { score = score +% 1; } }\n\
                 option::Option[usize]::None => { }\n\
             }\n\
             let c: text::Text = t.clone();\n\
             if c.len() == (12 as usize) { score = score +% 1; }\n\
             let v: str = unsafe { c.as_str() };\n\
             if #str_len(v) == (12 as usize) { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of stdlib/text consumer failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(7), "all 7 Text API checks must pass");
}

/// TEXT.2 + TEXT.1: `Text::as_str` is an `unsafe fn`, so calling it without an
/// `unsafe { ... }` block fails to compile (E0801) — the dangling-view escape
/// hatch is opt-in even through the real stdlib type.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_as_str_requires_unsafe_block() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let t: text::Text = text::from_str(\"hi\");\n\
             let v: str = t.as_str();\n\
             return 0;\n\
         }\n",
    );
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "bare Text::as_str call must fail to compile"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0801"), "expected E0801, stderr:\n{stderr}");
}

/// TEXT.R1: a string literal in a `Text`-typed `let` constructs an owned `Text`
/// (the `#[lang("string")]` lowering) — builds, runs, drops clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_literal_in_let_constructs_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let s: text::Text = \"hello, world\";\n\
             let mut score: i32 = 0;\n\
             if s.len() == (12 as usize) { score = score +% 1; }\n\
             if s.starts_with(\"hello\") { score = score +% 1; }\n\
             if s.contains(\"o, w\") { score = score +% 1; }\n\
             let v: str = unsafe { s.as_str() };\n\
             if #str_len(v) == (12 as usize) { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of `let s: Text = literal` failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(4), "all 4 literal-Text checks must pass");
}

/// TEXT.R1 (lockstep): literal→`Text` coercion is scoped to the `let` site.
/// A literal passed where a `Text` is expected (function arg) is still a `str`
/// and must error (E0302) — sema and codegen agree on the supported context.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_literal_as_arg_still_errors() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn take(t: text::Text) -> usize { return t.len(); }\n\
         fn main() -> i32 {\n\
             let n: usize = take(\"hello\");\n\
             return n as i32;\n\
         }\n",
    );
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "literal as a Text arg must fail (coercion is let-scoped)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0302"), "expected E0302, stderr:\n{stderr}");
}

/// TEXT.R1 + multi-line: a triple-quoted `"""..."""` literal in a `Text`-typed
/// `let` constructs an owned `Text` whose value is the bytes between the
/// delimiters, verbatim — no indentation stripping, leading/trailing newlines
/// kept. Builds, runs, ASan-clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_multiline_literal_is_verbatim() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let banner: text::Text = \"\"\"\nusage: build <file>\n  --out <dir>\n\"\"\";\n\
             let mut score: i32 = 0;\n\
             if banner.starts_with(\"\\nusage:\") { score = score +% 1; }\n\
             if banner.contains(\"--out <dir>\") { score = score +% 1; }\n\
             if banner.ends_with(\"\\n\") { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of multi-line Text literal failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "verbatim multi-line checks must pass");
}

/// TEXT.R1c: `return "literal";` (and a multi-line literal) from a
/// `Text`-returning function constructs an owned `Text`. Builds, runs, clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_literal_in_return_constructs_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn label() -> text::Text { return \"OK\"; }\n\
         fn banner() -> text::Text { return \"\"\"\nhi\n\"\"\"; }\n\
         fn main() -> i32 {\n\
             let a: text::Text = label();\n\
             let b: text::Text = banner();\n\
             let mut score: i32 = 0;\n\
             if a.starts_with(\"OK\") { score = score +% 1; }\n\
             if a.len() == (2 as usize) { score = score +% 1; }\n\
             if b.contains(\"hi\") { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of `return literal` -> Text failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "return-Text checks must pass");
}

/// TEXT.R1c: a string literal for a `Text`-typed struct field constructs an
/// owned `Text` — the common UI pattern `Widget { label: "OK", .. }`. Builds,
/// runs, the container drops the field clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_literal_in_struct_field_constructs_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         struct Widget { label: text::Text, id: i32 }\n\
         fn main() -> i32 {\n\
             let w: Widget = Widget { label: \"Submit\", id: 7 };\n\
             let mut score: i32 = 0;\n\
             if w.label.len() == (6 as usize) { score = score +% 1; }\n\
             if w.label.starts_with(\"Sub\") { score = score +% 1; }\n\
             if w.id == 7 { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of struct Text field literal failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "struct-field Text checks must pass");
}

fn tempdir() -> std::path::PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("cpc-test-")
        .tempdir()
        .expect("tempdir creation");
    // Leak intentionally: tests run in parallel and the returned PathBuf
    // outlives the test fn's scope (passed into Command::new, etc.).
    // OS cleans /tmp on reboot; tests use distinct paths so no collisions.
    let leaked: &'static tempfile::TempDir = Box::leak(Box::new(dir));
    leaked.path().to_path_buf()
}

/// Binutils program names differ on Windows, where the GNU `ar`/`nm` are
/// absent but LLVM ships `llvm-ar`/`llvm-nm` (same CLI surface). These let
/// the archive/symbol-inspection tests run unprivileged against the LLVM
/// toolchain on every host.
#[allow(dead_code)]
fn ar_prog() -> &'static str {
    if cfg!(windows) { "llvm-ar" } else { "ar" }
}
#[allow(dead_code)]
fn nm_prog() -> &'static str {
    if cfg!(windows) { "llvm-nm" } else { "nm" }
}

/// Make `link` a directory alias for the existing directory `target`.
///
/// Tests stage a tempdir project whose `vendor/stdlib` points at the
/// in-tree `vendor/stdlib` so the build picks up the current sources.
/// Unix uses a plain symlink. Windows uses a *directory junction*
/// (`mklink /J`) rather than a symlink: junctions need no Developer Mode
/// or admin privilege, so the suite runs unprivileged in CI. `target`
/// must be an existing directory and `link` must not already exist.
#[allow(dead_code)]
fn symlink_dir(target: &std::path::Path, link: &std::path::Path) {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).expect("create dir symlink");
    }
    #[cfg(windows)]
    {
        // `mklink` is a cmd builtin and parses `/x` tokens as switches, so a
        // path containing a forward slash (e.g. `vendor/stdlib`, which
        // `Path::join` does NOT normalize) makes it choke with
        // "Invalid switch". Normalize separators to backslashes first.
        let link = link.to_string_lossy().replace('/', "\\");
        let target = target.to_string_lossy().replace('/', "\\");
        let out = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .output()
            .expect("invoke mklink");
        assert!(
            out.status.success(),
            "mklink /J {link} {target} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
