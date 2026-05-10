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
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "hello, world\n");
    assert!(run.stderr.is_empty(), "unexpected stderr: {:?}", run.stderr);
}

#[test]
fn emit_ir_prints_module() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc).arg("--emit-ir").output().expect("invoke cpc");
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
    assert!(v["primary"]["file"].as_str().unwrap().ends_with("bad.cplus"));
    assert!(v["message"].as_str().unwrap().contains("non-chainable") || v["message"].as_str().unwrap().contains("comparison"));
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
    assert!(stderr.contains("error[E0100]"), "expected E0100 in stderr: {stderr}");
    assert!(stderr.contains("bad.cplus:"), "expected file path in stderr: {stderr}");
}

// ---- Phase 1 end-to-end: each sample program compiles, runs, prints expected output ----

fn compile_and_run(sample: &str) -> std::process::Output {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::copy(
        format!("{}/../docs/examples/{sample}", env!("CARGO_MANIFEST_DIR")),
        &src,
    ).expect("copy sample");
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
        "fn main() -> i32 { let xs: [i32; 3] = [1, 2, 3]; return xs[10 as usize]; }"
    ).unwrap();
    let bin = dir.join("oob");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run");
    assert!(!run.status.success(), "expected trap on out-of-bounds index");
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
    if release { cmd.arg("--release"); }
    let status = cmd.arg(&path).arg("-o").arg(&bin).status().expect("invoke cpc");
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
        run.status, String::from_utf8_lossy(&run.stderr)
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
    assert!(!result.status.success(), "expected sema failure to fail compilation");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("E0305"), "expected E0305 (immutable assign), got: {stderr}");
}

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "cpc-test-{}-{}-{n}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
