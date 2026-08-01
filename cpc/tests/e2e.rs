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

/// bug 2026-07-12: an unsuffixed integer literal above i32 range, cast to a wider
/// unsigned type, wrapped through i32 first and then SIGN-extended —
/// `4294967295 as u64` yielded 0xFFFFFFFFFFFFFFFF instead of 0x00000000FFFFFFFF
/// (no diagnostic). `gen_cast` now materializes an unsuffixed int-literal operand
/// at the target width directly. The other columns pin the neighbours that must
/// NOT regress: a hex literal takes the same path, small values are unchanged, and
/// `300 as i8` still truncates to 44.
#[test]
fn unsuffixed_int_literal_cast_takes_target_width() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("litcast.cplus");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         fn main() -> i32 {\n\
         printf(#str_ptr(\"%llx %llx %llx %d\\n\\0\"), 4294967295 as u64, 0xFFFFFFFF as u64, 5 as u64, 300 as i8);\n\
         return 0;\n\
         }\n",
    )
    .expect("write litcast.cplus");
    let bin = dir.join("litcast");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(compile.success(), "cpc exited non-zero: {compile}");
    let run = Command::new(&bin).output().expect("run litcast");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "ffffffff ffffffff 5 44\n"
    );
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
        "static FREES: i32 = 0;\n\
         struct B { opaque data: *u8 }\n\
         impl B { fn drop(ref this) { { FREES = FREES + 1; } return; } }\n\
         fn work() {\n\
             var i: i32 = 0;\n\
             while i < 3 { let b: B = B { data: { 0 as *u8 } }; i = i + 1; }\n\
             for j in 0..2 { let c: B = B { data: { 0 as *u8 } }; }\n\
             var k: i32 = 0;\n\
             loop { let d: B = B { data: { 0 as *u8 } }; if k == 1 { break; } k = k + 1; }\n\
             return;\n\
         }\n\
         fn main() -> i32 { work(); return { FREES }; }\n",
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

// v0.0.26: owned Drop *temporaries* (an unbound expression-statement value, a
// borrowing method's rvalue receiver, a non-`take` by-value arg) are dropped at
// the end of their enclosing statement — previously they leaked (only named
// bindings dropped). Guards the dangerous edges together: a `take` arg is
// consumed by the callee (dropped once, never double), a named-local borrow is
// NOT temp-dropped, and a loop-body temp drops each iteration. The static count
// is returned as the exit code.
#[test]
fn owned_temporaries_drop_at_end_of_statement() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("tempdrop.cplus");
    std::fs::write(
        &src,
        "static DROPS: i32 = 0;\n\
         struct R { id: i32 }\n\
         impl R {\n\
             fn from_raw(v: i32) -> R { return R { id: v }; }\n\
             fn use_it(this) -> i32 { return this.id; }\n\
             fn drop(ref this) { { DROPS = DROPS + 1; } return; }\n\
         }\n\
         fn lend(r: R) -> i32 { return r.id; }\n\
         fn consume(take r: R) -> i32 { let n: i32 = r.id; return n; }\n\
         fn work() {\n\
             R::from_raw(1);                        // (a) bare expr-stmt temp -> 1\n\
             let a: i32 = R::from_raw(2).use_it();  // (b) temp receiver (borrow) -> 1\n\
             let b: i32 = lend(R::from_raw(3));     // (c) temp borrow arg -> 1\n\
             let c: i32 = consume(R::from_raw(4));  // (d) temp take arg: callee drops once -> 1\n\
             var n: R = R::from_raw(5);             // named local\n\
             let d: i32 = lend(n);                  // named-local borrow: NOT temp-dropped\n\
             var i: i32 = 0;\n\
             while i < 3 { let t: i32 = lend(R::from_raw(9)); i = i + 1; } // loop temp -> 3\n\
             return;                                // named local n drops at scope end -> 1\n\
         }\n\
         fn main() -> i32 { work(); return { DROPS }; }\n",
    )
    .unwrap();
    let bin = dir.join("tempdrop");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "temp-drop program must compile");
    let run = Command::new(&bin).status().expect("run tempdrop");
    // a=1, b=1, c=1, d=1, loop=3, named-local n=1  ⇒  8 drops total, each exactly once.
    assert_eq!(
        run.code(),
        Some(8),
        "owned temporaries must drop exactly once at end of statement; got {:?}",
        run.code()
    );
}

// v0.0.19: a narrowing-literal cast (`<numeric literal> as T`) is accepted in
// `static` initializer position and produces the same value the runtime cast
// would. Compile a program whose statics use the cast form, then read them back
// and return a value derived from both to prove the globals hold the right bits.
#[test]
fn static_narrowing_literal_cast_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("statcast.cplus");
    std::fs::write(
        &src,
        "static X: i8 = 5 as i8;\n\
         static Y: i16 = -3 as i16;\n\
         fn main() -> i32 { let d: i32 = { (X as i32) - (Y as i32) }; return d; }\n",
    )
    .unwrap();
    let bin = dir.join("statcast");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        status.success(),
        "static narrowing-cast program must compile"
    );
    let run = Command::new(&bin).status().expect("run statcast");
    // 5 - (-3) = 8.
    assert_eq!(run.code(), Some(8), "got {:?}", run.code());
}

#[test]
fn static_pointer_init_runs() {
    // A pointer-typed `static` initialized from an integer literal must emit an
    // LLVM pointer constant — `null` for 0, `inttoptr` otherwise — not a bare
    // `ptr 0` (which fails to assemble: "integer constant must have integer
    // type"). Regression for the `static p: *u8 = 0 as *u8;` codegen bug.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("statptr.cplus");
    std::fs::write(
        &src,
        "static NULLP: *u8 = 0 as *u8;\n\
         static ADDR: *u8 = 4096 as *u8;\n\
         fn main() -> i32 {\n\
             var r: i32 = 0;\n\
             if NULLP == { 0 as *u8 } { r = r + 1; }\n\
             if ADDR == { 4096 as *u8 } { r = r + 2; }\n\
             return r;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("statptr");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "pointer-typed static program must compile");
    let run = Command::new(&bin).status().expect("run statptr");
    // Both branches taken: 1 + 2 = 3.
    assert_eq!(run.code(), Some(3), "got {:?}", run.code());
}

/// A `match` consumes its owned scrutinee, so it must tear it down exactly once
/// regardless of arm shape. Catch-all (`x =>`) and wildcard (`_ =>`) arms used
/// to leak the consumed enum (and its Drop payload); a *temporary* scrutinee
/// (`match f() { ... }`) leaked in every arm kind. The fix drops the bound enum
/// (catch-all) / the scrutinee value (wildcard) / registers the payload
/// (variant), for both an owned binding and an owned temporary — while a moved
/// payload isn't double-dropped and a borrowed-place scrutinee is left to its
/// owner. Also covers wildcard *payload* positions (`E::A(_)`, `Pair(r, _)`),
/// which discard an owning payload and otherwise leaked it. Each phase leaves
/// the drop counter at its expected value (1, or 2 for the two-payload enum);
/// ASan-clean.
#[test]
fn match_consumes_owned_scrutinee_exactly_once() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("m.cplus");
    std::fs::write(
        &src,
        "static DROPS: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPS = DROPS + 1; } return; } }\n\
         enum E { A(R), B }\n\
         enum P { Pair(R, R), None }\n\
         fn mke() -> E { return E::A(R { data: { 0 as *u8 } }); }\n\
         fn mkp() -> P { return P::Pair(R { data: { 0 as *u8 } }, R { data: { 0 as *u8 } }); }\n\
         fn consume(take r: R) -> i32 { return 0; }\n\
         struct H { e: E }\n\
         impl H { fn drop(ref this) { return; } }\n\
         fn p_bind() { let e: E = mke(); let _n: i32 = match e { x => 7 }; return; }\n\
         fn p_wild() { let e: E = mke(); let _n: i32 = match e { _ => 7 }; return; }\n\
         fn p_temp_var() { let _n: i32 = match mke() { E::A(r) => 7, E::B => 0 }; return; }\n\
         fn p_temp_moved() { let _n: i32 = match mke() { E::A(r) => consume(r), E::B => 0 }; return; }\n\
         fn p_field() { let h: H = H { e: mke() }; let _n: i32 = match h.e { _ => 7 }; return; }\n\
         fn p_wc_payload() { let e: E = mke(); let _n: i32 = match e { E::A(_) => 7, E::B => 0 }; return; }\n\
         fn p_wc_temp() { let _n: i32 = match mke() { E::A(_) => 7, E::B => 0 }; return; }\n\
         fn p_pair_mixed() { let p: P = mkp(); let _n: i32 = match p { P::Pair(r, _) => 7, P::None => 0 }; return; }\n\
         fn p_pair_moved() { let p: P = mkp(); let _n: i32 = match p { P::Pair(r, _) => consume(r), P::None => 0 }; return; }\n\
         fn main() -> i32 {\n\
             p_bind();      if { DROPS } != 1 { return 1; } { DROPS = 0; }\n\
             p_wild();      if { DROPS } != 1 { return 2; } { DROPS = 0; }\n\
             p_temp_var();  if { DROPS } != 1 { return 3; } { DROPS = 0; }\n\
             p_temp_moved();if { DROPS } != 1 { return 4; } { DROPS = 0; }\n\
             p_field();     if { DROPS } != 1 { return 5; } { DROPS = 0; }\n\
             p_wc_payload();if { DROPS } != 1 { return 6; } { DROPS = 0; }\n\
             p_wc_temp();   if { DROPS } != 1 { return 7; } { DROPS = 0; }\n\
             p_pair_mixed();if { DROPS } != 2 { return 8; } { DROPS = 0; }\n\
             p_pair_moved();if { DROPS } != 2 { return 9; } { DROPS = 0; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let bin = dir.join("m");
        let mut cmd = Command::new(cpc);
        cmd.arg(&src).arg("-o").arg(&bin);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(&bin).output().expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged match scrutinee teardown ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "every match arm must drop the owned scrutinee exactly once; failing phase = exit code ({sanitizer})"
        );
    }
}

/// bugs/moved-from-local-is-readable-and-double-frees.md, runtime half: a
/// `match` that binds NOTHING is a pure discriminant read, so it must leave
/// the owned binding intact — still droppable at scope end, still matchable
/// again. This is what keeps the presence-check idiom expressible now that
/// re-reading a *consumed* binding is E0335 (compile-time half below).
///
/// Every probe allocates exactly one droppable payload, so every expected
/// count is 1: more means the value was torn down twice, fewer means it
/// leaked.
#[test]
fn presence_check_match_does_not_consume_the_binding() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("pc.cplus");
    std::fs::write(
        &src,
        "static DROPS: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPS = DROPS + 1; } return; } }\n\
         enum E { A(R), B }\n\
         struct H { e: E }\n\
         impl H { fn drop(ref this) { return; } }\n\
         fn mke() -> E { return E::A(R { data: { 0 as *u8 } }); }\n\
         fn consume(take r: R) -> i32 { return 0; }\n\
         fn p_check_then_scope_end() { let e: E = mke(); match e { E::A(_) => {} E::B => {} } return; }\n\
         fn p_check_then_match() { let e: E = mke(); match e { E::A(_) => {} E::B => {} } let _n: i32 = match e { E::A(r) => consume(r), E::B => 0 }; return; }\n\
         fn p_check_thrice_then_match() { let e: E = mke();\n\
             match e { E::A(_) => {} E::B => {} }\n\
             match e { E::A(_) => {} E::B => {} }\n\
             match e { E::A(_) => {} E::B => {} }\n\
             let _n: i32 = match e { E::A(r) => consume(r), E::B => 0 }; return; }\n\
         fn p_wild_then_match() { let e: E = mke(); match e { _ => {} } let _n: i32 = match e { E::A(r) => consume(r), E::B => 0 }; return; }\n\
         fn p_check_then_bound_payload_unmoved() { let e: E = mke(); match e { E::A(_) => {} E::B => {} } match e { E::A(r) => {} E::B => {} } return; }\n\
         fn p_field_checked_twice() { let h: H = H { e: mke() }; match h.e { E::A(_) => {} E::B => {} } match h.e { E::A(_) => {} E::B => {} } return; }\n\
         fn p_temp_binding_nothing_still_drops() { match mke() { E::A(_) => {} E::B => {} } return; }\n\
         fn main() -> i32 {\n\
             p_check_then_scope_end();          if { DROPS } != 1 { return 1; } { DROPS = 0; }\n\
             p_check_then_match();              if { DROPS } != 1 { return 2; } { DROPS = 0; }\n\
             p_check_thrice_then_match();       if { DROPS } != 1 { return 3; } { DROPS = 0; }\n\
             p_wild_then_match();               if { DROPS } != 1 { return 4; } { DROPS = 0; }\n\
             p_check_then_bound_payload_unmoved(); if { DROPS } != 1 { return 5; } { DROPS = 0; }\n\
             p_field_checked_twice();           if { DROPS } != 1 { return 6; } { DROPS = 0; }\n\
             p_temp_binding_nothing_still_drops(); if { DROPS } != 1 { return 7; } { DROPS = 0; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let bin = dir.join("pc");
        let mut cmd = Command::new(cpc);
        cmd.arg(&src).arg("-o").arg(&bin);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(&bin).output().expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged a non-consuming match ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "a name-binding-free match must leave the scrutinee intact and drop it exactly once; failing phase = exit code ({sanitizer})"
        );
    }
}

/// bugs/moved-from-local-is-readable-and-double-frees.md, compile-time half:
/// a `match` that binds a name consumes an owned Drop-enum binding, so reading
/// that binding afterwards is E0335. This program used to compile and then
/// use-after-free, then double-free, at run time.
#[test]
fn rematching_a_consumed_binding_rejected_e0335() {
    assert_compile_fails_with(
        "struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         enum E { A(R), B }\n\
         fn mke() -> E { return E::A(R { data: { 0 as *u8 } }); }\n\
         fn main() -> i32 {\n\
           let e: E = mke();\n\
           match e { E::A(_v) => {} E::B => { return 1; } }\n\
           match e { E::A(w) => { let _k: R = w; } E::B => {} }\n\
           return 0;\n\
         }\n",
        "E0335",
    );
}

/// The vendor/sqlite shape. `guard let` desugars to a binding match, so its
/// else block cannot re-match the same local to reach the complement payload —
/// the payload's destructor has already run by the time the else body starts.
/// Use the `else |Pat|` complement binding instead (pinned to still work by
/// `guard_let_complement_binding_reaches_the_payload`).
#[test]
fn guard_let_else_cannot_rematch_the_scrutinee() {
    assert_compile_fails_with(
        "struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         enum E { A(R), B }\n\
         fn mke() -> E { return E::A(R { data: { 0 as *u8 } }); }\n\
         fn main() -> i32 {\n\
           let e: E = mke();\n\
           guard let E::A(v) = e else {\n\
             match e { E::A(_) => { return 1; } E::B => { return 2; } }\n\
           };\n\
           let _k: R = v;\n\
           return 0;\n\
         }\n",
        "E0335",
    );
}

/// The sanctioned replacement for the rejected re-match above: `else |Pat|`
/// binds the complement payload directly, so it is still live in the else
/// block and drops exactly once.
#[test]
fn guard_let_complement_binding_reaches_the_payload() {
    let out = compile_and_run_src(
        "guardcomp",
        "static DROPS: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPS = DROPS + 1; } return; } }\n\
         enum E { A(R), B(R) }\n\
         fn mkb() -> E { return E::B(R { data: { 0 as *u8 } }); }\n\
         fn probe() -> i32 {\n\
           let e: E = mkb();\n\
           guard let E::A(v) = e else |E::B(bad)| {\n\
             let _held: R = bad;\n\
             return 7;\n\
           };\n\
           let _k: R = v;\n\
           return 0;\n\
         }\n\
         fn main() -> i32 {\n\
           if probe() != 7 { return 1; }\n\
           if { DROPS } != 1 { return 2; }\n\
           return 0;\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "complement-bound payload must be live and drop once; exit: {} stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.0.23 unified match ownership model: the paths the model *allows* (after
/// fixing the over-rejections that the model's first cut caused) must run clean
/// under ASan. Compile-time rejections (raw-deref of a Drop type, move-out of a
/// borrowed scrutinee) are covered by sema unit tests — they don't compile, so
/// can't be e2e'd. Here we lock in that the ALLOWED reads are sound:
///   - a *Copy* field of a Drop struct read via `(*p).f` (agent_core::identity);
///   - a non-Copy but *drop-free* POD copied out of `*p` (agent_core::events);
///   - a correct owned-match move-out drops the payload exactly once.
#[test]
fn match_model_allowed_reads_runtime_safe() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("m.cplus");
    std::fs::write(
        &src,
        "static DROPS: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPS = DROPS + 1; } return; } }\n\
         enum Opt { Some(usize), None }\n\
         struct NodeView { id: R, parent: Opt }\n\
         struct PodS { a: usize }\n\
         enum Pod { X(PodS), Y }\n\
         enum E { A(R), B }\n\
         fn mke() -> E { return E::A(R { data: { 0 as *u8 } }); }\n\
         // Copy field of a Drop struct via raw deref → reads the Copy value, the\n\
         // struct still drops its R exactly once.\n\
         fn copy_field_via_deref() {\n\
             let nv: NodeView = NodeView { id: R { data: { 0 as *u8 } }, parent: Opt::Some(7 as usize) };\n\
             let p: *NodeView = { #addr_of(nv) };\n\
             let _parent: usize = match { (*p).parent } { Opt::Some(x) => x, Opt::None => 0 as usize };\n\
             return;\n\
         }\n\
         // Drop-free POD copied out of *p → harmless bit-copy, no destructor.\n\
         fn pod_via_deref() -> i32 {\n\
             let pd: Pod = Pod::X(PodS { a: 9 as usize });\n\
             let pp: *Pod = { #addr_of(pd) };\n\
             let out: Pod = match { *pp } { Pod::X(s) => Pod::X(s), Pod::Y => Pod::Y };\n\
             let v: usize = match out { Pod::X(s) => s.a, Pod::Y => 0 as usize };\n\
             return v as i32;\n\
         }\n\
         // Correct owned-match move-out: drops the R exactly once.\n\
         fn owned_move_once() { let e: E = mke(); let _r: R = match e { E::A(x) => x, E::B => R { data: { 0 as *u8 } } }; return; }\n\
         fn main() -> i32 {\n\
             copy_field_via_deref(); if { DROPS } != 1 { return 1; } { DROPS = 0; }\n\
             if pod_via_deref() != 9 { return 2; }\n\
             owned_move_once();      if { DROPS } != 1 { return 3; } { DROPS = 0; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let bin = dir.join("m");
        let mut cmd = Command::new(cpc);
        cmd.arg(&src).arg("-o").arg(&bin);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(&bin).output().expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged an allowed model read ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "model-allowed read mis-dropped; failing phase = exit code ({sanitizer})"
        );
    }
}

// v0.0.19: a polymorphic backend built on a user-defined interface bound
// compiles and runs — generic fn (inference + turbofish), a generic struct
// whose field is the bounded type, and a generic impl calling the interface
// method on that field. Returns a value derived from all three paths.
#[test]
fn interface_bound_generic_backend_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("backend.cplus");
    std::fs::write(
        &src,
        "interface Backend { fn flush(this) -> i32; }\n\
         struct Mac { fd: i32 }\n\
         impl Mac: Backend { fn flush(this) -> i32 { return this.fd; } }\n\
         struct App[B: Backend] { backend: B }\n\
         impl App[B: Backend] { fn run(this) -> i32 { return this.backend.flush(); } }\n\
         fn render[B: Backend](b: B) -> i32 { return b.flush(); }\n\
         fn main() -> i32 {\n\
             let viaturbo: i32 = render::[Mac](Mac { fd: 10 });\n\
             let a: App[Mac] = App[Mac] { backend: Mac { fd: 5 } };\n\
             return a.run() + viaturbo;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("backend");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "polymorphic backend must compile");
    let run = Command::new(&bin).status().expect("run backend");
    // a.run() = 5, render::[Mac] = 10 → 15.
    assert_eq!(run.code(), Some(15), "got {:?}", run.code());
}

// v0.0.19: the `__cplus_*` runtime/atomic builtins migrated to the `#` sigil.
// Exercise the migrated forms directly (no stdlib import): atomic load/store/
// fetch-add, a memory fence, and `#drop_in_place::[T]` — all end-to-end.
#[test]
fn cplus_intrinsic_sigil_forms_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("sig.cplus");
    std::fs::write(
        &src,
        "static DROPPED: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPPED = DROPPED + 1; } return; } }\n\
         fn main() -> i32 {\n\
             var x: i32 = 41;\n\
             let p: *i32 = { #addr_of(x) };\n\
             { #atomic_store_i32_seqcst(p, 7); }\n\
             let v: i32 = { #atomic_load_i32_seqcst(p) };\n\
             let old: i32 = { #atomic_fetch_add_i32_seqcst(p, 35) };\n\
             { #atomic_fence_seqcst(); }\n\
             var r: R = R { data: { 0 as *u8 } };\n\
             let rp: *R = { #addr_of(r) };\n\
             { #drop_in_place::[R](rp); }\n\
             return v + old + { DROPPED };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("sig");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "intrinsic-sigil program must compile");
    let run = Command::new(&bin).status().expect("run sig");
    // v=7, old=7 (value before the +35), DROPPED=1 → 15.
    assert_eq!(run.code(), Some(15), "got {:?}", run.code());
}

// v0.0.24 de-Rust: `#addr(p)` is the loud spelling of `p as usize`. Verify at
// runtime (compile + link + run) that the two produce the identical address.
#[test]
fn addr_intrinsic_matches_ptr_to_usize_cast() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("addr.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
            var x: i32 = 41;\n\
            let p: *i32 = { #addr_of(x) };\n\
            let via_intrinsic: usize = { #addr(p) };\n\
            let via_cast: usize = { p as usize };\n\
            if via_intrinsic == via_cast { return 0; }\n\
            return 1;\n\
        }\n",
    )
    .unwrap();
    let bin = dir.join("addr");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "#addr program must compile");
    let run = Command::new(&bin).status().expect("run addr");
    assert_eq!(
        run.code(),
        Some(0),
        "#addr(p) must equal `p as usize`; got {:?}",
        run.code()
    );
}

// v0.0.24 de-Rust: type-inferred struct literals `{ field: ... }`. The struct
// type is taken from the expected type at the use site (annotation / return /
// argument / nested field), so the type name need not be repeated. Verify at
// runtime that a value built through the inferred form behaves identically to
// the named form, across binding / return / argument / nested positions.
#[test]
fn inferred_struct_literal_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("inf.cplus");
    std::fs::write(
        &src,
        "struct Inner { a: i32 }\n\
         struct Outer { inner: Inner, k: i32 }\n\
         fn take_outer(o: Outer) -> i32 { return o.inner.a + o.k; }\n\
         fn make() -> Outer { return { inner: { a: 3 }, k: 4 }; }\n\
         fn main() -> i32 {\n\
            let o: Outer = { inner: { a: 7 }, k: 3 };\n\
            let s: i32 = take_outer({ inner: { a: 100 }, k: 1 });\n\
            let m: Outer = make();\n\
            return o.inner.a + o.k + s + m.inner.a + m.k;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("inf");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        status.success(),
        "inferred-struct-literal program must compile"
    );
    let run = Command::new(&bin).status().expect("run inf");
    // 7+3 (o) + 101 (s) + 3+4 (m) = 118.
    assert_eq!(run.code(), Some(118), "got {:?}", run.code());
}

// v0.0.24 de-Rust: an inferred literal against a GENERIC struct annotation
// (`let b: Box[i32] = { ... }`) must resolve to the same monomorphized struct
// the type annotation produces — sema records the mangled name, monomorphize
// rewrites the node to that `StructLit`. Regression guard for the
// sema-mangling / monomorphize-mangling alignment.
#[test]
fn inferred_struct_literal_generic_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("infgen.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { val: T }\n\
         fn main() -> i32 {\n\
            let b: Box[i32] = { val: 42 };\n\
            return b.val;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("infgen");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        status.success(),
        "generic inferred-literal program must compile"
    );
    let run = Command::new(&bin).status().expect("run infgen");
    assert_eq!(run.code(), Some(42), "got {:?}", run.code());
}

// v0.0.24 de-Rust: moving an owned (Drop) value into an inferred-literal field
// must disarm the source exactly like the named form — no double-free at
// scope exit. Move-tracking soundness is inherited because field checking
// delegates to `check_struct_lit`; this pins that it actually holds at runtime
// (run under the sanitizers the suite uses elsewhere would catch a double-free;
// here a clean exit code 5 is the observable).
#[test]
fn inferred_struct_literal_move_into_field_no_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("infmove.cplus");
    std::fs::write(
        &src,
        "struct Owned { p: i32 }\n\
         impl Owned { fn drop(ref this) { } }\n\
         struct Holder { o: Owned }\n\
         fn main() -> i32 {\n\
            let x: Owned = Owned { p: 5 };\n\
            let h: Holder = { o: x };\n\
            return h.o.p;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("infmove");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        status.success(),
        "move-into-inferred-field program must compile"
    );
    let run = Command::new(&bin).status().expect("run infmove");
    assert_eq!(run.code(), Some(5), "got {:?}", run.code());
}

// v0.0.24 de-Rust #9 (stage 1): the new binding/ownership spellings
// `var` / `ref this` / `take this` / `take x: T` compile and run as the
// dual-spellings of `let mut` / `mut this` / `move this` / `move x: T`. A
// mutable (`ref this`) receiver mutates in place; `take` transfers ownership.
// Result is stage-independent (it doesn't lean on Copy-vs-by-ref param
// semantics, which the later hard-switch stage changes).
#[test]
fn var_ref_take_spellings_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("vrt.cplus");
    std::fs::write(
        &src,
        "struct Acc { n: i32 }\n\
         impl Acc {\n\
            fn bump(ref this) { this.n = this.n +% 1; }\n\
            fn consume(take this) -> i32 { return this.n; }\n\
         }\n\
         fn combine(take a: Acc, take b: Acc) -> i32 {\n\
            return a.consume() + b.consume();\n\
         }\n\
         fn main() -> i32 {\n\
            var a: Acc = Acc { n: 20 };\n\
            a.bump();\n\
            var b: Acc = Acc { n: 21 };\n\
            return combine(a, b);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("vrt");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "var/ref/take program must compile");
    let run = Command::new(&bin).status().expect("run vrt");
    // a.bump() -> 21; combine moves a,b and sums their n: 21 + 21 = 42.
    assert_eq!(run.code(), Some(42), "got {:?}", run.code());
}

// v0.0.24 de-Rust #9 (stage 3c): a `ref` (by-reference) parameter writes back
// to the caller's value at runtime, and the caller's binding must be `var`.
// Confirms the by-pointer lowering end-to-end (the write reaches the caller).
#[test]
fn ref_param_writes_back_to_var_caller() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("rb.cplus");
    std::fs::write(
        &src,
        "struct Cell { n: i32 }\n\
         impl Cell { fn drop(ref this) { return; } }\n\
         fn add_one(ref c: Cell) { c.n = c.n +% 1; }\n\
         fn main() -> i32 {\n\
            var c: Cell = Cell { n: 41 };\n\
            add_one(c);\n\
            return c.n;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("rb");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        status.success(),
        "ref-param write-back program must compile"
    );
    let run = Command::new(&bin).status().expect("run rb");
    // add_one mutated c through the `ref` param: 41 -> 42.
    assert_eq!(run.code(), Some(42), "got {:?}", run.code());
}

// v0.0.20: the inferred (no-turbofish) companion to the test above. An
// inferred generic call has no AST type-args, so monomorphize resolves it
// via `call_monos` — which used to be keyed by a file-less `ByteSpan`. Two
// inferred `g::id(v)` calls at the SAME byte offset in different files (modA
// infers `id[i32]`, modB infers `id[i64]`) collided: modA's call picked up
// modB's `[i64]`, emitting `call i32 ... @id__i64(i32 ...)` — a type
// mismatch clang rejects. The fix keys `call_monos` by `(origin_file, span)`.
// modA/modB are byte-identical except `i32`<->`i64` and `fa`<->`fb` (equal
// lengths), so the calls share an offset; the program must build and return
// 2 (fa()=1 + fb()=1).
#[test]
fn monomorphize_inferred_same_offset_no_collision() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"infer_span\"\n\n[[bin]]\nname = \"infer_span\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/idlib.cplus"),
        "fn id[T](take x: T) -> T { return x; }\n",
    )
    .unwrap();
    let mod_a = "import \"./idlib\" as g;\n\
                 fn fa() -> i32 { let v: i32 = 1; return g::id(v); }\n";
    std::fs::write(dir.join("src/modA.cplus"), mod_a).unwrap();
    // Byte-identical except the 3-char type name and 2-char fn name → the
    // inferred `g::id(v)` calls share a byte offset.
    std::fs::write(
        dir.join("src/modB.cplus"),
        mod_a.replace("fa", "fb").replace("i32", "i64"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./modA\" as ma;\n\
         import \"./modB\" as mb;\n\
         fn main() -> i32 { return (ma::fa() +% (mb::fb() as i32)); }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "same-offset inferred build failed: {status}"
    );
    let run = Command::new(dir.join("target/debug/infer_span"))
        .status()
        .expect("run infer_span");
    assert_eq!(run.code(), Some(2), "got {:?}", run.code());
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

// v0.0.24 de-Rust #9 (stage 3): the retired keywords `let mut` / `mut x:` /
// `move x:` are rejected by the real cpc binary with a hint pointing at the
// new spelling (`var` / `ref` / `take`). Also: `var` is reserved as a binding
// name. Confirms the hard switch end-to-end, not just at the parser unit level.
#[test]
fn retired_keywords_rejected_with_hints() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let cases: &[(&str, &str, &str)] = &[
        (
            "lm.cplus",
            "fn main() -> i32 { let mut x: i32 = 0; return x; }",
            "var",
        ),
        (
            "mp.cplus",
            "fn f(mut x: i32) -> i32 { return x; }\nfn main() -> i32 { return f(1); }",
            "ref",
        ),
        (
            "mv.cplus",
            "fn f(move x: i32) -> i32 { return x; }\nfn main() -> i32 { return f(1); }",
            "take",
        ),
        (
            "vn.cplus",
            "fn main() -> i32 { let var: i32 = 0; return 0; }",
            "reserved",
        ),
    ];
    for (name, src, hint) in cases {
        let p = dir.join(name);
        std::fs::write(&p, src).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&p)
            .output()
            .expect("invoke cpc");
        assert!(
            !out.status.success(),
            "{name}: expected rejection, compiled clean"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(hint),
            "{name}: expected hint `{hint}` in diagnostic, got: {stderr}"
        );
    }
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
         static MUT_I32_TABLE: [i32; 16] = #zero::[[i32; 16]]();\n\
         static MUT_STRUCT:    S         = #zero::[S]();\n\
         extern fn c_set_table(idx: i32, val: i32);\n\
         extern fn c_set_struct(a: i32, b: i64);\n\
         fn main() -> i32 {\n\
             // initial: cpc-owned, both zero\n\
             let v0: i32 = { MUT_I32_TABLE[5] };\n\
             if v0 != (0 as i32) { return 1; }\n\
             if { MUT_STRUCT.a } != (0 as i32) { return 2; }\n\
             // C writes through extern decl, cpc reads same storage\n\
             { c_set_table(5 as i32, 42 as i32); }\n\
             { c_set_struct(7 as i32, 99 as i64); }\n\
             if { MUT_I32_TABLE[5] } != (42 as i32) { return 3; }\n\
             if { MUT_STRUCT.a } != (7 as i32) { return 4; }\n\
             if { MUT_STRUCT.b } != (99 as i64) { return 5; }\n\
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
             var i: i32 = 0;\n\
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
             { #asm(\"nop\"); }\n\
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
             var s: i64 = 0;\n\
             { #asm(\"add {s}, {a}, {b}\", s = out(reg) s, a = in(reg) a, b = in(reg) b); }\n\
             return s;\n\
         }\n\
         fn inc(x: i64) -> i64 {\n\
             var v: i64 = x;\n\
             { #asm(\"add {v}, {v}, #1\", v = inout(reg) v); }\n\
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
             { #asm(\"add x0, x0, x1\\nret\"); }\n\
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

// GAP 3 (v0.0.19): a lower-pass error (E0911 bad static initializer) in an
// imported file must render against THAT file in a multi-file build, not the
// entry file. Before `lower_multi`, the diagnostic pointed at the entry file.
#[test]
fn multi_file_static_init_error_points_at_imported_file_gap3() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // The bad static lives in lib.cplus; main.cplus is the entry and is clean.
    std::fs::write(
        dir.join("src/lib.cplus"),
        "static BAD: i32 = 1 + 2;\nfn ok() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./lib\" as lib;\nfn main() -> i32 { return lib::ok(); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--diagnostics=json")
        .arg("check")
        .arg(dir.join("src/main.cplus"))
        .output()
        .expect("invoke cpc check");
    assert!(!out.status.success(), "bad static must fail the build");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .find(|l| l.contains("E0911"))
        .expect("expected an E0911 diagnostic line");
    let v: serde_json::Value = serde_json::from_str(line).expect("diagnostic is JSON");
    assert_eq!(v["code"], "E0911");
    let file = v["primary"]["file"].as_str().unwrap_or("");
    assert!(
        file.ends_with("lib.cplus"),
        "E0911 must point at lib.cplus, got {file}"
    );
    assert_eq!(
        v["primary"]["start"]["line"], 1,
        "static is on line 1 of lib.cplus"
    );
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
        "fn real_fn() -> i32 { return 0; }\n\
         fn _hidden_fn() -> i32 { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/missing.cplus"),
        "import \"./lib\" as lib;\nfn main() -> i32 { return lib::nope(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/private.cplus"),
        "import \"./lib\" as lib;\nfn main() -> i32 { return lib::_hidden_fn(); }\n",
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
             var c: Chunk = #zero::[Chunk]();\n\
             if c.offset != (0 as usize) { return 1; }\n\
             if c.size   != (0 as usize) { return 2; }\n\
             c.size = 64 as usize;\n\
             if c.size != (64 as usize) { return 3; }\n\
             // *T.write_zeroed() — heap pointer, T-many bytes zeroed.\n\
             let p: *Chunk = { malloc(#size_of::[Chunk]()) as *Chunk };\n\
             { p.write_zeroed(); }\n\
             let d: Chunk = { *p };\n\
             if d.offset != (0 as usize) { return 4; }\n\
             if d.size   != (0 as usize) { return 5; }\n\
             { free(p as *u8); }\n\
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
    assert!(
        run.status.success(),
        "expected exit 0, got {:?}",
        run.status
    );
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
    )
    .unwrap();
    std::fs::write(
        &cplus_src,
        "#[repr(C)]\n\
         struct Big24 { a: i64, b: i64, c: i64 }\n\
         extern fn make_big(x: i64) -> Big24;\n\
         fn main() -> i32 {\n\
             let r: Big24 = { make_big(10 as i64) };\n\
             if r.a != (11 as i64) { return 1; }\n\
             if r.b != (12 as i64) { return 2; }\n\
             if r.c != (13 as i64) { return 3; }\n\
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
             let r8: i64 = { take_s8(v8) };\n\
             if r8 != (1 as i64) { return 1; }\n\
             let v16: S16 = S16 { a: 1 as i64, b: 2 as i64 };\n\
             let r16: i64 = { take_s16(v16) };\n\
             if r16 != (12 as i64) { return 2; }\n\
             let v24: S24 = S24 { a: 1 as usize, b: { 0 as *u8 }, c: true };\n\
             let r24: i64 = { take_s24(v24) };\n\
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
             let p: *u8 = { malloc(64 as usize) };\n\
             if p.is_null() { return 1; }\n\
             let nilp: *u8 = { 0 as *u8 };\n\
             if nilp.is_not_null() { return 2; }\n\
             if !nilp.is_null() { return 3; }\n\
             { free(p); }\n\
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
    assert!(
        run.status.success(),
        "is_null program returned non-zero: {:?}",
        run.status
    );
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
         fn use_hs(hs: *HashSet) -> i32 { return { (*hs).count }; }\n\
         fn main() -> i32 {\n\
             let g: Galloc = Galloc { id: 7, hash_set: HashSet { count: 99, capacity: 256 }, extra: 1000 as i64 };\n\
             let gp: *Galloc = { #addr_of(g) };\n\
             let hsp: *HashSet = { #addr_of((*gp).hash_set) };\n\
             let a: [i32; 4] = [10, 20, 30, 40];\n\
             let aip: *i32 = { #addr_of(a[2]) };\n\
             let third: i32 = { *aip };\n\
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
    assert!(
        run.status.success(),
        "expected exit 0, got {:?}",
        run.status
    );
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
    assert!(
        run.status.success(),
        "neg-literal program returned non-zero"
    );
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
         impl B { fn drop(ref this) {} fn consume(take this) -> i32 { return this.x; } }\n\
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

// ---- generic-fn-body soundness (previously generic bodies were unchecked) ----

/// Helper: compile `src` and assert it fails with `code` in stderr.
fn assert_compile_fails_with(src: &str, code: &str) {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let path = dir.join("g.cplus");
    std::fs::write(&path, src).unwrap();
    let out = Command::new(cpc)
        .arg(&path)
        .arg("-o")
        .arg(dir.join("g"))
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "expected compile failure ({code}) for:\n{src}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(code),
        "expected {code} in stderr, got: {stderr}"
    );
}

#[test]
fn generic_body_receiver_less_interface_call_rejected_e0327() {
    // `t.make()` where the interface method `make()` has no receiver: E0327,
    // not a codegen panic.
    assert_compile_fails_with(
        "struct P { x: i32 }\n\
         interface Maker { fn make() -> i32; }\n\
         impl P: Maker { fn make() -> i32 { return 7; } }\n\
         fn call_make[T: Maker](t: T) -> i32 { return t.make(); }\n\
         fn main() -> i32 { let p: P = P { x: 1 }; return call_make::[P](p); }\n",
        "E0327",
    );
}

#[test]
fn generic_body_use_after_move_rejected_e0335() {
    // Reusing a value after it was moved into a bound method's by-value arg:
    // E0335 (would otherwise double-free at run time).
    assert_compile_fails_with(
        "struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         struct P {}\n\
         interface Sink { fn sink(this, take r: R); }\n\
         impl P: Sink { fn sink(this, take r: R) { return; } }\n\
         fn use_twice[T: Sink](t: T) -> i32 {\n\
           let r: R = R { data: { 0 as *u8 } };\n\
           t.sink(r);\n\
           let y: R = r;\n\
           return 0;\n\
         }\n\
         fn main() -> i32 { let p: P = P {}; return use_twice::[P](p); }\n",
        "E0335",
    );
}

#[test]
fn generic_body_move_out_of_borrow_rejected_e0337() {
    // Moving a `borrow` parameter by value into a bound method's by-value arg:
    // E0337 (would otherwise double-free — both the callee and the owner drop).
    assert_compile_fails_with(
        "struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         struct P {}\n\
         interface Sink { fn sink(this, take r: R); }\n\
         impl P: Sink { fn sink(this, take r: R) { return; } }\n\
         fn steal[T: Sink](t: T, r: R) { t.sink(r); return; }\n\
         fn main() -> i32 {\n\
           let p: P = P {};\n\
           let r: R = R { data: { 0 as *u8 } };\n\
           steal::[P](p, r);\n\
           return 0;\n\
         }\n",
        "E0337",
    );
}

#[test]
fn assign_reassign_view_borrow_rejected_e0372() {
    // Memory-model audit (2026-07-22): a view assigned to an EXISTING local via
    // `=` (not `let`) must acquire the same borrow the `let` form does. Before
    // the fix, `s = b.view();` left `b` Owned with zero borrows, so `let b2 =
    // b;` moved it out from under the still-live view `s` — a safe-code
    // use-after-free the `let s: str = b.view();` form already rejected.
    assert_compile_fails_with(
        "struct Buf { opaque p: *u8 }\n\
         impl Buf {\n\
           fn drop(ref this) { return; }\n\
           fn view(this) -> str { return #str_from_raw_parts(this.p, 3 as usize); }\n\
         }\n\
         fn main() -> i32 {\n\
           var b: Buf = Buf { p: 0 as *u8 };\n\
           var s: str = \"x\";\n\
           s = b.view();\n\
           let b2: Buf = b;\n\
           return 0;\n\
         }\n",
        "E0372",
    );
}

#[test]
fn assign_reassign_view_then_release_lets_owner_move() {
    // The positive companion: reassigning the view binding to a non-borrowing
    // value RELEASES its borrow, so the owner can then move. This exercises the
    // reassignment's `drop_borrower` path — without release, this would falsely
    // report E0372.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("releas.cplus");
    std::fs::write(
        &src,
        "struct Buf { opaque p: *u8 }\n\
         impl Buf {\n\
           fn drop(ref this) { return; }\n\
           fn view(this) -> str { return #str_from_raw_parts(this.p, 3 as usize); }\n\
         }\n\
         fn main() -> i32 {\n\
           var b: Buf = Buf { p: 0 as *u8 };\n\
           var s: str = \"x\";\n\
           s = b.view();\n\
           s = \"y\";\n\
           let b2: Buf = b;\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("releas");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "reassigning the view away should release the borrow");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn generic_rawptr_deref_move_non_copy_rejected_e0337() {
    // Memory-model audit (2026-07-22): moving out of `*ptr` in a generic body
    // whose pointee is an unbounded `Ty::Param` was accepted (ty_carries_drop
    // reads false for Param), a double-free once instantiated with a Drop type.
    // The move must be rejected at the definition — the concrete `*Text` form
    // already is — unless `T: Copy` proves there is no destructor.
    assert_compile_fails_with(
        "fn extract[T](ptr: *T) -> T { return *ptr; }\n\
         fn main() -> i32 { return 0; }\n",
        "E0337",
    );
}

#[test]
fn generic_rawptr_deref_move_copy_bound_compiles_and_runs() {
    // The escape hatch: `T: Copy` makes moving out of `*ptr` provably safe (a
    // Copy pointee has no destructor), so the generic read compiles and runs.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("rdcopy.cplus");
    std::fs::write(
        &src,
        "fn extract[T: Copy](ptr: *T) -> T { return *ptr; }\n\
         fn main() -> i32 {\n\
           var n: i32 = 42;\n\
           let p: *i32 = #addr_of(n);\n\
           let m: i32 = extract::[i32](p);\n\
           return m - 42;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("rdcopy");
    let st = Command::new(cpc).arg(&src).arg("-o").arg(&bin).status().expect("invoke cpc");
    assert!(st.success(), "extract[T: Copy] should compile");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "extract::[i32] should read 42 back");
}

#[test]
fn generic_path_assoc_fn_through_bound_compiles_and_runs() {
    // `T::make()` — a receiver-less interface fn called through the bound, the
    // path form E0327 suggests. Must compile through monomorphization (the
    // segment `T` rewrites to the concrete type) and run.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("passoc.cplus");
    std::fs::write(
        &src,
        "struct P { x: i32 }\n\
         interface Maker { fn make() -> i32; }\n\
         impl P: Maker { fn make() -> i32 { return 7; } }\n\
         fn call_make[T: Maker]() -> i32 { return T::make(); }\n\
         fn main() -> i32 { return call_make::[P](); }\n",
    )
    .unwrap();
    let bin = dir.join("passoc");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for T::make() through bound");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(7), "expected exit 7 from call_make::[P]()");
}

#[test]
fn generic_bound_method_arity_mismatch_rejected_e0308() {
    // A bound method called with the wrong arg count is E0308, exactly like a
    // concrete call — the generic and concrete paths now share one checker, so
    // the generic path can't silently accept it (it used to).
    assert_compile_fails_with(
        "struct P { x: i32 }\n\
         interface Add { fn add(this, rhs: i32) -> i32; }\n\
         impl P: Add { fn add(this, rhs: i32) -> i32 { return this.x + rhs; } }\n\
         fn call_add[T: Add](t: T) -> i32 { return t.add(2, 3); }\n\
         fn main() -> i32 { let p: P = P { x: 4 }; return call_add::[P](p); }\n",
        "E0308",
    );
}

#[test]
fn generic_move_self_through_bound_on_borrow_rejected_e0337() {
    // `t.take()` where the bound interface method is `take(move self)` and `t`
    // is a `borrow` param: the receiver is moved out of the borrow (the caller
    // still drops it). Must be rejected (E0337), not compiled into a
    // double-free. Exercises the `move self` receiver path of the bound-method
    // checker, not just its args.
    assert_compile_fails_with(
        "interface Take { fn take(take this) -> i32; }\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         impl R: Take { fn take(take this) -> i32 { return 0; } }\n\
         fn steal[T: Take](t: T) -> i32 { return t.take(); }\n\
         fn main() -> i32 {\n\
           let r: R = R { data: { 0 as *u8 } };\n\
           return steal::[R](r);\n\
         }\n",
        "E0337",
    );
}

/// reports/bug-05: the borrowed-payload escape check sniffed the arm body for
/// a bare `Ident`, so `=> { x }` walked past it — sema said nothing and codegen
/// bit-copied the Drop payload out of a field the owner still drops, a
/// double-free. The check now peels value-transparent wrappers with
/// `collect_value_leaves`, the same peeler the consuming sites use.
#[test]
fn match_arm_block_body_cannot_escape_a_borrowed_drop_payload() {
    const PRELUDE: &str = "struct R { n: i64 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         enum Holder { Some(R), None }\n\
         struct Bag { h: Holder }\n";
    // Every wrapper shape around the escape is rejected, the bare one included.
    for body in ["x", "{ x }", "{ { x } }"] {
        assert_compile_fails_with(
            &format!(
                "{PRELUDE}fn peek(b: Bag) -> R {{\n\
                 let t: R = match b.h {{\n\
                 Holder::Some(x) => {body},\n\
                 Holder::None => R {{ n: 0 }},\n\
                 }};\n\
                 return t;\n\
                 }}\n\
                 fn main() -> i32 {{ return 0; }}\n"
            ),
            "E0337",
        );
    }
}

/// The other half of reports/bug-05: the shapes that must STAY legal. Reading
/// through the borrowed payload and constructing a fresh value are not escapes,
/// and without a `drop` there is no double-free to prevent — so the same
/// `=> { x }` body compiles.
#[test]
fn match_arm_block_body_still_allows_reads_and_fresh_values() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(
        &src,
        "struct D { n: i64 }\n\
         impl D { fn drop(ref this) { return; } }\n\
         enum HasDrop { Some(D), None }\n\
         struct DropBag { h: HasDrop }\n\
         struct P { n: i64 }\n\
         enum Plain { Some(P), None }\n\
         struct PlainBag { h: Plain }\n\
         fn read_through(b: DropBag) -> i64 {\n\
           return match b.h {\n\
             HasDrop::Some(x) => { x.n },\n\
             HasDrop::None => 0,\n\
           };\n\
         }\n\
         fn fresh_value(b: DropBag) -> D {\n\
           return match b.h {\n\
             HasDrop::Some(x) => { D { n: x.n } },\n\
             HasDrop::None => { D { n: 0 } },\n\
           };\n\
         }\n\
         fn no_drop_no_hazard(b: PlainBag) -> P {\n\
           return match b.h {\n\
             Plain::Some(x) => { x },\n\
             Plain::None => P { n: 0 },\n\
           };\n\
         }\n\
         fn main() -> i32 {\n\
           let d: DropBag = DropBag { h: HasDrop::Some(D { n: 7 }) };\n\
           let p: PlainBag = PlainBag { h: Plain::Some(P { n: 2 }) };\n\
           let a: i64 = read_through(d);\n\
           let c: P = no_drop_no_hazard(p);\n\
           return (a as i32) + (c.n as i32) - 9;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("ok");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "non-escaping arm bodies must stay legal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run ok");
    assert_eq!(run.code(), Some(0), "unexpected exit: {run}");
}

#[test]
fn fn_pointer_to_c_struct_by_value_c_abi() {
    // C-ABI unification: a fn-pointer to a real C function that takes a struct
    // BY VALUE must use the platform C ABI for the arg — a raw aggregate
    // segfaults (the reported bug). Covers a large struct (>16B → passed
    // indirectly) and an HFA float struct ({f64,f64} → FP registers). Links
    // against a clang-compiled C object: this is the ground-truth ABI check.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let c_src = dir.join("c_side.c");
    let cplus_src = dir.join("m.cplus");
    let c_obj = dir.join("c_side.o");
    let cplus_obj = dir.join("m.o");
    let bin = dir.join("fpc_abi");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         struct Big { int64_t a, b, c, d; };\n\
         int64_t c_sum(struct Big x) { return x.a + x.b + x.c + x.d; }\n\
         struct Pf { double x, y; };\n\
         double c_f(struct Pf p) { return p.x * 1000.0 + p.y; }\n",
    )
    .unwrap();
    std::fs::write(
        &cplus_src,
        "#[repr(C)] struct Big { a: i64, b: i64, c: i64, d: i64 }\n\
         #[repr(C)] struct Pf { x: f64, y: f64 }\n\
         extern fn c_sum(x: Big) -> i64;\n\
         extern fn c_f(p: Pf) -> f64;\n\
         fn main() -> i32 {\n\
             let f1: fn(Big) -> i64 = c_sum;\n\
             let b: Big = Big { a: 1 as i64, b: 2 as i64, c: 3 as i64, d: 4 as i64 };\n\
             if { f1(b) } != (10 as i64) { return 1; }\n\
             let f2: fn(Pf) -> f64 = c_f;\n\
             let p: Pf = Pf { x: 3.0, y: 4.0 };\n\
             if { f2(p) } != 3004.0 { return 2; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    assert!(
        Command::new("clang")
            .args(["-c", "-o"])
            .arg(&c_obj)
            .arg(&c_src)
            .status()
            .expect("clang")
            .success(),
        "clang -c failed"
    );
    assert!(
        Command::new(cpc)
            .arg("--emit-obj")
            .arg(&cplus_src)
            .arg("-o")
            .arg(&cplus_obj)
            .status()
            .expect("cpc")
            .success(),
        "cpc --emit-obj failed"
    );
    assert!(
        Command::new("clang")
            .arg(&cplus_obj)
            .arg(&c_obj)
            .arg("-o")
            .arg(&bin)
            .status()
            .expect("link")
            .success(),
        "link failed"
    );
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "fn-pointer to C struct-by-value used the wrong ABI (raw aggregate vs C ABI)"
    );
}

#[test]
fn fn_pointer_to_c_struct_return_c_abi() {
    // C-ABI unification (returns): a fn-pointer to a C function RETURNING a
    // struct by value must use the platform C ABI — large (>16B → sret), small
    // (≤16B → coerced register pair), and HFA float ({f64,f64} → FP registers).
    // A raw aggregate return segfaults. Ground-truth: links a clang C object.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let c_src = dir.join("c_side.c");
    let cplus_src = dir.join("m.cplus");
    let c_obj = dir.join("c_side.o");
    let cplus_obj = dir.join("m.o");
    let bin = dir.join("fpr_abi");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         struct Big { int64_t a, b, c, d; };\n\
         struct Big c_make(void) { struct Big r; r.a=10; r.b=20; r.c=30; r.d=40; return r; }\n\
         struct P16 { int64_t a, b; };\n\
         struct P16 c_p16(void) { struct P16 r; r.a=7; r.b=9; return r; }\n\
         struct Pf { double x, y; };\n\
         struct Pf c_pf(void) { struct Pf r; r.x=2.0; r.y=8.0; return r; }\n",
    )
    .unwrap();
    std::fs::write(
        &cplus_src,
        "#[repr(C)] struct Big { a: i64, b: i64, c: i64, d: i64 }\n\
         #[repr(C)] struct P16 { a: i64, b: i64 }\n\
         #[repr(C)] struct Pf { x: f64, y: f64 }\n\
         extern fn c_make() -> Big;\n\
         extern fn c_p16() -> P16;\n\
         extern fn c_pf() -> Pf;\n\
         fn main() -> i32 {\n\
             let f1: fn() -> Big = c_make;\n\
             let b: Big = { f1() };\n\
             if b.a != (10 as i64) { return 1; }\n\
             if b.d != (40 as i64) { return 2; }\n\
             let f2: fn() -> P16 = c_p16;\n\
             let p: P16 = { f2() };\n\
             if p.a != (7 as i64) { return 3; }\n\
             if p.b != (9 as i64) { return 4; }\n\
             let f3: fn() -> Pf = c_pf;\n\
             let q: Pf = { f3() };\n\
             if q.x != 2.0 { return 5; }\n\
             if q.y != 8.0 { return 6; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    assert!(
        Command::new("clang")
            .args(["-c", "-o"])
            .arg(&c_obj)
            .arg(&c_src)
            .status()
            .expect("clang")
            .success(),
        "clang -c failed"
    );
    assert!(
        Command::new(cpc)
            .arg("--emit-obj")
            .arg(&cplus_src)
            .arg("-o")
            .arg(&cplus_obj)
            .status()
            .expect("cpc")
            .success(),
        "cpc --emit-obj failed"
    );
    assert!(
        Command::new("clang")
            .arg(&cplus_obj)
            .arg(&c_obj)
            .arg("-o")
            .arg(&bin)
            .status()
            .expect("link")
            .success(),
        "link failed"
    );
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "fn-pointer to C struct-RETURN used the wrong ABI (raw aggregate vs C ABI sret/coerce)"
    );
}

#[test]
fn fn_pointer_call_moves_arg_no_double_free() {
    // A non-Copy value moved through a `fn(take R)` pointer (Ident-bound and
    // struct-field forms) is CONSUMED by the callee — the callee drops it once,
    // and the caller must give up ownership (no second drop). `tag` makes a
    // double-free observable: a single drop adds 7, a double adds 14. Expect
    // DROPS=7 + n(=1) = 8.
    for (label, run_body) in [
        (
            "ident",
            "let f: fn(take R) -> i32 = sink; let r: R = R { tag: 7 }; return f(r);",
        ),
        (
            "field",
            "let h: Handler = Handler { cb: sink }; let r: R = R { tag: 7 }; return h.cb(r);",
        ),
    ] {
        let cpc = env!("CARGO_BIN_EXE_cpc");
        let dir = tempdir();
        let src = dir.join("fnptr.cplus");
        std::fs::write(
            &src,
            format!(
                "static DROPS: i32 = 0;\n\
                 struct R {{ tag: i32 }}\n\
                 impl R {{ fn drop(ref this) {{ {{ DROPS = DROPS + this.tag; }}; return; }} }}\n\
                 fn sink(take r: R) -> i32 {{ return 1; }}\n\
                 struct Handler {{ cb: fn(take R) -> i32 }}\n\
                 fn run() -> i32 {{ {run_body} }}\n\
                 fn main() -> i32 {{ let n: i32 = run(); return {{ DROPS + n }}; }}\n"
            ),
        )
        .unwrap();
        let bin = dir.join("fnptr");
        let st = Command::new(cpc)
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .expect("invoke cpc");
        assert!(st.success(), "cpc build failed ({label})");
        let run = Command::new(&bin).status().expect("run");
        assert_eq!(
            run.code(),
            Some(8),
            "fn-pointer {label} call double-freed (expected 8 = DROPS 7 + n 1)"
        );
    }
}

#[test]
fn generic_body_copy_bound_reuse_compiles_and_runs() {
    // A `T: Copy` generic fn may reuse its value (bound-aware Copy); it must
    // compile through codegen and run with the expected value.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("copyparam.cplus");
    std::fs::write(
        &src,
        "fn pick[T: Copy](a: T, b: T) -> T { let c: T = a; return c; }\n\
         fn main() -> i32 { return pick::[i32](42, 0); }\n",
    )
    .unwrap();
    let bin = dir.join("copyparam");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for T: Copy reuse");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected exit 42 from pick::[i32]");
}

#[test]
fn fn_pointer_to_plain_fn_runs() {
    // After dropping `unsafe fn`, function pointers carry ordinary function
    // types and the indirect call runs: `f(6)` -> danger(6) -> 7.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("fnptr.cplus");
    std::fs::write(
        &src,
        "fn danger(x: i32) -> i32 { return x + 1; }\n\
         fn main() -> i32 {\n\
             let r: i32 = {\n\
                 let f: fn(i32) -> i32 = danger;\n\
                 f(6)\n\
             };\n\
             return r;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("fnptr");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "function pointer to plain fn should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "f(6) -> danger(6) -> 7");
}

#[test]
fn move_param_use_after_call_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("uam.cplus");
    std::fs::write(
        &src,
        "struct B { x: i32 }\n\
         impl B { fn drop(ref this) {} }\n\
         fn take(take b: B) -> i32 { return b.x; }\n\
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
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
    if a.x > b.x {
        return a;
    }
    return b;
}
fn drain(take b: B) { return; }
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
    // v0.0.24 #9 stage 3e: bare params are read-only borrows, so moving a
    // borrowed region input into an owned value is the borrow-escape E0337
    // (previously surfaced as E0335/E0372). Still correctly rejected.
    assert!(
        stderr.contains("E0335") || stderr.contains("E0372") || stderr.contains("E0337"),
        "expected E0335 / E0372 / E0337, got: {stderr}"
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
impl B { fn drop(ref this) { return; } }
fn passthrough(take b: B) -> B { return b; }
fn drain(take b: B) { return; }
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
impl B { fn drop(ref this) { return; } }
fn drain(n: i32, take b: B) { return; }
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
    "fn main() -> i32 { var x: i32 = 2147483647; x = x + 1; #println(x); return 0; }";

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

/// reports/bug-03: `!alias.scope` / `!noalias` metadata claimed that two
/// pointer-passed `ref` params could not alias. That is the promise the
/// `noalias` ATTRIBUTE was stripped of on 2026-07-27 — the borrow checker
/// denies by design at the statics and raw-pointer seams, so two `ref` params
/// reached through `#addr_of` legally DO alias. The metadata form outlived the
/// attribute fix, and at `-O3` LLVM hoisted both reads of `y.a` above the
/// stores through `x`: the program printed 23 in debug and 20 in release.
///
/// The correct answer is 11 + 12 = 23 in both modes. Debug is the control:
/// if it ever stops printing 23 the probe itself has drifted.
#[test]
fn aliasing_ref_params_are_not_promised_disjoint() {
    const PROBE: &str = "\
struct S { a: i64 }
impl S { fn drop(ref this) { } }
fn cross(ref x: S, ref y: S) -> i64 {
    x.a = x.a + 1;
    let v: i64 = y.a;
    x.a = x.a + 1;
    let w: i64 = y.a;
    return v + w;
}
fn main() -> i32 {
    var s: S = S { a: 10 };
    let p: *S = #addr_of(s);
    var r: i64 = 0;
    r = cross(*p, *p);
    return (r as i32) - 23;
}
";
    for release in [false, true] {
        let (_dir, bin) = compile_program(PROBE, release);
        let run = Command::new(&bin).status().expect("run alias probe");
        assert_eq!(
            run.code(),
            Some(0),
            "aliasing `ref` params must observe each other's writes ({}); \
             a non-zero code is the read that got hoisted",
            if release { "--release" } else { "debug" }
        );
    }
}

/// reports/bug-04, bug-06, bug-07 — monomorphize's walkers missing arms. Each
/// of these compiled to a compiler panic, not a diagnostic: a generic call in a
/// tuple element or an `#asm` operand kept the template name after the template
/// was deleted, and `Self` inside a `loop` was never substituted. The
/// interpolation case (bug-06) needs stdlib and is covered by
/// `generic_call_in_interpolation_monomorphizes` below.
#[test]
fn mono_rewrites_generic_calls_and_self_in_every_position() {
    let (_dir, bin) = compile_program(
        "fn id_it[T](take x: T) -> T { return x; }\n\
         struct W { a: i32 }\n\
         struct Holder[T] { v: T }\n\
         impl Holder[T] {\n\
           fn spin(this) -> i32 {\n\
             loop {\n\
               let h: Self = Self { v: this.v };\n\
               return h.v;\n\
             }\n\
           }\n\
           fn deferred(this) -> i32 {\n\
             var n: i32 = 0;\n\
             {\n\
               defer n = n;\n\
               let h: Self = Self { v: this.v };\n\
               n = h.v;\n\
             }\n\
             return n;\n\
           }\n\
           fn assoc() -> i32 { return 5; }\n\
           fn viaassoc(this) -> i32 { return Self::assoc(); }\n\
         }\n\
         fn main() -> i32 {\n\
           let t: (W, i32) = (W { a: id_it::[i32](7) }, 1);\n\
           var out: i64 = 0;\n\
           #asm(\"mov {v}, {o}\", o = out(reg) out, v = in(reg) id_it::[i64](2));\n\
           let b: Holder[i32] = Holder[i32] { v: 3 };\n\
           return t.0.a + (out as i32) + b.spin() + b.deferred() + b.viaassoc() - 20;\n\
         }",
        false,
    );
    let run = Command::new(&bin).status().expect("run mono probe");
    assert_eq!(run.code(), Some(0), "unexpected exit: {run}");
}

/// reports/bug-06: a generic call inside `"${...}"` was discovered by
/// `visit_ident_calls` but had no `rewrite_expr` arm, so the call site kept the
/// deleted template's name. Needs stdlib, since interpolation builds a `Text`.
#[test]
fn generic_call_in_interpolation_monomorphizes() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"interpmono\"\n\n[[bin]]\nname = \"interpmono\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/io\" as io;\n\
         import \"stdlib/text\" as text;\n\
         \n\
         fn double_it[T](take x: T) -> T { return x; }\n\
         \n\
         fn main() -> i32 {\n\
             let t: text::Text = \"v=${double_it::[i32](7)}\";\n\
             io::println(t.view());\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "generic call in interpolation must build: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let run = Command::new(dir.join("target/debug/interpmono"))
        .output()
        .expect("run interpmono");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "v=7\n");
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

// ----- Integer-literal range checking (E0314) + empty enums (E0361) -----

#[test]
fn int_literal_boundaries_compile_and_run() {
    // Valid extremes must still codegen and run correctly — including the
    // negated-minimum carve-out (`-128` for i8, `-2^63` for i64). `span` is
    // 127 - (-128) = 255, and the i64::MIN value participates so it can't be
    // dead-code eliminated before codegen.
    let (_dir, bin) = compile_program(
        "fn main() -> i32 {\n\
         let lo: i8 = -128;\n\
         let hi: i8 = 127;\n\
         let big: i64 = -9223372036854775808;\n\
         let span: i32 = (hi as i32) - (lo as i32);\n\
         if big < 0 { return span; }\n\
         return 0;\n\
         }",
        false,
    );
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(
        run.status.code(),
        Some(255),
        "expected exit 255 (127 - -128); stderr={:?}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn int_literal_out_of_range_rejected_e0314() {
    // The headline repro: `300` does not fit `i8` (would wrap to 44).
    assert_compile_fails_with(
        "fn main() -> i32 { let x: i8 = 300; return x as i32; }",
        "E0314",
    );
}

#[test]
fn int_literal_i64_overflow_rejected_e0314() {
    assert_compile_fails_with(
        "fn main() -> i32 { let x: i64 = 9223372036854775808; return 0; }",
        "E0314",
    );
}

#[test]
fn empty_enum_rejected_e0361() {
    assert_compile_fails_with(
        "enum Void {}\nfn main() -> i32 { return 0; }",
        "E0361",
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

// ---- pattern-binding `var` forms: `guard var` / `if var` / `while var` ----

/// `guard var` binds into the enclosing scope mutably; the complement form
/// still captures the failure payload alongside it.
#[test]
fn guard_var_binding_is_mutable() {
    let out = compile_and_run_src(
        "guard_var",
        "enum R { Ok(i32), Err(i32) }\n\
         fn get(ok: bool) -> R { if ok { return R::Ok(41); } return R::Err(7); }\n\
         fn happy() -> i32 {\n\
         \x20   guard var R::Ok(c) = get(true) else { return 0 -% 1; };\n\
         \x20   c = c +% 1;\n\
         \x20   return c;\n\
         }\n\
         fn sad() -> i32 {\n\
         \x20   guard var R::Ok(c) = get(false) else |R::Err(e)| { return e; };\n\
         \x20   c = c +% 1;\n\
         \x20   return c;\n\
         }\n\
         fn main() -> i32 {\n\
         \x20   #println(happy());\n\
         \x20   #println(sad());\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(out.status.success(), "exited {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n7\n");
}

/// `if var` / `while var` make the arm bindings mutable inside the body;
/// multi-binding payloads rebind every name.
#[test]
fn if_var_and_while_var_bindings_are_mutable() {
    let out = compile_and_run_src(
        "if_while_var",
        "enum P { Two(i32, i32), N }\n\
         fn tick(n: i32) -> P { if n > 0 { return P::Two(n, 10); } return P::N; }\n\
         fn main() -> i32 {\n\
         \x20   if var P::Two(a, b) = tick(3) {\n\
         \x20       a = a *% 2;\n\
         \x20       b = b +% a;\n\
         \x20       #println(b);\n\
         \x20   }\n\
         \x20   var total: i32 = 0;\n\
         \x20   var n: i32 = 3;\n\
         \x20   while var P::Two(c, _) = tick(n) {\n\
         \x20       c = c *% 2;\n\
         \x20       total = total +% c;\n\
         \x20       n = n -% 1;\n\
         \x20   }\n\
         \x20   #println(total);\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(out.status.success(), "exited {:?}", out.status);
    // if var: 10 + 3*2 = 16; while var: 3*2 + 2*2 + 1*2 = 12.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "16\n12\n");
}

/// The `let` spellings keep their frozen bindings: assignment through a
/// `guard let` / `if let` / `while let` binding (and through a `guard var`
/// complement binding, which is arm-scoped) still fails E0305.
#[test]
fn let_pattern_bindings_stay_immutable() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let cases = [
        (
            "guard_let",
            "enum M { S(i32), N }\n\
             fn main() -> i32 {\n\
             \x20   guard let M::S(v) = M::S(1) else { return 1; };\n\
             \x20   v = v +% 1;\n\
             \x20   return v;\n\
             }\n",
        ),
        (
            "if_let",
            "enum M { S(i32), N }\n\
             fn main() -> i32 {\n\
             \x20   if let M::S(v) = M::S(1) { v = v +% 1; }\n\
             \x20   return 0;\n\
             }\n",
        ),
        (
            "while_let",
            "enum M { S(i32), N }\n\
             fn main() -> i32 {\n\
             \x20   while let M::S(v) = M::N { v = v +% 1; }\n\
             \x20   return 0;\n\
             }\n",
        ),
        (
            "guard_var_complement",
            "enum M { S(i32), N(i32) }\n\
             fn main() -> i32 {\n\
             \x20   guard var M::S(v) = M::S(1) else |M::N(e)| { e = e +% 1; return e; };\n\
             \x20   return v;\n\
             }\n",
        ),
    ];
    for (name, src) in cases {
        let dir = tempdir();
        let src_path = dir.join(format!("{name}.cplus"));
        std::fs::write(&src_path, src).unwrap();
        let out = Command::new(cpc)
            .arg(&src_path)
            .arg("-o")
            .arg(dir.join(name))
            .output()
            .expect("invoke cpc");
        assert!(
            !out.status.success(),
            "{name}: expected compile failure on immutable-binding assignment"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("E0305"),
            "{name}: expected E0305 in stderr, got: {stderr}"
        );
    }
}

/// `guard var` runs through the same guard checks as `guard let`:
/// non-diverging else is still E0348, a bindingless pattern is still E0351,
/// and an irrefutable `if var` is still E0347.
#[test]
fn var_forms_keep_pattern_let_diagnostics() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let cases = [
        (
            "guard_var_nondiverging",
            "enum M { S(i32), N }\n\
             fn main() -> i32 {\n\
             \x20   guard var M::S(v) = M::S(1) else { let x: i32 = 1; };\n\
             \x20   return v;\n\
             }\n",
            "E0348",
        ),
        (
            "guard_var_no_binding",
            "enum M { S(i32), N }\n\
             fn main() -> i32 {\n\
             \x20   guard var _ = M::S(1) else { return 1; };\n\
             \x20   return 0;\n\
             }\n",
            "E0351",
        ),
        (
            "if_var_irrefutable",
            "fn main() -> i32 {\n\
             \x20   if var x = 7 { x = 8; return x; }\n\
             \x20   return 0;\n\
             }\n",
            "E0347",
        ),
    ];
    for (name, src, code) in cases {
        let dir = tempdir();
        let src_path = dir.join(format!("{name}.cplus"));
        std::fs::write(&src_path, src).unwrap();
        let out = Command::new(cpc)
            .arg(&src_path)
            .arg("-o")
            .arg(dir.join(name))
            .output()
            .expect("invoke cpc");
        assert!(!out.status.success(), "{name}: expected compile failure");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(code),
            "{name}: expected {code} in stderr, got: {stderr}"
        );
    }
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
struct Point {
    x: i32,
}

impl Point {
    fn new(x: i32) -> Point {
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

type Point = types::Point;
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
           let p: *u8 = { #str_ptr(cstr) };\n\
           let cls: *u8 = { objc_getClass(p) };\n\
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
        "fn _square(n: i32) -> i32 { return n * n; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./math\" as math;\nfn main() -> i32 { return math::_square(7); }\n",
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
        "fn square(n: i32) -> i32 { return 1.5; }\n",
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
        // v0.0.24 #10: visibility is name-based — `x` is public, `_y` is private.
        "struct Point { x: i32, _y: i32 }\nimpl Point { fn new(x: i32, y: i32) -> Point { return Point { x: x, _y: y }; } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nfn main() -> i32 { let p: g::Point = g::Point::new(1, 2); return p._y; }\n",
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
        "struct Point { x: i32, y: i32 }\nimpl Point { fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; } }\n",
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
        "struct Point { x: i32, _y: i32 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nfn main() -> i32 { let p = g::Point { x: 1, _y: 2 }; return 0; }\n",
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
        "fn square(n: i32) -> i32 { return n * n; }\n",
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
    // Only the hand-written example sources are the formatter's de facto spec.
    // `docs/examples/projects/*/vendor/**` holds vendored dependency copies (the
    // generated bindings, stdlib, etc.) so the example projects build hermetically
    // — those are build artifacts, not authored samples, and the generator does not
    // run its output through `cpc fmt`. Skip anything under a `vendor` segment.
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some("vendor") {
                    continue;
                }
                collect(&p, out);
            } else if p.extension().and_then(|e| e.to_str()) == Some("cplus") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    collect(&root, &mut files);
    assert!(!files.is_empty(), "no example .cplus files found under {root:?}");
    let out = Command::new(cpc)
        .arg("fmt")
        .arg("--check")
        .args(&files)
        .output()
        .expect("invoke cpc fmt --check");
    assert!(
        out.status.success(),
        "cpc fmt --check found drift in hand-written samples:\n{}",
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
impl Tag { fn drop(ref this) { return; } }
fn bump(ref t: Tag) {
    t.v = t.v + 1;
    return;
}
fn main() -> i32 {
    var x: Tag = Tag { v: 10 };
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
fn ref_param_copy_struct_propagates() {
    // #9 stage 3c-copy: a Copy struct `ref` param writes back to the caller's
    // `var` place — the mutation IS observable after the call (it used to be
    // value-passed and silently lost).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "\
struct P { v: i32 }
fn bump(ref p: P) {
    p.v = p.v + 1;
    return;
}
fn main() -> i32 {
    var q: P = P { v: 10 };
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
    // Write-back: the Copy struct `ref` mutation reaches the caller — q.v is 11.
    assert_eq!(String::from_utf8_lossy(&run.stdout), "11\n");
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
    fn drop(ref this) {
        #println(0 -% this.id);
        return;
    }
}
fn bump(ref t: Tracker) {
    t.id = t.id + 1;
    return;
}
fn main() -> i32 {
    var x: Tracker = Tracker { id: 6 };
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

/// extern fns share one unqualified name space (they bind a literal C symbol),
/// so two declarations of the same name with DIFFERENT signatures would silently
/// bind one to the wrong prototype. Sema rejects the mismatch with E0357 instead.
#[test]
fn test_conflicting_extern_signature_rejected_e0357() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(
        &src,
        "extern fn sym(x: i32) -> i32;\nextern fn sym(x: i32) -> i64;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("bad");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    assert!(!out.status.success(), "expected compile failure for conflicting extern sigs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0357"), "expected E0357, got: {stderr}");
}

/// The flip side of E0357: re-declaring the same extern symbol with an IDENTICAL
/// signature is allowed (several stdlib modules each declare `extern fn write` to
/// reach libc). The duplicate is deduped silently — the build must succeed.
#[test]
fn test_matching_extern_redeclaration_allowed() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(
        &src,
        "extern fn sym(x: i32) -> i64;\nextern fn sym(x: i32) -> i64;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("ok");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "matching extern re-decl should compile, got: {stderr}");
}

/// E0357 compares the *effective* linker symbol, not the raw `#[link_name]`:
/// a bare `extern fn sym` (symbol = source name) and `#[link_name = "sym"] extern
/// fn sym` denote the SAME symbol, so with matching sigs they dedup, not clash.
/// (This is the real objc/runtime vs objc/synthesis associated-object pattern.)
#[test]
fn test_extern_bare_and_explicit_same_symbol_allowed() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ok.cplus");
    std::fs::write(
        &src,
        "extern fn sym(x: i32) -> i32;\n#[link_name = \"sym\"]\nextern fn sym(x: i32) -> i32;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let bin = dir.join("ok");
    let out = Command::new(cpc).arg(&src).arg("-o").arg(&bin).output().expect("invoke cpc");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "bare + explicit-same-symbol extern should compile, got: {stderr}");
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
        "#[test] export fn t() { return; }\nfn main() -> i32 { return 0; }\n",
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
        "expected compile failure for export on #[test]"
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
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn main() -> i32 {
    var y: B = B { x: 1 };
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
impl B { fn drop(ref this) { return; } }
fn write_thing(ref a: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn main() -> i32 {
    var y: B = B { x: 1 };
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
impl B { fn drop(ref this) { return; } }
fn write_and_take(ref a: B, take b: B) { return; }
fn main() -> i32 {
    var y: B = B { x: 1 };
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
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn main() -> i32 {
    var y: B = B { x: 1 };
    var z: B = B { x: 2 };
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
fn ref_borrows_of_distinct_copy_places_accepted() {
    // #9 stage 3c-copy: a Copy `ref` is now a real exclusive borrow (it writes
    // back). Two `ref` args of DISTINCT `var` places are fine. (The same place
    // twice would be E0380; a `let` place would be E0328.)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
fn modify_both(ref a: i32, ref b: i32) { return; }
fn main() -> i32 {
    var x: i32 = 1;
    var y: i32 = 2;
    modify_both(x, y);
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
        "two `ref` args of distinct `var` places should compile; stderr: {}",
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
    fn drop(ref this) { return; }
    fn cursor(this) -> VecI32 { return this; }
    fn push(ref this, x: i32) { return; }
}
fn main() -> i32 {
    var v: VecI32 = VecI32 { data: [0, 0, 0, 0, 0, 0, 0, 0], len: 0 };
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
    // v0.0.23: `cursor(self) -> VecI32` returns `self` (a borrow) by value →
    // E0337 (VecI32 has a Drop impl), rejected before the iterator-invalidation
    // (E0381) conflict is reached.
    assert!(
        !out.status.success(),
        "returning `self` by value from a Drop type must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
    fn drop(ref this) { return; }
    fn push(ref this, x: i32) { return; }
}
fn main() -> i32 {
    var v: VecI32 = VecI32 { data: [0, 0, 0, 0, 0, 0, 0, 0], len: 0 };
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
impl B { fn drop(ref this) { return; } }
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
    // Direct unconditional drop call must still appear. It uses the C
    // convention: `preserve_nonecc` was removed from drop glue 2026-07-27
    // (it miscompiled under ASan at -O1+ on arm64 — see
    // bugs/flex-calculate-layout-segv-under-release-plus-asan.md), and the call
    // site has to match the definition or the mismatch is UB.
    assert!(
        ir.contains("call void @B.drop(ptr %x"),
        "expected unconditional drop call; got: {ir}"
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
impl B { fn drop(ref this) { return; } }
fn consume(take b: B) { return; }
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
fn mut_param_is_not_tagged_noalias_in_ir() {
    // The inverse of what this asserted until 2026-07-27, and a soundness fix.
    // `noalias` on a borrow param is a promise the borrow checker cannot keep:
    // a `static` is unchecked by design and `(*p).method()` makes a `ref`
    // receiver out of an untracked raw pointer, so the ordinary callback
    // pattern (method takes `ref this`, calls a stored fn pointer, callback
    // reaches the same object via `#addr_of(SOME_STATIC)`) aliases it legally.
    // LLVM acted on the promise at -O2+ and miscompiled `events::Signal::emit`.
    // `readonly` on shared borrows stays — that constrains the callee, which
    // the callee's own body proves.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn bump(ref b: B) -> i32 { b.x = b.x + 1; return b.x; }
fn main() -> i32 {
    var v: B = B { x: 1 };
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
    // The param is still pointer-passed and still carries the definite-value
    // attributes; it just makes no aliasing claim.
    let def = ir
        .lines()
        .find(|l| l.contains("i32 @bump("))
        .unwrap_or_else(|| panic!("expected a @bump definition; got: {ir}"));
    assert!(
        !def.contains("noalias"),
        "`ref b: B` must NOT promise noalias: {def}"
    );
    assert!(
        def.contains("nonnull noundef"),
        "`ref b: B` should keep nonnull/noundef: {def}"
    );
    // Call sites must agree — the inliner will use either one.
    for l in ir.lines().filter(|l| l.contains("call") && l.contains("@bump(")) {
        assert!(!l.contains("noalias"), "call site must not promise noalias: {l}");
    }
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
impl B { fn drop(ref this) { return; } }
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
        "expected shared borrow `b: B` to lower to `ptr readonly`; got: {ir}"
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
    // out (`fn forward(take x: T) -> T { return x; }`) used to lower as a shared
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
    fn make() -> Owned { return Owned { ptr: { malloc(16 as usize) } }; }
    fn drop(ref this) { { free(this.ptr); } return; }
}
fn forward(take x: Owned) -> Owned { return x; }
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
    fn make() -> Owned { return Owned { ptr: { malloc(16 as usize) } }; }
    fn drop(ref this) { { free(this.ptr); } return; }
}
struct Pair { a: Owned, b: Owned }
impl Pair {
    fn drop(ref this) { { free(this.a.ptr); } { free(this.b.ptr); } return; }
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
    assert!(stderr.contains("E0509"), "expected E0509, got: {stderr}");
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
    fn make() -> Owned { return Owned { ptr: { malloc(16 as usize) } }; }
    fn drop(ref this) { { free(this.ptr); } return; }
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
        format!(
            "{}{}",
            BUF_PRELUDE,
            "\
struct P { x: i32 }
enum R { Both(Buf, P), None }
fn mk() -> R { return R::Both(mk_buf(), P { x: 9 }); }
fn main() -> i32 {
    let r: R = mk();
    let out: i32 = match r {
        R::Both(s, p) => { let kept: Buf = s; kept.len() as i32 +% p.x }
        R::None => { 0 }
    };
    return out -% 13;
}
"
        ),
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
    fn make() -> Res { return Res { p: { malloc(16 as usize) } }; }
    fn drop(ref this) { { free(this.p); } return; }
}
struct Holder { r: Res }
fn consume(take h: Holder) -> i32 { return 0; }
fn main() -> i32 {
    var i: i32 = 0;
    var acc: i32 = 0;
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
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

// R4: these borrow-check tests exercise the `as_str`/`as_slice`-by-name view
// root via a tiny user Drop struct `Buf` whose `as_str()` borrows `self` —
// `returned_borrow_root` recognizes any `recv.as_str()` / `recv.as_slice()` by
// name, so E0513 fires on a returned view of a local. (`Text` no longer has an
// `as_str` method — it coerces to `str` directly; the coercion has its own
// E0513 coverage in `text_coercion_*` below.)
const BUF_PRELUDE: &str = "extern fn malloc(n: usize) -> *u8;\n\
     extern fn free(p: *u8);\n\
     struct Buf { ptr: *u8 }\n\
     impl Buf {\n\
         fn drop(ref this) { { free(this.ptr); } return; }\n\
         fn as_str(this) -> str { return { #str_from_raw_parts(this.ptr, 4 as usize) }; }\n\
         fn len(this) -> usize { return 4 as usize; }\n\
     }\n\
     fn mk_buf() -> Buf { return Buf { ptr: { malloc(4 as usize) } }; }\n";

/// 2026-07-22 view-lifetime audit toolkit: a Drop struct whose view accessor
/// is named `view()` (a *shape*-based view, NOT the allowlisted `as_str`),
/// plus a read-only `len()`, a `Slot { s: str }` carrier, a constructor, and a
/// consumer. Exercises the generalized (name-independent) view tracking.
const VIEW_PRELUDE: &str = "extern fn malloc(n: usize) -> *u8;\n\
     extern fn free(p: *u8);\n\
     struct Buf { ptr: *u8 }\n\
     impl Buf {\n\
         fn drop(ref this) { { free(this.ptr); } return; }\n\
         fn view(this) -> str { return { #str_from_raw_parts(this.ptr, 4 as usize) }; }\n\
         fn len(this) -> usize { return 4 as usize; }\n\
     }\n\
     struct Slot { s: str }\n\
     fn mk() -> Buf { return Buf { ptr: { malloc(4 as usize) } }; }\n\
     fn consume(take b: Buf) { return; }\n";

// ---------------------------------------------------------------------------
// 2026-07-22 view-lifetime / ownership audit (bugs/mem 01-09). Each `str`/slice
// view of storage that dies at function return must be rejected (E0513 escape,
// E0372 move-while-borrowed, E0381 mutate-while-borrowed) — regardless of the
// accessor's name, the syntactic form the view escapes through, or whether the
// owner is a local, a `take` parameter, or a generic instantiation.
// ---------------------------------------------------------------------------

#[test]
fn view_method_return_of_local_rejected_e0513() {
    // Bug 01: E0513 must trace a view by SHAPE, not the `as_str`/`as_slice`
    // name allowlist — `view()` returning a str borrows its local receiver.
    for tail in [
        "fn bad() -> str { let b: Buf = mk(); return b.view(); }",
        "fn bad() -> str { let b: Buf = mk(); let s: str = b.view(); return s; }",
    ] {
        let (ok, stderr) =
            try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\nfn main() -> i32 {{ return 0; }}\n"));
        assert!(!ok, "expected E0513 for `{tail}`, compiled instead");
        assert!(stderr.contains("E0513"), "expected E0513 for `{tail}`, got: {stderr}");
    }
}

#[test]
fn view_in_returned_aggregate_of_local_rejected_e0513() {
    // Bug 01 (aggregate): a `view()` leaf embedded in a returned struct still
    // dangles — the local drops at return under the escaped view.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{VIEW_PRELUDE}fn bad() -> Slot {{ let b: Buf = mk(); return Slot {{ s: b.view() }}; }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(!ok, "expected E0513, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn view_stored_into_projection_then_move_rejected_e0372() {
    // Bug 02: writing a view into a field / index place must pin the owner, so
    // moving it while the aggregate holds the view is E0372 — same as the
    // construction form `let w = Slot { s: b.view() }`.
    for tail in [
        "fn main() -> i32 { let b: Buf = mk(); var w: Slot = Slot { s: \"\" }; \
         w.s = b.view(); consume(b); return 0; }",
        "fn main() -> i32 { let b: Buf = mk(); var a: [str; 1] = [\"\"]; \
         a[0] = b.view(); consume(b); return 0; }",
    ] {
        let (ok, stderr) = try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\n"));
        assert!(!ok, "expected E0372 for `{tail}`, compiled instead");
        assert!(stderr.contains("E0372"), "expected E0372 for `{tail}`, got: {stderr}");
    }
}

#[test]
fn view_of_temporary_receiver_rejected_e0513() {
    // Bug 03: `mk().view()` binds a view of a statement-scoped temporary that
    // drops immediately — the binding would dangle.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{VIEW_PRELUDE}fn main() -> i32 {{ let s: str = mk().view(); return 0; }}\n"
    ));
    assert!(!ok, "expected E0513, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn view_return_of_take_param_rejected_e0513() {
    // Bug 04: a `take` parameter / `take this` owns its value and drops it at
    // return, so a returned view of it dangles — even via `as_str` or coercion,
    // forms E0513 already understood for locals.
    for tail in [
        "fn steal(take b: Buf) -> str { return b.view(); }",
        "impl Buf { fn into_view(take this) -> str { return this.view(); } }",
    ] {
        let (ok, stderr) =
            try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\nfn main() -> i32 {{ return 0; }}\n"));
        assert!(!ok, "expected E0513 for `{tail}`, compiled instead");
        assert!(stderr.contains("E0513"), "expected E0513 for `{tail}`, got: {stderr}");
    }
}

#[test]
fn view_return_via_free_fn_of_local_rejected_e0513() {
    // Bug 05: `return head(local)` where `head(b) -> str` returns a view of its
    // parameter escapes the local `b` — trace through the free-fn call.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{VIEW_PRELUDE}fn head(b: Buf) -> str {{ return b.view(); }}\n\
         fn bad() -> str {{ let b: Buf = mk(); return head(b); }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(!ok, "expected E0513, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn view_through_control_flow_expr_then_move_rejected_e0372() {
    // Bug 06: a view produced by an `if` / block / `match` *expression* still
    // pins its owner — moving the owner while the binding is live is E0372.
    for tail in [
        "fn main() -> i32 { let b: Buf = mk(); \
         let s: str = if true { b.view() } else { \"\" }; consume(b); return 0; }",
        "fn main() -> i32 { let b: Buf = mk(); let s: str = { b.view() }; consume(b); return 0; }",
    ] {
        let (ok, stderr) = try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\n"));
        assert!(!ok, "expected E0372 for `{tail}`, compiled instead");
        assert!(stderr.contains("E0372"), "expected E0372 for `{tail}`, got: {stderr}");
    }
}

#[test]
fn view_through_destructure_then_move_rejected_e0372() {
    // Bug 07: destructuring an aggregate that embeds a view re-binds a borrow,
    // not an owned resource — the owner must stay pinned.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{VIEW_PRELUDE}fn main() -> i32 {{ let b: Buf = mk(); \
         let Slot {{ s }} = Slot {{ s: b.view() }}; consume(b); return 0; }}\n"
    ));
    assert!(!ok, "expected E0372, compiled instead");
    assert!(stderr.contains("E0372"), "expected E0372, got: {stderr}");
}

#[test]
fn view_of_local_stored_into_static_rejected_e0513() {
    // Bug 09: storing a view of a frame-dying local into a `static` (or a
    // static's field) lets it outlive its owner for the whole program.
    for tail in [
        "static S: str = \"\";\nfn main() -> i32 { { let b: Buf = mk(); S = b.view(); } return 0; }",
        "static W: Slot = Slot { s: \"\" };\nfn main() -> i32 { let b: Buf = mk(); W.s = b.view(); return 0; }",
    ] {
        let (ok, stderr) = try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\n"));
        assert!(!ok, "expected E0513 for `{tail}`, compiled instead");
        assert!(stderr.contains("E0513"), "expected E0513 for `{tail}`, got: {stderr}");
    }
}

#[test]
fn view_stored_into_ref_out_param_rejected_e0513() {
    // Bug 02-C: writing a view of a callee-local into a `ref` out-parameter
    // escapes the borrow to the caller, who outlives the callee's local.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{VIEW_PRELUDE}fn stash(ref w: Slot) {{ let b: Buf = mk(); w.s = b.view(); return; }}\n\
         fn main() -> i32 {{ var w: Slot = Slot {{ s: \"\" }}; stash(w); return 0; }}\n"
    ));
    assert!(!ok, "expected E0513, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn view_lifetime_sound_forms_still_compile() {
    // Controls — none of these dangle, so all must keep compiling (guards the
    // audit fixes against over-rejection / false positives):
    //  * a view of a local used while the owner stays put (lexical pin),
    //  * a view returned from a bare (borrowing) parameter (caller-tied),
    //  * a read-only method called alongside a live view (shared + shared),
    //  * a view of a temporary passed as a direct argument (temp still alive),
    //  * moving an owned value into a returned aggregate (not a view).
    for (label, tail) in [
        (
            "used-before-move",
            "fn peek(x: str) -> i32 { return 0; }\n\
             fn main() -> i32 { let b: Buf = mk(); let s: str = b.view(); return peek(s); }",
        ),
        (
            "view-of-bare-param",
            "fn head(b: Buf) -> str { return b.view(); }\nfn main() -> i32 { return 0; }",
        ),
        (
            "read-method-alongside-view",
            "fn peek(x: str) -> i32 { return 0; }\n\
             fn main() -> i32 { let b: Buf = mk(); let s: str = b.view(); \
             let n: usize = b.len(); return peek(s) + (n as i32); }",
        ),
        (
            "view-of-temp-as-argument",
            "fn peek(x: str) -> i32 { return 0; }\n\
             fn main() -> i32 { return peek(mk().view()); }",
        ),
        (
            "move-owned-into-aggregate",
            "struct Own { b: Buf }\n\
             fn wrap(take b: Buf) -> Own { return Own { b: b }; }\nfn main() -> i32 { return 0; }",
        ),
    ] {
        let (ok, stderr) = try_compile_snippet(&format!("{VIEW_PRELUDE}{tail}\n"));
        assert!(ok, "sound form `{label}` must compile; stderr: {stderr}");
    }
}

#[test]
fn return_borrow_of_local_owned_rejected_e0513() {
    // v0.0.12 (#3): returning a `str` view into a function-local owned value
    // (which drops at function exit) dangles — reject it.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}fn bad() -> str {{\n\
             let s: Buf = mk_buf();\n\
             return s.as_str();\n\
         }}\n\
         fn main() -> i32 {{ return #str_len(bad()) as i32; }}\n"
    ));
    assert!(!ok, "expected E0513 rejection, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn return_borrow_alias_of_local_owned_rejected_e0513() {
    // Returning an alias to `s.as_str()` is the same dangling view as
    // returning `s.as_str()` directly.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}fn bad() -> str {{\n\
             let s: Buf = mk_buf();\n\
             let view: str = s.as_str();\n\
             return view;\n\
         }}\n\
         fn main() -> i32 {{ return #str_len(bad()) as i32; }}\n"
    ));
    assert!(!ok, "expected E0513 rejection, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn return_borrow_branch_alias_of_local_owned_rejected_e0513() {
    // Flow merging must keep the unsafe branch provenance even when another
    // branch assigns a literal-backed view.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}fn bad(flag: bool) -> str {{\n\
             let s: Buf = mk_buf();\n\
             var view: str;\n\
             if flag {{ view = s.as_str(); }} else {{ view = \"static\"; }}\n\
             return view;\n\
         }}\n\
         fn main() -> i32 {{ return #str_len(bad(true)) as i32; }}\n"
    ));
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
    assert!(
        ok,
        "returning a literal-backed str must compile; stderr: {stderr}"
    );
}

#[test]
fn return_slice_of_param_compiles() {
    // v0.0.12 (#3) positive: returning a view borrowed from a parameter is
    // caller-tied and sound — must not be flagged as a dangling local.
    let (ok, stderr) = try_compile_snippet(
        "fn first(s: str) -> str { return s; }\n\
         fn main() -> i32 { return #str_len(first(\"x\")) as i32; }\n",
    );
    assert!(
        ok,
        "returning a borrow of a parameter must compile; stderr: {stderr}"
    );
}

#[test]
fn escaping_view_in_returned_struct_rejected_e0513() {
    // v0.0.13 (Tier 1): the dangle hidden inside a returned aggregate. The
    // view borrows local `s`, which drops at return — so the struct carries a
    // dangling view. E0513 even though the return *type* is a struct, not a view.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}struct Holder {{ view: str }}\n\
         fn keep() -> Holder {{\n\
             let s: Buf = mk_buf();\n\
             return Holder {{ view: s.as_str() }};\n\
         }}\n\
         fn main() -> i32 {{ let h: Holder = keep(); return 0; }}\n"
    ));
    assert!(!ok, "expected E0513 on the escaping view, compiled instead");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn move_owned_field_into_returned_struct_compiles() {
    // v0.0.13 (Tier 1) negative-guard: moving an *owned* `string` into a
    // returned struct is a normal ownership transfer — must NOT be flagged.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}struct Owner {{ s: Buf }}\n\
         fn mk2() -> Owner {{\n\
             let s: Buf = mk_buf();\n\
             return Owner {{ s: s }};\n\
         }}\n\
         fn main() -> i32 {{ let o: Owner = mk2(); return 0; }}\n"
    ));
    assert!(
        ok,
        "moving an owned value into a returned struct must compile; stderr: {stderr}"
    );
}

#[test]
fn param_rooted_view_in_returned_struct_compiles() {
    // v0.0.13 (Tier 1) negative-guard: a view borrowed from a *parameter* is
    // caller-tied (the source outlives the call), so storing it in a returned
    // struct is sound — must not be flagged as a dangling local.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{BUF_PRELUDE}struct Holder {{ view: str }}\n\
         fn wrap(s: Buf) -> Holder {{ return Holder {{ view: s.as_str() }}; }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(
        ok,
        "param-rooted view in a returned struct must compile; stderr: {stderr}"
    );
}

// v0.0.24 #11: a minimal `#[lang("string")]` struct exercises the `Text`→`str`
// coercion and its E0513 view-escape re-base without pulling in the stdlib.
// `opaque ptr` exempts the field from raw-pointer drop accounting (a notional
// owner — we never allocate); the `drop` makes it non-Copy like `Text`, which
// is what makes a returned view of a *local* one dangle.
const LANG_STR_PRELUDE: &str = "#[lang(\"string\")]\n\
     struct LStr { opaque ptr: *u8, len: usize, cap: usize }\n\
     impl LStr {\n\
         fn drop(ref this) { return; }\n\
     }\n\
     fn mk() -> LStr { return LStr { ptr: 0 as *u8, len: 0 as usize, cap: 0 as usize }; }\n";

#[test]
fn text_coercion_return_local_rejected_e0513() {
    // The headline UB guard: a local lang-string coerced to `str` and returned
    // dangles (its owner drops at function exit). The coercion has no `as_str`
    // anchor, so E0513 must fire on the bare `return s` — `check_returned_borrow`
    // sees the `str`-shaped return rooted at a local non-Copy value.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}fn bad() -> str {{\n\
             let s: LStr = mk();\n\
             return s;\n\
         }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(!ok, "expected E0513 on a returned local-string view");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn text_coercion_escaping_in_aggregate_rejected_e0513() {
    // The same dangle hidden in a returned aggregate: `Holder`'s `str` field
    // coerces a local lang-string. There is no `as_str` to key on, so
    // `flag_view_leaves` must consult the coercion table to find the leaf and
    // fire E0513.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         fn keep() -> Holder {{\n\
             let s: LStr = mk();\n\
             return Holder {{ view: s }};\n\
         }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(!ok, "expected E0513 on a coerced view escaping in an aggregate");
    assert!(stderr.contains("E0513"), "expected E0513, got: {stderr}");
}

#[test]
fn text_coercion_param_root_compiles() {
    // Negative-guard: a view coerced from a *parameter* lang-string is
    // caller-tied (the source outlives the call), so returning it bare or
    // stored in an aggregate is sound — must NOT trip E0513.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         fn bare(p: LStr) -> str {{ return p; }}\n\
         fn wrap(p: LStr) -> Holder {{ return Holder {{ view: p }}; }}\n\
         fn main() -> i32 {{ return 0; }}\n"
    ));
    assert!(ok, "a param-rooted coerced view must compile; stderr: {stderr}");
}

// ── Memory-model contract §3.3 / §5 (2026-08-01): the scope-exit and
// param-store escapes. The compile-clean-then-ASan-UAF probes behind these
// live in bugs/mem/; each route below was a verified safe-code
// use-after-free before the checks landed.

#[test]
fn carrier_assigned_outward_over_dying_owner_rejected_e0514() {
    // skel.txt's route: the view arrives as a str param, escapes in the
    // returned carrier (`make`), the carrier is assigned to an OUTER
    // binding, and the owner dies at the block's end. The tie machinery
    // records the borrow (moving the owner already fired E0372) — the
    // scope exit must reject it too.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Data {{ key: str }}\n\
         fn make(k: str) -> Data {{ return Data {{ key: k }}; }}\n\
         fn main() -> i32 {{\n\
             var d: Data = Data {{ key: \"\" }};\n\
             {{\n\
                 let s: LStr = mk();\n\
                 d = make(s);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(!ok, "expected E0514 on carrier outliving its owner's scope");
    assert!(stderr.contains("E0514"), "expected E0514, got: {stderr}");
}

#[test]
fn undeclared_concrete_setter_compiles_and_caller_ties_e0514() {
    // Contract §3 narrowing (2026-08-01): a CONCRETE method storing a view
    // param needs no #[keeps(this)] — the flow pass computes the store and
    // every resolvable call site ties. The definition compiles; the caller
    // with a dying owner is rejected.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         impl Holder {{\n\
             fn set(ref this, k: str) {{ this.view = k; return; }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             let t: LStr = mk();\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             h.set(t);\n\
             return 0;\n\
         }}\n"
    ));
    assert!(ok, "undeclared concrete setter with sound order must compile; stderr: {stderr}");
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         impl Holder {{\n\
             fn set(ref this, k: str) {{ this.view = k; return; }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             {{\n\
                 let t: LStr = mk();\n\
                 h.set(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(
        !ok && stderr.contains("E0514"),
        "caller of undeclared setter must tie via computed flows, got ok={ok}: {stderr}"
    );
}

#[test]
fn view_param_stored_into_static_rejected_e0515() {
    let (ok, stderr) = try_compile_snippet(
        "static KEY: str = \"\";\n\
         fn stash(k: str) {\n\
             KEY = k;\n\
             return;\n\
         }\n\
         fn main() -> i32 { return 0; }\n",
    );
    assert!(!ok, "expected E0515 on a view param stored into a static");
    assert!(stderr.contains("E0515"), "expected E0515, got: {stderr}");
}

#[test]
fn keeps_this_setter_compiles_and_caller_ties_e0514() {
    // `#[keeps(this)]` lifts E0515 at the definition — the store becomes a
    // declared flow — and the CALLER now owes the lifetime: the receiver
    // borrows the argument's owner, so the owner dying first is E0514.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         impl Holder {{\n\
             #[keeps(this)]\n\
             fn set(ref this, k: str) {{\n\
                 this.view = k;\n\
                 return;\n\
             }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             {{\n\
                 let t: LStr = mk();\n\
                 h.set(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(!ok, "expected E0514 at the keeps(this) call site");
    assert!(stderr.contains("E0514"), "expected E0514, got: {stderr}");
}

#[test]
fn keeps_this_sound_orders_compile() {
    // Positive guards: owner outliving the receiver, a literal argument
    // (static bytes, nothing to tie), and a block-local receiver under an
    // outer owner must all stay legal.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         impl Holder {{\n\
             #[keeps(this)]\n\
             fn set(ref this, k: str) {{\n\
                 this.view = k;\n\
                 return;\n\
             }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             let t: LStr = mk();\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             h.set(t);\n\
             h.set(\"literal\");\n\
             {{\n\
                 var inner: Holder = Holder {{ view: \"\" }};\n\
                 inner.set(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(ok, "sound keeps(this) orders must compile; stderr: {stderr}");
}

#[test]
fn transitive_keeps_wrapper_caller_ties_e0514() {
    // Contract §5, computed half: an UNDECLARED wrapper that forwards its
    // view param to a #[keeps(this)] method must tie its callers exactly
    // like the declared method — the flow pass computes the transitive
    // param→receiver flow from the body.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         impl Holder {{\n\
             #[keeps(this)]\n\
             fn set(ref this, k: str) {{ this.view = k; return; }}\n\
             fn set_outer(ref this, k: str) {{ this.set(k); return; }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             {{\n\
                 let t: LStr = mk();\n\
                 h.set_outer(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(!ok, "expected E0514 through the undeclared wrapper");
    assert!(stderr.contains("E0514"), "expected E0514, got: {stderr}");
}

#[test]
fn raw_view_store_requires_keeps_e0516() {
    // Contract §5 mandatory choice: a view stored through a raw deref is
    // invisible to flow analysis — the fn must declare with #[keeps(...)].
    let (ok, stderr) = try_compile_snippet(
        "fn stash(slot: *str, v: str) {
             *slot = v;
             return;
         }
         fn main() -> i32 { return 0; }
",
    );
    assert!(!ok && stderr.contains("E0516"), "expected E0516, got ok={ok}: {stderr}");
    let (ok, stderr) = try_compile_snippet(
        "#[keeps(nothing)]
         fn stash(slot: *str, v: str) {
             *slot = v;
             return;
         }
         fn main() -> i32 { return 0; }
",
    );
    assert!(ok, "declared raw store must compile; stderr: {stderr}");
}

#[test]
fn free_fn_ref_param_flow_ties_e0514() {
    // Contract §5, free-fn half: an undeclared free fn forwarding its view
    // param into a keeps-method on a ref param ties the caller's dst arg.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}
         impl Holder {{
             #[keeps(this)]
             fn set(ref this, k: str) {{ this.view = k; return; }}
         }}
         fn store_in(ref h: Holder, k: str) {{ h.set(k); return; }}
         fn main() -> i32 {{
             var h: Holder = Holder {{ view: \"\" }};
             {{
                 let t: LStr = mk();
                 store_in(h, t);
             }}
             return 0;
         }}
"
    ));
    assert!(!ok && stderr.contains("E0514"), "expected E0514 via ref-param flow, got ok={ok}: {stderr}");
}

#[test]
fn undeclared_generic_setter_ties_e0514() {
    // Final pass: the flow pass analyzes GENERIC impl bodies too (the
    // param→receiver structure is type-agnostic); the Generic receiver
    // resolution substitutes at call sites to gate which instantiations
    // tie. An undeclared generic setter needs no attribute.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct GenHolder[T] {{ opaque p: *u8, view: str }}\n\
         impl GenHolder[T] {{\n\
             fn gset(ref this, k: str) {{ this.view = k; return; }}\n\
         }}\n\
         fn main() -> i32 {{\n\
             var g: GenHolder[str] = GenHolder[str] {{ p: 0 as *u8, view: \"\" }};\n\
             {{\n\
                 let t: LStr = mk();\n\
                 g.gset(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(
        !ok && stderr.contains("E0514"),
        "undeclared generic setter must tie via computed flows, got ok={ok}: {stderr}"
    );
}

#[test]
fn free_fn_ref_store_narrowed_but_address_taken_denied() {
    // Final pass: a concrete free fn storing a view param into a ref
    // param compiles (flows exported, direct callers tie) — but taking
    // the fn's ADDRESS keeps the E0515 deny, because indirect calls
    // carry no computed flows.
    let (ok, stderr) = try_compile_snippet(&format!(
        "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
         fn put(ref h: Holder, k: str) {{ h.view = k; return; }}\n\
         fn main() -> i32 {{\n\
             var h: Holder = Holder {{ view: \"\" }};\n\
             {{\n\
                 let t: LStr = mk();\n\
                 put(h, t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    ));
    assert!(
        !ok && stderr.contains("E0514"),
        "undeclared free-fn ref store must compile and tie callers, got ok={ok}: {stderr}"
    );
    let (ok, stderr) = try_compile_snippet(
        "struct Holder { view: str }\n\
         fn put(ref h: Holder, k: str) { h.view = k; return; }\n\
         fn consume(f: fn(ref Holder, str)) { return; }\n\
         fn main() -> i32 { consume(put); return 0; }\n",
    );
    assert!(
        !ok && stderr.contains("E0515"),
        "address-taken storing fn must keep the E0515 deny, got ok={ok}: {stderr}"
    );
}

#[test]
fn keeps_nothing_unties_view_return() {
    // `#[keeps(nothing)]` suppresses the conservative Rule E-VIEW-FN tie:
    // an intern-shaped fn's result may outlive the argument's owner. The
    // same program without the attribute is the E0514 control below.
    let with_attr = format!(
        "{LANG_STR_PRELUDE}#[keeps(nothing)]\n\
         fn intern_like(s: str) -> str {{ return \"\"; }}\n\
         fn main() -> i32 {{\n\
             var key: str = \"\";\n\
             {{\n\
                 let t: LStr = mk();\n\
                 key = intern_like(t);\n\
             }}\n\
             return 0;\n\
         }}\n"
    );
    let (ok, stderr) = try_compile_snippet(&with_attr);
    assert!(ok, "keeps(nothing) must untie the return; stderr: {stderr}");

    let without_attr = with_attr.replace("#[keeps(nothing)]\n", "");
    let (ok, stderr) = try_compile_snippet(&without_attr);
    assert!(
        !ok && stderr.contains("E0514"),
        "without keeps(nothing) the conservative tie must fire E0514; got ok={ok}, stderr: {stderr}"
    );
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
    assert_eq!(
        run.status.code(),
        Some(3),
        "expected #str_len(\"aaa\") == 3"
    );
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
fn musttail_large_by_value_aggregate_return_compiles_and_runs() {
    // Regression: `return make_big();` where Big is a >16-byte Copy struct
    // returned by value. Such a return is ABI-indirect (in memory) on BOTH
    // x86-64 SysV and arm64 AAPCS64, so the tail call cannot be `musttail` —
    // LLVM's backend aborts with "failed to perform tail call elimination on
    // a call site marked musttail". The eligibility guard used to apply the
    // >16B size check only on x86-64, so arm64-darwin emitted an illegal
    // musttail and clang's backend failed. Surfaced building the llama.cpp
    // bindings (the 72-byte `llama_model_params` return). Must compile and run.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
struct Big { a: i64, b: i64, c: i64 }
fn make_big() -> Big { return Big { a: 1, b: 2, c: 3 }; }
fn wrap() -> Big { return make_big(); }
fn main() -> i32 {
    let b: Big = wrap();
    return (b.a + b.b + b.c) as i32;
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
        ">16B by-value aggregate tail-call return must compile (no musttail backend abort); stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(6), "expected 1+2+3 == 6");
}

#[test]
fn interface_bound_satisfied_in_package_mode() {
    // Bound-qualification regression (2026-07-06, found building the facet
    // component prototype): the resolver qualified interface DECLARATIONS
    // and impl-block interface names to module scope, but never rewrote
    // generic-param BOUND names — `[B: Backend]` kept bare "Backend" while
    // interface_impls held ("src.main.Backend", "src.main.Mac"), so EVERY
    // user-interface bound failed E0502 in package mode. Single-file mode
    // qualifies nothing (both sides bare), which is why the single-file
    // e2e passed while the identical package failed. Resolver now
    // qualifies bounds at all generic_params sites (fn, method, struct,
    // enum, impl).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bp\"\n\n[[bin]]\nname = \"bp\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "interface Backend { fn flush(this) -> i32; }\n\
         struct Mac { fd: i32 }\n\
         impl Mac: Backend { fn flush(this) -> i32 { return this.fd; } }\n\
         struct App[B: Backend] { backend: B }\n\
         impl App[B: Backend] { fn run(this) -> i32 { return this.backend.flush(); } }\n\
         fn render[B: Backend](b: B) -> i32 { return b.flush(); }\n\
         fn render_ptr[B: Backend](p: *B) -> i32 { return { (*p).flush() }; }\n\
         fn main() -> i32 {\n\
             let byval: i32 = render(Mac { fd: 5 });\n\
             var m: Mac = Mac { fd: 3 };\n\
             let byptr: i32 = render_ptr(#addr_of(m));\n\
             let a: App[Mac] = App[Mac] { backend: Mac { fd: 7 } };\n\
             return byval + byptr + a.run();\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "interface bounds must satisfy in package mode (bound-name qualification)"
    );
    let run = Command::new(dir.join("target/debug/bp"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(15), "5 + 3 + 7");
}

#[test]
fn str_view_cannot_outlive_owner() {
    // Rule E-VIEW (2026-07-06): a borrow-receiver method returning `str`
    // or a slice ties its result to the receiver. Moving the owner while
    // the view is live is rejected; using the view before the move is fine.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vw\"\n\n[[bin]]\nname = \"vw\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let bad = "import \"stdlib/text\" as text;\n\
         fn consume(take t: text::Text) { return; }\n\
         fn peek(x: str) -> i32 { return 0; }\n\
         fn main() -> i32 {\n\
             let t: text::Text = \"hello\";\n\
             let s: str = t.view();\n\
             consume(t);\n\
             return peek(s);\n\
         }\n";
    std::fs::write(dir.join("src/main.cplus"), bad).unwrap();
    let out = Command::new(cpc).arg("check").current_dir(&dir).output().expect("cpc");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "view must not outlive its owner");
    assert!(all.contains("E0372"), "expected E0372, got: {all}");

    // Positive: a view used while the owner stays put is legal (borrows
    // are lexical — the owner is pinned for the view's scope, so the
    // no-move form is the sanctioned idiom).
    let ok = bad.replace("consume(t);\nreturn peek(s);", "return peek(s);");
    assert_ne!(ok, bad, "replace must rewrite the program");
    std::fs::write(dir.join("src/main.cplus"), ok).unwrap();
    let st = Command::new(cpc).arg("check").current_dir(&dir).status().expect("cpc");
    assert!(st.success(), "a view with a live owner must stay legal");
}

#[test]
fn str_builtin_methods_compile_and_run() {
    // STRM (v0.0.27): the blessed `impl str` block in stdlib/src/str.cplus.
    // One program exercises the whole pipeline: resolution on a builtin
    // receiver, sub-view returns, default-param splicing (`drop_first()`),
    // named params, `split -> Vec[str]` (sret return), `to_i64 -> Option`,
    // a method call inside another fn, and chained-receiver single
    // evaluation (the side-effecting producer must run exactly once).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sm\"\n\n[[bin]]\nname = \"sm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let prog = "import \"stdlib/str\" as str_methods;\n\
         import \"stdlib/option\" as option;\n\
         import \"stdlib/io\" as io;\n\
         fn produced() -> str {\n\
             io::println(\"produced\");\n\
             return \"  a,b,c  \";\n\
         }\n\
         fn main() -> i32 {\n\
             let t = produced().trim();\n\
             if t != \"a,b,c\" { return 1; }\n\
             if t.count() != (5 as usize) { return 2; }\n\
             if t.drop_first() != \",b,c\" { return 3; }\n\
             let parts = t.split(separator: \",\");\n\
             if parts.count() != (3 as usize) { return 4; }\n\
             match parts.at(index: 1 as usize) {\n\
                 option::Option[str]::Some(p) => { if p != \"b\" { return 5; } }\n\
                 option::Option[str]::None => { return 6; }\n\
             }\n\
             match \"-42\".to_i64() {\n\
                 option::Option[i64]::Some(v) => {\n\
                     if v != ((0 as i64) - (42 as i64)) { return 7; }\n\
                 }\n\
                 option::Option[i64]::None => { return 8; }\n\
             }\n\
             return 0;\n\
         }\n";
    std::fs::write(dir.join("src/main.cplus"), prog).unwrap();
    let st = Command::new(cpc).arg("build").current_dir(&dir).status().expect("cpc build");
    assert!(st.success(), "str-methods program must compile");
    let run = Command::new(dir.join("target/debug/sm"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(0), "all method checks must pass");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        stdout.matches("produced").count(),
        1,
        "chained rvalue receiver must evaluate exactly once; got: {stdout}"
    );
}

#[test]
fn slice_array_count_and_to_f64_run() {
    // STRM v2 (2026-07-31): blessed `count()`/`is_empty()` on slice and
    // array receivers (rvalue and place shapes), plus `str::to_f64` via
    // the shape-scan + strtod path.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sc\"\n\n[[bin]]\nname = \"sc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let prog = "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/str\" as str_methods;\n\
         import \"stdlib/option\" as option;\n\
         fn measure(xs: i32[]) -> usize {\n\
             if xs.is_empty() { return 100 as usize; }\n\
             return xs.count();\n\
         }\n\
         fn main() -> i32 {\n\
             var v = vec::Vec[i32]::new();\n\
             v.append(1);\n\
             v.append(2);\n\
             let s: i32[] = v.as_slice();\n\
             if s.count() != (2 as usize) { return 1; }\n\
             let arr: [i32; 4] = [9, 9, 9, 9];\n\
             if arr.count() != (4 as usize) { return 2; }\n\
             if arr.is_empty() { return 3; }\n\
             if measure(s) != (2 as usize) { return 4; }\n\
             match \"2.5\".to_f64() {\n\
                 option::Option[f64]::Some(f) => {\n\
                     if f * 2.0 != 5.0 { return 5; }\n\
                 }\n\
                 option::Option[f64]::None => { return 6; }\n\
             }\n\
             match \"2.5.1\".to_f64() {\n\
                 option::Option[f64]::Some(_f) => { return 7; }\n\
                 option::Option[f64]::None => {}\n\
             }\n\
             return 0;\n\
         }\n";
    std::fs::write(dir.join("src/main.cplus"), prog).unwrap();
    let st = Command::new(cpc).arg("build").current_dir(&dir).status().expect("cpc build");
    assert!(st.success(), "slice/array/to_f64 program must compile");
    let run = Command::new(dir.join("target/debug/sc"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(0), "all checks must pass");
}

#[test]
fn discard_import_alias_underscore() {
    // STRM v3 (2026-08-01): `import "path" as _;` — the discard alias for
    // extension-only imports. The module joins the build (str methods
    // resolve, program runs); multiple discard imports coexist; `_::x`
    // does not parse as a module reference.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"di\"\n\n[[bin]]\nname = \"di\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let prog = "import \"stdlib/str\" as _;\n\
         import \"stdlib/text\" as _;\n\
         fn main() -> i32 {\n\
             let n = \"  hi  \".trim().count();\n\
             if n != (2 as usize) { return 1; }\n\
             return 0;\n\
         }\n";
    std::fs::write(dir.join("src/main.cplus"), prog).unwrap();
    let st = Command::new(cpc).arg("build").current_dir(&dir).status().expect("cpc build");
    assert!(st.success(), "discard-import program must compile");
    let run = Command::new(dir.join("target/debug/di"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(0), "methods via `as _` must work");

    let bad = "import \"stdlib/text\" as _;\n\
         fn main() -> i32 { let t = _::from_str(\"x\"); return 0; }\n";
    std::fs::write(dir.join("src/main.cplus"), bad).unwrap();
    let out = Command::new(cpc).arg("check").current_dir(&dir).output().expect("cpc");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "`_::x` must not resolve");
    assert!(all.contains("E0100"), "expected a parse error, got: {all}");
}

#[test]
fn str_builtin_methods_negative_paths() {
    // STRM (v0.0.27): (a) no stdlib/str in the build → E0324 with the
    // import note; (b) a second `impl str` next to stdlib's → E0385;
    // (c) `#[no_alloc]` fn calling the allocating `split` → E0901.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"sn\"\n\n[[bin]]\nname = \"sn\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();

    let check = |src: &str| -> (bool, String) {
        std::fs::write(dir.join("src/main.cplus"), src).unwrap();
        let out = Command::new(cpc).arg("check").current_dir(&dir).output().expect("cpc");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), all)
    };

    let (ok, all) =
        check("fn main() -> i32 { let s = \"x\"; return s.count() as i32; }\n");
    assert!(!ok, "str method without stdlib/str must reject");
    assert!(all.contains("E0324"), "expected E0324, got: {all}");
    assert!(
        all.contains("import \"stdlib/str\""),
        "expected the import note, got: {all}"
    );

    let (ok, all) = check(
        "import \"stdlib/str\" as str_methods;\n\
         impl str { fn mine(this) -> usize { return #str_len(this); } }\n\
         fn main() -> i32 { return 0; }\n",
    );
    assert!(!ok, "a second `impl str` must reject");
    assert!(all.contains("E0385"), "expected E0385, got: {all}");

    let (ok, all) = check(
        "import \"stdlib/str\" as str_methods;\n\
         import \"stdlib/vec\" as vec;\n\
         #[no_alloc]\n\
         fn f(s: str) -> usize {\n\
             let parts: vec::Vec[str] = s.split(separator: \",\");\n\
             return parts.count();\n\
         }\n\
         fn main() -> i32 { return f(\"a,b\") as i32; }\n",
    );
    assert!(!ok, "`#[no_alloc]` calling `split` must reject");
    assert!(all.contains("E0901"), "expected E0901, got: {all}");

    // Positive tail: `#[no_alloc]` over the pure reads stays legal.
    let (ok, all) = check(
        "import \"stdlib/str\" as str_methods;\n\
         #[no_alloc]\n\
         fn f(s: str) -> usize { return s.trim().count(); }\n\
         fn main() -> i32 { return f(\" x \") as i32; }\n",
    );
    assert!(ok, "`#[no_alloc]` over pure str reads must stay legal: {all}");
}

#[test]
fn generic_vec_slice_view_invalidation_rejected() {
    // Bug 08 (2026-07-22): a slice view of a GENERIC container (`Vec[i32]`)
    // must pin it exactly like a non-generic owner — borrowck runs pre-mono, so
    // generic receivers used to be skipped and the view could dangle after a
    // move (realloc/free) of the `Vec`. Three programs in one package:
    //  (a) `as_slice` then move the owner   → E0372
    //  (b) `as_slice` then `append` (realloc) → E0381 (iterator invalidation)
    //  (c) a read (`count`) alongside a live slice → still compiles (shared+shared)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"gv\"\n\n[[bin]]\nname = \"gv\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let head = "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/status\" as status;\n\
         fn consume(take v: vec::Vec[i32]) { return; }\n\
         fn main() -> i32 {\n\
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             let a: status::Status = v.append(10);\n\
             let s: i32[] = v.as_slice();\n";
    let check = |body: &str| -> (bool, String) {
        std::fs::write(dir.join("src/main.cplus"), format!("{head}{body}")).unwrap();
        let out = Command::new(cpc).arg("check").current_dir(&dir).output().expect("cpc");
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };

    // (a) move the owner while the slice is live → E0372
    let (ok_a, out_a) = check("    consume(v);\n    return 0;\n}\n");
    assert!(!ok_a, "moving a Vec under a live slice must be rejected");
    assert!(out_a.contains("E0372"), "expected E0372, got: {out_a}");

    // (b) mutate (append → realloc) while the slice is live → E0381
    let (ok_b, out_b) = check("    let b: status::Status = v.append(2);\n    return 0;\n}\n");
    assert!(!ok_b, "append under a live slice must be rejected");
    assert!(out_b.contains("E0381"), "expected E0381, got: {out_b}");

    // (c) a read-only method alongside a live slice is sound (shared + shared)
    let (ok_c, out_c) = check(
        "    let n: usize = v.count();\n\
         let p: *i32 = { #slice_ptr(s) as *i32 };\n    return (n as i32) + { *p };\n}\n",
    );
    assert!(ok_c, "a read alongside a live slice must compile; got: {out_c}");
}

#[test]
fn memory_model_aliasing_hardening() {
    // 2026-07-06 memory-model audit fixes, three checks in one package:
    //  (a) `h.poke(h)` — a non-Copy receiver passed again as a borrow arg
    //      in the SAME call aliases the receiver's noalias pointer → E0381
    //      (previously compiled: receiver was not an intra-call claim).
    //  (b) method `ref` args are now flag-checked (method calls used to run
    //      the intra-call walk with no flags at all).
    //  (c) `ref`-on-Copy params may alias BY DESIGN (§2.9: local
    //      mutability, not a borrow) — codegen no longer emits `noalias`
    //      for them, so `bump2(x, x)` is defined and runs.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let bad = dir.join("bad.cplus");
    std::fs::write(
        &bad,
        "struct R { opaque p: *u8 }\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         impl R { fn drop(ref this) { { free(this.p); } return; } }\n\
         struct H { r: R }\n\
         impl H {\n\
             fn poke(ref this, other: H) { return; }\n\
             fn set2(ref this, ref a: R, b: R) { return; }\n\
         }\n\
         fn main() -> i32 {\n\
             var h: H = H { r: R { p: malloc(8 as usize) } };\n\
             h.poke(h);\n\
             var v: R = R { p: malloc(8 as usize) };\n\
             h.set2(v, v);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&bad).output().expect("cpc");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "receiver/method-arg aliasing must reject");
    assert!(all.contains("E0381"), "expected E0381, got: {all}");

    let ok = dir.join("ok.cplus");
    std::fs::write(
        &ok,
        "fn bump2(ref a: i32, ref b: i32) { a = a + 1; b = b + 1; return; }\n\
         fn main() -> i32 { var x: i32 = 0; bump2(x, x); return x; }\n",
    )
    .unwrap();
    let bin = dir.join("ok");
    let st = Command::new(cpc).arg(&ok).arg("-o").arg(&bin).status().expect("cpc");
    assert!(st.success(), "Copy ref aliasing is legal (Section 2.9)");
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(run.status.code(), Some(2), "aliased increments are DEFINED");
    let ll = Command::new(cpc).arg("--emit-ll").arg(&ok).output().expect("ll");
    let ir = String::from_utf8_lossy(&ll.stdout);
    let bump_line = ir.lines().find(|l| l.contains("define") && l.contains("bump2")).unwrap();
    assert!(
        !bump_line.contains("noalias"),
        "Copy ref params must not promise noalias: {bump_line}"
    );
}

#[test]
fn moved_binding_heals_on_reassignment_in_branch() {
    // 2026-07-06 memory-model completeness: `if c { consume(v); v = mk(); }
    // use(v)` is sound — the reassignment inside the branch makes `v`
    // definitely live at the join, so E0371 no longer fires. Verified at
    // runtime with drop counters on both branch outcomes. The
    // conditional-move-then-reassign-AFTER-the-join form must KEEP firing:
    // codegen has no drop flags, so `v = mk()` on a maybe-moved binding
    // can't decide statically whether to drop the old value.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let good = dir.join("good.cplus");
    std::fs::write(
        &good,
        "static MADE: i32 = 0;\n\
         static DROPS: i32 = 0;\n\
         struct R { opaque p: *u8 }\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         extern fn time(p: *u8) -> i64;\n\
         impl R { fn drop(ref this) { DROPS = DROPS + 1; { free(this.p); } return; } }\n\
         fn consume(take r: R) { return; }\n\
         fn mk() -> R { MADE = MADE + 1; return R { p: malloc(8 as usize) }; }\n\
         fn scenario(c: bool) -> i32 {\n\
             MADE = 0;\n\
             DROPS = 0;\n\
             var v: R = mk();\n\
             if c {\n\
                 consume(v);\n\
                 v = mk();\n\
             }\n\
             consume(v);\n\
             if c {\n\
                 if MADE == 2 { if DROPS == 2 { return 0; } }\n\
                 return DROPS * 10 + MADE;\n\
             }\n\
             if MADE == 1 { if DROPS == 1 { return 0; } }\n\
             return DROPS * 10 + MADE;\n\
         }\n\
         fn main() -> i32 {\n\
             let taken: bool = time(0 as *u8) > 0 as i64;\n\
             let skipped: bool = time(0 as *u8) < 0 as i64;\n\
             let a: i32 = scenario(taken);\n\
             let b: i32 = scenario(skipped);\n\
             return a * 100 + b;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("good");
    let st = Command::new(cpc).arg(&good).arg("-o").arg(&bin).status().expect("cpc");
    assert!(st.success(), "reassignment in the moved branch must heal E0371");
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(
        run.status.code(),
        Some(0),
        "drop counts must balance on both branch outcomes"
    );

    let bad = dir.join("bad.cplus");
    std::fs::write(
        &bad,
        "struct R { opaque p: *u8 }\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         impl R { fn drop(ref this) { { free(this.p); } return; } }\n\
         fn consume(take r: R) { return; }\n\
         fn mk() -> R { return R { p: malloc(8 as usize) }; }\n\
         fn main() -> i32 {\n\
             var v: R = mk();\n\
             let c: bool = true;\n\
             if c {\n\
                 consume(v);\n\
             }\n\
             v = mk();\n\
             consume(v);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&bad).output().expect("cpc");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "maybe-moved reassignment must stay rejected");
    assert!(all.contains("E0371"), "expected E0371, got: {all}");
}

#[test]
fn take_this_receiver_is_writable() {
    // 2026-07-06 two-paths completeness: a `take this` receiver OWNS the
    // value, so writing to it is legal — the concrete method path now
    // matches the generic path (which always allowed it). Verified at
    // runtime: the reassigned field is freed exactly once by the
    // receiver's scope-exit drop. Bare `this` stays read-only.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let good = dir.join("good.cplus");
    std::fs::write(
        &good,
        "static DROPS: i32 = 0;\n\
         struct R { opaque p: *u8, tag: i32 }\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         impl R { fn drop(ref this) { DROPS = DROPS + 1; { free(this.p); } return; } }\n\
         impl R {\n\
             fn eat(take this) -> i32 {\n\
                 free(this.p);\n\
                 this.p = malloc(4 as usize);\n\
                 this.tag = this.tag + 40;\n\
                 return this.tag;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let r: R = R { p: malloc(8 as usize), tag: 2 };\n\
             let got: i32 = r.eat();\n\
             if got == 42 { if DROPS == 1 { return 0; } }\n\
             return got;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("good");
    let st = Command::new(cpc).arg(&good).arg("-o").arg(&bin).status().expect("cpc");
    assert!(st.success(), "writing through `take this` must be legal");
    let run = Command::new(&bin).output().expect("run");
    assert_eq!(run.status.code(), Some(0), "owned-receiver write + single drop");

    let bad = dir.join("bad.cplus");
    std::fs::write(
        &bad,
        "struct S { tag: i32 }\n\
         impl S { fn poke(this) { this.tag = 1; return; } }\n\
         fn main() -> i32 { let s: S = S { tag: 0 }; s.poke(); return s.tag; }\n",
    )
    .unwrap();
    let out = Command::new(cpc).arg("check").arg(&bad).output().expect("cpc");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "bare `this` must stay read-only");
    assert!(all.contains("E0305"), "expected E0305, got: {all}");
}

#[test]
fn str_view_coercion_and_free_fn_ties() {
    // Rule E-VIEW residuals closed (2026-07-06): the two view-producing
    // forms that bypass method calls now tie to their owner too —
    //  (a) bare coercion: `let s: str = t;` (owner → view, no call);
    //  (b) free-fn views: `fn head(t: Text) -> str` returns a view of its
    //      non-Copy borrow param (Rule E-VIEW-FN).
    // Moving the owner while either view is live is E0372; both forms
    // stay legal while the owner lives.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vc\"\n\n[[bin]]\nname = \"vc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    let check = |src: &str| -> (bool, String) {
        std::fs::write(dir.join("src/main.cplus"), src).unwrap();
        let out = Command::new(cpc).arg("check").current_dir(&dir).output().expect("cpc");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), all)
    };

    let (ok, all) = check(
        "import \"stdlib/text\" as text;\n\
         fn consume(take t: text::Text) { return; }\n\
         fn peek(x: str) -> i32 { return 0; }\n\
         fn main() -> i32 {\n\
             let t: text::Text = \"hello\";\n\
             let s: str = t;\n\
             consume(t);\n\
             return peek(s);\n\
         }\n",
    );
    assert!(!ok, "bare-coercion view must not outlive its owner");
    assert!(all.contains("E0372"), "expected E0372, got: {all}");

    let (ok, all) = check(
        "import \"stdlib/text\" as text;\n\
         fn head(t: text::Text) -> str { return t.view(); }\n\
         fn consume(take t: text::Text) { return; }\n\
         fn peek(x: str) -> i32 { return 0; }\n\
         fn main() -> i32 {\n\
             let t: text::Text = \"hello\";\n\
             let s: str = head(t);\n\
             consume(t);\n\
             return peek(s);\n\
         }\n",
    );
    assert!(!ok, "free-fn view must not outlive its owner");
    assert!(all.contains("E0372"), "expected E0372, got: {all}");

    let (ok, all) = check(
        "import \"stdlib/text\" as text;\n\
         fn head(t: text::Text) -> str { return t.view(); }\n\
         fn peek(x: str) -> i32 { return 0; }\n\
         fn main() -> i32 {\n\
             let t: text::Text = \"hello\";\n\
             let s: str = t;\n\
             let h: str = head(t);\n\
             let a: i32 = peek(s);\n\
             let b: i32 = peek(h);\n\
             return a + b;\n\
         }\n",
    );
    assert!(ok, "views with a live owner must stay legal: {all}");
}

#[test]
fn bound_method_reference_wires_handler_and_ctx() {
    // 2026-07-06 bound method references: `f(recv.method)` in handler
    // position becomes (synthesized erased bridge, ctx = #addr_of(recv))
    // — the callee's defaulted `*u8` param right after the handler is
    // auto-filled. Covers free-fn position, METHOD position (the facet
    // `on_click` shape: `take this` fluent + defaulted ctx), a `ref this`
    // mutating handler, and a RETURNING bound method (the mount/render
    // shape `fn(*u8) -> i64`).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"br\"\n\n[[bin]]\nname = \"br\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "struct Counter { count: i32 }\n\
         impl Counter {\n\
             fn on_inc(ref this, sender: *u8) { this.count = this.count +% 1; return; }\n\
             fn render(this) -> i64 { return this.count as i64; }\n\
         }\n\
         static COUNTER: Counter = #zero::[Counter]();\n\
         struct Btn { h: fn(*u8, *u8), opaque hctx: *u8 }\n\
         fn noop(sender: *u8, ctx: *u8) { return; }\n\
         fn btn() -> Btn { return Btn { h: noop, hctx: 0 as *u8 }; }\n\
         impl Btn {\n\
             fn on_click(take this, h: fn(*u8, *u8), ctx: *u8 = 0 as *u8) -> Btn {\n\
                 var b: Btn = this; b.h = h; b.hctx = ctx; return b;\n\
             }\n\
             fn fire(this) { let f: fn(*u8, *u8) = this.h; f(0 as *u8, this.hctx); return; }\n\
         }\n\
         fn probe(shim: fn(*u8) -> i64, ctx: *u8 = 0 as *u8) -> i64 { return shim(ctx); }\n\
         fn main() -> i32 {\n\
             let b: Btn = btn().on_click(COUNTER.on_inc);\n\
             b.fire(); b.fire(); b.fire();\n\
             let r: i64 = probe(COUNTER.render);\n\
             return r as i32;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "bound method references must compile");
    let run = Command::new(dir.join("target/debug/br"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(3), "3 fires observed by render");
}

#[test]
fn bound_method_reference_misuse_rejected() {
    // The three shape errors, one precise diagnostic each (no trailing
    // unknown-field noise): E0822 `take this` handler, E0823 signature
    // mismatch, E0824 explicit ctx alongside a bound reference.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bn\"\n\n[[bin]]\nname = \"bn\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "struct C { n: i32 }\n\
         impl C {\n\
             fn eat(take this, sender: *u8) { return; }\n\
             fn wrong(this, a: i64) -> i64 { return a; }\n\
             fn ok(ref this, sender: *u8) { this.n = this.n + 1; return; }\n\
         }\n\
         static S: C = #zero::[C]();\n\
         fn fire(h: fn(*u8, *u8), ctx: *u8 = 0 as *u8) { h(0 as *u8, ctx); return; }\n\
         fn main() -> i32 {\n\
             fire(S.eat);\n\
             fire(S.wrong);\n\
             fire(S.ok, ctx: 7 as *u8);\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr);
    assert!(!out.status.success(), "misuse must be rejected");
    for code in ["E0822", "E0823", "E0824"] {
        assert!(
            combined.contains(code),
            "expected {code} in diagnostics, got: {combined}"
        );
    }
    assert!(
        !combined.contains("E0320"),
        "a resolved bound reference owns its diagnostics — no unknown-field noise: {combined}"
    );
}

#[test]
fn cross_module_interface_conformance_and_alias_bounds() {
    // 2026-07-06: `impl T: mod::Interface` + `[C: mod::Interface]` bounds.
    // The interface lives in one module; a struct in another module
    // conforms via the import alias, and generics in BOTH modules accept
    // the conforming type through the bound. Orphan rule (E0507) passes
    // via the target-file side.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"xi\"\n\n[[bin]]\nname = \"xi\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/contract.cplus"),
        "interface Component { fn render(this) -> i64; }\n\
         fn render_of[C: Component](p: *C) -> i64 { return { (*p).render() }; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./contract\" as contract;\n\
         struct Counter { count: i32 }\n\
         impl Counter {\n\
             fn on_inc(ref this, sender: *u8) { this.count = this.count +% 1; return; }\n\
         }\n\
         impl Counter: contract::Component {\n\
             fn render(this) -> i64 { return this.count as i64; }\n\
         }\n\
         static COUNTER: Counter = #zero::[Counter]();\n\
         fn use_bound[C: contract::Component](p: *C) -> i64 { return { (*p).render() }; }\n\
         fn main() -> i32 {\n\
             let c: *Counter = #addr_of(COUNTER);\n\
             (*c).on_inc(0 as *u8);\n\
             (*c).on_inc(0 as *u8);\n\
             return (contract::render_of(c) + use_bound(c)) as i32;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cross-module conformance must compile");
    let run = Command::new(dir.join("target/debug/xi"))
        .output()
        .expect("run binary");
    assert_eq!(run.status.code(), Some(4), "2 clicks x 2 render paths");
}

#[test]
fn uncalled_generic_fn_sig_does_not_drift_static_type_ids() {
    // Statics-id-drift regression (2026-07-06, found building the facet
    // Rx prototype): an UNINSTANTIATED generic fn whose signature names a
    // generic instantiation (`fn watch[T](p: *Rx[T])`) makes sema register
    // a placeholder struct entry that codegen never emits. Every struct
    // instantiated after it then has a sema id one past its codegen id.
    // `emit_statics` used to lower `StaticInfo.ty` (sema ids) against
    // codegen's table: a static typed with a later instantiation indexed
    // out of bounds (compiler panic in `llvm_ty`) — or, with more structs
    // following, landed on the WRONG entry and miscompiled silently.
    // Statics now re-derive their type from the post-mono AST annotation
    // (monomorphize rewrites static annotations like fn signatures).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    let bin = dir.join("t");
    std::fs::write(
        &src,
        "\
struct Rx[T] { _version: u64, _value: T }
struct Item { a: i64 }
struct Holder[T] { x: T }
fn watch[T](p: *Rx[T]) -> i64 { return 0; }
static H: Holder[Item] = #zero::[Holder[Item]]();
fn main() -> i32 {
    let v: i64 = H.x.a;
    return v as i32;
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
        "static of a post-placeholder generic instantiation must compile; stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0), "zero-initialized static reads back 0");
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
        var k: f32 = 1.0f32 - r_perp.x;
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
fn ref_copy_struct_param_is_by_pointer() {
    // #9 stage 3c-copy: `ref p: Point` is an exclusive borrow that writes back,
    // so even a Copy struct is passed BY POINTER (a `Point*` out-parameter), not
    // coerced to a value. The caller's place must therefore be `var`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "\
struct Point { x: i32, y: i32 }
fn shift(ref p: Point) -> i32 { p.x = p.x + 1; return p.x; }
fn main() -> i32 {
    var v: Point = Point { x: 1, y: 2 };
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
        ir.contains("i32 @shift(ptr"),
        "Copy struct `ref` param should be pointer-passed (write-back); got: {ir}"
    );
}

// ---- Phase 6 slice 6BC.5 — explicit `borrow REGION T` syntax ----

// ---- Phase 6 borrow-region tests (v0.0.23 feature-freeze note) ----
//
// These exercise the returned-borrow / borrow-region machinery: a function
// returns a `mut`/`borrow`/`self` parameter (a borrow), and the caller's source
// is then tracked as borrowed for the result's lifetime (E0372/E0374/E0381/
// E0383 conflict detection). For a *Drop* type that pattern double-frees today —
// the returned value is an owned bitwise copy that drops alongside the source,
// and C+ has no copy constructor to make the copy real. Making the returned
// borrow non-owning is unfinished codegen; under feature freeze the unsound
// pattern is REJECTED instead: returning a borrow of a Drop type by value is
// E0337 (see `BorrowedBinding` in sema). So these now assert that the
// returned-borrow function itself is rejected (E0337), which fires before any
// conflict is reached. The region/conflict machinery remains in the compiler
// (sound for Copy borrow-shapes like `str`/`T[]`, which never drop).

#[test]
fn array_and_tuple_of_owned_values_drop_once() {
    // v0.0.23 codegen fix: building an array or tuple from owned (non-Copy)
    // bindings moves each element in — the source binding's drop must be
    // disarmed, or both the source and the aggregate element free it
    // (pre-existing `let a: [R; 2] = [p, q]` double-free: DROPS was 4). Verify
    // each element drops exactly once, ASan-clean.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("m.cplus");
    std::fs::write(
        &src,
        "static DROPS: i32 = 0;\n\
         struct R { opaque data: *u8 }\n\
         impl R { fn drop(ref this) { { DROPS = DROPS + 1; } return; } }\n\
         fn mkR() -> R { return R { data: { 0 as *u8 } }; }\n\
         fn arr() { let p: R = mkR(); let q: R = mkR(); let _a: [R; 2] = [p, q]; return; }\n\
         fn tup() { let p: R = mkR(); let q: R = mkR(); let _t: (R, R) = (p, q); return; }\n\
         fn main() -> i32 {\n\
             arr(); if { DROPS } != 2 { return 1; } { DROPS = 0; }\n\
             tup(); if { DROPS } != 2 { return 2; } { DROPS = 0; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let bin = dir.join("m");
        let mut cmd = Command::new(cpc);
        cmd.arg(&src).arg("-o").arg(&bin);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(&bin).output().expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged array/tuple element teardown ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "array/tuple elements must drop exactly once; failing phase = exit code ({sanitizer})"
        );
    }
}

#[test]
fn e3_mut_longest_pattern_compiles_cleanly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("good.cplus");
    std::fs::write(
        &src,
        "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest_mut(ref a: B, ref b: B) -> B {
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
    // v0.0.23: `longest_mut` returns a `mut` param (a borrow) by value → E0337
    // (would double-free). Rejected at the function, before any region check.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
fn longest_mut(ref a: B, ref b: B) -> B {
    if a.x > b.x { return a; }
    return b;
}
fn drain(take b: B) { return; }
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
    // v0.0.23: `longest_mut` returns a `mut` param by value → E0337, rejected
    // before the move-while-borrowed (E0372) conflict is reached.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
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
        "expected compile failure: `return a` escapes a borrowed input"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // v0.0.24 #9 stage 3e: bare params are borrows, so `return a` escapes a
    // borrowed binding — the precise error is now E0337 (borrow escape), and
    // the actionable fix is to take ownership (`take a: B`), not the old
    // region annotation (E0384 / `borrow REGION T`), which this shape no
    // longer needs. The region-suggestion path is revisited in #9 stage 4.
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
    assert!(
        stderr.contains("Take ownership by value") || stderr.contains("take"),
        "E0337 should guide toward taking ownership; got: {stderr}"
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
impl B { fn drop(ref this) { return; } }
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
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn modify_both(ref a: Inner, ref b: Inner) { return; }
fn main() -> i32 {
    var p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
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
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn write_pair(ref a: Pair, b: Inner) { return; }
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
    // Rejected as a parent+subfield borrow conflict (E0374), a partial move out
    // of a Drop aggregate (E0509), the generic borrow-escape (E0337), or — since
    // v0.0.24 #9 stage 3e, where a `ref` arg requires a `var` place — E0328.
    // All are correct refusals of `write_pair(p, p.left)`.
    assert!(
        stderr.contains("E0374")
            || stderr.contains("E0337")
            || stderr.contains("E0509")
            || stderr.contains("E0328"),
        "expected E0374 / E0337 / E0509, got: {stderr}"
    );
}

/// reports/bug-01: E0328 (a `ref` parameter writes back, so the argument must
/// be a `var` place) lived only inside `check_arg_with_move`, which the
/// CONCRETE call path uses. The generic paths hand-rolled their own argument
/// loops and never ran it, so `bump_g(y, 99)` on a frozen `let` compiled and
/// mutated it at runtime (the probe exited 99). All three generic spellings —
/// inference, turbofish, generic method — must reject exactly like the
/// concrete control, and the `var` versions must still compile and write back.
#[test]
fn generic_call_paths_enforce_ref_writable_place_e0328() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let frozen = "\
fn bump_g[T: Copy](ref x: T, v: T) { x = v; }
struct S { m: i32 }
impl S { fn setg[T: Copy](this, ref x: T, v: T) { x = v; } }
fn main() -> i32 {
    let a = 5;
    let b = 5;
    let c = 5;
    let s = S { m: 0 };
    bump_g(a, 99);
    bump_g::[i32](b, 99);
    s.setg(c, 99);
    return a + b + c - 297;
}
";
    let dir = tempdir();
    let src = dir.join("frozen.cplus");
    std::fs::write(&src, frozen).unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "a `ref` generic arg on a `let` must not compile"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.matches("E0328").count(),
        3,
        "expected one E0328 per generic spelling (inference, turbofish, method), got: {stderr}"
    );

    // The `var` versions stay legal and the write-back reaches the caller.
    let good = frozen.replace("    let a = 5;", "    var a = 5;")
        .replace("    let b = 5;", "    var b = 5;")
        .replace("    let c = 5;", "    var c = 5;");
    let gsrc = dir.join("ok.cplus");
    std::fs::write(&gsrc, good).unwrap();
    let bin = dir.join("ok");
    let compile = Command::new(cpc)
        .arg(&gsrc)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "`var` places must still pass: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).status().expect("run ok");
    assert_eq!(run.code(), Some(0), "write-back lost: {run}");
}

/// reports/bug-11: `check_generic_method_call`'s inference branch type-checked
/// every argument TWICE — once with no expected type to unify against, once
/// against the substituted parameter type. `check_expr` is side-effecting, so
/// the first pass already marked the nested `eat(r)` consuming call's operand
/// moved and the second reported a false E0335 on legal code. The inference
/// pass is now a restored probe. The second half of the test pins that a REAL
/// double use is still rejected.
#[test]
fn generic_method_inference_does_not_double_mark_moves() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let prelude = "\
struct R { n: i64 }
impl R { fn drop(ref this) { } }
fn eat(take r: R) -> i32 { return 1; }
struct S { m: i32 }
impl S { fn g[T](this, take v: T) -> T { return v; } }
";
    let dir = tempdir();
    let src = dir.join("once.cplus");
    std::fs::write(
        &src,
        format!(
            "{prelude}fn main() -> i32 {{\n\
             let s = S {{ m: 1 }};\n\
             let r = R {{ n: 2 }};\n\
             let x = s.g(eat(r));\n\
             return x - 1;\n\
             }}\n"
        ),
    )
    .unwrap();
    let bin = dir.join("once");
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "one consuming use through a generic method must compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).status().expect("run once");
    assert_eq!(run.code(), Some(0), "unexpected exit: {run}");

    let bad = dir.join("twice.cplus");
    std::fs::write(
        &bad,
        format!(
            "{prelude}fn main() -> i32 {{\n\
             let s = S {{ m: 1 }};\n\
             let r = R {{ n: 2 }};\n\
             let x = s.g(eat(r));\n\
             let y = s.g(eat(r));\n\
             return x + y;\n\
             }}\n"
        ),
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&bad)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "a real double move must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0335"), "expected E0335, got: {stderr}");
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
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn cursor(ref i: Inner) -> Inner { return i; }
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
    // v0.0.23: `cursor` returns a `mut` param by value → E0337, rejected before
    // the parent/sub-place conflict (E0374) is reached.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn cursor(ref i: Inner) -> Inner { return i; }
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
    // v0.0.23: `cursor` returns a `mut` param (a borrow) by value → E0337,
    // regardless of the disjoint sub-place; the function is rejected.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
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
    // v0.0.23: `cursor` returns a `mut` param by value → E0337, rejected before
    // the read-of-exclusively-borrowed (E0383) conflict is reached.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn drain(take c: B) { return; }
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
    // v0.0.23: `cursor` returns a `mut` param (a borrow) by value → E0337,
    // so the program is rejected at the function (before the borrow-release).
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn drain(take b: B) { return; }
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
    // v0.0.23: `cursor` returns a `mut` param by value → E0337, rejected before
    // the move-while-exclusive (E0372) conflict is reached.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
    fn drop(ref this) { return; }
    fn cursor(ref this) -> B { return this; }
}
fn peek(b: B) -> i32 { return b.x; }
fn main() -> i32 {
    var v: B = B { x: 1 };
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
    // v0.0.23: the `cursor(mut self) -> B` method returns `self` (a borrow) by
    // value → E0337, rejected before the read-while-borrowed (E0383) conflict.
    assert!(
        !out.status.success(),
        "returning `ref this` by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
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
    // v0.0.23: `cursor` returns a `mut` param (a borrow) by value → E0337.
    assert!(
        !out.status.success(),
        "returning a `mut` param by value must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0337"), "expected E0337, got: {stderr}");
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
    // an interface decl, and an `impl Type: Interface` block with a
    // matching method signature. Pre-monomorphization (7GEN.5) the
    // generic items are codegen-skipped; the concrete `main` runs.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("p7.cplus");
    std::fs::write(
        &src,
        // Slice 7GEN.6: `Ord` is now blessed; the interface body in
        // this test uses a different name to avoid the collision.
        "interface Compare { fn compare(this, other: i32) -> i32; }\n\
         struct Pair[A, B] { first: A, second: B }\n\
         enum Maybe[T] { Some(T), None }\n\
         struct Point { x: i32, y: i32 }\n\
         impl Point: Compare { fn compare(this, other: i32) -> i32 { return 0; } }\n\
         fn identity[T](take x: T) -> T { return x; }\n\
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
        "interface Two { fn a(this) -> i32; fn b(this) -> i32; }\n\
         struct P { x: i32 }\n\
         impl P: Two { fn a(this) -> i32 { return 0; } }\n\
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
        "fn identity[T](take x: T) -> T { return x; }\n\
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
        "fn id[T](take x: T) -> T { return x; }\n\
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
        "fn identity[T](take x: T) -> T { return x; }\n\
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
        "fn id[T](take x: T) -> T { return x; }\n\
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
             fn get(this) -> T { return this.value; }\n\
             fn set(ref this, v: T) { this.value = v; }\n\
         }\n\
         fn main() -> i32 {\n\
             var b: Box[i32] = Box[i32] { value: 0 };\n\
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
        "mut-this generic-typed impl should build: stderr={}",
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
             return { abs(0 -% 42) };\n\
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
         fn main() -> i32 { return { abs(7) }; }\n",
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
             return { printf(#str_ptr(fmt), 42) };\n\
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
             return {\n\
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
             return {\n\
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
         fn main() -> i32 { return { abs(0 -% 5) }; }\n",
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
             fn get(this) -> T { return this.value; }\n\
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
             fn cast[T](this, take value: T) -> T { return value; }\n\
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
fn phase7_generic_method_on_generic_struct_runs() {
    // Regression: a method-level generic (`fn id[U]`) on a GENERIC struct
    // impl (`impl Box[T]`) carries two substitutions — the struct's `T`
    // and the method's `U`. The generic-struct instantiation path used to
    // clone the method template verbatim (keeping `[U]` and an
    // unsubstituted `U` param) instead of expanding it per call, so the
    // mangled callee (`id__i32`) was never produced and codegen panicked
    // with "sema validated". `b.id::[i32](7)` must build and return 7.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gms_id.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] {\n\
             fn id[U](this, take x: U) -> U { return x; }\n\
         }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = Box[i32] { value: 0 };\n\
             return b.id::[i32](7);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("gms_id");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic method on generic struct should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "expected b.id::[i32](7) → 7");
}

#[test]
fn phase7_method_generic_interface_bound_satisfied_runs() {
    // A method-level generic with an interface bound (`fn run[U: Show]`) on a
    // generic struct, called with a satisfying type, dispatches the bound
    // method and runs. (The negative — a type not satisfying the bound — is
    // covered by sema unit tests; the bound used to be dropped during generic
    // instantiation, so this confirms the satisfying path still works.)
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("mgbound.cplus");
    std::fs::write(
        &src,
        "interface Show { fn show(this) -> i32; }\n\
         struct Box[T] { value: T }\n\
         impl Box[T] { fn run[U: Show](this, x: U) -> i32 { return x.show(); } }\n\
         struct W { n: i32 }\n\
         impl W: Show { fn show(this) -> i32 { return this.n; } }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = Box[i32] { value: 0 };\n\
             let w: W = W { n: 7 };\n\
             return b.run::[W](w);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("mgbound");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "satisfying interface bound on generic-struct method should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "b.run::[W](w) → w.show() → 7");
}

#[test]
fn phase7_generic_method_on_generic_struct_uses_both_type_params() {
    // The method body reads the struct's `T` (via `self.value`) AND takes
    // a method-`U` arg, and the same method is instantiated with two
    // different `U` on the same struct instance — exercising the combined
    // T+U substitution and multiple per-method instantiations. `get[U]`
    // ignores its `U` arg and returns `self.value` (i32 42); calling it
    // with `U = bool` then `U = i32` must both resolve and return 42.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gms_both.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] {\n\
             fn get[U](this, x: U) -> T { return this.value; }\n\
         }\n\
         fn main() -> i32 {\n\
             let b: Box[i32] = Box[i32] { value: 42 };\n\
             let a: i32 = b.get::[bool](true);\n\
             let c: i32 = b.get::[i32](0);\n\
             return a + c -% 42;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("gms_both");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic method using both T and U should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(42), "expected 42 + 42 - 42 = 42");
}

#[test]
fn phase7_generic_method_on_generic_enum_runs() {
    // Sibling of the generic-struct case: a method-level generic (`fn id[U]`)
    // on a GENERIC ENUM impl (`impl Maybe[T]`). The enum method-call path
    // used to ignore method generics entirely (empty subst → E0302 at sema);
    // it now routes through the same shared generic-method dispatch as
    // structs, and the generic-enum impl synthesis (which already covers
    // enums) produces the mangled callee. `m.id::[i32](7)` → 7.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gme_id.cplus");
    std::fs::write(
        &src,
        "enum Maybe[T] { Some(T), None }\n\
         impl Maybe[T] {\n\
             fn id[U](this, take x: U) -> U { return x; }\n\
         }\n\
         fn main() -> i32 {\n\
             let m: Maybe[i32] = Maybe[i32]::Some(0);\n\
             return m.id::[i32](7);\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("gme_id");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic method on generic enum should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "expected m.id::[i32](7) → 7");
}

#[test]
fn phase7_generic_method_on_generic_enum_two_instantiations() {
    // The same enum-method generic instantiated with two different `U` on
    // one instance — exercises per-method instantiation synthesis and both
    // turbofish resolutions on the enum path. `id::[i32](5)` then
    // `id::[bool](true)`; returns 5.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gme_two.cplus");
    std::fs::write(
        &src,
        "enum Maybe[T] { Some(T), None }\n\
         impl Maybe[T] {\n\
             fn id[U](this, take x: U) -> U { return x; }\n\
         }\n\
         fn main() -> i32 {\n\
             let m: Maybe[i32] = Maybe[i32]::Some(0);\n\
             let a: i32 = m.id::[i32](5);\n\
             let b: bool = m.id::[bool](true);\n\
             if b { return a; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("gme_two");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "enum generic method with two instantiations should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(5), "expected 5 (a, guarded by b==true)");
}

#[test]
fn phase7_generic_assoc_fn_on_generic_struct_turbofish() {
    // Regression: a generic ASSOCIATED function (no `self`) on a GENERIC
    // struct, called with a method-level turbofish:
    // `Box[i32]::make::[i32](7)`. The `Type[args]::method::[targs]` form
    // used to be a parse error (the method turbofish after the variant was
    // never accepted); the inferred form panicked codegen (un-mangled
    // method name). Now both resolve to the synthesized `make__i32`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gas_tf.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] { fn make[U](take x: U) -> U { return x; } }\n\
         fn main() -> i32 { return Box[i32]::make::[i32](7); }\n",
    )
    .unwrap();
    let bin = dir.join("gas_tf");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic assoc fn on generic struct (turbofish) should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "Box[i32]::make::[i32](7) → 7");
}

#[test]
fn phase7_generic_assoc_fn_on_generic_struct_inferred() {
    // Companion of the turbofish case: the inferred form
    // `Box[i32]::make(7)` (sema infers the method `U` from the arg). Used
    // to panic codegen because the rewrite kept the un-mangled `make`.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("gas_inf.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] { fn pick[U, V](a: U, b: V) -> V { return b; } }\n\
         fn main() -> i32 { return Box[i32]::pick(true, 7); }\n",
    )
    .unwrap();
    let bin = dir.join("gas_inf");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "generic assoc fn on generic struct (inferred) should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "Box[i32]::pick(true, 7) → 7");
}

#[test]
fn phase7_no_arg_assoc_fn_on_generic_struct_runs() {
    // A NO-ARG associated function on a generic struct: `Box[i32]::make()`.
    // Monomorphize used to lower an empty-args `Type[..]::name()` to a bare
    // variant Path (it can't tell `None` from `make()` in the AST), so codegen
    // hit `gen_path` and panicked on a struct name. Now sema marks the span as
    // an assoc-fn dispatch so it lowers to a Call.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("noarg_struct.cplus");
    std::fs::write(
        &src,
        "struct Box[T] { value: T }\n\
         impl Box[T] { fn make() -> i32 { return 7; } }\n\
         fn main() -> i32 { return Box[i32]::make(); }\n",
    )
    .unwrap();
    let bin = dir.join("noarg_struct");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "no-arg assoc fn on generic struct should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "Box[i32]::make() → 7");
}

#[test]
fn phase7_assoc_fn_on_generic_enum_runs() {
    // Associated functions on ENUMS were unsupported (`Enum[args]::name`
    // assumed a variant → E0317). Now the resolution, mono, and codegen
    // paths fall back to the enum's method table. `Maybe[i32]::make()` → 7.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("enum_assoc.cplus");
    std::fs::write(
        &src,
        "enum Maybe[T] { Some(T), None }\n\
         impl Maybe[T] { fn make() -> i32 { return 7; } }\n\
         fn main() -> i32 { return Maybe[i32]::make(); }\n",
    )
    .unwrap();
    let bin = dir.join("enum_assoc");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "assoc fn on generic enum should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "Maybe[i32]::make() → 7");
}

#[test]
fn phase7_assoc_fn_on_generic_enum_factory_self_instance() {
    // The factory pattern — an enum assoc fn that constructs and returns its
    // OWN concrete instance (`fn of(v: i32) -> Maybe[i32]`). The return type
    // names the instance being built, which created it method-less mid-
    // template-collection; the dedup-path backfill repopulates its methods.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("enum_factory.cplus");
    std::fs::write(
        &src,
        "enum Maybe[T] { Some(T), None }\n\
         impl Maybe[T] { fn of(v: i32) -> Maybe[i32] { return Maybe[i32]::Some(v); } }\n\
         fn unwrap(m: Maybe[i32]) -> i32 {\n\
             let r: i32 = match m { Maybe[i32]::Some(v) => v, Maybe[i32]::None => 0, };\n\
             return r;\n\
         }\n\
         fn main() -> i32 { return unwrap(Maybe[i32]::of(7)); }\n",
    )
    .unwrap();
    let bin = dir.join("enum_factory");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "enum assoc-fn factory returning own instance should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "Maybe[i32]::of(7) round-trip → 7");
}

#[test]
fn phase7_assoc_fn_on_nongeneric_enum_runs() {
    // Non-generic enum assoc fn `E::make()` (the 2-segment path form).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("ng_enum_assoc.cplus");
    std::fs::write(
        &src,
        "enum E { A, B }\n\
         impl E { fn make() -> i32 { return 7; } }\n\
         fn main() -> i32 { return E::make(); }\n",
    )
    .unwrap();
    let bin = dir.join("ng_enum_assoc");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "assoc fn on non-generic enum should build: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(7), "E::make() → 7");
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
             fn ident[T](take value: T) -> T { return value; }\n\
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
        "fn loose(x: This) -> i32 { return 0; }\n\
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
        "This outside impl/interface should reject"
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
             let p: *u8 = { malloc(4 as usize) };\n\
             let q: *i32 = { p as *i32 };\n\
             { *q = 42; }\n\
             return { *q };\n\
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
         fn main() -> i32 { { atexit(cleanup); } return 0; }\n",
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
// `0 as *T` is how C+ expresses FFI null without adding a `null` keyword to the
// language.

#[test]
fn phase11_int_to_ptr_cast_compiles() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("int_to_ptr.cplus");
    std::fs::write(
        &src,
        "extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             let null_ptr: *u8 = { 0 as *u8 };\n\
             { free(null_ptr); }\n\
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
    assert!(compile.success(), "0 as *u8 should compile");
    // libc's free(NULL) is a no-op per POSIX, so the binary should exit 0.
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(run.status.code(), Some(0));
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
         fn main() -> i32 { return { my_abs(0 -% 42) }; }\n",
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
         fn main() -> i32 { return { my_abs(0 -% 7) }; }\n",
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
         fn main() -> i32 { return { abs_i32(0 -% 7) + abs_again(0 -% 35) }; }\n",
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
         fn boxed[T](take v: T) -> Box[T] { return Box[T] { value: v }; }\n\
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
         fn boxed[T](take v: T) -> Box[T] { return Box[T] { value: v }; }\n\
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
         fn wrap[T](take v: T, tag: i32) -> Pair[Box[T], i32] {\n\
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
             fn new(take v: T) -> Box[T] { return Box[T] { value: v }; }\n\
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
             fn sum_first_and_b(this) -> i32 { return this.first; }\n\
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
         fn measure(take n: Bytes) -> Bytes { return n; }\n\
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

// Phase 8 owned-`string` + interpolation single-file tests removed in R4
// (string → Text). Coverage now lives in the `stdlib_text_*` project tests:
// core_api (new/with_capacity/len/is_empty/as_str), to_string_produces_owned_text,
// interpolation_produces_owned_text. Single-file owned strings no longer exist
// (Text is import-required; single-file uses `str`).

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
             let s = \"point: ${p}\";\n\
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
             let p: *u8 = { malloc(8 as usize) };\n\
             var i: usize = 0 as usize;\n\
             while i < 100 as usize {\n\
                 { *(p + i) = 42 as u8; }\n\
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
struct Point { x: i32, y: i32 }

/// Sum two integers, wrapping on overflow.
fn add(a: i32, b: i32) -> i32 { return a +% b; }

/// Internal helper — not documented (`_`-private).
fn _private(n: i32) -> i32 { return n; }
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
fn add(a: i32, b: i32) -> i32 { return a +% b; }
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

// Phase 11 owned-`string` Drop ASan tests removed in R4 (string → Text).
// Text drop is ASan-verified by the `stdlib_text_*` project tests (literal/
// return/field/arg construction, slice/split, and the Vec[Text] drop all run
// clean under --asan).

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
    var acc: i32 = 0;
    var i: usize = 0 as usize;
    while i < n {
        acc = acc +% { *(p + i) };
        i = i +% 1 as usize;
    }
    return acc;
}

fn main() -> i32 {
    let buf: *u8 = { malloc(16 as usize) };
    let p: *i32 = { buf as *i32 };
    {
        *(p + 0 as usize) = 10;
        *(p + 1 as usize) = 20;
        *(p + 2 as usize) = 12;
    }
    let xs: i32[] = { #slice_from_raw_parts(p, 3 as usize) };
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
    let p: *u8 = { 0 as *u8 };
    let bytes: u8[] = { #slice_from_raw_parts(p, 0 as usize) };
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn unused() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helper.cplus"),
        "fn local() -> i32 { return 7; }\n",
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
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // The driver normalises before the triple names a directory: clang reports
    // the running system (`arm64-apple-darwin25.5.0`), and a shipped artifact
    // must keep resolving after an OS upgrade. A fixture that creates
    // `lib/<raw-triple>/` would never be found, so use the same rule.
    cplus_core::target::normalize_triple(&raw)
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
        "fn answer() -> i32 { return 42; }\n",
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
fn bin_package_link_libs_warns_w0003() {
    // v0.0.20 (W0003): a `[[bin]]` package's own `[link] libs`/`frameworks`
    // are dead (read only when the package is a *dependency*). Declaring them
    // must warn and point to `[[bin]] libs`, but the build still succeeds
    // (the entries are simply ignored — here `boguslib` would not resolve if
    // it were actually passed to the linker).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[link]\nlibs = [\"boguslib\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
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
    assert!(
        out.status.success(),
        "build must succeed (the dead [link] libs are ignored, not linked); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("W0003"),
        "expected W0003 warning, got: {stderr}"
    );
    assert!(
        stderr.contains("[[bin]] libs"),
        "warning should point to `[[bin]] libs`: {stderr}"
    );
}

#[test]
fn dep_walk_links_bundled_static_lib_end_to_end() {
    // Full bundled-artifact path: vendor ships a real `.a` at
    // `lib/<host>/libtiny.a`; consumer's C+ source declares an extern
    // fn matching the C symbol, calls it, and the dep walk wires the
    // archive into the link line.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();

    // 1. Build a tiny static archive from C, deposit at the vendor path.
    let lib_dir = dir.join("vendor/tiny/lib").join(&host);
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
            "[package]\nname = \"tiny\"\n\n[link]\nbundled = [\"libtiny.a\"]\n"
        ),
    ).unwrap();
    std::fs::create_dir_all(dir.join("vendor/tiny/src")).unwrap();
    std::fs::write(
        dir.join("vendor/tiny/src/api.cplus"),
        "fn double(n: i32) -> i32 { return { tiny_double(n) }; }\n\
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
fn dep_link_expands_env_var_in_extra_objects_end_to_end() {
    // v0.0.20: a `[link]` path may reference `${VAR}` so a vendor binding can
    // point at an external SDK via the environment instead of a hardcoded
    // absolute path. Build a `.o` into an out-of-tree dir, point a dep's
    // `extra-objects` at it through `${CPLUS_E2E_OBJDIR}`, and confirm the
    // dep walk expands the var and links the object. Uses an object file
    // (not `-l<name>`) so the test is portable: no platform archive-naming.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    // 1. Compile a tiny object into an `objs/` subdir (not the manifest root).
    let objs_dir = dir.join("objs");
    std::fs::create_dir_all(&objs_dir).unwrap();
    let c_src = dir.join("extra_src.c");
    std::fs::write(&c_src, "int extra_answer(void) { return 7; }\n").unwrap();
    let obj = objs_dir.join("extra.o");
    let cc = Command::new("clang")
        .arg("-c")
        .arg(&c_src)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("invoke clang -c");
    assert!(cc.success(), "clang -c on extra_src.c failed");

    // 2. Vendor manifest references the object via an env var.
    std::fs::create_dir_all(dir.join("vendor/mathy/src")).unwrap();
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\n\n[link]\nextra-objects = [\"${CPLUS_E2E_OBJDIR}/extra.o\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/api.cplus"),
        "fn answer() -> i32 { return { extra_answer() }; }\n\
         extern fn extra_answer() -> i32;\n",
    )
    .unwrap();

    // 3. Consumer.
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\nmathy = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"mathy/api\" as m;\nfn main() -> i32 { return m::answer(); }\n",
    )
    .unwrap();

    // 4. Build with CPLUS_E2E_OBJDIR set in the child env (no global mutation).
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .env("CPLUS_E2E_OBJDIR", objs_dir.to_string_lossy().into_owned())
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "build with ${{CPLUS_E2E_OBJDIR}} set should link"
    );
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(7), "expected extra_answer() == 7");

    // 5. Same build with the var UNSET → E0865 before reaching the linker.
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .env_remove("CPLUS_E2E_OBJDIR")
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "build must fail when the var is unset"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0865"), "expected E0865, got: {stderr}");
    assert!(
        stderr.contains("CPLUS_E2E_OBJDIR"),
        "diagnostic should name the variable: {stderr}"
    );
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
    // The slice directory for this triple EXISTS, so the manifest is truth
    // inside it — and the file it declares is absent → E0860. (Without the
    // directory the package would simply resolve to source; that is the
    // `slice_for_another_triple_only_falls_back_to_source` case.)
    std::fs::create_dir_all(dir.join("vendor/foo/lib").join(&host)).unwrap();
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libmissing.a\"]\n",
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
    assert!(stderr.contains("E0860"), "expected E0860, got: {stderr}");
    assert!(
        stderr.contains("libmissing.a"),
        "diagnostic should name the file: {stderr}"
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
    // Compile the C source into a dylib (libtiny.dylib) — the realistic shape
    // for generated bindings (e.g. llama.cpp links libllama.dylib), and
    // order-independent at link time. `@rpath` install-name + cpc's
    // `-Wl,-rpath,<search-path>` make it resolvable at run time.
    let lib = dir.join("libtiny.dylib");
    let st = Command::new("clang")
        .arg("-dynamiclib")
        .arg("-install_name")
        .arg("@rpath/libtiny.dylib")
        .arg(&c_src)
        .arg("-o")
        .arg(&lib)
        .status()
        .expect("invoke clang -dynamiclib");
    assert!(st.success(), "clang -dynamiclib failed");

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
    // cpc-bindgen emits each C function as a `#[link_name]` extern (`__c_<name>`)
    // plus a safe `pub fn <name>` wrapper that calls it in `unsafe` — so callers
    // get a safe surface and the raw extern stays private.
    assert!(
        bindings.contains("#[link_name = \"add_ints\"]"),
        "{bindings}"
    );
    assert!(
        bindings.contains("extern fn __c_add_ints(a: i32, b: i32) -> i32;"),
        "{bindings}"
    );
    assert!(
        bindings.contains("fn add_ints(a: i32, b: i32) -> i32 {"),
        "{bindings}"
    );
    assert!(
        bindings.contains("extern fn __c_max_u32(a: u32, b: u32) -> u32;"),
        "{bindings}"
    );
    assert!(
        bindings.contains("fn max_u32(a: u32, b: u32) -> u32 {"),
        "{bindings}"
    );
    assert!(
        bindings.contains("extern fn __c_count_bytes(s: *i8) -> i64;"),
        "{bindings}"
    );
    assert!(
        bindings.contains("fn count_bytes(s: *i8) -> i64 {"),
        "{bindings}"
    );
    assert!(
        bindings.contains("extern fn __c_area_of_rect(w: f64, h: f64) -> f64;"),
        "{bindings}"
    );
    assert!(
        bindings.contains("fn area_of_rect(w: f64, h: f64) -> f64 {"),
        "{bindings}"
    );

    // Consume the bindings the way generated bindings are actually used: as an
    // imported module. The safe `pub fn` wrappers are then module-mangled, so
    // they don't collide with the bare `#[link_name]` extern symbols (inlining
    // the bindings into one file would make `add_ints` the wrapper and the
    // link-name clash). Build a package that links libtiny.a via `[link]`.
    let _ = lib; // libtiny.dylib is linked by name (`libs`) + search-path below
                 // The consumer's own libs go on `[[bin]]`; `[link]` supplies its
                 // search-paths (and `-Wl,-rpath` so the dylib resolves at run time).
    std::fs::write(
        dir.join("Cplus.toml"),
        format!(
            "[package]\nname = \"bgtiny\"\n\n[[bin]]\nname = \"bgtiny\"\npath = \"src/main.cplus\"\nlibs = [\"tiny\"]\n\n[link]\nsearch-paths = [\"{}\"]\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/api.cplus"), bindings.as_ref()).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./api\" as api;\n\
         fn main() -> i32 {\n\
         \x20   let s: str = \"hello\\0\";\n\
         \x20   let p: *i8 = { #str_ptr(s) as *i8 };\n\
         \x20   if api::count_bytes(p) != (5 as i64) { return 1; }\n\
         \x20   if api::add_ints(20 as i32, 22 as i32) != (42 as i32) { return 2; }\n\
         \x20   if api::max_u32(7 as u32, 11 as u32) != (11 as u32) { return 3; }\n\
         \x20   if api::area_of_rect(3.0 as f64, 4.0 as f64) != (12.0 as f64) { return 4; }\n\
         \x20   return 0;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(st.success(), "cpc build of bindgen round-trip failed");
    let run = Command::new(dir.join("target/debug/bgtiny"))
        .status()
        .expect("run");
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
             var x: i32 = 10 as i32;\n\
             x += 5 as i32;            // 15\n\
             x -= 2 as i32;            // 13\n\
             x *= 2 as i32;            // 26\n\
             x /= 3 as i32;            // 8\n\
             x %= 5 as i32;            // 3\n\
             var b: u32 = 0xff as u32;\n\
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
/// dispatches to that type's `impl T: Ord` method.
fn generic_max_with_ord_bound_calls_cmp_in_body() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("max.cplus");
    std::fs::write(
        &src,
        "\
struct Point { x: i32, y: i32 }
impl Point: Ord {
    fn cmp(this, other: Point) -> i32 {
        if this.x < other.x { return 0 -% 1; }
        if this.x > other.x { return 1; }
        return 0;
    }
}
fn max[T: Ord + Copy](a: T, b: T) -> T {
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
/// Regression: `Self` nested inside a fn-pointer parameter type. An
/// interface method `fn apply(self, f: fn(Self) -> i32) -> i32` whose impl
/// writes the same param as `fn(P) -> i32` used to be rejected (false
/// E0505) because the `Self`-substitution helper stopped at the top level
/// and never recursed into `FnPtr`. With the recursion fixed, the fn
/// pointer flows through generic dispatch (`call[T: Apply]`) and the
/// indirect call runs. End-to-end value: `call::[P](p, read)` =
/// `p.apply(read)` = `read(p)` = `p.x` = 7.
fn interface_self_in_fn_ptr_through_generic_dispatch() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("applyself.cplus");
    std::fs::write(
        &src,
        "\
struct P { x: i32 }
interface Apply { fn apply(this, f: fn(This) -> i32) -> i32; }
impl P: Apply {
    fn apply(this, f: fn(P) -> i32) -> i32 { return f(this); }
}
fn read(p: P) -> i32 { return p.x; }
fn call[T: Apply](t: T, f: fn(T) -> i32) -> i32 { return t.apply(f); }
fn main() -> i32 {
    let p: P = P { x: 7 };
    return call::[P](p, read);
}
",
    )
    .unwrap();
    let bin = dir.join("applyself");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for Self-in-fn-ptr interface"
    );
    let run = Command::new(&bin).status().expect("run applyself");
    assert_eq!(run.code(), Some(7), "call::[P](p, read) should be 7 (p.x)");
}

#[test]
/// Companion: `Self` nested inside a generic *instantiation*. Interface
/// `fn wrap(self) -> Holder[Self]`, impl returns `Holder[P]`. The match
/// compares the instantiation by origin (name + recursive args) so the
/// buried `Self` substitutes, then the value flows back through a generic
/// `run[T: Wrap]`. `run::[P](p).v.x` = 9.
fn interface_self_in_generic_instantiation_return() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("holderself.cplus");
    std::fs::write(
        &src,
        "\
struct Holder[A] { v: A }
struct P { x: i32 }
interface Wrap { fn wrap(this) -> Holder[This]; }
impl P: Wrap {
    fn wrap(this) -> Holder[P] { return Holder[P] { v: this }; }
}
fn run[T: Wrap](t: T) -> Holder[T] { return t.wrap(); }
fn main() -> i32 {
    let p: P = P { x: 9 };
    let h: Holder[P] = run::[P](p);
    return h.v.x;
}
",
    )
    .unwrap();
    let bin = dir.join("holderself");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for Self-in-generic-instantiation"
    );
    let run = Command::new(&bin).status().expect("run holderself");
    assert_eq!(run.code(), Some(9), "run::[P](p).v.x should be 9");
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
        format!(
            "{}{}",
            BUF_PRELUDE,
            "\
fn echo(take x: Buf) -> Buf {
    return x;
}
fn main() -> i32 {
    let s: Buf = mk_buf();
    let t: Buf = echo(s);
    if t.len() != (4 as usize) { return 1 as i32; }
    return 0 as i32;
}
"
        ),
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
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for SIMD convert/reinterpret e2e"
    );
    let run = Command::new(&bin).status().expect("run conv");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD convert/reinterpret failed; exit {:?}",
        run.code()
    );
}

/// Negative: the SIMD Tier-1 conversions reject shape mismatches with E0324.
#[test]
fn simd_convert_rejects_shape_mismatches() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let cases: &[(&str, &str)] = &[
        // from_int needs an int source of the same lane width
        (
            "from_int_lane_width",
            "let a: i16x8 = i16x8::splat(1i16); let _b: f32x4 = f32x4::from_int(a);",
        ),
        // from_int target must be float
        (
            "from_int_int_target",
            "let a: i32x4 = i32x4::splat(1); let _b: i32x4 = i32x4::from_int(a);",
        ),
        // from_float target must be int
        (
            "from_float_float_target",
            "let a: f32x4 = f32x4::splat(1.0f32); let _b: f32x4 = f32x4::from_float(a);",
        ),
        // reinterpret needs equal total width (128 vs 256 bits)
        (
            "reinterpret_width",
            "let a: f64x4 = f64x4::splat(1.0f64); let _b: i8x16 = i8x16::reinterpret(a);",
        ),
    ];
    for (label, body) in cases {
        let src = dir.join(format!("{label}.cplus"));
        std::fs::write(&src, format!("fn main() -> i32 {{ {body} return 0; }}\n")).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("invoke cpc");
        assert!(!out.status.success(), "{label}: expected rejection");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("E0324"),
            "{label}: expected E0324, got:\n{stderr}"
        );
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
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for SIMD low/high/combine/widen/narrow"
    );
    let run = Command::new(&bin).status().expect("run halves");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD half/widen/narrow failed; exit {:?}",
        run.code()
    );
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
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for widening dot product");
    let run = Command::new(&bin).status().expect("run qdot");
    assert_eq!(
        run.code(),
        Some(0),
        "widening dot product wrong; exit {:?}",
        run.code()
    );
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
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed for SIMD table lookup");
    let run = Command::new(&bin).status().expect("run tbl");
    assert_eq!(
        run.code(),
        Some(0),
        "SIMD table lookup wrong; exit {:?}",
        run.code()
    );
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
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "W0001 is a warning — must not fail the build"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("W0001"),
        "expected W0001 warning, got:\n{stderr}"
    );

    // The correct widening sum (i32x4) must NOT warn.
    let ok = dir.join("ok.cplus");
    std::fs::write(
        &ok,
        "fn main() -> i32 { let a: i32x4 = i32x4::splat(5); return a.sum(); }\n",
    )
    .unwrap();
    let out2 = Command::new(cpc)
        .arg("check")
        .arg(&ok)
        .output()
        .expect("invoke cpc");
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        !stderr2.contains("W0001"),
        "i32x4 sum must not warn, got:\n{stderr2}"
    );
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
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
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
        (
            "widen_float",
            "let a: f32x4 = f32x4::splat(1.0f32); let _b = a.widen();",
        ),
        (
            "widen_64bit_lane",
            "let a: i64x2 = i64x2::splat(1i64); let _b = a.widen();",
        ),
        (
            "narrow_byte_lane",
            "let a: i8x16 = i8x16::splat(1i8); let _b = a.narrow();",
        ),
    ];
    for (label, body) in cases {
        let src = dir.join(format!("{label}.cplus"));
        std::fs::write(&src, format!("fn main() -> i32 {{ {body} return 0; }}\n")).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("invoke cpc");
        assert!(!out.status.success(), "{label}: expected rejection");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("E0324"),
            "{label}: expected E0324, got:\n{stderr}"
        );
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
/// `malloc`'d buffer. Exercises raw-pointer interop.
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
    let buf: *u8 = { malloc(16 as usize) };
    let fp: *f32 = { buf as *f32 };
    let v: f32x4 = f32x4::new(2.0f32, 4.0f32, 6.0f32, 8.0f32);
    { v.store(fp); }
    let r: f32x4 = { f32x4::load(fp) };
    let s: f32 = r.lane(0 as u32) + r.lane(1 as u32) + r.lane(2 as u32) + r.lane(3 as u32);
    { free(buf); }
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
    let bytes: *u8 = { p as *u8 };
    let b0: u8 = { bytes[0 as usize] };
    let b1: u8 = { bytes[1 as usize] };
    let b4: u8 = { bytes[4 as usize] };
    let b5: u8 = { bytes[5 as usize] };
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
    let b1: *u8 = { p1 as *u8 };
    let b2: *u8 = { p2 as *u8 };
    let v1: u8 = { b1[0 as usize] };
    let v2: u8 = { b2[0 as usize] };
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
    let b0: u8 = { p[0 as usize] };
    let b1: u8 = { p[1 as usize] };
    let b2: u8 = { p[2 as usize] };
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
    std::fs::write(
        &src,
        "\
struct V { x: f32, y: f32, z: f32 }
fn v_make(x: f32, y: f32) -> V { return V { x: x, y: y, z: 0.0f32 }; }

fn refract(dir: V, n: V, cond: bool) -> V {
    let result: V = if cond {
        v_make(3.0f32, 4.0f32)
    } else {
        let r_perp: V = V { x: dir.x + n.x, y: dir.y + n.y, z: 0.0f32 };
        var k: f32 = 1.0f32 - r_perp.x;
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
",
    )
    .unwrap();
    let bin = dir.join("mixed");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for mixed-if-arm reproducer (regression)"
    );
    let run = Command::new(&bin).status().expect("run mixed");
    assert_eq!(
        run.code(),
        Some(0),
        "mixed-if-arm reproducer expected exit 0; got {:?}",
        run.code()
    );
}

#[test]
fn block_tail_ident_non_copy_does_not_double_free() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("blkmv.cplus");
    std::fs::write(
        &src,
        format!(
            "{}{}",
            BUF_PRELUDE,
            "\
fn main() -> i32 {
    // Block-tail rebind.
    let f: Buf = {
        let inner: Buf = mk_buf();
        inner
    };
    if f.len() != (4 as usize) { return 1 as i32; }
    // Nested block-tail rebind.
    let g: Buf = {
        let outer: Buf = {
            let deep: Buf = mk_buf();
            deep
        };
        outer
    };
    if g.len() != (4 as usize) { return 2 as i32; }
    return 0 as i32;
}
"
        ),
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

/// Regression: calling a fn-pointer whose return type is a Drop-carrying
/// (non-Copy) struct used to segfault. A non-Copy struct is returned via a
/// hidden sret slot on the definition side regardless of size, but
/// `gen_indirect_call` applied sret-return handling only to Copy structs, so a
/// non-Copy return fell through to a by-value `call <ty> %f(...)` that
/// mismatched the callee's `void f(ptr sret(<ty>))` — the callee wrote through
/// the absent sret register and the program SIGBUS'd. The fix mirrors the
/// direct-call predicate `return_passes_by_sret_widened`. See
/// `bugs/fnptr-drop-sret/`. Builds and runs both normally and under `--asan`
/// (the latter also guards drop-once / no double-free on the returned value).
#[test]
fn fnptr_returning_drop_struct_no_crash() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"fpd\"\n\n[[bin]]\nname = \"fpd\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // A Drop-carrying (non-Copy) struct returned through a fn-pointer; the body
    // also bumps a field so a wrong sret slot would corrupt the value, not just
    // crash.
    std::fs::write(
        dir.join("src/main.cplus"),
        "struct R { a: i64, b: i64, c: i64 }\n\
         impl R { fn drop(ref this) { return; } }\n\
         fn make(n: i64) -> R { return R { a: n, b: n +% (1 as i64), c: n +% (2 as i64) }; }\n\
         fn main() -> i32 {\n\
             let f: fn(i64) -> R = make;\n\
             let v: R = f(13 as i64);\n\
             return (v.a +% v.b +% v.c) as i32;\n\
         }\n",
    )
    .unwrap();

    // normal: used to SIGBUS (exit 139); must now return 13+14+15 = 42.
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(st.success(), "fnptr-drop-sret build failed");
    let run = Command::new(dir.join("target/debug/fpd"))
        .status()
        .expect("run fpd");
    assert_eq!(
        run.code(),
        Some(42),
        "fn-pointer returning a Drop struct must return 42, not crash"
    );

    // under --asan: guards the sret ABI + drop-once on the returned value.
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build --asan");
    assert!(st.success(), "fnptr-drop-sret --asan build failed");
    let run = Command::new(dir.join("target/debug/fpd"))
        .status()
        .expect("run fpd asan");
    assert_eq!(
        run.code(),
        Some(42),
        "fn-pointer returning a Drop struct must be clean under ASan"
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
enum Tag { Yes, No }
impl Tag {
    fn flip(this) -> Tag {
        return match this {
            Tag::Yes => Tag::No,
            Tag::No => Tag::Yes,
        };
    }
    fn is_yes(this) -> bool {
        return match this {
            Tag::Yes => true,
            Tag::No => false,
        };
    }
}
enum Shape { Circle(i32), Square(i32) }
impl Shape {
    fn area(this) -> i32 {
        return match this {
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
             fn is_some(this) -> bool {\n\
                 return match this {\n\
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
    // lib/<host>/ — orphan, manifest-is-truth violation.
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n",
    )
    .unwrap();
    let lib_dir = dir.join("vendor/foo/lib").join(&host);
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
fn slice_for_another_triple_only_falls_back_to_source() {
    // A package ships binaries, but not for what we're building: there is no
    // `lib/<our-triple>/`. That is not an error — it means this package has
    // nothing prebuilt for us, so it compiles from `src/` like any source
    // package. The directory's existence is the whole signal, which is what
    // lets a slice be a build artifact nobody has to version.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfoo = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/foo/src")).unwrap();
    // The slice that IS present is for an alien triple. (`not-a-real-triple`
    // is deliberately nonsensical so this test stays host-agnostic.)
    let alien = dir.join("vendor/foo/lib/not-a-real-triple");
    std::fs::create_dir_all(&alien).unwrap();
    std::fs::write(alien.join("libfoo.a"), b"!<arch>\n").unwrap();
    std::fs::write(
        dir.join("vendor/foo/Cplus.toml"),
        "[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libfoo.a\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/foo/src/api.cplus"),
        "fn answer() -> i32 { return 7 as i32; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"foo/api\" as foo;\nfn main() -> i32 { return foo::answer(); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "a package with no slice for this triple must compile from source: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(7), "source-compiled dep must still work");
}

#[test]
fn unknown_manifest_key_is_rejected_not_ignored() {
    // `triples` was removed when the build started deriving the triple itself.
    // A manifest still carrying it must fail loudly: a build-policy key that
    // is silently ignored is worse than one that refuses to load, because the
    // author believes it is in effect.
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
        "[package]\nname = \"foo\"\n\n[link]\nbundled = [\"libfoo.a\"]\ntriples = [\"aarch64-apple-darwin\"]\n",
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
    assert!(
        stderr.contains("unknown field `triples`"),
        "expected the parse error to name the key, got: {stderr}"
    );
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
    // The key property for C-consumability: `export fn add` in src/lib.cplus
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
        "export fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
        "expected `fn add` to skip path-mangling; got mangled form in:\n{out}"
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
        "export fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         export fn sub(a: i32, b: i32) -> i32 { return a - b; }\n",
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
fn ref_param_writes_back_native() {
    // #9 stage 3c-copy: a Copy `ref` (scalar + Copy struct) writes back to the
    // caller's `var` place — the increment is observable after the call.
    let (_dir, bin) = compile_program(
        "fn bump(ref n: i32) { n = n +% 1; }\n\
         struct Pt { x: i32, y: i32 }\n\
         fn shift(ref p: Pt) { p.x = p.x +% 10; p.y = p.y +% 20; }\n\
         fn main() -> i32 {\n\
             var i: i32 = 5; bump(i);\n\
             var p: Pt = Pt { x: 1, y: 2 }; shift(p);\n\
             if i != 6 { return 1; }\n\
             if p.x != 11 { return 2; }\n\
             if p.y != 22 { return 3; }\n\
             return 0;\n\
         }",
        false,
    );
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "Copy ref write-back lost (1=scalar, 2/3=struct field)"
    );
}

#[test]
// POSIX-only link harness: drives clang with `-L`/`-lreflib`, `-Wl,-rpath`, and
// `LD_LIBRARY_PATH`. On Windows/MSVC `-lreflib` makes lld-link look for
// `reflib.lib`, which cpc's Windows `build` doesn't yet emit (MSVC static/import
// -lib export is an untested port path) — mirrors `c_consumer_links_static_and_
// dynamic`, which is macOS-gated. The platform-neutral `ref`→`T*` lowering this
// checks is also covered in-process by tests that pass on Windows.
#[cfg(not(windows))]
fn export_ref_param_writes_back_through_c() {
    // #9 stage 3c-copy, the C-ABI half: `export fn bump(ref n: i32)` is a C
    // out-parameter `void bump(int32_t*)`. A clang-compiled C caller passing
    // `&i` must observe the write — verifies the `ref`→`T*` lowering against the
    // real C ABI (strict-C-ABI rule), including a `#[repr(C)]` Copy struct.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"reflib\"\n\n[lib]\ncrate-type = \"both\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "#[repr(C)] struct Pt { x: i32, y: i32 }\n\
         export fn bump(ref n: i32) { n = n +% 1; }\n\
         export fn shift(ref p: Pt) { p.x = p.x +% 10; p.y = p.y +% 20; }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed");

    let c_src = dir.join("c_user.c");
    std::fs::write(
        &c_src,
        "#include <stdint.h>\n\
         extern void bump(int32_t*);\n\
         typedef struct { int32_t x, y; } Pt;\n\
         extern void shift(Pt*);\n\
         int main(void) {\n\
             int32_t i = 5; bump(&i);\n\
             Pt p = {1, 2}; shift(&p);\n\
             return (i == 6 && p.x == 11 && p.y == 22) ? 0 : 1;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("c_user");
    let libdir = dir.join("target/debug");
    let st = Command::new("clang")
        .arg(&c_src)
        .arg("-L")
        .arg(&libdir)
        .arg("-lreflib")
        // `crate-type = "both"` emits both `.a` and `.so`; the linker prefers
        // the `.so`, so embed an rpath to the lib dir so the loader resolves
        // `libreflib.so` at runtime on Linux (matching macOS's @rpath dylib).
        .arg(format!("-Wl,-rpath,{}", libdir.display()))
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang link");
    assert!(st.success(), "C link against the C+ lib failed");
    // Belt-and-suspenders for loaders that ignore rpath: also point
    // LD_LIBRARY_PATH at the lib dir.
    let run = Command::new(&bin)
        .env("LD_LIBRARY_PATH", &libdir)
        .status()
        .expect("run c_user");
    assert_eq!(
        run.code(),
        Some(0),
        "C caller must observe `ref` write-back through the pointer params"
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
        "fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
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
    std::fs::write(&src, "fn add(a: i32, b: i32) -> i32 { return a + b; }\n").unwrap();
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
    // Visibility in a library build is NAME-based: a leading `_` marks an item
    // module-private, and only those get `internal` linkage. A name-public
    // helper is part of what the archive exists to offer — keeping it internal
    // produced a valid archive that exported nothing (stdlib: 13 KB, 14
    // symbols). `-O2` may inline `_helper` away entirely, which is also fine
    // (the assertion accepts either absent or internal).
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
        "export fn pub_api(x: i32) -> i32 { return _helper(x); }\n\
         fn _helper(x: i32) -> i32 { return x +% (1 as i32); }\n",
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
    // `_helper` is module-private by name, so it must NOT be a globally-visible
    // symbol — either inlined away by LTO or carrying internal linkage. (`nm -g`
    // lists external symbols only; the Mach-O form of `_helper` is `__helper`.)
    assert!(
        !out.contains("_helper"),
        "private `_helper` leaked into nm -g output:\n{out}"
    );
}

#[test]
fn lib_target_non_pub_methods_get_internal_linkage() {
    // Same name-based rule for `impl` block methods: `_`-prefixed methods stay
    // internal, name-public ones are exported. A consumer compiles against the
    // generated header and links these definitions, so a name-public method
    // that stayed internal would be an undefined symbol at the consumer's link.
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
        "struct Counter { v: i32 }\n\
         impl Counter {\n\
           fn make() -> Counter { return Counter { v: 0 }; }\n\
           fn value(this) -> i32 { return this.v +% _bias(); }\n\
           fn _priv_bump(ref this) -> Counter { return Counter { v: this.v +% (1 as i32) }; }\n\
         }\n\
         fn _bias() -> i32 { return 0 as i32; }\n",
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
        "private method `_priv_bump` leaked into nm -g output:\n{out}"
    );
    // The flip side, and the reason the archive is worth linking: a
    // name-public method IS exported.
    assert!(
        out.contains("Counter.value"),
        "name-public method `value` must be exported from the archive:\n{out}"
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
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         export extern fn noop() { return; }\n",
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
         export struct Point { x: i32, y: i32 }\n\
         export extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }\n",
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
        "export enum Color { Red, Green, Blue }\n\
         export extern fn first() -> i32 { return 0; }\n",
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
        "export extern fn pub_api(x: i32) -> i32 { return helper(x); }\n\
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
         export extern fn my_alloc(n: usize) -> *u8 { return { malloc(n) }; }\n",
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
         export struct Vec3 { x: f32, y: f32, z: f32 }\n\
         export enum Shape { Circle, Square, Triangle }\n\
         export extern fn norm(v: Vec3) -> f32 {\n\
           return v.x * v.x + v.y * v.y + v.z * v.z;\n\
         }\n\
         export extern fn area(s: Shape, side: f64) -> f64 { return side; }\n\
         export extern fn buf_ptr(n: usize) -> *u8 { { return 0 as *u8; } }\n",
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
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
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
         export extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }\n",
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
         export extern fn sum_pair(p: Pair) -> i64 { return p.a + p.b; }\n",
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
         export extern fn sum_triple(t: Triple) -> i64 { return t.a + t.b + t.c; }\n",
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
         export extern fn make_point(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }\n",
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
         export extern fn make_triple() -> Triple { return Triple { a: 11 as i64, b: 22 as i64, c: 33 as i64 }; }\n",
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
        "export extern fn cab_add(a: i32, b: i32) -> i32 { return a + b; }\n\
         export extern fn cab_neg(x: i32) -> i32 { return -x; }\n",
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
    assert!(st.success(), "C link against export extern fn lib failed");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(42), "expected 42 from cab_add(20, 22)");
}

#[test]
fn pub_extern_fn_with_str_param_is_rejected_e0410() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("bad.cplus");
    std::fs::write(&src, "export extern fn echo(s: str) -> i32 { return 0; }\n").unwrap();
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

/// Run `bin` (with `FETCH_PORT` set) against a one-shot sidecar TCP echo
/// server on 127.0.0.1, returning the child's exit code. The server accepts
/// one connection, echoes the single byte the client sends, and lingers
/// briefly so the client's read sees the byte rather than EOF. Mirrors the
/// macOS `recipe_async_fetch_runs` harness.
#[cfg(target_os = "windows")]
fn run_against_echo_server(bin: &std::path::Path) -> i32 {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 1];
        conn.read_exact(&mut buf).expect("read");
        conn.write_all(&buf).expect("echo");
        std::thread::sleep(std::time::Duration::from_millis(40));
        drop(conn);
    });
    let out = Command::new(bin)
        .env("FETCH_PORT", port.to_string())
        .output()
        .expect("run");
    server.join().expect("server thread");
    out.status.code().unwrap_or(-1)
}

/// Copy a vendor package (its `Cplus.toml` + every `src/*.cplus`) from the repo
/// into a temp project's `vendor/`. Used by the agent-surface test, which spans
/// several packages (win32 + agent_win32 + agent_core + stdlib) — copying the
/// real sources keeps the fixture in sync without a giant `include_str!` list.
#[cfg(target_os = "windows")]
fn copy_vendor_pkg(dir: &std::path::Path, pkg: &str) {
    let repo_vendor = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vendor");
    let dst = dir.join("vendor").join(pkg);
    std::fs::create_dir_all(dst.join("src")).unwrap();
    std::fs::copy(repo_vendor.join(pkg).join("Cplus.toml"), dst.join("Cplus.toml")).unwrap();
    for entry in std::fs::read_dir(repo_vendor.join(pkg).join("src")).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().map_or(false, |e| e == "cplus") {
            std::fs::copy(&p, dst.join("src").join(p.file_name().unwrap())).unwrap();
        }
    }
}

/// v0.0.24: the `agent_win32` surface — the Windows counterpart to
/// `agent_appkit`. Builds a real win32 window (Button/Label/Edit), tags the
/// button + edit agent-exposed, then drives the bridge: `open` walks the HWND
/// tree into agent-core's identity registry, `describe` snapshots it, and the
/// gated write path runs — `click` authorizes + routes BM_CLICK into the C+
/// handler, an unknown id is `NotFound`, and `set_text` authorizes + edits +
/// bumps the optimistic-concurrency version. The encoded exit (30) confirms
/// each step; `agent_core` is reused unchanged (platform-neutral).
#[test]
#[cfg(target_os = "windows")]
fn agent_win32_describe_and_gated_actions() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"ag32\"\n\n[[bin]]\nname = \"ag32\"\npath = \"src/main.cplus\"\n\n[dependencies]\nwin32 = \"*\"\nagent_win32 = \"*\"\nagent_core = \"*\"\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    copy_vendor_pkg(&dir, "win32");
    copy_vendor_pkg(&dir, "agent_win32");
    copy_vendor_pkg(&dir, "agent_core");
    copy_vendor_pkg(&dir, "stdlib");
    std::fs::write(
        dir.join("src/main.cplus"),
        r#"import "win32/win32" as win32;
import "win32/controls" as controls;
import "agent_win32/agent_win32" as agent;
import "agent_core/surface" as surface;
import "stdlib/vec" as vec;

extern fn malloc(n: usize) -> *u8;

fn on_click(sender: *u8, user: *u8) {
    let c: *i32 = { user as *i32 };
    { *c = *c +% (5 as i32); }
    return;
}

fn main() -> i32 {
    let counter: *i32 = { malloc(4 as usize) as *i32 };
    { *counter = 0 as i32; }
    let win: win32::Window = win32::Window::new(#str_ptr("agent demo\0"), 400 as i32, 300 as i32);
    let btn: controls::Button = controls::Button::new(win.raw(), #str_ptr("Go\0"), 20 as i32, 20 as i32, 100 as i32, 30 as i32);
    btn.on_click(on_click, counter as *u8);
    let _lbl: controls::Label = controls::Label::new(win.raw(), #str_ptr("label\0"), 20 as i32, 60 as i32, 200 as i32, 20 as i32);
    let ed: controls::Edit = controls::Edit::new(win.raw(), #str_ptr("\0"), 20 as i32, 90 as i32, 200 as i32, 24 as i32);

    agent::set_agent_id(btn.raw(), #str_ptr("btn_go\0"));
    agent::set_agent_id(ed.raw(), #str_ptr("field\0"));

    var s: agent::Surface = agent::open(win.raw());
    let nodes: vec::Vec[agent::Win32Node] = s.describe();

    var r: i32 = 0;
    if surface::outcome_eq({ s.click("btn_go") }, surface::Outcome::Allowed) { r = r +% (1 as i32); }
    r = r +% { *counter };
    if surface::outcome_eq({ s.click("ghost") }, surface::Outcome::NotFound) { r = r +% (2 as i32); }
    if nodes.count() >= (4 as usize) { r = r +% (10 as i32); }
    let v0: u64 = s.text_version("field");
    if surface::outcome_eq({ s.set_text("field", "hi", v0) }, surface::Outcome::Allowed) { r = r +% (4 as i32); }
    if s.text_version("field") > v0 { r = r +% (8 as i32); }
    return r;
}
"#,
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (agent_win32)");
    let run = Command::new(dir.join("target/debug/ag32"))
        .status()
        .expect("run ag32");
    assert_eq!(
        run.code(),
        Some(30),
        "expected describe + gated click/set_text round-trip (1+5+2+10+4+8=30); got {:?}",
        run.code()
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
    assert!(
        st.success(),
        "cpc build failed for if-arm enum-ctor reproducer"
    );
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
    fn shift(this) -> P { return P { x: this.x +% 1, y: this.y +% 1 }; }
    fn keep(this) -> P { return P { x: this.x, y: this.y }; }
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
    assert!(
        st.success(),
        "cpc build failed for if-arm method-call reproducer"
    );
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
    std::fs::write(&src, "#asm(\".text\");\nfn main() -> i32 { return 0; }\n").unwrap();

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
static DROPS: i32 = 0;
struct Res { tag: i32 }
impl Res { fn drop(ref this) { { DROPS = DROPS +% 1; }; } }
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
    return { DROPS };
}
",
    )
    .unwrap();
    let bin = dir.join("ce");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for consumed-enum payload test"
    );
    let run = Command::new(&bin).status().expect("run ce");
    // Each scenario drops its payload exactly once: leak fixed (s_not_moved)
    // and no double-free on any move-out path. 4 total.
    assert_eq!(
        run.code(),
        Some(4),
        "expected 4 drops, got {:?}",
        run.code()
    );
}

// ---- v0.0.14: broad raw-ptr !Send rule + explicit Send/Sync marker impls ----

#[test]
fn marker_impl_send_compiles_and_runs_end_to_end() {
    // A raw-ptr-hiding struct is !Send by the structural rule; empty
    // `impl Handle: Send {}` re-enables it. Verifies the override flows
    // through parser + sema + codegen and runs (the impl is sema-only, with
    // no codegen).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("snd.cplus");
    std::fs::write(
        &src,
        "\
struct Handle { opaque p: *u8 }
impl Handle: Send {}
fn ship[T: Send](take v: T) -> T { return v; }
fn main() -> i32 {
    let h: Handle = Handle { p: { 7 as *u8 } };
    let q: Handle = ship::[Handle](h);
    return { q.p as usize as i32 };
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
    assert!(st.success(), "cpc build failed for impl Send program");
    let run = Command::new(&bin).status().expect("run snd");
    assert_eq!(run.code(), Some(7), "expected exit 7, got {:?}", run.code());
}

#[test]
fn raw_ptr_struct_without_override_rejected_at_compile_time() {
    // The same program without the explicit marker impl must fail to compile
    // with E0502 (Handle does not satisfy the `Send` bound).
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nosend.cplus");
    std::fs::write(
        &src,
        "\
struct Handle { opaque p: *u8 }
fn ship[T: Send](take v: T) -> T { return v; }
fn main() -> i32 {
    let h: Handle = Handle { p: { 0 as *u8 } };
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
    let owned = s.to_text();
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
             #println({ the_answer() });\n\
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
        "fn answer() -> i32 { return 42; }\n",
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
        "fn double(x: i32) -> i32 { return x +% x; }\n",
    )
    .unwrap();
    let entry = dir.join("entry.cplus");
    std::fs::write(
        &entry,
        "import \"./util\" as u;\n\
         fn main_shim() -> i32 { return u::double(21); }\n",
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
    // v0.0.24 #9 stage 3d: every `static` is mutable, so all emit as `global`
    // (.data); const items emit no global at all (substituted in `lower`).
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
        ir.contains("@IMMUTABLE_OFFSET = global i32 50"),
        "expected static emitted as global; ir was:\n{ir}"
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

// v0.0.24 #9 stage 3d: the old `const_static_mut_write_outside_unsafe_rejected`
// test is removed — there is no `static mut` and no `unsafe` gate on a static
// write (the write-accountability code was retired; access is bare). The positive rule "a static write is
// bare" is covered by the sema test `static_write_is_bare`.

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
             let _w: isize = { write(1 as i32, p, n) };\n\
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
    assert!(
        compile.success(),
        "cpc failed to compile static-str program"
    );
    let out = Command::new(&bin).output().expect("run produced binary");
    assert!(
        out.status.success(),
        "static str round-trip failed; exited {:?}",
        out.status
    );
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
             if { *p } != (27 as u8) { return 2; }\n\
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
    assert!(
        compile.success(),
        "cpc failed to compile \\xHH-in-static-str"
    );
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
             { printf(#str_ptr(\"%g %g %g %g\\n\\0\"), l0 as f64, l1 as f64, l2 as f64, l3 as f64); }\n\
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
             var t: i64 = 0;\n\
             let returned: i64 = { time(#addr_of(t)) };\n\
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
             var t: i64 = 0;\n\
             let _r: i64 = { time(#addr_of(t)) };\n\
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
    let calls_time_with_t_addr = ir.lines().any(|l| l.contains("call i64 @time(ptr %t"));
    assert!(
        calls_time_with_t_addr,
        "expected `call i64 @time(ptr %t<suffix>)` — the alloca pointer fed \
         directly with no intermediate; got ir:\n{ir}"
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
         [profile.realtime]\ndeny-alloc = true\ndeny-block = true\nstack-limit = 4096\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "extern fn malloc(n: usize) -> *u8;\n\
         fn hot() -> *u8 { return { malloc(64 as usize) }; }\n\
         fn main() -> i32 { let _p: *u8 = hot(); return 0; }",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc check");
    assert!(
        !out.status.success(),
        "profile must reject local allocation"
    );
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
         [profile.realtime]\ndeny-alloc = true\ndeny-block = true\nstack-limit = 4096\n",
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
    assert!(
        status.success(),
        "clean realtime program must pass cpc check"
    );
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
             fn grow(ref this) { { this.ptr = malloc(64 as usize); } return; }\n\
         }\n\
         #[no_alloc]\n\
         fn hot(ref b: Bag) { b.grow(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "allocating method via receiver must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0901"), "expected E0901, got:\n{stderr}");
    assert!(
        stderr.contains("Bag::grow"),
        "diagnostic should name the method, got:\n{stderr}"
    );
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
             fn bump(ref this) { this.v = this.v +% 1; return; }\n\
         }\n\
         #[no_alloc]\n\
         fn hot(ref c: Ctr) { c.bump(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
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
         fn hot(n: i32) { let _s = n.to_text(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "to_string in #[no_alloc] must be rejected"
    );
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
             fn take(this) { { let _r: i32 = pthread_mutex_lock(this.h); } return; }\n\
         }\n\
         #[no_block]\n\
         fn hot(l: Lock) { l.take(); return; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "blocking method via receiver must be rejected"
    );
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
/// rejected with E0911 (literal-only). Elements coerce to the declared element
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
/// is still E0911 (consts are inlined at use sites; arrays belong in `static`).
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
    assert!(
        !out.status.success(),
        "const array initializer must be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0911"), "expected E0911, got: {stderr}");
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
        "static TABLE: [i32; 16] = #zero::[[i32; 16]]();\n\
         fn fill() {\n\
             var i: usize = 0 as usize;\n\
             while i < (16 as usize) {\n\
                 { TABLE[i] = (i as i32) *% (2 as i32); };\n\
                 i = i +% (1 as usize);\n\
             }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             fill();\n\
             let v: i32 = { TABLE[5 as usize] };\n\
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
    assert!(
        !out.status.success(),
        "undefined indexed write must be rejected"
    );
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
             var arr: [f16; 2] = [0.0f32 as f16, 0.0f32 as f16];\n\
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
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
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
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc check");
    assert!(
        !out.status.success(),
        "from_bits with float arg must be rejected"
    );
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
        "struct Point { x: i32, y: i32 }\n\
         impl Point {\n\
             fn sum(this) -> i32 { return this.x +% this.y; }\n\
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
    assert!(
        out.status.success(),
        "cpc graph exited non-zero: {}",
        out.status
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"nodes\""), "missing nodes array: {s}");
    assert!(s.contains("\"edges\""), "missing edges array: {s}");
    assert!(s.contains("\"name\": \"Point\""), "missing Point node: {s}");
    assert!(
        s.contains("\"name\": \"sum\""),
        "missing sum method node: {s}"
    );
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
    assert!(
        def.status.success(),
        "query def Point should find the symbol"
    );
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
    assert!(
        m.contains("\"name\": \"x\""),
        "members missing field x: {m}"
    );
    assert!(
        m.contains("\"name\": \"sum\""),
        "members missing method sum: {m}"
    );
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
    assert!(
        !bad.status.success(),
        "malformed position must exit non-zero"
    );
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
    assert!(
        c.contains("\"name\": \"main\""),
        "main should call sum: {c}"
    );
    assert!(
        c.contains("\"unresolved\""),
        "callers carries unresolved count: {c}"
    );

    let callees = Command::new(cpc)
        .args(["query", "callees", "main"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query callees");
    assert!(callees.status.success());
    let ce = String::from_utf8_lossy(&callees.stdout);
    assert!(
        ce.contains("\"name\": \"sum\""),
        "callees of main include sum: {ce}"
    );
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
    assert!(
        s.contains("\"in_context\""),
        "a reference carries its enclosing item: {s}"
    );
    assert!(
        s.contains("\"line\""),
        "a reference carries a location: {s}"
    );

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
    assert!(
        c.contains("\"name\": \"mid\""),
        "mid should call helper: {c}"
    );
    assert!(
        c.contains("\"unresolved\": 0"),
        "free calls must resolve, not land in unresolved: {c}"
    );
    // refs(helper) finds both call sites.
    let refs = Command::new(cpc)
        .args(["query", "refs", "helper"])
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query refs");
    let r = String::from_utf8_lossy(&refs.stdout);
    assert_eq!(
        r.matches("\"line\"").count(),
        2,
        "two call sites of helper: {r}"
    );
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
    assert!(
        s.contains("\"unresolved\": 1"),
        "fn-pointer call is the unresolved floor: {s}"
    );
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
    assert!(
        s.contains("\"target\""),
        "context carries the target node: {s}"
    );
    assert!(s.contains("\"callees\""), "context carries callees: {s}");
    assert!(
        s.contains("\"name\": \"sum\""),
        "main's callee sum appears: {s}"
    );

    let u = Command::new(cpc)
        .args(["query", "context", "Point"]) // a struct, not a fn → not found
        .current_dir(&dir)
        .output()
        .expect("invoke cpc query context");
    assert!(
        !u.status.success(),
        "context of a non-function exits non-zero"
    );
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
    assert!(
        text.contains("\"name\": \"main\""),
        "main calls sum: {text}"
    );
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
             { printf(m); }\n\
             { printf(c\"n=%d\\n\", 7 as i32); }\n\
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
             { printf(c\"%.3f\\n\", x as f64); }\n\
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
             var acc: i32 = SUN.color +% (SUN.radius as i32);\n\
             // sum of radii (1+3+4)=8, sum of colors (1+2+3)=6, z of [2]=5\n\
             var i: i32 = 0;\n\
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
    assert!(
        compile.success(),
        "struct-literal-static program must compile"
    );
    let run = Command::new(&bin).output().expect("run produced binary");
    assert_eq!(run.status.code(), Some(121), "expected exit 121");
}

// A struct-literal static with a non-literal field value is rejected (E0911),
// and the generic struct-literal form is excluded.
#[test]
fn struct_literal_static_non_literal_field_rejected_e0911() {
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
    assert!(stderr.contains("E0911"), "expected E0911, got: {stderr}");
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
             var s: i32 = 0;\n\
             var i: i32 = 0;\n\
             while i < (CAP as i32) { s = s +% buf[i as usize]; i = i +% 1; }\n\
             return s;\n\
         }\n\
         fn main() -> i32 {\n\
             let a: [i32; CAP] = [2; CAP];\n\
             let g: Grid = Grid { cells: [1; CAP] };\n\
             var total: i32 = sum(a);\n\
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

// An unknown const-name array length is rejected with E0912.
#[test]
fn unknown_const_array_length_rejected_e0912() {
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
    assert!(stderr.contains("E0912"), "expected E0912, got: {stderr}");
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
    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("cpc");
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
         [profile.realtime]\ndeny-alloc = true\ndeny-block = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "extern fn malloc(n: usize) -> *u8;\n\
         fn bad() -> i32 { let p: *u8 = { malloc(8 as usize) }; if p.is_null() { return 1; } return 0; }\n\
         fn main() -> i32 { return bad(); }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--realtime-report=json")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc --realtime-report=json");
    // Non-zero: violations present (CI gate).
    assert!(
        !out.status.success(),
        "expected non-zero exit on violations"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"kind\": \"realtime-report\""),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("E0901"),
        "expected a no_alloc violation; stdout:\n{stdout}"
    );
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
         [profile.realtime]\ndeny-alloc = true\ndeny-block = true\nstack-limit = 4096\n",
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
    assert!(
        stdout.contains("functions under contract: 2"),
        "stdout:\n{stdout}"
    );
}

/// TEXT.1 retired: ordinary functions and methods that used to be `unsafe fn`
/// now compile and run directly.
#[test]
fn formerly_unsafe_fn_compiles_and_runs_directly() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("unsafe_ok.cplus");
    std::fs::write(
        &src,
        "struct Counter { n: i32 }\n\
         impl Counter { fn raw_get(this) -> i32 { return this.n; } }\n\
         fn danger() -> i32 { return 42; }\n\
         fn main() -> i32 {\n\
             let c: Counter = Counter { n: 7 };\n\
             let a: i32 = { c.raw_get() };\n\
             let b: i32 = { danger() };\n\
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
    assert!(status.success(), "cpc must compile direct-call program");
    let run = Command::new(&bin).status().expect("run binary");
    assert_eq!(run.code(), Some(49), "7 + 42 should reach the exit code");
}

// ---- v0.0.21 multi-backend slices 1-2: --target plumbing + iOS object emission ----
//
// `cpc --target NAME` selects a named TargetSpec (host, ios-arm64,
// ios-arm64-simulator). External-builder targets stop at object emission —
// cpc never runs their final link — and bundled vendor artifacts resolve by
// the *selected* target's artifact triple instead of the host's.

/// Probe: can the resolved clang emit an arm64-apple-ios object from IR?
/// True for Apple clang, Homebrew clang, and the full LLVM builds Linux and
/// Windows CI install; false only for a clang built without the AArch64
/// backend or Mach-O support. Tests that need clang to *consume* an iOS
/// target skip (loudly) when this fails; the pure-cpc assertions
/// (diagnostics, IR text, dep-walk routing) never skip.
fn clang_supports_ios_arm64() -> bool {
    let clang = std::env::var("CPC_CLANG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "clang".to_string());
    let dir = tempdir();
    let ll = dir.join("probe.ll");
    let obj = dir.join("probe.o");
    std::fs::write(&ll, "define i32 @cpc_ios_probe() {\n  ret i32 0\n}\n").unwrap();
    // `output()` (not `status()`) so the probe's clang chatter — e.g.
    // -Wincompatible-sysroot when SDKROOT points at MacOSX — stays out of
    // the test log; only the verdict matters here.
    Command::new(&clang)
        .arg("-Wno-override-module")
        .arg("-target")
        .arg("arm64-apple-ios13.0")
        .arg("-c")
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn target_unknown_name_is_rejected_with_supported_list() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("ios9000")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "unknown target must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown target `ios9000`"),
        "diagnostic must name the bad target: {stderr}"
    );
    for name in ["host", "ios-arm64", "ios-arm64-simulator"] {
        assert!(
            stderr.contains(name),
            "diagnostic must list supported target `{name}`: {stderr}"
        );
    }
}

#[test]
fn target_flag_requires_an_argument() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let out = Command::new(cpc)
        .arg("--target")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--target requires a NAME"),
        "missing-argument diagnostic expected: {stderr}"
    );
}

#[test]
fn target_ios_emit_ll_pins_triple_and_target_arch() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    // `#cpu_relax()` makes the per-arch intrinsic choice observable: an iOS
    // (arm64) build must emit the aarch64 hint even on an x86_64 host.
    std::fs::write(&src, "fn main() -> i32 { #cpu_relax(); return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("ios-arm64")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "emit-ll --target ios-arm64 must succeed"
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"arm64-apple-ios13.0\""),
        "iOS IR must pin its triple: {ir}"
    );
    assert!(
        ir.contains("call void @llvm.aarch64.hint(i32 1)"),
        "iOS IR must use the aarch64 spin hint regardless of host arch: {ir}"
    );
    assert!(
        !ir.contains("llvm.x86.sse2.pause"),
        "iOS IR must not reference x86 intrinsics: {ir}"
    );
    assert!(
        !ir.contains("@_setmode"),
        "iOS IR must not carry the Windows binary-mode ctor: {ir}"
    );

    // The `--target=NAME` spelling and the simulator triple.
    let out = Command::new(cpc)
        .arg("--target=ios-arm64-simulator")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"arm64-apple-ios13.0-simulator\""),
        "simulator IR must pin the -simulator triple: {ir}"
    );
}

#[test]
fn target_host_is_default_and_byte_identical_to_explicit_host() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 41 + 1; }\n").unwrap();
    let default_out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    let host_out = Command::new(cpc)
        .arg("--target")
        .arg("host")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(default_out.status.success() && host_out.status.success());
    // Host-preserving exit criterion: `--target host` is today's behavior,
    // byte-for-byte, and neither form pins an IR triple.
    assert_eq!(
        default_out.stdout, host_out.stdout,
        "--target host must match the default output exactly"
    );
    let ir = String::from_utf8_lossy(&default_out.stdout);
    assert!(
        !ir.contains("target triple"),
        "host IR must not pin a triple (clang's default applies): {ir}"
    );
}

#[test]
fn target_ios_emit_obj_produces_macho_arm64_object() {
    if !clang_supports_ios_arm64() {
        eprintln!("skipping: clang lacks arm64-apple-ios object support");
        return;
    }
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    for (target, obj_name) in [("ios-arm64", "t_ios.o"), ("ios-arm64-simulator", "t_sim.o")] {
        let obj = dir.join(obj_name);
        let out = Command::new(cpc)
            .arg("--target")
            .arg(target)
            .arg("--emit-obj")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .output()
            .expect("invoke cpc");
        assert!(
            out.status.success(),
            "--emit-obj --target {target} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bytes = std::fs::read(&obj).expect("read emitted object");
        // Mach-O 64-bit little-endian magic, then cputype CPU_TYPE_ARM64
        // (0x0100000c) — both as they appear on disk.
        assert!(
            bytes.len() > 8,
            "object for {target} is implausibly small ({} bytes)",
            bytes.len()
        );
        assert_eq!(
            &bytes[0..4],
            &[0xcf, 0xfa, 0xed, 0xfe],
            "object for {target} must be 64-bit Mach-O"
        );
        assert_eq!(
            &bytes[4..8],
            &[0x0c, 0x00, 0x00, 0x01],
            "object for {target} must target arm64"
        );
    }
}

#[test]
fn target_ios_single_file_binary_is_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let out = Command::new(cpc)
        .arg(&src)
        .arg("--target")
        .arg("ios-arm64")
        .arg("-o")
        .arg(dir.join("t.bin"))
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "host-link path must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stops at object emission"),
        "rejection must explain the external-builder handoff: {stderr}"
    );
    assert!(
        stderr.contains("--emit-obj"),
        "rejection must point at the supported flows: {stderr}"
    );
}

#[test]
fn target_ios_bin_project_build_is_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--target")
        .arg("ios-arm64")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "[[bin]] + external-builder target must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`[[bin]]` projects can't be built"),
        "rejection must name the [[bin]] restriction: {stderr}"
    );
    assert!(
        stderr.contains("staticlib"),
        "rejection must point at the [lib] staticlib flow: {stderr}"
    );
}

#[test]
fn target_ios_cdylib_crate_type_is_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"shaky\"\n\n[lib]\nname = \"shaky\"\npath = \"src/lib.cplus\"\ncrate-type = \"cdylib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "export extern fn answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--target")
        .arg("ios-arm64")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "cdylib needs a final link — must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cdylib"),
        "rejection must name the crate-type: {stderr}"
    );
    assert!(
        stderr.contains("staticlib"),
        "rejection must suggest staticlib: {stderr}"
    );
}

#[test]
fn target_ios_test_subcommand_is_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[test]\nfn passes() { assert 1 == 1; return; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("test")
        .arg(&src)
        .arg("--target")
        .arg("ios-arm64")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "cpc test --target must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("test binaries link and run on the host"),
        "rejection must explain why: {stderr}"
    );
}

#[test]
fn target_check_accepts_explicit_target() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    let st = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .arg("--target")
        .arg("ios-arm64")
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc check --target ios-arm64 must pass on clean source"
    );
}

#[test]
fn target_ios_staticlib_build_lands_in_per_target_tree() {
    if !clang_supports_ios_arm64() {
        eprintln!("skipping: clang lacks arm64-apple-ios object support");
        return;
    }
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"gadget\"\n\n[lib]\nname = \"gadget\"\npath = \"src/lib.cplus\"\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "export extern fn gadget_answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--target")
        .arg("ios-arm64")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "iOS staticlib build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Explicit targets build into target/<target-name>/<mode>/ so host and
    // iOS artifacts of one package never collide.
    for artifact in ["gadget.o", "libgadget.a", "gadget.h"] {
        let p = dir.join("target/ios-arm64/debug").join(artifact);
        assert!(
            p.is_file(),
            "expected {} in the per-target tree",
            p.display()
        );
    }
    // The object inside the per-target tree is an arm64 Mach-O.
    let bytes = std::fs::read(dir.join("target/ios-arm64/debug/gadget.o")).unwrap();
    assert_eq!(&bytes[0..4], &[0xcf, 0xfa, 0xed, 0xfe]);
    assert_eq!(&bytes[4..8], &[0x0c, 0x00, 0x00, 0x01]);

    // A host build of the same package keeps today's layout untouched.
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "host build of the same package must still work"
    );
    assert!(
        dir.join("target/debug/libgadget.a").is_file(),
        "host build must keep the target/<mode>/ layout"
    );
}

#[test]
fn target_dep_bundled_artifacts_resolve_by_selected_target() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    // Vendor ships a bundled archive *only* for arm64-apple-ios — the
    // stable artifact triple, not a versioned clang triple.
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\ngadget = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("vendor/gadget/src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/gadget/lib/arm64-apple-ios")).unwrap();
    std::fs::write(
        dir.join("vendor/gadget/Cplus.toml"),
        "[package]\nname = \"gadget\"\n\n[link]\nbundled = [\"libgadget.a\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/gadget/src/api.cplus"),
        "fn answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/gadget/lib/arm64-apple-ios/libgadget.a"),
        b"!<arch>\n",
    )
    .unwrap();

    // Selected target ios-arm64: the dep walk resolves by the artifact
    // triple `arm64-apple-ios` and passes. (--emit-ll-project exercises the
    // walk without needing clang, and the IR pins the iOS triple.)
    let out = Command::new(cpc)
        .arg("--target")
        .arg("ios-arm64")
        .arg("--emit-ll-project")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "ios-arm64 dep walk must accept the arm64-apple-ios bundle: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("target triple = \"arm64-apple-ios13.0\""));

    // Host target: the same package has no `lib/<host-triple>/`, so it simply
    // has nothing prebuilt for us and resolves to source. Selecting a target
    // changes which slice is looked for, not whether a missing one is fatal.
    let out = Command::new(cpc)
        .arg("--emit-ll-project")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "host build must fall back to source, not fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn target_dep_without_a_slice_for_the_selected_target_uses_source() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    // Vendor bundles a binary for some other triple only; selecting
    // ios-arm64 must fail E0862 and word it for the *target* triple.
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\ngadget = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("vendor/gadget/src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/gadget/lib/riscv32-unknown-none")).unwrap();
    std::fs::write(
        dir.join("vendor/gadget/Cplus.toml"),
        "[package]\nname = \"gadget\"\n\n[link]\nbundled = [\"libgadget.a\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/gadget/src/api.cplus"),
        "fn answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/gadget/lib/riscv32-unknown-none/libgadget.a"),
        b"!<arch>\n",
    )
    .unwrap();
    // The only slice is riscv32; building for ios-arm64 finds no
    // `lib/arm64-apple-ios/`, so `gadget` compiles from source for iOS.
    let out = Command::new(cpc)
        .arg("--target")
        .arg("ios-arm64")
        .arg("--emit-ll-project")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "a slice for another triple must not block this target: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"arm64-apple-ios13.0\""),
        "IR should still be built for the selected target"
    );
}

// ---- v0.0.21 multi-backend rung 2: android-arm64 via the NDK toolchain ----

/// Probe: resolve the Android NDK clang the way cpc does (env overrides,
/// then the SDK's default ndk/ directory, newest version, LLVM >= 19).
/// Tests that need the NDK to consume IR skip (loudly) when this returns
/// `None`; the pure-cpc assertions (IR text, diagnostics) never skip.
fn ndk_clang_for_test() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CPC_NDK_CLANG") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    let mut root: Option<std::path::PathBuf> = None;
    for var in [
        "ANDROID_NDK_HOME",
        "ANDROID_NDK_ROOT",
        "ANDROID_NDK_LATEST_HOME",
    ] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                let p = std::path::PathBuf::from(v);
                if p.is_dir() {
                    root = Some(p);
                }
                break;
            }
        }
    }
    if root.is_none() {
        let ndk_dir = if cfg!(target_os = "macos") {
            std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Android/sdk/ndk")
        } else if cfg!(windows) {
            std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                .join("Android")
                .join("Sdk")
                .join("ndk")
        } else {
            std::path::PathBuf::from(std::env::var_os("HOME")?).join("Android/Sdk/ndk")
        };
        let mut best: Option<(Vec<u64>, std::path::PathBuf)> = None;
        for entry in std::fs::read_dir(&ndk_dir).ok()?.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            let Ok(parts) = name
                .split('.')
                .map(|s| s.parse::<u64>())
                .collect::<Result<Vec<u64>, _>>()
            else {
                continue;
            };
            if path.is_dir() && best.as_ref().map_or(true, |(b, _)| parts > *b) {
                best = Some((parts, path));
            }
        }
        root = best.map(|(_, p)| p);
    }
    let root = root?;
    let host_tag = if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    };
    let clang = root
        .join("toolchains/llvm/prebuilt")
        .join(host_tag)
        .join("bin")
        .join(if cfg!(windows) { "clang.exe" } else { "clang" });
    if !clang.is_file() {
        return None;
    }
    // LLVM >= 19, same floor cpc enforces.
    let out = Command::new(&clang).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let major: u32 = text
        .split("clang version ")
        .nth(1)?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    if major < 19 {
        return None;
    }
    Some(clang)
}

#[test]
fn target_android_emit_ll_pins_triple() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    // Pure IR emission needs no NDK — the coro probe falls back to the
    // host clang when the external toolchain is absent.
    let out = Command::new(cpc)
        .arg("--target")
        .arg("android-arm64")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "emit-ll --target android-arm64 must succeed without the NDK: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"aarch64-linux-android24\""),
        "android IR must pin its triple: {ir}"
    );
    assert!(
        !ir.contains("@_setmode"),
        "android IR must not carry the Windows binary-mode ctor: {ir}"
    );
}

#[test]
fn target_android_missing_ndk_is_rejected_with_setup_hint() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    // A set-but-wrong $ANDROID_NDK_HOME is an error naming the variable,
    // never a fallback to other install locations — deterministic on every
    // host regardless of what NDKs are actually installed.
    let out = Command::new(cpc)
        .env_remove("CPC_NDK_CLANG")
        .env("ANDROID_NDK_HOME", "/nonexistent/cpc-test-ndk")
        .arg("--target")
        .arg("android-arm64")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "bad ANDROID_NDK_HOME must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ANDROID_NDK_HOME") && stderr.contains("not a directory"),
        "rejection must name the misconfigured variable: {stderr}"
    );
}

#[test]
fn target_android_emit_obj_produces_elf_aarch64_object() {
    if ndk_clang_for_test().is_none() {
        eprintln!("skipping: no Android NDK (r28.2+) found");
        return;
    }
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let obj = dir.join("t_android.o");
    let out = Command::new(cpc)
        .arg("--target")
        .arg("android-arm64")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "--emit-obj --target android-arm64 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&obj).expect("read emitted object");
    assert!(bytes.len() > 20, "object is implausibly small");
    // ELF magic, 64-bit class, then e_machine EM_AARCH64 (0xB7) at offset 18 LE.
    assert_eq!(&bytes[0..4], b"\x7fELF", "object must be ELF");
    assert_eq!(bytes[4], 2, "object must be ELFCLASS64");
    assert_eq!(
        (bytes[18], bytes[19]),
        (0xb7, 0x00),
        "object must target aarch64 (EM_AARCH64)"
    );
}

/// The full rung-2 handoff, including the archive-format lesson: the
/// staticlib must be indexed by the NDK's llvm-ar (macOS BSD ar skips ELF
/// members, leaving an archive lld resolves no symbols from), and the NDK
/// clang must link it into an Android executable.
#[test]
fn target_android_staticlib_links_under_ndk_clang() {
    let Some(ndk_clang) = ndk_clang_for_test() else {
        eprintln!("skipping: no Android NDK (r28.2+) found");
        return;
    };
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"droid\"\n\n[lib]\nname = \"droid\"\npath = \"src/lib.cplus\"\ncrate-type = \"staticlib\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.cplus"),
        "export extern fn droid_answer() -> i32 { return 42; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .arg("--target")
        .arg("android-arm64")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "android staticlib build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for artifact in ["droid.o", "libdroid.a", "droid.h"] {
        let p = dir.join("target/android-arm64/debug").join(artifact);
        assert!(
            p.is_file(),
            "expected {} in the per-target tree",
            p.display()
        );
    }
    let obj_bytes = std::fs::read(dir.join("target/android-arm64/debug/droid.o")).unwrap();
    assert_eq!(
        &obj_bytes[0..4],
        b"\x7fELF",
        "per-target object must be ELF"
    );

    std::fs::write(
        dir.join("main.c"),
        "extern int droid_answer(void);\nint main(void) { return droid_answer() == 42 ? 0 : 1; }\n",
    )
    .unwrap();
    let exe = dir.join("droid_exe");
    let link = Command::new(&ndk_clang)
        .arg("-target")
        .arg("aarch64-linux-android24")
        .arg(dir.join("main.c"))
        .arg(dir.join("target/android-arm64/debug/libdroid.a"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("invoke NDK clang");
    assert!(
        link.status.success(),
        "NDK link of the C+ staticlib failed (archive symbol index?): {}",
        String::from_utf8_lossy(&link.stderr)
    );
    assert!(exe.is_file(), "linked Android executable missing");
}

// ---- v0.0.21 multi-backend rungs 3-4: esp32-xtensa (first 32-bit target) ----

/// Probe: resolve esp-clang the way cpc does ($CPC_ESP_CLANG, $IDF_TOOLS_PATH,
/// ~/.espressif), newest version, LLVM >= 19. Object-emission tests skip
/// (loudly) without it; the pure-cpc 32-bit IR assertions never skip.
fn esp_clang_for_test() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CPC_ESP_CLANG") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    let root = match std::env::var("IDF_TOOLS_PATH") {
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => std::path::PathBuf::from(
            std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?,
        )
        .join(".espressif"),
    };
    let tool_dir = root.join("tools/esp-clang");
    let mut best: Option<(Vec<u64>, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(&tool_dir).ok()?.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        let nums: Vec<u64> = name
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        if path.is_dir() && !nums.is_empty() && best.as_ref().map_or(true, |(b, _)| nums > *b) {
            best = Some((nums, path));
        }
    }
    let clang =
        best?
            .1
            .join("esp-clang/bin")
            .join(if cfg!(windows) { "clang.exe" } else { "clang" });
    if clang.is_file() {
        Some(clang)
    } else {
        None
    }
}

#[test]
fn target_esp32_emits_32_bit_ir_with_xtensa_abi() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[repr(C)] struct V3 { x: i32, y: i32, z: i32 }\n\
         #[repr(C)] struct Big { a: i64, b: i64, c: i64, d: i64 }\n\
         extern fn c_take_v3(v: V3) -> i32;\n\
         extern fn c_take_big(b: Big) -> i64;\n\
         export extern fn use_usize(n: usize) -> usize {\n\
             let sz: usize = #size_of::[*u8]();\n\
             return n + sz;\n\
         }\n\
         export extern fn drive() -> i64 {\n\
             let v: V3 = V3 { x: 1, y: 2, z: 3 };\n\
             let b: Big = Big { a: 1 as i64, b: 2 as i64, c: 3 as i64, d: 4 as i64 };\n\
             let r1: i32 = { c_take_v3(v) };\n\
             let r2: i64 = { c_take_big(b) };\n\
             return (r1 as i64) + r2;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "emit-ll --target esp32-xtensa must succeed without esp-clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"xtensa-esp32-elf\""),
        "esp32 IR must pin its triple: {ir}"
    );
    // 32-bit pointer-sized integers: usize lowers to i32, and #size_of
    // computes through a 32-bit ptrtoint.
    assert!(
        ir.contains("define i32 @use_usize(i32"),
        "usize must lower to i32 on esp32-xtensa: {ir}"
    );
    assert!(
        ir.contains("ptrtoint ptr") && ir.contains("to i32"),
        "#size_of must fold through a 32-bit ptrtoint: {ir}"
    );
    // Empirical Xtensa shapes: 12B → [3 x i32] argument, 32B → indirect.
    assert!(
        ir.contains("declare i32 @c_take_v3([3 x i32])"),
        "12-byte aggregate must coerce to [3 x i32]: {ir}"
    );
    // The esp-clang probe pinned Xtensa's >24-byte convention as indirect
    // BYVAL (a stack copy, like x86_64-sysv) — the bring-up left the import
    // declare/call sites bare-ptr, mismatching both clang and cpc's own fn
    // definitions. Declare and call site must carry byval.
    assert!(
        ir.contains("declare i64 @c_take_big(ptr byval(%Big) align 8)"),
        "32-byte aggregate must pass indirect byval on Xtensa: {ir}"
    );
    assert!(
        ir.contains("call i64 @c_take_big(ptr byval(%Big) align 8 "),
        "the call site must carry the same byval attr as the declare: {ir}"
    );
    // No foreign-arch intrinsics in the preamble.
    assert!(
        !ir.contains("llvm.aarch64") && !ir.contains("llvm.x86"),
        "esp32 IR must not declare aarch64/x86 intrinsics: {ir}"
    );
}

#[test]
fn target_esp32_realtime_contract_holds_across_targets() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    // The headline shape: a #[realtime] control step compiles for the
    // 32-bit MCU target...
    let good = dir.join("pid.cplus");
    std::fs::write(
        &good,
        "#[repr(C)] struct PidOut { control: i32, integral: i32 }\n\
         #[realtime]\n\
         export extern fn pid_step(setpoint: i32, measured: i32, integral: i32) -> PidOut {\n\
             let err: i32 = setpoint - measured;\n\
             return PidOut { control: (205 * err) / 256, integral: integral + err };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("check")
        .arg(&good)
        .arg("--target")
        .arg("esp32-xtensa")
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "#[realtime] PID must check clean for esp32-xtensa"
    );
    // ...and the same contract rejects allocation regardless of target.
    let bad = dir.join("bad.cplus");
    std::fs::write(
        &bad,
        "extern fn malloc(n: usize) -> *u8;\n\
         #[realtime]\n\
         fn rt_with_alloc() -> *u8 {\n\
             return { malloc(64 as usize) };\n\
         }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&bad)
        .arg("--target")
        .arg("esp32-xtensa")
        .output()
        .expect("invoke cpc");
    assert!(
        !out.status.success(),
        "allocation under #[realtime] must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0901"), "expected E0901, got: {stderr}");
}

#[test]
fn target_esp32_missing_esp_clang_is_rejected_with_setup_hint() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "export extern fn f() -> i32 { return 1; }\n").unwrap();
    // Set-but-wrong $IDF_TOOLS_PATH errors naming the variable.
    let out = Command::new(cpc)
        .env_remove("CPC_ESP_CLANG")
        .env("IDF_TOOLS_PATH", "/nonexistent/cpc-test-espressif")
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("IDF_TOOLS_PATH") && stderr.contains("not a directory"),
        "rejection must name the misconfigured variable: {stderr}"
    );
    // No esp-clang anywhere: the install hint.
    let empty_home = tempdir();
    let out = Command::new(cpc)
        .env_remove("CPC_ESP_CLANG")
        .env_remove("IDF_TOOLS_PATH")
        .env("HOME", &empty_home)
        .env("USERPROFILE", &empty_home)
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("idf_tools.py install esp-clang"),
        "rejection must carry the install hint: {stderr}"
    );
}

#[test]
fn target_esp32_emit_obj_produces_xtensa_elf_object() {
    if esp_clang_for_test().is_none() {
        eprintln!("skipping: esp-clang not installed");
        return;
    }
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "export extern fn add(a: i32, b: i32) -> i32 { return a + b; }\n",
    )
    .unwrap();
    let obj = dir.join("t_esp32.o");
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "--emit-obj --target esp32-xtensa failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&obj).expect("read emitted object");
    assert!(bytes.len() > 20);
    // ELF magic, 32-bit class, e_machine EM_XTENSA (94 = 0x5e) at offset 18.
    assert_eq!(&bytes[0..4], b"\x7fELF", "object must be ELF");
    assert_eq!(
        bytes[4], 1,
        "object must be ELFCLASS32 (the first 32-bit target)"
    );
    assert_eq!(
        (bytes[18], bytes[19]),
        (0x5e, 0x00),
        "object must target Xtensa (EM_XTENSA)"
    );
}

/// v0.0.21 32-bit heap slice: fat pointers, lengths, and the libc size_t
/// surface (`malloc`/`memcpy`/`memcmp`/`snprintf`) follow the target's
/// pointer width. Pure cpc — no esp-clang needed.
#[test]
fn target_esp32_heap_ir_is_pointer_width_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
             let s: str = \"hello esp32\";\n\
             #println(s);\n\
             let n: usize = #str_len(s);\n\
             return (n as i32) - 11;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("{ ptr, i32 }"),
        "str must be a 32-bit fat pointer on esp32: {ir}"
    );
    assert!(
        !ir.contains("{ ptr, i64 }"),
        "no 64-bit fat pointers may remain in 32-bit IR: {ir}"
    );
    assert!(
        ir.contains("@malloc(i32 noundef)"),
        "malloc must declare a 32-bit size_t: {ir}"
    );
    assert!(
        !ir.contains("@malloc(i64"),
        "no 64-bit malloc declaration in 32-bit IR: {ir}"
    );

    // The same source for the host keeps the 64-bit shapes byte-for-byte.
    let out = Command::new(cpc)
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("{ ptr, i64 }"), "host str stays 64-bit: {ir}");
    assert!(
        ir.contains("@malloc(i64 noundef)"),
        "host malloc stays 64-bit: {ir}"
    );
}

/// v0.0.21 embedded profile: `async fn` is rejected on 32-bit targets at
/// check time (E0867) — the coroutine runtime is 64-bit only — and the
/// gate never fires for the host.
#[test]
fn target_esp32_async_fn_fires_e0867() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "fn helper() -> i32 { return 1; }\n\
         async fn fetch() -> i32 { return helper(); }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .arg("--target")
        .arg("esp32-xtensa")
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "async fn must be rejected on esp32");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E0867"), "expected E0867: {stderr}");
    assert!(
        stderr.contains("32-bit"),
        "E0867 must explain the 32-bit restriction: {stderr}"
    );
    // Host: whatever else this snippet needs, the 32-bit gate is silent.
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("E0867"),
        "E0867 must never fire for the host: {stderr}"
    );
}

/// v0.0.22: esp32c3-riscv32 — the mainline-LLVM 32-bit comparison point.
/// Pure-cpc IR assertions everywhere; object emission when esp-clang is
/// installed (EM_RISCV = 243).
#[test]
fn target_esp32c3_emits_rv32_ir_and_object() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(
        &src,
        "#[repr(C)] struct V3 { x: i32, y: i32, z: i32 }\n\
         extern fn c_take_v3(v: V3) -> i32;\n\
         export extern fn use_usize(n: usize) -> usize {\n\
             return n + #size_of::[*u8]();\n\
         }\n\
         export extern fn drive() -> i32 {\n\
             let v: V3 = V3 { x: 1, y: 2, z: 3 };\n\
             return { c_take_v3(v) };\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32c3-riscv32")
        .arg("--emit-ll")
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(
        ir.contains("target triple = \"riscv32-esp-elf\""),
        "C3 IR must pin its triple: {ir}"
    );
    assert!(
        ir.contains("define i32 @use_usize(i32"),
        "usize must lower to i32 on rv32: {ir}"
    );
    // RV32 ilp32: a 12-byte aggregate passes as a bare pointer (no byval,
    // unlike Xtensa's 24-byte direct window).
    assert!(
        ir.contains("declare i32 @c_take_v3(ptr)"),
        "12-byte aggregate must pass indirect on rv32: {ir}"
    );
    if esp_clang_for_test().is_none() {
        eprintln!("skipping object half: esp-clang not installed");
        return;
    }
    let obj = dir.join("t_c3.o");
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32c3-riscv32")
        .arg("--emit-obj")
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "--emit-obj for esp32c3 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&obj).unwrap();
    assert_eq!(&bytes[0..4], b"\x7fELF");
    assert_eq!(bytes[4], 1, "ELFCLASS32");
    assert_eq!((bytes[18], bytes[19]), (0xf3, 0x00), "EM_RISCV");
}

/// v0.0.22: `--min-os` overrides the OS floor baked into a versioned
/// target triple; unversioned targets and bad versions are rejected.
#[test]
fn target_min_os_overrides_versioned_triples() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    std::fs::write(&src, "fn main() -> i32 { return 0; }\n").unwrap();
    for (target, ver, expect) in [
        (
            "ios-arm64",
            "15.2",
            "target triple = \"arm64-apple-ios15.2\"",
        ),
        (
            "ios-arm64-simulator",
            "14.0",
            "target triple = \"arm64-apple-ios14.0-simulator\"",
        ),
        (
            "android-arm64",
            "28",
            "target triple = \"aarch64-linux-android28\"",
        ),
    ] {
        let out = Command::new(cpc)
            .arg("--target")
            .arg(target)
            .arg("--min-os")
            .arg(ver)
            .arg("--emit-ll")
            .arg(&src)
            .output()
            .expect("invoke cpc");
        assert!(
            out.status.success(),
            "--min-os {ver} for {target} must work"
        );
        let ir = String::from_utf8_lossy(&out.stdout);
        assert!(
            ir.contains(expect),
            "expected `{expect}` for {target}: {ir}"
        );
    }
    // Unversioned targets reject the flag with the placement hint.
    for args in [
        vec!["--min-os", "15.0"],
        vec!["--target", "esp32-xtensa", "--min-os", "9"],
    ] {
        let out = Command::new(cpc)
            .args(&args)
            .arg("--emit-ll")
            .arg(&src)
            .output()
            .expect("invoke cpc");
        assert!(
            !out.status.success(),
            "--min-os must be rejected for {args:?}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("versioned triple"),
            "rejection must explain: {stderr}"
        );
    }
    // Malformed version.
    let out = Command::new(cpc)
        .args(["--target", "ios-arm64", "--min-os", "15.x", "--emit-ll"])
        .arg(&src)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("dotted numeric"),
        "bad version must be named"
    );
}

/// Regression (v0.0.22, android_view listener): one module *declares* an
/// extern symbol as an import while another module in the same program
/// *defines* it (`pub extern fn`) — the app-provided-hook pattern. Codegen
/// used to emit both the `declare` and the `define`, which LLVM rejects as
/// a redefinition; the import declare is now skipped for program-defined
/// symbols. Host-runnable: the caller module invokes the hook through its
/// extern declaration and the result proves the call landed in the
/// definition.
#[test]
fn extern_import_of_program_defined_symbol_links_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hookapp\"\n\n[[bin]]\nname = \"hookapp\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/caller.cplus"),
        "extern fn app_hook(x: i32) -> i32;\n\
         fn call_through_hook(x: i32) -> i32 {\n\
             return { app_hook(x) };\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./caller\" as caller;\n\
         export extern fn app_hook(x: i32) -> i32 {\n\
             return x * 2 + 1;\n\
         }\n\
         fn main() -> i32 {\n\
             if caller::call_through_hook(20) != 41 { return 1; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "declare+define of one symbol must compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("target/debug/hookapp"))
        .status()
        .expect("run hookapp");
    assert_eq!(run.code(), Some(0), "hook call must reach the definition");
}

/// Regression: a `pub extern fn` wrapper tail-calling an internal fn that
/// returns the same aggregate used to emit `musttail` even though the
/// export's IR return is ABI-coerced (`[2 x i64]`) and the callee's is the
/// bare struct — LLVM rejects the mismatch. Host-affecting bug, surfaced by
/// the esp32 realtime demo's wrapper shape; the fix skips musttail when
/// either side's return is coerced.
#[test]
fn extern_wrapper_tail_call_with_coerced_return_compiles_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("t.cplus");
    // Out3 (12 bytes) takes the Coerce arm on aarch64/x86_64-sysv and the
    // Indirect (sret) arm on Microsoft x64; Out6 (24 bytes) takes the
    // Indirect arm everywhere — so every platform exercises the
    // export-only-sret guard, not just Windows.
    std::fs::write(
        &src,
        "#[repr(C)] struct Out3 { a: i32, b: i32, c: i32 }\n\
         #[repr(C)] struct Out6 { a: i32, b: i32, c: i32, d: i32, e: i32, f: i32 }\n\
         export extern fn wrapped(x: i32) -> Out3 {\n\
             return inner(x);\n\
         }\n\
         fn inner(x: i32) -> Out3 {\n\
             return Out3 { a: x + 1, b: x + 2, c: x + 3 };\n\
         }\n\
         export extern fn wrapped_wide(x: i32) -> Out6 {\n\
             return inner_wide(x);\n\
         }\n\
         fn inner_wide(x: i32) -> Out6 {\n\
             return Out6 { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: x + 5, f: x + 6 };\n\
         }\n\
         fn main() -> i32 {\n\
             let r: Out3 = inner(10);\n\
             if r.a != 11 { return 1; }\n\
             if r.b != 12 { return 2; }\n\
             if r.c != 13 { return 3; }\n\
             let w: Out6 = inner_wide(20);\n\
             if w.a != 21 { return 4; }\n\
             if w.f != 26 { return 5; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("t.bin");
    let out = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "extern wrapper with coerced aggregate return must compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0), "wrapper program must run clean");
}

// ---- v0.0.21 multi-backend slice 3: the uikit package ----

/// A `@ctx { }` modifier chain that mixes a value-returning builder
/// (`.width()`, take-self) with an in-place mutator (`.set_pad()`) must COMPOSE:
/// the builder's result is threaded back into the item temp
/// (`__i = __i.width(3)`) so the following `.set_pad(5)` mutates the built node
/// instead of a moved value. (Previously this was rejected with a confusing
/// E0335 — iris bug ui-chain-take-self-vs-mutator; now fixed in
/// `lower::desugar_builder_entry` + the assignment re-init in `check_assign`.)
#[test]
fn builder_chain_take_self_and_mutator_compose() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bt\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"bt\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    // A non-Copy `Node` (has drop glue) with a take-self builder + a ref-self
    // mutator, plus the `Builder` the `@demo { }` context desugars to.
    std::fs::write(
        dir.join("src/demo.cplus"),
        "struct Node { x: i32 }
impl Node {
    fn drop(ref this) { return; }
    fn width(take this, v: i32) -> Node { var n: Node = this; n.x = v; return n; }
    fn set_pad(ref this, v: i32) { this.x = v; return; }
}
fn node() -> Node { return Node { x: 0 }; }
struct Builder { last: i32 }
impl Builder {
    fn new() -> Builder { return Builder { last: 0 }; }
    fn add(ref this, n: Node) { this.last = n.x; return; }
    fn finish(take this) -> Node { return Node { x: this.last }; }
}
",
    )
    .unwrap();
    // The chain mixes `.width(3)` (take-self builder) with `.set_pad(5)`
    // (ref-self mutator). It must compile AND compose: node() x=0 -> .width(3)
    // x=3 -> .set_pad(5) x=5, so `build().x == 5`.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./demo\" as demo;
fn build() -> demo::Node {
    return @demo {
        demo::node()
        .width(3)
        .set_pad(5)
    };
}
#[test]
fn chain_composes() {
    let n: demo::Node = build();
    assert n.x == 5;
}
fn main() -> i32 { return 0; }
",
    )
    .unwrap();

    let out = Command::new(cpc)
        .arg("test")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc test");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the builder+mutator chain must now compile and compose (no E0335):\n{stderr}\n{stdout}"
    );
    assert!(
        !stderr.contains("E0335") && !stdout.contains("E0335"),
        "no use-of-moved on the composed chain: {stderr}{stdout}"
    );
}

/// OBS.1: `#[watch]` emits a write barrier after every field store made
/// through a safe owned place, calling `on_value(ref this, field: str)` with
/// the written field's name.
///
/// The columns pinned here are the ones that would silently no-op if the
/// barrier were wired at the wrong layer:
///   - a direct field write from outside the type,
///   - a compound assign (`+=`), which takes the read-modify-write path
///     rather than the plain-store path,
///   - a write made from *inside* a method (`this.count = ...`),
///   - a nested watched struct reached through an outer field,
///   - reentrancy: the hook writes one of its own fields, which must NOT
///     re-enter the hook (else this test hangs instead of failing),
///   - a non-watched sibling field, which must stay silent.
#[test]
fn watch_struct_fires_write_barrier() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("watch.cplus");
    let bin = dir.join("watch");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         #[watch]\n\
         struct Leaf { v: i32, hits: i32 }\n\
         impl Leaf {\n\
           fn on_value(ref this, field: str) {\n\
             this.hits = this.hits + 1;\n\
             printf(#str_ptr(\"%.*s \\0\"), #str_len(field) as i32, #str_ptr(field));\n\
           }\n\
           fn bump(ref this) { this.v = this.v + 1; }\n\
         }\n\
         struct Outer { leaf: Leaf, tag: i32 }\n\
         fn main() -> i32 {\n\
           var o = Outer { leaf: Leaf { v: 0, hits: 0 }, tag: 0 };\n\
           o.leaf.v = 10;\n\
           o.leaf.v += 5;\n\
           o.leaf.bump();\n\
           o.tag = 1;\n\
           printf(#str_ptr(\"| v=%d hits=%d tag=%d\\n\\0\"), o.leaf.v, o.leaf.hits, o.tag);\n\
           return 0;\n\
         }\n",
    )
    .expect("write watch.cplus");

    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}{}",
        String::from_utf8_lossy(&compile.stderr),
        String::from_utf8_lossy(&compile.stdout)
    );

    let run = Command::new(&bin).output().expect("run watch binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    let stdout = String::from_utf8_lossy(&run.stdout);
    // Three notifications, all naming `v`; `o.tag = 1` is on a non-watched
    // struct and contributes nothing. `hits` reaching exactly 3 is the
    // reentrancy proof — the hook's own `this.hits = ...` store did not
    // re-fire the barrier.
    assert_eq!(
        stdout, "v v v | v=16 hits=3 tag=1\n",
        "unexpected observer trace: {stdout}"
    );
}

/// OBS.1 negative: the attribute is not a silent no-op. A `#[watch]`
/// struct with no hook, or a hook with the wrong signature, is a hard error
/// rather than a type that quietly never notifies.
#[test]
fn watch_struct_without_valid_hook_is_rejected() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();

    for (name, body, expected) in [
        ("missing", "#[watch]\nstruct S { x: i32 }\n", "E0361"),
        (
            "badsig",
            "#[watch]\nstruct S { x: i32 }\n\
             impl S { fn on_value(this, field: str) { return; } }\n",
            "E0362",
        ),
    ] {
        let src = dir.join(format!("watch_{name}.cplus"));
        std::fs::write(&src, format!("{body}fn main() -> i32 {{ return 0; }}\n"))
            .expect("write source");
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("invoke cpc check");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            combined.contains(expected),
            "case `{name}`: expected {expected}, got: {combined}"
        );
    }
}

/// OBS.1: a `#[watch]` generic struct stays observed through
/// monomorphization. This is the regression guard for the attribute-dropping
/// bug in `run_monomorphize` — instantiated `StructDecl`s used to be rebuilt
/// with an empty attribute list, so codegen (which re-derives type-level
/// flags off the post-mono AST) saw every instantiation as unmarked while
/// sema had it marked, and the barrier silently vanished.
#[test]
fn watch_generic_struct_keeps_barrier_after_mono() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("obs_generic.cplus");
    let bin = dir.join("obs_generic");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         #[watch]\n\
         struct Cell[T] { value: T, hits: i32 }\n\
         impl Cell[T] {\n\
           fn on_value(ref this, field: str) { this.hits = this.hits + 1; }\n\
         }\n\
         fn main() -> i32 {\n\
           var a = Cell[i32] { value: 0, hits: 0 };\n\
           var b = Cell[f64] { value: 0.0, hits: 0 };\n\
           a.value = 1;\n\
           a.value = 2;\n\
           b.value = 1.5;\n\
           printf(#str_ptr(\"%d %d\\n\\0\"), a.hits, b.hits);\n\
           return 0;\n\
         }\n",
    )
    .expect("write obs_generic.cplus");

    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}{}",
        String::from_utf8_lossy(&compile.stderr),
        String::from_utf8_lossy(&compile.stdout)
    );
    let run = Command::new(&bin).output().expect("run obs_generic binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "2 1\n",
        "each instantiation must observe its own writes"
    );
}

/// OBS.1: the 4-parameter snapshot hook. `old` is captured before the store
/// and `new` after, so the hook sees both sides of every write.
///
/// The load-bearing column is the last one: a snapshot is a *frozen value*,
/// not a view of the live struct. The hook stashes `new` into a static; 99
/// further writes then move the live struct on. If the snapshot aliased
/// `this`, the stashed reading would track those writes. It must not — that
/// distinction is the entire reason the hook takes values instead of letting
/// the handler read `this.field` back.
#[test]
fn watch_snapshot_hook_passes_frozen_old_and_new() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("watch_snap.cplus");
    let bin = dir.join("watch_snap");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         #[watch]\n\
         struct Sensor { reading: i32, seq: i32 }\n\
         static PENDING: Sensor = Sensor { reading: 0, seq: 0 };\n\
         impl Sensor {\n\
           fn on_value(ref this, field: str, old: Sensor, new: Sensor) {\n\
             if new.reading - old.reading < 10 { return; }\n\
             PENDING = new;\n\
           }\n\
         }\n\
         fn main() -> i32 {\n\
           var s = Sensor { reading: 0, seq: 0 };\n\
           s.reading = 100;\n\
           var i = 0;\n\
           while i < 99 { s.reading = s.reading + 1; i = i + 1; }\n\
           printf(#str_ptr(\"%d %d\\n\\0\"), s.reading, PENDING.reading);\n\
           return 0;\n\
         }\n",
    )
    .expect("write watch_snap.cplus");

    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}{}",
        String::from_utf8_lossy(&compile.stderr),
        String::from_utf8_lossy(&compile.stdout)
    );
    let run = Command::new(&bin).output().expect("run watch_snap binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    // live = 199; the stashed snapshot stays at the value it was notified
    // about (100), and the 99 sub-threshold writes never reached the static.
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "199 100\n",
        "a snapshot must be a frozen value, not a view of the live struct"
    );
}

/// OBS.1: the snapshot form is gated on the struct being safe to copy and
/// hold. A raw-pointer field is `Copy`, so the Copy rule alone would admit it
/// — but its pointee can be freed while a snapshot still names it.
#[test]
fn watch_snapshot_hook_rejects_unsafe_struct() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("watch_unsafe.cplus");
    std::fs::write(
        &src,
        "#[watch]\n\
         struct S { x: i32, opaque p: *i32 }\n\
         impl S { fn on_value(ref this, field: str, old: S, new: S) { return; } }\n\
         fn main() -> i32 { return 0; }\n",
    )
    .expect("write watch_unsafe.cplus");
    let out = Command::new(cpc)
        .arg("check")
        .arg(&src)
        .output()
        .expect("invoke cpc check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("E0363"),
        "expected E0363, got: {combined}"
    );
}

/// OBS.1: a store that replaces an entire watched struct fires the hook
/// exactly ONCE, with the `"*"` sentinel, rather than once per field or (as
/// it did originally) not at all.
///
/// Columns, and why each is load-bearing:
///   - two field writes fire twice — they are two independent updates;
///   - one whole-struct assign fires once — the batching answer, and silence
///     here would be a state change that bypasses the barrier entirely;
///   - `o.leaf = Leaf { .. }` is syntactically a Field target but semantically
///     a whole-struct replacement, so it must report `"*"` on `leaf`, not
///     `"leaf"` on `o` — only the innermost watched struct is told, matching
///     how `o.leaf.v = 5` behaves;
///   - a `let` initializer does NOT fire: there is no previous state;
///   - the hook still works after a whole-struct assign (the hook is a method
///     on the type, so nothing about the instance's observability is lost).
#[test]
fn watch_whole_struct_assign_fires_once_with_sentinel() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("watch_whole.cplus");
    let bin = dir.join("watch_whole");
    std::fs::write(
        &src,
        "extern fn printf(fmt: *u8, ...) -> i32;\n\
         #[watch]\n\
         struct Leaf { v: i32, w: i32 }\n\
         impl Leaf {\n\
           fn on_value(ref this, field: str) {\n\
             printf(#str_ptr(\"%.*s \\0\"), #str_len(field) as i32, #str_ptr(field));\n\
           }\n\
         }\n\
         struct Outer { leaf: Leaf, tag: i32 }\n\
         fn main() -> i32 {\n\
           var l = Leaf { v: 1, w: 2 };\n\
           l.v = 10;\n\
           l.w = 20;\n\
           l = Leaf { v: 100, w: 200 };\n\
           l.v = 5;\n\
           var o = Outer { leaf: Leaf { v: 0, w: 0 }, tag: 0 };\n\
           o.leaf = Leaf { v: 7, w: 8 };\n\
           o.tag = 9;\n\
           printf(#str_ptr(\"|\\n\\0\"));\n\
           return 0;\n\
         }\n",
    )
    .expect("write watch_whole.cplus");

    let compile = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "compile failed: {}{}",
        String::from_utf8_lossy(&compile.stderr),
        String::from_utf8_lossy(&compile.stdout)
    );
    let run = Command::new(&bin).output().expect("run watch_whole binary");
    assert!(run.status.success(), "binary exited non-zero: {}", run.status);
    // v w  → two field writes
    // *    → whole-struct assign, once
    // v    → the hook still fires afterwards
    // *    → nested whole-struct replacement, reported on `leaf`
    //        (`o.tag = 9` is not watched and adds nothing; the `var` inits add
    //        nothing either)
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "v w * v * |\n",
        "unexpected watch trace"
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

/// Binutils program names differ on Windows, where the GNU `ar`/`nm` are
/// absent but LLVM ships `llvm-ar`/`llvm-nm` (same CLI surface). These let
/// the archive/symbol-inspection tests run unprivileged against the LLVM
/// toolchain on every host.
#[allow(dead_code)]
fn ar_prog() -> &'static str {
    if cfg!(windows) {
        "llvm-ar"
    } else {
        "ar"
    }
}
#[allow(dead_code)]
fn nm_prog() -> &'static str {
    if cfg!(windows) {
        "llvm-nm"
    } else {
        "nm"
    }
}

/// The shared pure-C+ builder package for the DSL.2 e2e tests: `Item`
/// carries a value and a weight, `leaf(v)` constructs one, `boost(by)`
/// is a method modifier, and `Builder::finish` returns an `Item` so
/// nested `@group { ... }` blocks compose.
const DSL_GROUP_PACKAGE: &str = "struct Item {\n\
     \x20   value: i32,\n\
     \x20   weight: i32,\n\
     }\n\
     \n\
     fn leaf(v: i32) -> Item {\n\
     \x20   return Item { value: v, weight: 1 };\n\
     }\n\
     \n\
     impl Item {\n\
     \x20   fn boost(ref this, by: i32) {\n\
     \x20       this.weight = this.weight + by;\n\
     \x20       return;\n\
     \x20   }\n\
     }\n\
     \n\
     struct Builder {\n\
     \x20   sum: i32,\n\
     }\n\
     \n\
     impl Builder {\n\
     \x20   fn new() -> Builder {\n\
     \x20       return Builder { sum: 0 };\n\
     \x20   }\n\
     \n\
     \x20   fn add(ref this, item: Item) {\n\
     \x20       this.sum = this.sum + item.value * item.weight;\n\
     \x20       return;\n\
     \x20   }\n\
     \n\
     \x20   fn finish(take this) -> Item {\n\
     \x20       return Item { value: this.sum, weight: 1 };\n\
     \x20   }\n\
     }\n\
     \n\
     // A container element: takes a filled Builder, folds its children\n\
     // into one Item (weight 1).\n\
     fn nest(b: Builder) -> Item {\n\
     \x20   return Item { value: b.sum, weight: 1 };\n\
     }\n";

/// v0.0.22 DSL.2: `@ctx { ... }` lowers to the fixed builder protocol
/// (`ctx::Builder::new()` / `.add(item)` / `.finish()`) and runs end to
/// end against a pure-C+ package: assign modifiers, method modifiers,
/// `let` entries, an empty block, and a nested block all compose.
#[test]
fn builder_block_lowers_and_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bb\"\n\n[[bin]]\nname = \"bb\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let zero = @group { };\n\
         \x20   let base = 4;\n\
         \x20   let tree = @group {\n\
         \x20       let doubled = base * 2;\n\
         \x20       group::leaf(doubled)\n\
         \x20           .weight = 2\n\
         \x20       group::leaf(3)\n\
         \x20           .boost(1)\n\
         \x20       nest {\n\
         \x20           group::leaf(5)\n\
         \x20       }\n\
         \x20   };\n\
         \x20   return tree.value + zero.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");
    let out = Command::new(dir.join("target/debug/bb"))
        .output()
        .expect("run binary");
    // 8*2 + 3*2 + (nest folds leaf(5) -> value 5, added as 5*1) = 16+6+5 = 27;
    // the empty block contributes 0.
    assert_eq!(out.status.code(), Some(27));
}

/// v0.0.22 DSL.2: sema's ordinary diagnostics render at the user-written
/// DSL lines because the desugar reuses their spans — wrong item type at
/// the item line, unknown modifier field at the modifier line, missing
/// `Builder` at the `@ctx` line.
#[test]
fn builder_block_diagnostics_at_dsl_lines() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bd\"\n\n[[bin]]\nname = \"bd\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/empty.cplus"),
        "fn nothing() -> i32 {\n    return 0;\n}\n",
    )
    .unwrap();
    let check = |main_src: &str| -> String {
        std::fs::write(dir.join("src/main.cplus"), main_src).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .current_dir(&dir)
            .output()
            .expect("invoke cpc");
        assert!(!out.status.success(), "expected check failure");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    // Wrong item type: `42` is not a group::Item — reported at the item
    // line (line 5).
    let stderr = check(
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let v = @group {\n\
         \x20       42\n\
         \x20   };\n\
         \x20   return v.value;\n\
         }\n",
    );
    assert!(
        stderr.contains("main.cplus:5:"),
        "wrong-item-type renders at the item line: {stderr}"
    );

    // Unknown modifier field — reported at the modifier line (line 6).
    let stderr = check(
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let v = @group {\n\
         \x20       group::leaf(1)\n\
         \x20           .wieght = 2\n\
         \x20   };\n\
         \x20   return v.value;\n\
         }\n",
    );
    assert!(
        stderr.contains("no field `wieght`"),
        "unknown modifier field message: {stderr}"
    );
    assert!(
        stderr.contains("main.cplus:6:"),
        "unknown field renders at the modifier line: {stderr}"
    );

    // A context module without a Builder — reported at the `@ctx` line.
    let stderr = check(
        "import \"./empty\" as empty;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let v = @empty {\n\
         \x20       empty::nothing()\n\
         \x20   };\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(
        stderr.contains("Builder"),
        "missing-Builder message names the protocol type: {stderr}"
    );
    assert!(
        stderr.contains("main.cplus:4:"),
        "missing Builder renders at the @ctx line: {stderr}"
    );
}

/// v0.0.22 DSL.3: inside `@group { ... }` a bare item name (`leaf`) and a
/// bare context member used as a modifier value resolve through the
/// context (`group::leaf`, `group::...`) without qualification, while a
/// local binding shadows the context. Runs end to end against the pure-C+
/// package.
#[test]
fn builder_block_contextual_lookup_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bc\"\n\n[[bin]]\nname = \"bc\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let seed = 5;\n\
         \x20   let tree = @group {\n\
         \x20       leaf(seed)\n\
         \x20           .boost(2)\n\
         \x20       leaf(1)\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");
    let out = Command::new(dir.join("target/debug/bc"))
        .output()
        .expect("run binary");
    // bare `leaf` → group::leaf; `seed` is the local; .boost(2) makes the
    // first item's weight 3. sum = 5*3 + 1*1 = 16.
    assert_eq!(out.status.code(), Some(16));
}

/// v0.0.22 DSL.3 precedence: a same-file top-level `leaf` shadows the
/// context member `group::leaf` (locals → normal → contextual), and a
/// bare name that is no member at all falls through to the ordinary
/// located "undefined function" error rather than a path-rewrite error.
#[test]
fn builder_block_contextual_precedence_and_unknown() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bp\"\n\n[[bin]]\nname = \"bp\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();

    // Same-file `leaf` doubles the value; if it (not group::leaf) is used,
    // the result is 20, not 10.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn leaf(v: i32) -> group::Item {\n\
         \x20   return group::leaf(v * 2);\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let tree = @group {\n\
         \x20       leaf(10)\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");
    let out = Command::new(dir.join("target/debug/bp"))
        .output()
        .expect("run binary");
    assert_eq!(
        out.status.code(),
        Some(20),
        "same-file leaf must win over the contextual group::leaf"
    );

    // Unknown bare name in the block → normal located error.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let tree = @group {\n\
         \x20       tabel(1)\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "unknown bare name must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tabel") && stderr.contains("main.cplus:5:"),
        "unknown contextual name reports located at the item line: {stderr}"
    );
}

/// v0.0.22 DSL.4: a bare container element `nest { ... }` (same context,
/// no `@`), `if`/`else` and `for` item-control, all run end to end against
/// the pure-C+ package — items from every construct add into the same
/// builder.
#[test]
fn builder_block_containers_and_flow_control_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bf\"\n\n[[bin]]\nname = \"bf\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let flag = true;\n\
         \x20   let tree = @group {\n\
         \x20       leaf(1)\n\
         \x20       if flag {\n\
         \x20           leaf(2)\n\
         \x20       } else {\n\
         \x20           leaf(99)\n\
         \x20       }\n\
         \x20       for k in 0..3 {\n\
         \x20           leaf(10)\n\
         \x20       }\n\
         \x20       nest {\n\
         \x20           leaf(4)\n\
         \x20           leaf(5)\n\
         \x20       }\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");
    let out = Command::new(dir.join("target/debug/bf"))
        .output()
        .expect("run binary");
    // leaf(1)=1, if-true leaf(2)=2, for 3x leaf(10)=30, nest folds 4+5=9.
    // All weight 1. tree.value = 1 + 2 + 30 + 9 = 42.
    assert_eq!(out.status.code(), Some(42));
}

/// The non-Copy variant of the builder test package: `Item` has a `drop`
/// impl, so it moves instead of copying — the shape where consuming
/// fluent modifiers used to E0335. `boosted` is the fluent
/// (`take this -> Item`) modifier; the rest mirrors DSL_GROUP_PACKAGE.
const DSL_GROUP_NONCOPY_PACKAGE: &str = "struct Item {\n\
     \x20   value: i32,\n\
     \x20   weight: i32,\n\
     }\n\
     \n\
     fn leaf(v: i32) -> Item {\n\
     \x20   return Item { value: v, weight: 1 };\n\
     }\n\
     \n\
     impl Item {\n\
     \x20   // Owns a notional resource: `drop` makes Item non-Copy.\n\
     \x20   fn drop(ref this) { return; }\n\
     \n\
     \x20   fn boosted(take this, by: i32) -> Item {\n\
     \x20       return Item { value: this.value, weight: this.weight + by };\n\
     \x20   }\n\
     }\n\
     \n\
     struct Builder {\n\
     \x20   sum: i32,\n\
     }\n\
     \n\
     impl Builder {\n\
     \x20   fn new() -> Builder {\n\
     \x20       return Builder { sum: 0 };\n\
     \x20   }\n\
     \n\
     \x20   fn add(ref this, take item: Item) {\n\
     \x20       this.sum = this.sum + item.value * item.weight;\n\
     \x20       return;\n\
     \x20   }\n\
     \n\
     \x20   fn finish(take this) -> Item {\n\
     \x20       return Item { value: this.sum, weight: 1 };\n\
     \x20   }\n\
     }\n\
     \n\
     fn nest(take b: Builder) -> Item {\n\
     \x20   return Item { value: b.sum, weight: 1 };\n\
     }\n";

/// Same-line postfix after a container's `}` chains onto the container
/// item, so consuming fluent modifiers (`take this -> Self`) work on
/// containers exactly as they do on leaves — on a NON-Copy item, the
/// shape that used to E0335 before the chain fix.
#[test]
fn builder_block_container_fluent_chain_runs() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bc\"\n\n[[bin]]\nname = \"bc\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_NONCOPY_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let tree = @group {\n\
         \x20       leaf(2)\n\
         \x20       nest {\n\
         \x20           leaf(5)\n\
         \x20       }.boosted(3)\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "cpc build failed: {status}");
    let out = Command::new(dir.join("target/debug/bc"))
        .output()
        .expect("run binary");
    // leaf(2) adds 2. nest folds leaf(5) -> Item{5,1}; .boosted(3) lifts
    // its weight to 4; add contributes 5*4 = 20. tree.value = 22.
    assert_eq!(out.status.code(), Some(22));
}

/// A consuming fluent modifier on its OWN leading-dot line composes exactly
/// like the same-line `}.m()` form (see builder_block_container_fluent_chain_runs):
/// every modifier threads onto the item temp (`__i = __i.boosted(3)`), so a
/// `take self -> Self` builder re-inits the item rather than moving it away.
/// Before the compose fix an own-line modifier was a discard statement and this
/// was a use-after-move E0335; the two forms now agree (no whitespace-sensitive
/// semantic split). The thread-vs-in-place-mutate choice is type-directed in
/// sema (a unit-returning modifier is an in-place mutation), so no naming
/// convention is required of modifiers.
#[test]
fn builder_block_own_line_fluent_modifier_composes() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bm\"\n\n[[bin]]\nname = \"bm\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_NONCOPY_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let tree = @group {\n\
         \x20       nest {\n\
         \x20           leaf(5)\n\
         \x20       }\n\
         \x20           .boosted(3)\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "own-line fluent modifier must compose (not E0335): {status}"
    );
    let out = Command::new(dir.join("target/debug/bm"))
        .output()
        .expect("run binary");
    // nest folds leaf(5) -> Item{5,1}; .boosted(3) lifts its weight to 4;
    // add contributes 5*4 = 20; tree.value = 20.
    assert_eq!(out.status.code(), Some(20));
}

/// v0.0.22 DSL.4: a nested `@`-DSL block is rejected with a message that
/// points at the bare-container alternative; the error sits at the inner
/// `@` line.
#[test]
fn builder_block_nested_at_rejected_e2e() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"bn\"\n\n[[bin]]\nname = \"bn\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/group.cplus"), DSL_GROUP_PACKAGE).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./group\" as group;\n\
         \n\
         fn main() -> i32 {\n\
         \x20   let tree = @group {\n\
         \x20       @group {\n\
         \x20           leaf(1)\n\
         \x20       }\n\
         \x20   };\n\
         \x20   return tree.value;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(!out.status.success(), "nested @ must be rejected");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("without `@`") && stderr.contains("main.cplus:5:"),
        "nested-@ rejection points at the inner @ and suggests bare container: {stderr}"
    );
}

/// v0.0.22 DSL.1 negatives: a leading-dot modifier with no current item
/// and control-flow statements inside a builder block are parse errors
/// with builder-specific phrasing; a leading-dot line outside any builder
/// block stays a plain parse error.
#[test]
fn builder_block_parse_negatives() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let check = |name: &str, body: &str| -> String {
        let src = dir.join(name);
        std::fs::write(&src, body).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("invoke cpc");
        assert!(!out.status.success(), "{name} must fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    let stderr = check(
        "no_item.cplus",
        "fn main() -> i32 {\n    let v = @view {\n        .font = 1\n    };\n    return 0;\n}\n",
    );
    assert!(
        stderr.contains("modifier needs a current item"),
        "modifier-without-item phrasing: {stderr}"
    );

    let stderr = check(
        "ctl.cplus",
        "fn main() -> i32 {\n    let v = @view {\n        return 1;\n    };\n    return 0;\n}\n",
    );
    assert!(
        stderr.contains("not allowed in a builder block"),
        "control-flow phrasing: {stderr}"
    );

    let stderr = check(
        "outside.cplus",
        "fn main() -> i32 {\n    .font = 1;\n    return 0;\n}\n",
    );
    assert!(
        stderr.contains("expected expression"),
        "leading dot outside a builder block is a plain parse error: {stderr}"
    );
}

/// Named arguments: a free-function call may pass arguments as `label: value` in
/// any order (and mix leading positional args with trailing named ones). The
/// parser records labels; `lower` reorders them into parameter order and clears
/// them, so codegen sees a plain positional call. An order-sensitive `sub`
/// proves the reorder is correct — a wrong binding would change the result.
#[test]
fn named_arguments_reorder_and_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "fn sub(n1: i32, n2: i32) -> i32 { return n1 -% n2; }\n\
         fn main() -> i32 {\n\
             var score: i32 = 0;\n\
             if sub(10, 3) == 7 { score = score +% 1; }\n\
             if sub(n1: 10, n2: 3) == 7 { score = score +% 1; }\n\
             if sub(n2: 3, n1: 10) == 7 { score = score +% 1; }\n\
             if sub(10, n2: 3) == 7 { score = score +% 1; }\n\
             return score;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc failed to compile named-arg program");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(4),
        "positional / named / reordered / mixed calls must all bind correctly"
    );
}

/// Named-argument diagnostics: a positional argument after a named one (E1004),
/// an unknown label (E1005), and a duplicated argument (E1006).
#[test]
fn named_arguments_diagnostics() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let check = |body: &str| -> String {
        let src = dir.join("d.cplus");
        std::fs::write(
            &src,
            format!(
                "fn sub(n1: i32, n2: i32) -> i32 {{ return n1 -% n2; }}\n\
                 fn main() -> i32 {{ return {body}; }}\n"
            ),
        )
        .unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("run cpc check");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    assert!(
        check("sub(n1: 1, 2)").contains("E1004"),
        "positional argument after a named argument should be E1004"
    );
    assert!(
        check("sub(bogus: 1, n2: 2)").contains("E1005"),
        "unknown label should be E1005"
    );
    assert!(
        check("sub(n1: 1, n1: 2)").contains("E1006"),
        "duplicate argument should be E1006"
    );
}

/// Named arguments on method calls. `lower` has no type information, but it
/// resolves the call by matching the labels against every method of that name —
/// the labels single out the right overload. Proved here with two types whose
/// `m` methods take differently-named params: `a.m(value:, at:)` can only be
/// `A::m`, `b.m(key:, val:)` can only be `B::m`. The order-sensitive bodies
/// prove the reorder binds correctly.
#[test]
fn named_arguments_on_methods() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "struct A { x: i32 }\n\
         struct B { x: i32 }\n\
         impl A { fn m(this, value: i32, at: i32) -> i32 { return value -% at; } }\n\
         impl B { fn m(this, key: i32, val: i32) -> i32 { return key +% val; } }\n\
         fn main() -> i32 {\n\
             var a: A = A { x: 0 };\n\
             var b: B = B { x: 0 };\n\
             var score: i32 = 0;\n\
             if a.m(10, 3) == 7 { score = score +% 1; }\n\
             if a.m(at: 3, value: 10) == 7 { score = score +% 1; }\n\
             if a.m(10, at: 3) == 7 { score = score +% 1; }\n\
             if b.m(val: 5, key: 2) == 7 { score = score +% 1; }\n\
             return score;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc failed to compile method named-arg program");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(4),
        "method positional / reordered / mixed / overload-disambiguated calls must all bind correctly"
    );
}

/// Receiver-less associated functions use the same named-argument matching as
/// free functions. Both positional and labeled forms must remain valid.
#[test]
fn named_arguments_on_associated_functions() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "struct Parser { tag: i32 }\n\
         impl Parser {\n\
             fn parse(source: i32) -> i32 { return source; }\n\
         }\n\
         fn main() -> i32 {\n\
             if Parser::parse(source: 7) != 7 { return 1; }\n\
             if Parser::parse(9) != 9 { return 2; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc failed to compile associated named-arg program");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(run.code(), Some(0));
}

/// Default parameter values: an omitted trailing argument is filled from the
/// parameter's `= EXPR` default at the call site — for free functions and
/// methods, whether the explicit args are positional or labeled. Order-sensitive
/// bodies (`-%`) prove the fill lands in the right position.
#[test]
fn default_values_splice_and_run() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("prog.cplus");
    std::fs::write(
        &src,
        "fn sub(n1: i32, n2: i32 = 4) -> i32 { return n1 -% n2; }\n\
         struct C { z: i32 }\n\
         impl C { fn shift(this, base: i32, by: i32 = 1) -> i32 { return base -% by; } }\n\
         fn main() -> i32 {\n\
             var c: C = C { z: 0 };\n\
             var s: i32 = 0;\n\
             if sub(10) == 6 { s = s +% 1; }\n\
             if sub(10, 3) == 7 { s = s +% 1; }\n\
             if sub(10, n2: 3) == 7 { s = s +% 1; }\n\
             if sub(n2: 3, n1: 10) == 7 { s = s +% 1; }\n\
             if c.shift(9) == 8 { s = s +% 1; }\n\
             if c.shift(9, by: 2) == 7 { s = s +% 1; }\n\
             return s;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("prog");
    let st = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc failed to compile default-value program");
    let run = Command::new(&bin).status().expect("run");
    assert_eq!(
        run.code(),
        Some(6),
        "omitted / positional / labeled defaults must fill correctly for fns and methods"
    );
}

/// Default-value declaration errors: a required parameter after a defaulted one
/// (E1007), and a default on an `extern fn` parameter (E1008).
#[test]
fn default_value_diagnostics() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let check = |body: &str| -> String {
        let src = dir.join("d.cplus");
        std::fs::write(&src, body).unwrap();
        let out = Command::new(cpc)
            .arg("check")
            .arg(&src)
            .output()
            .expect("run cpc check");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    assert!(
        check("fn f(a: i32 = 0, b: i32) -> i32 { return a +% b; }\nfn main() -> i32 { return f(1, 2); }\n")
            .contains("E1007"),
        "required parameter after a defaulted one should be E1007"
    );
    assert!(
        check("extern fn g(x: i32 = 0) -> i32;\nfn main() -> i32 { return 0; }\n")
            .contains("E1008"),
        "default on an extern fn parameter should be E1008"
    );
}

// Float `!=` must be UNORDERED (IEEE / C, clang-verified): `x != x` is TRUE for
// NaN, so the canonical NaN test works. Was lowered as ordered `fcmp one` (false
// for NaN) — the report's program returned 6 instead of 7.
#[test]
fn float_ne_is_unordered_for_nan() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src = dir.join("nan_ne.cplus");
    std::fs::write(
        &src,
        "fn main() -> i32 {\n\
         \x20   let zero: f64 = 0.0f64;\n\
         \x20   let nan: f64 = zero / zero;\n\
         \x20   var out: i32 = 0;\n\
         \x20   if nan != nan { out = out + 1; }\n\
         \x20   if !(nan == nan) { out = out + 2; }\n\
         \x20   if !(nan <= 0.0f64) && !(nan >= 0.0f64) { out = out + 4; }\n\
         \x20   return out;\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("nan_ne");
    let status = Command::new(cpc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke cpc");
    assert!(status.success(), "NaN-ne program must compile");
    let run = Command::new(&bin).status().expect("run nan_ne");
    // nan != nan (+1), !(nan == nan) (+2), ordered-safe NaN test (+4) = 7.
    assert_eq!(run.code(), Some(7), "x != x must be true for NaN; got {:?}", run.code());
}

#[test]
fn gen_fn_protocol_survives_nested_option_instantiation() {
    // 2026-07-06: codegen recovers a gen fn's `next()` protocol enum by
    // mangled NAME (`lookup_option_ty`). The old match accepted any
    // qualified name ending in `.Option__usize` — which the NESTED
    // instantiation `Option[Option[usize]]` (mangled
    // `...option.Option__vendor.stdlib.src.option.Option__usize`) also
    // satisfies. `enum_by_name` is a HashMap, so whenever both are
    // instantiated the winner was per-process RANDOM: builds failed ~half
    // the time with an LLVM type mismatch (`fn(i64)` handed an
    // `Option[usize]` payload) and could silently pick wrong bodies.
    // Found combining facet_appkit + agent_core; reduced to this shape.
    // Eight fresh compiles make a pre-fix pass vanishingly unlikely.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"genopt\"\n\n[[bin]]\nname = \"genopt\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/iterator\" as iterator;\n\
         import \"stdlib/option\" as option;\n\
         \n\
         fn keep_big(v: usize) -> bool { return v > (1 as usize); }\n\
         \n\
         gen fn nums() -> usize {\n\
             yield (1 as usize);\n\
             yield (2 as usize);\n\
             yield (3 as usize);\n\
             return;\n\
         }\n\
         \n\
         // Pins the AMBIGUOUS name: Option[Option[usize]] must coexist with\n\
         // the gen-fn protocol's Option[usize] without stealing its lookup.\n\
         fn nested_none() -> option::Option[option::Option[usize]] {\n\
             return option::Option[option::Option[usize]]::None;\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
             let _pin: option::Option[option::Option[usize]] = nested_none();\n\
             var acc: usize = 0 as usize;\n\
             for v in nums().filter(keep_big) {\n\
                 acc = acc + v;\n\
             }\n\
             return (acc as i32) - 5;\n\
         }\n",
    )
    .unwrap();
    for round in 0..8 {
        let out = Command::new(cpc)
            .arg("build")
            .current_dir(&dir)
            .output()
            .expect("invoke cpc build");
        assert!(
            out.status.success(),
            "gen-fn protocol lookup must be deterministic (round {round}): {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let run = Command::new(dir.join("target/debug/genopt"))
        .status()
        .expect("run genopt");
    assert_eq!(run.code(), Some(0), "filter must keep 2+3 = 5");
}

/// reports/bug-12: the `str` literal → owned `Text` rule was an inline copy at
/// each value position, so the positions nobody copied it to rejected valid
/// code — an enum payload gave a spurious E0302. The rule now has one home
/// (`is_str_lit_to_lang_string`), and the enum-payload position has BOTH halves:
/// sema's coercion and codegen's owning lowering. Half a fix is worse than none
/// here — a coerced payload that codegen left as a view of the static `@.str`
/// made the enum's drop `free()` a constant, so this test runs the program.
#[test]
fn str_literal_coerces_to_text_in_every_owning_position() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"textco\"\n\n[[bin]]\nname = \"textco\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/io\" as io;\n\
         import \"stdlib/text\" as text;\n\
         \n\
         enum Holder { Some(text::Text), None }\n\
         struct S { t: text::Text }\n\
         fn ret() -> text::Text { return \"ret\"; }\n\
         fn arg(t: text::Text) -> i32 { return 0; }\n\
         fn generic[T](v: T) -> i32 { return 0; }\n\
         \n\
         fn main() -> i32 {\n\
             let a: text::Text = \"let\";\n\
             var b: text::Text = \"init\";\n\
             b = \"assign\";\n\
             let s: S = S { t: \"field\" };\n\
             let r: text::Text = ret();\n\
             let n: i32 = arg(\"call-arg\") + generic::[text::Text](\"generic-arg\");\n\
             // The position that had no copy of the rule.\n\
             let h: Holder = Holder::Some(\"payload\");\n\
             match h {\n\
                 Holder::Some(t) => { io::println(t.view()); },\n\
                 Holder::None => { io::println(\"none\"); },\n\
             }\n\
             io::println(a.view());\n\
             io::println(b.view());\n\
             io::println(s.t.view());\n\
             io::println(r.view());\n\
             return n;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "every owning position must coerce a literal: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let run = Command::new(dir.join("target/debug/textco"))
        .output()
        .expect("run textco");
    // Running it is the point: a coerced-but-not-lowered payload frees a
    // constant at drop time and aborts.
    assert!(
        run.status.success(),
        "coerced Text values must be owned, not views of the static literal: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "payload\nlet\nassign\nfield\nret\n"
    );
}

/// reports/bug-10: `check_if` asked "does this branch diverge" through a
/// private predicate with no `Match` arm, so a then-branch ending in a match
/// whose every arm returns read as `()` and collided with the else-branch's
/// type. The same match in fn-tail position already compiled — that path used
/// the canonical `crate::lower::expr_diverges`, which `check_if` now uses too.
#[test]
fn if_branch_diverging_through_a_match_imposes_no_type() {
    const SRC: &str = "\
enum E { A, B }

fn f(c: bool, e: E) -> i32 {
    let v: i32 = 7;
    let x: i32 = if c {
        match e { E::A => { return 1; }, E::B => { return 2; } }
    } else {
        v
    };
    return x;
}

fn main() -> i32 { return f(RETURNS_TRUE, E::B); }
";
    // c = false takes the else arm (7); c = true diverges through the match (2).
    for (cond, want) in [("false", 7), ("true", 2)] {
        let (_dir, bin) = compile_program(&SRC.replace("RETURNS_TRUE", cond), false);
        let run = Command::new(&bin).status().expect("run diverge probe");
        assert_eq!(run.code(), Some(want), "c = {cond}: {run}");
    }
}

/// reports/bug-08: codegen identified compiler-synthesized coroutine shapes by
/// SUBSTRING-matching the mangled name — `name.rfind("Iterator__")`. Generic
/// mangling is `Base__Arg`, so any user generic whose base name ends in
/// `Iterator` collided: `LineIterator[Token]` mangles to `LineIterator__Token`,
/// the blessed `next()` lowering fired before user-method dispatch, and the
/// program ICEd (or, when `Option[T]` happened to be instantiated, emitted
/// `llvm.coro.done` against a plain struct — a silent miscompile). Identity now
/// comes from `#[lang("iterator")]` on the stdlib template.
///
/// The real `gen fn` in the same program is the regression guard for the other
/// direction: recognition must not have been lost along with the substring.
#[test]
fn user_generic_named_iterator_is_not_a_coroutine() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hijack\"\n\n[[bin]]\nname = \"hijack\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/iterator\" as _;\n\
         \n\
         struct Token { v: i32 }\n\
         struct LineIterator[T] { cur: T }\n\
         impl LineIterator[T] {\n\
           fn next(this) -> T { return this.cur; }\n\
         }\n\
         \n\
         gen fn nums() -> i32 {\n\
           yield 1;\n\
           yield 2;\n\
           return;\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
           let it: LineIterator[Token] = LineIterator[Token] { cur: Token { v: 7 } };\n\
           let t: Token = it.next();\n\
           var acc: i32 = 0;\n\
           for v in nums() {\n\
             acc = acc + v;\n\
           }\n\
           // 7 from the user's own next(), 1+2 from the real coroutine.\n\
           return t.v + acc - 10;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "a user generic named `*Iterator` must not be treated as a coroutine: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let run = Command::new(dir.join("target/debug/hijack"))
        .status()
        .expect("run hijack");
    assert_eq!(run.code(), Some(0), "unexpected exit: {run}");
}

/// reports/bug-02: of the nine hand-rolled function-definition emitters in
/// codegen, `gen_async_function` and `gen_gen_function` were the two that never
/// ran `param_passes_by_ptr` — they emitted every parameter as a plain SSA
/// value. The call side pointer-passes a `ref` param, so the definition read a
/// pointer bit-pattern as an integer: the printed result was stack garbage and
/// the write-back never reached the caller, with no diagnostic. Their method
/// twins (`gen_async_method` / `gen_gen_method`) always classified correctly and
/// are what the fix copies.
///
/// Strict C ABI symmetry is a hard project requirement, so this pins the
/// OBSERVABLE contract at runtime: the value the coroutine computes and the
/// write-back the caller sees, for both `async fn` and `gen fn`.
#[test]
fn async_and_gen_fns_pointer_pass_ref_params() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"coro_ref\"\n\n[[bin]]\nname = \"coro_ref\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/reactor\" as _;\n\
         import \"stdlib/iterator\" as _;\n\
         \n\
         async fn bump(ref n: i64) -> i64 {\n\
             n = n + 1;\n\
             return n;\n\
         }\n\
         \n\
         gen fn steps(ref n: i64) -> i64 {\n\
             n = n + 1;\n\
             yield n;\n\
             n = n + 1;\n\
             yield n;\n\
             return;\n\
         }\n\
         \n\
         fn main() -> i32 {\n\
             var x: i64 = 5;\n\
             let f: future::Future[i64] = bump(x);\n\
             let r: i64 = #block_on::[i64](f);\n\
             var y: i64 = 5;\n\
             var total: i64 = 0;\n\
             for v in steps(y) {\n\
                 total = total + v;\n\
             }\n\
             // r = 6 (result), x = 6 (async write-back),\n\
             // total = 6 + 7 = 13, y = 7 (gen write-back).\n\
             return ((r + x) as i32) - 12 + ((total + y) as i32) - 20;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc build");
    assert!(
        out.status.success(),
        "coroutine `ref` param program must build: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let run = Command::new(dir.join("target/debug/coro_ref"))
        .status()
        .expect("run coro_ref");
    assert_eq!(
        run.code(),
        Some(0),
        "async/gen `ref` params must read their value and write back; got {:?}",
        run.code()
    );
}

#[test]
fn empty_text_coerces_to_valid_str_view() {
    // 2026-07-06 (bugs/empty-json-string-null-cstring-crash.md): an empty
    // `Text` holds a null `_ptr` (never allocated), and the `Text`→`str`
    // coercion copied the fields raw — every empty owned string coerced to a
    // (NULL, 0) view. `Text::view()` guards this case; the coercion must
    // match it (gen_text_to_str now selects a module-level 1-byte constant
    // for the null case). Downstream, `+[NSString stringWithBytes:]` aborts
    // on NULL, which took down iris at launch from one empty JSON field.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"etc\"\n\n[[bin]]\nname = \"etc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let t: text::Text = text::new();\n\
             let s: str = { t };\n\
             if #str_len(s) != (0 as usize) { return 1; }\n\
             if { #addr(#str_ptr(s)) } == (0 as usize) { return 2; }\n\
             let u: text::Text = text::from_str(\"\");\n\
             let v: str = { u };\n\
             if #str_len(v) != (0 as usize) { return 3; }\n\
             if { #addr(#str_ptr(v)) } == (0 as usize) { return 4; }\n\
             let w: text::Text = text::from_str(\"hi\");\n\
             let x: str = { w };\n\
             if #str_len(x) != (2 as usize) { return 5; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(status.success(), "coercion program must compile");
    let run = Command::new(dir.join("target/debug/etc")).status().expect("run etc");
    assert_eq!(
        run.code(),
        Some(0),
        "empty Text must coerce to a zero-length str with a VALID pointer (1/3=len wrong, 2/4=null ptr)"
    );
}

#[test]
fn borrow_error_names_the_offending_module() {
    // 2026-07-06 (iris review, top ask): borrowck attributed EVERY diagnostic
    // to the entry file — an error inside an imported module printed
    // `main.cplus:<line>` with the line number (and snippet) of a different
    // file. borrowck::check_multi now routes each raw diagnostic through the
    // item's `origin_file` (the same per-file map sema uses), and the human
    // renderer quotes snippets from the span's own file.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"dm\"\n\n[[bin]]\nname = \"dm\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::os::unix::fs::symlink(
        format!("{}/../vendor", env!("CARGO_MANIFEST_DIR")),
        dir.join("vendor"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./codex\" as codex;\n\
         import \"stdlib/io\" as io;\n\
         \n\
         fn main() -> i32 {\n\
             io::println(codex::describe());\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    // The borrow error lives in the SECOND module, on line 8/7.
    // `s` is a view of `cmd_str`; consuming `cmd_str` on line 8 while `s` is
    // still used (line 9) is the move-while-borrowed E0372. `describe` returns
    // a literal so this stays a pure move-routing test — returning `s` itself
    // would (correctly) add an E0513 dangling-view-of-local on line 9.
    std::fs::write(
        dir.join("src/codex.cplus"),
        "import \"stdlib/text\" as text;\n\
         \n\
         fn consume(take t: text::Text) { return; }\n\
         \n\
         fn describe() -> str {\n\
             let cmd_str: text::Text = text::from_str(\"codex exec\");\n\
             let s: str = cmd_str.view();\n\
             consume(cmd_str);\n\
             if s == \"codex exec\" { return \"yes\"; }\n\
             return \"no\";\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("cpc check");
    assert!(!out.status.success(), "the move-while-borrowed must be rejected");
    let err = String::from_utf8_lossy(&out.stderr);
    // Primary span: the offending module, correct line (8 = `consume(cmd_str)`).
    assert!(
        err.contains("codex.cplus:8"),
        "primary must name codex.cplus:8, got:\n{err}"
    );
    // The "borrowed here" label: same module, line 7.
    assert!(
        err.contains("codex.cplus:7"),
        "label must name codex.cplus:7, got:\n{err}"
    );
    // Snippets quote the module's own lines, not the entry file's.
    assert!(
        err.contains("consume(cmd_str)"),
        "snippet must quote the offending line, got:\n{err}"
    );
    assert!(
        !err.contains("main.cplus:8"),
        "the error must NOT be attributed to the entry file, got:\n{err}"
    );
}

// ---- 2026-07-16: generic template BODIES instantiate what they mention ----
//
// Generic-impl method bodies are never type-checked (name-resolution only), so
// a generic type used ONLY inside one — `MyVec[T]::new()`, `let m: MyVec[T]`,
// a same-module free-fn dispatch — used to leave no instantiation and no
// span-keyed dispatch record, and the enclosing generic's instantiation
// panicked codegen ("sema validated enum name" for the no-arg assoc call,
// "TypeKind::Generic" for the annotation). `propagate_body_instantiations`
// (sema) + the by-name fn-instantiation lookup (monomorphize) close this.

fn compile_and_run_src(name: &str, src: &str) -> std::process::Output {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let src_path = dir.join(format!("{name}.cplus"));
    std::fs::write(&src_path, src).expect("write source");
    let bin = dir.join(name);
    let compile = Command::new(cpc)
        .arg(&src_path)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("invoke cpc");
    assert!(
        compile.status.success(),
        "cpc failed to compile {name}: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    Command::new(&bin).output().expect("run produced binary")
}

/// The events-package ctor shape: a generic static method building a field
/// whose type is ANOTHER generic, via that generic's own static method. The
/// call is no-arg, so it also pins the Path-vs-Call dispatch decision for
/// spans sema never type-checks.
#[test]
fn generic_static_method_calls_generic_static_ctor() {
    let out = compile_and_run_src(
        "genstatic",
        "struct MyVec[T] { n: i64 }\n\
         impl MyVec[T] {\n\
             fn new() -> MyVec[T] { return MyVec[T] { n: 7 }; }\n\
         }\n\
         struct Signal[T] { subs: MyVec[T] }\n\
         impl Signal[T] {\n\
             fn new() -> Signal[T] { return Signal[T] { subs: MyVec[T]::new() }; }\n\
         }\n\
         fn main() -> i32 {\n\
             var s: Signal[i64] = Signal[i64]::new();\n\
             return (s.subs.n - 7) as i32;\n\
         }\n",
    );
    assert!(out.status.success(), "exit: {}", out.status);
}

/// A generic type mentioned ONLY in a `let` annotation inside a generic
/// body — no field of the enclosing generic covers the instantiation.
#[test]
fn generic_body_type_annotation_instantiates() {
    let out = compile_and_run_src(
        "genannot",
        "struct MyVec[T] { n: i64 }\n\
         impl MyVec[T] {\n\
             fn make(v: i64) -> MyVec[T] { return MyVec[T] { n: v }; }\n\
         }\n\
         struct Signal[T] { dummy: i64 }\n\
         impl Signal[T] {\n\
             fn probe() -> i64 { let m: MyVec[T] = MyVec[T]::make(1); return m.n; }\n\
         }\n\
         fn main() -> i32 {\n\
             return (Signal[i64]::probe() - 1) as i32;\n\
         }\n",
    );
    assert!(out.status.success(), "exit: {}", out.status);
}

/// An enum associated fn whose signature names its own enum, reached both
/// from a generic body and a concrete site. Pins two fixes at once: the
/// enum-side no-arg dispatch record, and the `populate_generic_enum_methods`
/// reentrancy guard (this shape used to stack-overflow the compiler).
#[test]
fn generic_body_enum_assoc_fn_self_referential_sig() {
    let out = compile_and_run_src(
        "genenum",
        "enum Maybe[T] { Yes(T), No }\n\
         impl Maybe[T] {\n\
             fn empty() -> Maybe[T] { return Maybe[T]::No; }\n\
         }\n\
         struct Signal[T] { dummy: i64 }\n\
         impl Signal[T] {\n\
             fn probe() -> Maybe[T] { return Maybe[T]::empty(); }\n\
         }\n\
         fn main() -> i32 {\n\
             let m: Maybe[i64] = Signal[i64]::probe();\n\
             let d: Maybe[i64] = Maybe[i64]::empty();\n\
             let a: i32 = match m { Maybe[i64]::Yes(v) => 1, Maybe[i64]::No => 0 };\n\
             let b: i32 = match d { Maybe[i64]::Yes(v) => 1, Maybe[i64]::No => 0 };\n\
             return a + b;\n\
         }\n",
    );
    assert!(out.status.success(), "exit: {}", out.status);
}

/// `Type[args]::name()` resolving to a same-module FREE generic fn, called
/// inside a generic body with a NOMINAL type-arg (`MyVec[Item[T]]::of()`).
/// The span has no `call_monos` record and the AST→Ty fallback can't resolve
/// nominal args, so the callee resolves through the by-name fn-instantiation
/// lookup added to monomorphize.
#[test]
fn generic_body_free_fn_dispatch_nominal_args() {
    let out = compile_and_run_src(
        "genfreefn",
        "struct Item[T] { v: T }\n\
         struct MyVec[T] { n: i64 }\n\
         fn of[T]() -> MyVec[T] { return MyVec[T] { n: 3 }; }\n\
         struct Signal[T] { subs: MyVec[Item[T]] }\n\
         impl Signal[T] {\n\
             fn new() -> Signal[T] { return Signal[T] { subs: MyVec[Item[T]]::of() }; }\n\
         }\n\
         fn main() -> i32 {\n\
             var s: Signal[i64] = Signal[i64]::new();\n\
             return (s.subs.n - 3) as i32;\n\
         }\n",
    );
    assert!(out.status.success(), "exit: {}", out.status);
}

/// 2026-07-16: omitted trailing DEFAULTS on a method whose bare name is
/// shared by several types. Lower's splice table is keyed by method name
/// alone (no type info), so both candidates accepted the positional call
/// differently and it reached sema unspliced — E0308 arity error on
/// perfectly valid code. Sema now finishes the splice from the receiver's
/// type (`try_splice_method_defaults`) and monomorphize appends the
/// recorded defaults. The bound-method reference rides the same splice for
/// its ctx slot.
#[test]
fn method_name_collision_still_splices_defaults() {
    let out = compile_and_run_src(
        "defcollide",
        "struct Sig { total: i64 }\n\
         impl Sig {\n\
             fn on(ref this, v: i64, ctx: *u8 = 0 as *u8, once: bool = false) -> u64 {\n\
                 this.total = this.total + v + (ctx as i64);\n\
                 return 1u64;\n\
             }\n\
         }\n\
         struct Bus { total: i64 }\n\
         impl Bus {\n\
             fn on(ref this, name: str, v: i64, ctx: *u8 = 0 as *u8) -> u64 {\n\
                 this.total = this.total + v;\n\
                 return 2u64;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             var s: Sig = Sig { total: 0 };\n\
             let _a: u64 = s.on(5);\n\
             let _b: u64 = s.on(5, ctx: 2 as *u8);\n\
             var b: Bus = Bus { total: 0 };\n\
             let _c: u64 = b.on(\"tick\", 3);\n\
             return ((s.total - 12) + (b.total - 3)) as i32;\n\
         }\n",
    );
    assert!(out.status.success(), "exit: {}", out.status);
}

// ---- `[build]` — prebuild cache + dev override ----

/// Lay out a consumer app plus one vendor package whose `[build]` table is
/// `build_table`. The package exports `answer()`, which returns `answer`.
fn prebuild_fixture(dir: &std::path::Path, build_table: &str, answer: i32) {
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nmathy = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"mathy/mathy\" as mathy;\nfn main() -> i32 { return mathy::answer(); }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("vendor/mathy/src")).unwrap();
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        format!("[package]\nname = \"mathy\"\n\n{build_table}"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/mathy.cplus"),
        format!("fn answer() -> i32 {{ return {answer} as i32; }}\n"),
    )
    .unwrap();
}

fn build_app(cpc: &str, dir: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut c = Command::new(cpc);
    c.arg("build");
    for a in extra {
        c.arg(a);
    }
    c.current_dir(dir).output().expect("invoke cpc")
}

#[test]
fn prebuild_compiles_the_dep_once_and_links_the_slice() {
    // `[build] prebuild = true` is the whole opt-in: no `[lib]`, no `[link]`,
    // no triple. The first consumer build produces the slice and its headers;
    // the second reuses them.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();
    prebuild_fixture(&dir, "[build]\nprebuild = true\n", 21);

    let out = build_app(cpc, &dir, &[]);
    assert!(
        out.status.success(),
        "prebuild build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("prebuilding `mathy`"),
        "first build should announce the prebuild: {stderr}"
    );

    let slice = dir.join("vendor/mathy/lib").join(&host);
    assert!(
        slice.join("libmathy.a").is_file(),
        "expected an archive at {}",
        slice.display()
    );
    assert!(
        slice.join("mathy.fingerprint").is_file(),
        "expected a fingerprint next to the archive"
    );
    assert!(
        dir.join("vendor/mathy/lib/include/mathy.cplus").is_file(),
        "headers must be generated with the archive, never separately"
    );
    // The slice holds the shipped surface and nothing else — no object file,
    // no C header from the library pipeline's own output directory.
    let stray: Vec<String> = std::fs::read_dir(&slice)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "libmathy.a" && n != "mathy.fingerprint")
        .collect();
    assert!(stray.is_empty(), "build litter in the slice dir: {stray:?}");

    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(21));

    // Second build: fingerprint matches, nothing recompiles.
    let out = build_app(cpc, &dir, &[]);
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("prebuilding"),
        "an unchanged dep must not rebuild"
    );
}

#[test]
fn prebuild_rebuilds_when_the_package_source_changes() {
    // The fingerprint covers package source, so an edit invalidates it. This
    // is what makes the cache trustworthy rather than merely fast.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    prebuild_fixture(&dir, "[build]\nprebuild = true\n", 21);
    assert!(build_app(cpc, &dir, &[]).status.success());

    std::fs::write(
        dir.join("vendor/mathy/src/mathy.cplus"),
        "fn answer() -> i32 { return 33 as i32; }\n",
    )
    .unwrap();
    let out = build_app(cpc, &dir, &[]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("prebuilding `mathy`"),
        "an edited dep must rebuild"
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(33), "the consumer must see the new code");
}

#[test]
fn prebuild_rebuilds_when_the_build_mode_changes() {
    // Debug and release share one slice path, so the mode is in the
    // fingerprint. Without it a `--release` consumer would silently link
    // overflow-checked debug code.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    prebuild_fixture(&dir, "[build]\nprebuild = true\n", 21);
    assert!(build_app(cpc, &dir, &[]).status.success());

    let out = build_app(cpc, &dir, &["--release"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("prebuilding `mathy`"),
        "switching build mode must rebuild the slice"
    );
    let run = Command::new(dir.join("target/release/app")).status().expect("run");
    assert_eq!(run.code(), Some(21));
}

#[test]
fn prebuilt_slice_is_not_an_orphan() {
    // E0861 rejects undeclared binaries under `lib/<triple>/` — the manifest
    // is the source of truth for what a package SHIPS. The archive the
    // compiler produced is not that, and must not trip the check.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    prebuild_fixture(&dir, "[build]\nprebuild = true\n", 21);
    assert!(build_app(cpc, &dir, &[]).status.success());
    let out = build_app(cpc, &dir, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && !stderr.contains("E0861"),
        "the compiler's own slice must not be an orphan: {stderr}"
    );
}

#[test]
fn dev_mode_compiles_from_source_and_ignores_the_slice() {
    // The development loop: a slice exists and is up to date, but `dev = true`
    // means the package is being worked on. Edits must take effect with no
    // rebuild-the-archive step in between.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();
    prebuild_fixture(&dir, "[build]\nprebuild = true\n", 21);
    assert!(build_app(cpc, &dir, &[]).status.success());
    let archive = dir.join("vendor/mathy/lib").join(&host).join("libmathy.a");
    let before = std::fs::metadata(&archive).unwrap().len();

    // Flip to dev and change the answer, leaving the stale slice in place.
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\n\n[build]\nprebuild = true\ndev      = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/mathy.cplus"),
        "fn answer() -> i32 { return 44 as i32; }\n",
    )
    .unwrap();

    let out = build_app(cpc, &dir, &[]);
    assert!(
        out.status.success(),
        "dev-mode build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dev mode"),
        "dev mode must announce itself on every build: {stderr}"
    );
    assert!(
        !stderr.contains("prebuilding"),
        "dev mode must not maintain the cache: {stderr}"
    );
    assert_eq!(
        std::fs::metadata(&archive).unwrap().len(),
        before,
        "the stale slice must be left untouched, just unused"
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(
        run.code(),
        Some(44),
        "dev mode must compile the edited source, not link the stale archive"
    );
}

#[test]
fn dev_mode_ignores_author_shipped_binaries_too() {
    // `dev` overrides every source of binaries, not just `prebuild`. A
    // deliberately corrupt archive proves the link line never sees it.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    let host = host_triple_for_test();
    prebuild_fixture(&dir, "[build]\ndev = true\n", 9);
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\n\n[build]\ndev = true\n\n[link]\nbundled = [\"libmathy.a\"]\n",
    )
    .unwrap();
    let slice = dir.join("vendor/mathy/lib").join(&host);
    std::fs::create_dir_all(&slice).unwrap();
    std::fs::write(slice.join("libmathy.a"), b"not a real archive").unwrap();

    let out = build_app(cpc, &dir, &[]);
    assert!(
        out.status.success(),
        "dev mode must bypass the bundled archive entirely: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(9));
}

#[test]
fn build_table_defaults_to_source_only() {
    // A package that says nothing keeps the old behaviour: compiled from
    // `src/` every time, no slice, no headers. `prebuild` is opt-in.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    prebuild_fixture(&dir, "", 5);
    let out = build_app(cpc, &dir, &[]);
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("prebuilding"),
        "nothing should be cached without `prebuild = true`"
    );
    assert!(
        !dir.join("vendor/mathy/lib").exists(),
        "a source-only package must not grow a lib/ directory"
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(5));
}

#[test]
fn unknown_build_key_is_rejected() {
    // Negative case: `[build]` is closed. A typo'd policy key must fail the
    // build rather than sit there doing nothing.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    prebuild_fixture(&dir, "[build]\nprebuilt = true\n", 21);
    let out = build_app(cpc, &dir, &[]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown field `prebuilt`"),
        "expected the parse error to name the key: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn prebuilt_archive_covers_modules_the_entry_never_imports() {
    // A package's public surface is its `src/` directory, and `cpc headers`
    // emits a declaration file per module there. If the archive only held the
    // entry's import tree, a consumer would compile against a header for a
    // module nothing defines and fail at link. `appkit` is the live shape:
    // `appkit_ext.cplus` sits alongside `appkit.cplus`, imported by neither.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nmathy = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/mathy/src")).unwrap();
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\n\n[build]\nprebuild = true\n",
    )
    .unwrap();
    // The entry. It does NOT import `extra`.
    std::fs::write(
        dir.join("vendor/mathy/src/mathy.cplus"),
        "fn base() -> i32 { return 20 as i32; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/extra.cplus"),
        "fn bonus() -> i32 { return 1 as i32; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"mathy/mathy\" as mathy;\n\
         import \"mathy/extra\" as extra;\n\
         fn main() -> i32 { return mathy::base() +% extra::bonus(); }\n",
    )
    .unwrap();

    let out = build_app(cpc, &dir, &[]);
    assert!(
        out.status.success(),
        "a module outside the entry's import tree must still be in the archive: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(21));
}

#[test]
fn a_package_can_import_itself_by_name() {
    // `appkit_ext.cplus` writes `import "appkit/appkit"`. That resolves through
    // `vendor/appkit/` for every consumer, but inside appkit's own build there
    // is no `vendor/appkit` and a package is not its own dependency. Both
    // spellings must mean the same module, or such a package cannot be
    // compiled on its own — which is exactly what `prebuild` has to do.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nmathy = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/mathy/src")).unwrap();
    std::fs::write(
        dir.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\n\n[build]\nprebuild = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/mathy/src/mathy.cplus"),
        "fn base() -> i32 { return 20 as i32; }\n",
    )
    .unwrap();
    // The companion module refers to its own package BY NAME, not relatively.
    std::fs::write(
        dir.join("vendor/mathy/src/extra.cplus"),
        "import \"mathy/mathy\" as own;\n\
         fn twice_base() -> i32 { return own::base() +% own::base(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"mathy/extra\" as extra;\n\
         fn main() -> i32 { return extra::twice_base(); }\n",
    )
    .unwrap();

    let out = build_app(cpc, &dir, &[]);
    assert!(
        out.status.success(),
        "a package must be able to name itself: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("target/debug/app")).status().expect("run");
    assert_eq!(run.code(), Some(40));
}

#[test]
fn self_import_of_a_missing_module_is_still_an_error() {
    // The self-name path must not become a way to import anything: only a file
    // that actually exists under this package's `src/` resolves.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"app/nope\" as nope;\nfn main() -> i32 { return 0; }\n",
    )
    .unwrap();
    let out = build_app(cpc, &dir, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0852") || stderr.contains("not a declared dependency"),
        "expected the ordinary undeclared-package error, got: {stderr}"
    );
}

#[test]
fn same_package_impl_extension_compiles_and_runs() {
    // EXT.1 (v0.0.27): a package extends its own type from another of its
    // files — the layout a generator-owned contract file needs. The method
    // must resolve at a third file's call site and RUN.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/geom.cplus"),
        "struct Point { x: i32, y: i32 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/ext.cplus"),
        "import \"./geom\" as g;\nimpl g::Point { fn sum(this) -> i32 { return this.x + this.y; } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./geom\" as g;\nimport \"./ext\" as e;\nfn main() -> i32 { let p: g::Point = g::Point { x: 3, y: 4 }; return p.sum(); }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("cpc build");
    assert!(st.success(), "same-package extension must compile");
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run binary");
    assert_eq!(run.code(), Some(7), "extension method must run: 3 + 4");
}

/// EXT.2 spike: lay down an app that depends on `dep` (declares `Point`) and
/// on `ext` (adds `sum` to it). `src/main.cplus` is written by the caller.
fn ext_project(main_src: &str, extra: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n[dependencies]\ndep = \"*\"\next = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/dep/src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/ext/src")).unwrap();
    std::fs::write(
        dir.join("vendor/dep/Cplus.toml"),
        "[package]\nname = \"dep\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/dep/src/dep.cplus"),
        "struct Point { x: i32, y: i32 }\nimpl Point { fn area(this) -> i32 { return this.x * this.y; } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/ext/Cplus.toml"),
        "[package]\nname = \"ext\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/ext/src/ext.cplus"),
        "import \"dep/dep\" as d;\nimpl d::Point { fn sum(this) -> i32 { return this.x + this.y; } }\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/main.cplus"), main_src).unwrap();
    for (rel, src) in extra {
        std::fs::write(dir.join(rel), src).unwrap();
    }
    dir
}

#[test]
fn extension_runs_where_the_extending_module_is_imported() {
    // EXT.2's line: any module may add methods to any type, and they are real
    // methods — `p.sum()` from `ext` beside `p.area()` from `dep`. Two
    // packages here only to show the package boundary is not the rule.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = ext_project(
        "import \"dep/dep\" as d;\nimport \"ext/ext\" as e;\n\
         fn main() -> i32 { let p: d::Point = d::Point { x: 3, y: 4 }; return p.sum() + p.area(); }\n",
        &[],
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("cpc build");
    assert!(st.success(), "an imported extension must compile");
    let run = Command::new(dir.join("target/debug/app"))
        .status()
        .expect("run binary");
    assert_eq!(run.code(), Some(19), "3 + 4 from `ext`, 3 * 4 from `dep`");
}

#[test]
fn extension_is_invisible_without_the_import_e0388() {
    // The extension is in the build (main imports it), but `helper.cplus`
    // never named `ext/ext` — so `sum` is not in ITS scope.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = ext_project(
        "import \"dep/dep\" as d;\nimport \"ext/ext\" as e;\nimport \"./helper\" as h;\n\
         fn main() -> i32 { let p: d::Point = d::Point { x: 3, y: 4 }; return h::probe(p); }\n",
        &[(
            "src/helper.cplus",
            "import \"dep/dep\" as d;\nfn probe(p: d::Point) -> i32 { return p.sum(); }\n",
        )],
    );
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("cpc check");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "an unimported extension must not resolve"
    );
    assert!(all.contains("E0388"), "expected E0388, got: {all}");
    assert!(
        all.contains("ext/ext"),
        "the diagnostic names the import to add; got: {all}"
    );
}

#[test]
fn two_modules_may_not_declare_the_same_extension_method_e0326() {
    // The discriminating program: `ext/ext` and `./mine` both add `sum` to
    // `Point`, and NO file imports both — `h1` reaches one, `h2` the other.
    // There is still exactly one `Point.sum` in a program, so this is an
    // error, not two methods.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = ext_project(
        "import \"./h1\" as h1;\nimport \"./h2\" as h2;\n\
         fn main() -> i32 { return h1::a() + h2::b(); }\n",
        &[
            (
                "src/mine.cplus",
                "import \"dep/dep\" as d;\nimpl d::Point { fn sum(this) -> i32 { return 0; } }\n",
            ),
            (
                "src/h1.cplus",
                "import \"dep/dep\" as d;\nimport \"ext/ext\" as e;\n\
                 fn a() -> i32 { let p: d::Point = d::Point { x: 1, y: 2 }; return p.sum(); }\n",
            ),
            (
                "src/h2.cplus",
                "import \"dep/dep\" as d;\nimport \"./mine\" as m;\n\
                 fn b() -> i32 { let p: d::Point = d::Point { x: 1, y: 2 }; return p.sum(); }\n",
            ),
        ],
    );
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("cpc check");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "a second `Point.sum` must be rejected");
    assert!(all.contains("E0326"), "expected E0326, got: {all}");
    assert!(
        all.contains("already declared"),
        "the message says the name is taken; got: {all}"
    );
}

#[test]
fn sibling_file_extension_still_needs_the_import_e0388() {
    // Same package, next-door file: the gate does not care. `helper.cplus`
    // must import the module that wrote `twice`, exactly as it would for a
    // vendored one.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = ext_project(
        "import \"dep/dep\" as d;\nimport \"./mine\" as m;\nimport \"./helper\" as h;\n\
         fn main() -> i32 { return h::probe(); }\n",
        &[
            (
                "src/mine.cplus",
                "import \"dep/dep\" as d;\nimpl d::Point { fn twice(this) -> i32 { return this.x * 2; } }\n",
            ),
            (
                "src/helper.cplus",
                "import \"dep/dep\" as d;\n\
                 fn probe() -> i32 { let p: d::Point = d::Point { x: 1, y: 2 }; return p.twice(); }\n",
            ),
        ],
    );
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("cpc check");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "a sibling extension is gated too");
    assert!(all.contains("E0388"), "expected E0388, got: {all}");
}

#[test]
fn extension_may_not_replace_an_existing_method_e0326() {
    // `ext` already adds `sum`; here it also tries to redefine `area`, which
    // `dep` declares. An extension adds, it never overrides.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = ext_project(
        "import \"dep/dep\" as d;\nimport \"ext/ext\" as e;\nfn main() -> i32 { return 0; }\n",
        &[(
            "vendor/ext/src/ext.cplus",
            "import \"dep/dep\" as d;\nimpl d::Point { fn area(this) -> i32 { return 0; } }\n",
        )],
    );
    let out = Command::new(cpc)
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("cpc check");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "an override must be rejected");
    assert!(all.contains("E0326"), "expected E0326, got: {all}");
}
