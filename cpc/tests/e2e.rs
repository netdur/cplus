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

// Phase 3 slice 3B: wrapping operators `+% -% *%`

#[test]
fn wrap_arith_runs() {
    let out = compile_and_run("wrap_arith.cplus");
    assert!(out.status.success(), "exited {:?}", out.status);
    // 255u8 +% 1u8 = 0; 127i8 +% 1i8 = -128; 200u8 *% 2u8 = 144; 0u8 -% 1u8 = 255
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n-128\n144\n255\n");
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
    let status = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for use-after-move");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0335"), "expected E0335 in stderr, got: {stderr}");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for double-consume");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0335"), "expected E0335 in stderr, got: {stderr}");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure on bare `break`");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure on bare `continue`");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0353"), "expected E0353, got: {stderr}");
}

/// Phase 5 slice 5BC.4 — Rule E3 multi-parameter elision. This is the
/// design note's Phase-5 exit criterion: `fn longest(xs, ys) -> ...`
/// accepts under elided lifetimes, and moving either input while the
/// return-binding is alive fires E0372. The test moves the first input
/// (`a`); the symmetric "move `b`" case is covered by a unit test.
#[test]
fn longest_move_either_input_while_borrowed_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move-while-multi-source-borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
}

/// Phase 5 slice 5BC.3b: E0372 — moving a binding while a Rule-E1 /
/// Rule-E2 return-borrow of it is still live. The classic pattern is
/// `let r = passthrough(x); drain(move x);`. Rule E1 classifies
/// `passthrough` as returning a borrow of its only parameter; the
/// borrow checker records `r` as borrowing from `x`. When `drain(move x)`
/// runs, the borrow is still live → E0372.
#[test]
fn move_while_return_borrow_live_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move-while-borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
}

/// Phase 5 slice 5BC.2a: moving a non-Copy binding while shared-borrowing
/// it in another argument of the same call is E0370. The case below puts
/// the read-arg first and the `move`-arg second — sema's Phase-3 linear
/// move tracker accepts this (the read happens on an owned value before
/// the move consumes it), but the borrow checker rejects it at the
/// call-expression level: the shared borrow and the move are both alive
/// during the same call evaluation, which is the conflict pattern §3.1
/// of the design note catches.
#[test]
fn move_and_borrow_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn drain(n: i32, move b: B) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    drain(peek(y), y);
    return 0;
}
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move-and-borrow conflict");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0370"), "expected E0370, got: {stderr}");
}

#[test]
fn uninit_read_before_assign_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ua.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let x: i32; return x; }\n",
    )
    .unwrap();
    let bin = dir.join("ua");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure on read-before-assign");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0345"), "expected E0345 in stderr, got: {stderr}");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for non-exhaustive match");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0340"), "expected E0340 in stderr, got: {stderr}");
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
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n2\n200\n-2\n100\n-1\n");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure on irrefutable if-let");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0347"), "expected E0347 in stderr, got: {stderr}");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure on non-diverging guard-let else");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0348"), "expected E0348 in stderr, got: {stderr}");
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
    assert!(out.status.success(), "binary exited non-zero: {}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "49\n");
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
        "import \"math.cplus\" as math;\nfn main() -> i32 { return math::square(7); }\n",
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
        "import \"math.cplus\" as math;\nfn main() -> i32 { return math::square(7); }\n",
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
        "import \"geom.cplus\" as g;\nfn main() -> i32 { let p: g::Point = g::Point::new(1, 2); return p.y; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected E0403 from private-field read");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0403"), "expected E0403 in stderr, got: {stderr}");
    assert!(stderr.contains("private"), "expected diagnostic to mention 'private': {stderr}");
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
        "import \"geom.cplus\" as g;\nfn main() -> i32 { let p: g::Point = g::Point::new(3, 4); return p.x; }\n",
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
        "import \"geom.cplus\" as g;\nfn main() -> i32 { let p = g::Point { x: 1, y: 2 }; return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "expected E0403 from private-field bind");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0403"), "expected E0403 in stderr, got: {stderr}");
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
        .parent().unwrap().join("docs/examples/projects/calc");
    let manifest = std::fs::read_to_string(proj_root.join("Cplus.toml")).unwrap();
    std::fs::write(dir.join("Cplus.toml"), manifest).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for f in ["main.cplus", "eval.cplus", "util.cplus", "expr.cplus", "ops.cplus"] {
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
    assert!(out.status.success(), "binary exited non-zero: {}", out.status);
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
        "import \"missing.cplus\" as m;\nfn main() -> i32 { return 0; }\n",
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
    let line = stderr.lines().next().expect("expected at least one diagnostic line");
    let v: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stderr line not valid JSON: {e}\nline: {line}"));
    assert_eq!(v["severity"], "error");
    assert_eq!(v["code"], "E0401");
    let primary_file = v["primary"]["file"].as_str().expect("primary.file");
    assert!(primary_file.ends_with("main.cplus"),
        "primary file should be the importing file, got: {primary_file}");
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
        "import \"maths.cplus\" as m;\nfn main() -> i32 { return 0; }\n",
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
        "import \"nope.cplus\" as nope;\nfn main() -> i32 { return 0; }\n",
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
        "import \"b.cplus\" as b;\nfn from_a() -> i32 { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/b.cplus"),
        "import \"a.cplus\" as a;\nfn from_b() -> i32 { return 2; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"a.cplus\" as a;\nfn main() -> i32 { return 0; }\n",
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
    child.stdin.as_mut().unwrap()
        .write_all(b"fn  f( x:i32 )->i32{return x+1;}\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "fn f(x: i32) -> i32 { return x + 1; }\n");
}

/// `cpc fmt --check PATH/` over the in-tree samples must succeed with
/// no diff. This is the load-bearing test: the samples are the
/// formatter's de facto spec.
#[test]
fn fmt_check_all_samples_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().join("docs/examples");
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
    assert!(!out.status.success(), "expected non-zero exit on dirty file");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("bad.cplus"), "expected file path in diff header, got: {stderr}");
    assert!(stderr.contains("-fn"), "expected `-` lines in diff, got: {stderr}");
    assert!(stderr.contains("+fn"), "expected `+` lines in diff, got: {stderr}");
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
    assert_eq!(String::from_utf8_lossy(&out.stdout), "fn main() -> i32 { return 0; }\n");
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
    let once = Command::new(cpc).arg("fmt").arg(&f).status().expect("invoke");
    assert!(once.success());
    let first = std::fs::read_to_string(&f).unwrap();
    let twice = Command::new(cpc).arg("fmt").arg(&f).status().expect("invoke");
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
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(compile.status.success(), "compile failed: {}", String::from_utf8_lossy(&compile.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
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
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(compile.status.success(), "compile failed: {}", String::from_utf8_lossy(&compile.stderr));
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
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("prog");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(compile.status.success(), "compile failed: {}", String::from_utf8_lossy(&compile.stderr));
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
    std::fs::write(&src, "#[tset]\nfn f() { return; }\nfn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for unknown attribute");
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
    std::fs::write(&src, "#[test]\nstruct P { v: i32 }\nfn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for misplaced #[test]");
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
    std::fs::write(&src, "#[test] fn t(n: i32) { return; }\nfn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for bad test signature");
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
    std::fs::write(&src, "#[test] pub fn t() { return; }\nfn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for pub on #[test]");
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
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(compile.status.success(),
        "expected clean compile, stderr: {}", String::from_utf8_lossy(&compile.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
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
    std::fs::write(&src, "fn main() -> i32 {\n  assert 1 == 2;\n  println(999);\n  return 0;\n}\n").unwrap();
    let bin = dir.join("bad");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(compile.status.success(),
        "expected clean compile, stderr: {}", String::from_utf8_lossy(&compile.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert!(!run.status.success(), "expected non-zero exit on trap, got: {}", run.status);
    // The `println(999)` after the failing assertion must not have run.
    assert!(!String::from_utf8_lossy(&run.stdout).contains("999"),
        "code after failing assert ran: {:?}", run.stdout);
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected sema rejection of non-bool assert");
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
    std::fs::write(&src, "#[test]\nfn t1() { return; }\nfn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("prog");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected clean compile, stderr: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().expect("run produced binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected all-pass, stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout));
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected non-zero exit on failing test");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).arg("--json").output()
        .expect("invoke cpc");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 lines (2 tests + 1 summary): {stdout}");
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
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(), "no tests should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 passed; 0 failed"), "got stdout: {stdout}");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected pass, stderr: {}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected pass, stderr: {}", String::from_utf8_lossy(&out.stderr));
    // The driver should return the failed-count (0), not the user's 42.
    assert_eq!(out.status.code(), Some(0));
}

// ---- Phase 6 slice 6BC.1 — intra-call exclusive-borrow conflicts ----

#[test]
fn e0380_two_mut_borrows_of_same_binding_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn modify_both(mut a: B, mut b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    modify_both(y, y);
    return 0;
}
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for two mut borrows");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0380"), "expected E0380, got: {stderr}");
}

#[test]
fn e0381_mut_and_shared_borrow_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn write_thing(mut a: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    write_thing(y, peek(y));
    return 0;
}
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for mut+shared");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0381"), "expected E0381, got: {stderr}");
}

#[test]
fn e0382_mut_and_move_in_same_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn write_and_take(mut a: B, move b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    write_and_take(y, y);
    return 0;
}
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for mut+move");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0382"), "expected E0382, got: {stderr}");
    // E0370 must NOT fire for the same pair — E0382 is the more specific
    // diagnostic and suppresses cascading errors.
    assert!(!stderr.contains("E0370"), "E0370 should be suppressed for mut+move pair, got: {stderr}");
}

#[test]
fn mut_borrows_of_different_bindings_accepted() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn modify_both(mut a: B, mut b: B) { return; }
fn main() -> i32 {
    let y: B = B { x: 1 };
    let z: B = B { x: 2 };
    modify_both(y, z);
    return 0;
}
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(), "two mut borrows of distinct places should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn mut_borrows_of_copy_type_accepted() {
    // `mut x: i32` is local-mutability on Copy types, not a borrow. Two
    // such args should compile without E0380 / E0381.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
fn modify_both(mut a: i32, mut b: i32) { return; }
fn main() -> i32 {
    let y: i32 = 1;
    modify_both(y, y);
    return 0;
}
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(), "Copy mut args should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
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
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bin");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(),
        "Phase-6 exit: iterator invalidation must reject; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0381"),
        "expected E0381 on iterator-invalidation; got: {stderr}");
}

#[test]
fn phase6_exit_sequential_pushes_accepted() {
    // Positive: pushes without an outstanding cursor compile fine.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("vec_ok.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bin");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "sequential pushes should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
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
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn main() -> i32 {
    let x: B = B { x: 7 };
    return x.x;
}
").unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(!ir.contains("%x.drop_flag"),
        "drop flag should be elided when binding is never moved; got: {ir}");
    // Direct unconditional drop call must still appear.
    assert!(ir.contains("call void @B.drop(ptr %x"),
        "expected unconditional drop call; got: {ir}");
}

#[test]
fn moved_drop_binding_keeps_runtime_flag() {
    // When a binding IS moved somewhere in the function, the
    // runtime flag mechanism stays — flag alloca, init store,
    // flip-on-move store, load-and-branch at scope exit.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn consume(move b: B) { return; }
fn main() -> i32 {
    let x: B = B { x: 7 };
    consume(x);
    return 0;
}
").unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("%x.drop_flag = alloca i1"),
        "drop flag alloca should remain for moved binding; got: {ir}");
    assert!(ir.contains("load i1, ptr %x.drop_flag"),
        "flag load should remain at scope exit; got: {ir}");
}

#[test]
fn never_moved_drop_runtime_behavior_unchanged() {
    // The Phase-3 drop_basic sample expects output `1\n2\n-2\n-1\n`.
    // Confirm that 6BC.opt's optimization doesn't change the runtime
    // behavior: the drop calls still fire in the right order.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_path = dir.join("drop_basic.cplus");
    let sample = format!("{}/../docs/examples/drop_basic.cplus", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(&sample, &src_path).expect("copy sample");
    let bin = dir.join("drop_basic");
    let compile = Command::new(cpc).arg(&src_path).arg("-o").arg(&bin)
        .status().expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, "1\n2\n-2\n-1\n",
        "drop_basic output changed after 6BC.opt optimization; got: {stdout:?}");
}

// ---- Phase 6 slice 6BC.codegen — noalias / readonly param attributes ----

#[test]
fn mut_param_tagged_noalias_in_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn bump(mut b: B) -> i32 { b.x = b.x + 1; return b.x; }
fn main() -> i32 {
    let mut v: B = B { x: 1 };
    return bump(v);
}
").unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected clean emit; stderr: {}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @bump(ptr noalias "),
        "expected `mut b: B` to lower to `ptr noalias`; got: {ir}");
}

#[test]
fn shared_param_tagged_readonly_in_ir() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn peek(b: B) -> i32 { return b.x; }
fn main() -> i32 {
    let v: B = B { x: 7 };
    return peek(v);
}
").unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected clean emit; stderr: {}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @peek(ptr readonly "),
        "expected shared `b: B` to lower to `ptr readonly`; got: {ir}");
    // And NOT `noalias` — shared borrows can alias per §2.9.
    assert!(!ir.contains("@peek(ptr noalias"),
        "shared borrow must not get `noalias`; got: {ir}");
}

#[test]
fn copy_struct_param_stays_by_value_no_attr() {
    // `mut p: Point` on a Copy struct is local-mutability, not a
    // borrow. Stays struct-by-value, no LLVM attribute.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "\
struct Point { x: i32, y: i32 }
fn shift(mut p: Point) -> i32 { p.x = p.x + 1; return p.x; }
fn main() -> i32 {
    let v: Point = Point { x: 1, y: 2 };
    return shift(v);
}
").unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @shift(%Point "),
        "Copy struct should stay struct-by-value; got: {ir}");
}

// ---- Phase 6 slice 6BC.5 — explicit `borrow REGION T` syntax ----

#[test]
fn borrow_region_annotation_compiles_and_links() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "annotated function should compile and link; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn borrow_region_annotation_establishes_multi_source_borrow() {
    // Verifies that the annotation flows through to call-site borrow
    // tracking: moving either source while the result is alive fires
    // E0372.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move-while-multi-borrowed");
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
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for read while exclusively borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn move_with_borrow_annotation_rejected_at_parse() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
fn take(move x: borrow A B) { return; }
fn main() -> i32 { return 0; }
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move+borrow");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Parser error — E0100 with text about region annotations.
    assert!(stderr.contains("E0100") || stderr.contains("borrow"),
        "expected parse error mentioning borrow, got: {stderr}");
}

#[test]
fn explicit_annotation_fixes_e0384() {
    // The original E0384 case (Phase 6 slice 6BC.4) becomes
    // compilable once the user adds explicit annotations.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: borrow A B, b: borrow A B) -> borrow A B {
    if a.x > 0 { return a; }
    return B { x: 0 };
}
fn main() -> i32 { return 0; }
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "explicit annotation should suppress E0384; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

// ---- Phase 6 slice 6BC.4 — Rule E3-mut + E0384 ----

#[test]
fn e3_mut_longest_pattern_compiles_cleanly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "E3-mut should admit the longest-mut pattern; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn e3_mut_move_of_either_source_while_borrowed_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move-while-multi-borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
    assert!(stderr.contains("exclusively borrowed"),
        "E0372 should report exclusive flavor under E3-mut; got: {stderr}");
}

#[test]
fn e0384_mixed_rooting_requires_annotation() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn merge(a: B, b: B) -> B {
    if a.x > 0 { return a; }
    return B { x: 0 };
}
fn main() -> i32 { return 0; }
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for ambiguous elision");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0384"), "expected E0384, got: {stderr}");
    assert!(stderr.contains("borrow REGION T"),
        "E0384 suggestion should reference `borrow REGION T`; got: {stderr}");
}

#[test]
fn e0384_does_not_fire_on_fresh_value_returns() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
struct B { x: i32 }
impl B { fn drop(mut self) { return; } }
fn fresh(a: B, b: B) -> B { return B { x: 0 }; }
fn main() -> i32 { return 0; }
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "always-fresh returns should not trigger E0384; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

// ---- Phase 6 slice 6BC.3 — partial-place activation ----

#[test]
fn disjoint_subfield_borrows_accepted_in_one_call() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "disjoint sub-places should admit; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn e0374_parent_and_subfield_in_one_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for parent+sub-place");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0374"), "expected E0374, got: {stderr}");
}

#[test]
fn e0374_cross_statement_subfield_borrow_blocks_parent_read() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for read of parent while sub-place borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0374"), "expected E0374, got: {stderr}");
}

#[test]
fn disjoint_subfield_cross_statement_accepted() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "disjoint sub-places should admit cross-statement; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

// ---- Phase 6 slice 6BC.2 — cross-statement exclusive-borrow tracking ----

#[test]
fn e0383_read_of_exclusively_borrowed_place_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for read of exclusively-borrowed place");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn e0383_does_not_fire_when_borrower_consumed_first() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "moving the exclusive borrower should release the borrow; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn e0372_message_refined_when_borrow_is_exclusive() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for move while exclusively borrowed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
    assert!(stderr.contains("exclusively borrowed"),
        "E0372 should report 'exclusively borrowed' for the mut-borrow case; got: {stderr}");
    // E0383 must NOT fire for the same conflict.
    assert!(!stderr.contains("E0383"),
        "E0383 should be suppressed for move-while-exclusive; got: {stderr}");
}

#[test]
fn e2_mut_method_call_establishes_exclusive_borrow() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for read while mut-self method's return is alive");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0383"), "expected E0383, got: {stderr}");
}

#[test]
fn reading_the_exclusive_borrower_itself_accepted() {
    // Reading the borrower itself is fine — it owns the borrow.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(&src, "\
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
").unwrap();
    let bin = dir.join("good");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "reading the borrower itself should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expected pass, stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DOC_TEST::helper::0 ... ok"), "got: {stdout}");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected failing exit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DOC_TEST::bad::0 ... FAILED"), "got: {stdout}");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout));
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("test").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 passed; 0 failed"),
        "no tests should be discovered, got: {stdout}");
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
    ).unwrap();
    let bin = dir.join("prog");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "build with doctests failed: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "phase 7 syntax should sema-clean: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "missing method should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0503"), "expected E0503 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("p7gen5");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic fn should build cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "identity(7) + identity(35) should yield 42");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "build failed: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7tb");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "turbofish call should build cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "identity::[i32](7) + identity::[i32](35) should yield 42");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "arity mismatch should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0501"), "expected E0501 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("p7c");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic struct should build cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "use_int(Pair{{10,20}}) + use_mixed(Pair{{true,12}}) = 30 + 12 = 42");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "build failed: stderr={}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("%Pair__i32__i32"), "missing %Pair__i32__i32 in IR: {ir}");
    assert!(ir.contains("%Pair__bool__i32"), "missing %Pair__bool__i32 in IR: {ir}");
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
    ).unwrap();
    let bin = dir.join("p7d");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Option[T] should build cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7e_genimpl_mut");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "mut-self generic-typed impl should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Phase-7 exit demo should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p10a");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "extern fn abs should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg(&src).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "extern fn should emit IR cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("declare i32 @abs(i32)"),
        "expected `declare i32 @abs(i32)`, got IR:\n{ir}");
    assert!(!ir.contains("define i32 @abs("),
        "extern fn must not emit a body, got IR:\n{ir}");
    // Call site uses the literal symbol name (no module prefix).
    assert!(ir.contains("call i32 @abs(i32"),
        "expected call to literal `@abs`, got IR:\n{ir}");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Phase-10 exit demo should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p10rc");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "#[repr(C)] struct should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p10va");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "varargs printf should build: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(12), "printf returns bytes written = 12");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "owned-string sample should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p10rt");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "pointer roundtrip should build: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "malloc + store + load roundtrips → 42");
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
    ).unwrap();
    let bin = dir.join("p10ia");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "pointer index+arith should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg(&src).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "raw pointer in extern signature should build: stderr={}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("declare i64 @strlen(ptr)"),
        "expected `declare i64 @strlen(ptr)`, got IR:\n{ir}");
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
    ).unwrap();
    let bin = dir.join("p8s");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "println(str) should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p8e");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "str equality should build: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected a==b && a!=c to take us to 42");
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
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Phase-8 FizzBuzz exit demo should build: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 15, "expected 15 lines, got {}: {:?}", lines.len(), lines);
    assert_eq!(lines[0], "1");
    assert_eq!(lines[2], "Fizz");      // i=3
    assert_eq!(lines[4], "Buzz");      // i=5
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
    ).unwrap();
    let bin = dir.join("p7e_bound");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "bound-satisfied call should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg(&src).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "bound violation should fail compilation");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0502"), "expected E0502 in stderr, got: {}", stderr);
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
    ).unwrap();
    let bin = dir.join("p7e_genimpl");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic-typed impl should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7e_meth");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic method with turbofish should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7e_assoc");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic assoc call with turbofish should build: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("p7e_unqual");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "unqualified Option pattern should build cleanly: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "Some(35) + None|7 = 42 (unqualified pattern)");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(),
        "build failed: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "Self outside impl/interface should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0508"), "expected E0508 in stderr: {stderr}");
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
    let emit = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(emit.status.success(), "cpc --emit-ll failed: {}", String::from_utf8_lossy(&emit.stderr));
    std::fs::write(&ll, &emit.stdout).unwrap();
    // Link with Cocoa.
    let bin = dir.join("hello_appkit");
    let link = Command::new("clang")
        .arg(&ll)
        .arg("-framework").arg("Cocoa")
        .arg("-lobjc")
        .arg("-Wno-override-module")
        .arg("-o").arg(&bin)
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
    ).unwrap();
    let bin = dir.join("ptr_reinterpret");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "ptr-to-ptr reinterpret outside unsafe should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0801"), "expected E0801 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("if_usize");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(compile.success());
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(8));
}

// Phase 11 slice 11.FN_PTR: function pointer types and values.

#[test]
fn phase11_fn_pointer_demo_runs() {
    let out = compile_and_run("phase11_fn_pointers.cplus");
    // Exit 42 = handle_click(0) + handle_hover(0) = 35 + 7.
    assert_eq!(out.status.code(), Some(42), "phase11_fn_pointers should exit 42");
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
    ).unwrap();
    let bin = dir.join("fnptr_local");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let bin = dir.join("fnptr_struct");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let bin = dir.join("fnptr_atexit");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0302"), "expected E0302 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("int_to_ptr");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "0 as *u8 outside unsafe should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0801"), "expected E0801 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("link_name_abs");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(), "compile should succeed");
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("declare i32 @abs("), "expected `declare i32 @abs(...)` in IR: {ir}");
    assert!(ir.contains("@abs(i32"), "expected call to use `@abs` not `@my_abs`: {ir}");
    assert!(!ir.contains("@my_abs"), "should NOT emit `@my_abs` anywhere: {ir}");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(out.status.success(), "two link_name aliases for same symbol should compile");
    let ir = String::from_utf8_lossy(&out.stdout);
    let declare_count = ir.matches("declare i32 @abs(").count();
    assert_eq!(declare_count, 1, "expected exactly one `declare @abs`, got {declare_count}: {ir}");
    // And the binary still runs.
    let bin = dir.join("link_name_dedup");
    let _ = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(42), "abs(-7) + abs(-35) should be 42");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "link_name on non-extern fn should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0356"), "expected E0356 in stderr: {stderr}");
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
    assert_eq!(out.status.code(), Some(8), "exit code should be size_of[Point] = 8");
}

#[test]
fn phase11_size_of_inside_generic_fn_runs() {
    // size_of::[T]() inside a generic fn body — monomorphize must substitute
    // T to the concrete type via subst_type_ast in the call's type_args, or
    // codegen panics on Ty::Param. This pins that substitution.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("size_of_generic.cplus");
    std::fs::write(
        &src,
        "fn typed_size[T]() -> usize { return size_of::[T](); }\n\
         fn main() -> i32 { let n: usize = typed_size::[i32](); return n as i32; }\n",
    ).unwrap();
    let bin = dir.join("size_of_generic");
    let compile = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(compile.success(), "size_of inside generic fn should compile cleanly");
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(4), "typed_size::[i32]() should return 4");
}

#[test]
fn phase11_size_of_no_type_arg_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad_size_of.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { let n: usize = size_of(); return 0; }\n",
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "size_of() with no type arg should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0501"), "expected E0501 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("g_ret");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic fn returning Box[T] should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("g_ret_inf");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic fn returning Box[T] via inference should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("g_nested");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "generic fn returning nested generic should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("g_assoc");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Box[i32]::new should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("g_assoc_multi");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Pair[i32,bool]::make should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
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
    let out = Command::new(cpc).arg(src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "Vec[T, A] sample should compile: stderr={}",
        String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(36),
        "Vec generic demo should exit with sum 1..=8 = 36; stdout={}",
        String::from_utf8_lossy(&run.stdout));
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
    ).unwrap();
    let bin = dir.join("alias_prim");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "type alias should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("alias_struct");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "struct alias should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("alias_chain");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "chained alias should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "cyclic alias should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0510"), "expected E0510 in stderr: {stderr}");
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "duplicate type definition should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0301"), "expected E0301 in stderr: {stderr}");
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
    ).unwrap();
    let bin = dir.join("alias_fn");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "alias in fn signature should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("s");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "string methods should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("ts");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "to_string should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("ip");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "interpolation should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("ipe");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "expr-inside-interp should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let bin = dir.join("dd");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "$$ escape should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src).output().expect("invoke cpc");
    assert!(!out.status.success(), "non-ToString type should reject");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0612"), "expected E0612 in stderr: {stderr}");
}

#[test]
fn phase8_interp_demo_sample_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = "/Users/adel/Workspace/C+/docs/examples/phase8_interpolation.cplus";
    let bin = dir.join("interp_demo");
    let out = Command::new(cpc).arg(src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(out.status.success(),
        "interpolation demo should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
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
    ).unwrap();
    let out = Command::new(cpc).arg("-g").arg("--emit-ll").arg(&src)
        .output().expect("invoke cpc");
    assert!(out.status.success(),
        "-g should emit IR: stderr={}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("!llvm.module.flags"), "missing module flags: {ir}");
    assert!(ir.contains("!DICompileUnit"), "missing DICompileUnit: {ir}");
    assert!(ir.contains("!DIFile"), "missing DIFile: {ir}");
    assert!(ir.contains("!DISubprogram(name: \"main\""),
        "missing DISubprogram for main: {ir}");
    assert!(ir.contains("!DISubprogram(name: \"helper\""),
        "missing DISubprogram for helper: {ir}");
    assert!(ir.contains("!DILocation"), "missing DILocation: {ir}");
    // define lines should reference !dbg.
    assert!(ir.contains("define i32 @main()") && ir.contains("!dbg "),
        "main define should carry !dbg: {ir}");
}

#[test]
fn phase11_debuginfo_g_binary_links() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("dbg_bin.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { return 42; }\n",
    ).unwrap();
    let bin = dir.join("dbg_bin");
    let out = Command::new(cpc).arg("-g").arg(&src).arg("-o").arg(&bin)
        .output().expect("invoke cpc");
    assert!(out.status.success(),
        "cpc -g should link the binary: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn phase11_debuginfo_off_by_default_no_di() {
    // Sanity: without -g, no DI metadata.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nodbg.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 { return 0; }\n",
    ).unwrap();
    let out = Command::new(cpc).arg("--emit-ll").arg(&src)
        .output().expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(!ir.contains("!DICompileUnit"), "DI should be absent without -g: {ir}");
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
    let out = Command::new(cpc).arg("--asan").arg("--emit-ll").arg(&src)
        .output().expect("invoke cpc");
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main() sanitize_address"),
        "main should carry sanitize_address attr: {ir}");
}

#[test]
fn phase11_ubsan_no_function_attr() {
    // UBSan doesn't gate on a function attribute; we just forward
    // -fsanitize=undefined to clang. Verify the IR is unchanged.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("u.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc).arg("--ubsan").arg("--emit-ll").arg(&src)
        .output().expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(!ir.contains("sanitize_"), "UBSan should not attach a sanitize_ attr: {ir}");
}

#[test]
fn phase11_sanitizer_exclusive_combo_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("x.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let bin = dir.join("x");
    let out = Command::new(cpc).arg("--asan").arg("--tsan")
        .arg(&src).arg("-o").arg(&bin)
        .output().expect("invoke cpc");
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
    ).unwrap();
    let bin = dir.join("oob");
    let out = Command::new(cpc).arg("--asan").arg(&src).arg("-o").arg(&bin)
        .output().expect("invoke cpc");
    assert!(out.status.success(),
        "asan build should compile: stderr={}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().expect("run binary");
    // ASan exits non-zero and prints "AddressSanitizer:" on stderr.
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("AddressSanitizer"),
        "ASan didn't fire on heap overflow; stderr={stderr}, status={:?}", run.status);
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
