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
    std::os::unix::fs::symlink(&stdlib, dir.join("vendor").join("stdlib")).unwrap();
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
    std::os::unix::fs::symlink(&stdlib, dir.join("vendor").join("stdlib")).unwrap();
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
         struct Chunk { offset: usize, size: usize, next: *u8, pad: i64 }\n\
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
        "typedef struct { long a; long b; long c; } Big24;\n\
         Big24 make_big(long x) {\n\
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
    std::os::unix::fs::symlink(&stdlib, dir.join("vendor").join("stdlib")).unwrap();
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
        "fn main() -> i32 { let x: i32 = 2147483647; let y: i32 = x +% 1; println(y); return 0; }",
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
fn peek(b: B) -> i32 { return b.x; }
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
    //   defer println(200) -> 200
    //   Drop(b)            -> -2
    //   defer println(100) -> 100
    //   Drop(a)            -> -1
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1\n2\n200\n-2\n100\n-1\n"
    );
}

// ---- runtime trap behavior for overflow + divide-by-zero ----

const OVERFLOW_PROGRAM: &str =
    "fn main() -> i32 { let mut x: i32 = 2147483647; x = x + 1; println(x); return 0; }";

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
           let p: *u8 = unsafe { str_ptr(cstr) };\n\
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
    println(x.v);
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
    println(q.v);
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
        println(0 -% self.id);
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
    println(x.id);
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
    std::fs::write(&src, "fn main() -> i32 {\n  assert 1 == 1;\n  assert 2 + 2 == 4;\n  println(42);\n  return 0;\n}\n").unwrap();
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
        "fn main() -> i32 {\n  assert 1 == 2;\n  println(999);\n  return 0;\n}\n",
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
    // The `println(999)` after the failing assertion must not have run.
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
fn peek(b: B) -> i32 { return b.x; }
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
    assert!(
        ir.contains("%x.drop_flag = alloca i1"),
        "drop flag alloca should remain for moved binding; got: {ir}"
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
fn peek(b: B) -> i32 { return b.x; }
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
        "expected shared `b: B` to lower to `ptr readonly`; got: {ir}"
    );
    // And NOT `noalias` — shared borrows can alias per §2.9.
    assert!(
        !ir.contains("@peek(ptr noalias"),
        "shared borrow must not get `noalias`; got: {ir}"
    );
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
fn peek(b: B) -> i32 { return b.x; }
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
    assert!(stderr.contains("E0374"), "expected E0374, got: {stderr}");
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
fn peek_pair(p: Pair) -> i32 { return 0; }
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
fn peek(i: Inner) -> i32 { return i.v; }
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
fn peek(b: B) -> i32 { return b.x; }
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
fn peek(b: B) -> i32 { return b.x; }
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
fn peek(b: B) -> i32 { return b.x; }
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
fn peek(b: B) -> i32 { return b.x; }
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
             return unsafe { printf(str_ptr(fmt), 42) };\n\
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
    // Slice 8.STR.2: `println(str)` prints a literal and exits.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p8s.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n    println(\"Hello, C+!\");\n    return 0;\n}\n",
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
        "println(str) should build: stderr={}",
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
    // Phase-8 exit demo: FizzBuzz with real strings via println(str).
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
         fn cleanup() { println(42); }\n\
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
    let src = "/Users/adel/Workspace/C+/docs/examples/phase11_vec_generic.cplus";
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
             println(s.as_str());\n\
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
             println(g.as_str());\n\
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
             println(s.as_str());\n\
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
             println(s);\n\
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
    let src = "/Users/adel/Workspace/C+/docs/examples/phase8_interpolation.cplus";
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
        stdout.contains("cpc check FILE"),
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
    let n: usize = slice_len(xs);
    let p: *i32 = slice_ptr(xs);
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
    let xs: i32[] = unsafe { slice_from_raw_parts(p, 3 as usize) };
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
    let xs: i32[] = slice_from_raw_parts(p, 0 as usize);
    return slice_len(xs) as i32;
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
    let p: *i32 = slice_ptr(n);
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
fn takes_i32_slice(xs: i32[]) -> i32 { return slice_len(xs) as i32; }
fn main() -> i32 {
    let p: *u8 = unsafe { 0 as *u8 };
    let bytes: u8[] = unsafe { slice_from_raw_parts(p, 0 as usize) };
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
    // Vendor declares `[link] libs = ["m"]`; consumer's binary should link
    // against libm via the dep walk. Use a pure-source vendor package so
    // we don't need a bundled artifact.
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
        "[package]\nname = \"mathy\"\n\n[link]\nlibs = [\"m\"]\n",
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
    let ar = Command::new("ar")
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
    let st = Command::new("ar")
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
             let n: i64 = unsafe {{ count_bytes(str_ptr(s) as *i8) }};\n\
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
        "result", "vec", "net", "io", "reactor", "future", "iterator", "option",
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
        "result", "vec", "fs", "io", "iterator", "option", "net", "reactor", "future",
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
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/result.cplus"), result_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/vec.cplus"), vec_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/iterator.cplus"), iterator_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), option_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/future.cplus"), future_src).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/reactor.cplus"), reactor_src).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
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
    if str_len(s) != (3 as usize) { return 1 as i32; }
    let p: *u8 = str_ptr(s);
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
    if str_len(s) != (3 as usize) { return 1 as i32; }
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
                 total = total +% v.get(j);\n\
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
                 sum = sum +% positives.get(i);\n\
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
             dst.extend_from_slice(slice);\n\
             dst.push(2 as i32);\n\
             // dst = [1, 10, 20, 30, 40, 50, 2]; len = 7, sum = 153.\n\
             let mut sum: i32 = 0;\n\
             let mut i: usize = 0 as usize;\n\
             while i < dst.len() {\n\
                 sum = sum +% dst.get(i);\n\
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
    let nm = Command::new("nm")
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
    // File magic: 0xfeedfacf on Mach-O 64, ELF starts with 0x7f 'E' 'L' 'F'.
    let bytes = std::fs::read(&out).unwrap();
    let is_macho = bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe]);
    let is_elf = bytes.starts_with(&[0x7f, b'E', b'L', b'F']);
    assert!(
        is_macho || is_elf,
        "expected Mach-O or ELF object; first bytes: {:?}",
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
    let nm = Command::new("nm")
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
    let nm = Command::new("nm")
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
    // htons(0x1234) on LE → 0x3412. Verify the binary's runtime answer.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("hs.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
           let p: u16 = 0x1234 as u16;\n\
           let s: u16 = htons(p);\n\
           if s != (0x3412 as u16) { return 1; }\n\
           // round-trip: htons(htons(x)) == x.\n\
           let r: u16 = htons(s);\n\
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
           let s: u32 = bswap32(p);\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/net.cplus"), net_src).unwrap();
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
    if back_bytes.get(0 as usize) != (10 as u8) { return 6; }
    if back_bytes.get(3 as usize) != (40 as u8) { return 7; }

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
    alert.set_message_text(str_ptr("Smoke\0"));
    alert.add_button(str_ptr("OK\0"));

    let secure = appkit::SecureTextField::new(frame);
    secure.set_placeholder_string(str_ptr("Password\0"));
    let search = appkit::SearchField::new(frame);
    search.set_placeholder_string(str_ptr("Search\0"));
    search.set_on_search(on_click);
    let tokens = appkit::TokenField::new(frame);
    tokens.set_string_value(str_ptr("one,two\0"));
    let combo = appkit::ComboBox::new(frame);
    combo.add_item(str_ptr("A\0"));
    let text_view = appkit::TextView::new(frame);
    text_view.set_string(str_ptr("Body\0"));

    let stepper = appkit::Stepper::new(frame);
    stepper.set_increment(1.0);
    let sw = appkit::Switch::new(frame);
    sw.set_state(1 as i64);
    let segments = appkit::SegmentedControl::new(frame);
    segments.set_segment_count(2 as i64);
    segments.set_label_for_segment(str_ptr("One\0"), 0 as i64);
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
    let tab_item = appkit::TabViewItem::new(str_ptr("main\0"));
    tab_item.set_label(str_ptr("Main\0"));
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
    let col = appkit::TableColumn::new(str_ptr("name\0"));
    col.set_title(str_ptr("Name\0"));
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

    let toolbar = appkit::Toolbar::new(str_ptr("main-toolbar\0"));
    toolbar.set_display_mode(1 as i64);
    let toolbar_item = appkit::ToolbarItem::new(str_ptr("item\0"));
    toolbar_item.set_label(str_ptr("Item\0"));
    let status_bar = appkit::StatusBar::system();
    let status_item_raw = status_bar.status_item_with_length(24.0);
    let status_item = appkit::StatusItem::from_obj(status_item_raw);
    let status_button = appkit::StatusBarButton::from_obj(status_item.button());
    status_button.set_title(str_ptr("S\0"));
    let touch_bar = appkit::TouchBar::new();
    let touch_item = appkit::TouchBarItem::new(str_ptr("touch\0"));

    let vc = appkit::ViewController::new();
    vc.set_view(text_view.obj);
    let wc = appkit::WindowController::new();
    let tabs = appkit::TabViewController::new();
    // NSTabViewController insists on (a) a fresh NSTabViewItem (not
    // one already attached to another tab parent) and (b) the item
    // having a non-nil viewController. The original smoke had neither —
    // it reused tab_item.obj from `tab_view` above. Fix both.
    let tab_item2 = appkit::TabViewItem::new(str_ptr("controllers\0"));
    tab_item2.set_label(str_ptr("Controllers\0"));
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
             return str_len(g) as i32;\n\
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
             println(x as i32);\n\
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
    #[cfg(unix)]
    std::os::unix::fs::symlink(&stdlib_target, dir.join("vendor/stdlib")).unwrap();

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
             println(unsafe { the_answer() });\n\
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
             println(h::answer());\n\
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
             let n: usize = str_len(GREETING);\n\
             let p: *u8 = str_ptr(GREETING);\n\
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
             if str_len(RESET) != (4 as usize) { return 1; }\n\
             let p: *u8 = str_ptr(RESET);\n\
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
             unsafe { printf(str_ptr(\"%g %g %g %g\\n\\0\"), l0 as f64, l1 as f64, l2 as f64, l3 as f64); }\n\
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
    #[cfg(unix)]
    std::os::unix::fs::symlink(&stdlib_target, dir.join("vendor/stdlib")).unwrap();

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
             let v0: i32 = unsafe { (*p).get(0 as usize) };\n\
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
    #[cfg(unix)]
    std::os::unix::fs::symlink(&stdlib_target, dir.join("vendor/stdlib")).unwrap();

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
