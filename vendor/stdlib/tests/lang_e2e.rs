// Integration tests for the `stdlib` package, migrated out of the compiler's
// language e2e suite (cpc/tests/e2e.rs): the language test suite must not depend
// on a shipped vendor package, so these tests live with the package they exercise.
//
// These are the original Rust harness tests (each builds an embedded C+ program
// that imports `stdlib`, then asserts on the cpc build/run output). They still
// reference the e2e harness helpers + workspace-relative vendor paths, so they
// are preserved here verbatim for re-wiring into this package's own runner (or
// conversion to `cpc test` C+ `#[test]`s) — not yet runnable standalone.


// v0.0.19: monomorphization fix — a turbofish generic call must mangle its
// callee from its own (collision-free) AST type-args, not from `call_monos`
// (keyed by a file-less `ByteSpan`). Two turbofish `vec::new::[T]()` calls at
// the SAME byte offset in different files used to collide: one got the other's
// type-args, miscompiling a `Vec[A]` value into a `Vec[B]` slot. Here modA and
// modB are byte-identical except `Aaa`<->`Bbb` / `fa`<->`fb` (same lengths), so
// the calls land at the same offset; the program must build and return 2
// (fa()=1 + fb()=1). Before the fix this failed at the clang stage.
#[test]
fn monomorphize_turbofish_same_offset_no_collision() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"mono_span\"\n\n[[bin]]\nname = \"mono_span\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    symlink_dir(&root.join("vendor/stdlib"), &dir.join("vendor/stdlib"));
    let mod_a = "import \"stdlib/vec\" as vec;\n\
                 struct Aaa { x: i32 }\n\
                 fn fa() -> usize {\n\
                 \x20   var v: vec::Vec[Aaa] = vec::new::[Aaa]();\n\
                 \x20   v.push(Aaa { x: 1 });\n\
                 \x20   return v.len();\n\
                 }\n";
    std::fs::write(dir.join("src/modA.cplus"), mod_a).unwrap();
    // Byte-identical except the 3-char type name and 2-char fn name → the
    // `vec::new::[...]` calls share a byte offset.
    std::fs::write(
        dir.join("src/modB.cplus"),
        mod_a.replace("Aaa", "Bbb").replace("fa", "fb"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./modA\" as ma;\n\
         import \"./modB\" as mb;\n\
         fn main() -> i32 { return (ma::fa() +% mb::fb()) as i32; }\n",
    )
    .unwrap();
    let status = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(
        status.success(),
        "same-offset turbofish build failed: {status}"
    );
    let run = Command::new(dir.join("target/debug/mono_span"))
        .status()
        .expect("run mono_span");
    assert_eq!(run.code(), Some(2), "got {:?}", run.code());
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
    assert!(
        status.success(),
        "atomic_thread_fence must compile under cpc build"
    );
    let run = Command::new(dir.join("target/debug/f"))
        .output()
        .expect("run");
    assert!(run.status.success(), "fence program returned non-zero");
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
fn text_coercion_end_to_end() {
    // v0.0.24 #11 positive run: a `Text` coerces to `str` at an argument, a
    // binding, and a comparison — the borrowed view reads the live buffer
    // (including after `push_str` reallocs it). No `as_str` method anywhere.
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"textcoerce\"\n\n[[bin]]\nname = \"textcoerce\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["text", "option", "vec", "iterator"] {
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
        "import \"stdlib/text\" as text;\n\
         fn take(s: str) -> usize { return #str_len(s); }\n\
         fn main() -> i32 {\n\
             var t = \"AB\".to_text();\n\
             t.push_str(\"CD\");\n\
             let n = take(t);\n\
             if n != (4 as usize) { return 1; }\n\
             let v: str = t;\n\
             if #str_len(v) != (4 as usize) { return 2; }\n\
             if t != \"ABCD\" { return 3; }\n\
             if \"ABCD\" != t { return 4; }\n\
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
    let bin = dir.join("target/debug/textcoerce");
    let out = Command::new(&bin).output().expect("run textcoerce");
    assert!(
        out.status.success(),
        "Text→str coercion run failed (exit {:?}): the return code marks which \
         coercion site is wrong (1=arg, 2=binding, 3/4=comparison)",
        out.status.code()
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var buf: vec::Vec[u8] = vec::new::[u8]();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var m: map::HashMap[str, i32] = map::new_str_int_map();\n\
             m.insert(\"apple\",  1 as i32);\n\
             m.insert(\"banana\", 2 as i32);\n\
             m.insert(\"cherry\", 3 as i32);\n\
             m.insert(\"apple\",  10 as i32);\n\
             var fails: i32 = 0 as i32;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var m1: hm::HashMap[i32, i32] = hm::new::[i32, i32]();\n\
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
             var m2: hm::HashMap[str, i32] = hm::new::[str, i32]();\n\
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
             var m3: hm::HashMap[i32, i32] = hm::new::[i32, i32]();\n\
             var i: i32 = 0;\n\
             while i < (100 as i32) {\n\
                 m3.insert(i, i *% (10 as i32));\n\
                 i = i +% (1 as i32);\n\
             }\n\
             if m3.len() != (100 as usize) { return 10 as i32; }\n\
             var sum: i32 = 0;\n\
             var j: i32 = 0;\n\
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

/// `HashMap[K, V]` declares `K: Copy, V: Copy` because insert/overwrite/get
/// bit-copy and overwrite slots without running destructors. A non-Copy
/// (owning / `drop`-carrying) value must be rejected at the use site with
/// E0502 — NOT silently miscompiled into a double-free, and NOT a compiler
/// panic (the pre-fix behavior: codegen hit `Ty::Error` and aborted). This is
/// the soundness counterpart to the plan's long-deferred "non-Copy V revisit".
#[test]
fn stdlib_hash_map_noncopy_value_rejected_e0502() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"hmnc\"\n\n[[bin]]\nname = \"hmnc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/hash_map.cplus"),
        include_str!("../../vendor/stdlib/src/hash_map.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/result.cplus"),
        include_str!("../../vendor/stdlib/src/result.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/hash_map\" as hm;\n\
         struct Owner { p: *u8 }\n\
         impl Owner {\n\
             fn drop(ref this) { { free(this.p); } return; }\n\
             fn hash(this) -> u64 { return 7 as u64; }\n\
             fn eq(this, other: This) -> bool { return true; }\n\
         }\n\
         extern fn free(p: *u8);\n\
         fn main() -> i32 {\n\
             var m: hm::HashMap[i32, Owner] = hm::new::[i32, Owner]();\n\
             return 0 as i32;\n\
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
        "expected compile failure for a non-Copy HashMap value"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0502"),
        "expected E0502 (Copy bound not satisfied) in stderr, got: {stderr}"
    );
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
                 var listener: net::TcpListener = lis;\n\
                 guard let result::Result[net::TcpStream, result::IoError]::Ok(client) = listener.accept()\n\
                     else {{ return 2; }};\n\
                 var stream: net::TcpStream = client;\n\
                 guard let result::Result[vec::Vec[u8], result::IoError]::Ok(data) = stream.read_to_end()\n\
                     else {{ return 3; }};\n\
                 guard let result::Result[usize, result::IoError]::Ok(w) = stream.write_all(data)\n\
                     else {{ return 4; }};\n\
                 if w == (0 as usize) {{ return 5; }}\n\
                 return 0;\n\
             }}\n\
             fn run_client() -> usize {{\n\
                 {{ sleep(1 as u32); }}\n\
                 guard let result::Result[net::TcpStream, result::IoError]::Ok(s) = net::connect_tcp(\"127.0.0.1\", {port} as u16)\n\
                     else {{ return 0 as usize; }};\n\
                 var stream: net::TcpStream = s;\n\
                 var payload: vec::Vec[u8] = vec::new::[u8]();\n\
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
                 let pid: i32 = {{ fork() }};\n\
                 if pid < (0 as i32) {{ return 9; }}\n\
                 if pid == (0 as i32) {{\n\
                     let rc: i32 = run_server();\n\
                     {{ _exit(rc); }}\n\
                     return rc;\n\
                 }}\n\
                 let n: usize = run_client();\n\
                 let null_status: *i32 = {{ 0 as *i32 }};\n\
                 {{ waitpid(pid, null_status, 0 as i32); }}\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         fn make_three_bytes() -> result::Result[vec::Vec[u8], result::IoError] {\n\
             var v: vec::Vec[u8] = vec::new::[u8]();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // v0.0.5 Phase 4 Slice 4C: fs.cplus now imports net + reactor +
    // future (for File::read_async). Stage them too.
    for name in &[
        "result", "vec", "fs", "io", "iterator", "option", "net", "netsys", "reactor", "future",
        "text",
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
                 var data: vec::Vec[u8] = vec::new::[u8]();\n\
                 data.push(72 as u8);\n\
                 data.push(73 as u8);\n\
                 data.push(33 as u8);\n\
                 guard let result::Result[fs::File, result::IoError]::Ok(w) = fs::create(path)\n\
                     else {{ return false; }};\n\
                 var writer: fs::File = w;\n\
                 guard let result::Result[usize, result::IoError]::Ok(wrote) = writer.write_all(data)\n\
                     else {{ return false; }};\n\
                 if wrote == (0 as usize) {{ return false; }}\n\
                 writer.close();\n\
                 return true;\n\
             }}\n\
             fn read_len(path: str) -> usize {{\n\
                 guard let result::Result[fs::File, result::IoError]::Ok(r) = fs::open_read(path)\n\
                     else {{ return 0 as usize; }};\n\
                 var reader: fs::File = r;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         struct Maker { _x: i32 }\n\
         fn make_maker() -> Maker { return Maker { _x: 0 as i32 }; }\n\
         impl Maker {\n\
             fn make_buf(this) -> vec::Vec[u8] {\n\
                 var buf: vec::Vec[u8] = vec::new::[u8]();\n\
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
             var v: vec::Vec[u8] = vec::new::[u8]();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         fn make_empty_buf() -> vec::Vec[u8] {\n\
             return vec::new::[u8]();\n\
         }\n",
    )
    .unwrap();
    // Consumer pushes onto the producer's returned Vec.
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"./maker\" as maker;\n\
         fn main() -> i32 {\n\
             var buf = maker::make_empty_buf();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var b = make_buf::[i32]();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var b = vec::Vec[i32]::with_capacity(16);\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    // R4: payload is now `Text` (stdlib), which imports vec → option + iterator.
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/thread\" as thread;\n\
         import \"stdlib/text\" as text;\n\
         fn produce() -> text::Text { return text::from_str(\"hello from worker\"); }\n\
         fn main() -> i32 {\n\
             let h: thread::JoinHandle[text::Text] = thread::spawn::[text::Text](produce);\n\
             let s: text::Text = h.join();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    // R4: async return type is now `Text` (stdlib), which imports vec → option
    // + iterator.
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/executor\" as executor;\n\
         import \"stdlib/text\" as text;\n\
         async fn inner() -> text::Text {\n\
             return text::from_str(\"hello from coro\");\n\
         }\n\
         async fn outer() -> text::Text {\n\
             let s = await inner();\n\
             return s;\n\
         }\n\
         fn main() -> i32 {\n\
             let f: future::Future[text::Text] = outer();\n\
             let s: text::Text = executor::block_on::[text::Text](f);\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         fn produce() -> *u8 { return { malloc(64 as usize) }; }\n\
         fn main() -> i32 {\n\
             let h: thread::JoinHandle[*u8] = thread::spawn::[*u8](produce);\n\
             let p: *u8 = h.join();\n\
             { free(p); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         async fn id[T](take x: T) -> T { return x; }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let box_src = include_str!("../../vendor/stdlib/src/box.cplus");
    std::fs::write(dir.join("vendor/stdlib/src/box.cplus"), box_src).unwrap();
    for name in &["text", "vec", "option", "iterator"] {
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
         import \"stdlib/text\" as text;\n\
         import \"stdlib/option\" as option;\n\
         fn main() -> i32 {\n\
             guard let option::Option[box::Box[i32]]::Some(btmp) = box::new::[i32](7) else { return 10; };\n\
             var b: box::Box[i32] = btmp;\n\
             if b.get() != 7 { return 1; }\n\
             b.set(100);\n\
             if b.get() != 100 { return 2; }\n\
             if b.unwrap() != 100 { return 3; }\n\
             let s = text::from_str(\"boxed-string\");\n\
             guard let option::Option[box::Box[text::Text]]::Some(b2) = box::new::[text::Text](s) else { return 11; };\n\
             let recovered: text::Text = b2.unwrap();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         import \"stdlib/option\" as option;\n\
         fn worker(take handle: arc::Arc[i32]) -> i32 {\n\
             return handle.get();\n\
         }\n\
         fn main() -> i32 {\n\
             guard let option::Option[arc::Arc[i32]]::Some(root) = arc::new::[i32](7) else { return 9; };\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         import \"stdlib/option\" as option;\n\
         fn main() -> i32 {\n\
             guard let option::Option[rc::Rc[i32]]::Some(a) = rc::new::[i32](42) else { return 9; };\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         import \"stdlib/option\" as option;\n\
         fn worker(take m: mutex::Mutex[i32]) -> i32 {\n\
             var g = m.lock();\n\
             let cur: i32 = g.get();\n\
             g.set(cur + 1);\n\
             return 0;\n\
         }\n\
         fn main() -> i32 {\n\
             guard let option::Option[mutex::Mutex[i32]]::Some(root) = mutex::new::[i32](10) else { return 9; };\n\
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

/// #5: a `MutexGuard` takes its own refcount in `lock`, so it can outlive the
/// `Mutex` handle that produced it without dangling. Here `make_locked`'s only
/// `Mutex` handle drops at function exit, yet the returned guard stays valid;
/// the inner Drop-carrying value is torn down exactly once when the guard
/// finally drops. Pre-fix the guard held no reference, so the handle's drop
/// freed the heap block and the escaped guard was a use-after-free / would
/// double-drop the inner value. Builds and runs clean under ASan; the program
/// returns the inner-Drop count, which must be exactly 1.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_mutex_guard_outlives_handle_no_uaf() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"muxesc\"\n\n[[bin]]\nname = \"muxesc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/mutex.cplus"),
        include_str!("../../vendor/stdlib/src/mutex.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/atomic.cplus"),
        include_str!("../../vendor/stdlib/src/atomic.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/mutex\" as mutex;\n\
         import \"stdlib/option\" as option;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         static FREES: i32 = 0;\n\
         struct Res { p: *u8 }\n\
         impl Res { fn drop(ref this) { { FREES = FREES +% 1; free(this.p); } return; } }\n\
         fn make_locked() -> option::Option[mutex::MutexGuard[Res]] {\n\
             let r: Res = Res { p: { malloc(8 as usize) } };\n\
             guard let option::Option[mutex::Mutex[Res]]::Some(m) = mutex::new::[Res](r) else { return option::Option[mutex::MutexGuard[Res]]::None; };\n\
             return option::Option[mutex::MutexGuard[Res]]::Some(m.lock());\n\
         }\n\
         fn main() -> i32 {\n\
             { guard let option::Option[mutex::MutexGuard[Res]]::Some(_g) = make_locked() else { return 2; }; }\n\
             return { FREES };\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        let st = cmd.status().expect("invoke cpc");
        assert!(st.success(), "cpc build failed with {}", sanitizer);
        let bin = dir.join("target/debug/muxesc");
        let run = Command::new(&bin).output().expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged the escaped guard ({}): {stderr}",
            sanitizer
        );
        assert_eq!(
            run.status.code(),
            Some(1),
            "escaped guard must drop its inner value exactly once ({}): stderr={stderr}",
            sanitizer
        );
    }
}

/// `Box::set` now drops the value the box currently owns before storing the new
/// one (mirrors `Vec::set`). Pre-fix it overwrote the old value, leaking it for
/// a Drop `T`. The program boxes one resource, `set`s a second, then drops the
/// box in an inner scope; the alloc/free counter must balance (exactly two
/// allocs, two frees). Runs clean under ASan.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_box_set_drops_old_value() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"boxset\"\n\n[[bin]]\nname = \"boxset\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/box.cplus"),
        include_str!("../../vendor/stdlib/src/box.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/box\" as box;\n\
         import \"stdlib/option\" as option;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         static A: i32 = 0;\n\
         static F: i32 = 0;\n\
         struct R { p: *u8 }\n\
         impl R { fn drop(ref this) { { F = F +% 1; free(this.p); } return; } }\n\
         fn mk() -> R { { A = A +% 1; } return R { p: { malloc(8 as usize) } }; }\n\
         fn main() -> i32 {\n\
             { guard let option::Option[box::Box[R]]::Some(btmp) = box::new::[R](mk()) else { return 5; }; var b: box::Box[R] = btmp; b.set(mk()); }\n\
             return { A -% F };\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(dir.join("target/debug/boxset"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged Box::set ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "Box::set must drop the old value (balanced alloc/free) ({sanitizer})"
        );
    }
}

/// `Box::get` bit-copies the boxed value out without consuming the box, so it
/// lives in a `Copy`-bounded impl block (`impl Box[T: Copy]`). Calling it on a
/// non-Copy `T` is rejected with E0502 — pre-fix it silently bit-duplicated an
/// owner and double-freed. (This also exercises impl-block bound enforcement.)
/// Non-Copy boxes remain usable via `new` / `set` / `unwrap`, covered above.
#[test]
fn stdlib_box_get_noncopy_rejected_e0502() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"boxget\"\n\n[[bin]]\nname = \"boxget\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/box.cplus"),
        include_str!("../../vendor/stdlib/src/box.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/box\" as box;\n\
         import \"stdlib/option\" as option;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         struct R { p: *u8 }\n\
         impl R { fn drop(ref this) { { free(this.p); } return; } }\n\
         fn mk() -> R { return R { p: { malloc(8 as usize) } }; }\n\
         fn main() -> i32 {\n\
             guard let option::Option[box::Box[R]]::Some(b) = box::new::[R](mk()) else { return 1; };\n\
             let _r: R = b.get();\n\
             return 0;\n\
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
        "expected compile failure for Box::get on a non-Copy T"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0502"),
        "expected E0502 (Copy bound not satisfied) in stderr, got: {stderr}"
    );
}

/// TEXT.R1 at the assignment site: a bare string literal assigned into a `Text`
/// binding is constructed into an owned `Text`, like the `let`-init coercion —
/// so `let mut s: Text = "a"; s = "bb";` works. The reassignment must also drop
/// the old `Text`'s heap buffer first (the #8 pre-drop), so repeated literal
/// reassignment is leak- and double-free-free. Runs clean under ASan and the
/// final value is correct.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_reassign_str_literal_coerces() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"txtre\"\n\n[[bin]]\nname = \"txtre\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             var s: text::Text = \"a\";\n\
             s = \"bb\";\n\
             s = \"ccc\";\n\
             s = 9.to_text();\n\
             s = \"dddd\";\n\
             if s.len() != (4 as usize) { return 1; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(dir.join("target/debug/txtre"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged Text reassign ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "Text literal reassignment must coerce + drop old cleanly ({sanitizer})"
        );
    }
}

/// `Vec[T]::new()` (associated-fn-call syntax) with a *nominal* element type —
/// e.g. a user struct — used to crash the compiler: the call parses as a
/// `GenericEnumCall`, and the monomorphize free-fn-dispatch rewrite re-derived
/// the element `Ty` from the AST (which can't resolve a nominal name), so the
/// constructor was left mangled to the bare generic `vec.new` and codegen
/// panicked. Primitives (`Vec[u8]::new()`) and the free-fn form
/// (`vec::new::[T]()`) happened to work. The fix keys the rewrite off sema's
/// authoritative `call_monos` args. Here a non-Copy Drop struct is stored via
/// the assoc form and all elements drop cleanly (ASan).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_vec_assoc_new_with_struct_element() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"vecassoc\"\n\n[[bin]]\nname = \"vecassoc\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["vec", "option", "iterator"] {
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
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         static A: i32 = 0;\n\
         static F: i32 = 0;\n\
         struct R { p: *u8 }\n\
         impl R { fn drop(ref this) { { F = F +% 1; free(this.p); } return; } }\n\
         fn mk() -> R { { A = A +% 1; } return R { p: { malloc(8 as usize) } }; }\n\
         fn main() -> i32 {\n\
             { var v: vec::Vec[R] = vec::Vec[R]::new(); v.push(mk()); v.push(mk()); v.push(mk()); }\n\
             return { A -% F };\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer}) — Vec[Struct]::new() regressed"
        );
        let run = Command::new(dir.join("target/debug/vecassoc"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged Vec[Struct] assoc ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "Vec[Struct]::new() assoc form must build + drop all elements ({sanitizer})"
        );
    }
}

/// `break` out of an iterator-protocol `for x in <iter>` loop must not crash.
/// The gen-fn / iterator coroutine's yield-suspend mapped its destroy edge to
/// `llvm.trap`, so abandoning the loop early (`break`) — which calls
/// `coro.destroy` on the still-suspended coroutine — SIGTRAPped (exit 133).
/// Full-drain, `continue`, and early `return` worked; only `break` (and a
/// dropped-undrained iterator) hit the trap. The destroy edge now routes to the
/// coroutine cleanup, like the final-suspend edge. Covers both a user `gen fn`
/// and `Vec::iter`, and checks the partial result is correct.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_for_in_break_does_not_crash() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"brk\"\n\n[[bin]]\nname = \"brk\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["vec", "option", "iterator"] {
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
        "import \"stdlib/iterator\" as iterator;\n\
         import \"stdlib/vec\" as vec;\n\
         gen fn upto(n: i32) -> i32 { var i: i32 = 0; while i < n { yield i; i = i +% 1; } return; }\n\
         fn main() -> i32 {\n\
             // break out of a user gen-fn loop after summing 0+1+2 = 3\n\
             var a: i32 = 0;\n\
             for x in upto(100) { if x == 3 { break; } a = a +% x; }\n\
             if a != 3 { return 1; }\n\
             // break out of a Vec::iter loop\n\
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(10); v.push(20); v.push(30);\n\
             var b: i32 = 0;\n\
             for y in v.iter() { if y == 20 { break; } b = b +% y; }\n\
             if b != 10 { return 2; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(dir.join("target/debug/brk"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged for-in break ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "break out of a for-in iterator loop must not crash and must yield the right partial ({sanitizer})"
        );
    }
}

/// `break`-ing out of a `for x in g()` over a `gen fn` that holds Drop locals
/// across a yield must DROP those locals (not leak them). The destroy edge of
/// each yield routes to a per-yield cancel block that drops the in-scope locals
/// before freeing the frame. Verifies exactly-once teardown via an alloc/free
/// counter (balance 0), including the staggered-init case (a local declared
/// after the break point must NOT be dropped), and that full drain does not
/// double-free. ASan-clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_for_in_break_drops_inscope_coroutine_locals() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"brkd\"\n\n[[bin]]\nname = \"brkd\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &["vec", "option", "iterator"] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!("vendor/stdlib/src/{name}.cplus")),
        )
        .unwrap();
        std::fs::write(dir.join(format!("vendor/stdlib/src/{name}.cplus")), src).unwrap();
    }
    // `phase` selects: 0 = break with one Drop local; 1 = staggered (second
    // local declared after the break point, must not drop); 2 = full drain
    // (must not double-free). All must leave alloc/free balanced (exit 0).
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/iterator\" as iterator;\n\
         extern fn malloc(n: usize) -> *u8;\n\
         extern fn free(p: *u8);\n\
         static A: i32 = 0;\n\
         static F: i32 = 0;\n\
         struct R { p: *u8 }\n\
         impl R { fn drop(ref this) { { F = F +% 1; free(this.p); } return; } }\n\
         fn mk() -> R { { A = A +% 1; } return R { p: { malloc(8 as usize) } }; }\n\
         gen fn one() -> i32 { let r: R = mk(); yield 1; yield 2; return; }\n\
         gen fn staggered() -> i32 { let r1: R = mk(); yield 1; let r2: R = mk(); yield 2; return; }\n\
         fn main() -> i32 {\n\
             { for x in one() { if x == 1 { break; } } }\n\
             if { A -% F } != 0 { return 1; }\n\
             { A = 0; F = 0; }\n\
             { for x in staggered() { if x == 1 { break; } } }\n\
             if { A -% F } != 0 { return 2; }\n\
             { A = 0; F = 0; }\n\
             { var s: i32 = 0; for x in one() { s = s +% x; } }\n\
             if { A -% F } != 0 { return 3; }\n\
             return 0;\n\
         }\n",
    )
    .unwrap();
    for sanitizer in &["", "--asan"] {
        let mut cmd = Command::new(cpc);
        cmd.arg("build").current_dir(&dir);
        if !sanitizer.is_empty() {
            cmd.arg(sanitizer);
        }
        assert!(
            cmd.status().expect("invoke cpc").success(),
            "build failed ({sanitizer})"
        );
        let run = Command::new(dir.join("target/debug/brkd"))
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            !stderr.contains("AddressSanitizer"),
            "ASan flagged coroutine cancel-drop ({sanitizer}): {stderr}"
        );
        assert_eq!(
            run.status.code(),
            Some(0),
            "coroutine locals must drop exactly once on early break (incl. staggered + full-drain) ({sanitizer})"
        );
    }
}

/// `executor::block_on(amain())` must type-check and run *without* a turbofish —
/// the type arg `T` is inferred from the `Future[i32]` argument. Before the
/// generic-struct unification fix, `block_on(f())` failed (E0302 "struct vs
/// struct") and every async entry point needed `block_on::[T](...)`. This is
/// the canonical async-entry idiom (see the "no async main" decision: keep the
/// entry point a library call, but make it ergonomic).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_block_on_infers_type_arg_no_turbofish() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"boninf\"\n\n[[bin]]\nname = \"boninf\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &[
        "future",
        "executor",
        "reactor",
        "reactor_linux",
        "reactor_windows",
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
        "import \"stdlib/future\" as future;\n\
         import \"stdlib/executor\" as executor;\n\
         async fn amain() -> i32 { return 42; }\n\
         fn main() -> i32 {\n\
             let r: i32 = executor::block_on(amain());\n\
             if r != 42 { return 1; }\n\
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
        "block_on without turbofish must type-check (generic-struct inference)"
    );
    let run = Command::new(dir.join("target/debug/boninf"))
        .status()
        .expect("run");
    assert_eq!(
        run.code(),
        Some(0),
        "block_on(amain()) must run and return the inner result"
    );
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         import \"stdlib/option\" as option;\n\
         fn producer(take ch: channel::Channel[i32]) -> i32 {\n\
             var i: i32 = 0;\n\
             while i < 100 {\n\
                 ch.send(i);\n\
                 i = i +% 1;\n\
             }\n\
             return 0;\n\
         }\n\
         fn consumer(take ch: channel::Channel[i32]) -> i32 {\n\
             var count: i32 = 0;\n\
             var done: bool = false;\n\
             while !done {\n\
                 match ch.recv() {\n\
                     channel::RecvResult[i32]::Value(_v) => { count = count +% 1; },\n\
                     channel::RecvResult[i32]::Closed => { done = true; },\n\
                 }\n\
             }\n\
             return count;\n\
         }\n\
         fn main() -> i32 {\n\
             guard let option::Option[channel::Channel[i32]]::Some(root) = channel::new::[i32]() else { return 9; };\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // `cow` now wraps `Text` (R4 migration), which imports vec → option +
    // iterator. Vendor the whole chain.
    for name in &["cow", "text", "vec", "option", "iterator"] {
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
        // main imports text so `.to_string()` yields the owned `Text` that
        // `cow::from_owned` now takes.
        "import \"stdlib/cow\" as cow;\n\
         import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let c1 = cow::from_view(\"hello\");\n\
             if cow::is_owned(c1) { return 1; }\n\
             if cow::len(c1) != (5 as usize) { return 2; }\n\
             let initial = \"world\".to_text();\n\
             let c2 = cow::from_owned(initial);\n\
             if !cow::is_owned(c2) { return 3; }\n\
             if cow::len(c2) != (5 as usize) { return 4; }\n\
             let c3 = cow::from_view(\"abc\");\n\
             let s3 = cow::into_owned(c3);\n\
             if s3.len() != (3 as usize) { return 5; }\n\
             let init2 = \"xyzpq\".to_text();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let raw: *u8 = { malloc(16 as usize) };\n\
             let p: *Ts = { raw as *Ts };\n\
             let _r: i32 = { clock_gettime(6 as i32, p) };\n\
             let s: i64 = { p[0].sec };\n\
             let n: i64 = { p[0].ns };\n\
             { free(raw); }\n\
             return s *% (1000000000 as i64) +% n;\n\
         }\n\
         fn slow_worker() -> i32 {\n\
             let _r: i32 = { usleep(200000 as u32) };\n\
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
             let _r: i32 = { usleep(250000 as u32) };\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var i: i32 = 1;\n\
             while i <= n {\n\
                 yield i;\n\
                 i = i +% (1 as i32);\n\
             }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             // Path 1: `for x in iter` desugar.\n\
             var sum: i32 = 0;\n\
             for x in count_up(5 as i32) {\n\
                 sum = sum +% x;\n\
             }\n\
             if sum != (15 as i32) { return 1 as i32; }\n\
             // Path 2: explicit `it.next()` pull-style consumption.\n\
             var it: iter::Iterator[i32] = count_up(3 as i32);\n\
             var pulled: i32 = 0;\n\
             var loops: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var i: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             { #reactor_wait_read(rfd); }\n\
             let buf: *u8 = { malloc(1 as usize) };\n\
             let n: isize = { read(rfd, buf, 1 as usize) };\n\
             let v: u8 = { *buf };\n\
             { free(buf); }\n\
             if n != (1 as isize) { return -1 as i32; }\n\
             return v as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = { malloc(8 as usize) };\n\
             let _r: i32 = { pipe(fds_buf) };\n\
             let fds_i32: *i32 = { fds_buf as *i32 };\n\
             let rfd: i32 = { *fds_i32 };\n\
             let wfd_p: *i32 = { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = { *wfd_p };\n\
             let payload: *u8 = { malloc(1 as usize) };\n\
             { *payload = 42 as u8; }\n\
             let _w: isize = { write(wfd, payload, 1 as usize) };\n\
             { free(payload); }\n\
             let f: future::Future[i32] = await_and_read(rfd);\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             { close(rfd); }\n\
             { close(wfd); }\n\
             { free(fds_buf); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    // fs::lines now yields `Text` (R4); fs imports stdlib/text.
    std::fs::write(
        dir.join("vendor/stdlib/src/text.cplus"),
        include_str!("../../vendor/stdlib/src/text.cplus"),
    )
    .unwrap();
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
             var count: i32 = 0;\n\
             var total_len: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    // fs::lines now yields `Text` (R4); fs imports stdlib/text.
    std::fs::write(
        dir.join("vendor/stdlib/src/text.cplus"),
        include_str!("../../vendor/stdlib/src/text.cplus"),
    )
    .unwrap();
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
         async fn read_first(take f: fs::File) -> i32 {{\n\
             // Re-bind locally so the body has a `mut` handle without\n\
             // tripping the E0900 (mut-pointer-pass + await) guard.\n\
             var f: fs::File = f;\n\
             let _nb: i32 = f.make_nonblocking();\n\
             let buf: *u8 = {{ malloc(1 as usize) }};\n\
             let n: isize = await f.read_async(buf, 1 as usize);\n\
             let v: u8 = {{ *buf }};\n\
             {{ free(buf); }}\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let buf: *u8 = { malloc(16 as usize) };\n\
             let _rc: i32 = { gettimeofday(buf, 0 as *u8) };\n\
             let sec: i64 = { *(buf as *i64) };\n\
             let usec: i64 = { *((buf + (8 as usize)) as *i64) };\n\
             { free(buf); }\n\
             return ((sec *% (1000 as i64)) +% (usec / (1000 as i64))) as u64;\n\
         }\n\
         async fn unit_sleep() -> i32 {\n\
             await time::sleep(50 as u64);\n\
             return 0 as i32;\n\
         }\n\
         async fn stress(n: i32) -> i32 {\n\
             let bytes: usize = (n as usize) *% (8 as usize);\n\
             let buf: *u8 = { malloc(bytes) };\n\
             let hdls: **u8 = { buf as **u8 };\n\
             var i: i32 = 0;\n\
             while i < n {\n\
                 let f: future::Future[i32] = unit_sleep();\n\
                 let slot: **u8 = { hdls + (i as usize) };\n\
                 { *slot = f.handle; }\n\
                 i = i +% (1 as i32);\n\
             }\n\
             var j: i32 = 0;\n\
             while j < n {\n\
                 let slot: **u8 = { hdls + (j as usize) };\n\
                 let h: *u8 = { *slot };\n\
                 let f: future::Future[i32] = future::Future[i32] { handle: h };\n\
                 let _r: i32 = await f;\n\
                 j = j +% (1 as i32);\n\
             }\n\
             { free(buf); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             async fn read_byte(ref this) -> i32 {\n\
                 let buf: *u8 = { malloc(1 as usize) };\n\
                 let n: isize = await net::read_fd_async(this.fd, buf, 1 as usize);\n\
                 let v: u8 = { *buf };\n\
                 { free(buf); }\n\
                 if n != (1 as isize) { return -1 as i32; }\n\
                 return v as i32;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = { malloc(8 as usize) };\n\
             let _r: i32 = { pipe(fds_buf) };\n\
             let fds_i32: *i32 = { fds_buf as *i32 };\n\
             let rfd: i32 = { *fds_i32 };\n\
             let wfd_p: *i32 = { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = { *wfd_p };\n\
             let nb: i32 = net::set_nonblocking(rfd);\n\
             if nb != (0 as i32) { return 90 as i32; }\n\
             var reader: PipeReader = PipeReader { fd: rfd };\n\
             let f: future::Future[i32] = reader.read_byte();\n\
             let payload: *u8 = { malloc(1 as usize) };\n\
             { *payload = 42 as u8; }\n\
             let _w: isize = { write(wfd, payload, 1 as usize) };\n\
             { free(payload); }\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             let _c1: i32 = { close(rfd) };\n\
             let _c2: i32 = { close(wfd) };\n\
             { free(fds_buf); }\n\
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
///   - `time::sleep(80ms)` translates to `#reactor_wait_timer(80)`
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let buf: *u8 = { malloc(16 as usize) };\n\
             let _rc: i32 = { gettimeofday(buf, 0 as *u8) };\n\
             let sec: i64 = { *(buf as *i64) };\n\
             let usec: i64 = { *((buf + (8 as usize)) as *i64) };\n\
             { free(buf); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let buf: *u8 = { malloc(1 as usize) };\n\
             let n: isize = await net::read_fd_async(rfd, buf, 1 as usize);\n\
             let v: u8 = { *buf };\n\
             { free(buf); }\n\
             if n != (1 as isize) { return -1 as i32; }\n\
             return v as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let fds_buf: *u8 = { malloc(8 as usize) };\n\
             let _r: i32 = { pipe(fds_buf) };\n\
             let fds_i32: *i32 = { fds_buf as *i32 };\n\
             let rfd: i32 = { *fds_i32 };\n\
             let wfd_p: *i32 = { fds_i32 + (1 as usize) };\n\
             let wfd: i32 = { *wfd_p };\n\
             let nb: i32 = net::set_nonblocking(rfd);\n\
             if nb != (0 as i32) { return 90 as i32; }\n\
             // Start the reader coroutine; reactor body runs eagerly,\n\
             // hits EAGAIN on the empty pipe, registers a waiter, suspends.\n\
             let f: future::Future[i32] = reader(rfd);\n\
             // Now write the byte synchronously. kqueue's EVFILT_READ on\n\
             // rfd will fire when block_on calls kevent_wait below.\n\
             let payload: *u8 = { malloc(1 as usize) };\n\
             { *payload = 42 as u8; }\n\
             let _w: isize = { write(wfd, payload, 1 as usize) };\n\
             { free(payload); }\n\
             let got: i32 = executor::block_on::[i32](f);\n\
             let _c1: i32 = { close(rfd) };\n\
             let _c2: i32 = { close(wfd) };\n\
             { free(fds_buf); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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

/// v0.0.5 Phase 1C: container `drop` invokes inner-T Drop via the
/// `#drop_in_place::[T]` intrinsic. Without this fix, every
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for name in &[
        "box", "vec", "arc", "rc", "hash_map", "atomic", "result", "iterator", "option", "text",
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
         import \"stdlib/text\" as text;\n\
         import \"stdlib/option\" as option;\n\
         fn box_scope() { guard let option::Option[box::Box[text::Text]]::Some(_b) = box::new::[text::Text](text::from_str(\"hello\")) else { return; }; return; }\n\
         fn vec_scope() {\n\
             var v: vec::Vec[text::Text] = vec::new::[text::Text]();\n\
             v.push(text::from_str(\"one\"));\n\
             v.push(text::from_str(\"two\"));\n\
             v.push(text::from_str(\"three\"));\n\
             return;\n\
         }\n\
         fn arc_scope() {\n\
             guard let option::Option[arc::Arc[text::Text]]::Some(a) = arc::new::[text::Text](text::from_str(\"arc-value\")) else { return; };\n\
             let _c: u64 = a.strong_count();\n\
             return;\n\
         }\n\
         fn rc_scope() {\n\
             guard let option::Option[rc::Rc[text::Text]]::Some(r) = rc::new::[text::Text](text::from_str(\"rc-value\")) else { return; };\n\
             let _c: u64 = r.strong_count();\n\
             return;\n\
         }\n\
         fn hm_scope() {\n\
             var m: hm::HashMap[str, i32] = hm::new::[str, i32]();\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         async fn id[T](take x: T) -> T { return x; }\n\
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

/// Regression for issue #11: a `spawn_local`'d task PLUS a nested `await`
/// of an `async fn` in the driving future used to segfault.
///
/// Async fns run their body eagerly to the first suspend point at the
/// call site (no initial_suspend), so the spawned `timed(20)` had already
/// registered its one-shot timer and parked BEFORE `spawn_local` saw it.
/// `spawn_local` then enqueued the already-parked handle, `block_on`'s
/// `drain_pending` resumed it to completion while its timer stayed armed,
/// and when that timer fired `poll_one_event` resumed the now-completed
/// coroutine (its resume pointer nulled at final suspend) → jump through
/// null → SEGV. The fix: `spawn_local` no longer enqueues a task that is
/// already tracked by the reactor/pending/awaiter machinery.
///
/// Build + run both normally and under `--asan`; expect exit 9, clean.
/// macOS-gated (kqueue timer reactor); the fix is in platform-neutral
/// codegen, so this guards every target.
#[test]
#[cfg(target_os = "macos")]
fn issue11_spawn_local_plus_nested_await_no_crash() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"i11\"\n\n[[bin]]\nname = \"i11\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/future.cplus"),
        include_str!("../../vendor/stdlib/src/future.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/executor.cplus"),
        include_str!("../../vendor/stdlib/src/executor.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/src/reactor.cplus"),
        include_str!("../../vendor/stdlib/src/reactor.cplus"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/executor\" as executor;\n\
         import \"stdlib/future\" as future;\n\
         async fn timed(ms: u64) -> i32 {\n\
             { #reactor_wait_timer(ms); }\n\
             return 0 as i32;\n\
         }\n\
         async fn outer() -> i32 {\n\
             let f: future::Future[i32] = timed(20 as u64);\n\
             executor::spawn_local::[i32](f);\n\
             await timed(40 as u64);\n\
             return 9 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             return executor::block_on::[i32](outer());\n\
         }\n",
    )
    .unwrap();

    // (1) normal build + run: used to SEGV (exit 139); must now exit 9.
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(st.success(), "issue #11 repro build failed");
    let run = Command::new(dir.join("target/debug/i11"))
        .status()
        .expect("run i11");
    assert_eq!(
        run.code(),
        Some(9),
        "spawn_local + nested await must return 9, not crash"
    );

    // (2) under --asan: the bug was a resume-after-complete coroutine UB,
    // so ASan is the definitive guard — a regression reappears as a
    // DEADLYSIGNAL rather than a clean exit 9.
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build --asan");
    assert!(st.success(), "issue #11 repro --asan build failed");
    let run = Command::new(dir.join("target/debug/i11"))
        .status()
        .expect("run i11 asan");
    assert_eq!(
        run.code(),
        Some(9),
        "spawn_local + nested await must be clean under ASan"
    );
}

/// Struct destructuring (`let TYPE { fields } = expr;`) — the builder-pattern
/// extract that a bare field move (E0509) rejects. Destructures a `Vec`-owning
/// (auto-`drop`) struct, moves the `Vec` out, and uses it. Built under `--asan`:
/// the destructure must move the `Vec` exactly once (no double-free), so a wrong
/// drop would surface as a DEADLYSIGNAL rather than a clean exit 42.
#[test]
fn struct_destructure_moves_vec_out_clean() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"d\"\n\n[[bin]]\nname = \"d\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    for (m, src) in [
        ("vec", include_str!("../../vendor/stdlib/src/vec.cplus")),
        ("option", include_str!("../../vendor/stdlib/src/option.cplus")),
        ("iterator", include_str!("../../vendor/stdlib/src/iterator.cplus")),
    ] {
        std::fs::write(dir.join(format!("vendor/stdlib/src/{m}.cplus")), src).unwrap();
    }
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/vec\" as vec;\n\
         import \"stdlib/option\" as option;\n\
         struct Builder { children: vec::Vec[i64] }\n\
         fn take_inner(take b: Builder) -> vec::Vec[i64] {\n\
             let Builder { children } = b;\n\
             return children;\n\
         }\n\
         fn main() -> i32 {\n\
             var b: Builder = Builder { children: vec::new::[i64]() };\n\
             b.children.push(40 as i64); b.children.push(2 as i64);\n\
             let v: vec::Vec[i64] = take_inner(b);\n\
             var s: i64 = 0 as i64; var i: usize = 0 as usize;\n\
             while i < v.len() {\n\
                 match v.at(i) { option::Option[*i64]::Some(p) => { s = s +% { *p }; } option::Option[*i64]::None => {} }\n\
                 i = i +% (1 as usize);\n\
             }\n\
             return s as i32;\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .arg("--asan")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build --asan");
    assert!(st.success(), "struct-destructure build failed");
    let run = Command::new(dir.join("target/debug/d"))
        .status()
        .expect("run d");
    assert_eq!(
        run.code(),
        Some(42),
        "destructure must move the Vec out exactly once (40+2), ASan-clean"
    );
}

/// v0.0.5 Phase 2B: `gen fn iter(self) -> T` on a user struct.
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
         struct Counter { n: i32 }\n\
         impl Counter {\n\
             gen fn iter(this) -> i32 {\n\
                 var i: i32 = 0;\n\
                 while i < this.n {\n\
                     yield i;\n\
                     i = i +% (1 as i32);\n\
                 }\n\
                 return;\n\
             }\n\
         }\n\
         fn main() -> i32 {\n\
             let c: Counter = Counter { n: 5 as i32 };\n\
             var sum: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             var i: i32 = 1;\n\
             while i <= 8 {\n\
                 v.push(i);\n\
                 i = i +% 1;\n\
             }\n\
             var total: i32 = 0;\n\
             var j: usize = 0 as usize;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(10 as i32);\n\
             v.push(20 as i32);\n\
             v.push(30 as i32);\n\
             var sum: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var src: vec::Vec[i32] = vec::new::[i32]();\n\
             src.push(0 -% (1 as i32));\n\
             src.push(2 as i32);\n\
             src.push(0 -% (3 as i32));\n\
             src.push(4 as i32);\n\
             src.push(5 as i32);\n\
             let positives: vec::Vec[i32] = vec::collect::[i32](src.iter().filter(is_pos));\n\
             if positives.len() != (3 as usize) { return 1 as i32; }\n\
             var sum: i32 = 0;\n\
             var i: usize = 0 as usize;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(1 as i32);\n\
             v.push(2 as i32);\n\
             v.push(3 as i32);\n\
             v.push(4 as i32);\n\
             v.push(5 as i32);\n\
             v.push(6 as i32);\n\
             // filter: keep even — sum 2+4+6 = 12\n\
             var sum: i32 = 0;\n\
             for x in v.iter().filter(is_even) {\n\
                 sum = sum +% x;\n\
             }\n\
             if sum != (12 as i32) { return 1 as i32; }\n\
             // take(3): count exactly three elements\n\
             var count: i32 = 0;\n\
             for _x in v.iter().take(3 as usize) {\n\
                 count = count +% (1 as i32);\n\
             }\n\
             if count != (3 as i32) { return 2 as i32; }\n\
             // map: double every element — sum 2+4+6+8+10+12 = 42\n\
             var sum2: i32 = 0;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var src_vec: vec::Vec[i32] = vec::new::[i32]();\n\
             src_vec.push(10 as i32);\n\
             src_vec.push(20 as i32);\n\
             src_vec.push(30 as i32);\n\
             src_vec.push(40 as i32);\n\
             src_vec.push(50 as i32);\n\
             let slice: i32[] = src_vec.as_slice();\n\
             // Extend a fresh Vec; assert total + count.\n\
             var dst: vec::Vec[i32] = vec::new::[i32]();\n\
             dst.push(1 as i32);\n\
             vec::extend_from_slice::[i32](dst, slice);\n\
             dst.push(2 as i32);\n\
             // dst = [1, 10, 20, 30, 40, 50, 2]; len = 7, sum = 153.\n\
             var sum: i32 = 0;\n\
             var i: usize = 0 as usize;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let p64: *u64 = { malloc(8 as usize) as *u64 };\n\
             atomic::atomic_store_u64(p64, 0 as u64, atomic::Ordering::SeqCst);\n\
             let prev: u64 = atomic::atomic_fetch_add_u64(p64, 10 as u64, atomic::Ordering::SeqCst);\n\
             if prev != (0 as u64) { { free(p64 as *u8); } return 1; }\n\
             let cur: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if cur != (10 as u64) { { free(p64 as *u8); } return 2; }\n\
             let _s: u64 = atomic::atomic_fetch_sub_u64(p64, 3 as u64, atomic::Ordering::SeqCst);\n\
             let after_sub: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_sub != (7 as u64) { { free(p64 as *u8); } return 3; }\n\
             let cx: u64 = atomic::atomic_compare_exchange_u64(p64, 7 as u64, 42 as u64, atomic::Ordering::SeqCst);\n\
             if cx != (7 as u64) { { free(p64 as *u8); } return 4; }\n\
             let after_cx: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_cx != (42 as u64) { { free(p64 as *u8); } return 5; }\n\
             let cx_fail: u64 = atomic::atomic_compare_exchange_u64(p64, 0 as u64, 99 as u64, atomic::Ordering::SeqCst);\n\
             if cx_fail != (42 as u64) { { free(p64 as *u8); } return 6; }\n\
             let after_fail: u64 = atomic::atomic_load_u64(p64, atomic::Ordering::SeqCst);\n\
             if after_fail != (42 as u64) { { free(p64 as *u8); } return 7; }\n\
             { free(p64 as *u8); }\n\
             let p32: *i32 = { malloc(4 as usize) as *i32 };\n\
             atomic::atomic_store_i32(p32, 0xF0 as i32, atomic::Ordering::SeqCst);\n\
             let _o: i32 = atomic::atomic_fetch_or_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let or_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if or_val != (0xFF as i32) { { free(p32 as *u8); } return 8; }\n\
             let _a: i32 = atomic::atomic_fetch_and_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let and_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if and_val != (0x0F as i32) { { free(p32 as *u8); } return 9; }\n\
             let _x: i32 = atomic::atomic_fetch_xor_i32(p32, 0x0F as i32, atomic::Ordering::SeqCst);\n\
             let xor_val: i32 = atomic::atomic_load_i32(p32, atomic::Ordering::SeqCst);\n\
             if xor_val != (0 as i32) { { free(p32 as *u8); } return 10; }\n\
             { free(p32 as *u8); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             let p: *u64 = { malloc(8 as usize) as *u64 };\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/thread\" as thread;\n\
         import \"stdlib/text\" as text;\n\
         struct Range { start: i64, end: i64 }\n\
         fn sum_range(r: Range) -> i64 {\n\
             var total: i64 = 0 as i64;\n\
             var i: i64 = r.start;\n\
             while i < r.end {\n\
                 total = total +% i;\n\
                 i = i +% (1 as i64);\n\
             }\n\
             return total;\n\
         }\n\
         fn measure(take s: text::Text) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let left:  Range = Range { start: 1 as i64,   end: 501 as i64  };\n\
             let right: Range = Range { start: 501 as i64, end: 1001 as i64 };\n\
             let h1: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](left, sum_range);\n\
             let h2: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](right, sum_range);\n\
             let total: i64 = h1.join() +% h2.join();\n\
             if total != (500500 as i64) { return 1; }\n\
             let s: text::Text = text::from_str(\"hello, threaded world\");\n\
             let hs: thread::JoinHandle[i64] = thread::spawn_with::[text::Text, i64](s, measure);\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/thread\" as thread;\n\
         import \"stdlib/text\" as text;\n\
         fn measure(take s: text::Text) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let s: text::Text = text::from_str(\"hello, threaded world\");\n\
             let h: thread::JoinHandle[i64] = thread::spawn_with::[text::Text, i64](s, measure);\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    for name in &["text", "vec", "option", "iterator"] {
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
        "import \"stdlib/thread\" as thread;\n\
         import \"stdlib/text\" as text;\n\
         fn measure(take s: text::Text) -> i64 { return s.len() as i64; }\n\
         fn main() -> i32 {\n\
             let s: text::Text = text::from_str(\"hi\");\n\
             let h: thread::JoinHandle[i64] = thread::spawn_with::[text::Text, i64](s, measure);\n\
             // Post-take use: borrow checker rejects.\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var i: i64 = 0 as i64;\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
             var i: i32 = 0 as i32;\n\
             while i < (100000 as i32) {\n\
                 { *counter = *counter +% (1 as u64); }\n\
                 i = i +% (1 as i32);\n\
             }\n\
             return 0 as i32;\n\
         }\n\
         fn main() -> i32 {\n\
             let counter: *u64 = { malloc(8 as usize) as *u64 };\n\
             { *counter = 0 as u64; }\n\
             let h1: thread::JoinHandle[i32] = thread::spawn_with::[*u64, i32](counter, bump_racy);\n\
             let h2: thread::JoinHandle[i32] = thread::spawn_with::[*u64, i32](counter, bump_racy);\n\
             let _r1: i32 = h1.join();\n\
             let _r2: i32 = h2.join();\n\
             { free(counter as *u8); }\n\
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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

/// Stage the stdlib modules the Windows async-I/O tests import (+ their
/// transitive deps), including the Winsock variants the resolver swaps in on
/// Windows: `reactor_windows.cplus` (WSAPoll readiness) and
/// `netsys_windows.cplus` (recv/send/closesocket/ioctlsocket + WSAStartup).
/// The base `reactor.cplus`/`netsys.cplus` are staged alongside so the
/// platform-override resolution has a base to shadow.
#[cfg(target_os = "windows")]
fn stage_win_async_stdlib(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("vendor/stdlib/src")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    let files: &[(&str, &str)] = &[
        ("future.cplus", include_str!("../../vendor/stdlib/src/future.cplus")),
        ("executor.cplus", include_str!("../../vendor/stdlib/src/executor.cplus")),
        ("reactor.cplus", include_str!("../../vendor/stdlib/src/reactor.cplus")),
        ("reactor_windows.cplus", include_str!("../../vendor/stdlib/src/reactor_windows.cplus")),
        ("net.cplus", include_str!("../../vendor/stdlib/src/net.cplus")),
        ("netsys.cplus", include_str!("../../vendor/stdlib/src/netsys.cplus")),
        ("netsys_windows.cplus", include_str!("../../vendor/stdlib/src/netsys_windows.cplus")),
        ("time.cplus", include_str!("../../vendor/stdlib/src/time.cplus")),
        ("result.cplus", include_str!("../../vendor/stdlib/src/result.cplus")),
        ("vec.cplus", include_str!("../../vendor/stdlib/src/vec.cplus")),
        ("iterator.cplus", include_str!("../../vendor/stdlib/src/iterator.cplus")),
        ("option.cplus", include_str!("../../vendor/stdlib/src/option.cplus")),
    ];
    for (name, src) in files {
        std::fs::write(dir.join("vendor/stdlib/src").join(name), src).unwrap();
    }
}

/// v0.0.24 (issue #5): Windows async I/O round-trip. The Windows reactor
/// (`reactor_windows.cplus`) wakes a suspended coroutine via WSAPoll
/// readiness — the readiness analogue of the macOS kqueue / Linux epoll
/// backends — and the Winsock socket stack (`netsys_windows.cplus`) routes
/// recv/send/closesocket/ioctlsocket + WSAStartup behind netsys. A C+ async
/// client connects, sends 'A', and reads the echoed byte under `block_on`,
/// driven by WSAPoll rather than busy-polling.
#[test]
#[cfg(target_os = "windows")]
fn windows_async_tcp_echo_round_trip() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"wecho\"\n\n[[bin]]\nname = \"wecho\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    stage_win_async_stdlib(&dir);
    std::fs::write(
        dir.join("src/main.cplus"),
        r#"import "stdlib/executor" as executor;
import "stdlib/future" as future;
import "stdlib/net" as net;
import "stdlib/result" as result;
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn getenv(name: *u8) -> *u8;
extern fn atoi(s: *u8) -> i32;

fn target_port() -> u16 {
    let env_p: *u8 = { getenv(#str_ptr("FETCH_PORT\0")) };
    let null_p: *u8 = { 0 as *u8 };
    if env_p == null_p { return 7878 as u16; }
    return { atoi(env_p) } as u16;
}

async fn fetch_one_byte(port: u16) -> i32 {
    guard let result::Result[net::TcpStream, result::IoError]::Ok(s) = net::connect_tcp("127.0.0.1", port)
        else { return 0 -% 1 as i32; };
    var stream: net::TcpStream = s;
    let _nb: i32 = stream.make_nonblocking();
    let req: *u8 = { malloc(1 as usize) };
    { *req = 0x41 as u8; }
    let _w: isize = await stream.write_all_async(req, 1 as usize);
    { free(req); }
    let buf: *u8 = { malloc(1 as usize) };
    let n: isize = await stream.read_async(buf, 1 as usize);
    if n != (1 as isize) { { free(buf); } return 0 -% 2 as i32; }
    let v: u8 = { *buf };
    { free(buf); }
    return v as i32;
}

fn main() -> i32 {
    let f: future::Future[i32] = fetch_one_byte(target_port());
    return executor::block_on::[i32](f);
}
"#,
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (windows async echo)");
    let code = run_against_echo_server(&dir.join("target/debug/wecho"));
    assert_eq!(
        code, 0x41,
        "expected WSAPoll-woken async read to yield echoed 'A' (0x41); got {code}"
    );
}

/// v0.0.24 (issue #5, acceptance #2): on Windows a `time::sleep` (timer
/// source) and a pending socket read (WSAPoll readiness) both fire correctly
/// in the same `block_on`. The async fn first awaits a 25ms timer, then
/// connects + reads the echoed byte; main asserts the byte arrived (0x41)
/// and that the timer actually elapsed (>= 20ms), so both reactor sources
/// are exercised in one drive loop.
#[test]
#[cfg(target_os = "windows")]
fn windows_async_timer_and_socket_coexist() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"wcoex\"\n\n[[bin]]\nname = \"wcoex\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    ).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    stage_win_async_stdlib(&dir);
    std::fs::write(
        dir.join("src/main.cplus"),
        r#"import "stdlib/executor" as executor;
import "stdlib/future" as future;
import "stdlib/net" as net;
import "stdlib/time" as time;
import "stdlib/result" as result;
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn getenv(name: *u8) -> *u8;
extern fn atoi(s: *u8) -> i32;
extern fn GetTickCount64() -> u64;

fn target_port() -> u16 {
    let env_p: *u8 = { getenv(#str_ptr("FETCH_PORT\0")) };
    let null_p: *u8 = { 0 as *u8 };
    if env_p == null_p { return 7878 as u16; }
    return { atoi(env_p) } as u16;
}

async fn sleep_then_read(port: u16) -> i32 {
    await time::sleep(25 as u64);
    guard let result::Result[net::TcpStream, result::IoError]::Ok(s) = net::connect_tcp("127.0.0.1", port)
        else { return 0 -% 1 as i32; };
    var stream: net::TcpStream = s;
    let _nb: i32 = stream.make_nonblocking();
    let req: *u8 = { malloc(1 as usize) };
    { *req = 0x41 as u8; }
    let _w: isize = await stream.write_all_async(req, 1 as usize);
    { free(req); }
    let buf: *u8 = { malloc(1 as usize) };
    let n: isize = await stream.read_async(buf, 1 as usize);
    if n != (1 as isize) { { free(buf); } return 0 -% 2 as i32; }
    let v: u8 = { *buf };
    { free(buf); }
    return v as i32;
}

fn main() -> i32 {
    let t0: u64 = { GetTickCount64() };
    let got: i32 = executor::block_on::[i32](sleep_then_read(target_port()));
    let elapsed: u64 = { GetTickCount64() } -% t0;
    if got != (0x41 as i32) { return got; }
    if elapsed < (20 as u64) { return 50 as i32; }
    return 0 as i32;
}
"#,
    )
    .unwrap();
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build failed (windows timer+socket coexist)");
    let code = run_against_echo_server(&dir.join("target/debug/wcoex"));
    assert_eq!(
        code, 0,
        "expected timer + WSAPoll socket read to both fire in one block_on; got {code}"
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
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

/// v0.0.14: container element drop — verify (by count, not just crash-free)
/// that dropping a `Vec[T]` runs each element's `drop` exactly once via the
/// `#drop_in_place::[T]` loop, including when the Vec is itself an
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
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
         static DROPS: i32 = 0;\n\
         struct Cell { tag: i32 }\n\
         impl Cell { fn drop(ref this) { { DROPS = DROPS +% 1; }; } }\n\
         struct Wrap { items: vec::Vec[Cell], name: i32 }\n\
         fn direct() {\n\
             var v: vec::Vec[Cell] = vec::new::[Cell]();\n\
             v.push(Cell { tag: 1 });\n\
             v.push(Cell { tag: 2 });\n\
             v.push(Cell { tag: 3 });\n\
             return;\n\
         }\n\
         fn nested() {\n\
             var v: vec::Vec[Cell] = vec::new::[Cell]();\n\
             v.push(Cell { tag: 1 });\n\
             v.push(Cell { tag: 2 });\n\
             let w: Wrap = Wrap { items: v, name: 9 };\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             direct();\n\
             nested();\n\
             return { DROPS };\n\
         }\n",
    )
    .unwrap();
    let bin = dir.join("vd");
    let st = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for vec element-drop count test"
    );
    let run = Command::new(dir.join("target/debug/vd"))
        .status()
        .expect("run vd");
    // 3 (direct) + 2 (nested, auto-dropped through Wrap) = 5 element drops.
    assert_eq!(
        run.code(),
        Some(5),
        "expected 5 element drops, got {:?}",
        run.code()
    );
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
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
         static DROPS: i32 = 0;\n\
         static SUM: i32 = 0;\n\
         struct Leaf { tag: i32 }\n\
         impl Leaf { fn drop(ref this) { { DROPS = DROPS +% 1; }; } }\n\
         enum Node { One(Leaf), Many(vec::Vec[Node]) }\n\
         enum Parse { Ok(Node, i32), Fail(i32) }\n\
         fn make_inner() -> Parse {\n\
             var kids: vec::Vec[Node] = vec::new::[Node]();\n\
             kids.push(Node::One(Leaf { tag: 1 }));\n\
             kids.push(Node::One(Leaf { tag: 2 }));\n\
             return Parse::Ok(Node::Many(kids), 0);\n\
         }\n\
         fn build() -> Node {\n\
             var elems: vec::Vec[Node] = vec::new::[Node]();\n\
             let r: Parse = make_inner();\n\
             match r {\n\
                 Parse::Ok(v, rp) => { let _p: i32 = rp; elems.push(v); }\n\
                 Parse::Fail(rp) => { return Node::One(Leaf { tag: rp }); }\n\
             }\n\
             return Node::Many(elems);\n\
         }\n\
         fn count(n: Node) -> i32 {\n\
             return match n {\n\
                 Node::One(l) => l.tag,\n\
                 Node::Many(kids) => {\n\
                     var total: i32 = 0;\n\
                     var i: usize = 0 as usize;\n\
                     while i < kids.len() { match kids.at(i) { option::Option[*Node]::Some(p) => { total = total +% count({ *p }); } option::Option[*Node]::None => {} } i = i +% (1 as usize); }\n\
                     total\n\
                 }\n\
             };\n\
         }\n\
         fn run_once() {\n\
             let n: Node = build();\n\
             { SUM = SUM +% count(n); }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             var iter: i32 = 0;\n\
             while iter < 8 { run_once(); iter = iter +% 1; }\n\
             if { SUM } != 24 { return 100; }\n\
             return { DROPS };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for enum-move double-free test"
    );
    let run = Command::new(dir.join("target/debug/df"))
        .status()
        .expect("run df");
    // 2 leaves per iter × 8 iters = 16 drops, each exactly once. A double-free
    // (the bug) crashes or yields a different count.
    assert_eq!(
        run.code(),
        Some(16),
        "expected 16 leaf drops (no double-free), got {:?}",
        run.code()
    );
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
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
         static DROPS: i32 = 0;\n\
         static SUM: i32 = 0;\n\
         struct Leaf { tag: i32 }\n\
         impl Leaf { fn drop(ref this) { { DROPS = DROPS +% 1; }; } }\n\
         enum Node { One(Leaf), Many(vec::Vec[Node]) }\n\
         enum Parse { Ok(Node), Fail }\n\
         fn make() -> Parse {\n\
             var kids: vec::Vec[Node] = vec::new::[Node]();\n\
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
         fn count(n: Node) -> i32 {\n\
             return match n {\n\
                 Node::One(l) => l.tag,\n\
                 Node::Many(kids) => {\n\
                     var total: i32 = 0;\n\
                     var i: usize = 0 as usize;\n\
                     while i < kids.len() { match kids.at(i) { option::Option[*Node]::Some(p) => { total = total +% count({ *p }); } option::Option[*Node]::None => {} } i = i +% (1 as usize); }\n\
                     total\n\
                 }\n\
             };\n\
         }\n\
         fn run_once() {\n\
             let n: Node = unwrap_or(false);\n\
             { SUM = SUM +% count(n); }\n\
             return;\n\
         }\n\
         fn main() -> i32 {\n\
             var iter: i32 = 0;\n\
             while iter < 8 { run_once(); iter = iter +% 1; }\n\
             if { SUM } != 24 { return 100; }\n\
             return { DROPS };\n\
         }\n",
    )
    .unwrap();
    let st = Command::new(cpc)
        .current_dir(&dir)
        .arg("build")
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build failed for conditional-branch-tail move test"
    );
    let run = Command::new(dir.join("target/debug/cb"))
        .status()
        .expect("run cb");
    assert_eq!(
        run.code(),
        Some(16),
        "expected 16 leaf drops (no double-free), got {:?}",
        run.code()
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
    assert!(
        out.status.success(),
        "binary exited non-zero: {}",
        out.status
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hash_map combos: 6/6 ok\n"
    );
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
    std::fs::write(dir.join("vendor/stdlib/src/status.cplus"), include_str!("../../vendor/stdlib/src/status.cplus")).unwrap();
    std::fs::write(dir.join("vendor/stdlib/src/option.cplus"), include_str!("../../vendor/stdlib/src/option.cplus")).unwrap();
    std::fs::write(
        dir.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\n",
    )
    .unwrap();
    // `text` imports `vec` (for `split`), which imports `option` + `iterator`.
    for name in &["text", "option", "vec", "iterator"] {
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
/// clone / as_str) returns correct results. The exit code is the
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
             var t: text::Text = text::from_str(\"hello\");\n\
             t.push_str(\", world\");\n\
             var score: i32 = 0;\n\
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
             let v: str = { c };\n\
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
             var score: i32 = 0;\n\
             if s.len() == (12 as usize) { score = score +% 1; }\n\
             if s.starts_with(\"hello\") { score = score +% 1; }\n\
             if s.contains(\"o, w\") { score = score +% 1; }\n\
             let v: str = { s };\n\
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

/// TEXT.R1c: a string literal for an owning `Text` arg constructs an owned
/// `Text` across the free-fn, method, and assoc-fn call paths. Builds, runs,
/// each callee owns and drops its arg clean (ASan-verified separately).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_literal_as_arg_constructs_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         struct Setter { tag: i32 }\n\
         impl Setter {\n\
             fn set(this, t: text::Text) -> usize { return t.len(); }\n\
             fn make(t: text::Text) -> usize { return t.len(); }\n\
         }\n\
         fn take(t: text::Text) -> usize { return t.len(); }\n\
         fn main() -> i32 {\n\
             var score: i32 = 0;\n\
             if take(\"hello\") == (5 as usize) { score = score +% 1; }\n\
             let s: Setter = Setter { tag: 1 };\n\
             if s.set(\"hi there\") == (8 as usize) { score = score +% 1; }\n\
             if Setter::make(\"yo\") == (2 as usize) { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of literal Text args failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(
        run.code(),
        Some(3),
        "free/method/assoc Text-arg checks must pass"
    );
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
             var score: i32 = 0;\n\
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
             var score: i32 = 0;\n\
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
             var score: i32 = 0;\n\
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
    assert!(
        st.success(),
        "cpc build of struct Text field literal failed"
    );
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "struct-field Text checks must pass");
}

/// TEXT.R2: string interpolation produces an owned `Text` (when `stdlib/text`
/// is imported). Covers a primitive part (`${n}`) and an embedded owned-`Text`
/// part (`${a}` — its bytes are copied, the binding still drops it once).
/// Builds, runs, ASan-clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_interpolation_produces_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let n: i32 = 42;\n\
             let a: text::Text = \"world\";\n\
             let s: text::Text = \"count=${n} hi ${a}\";\n\
             var score: i32 = 0;\n\
             if s.len() == (17 as usize) { score = score +% 1; }\n\
             if s.starts_with(\"count=42\") { score = score +% 1; }\n\
             if s.contains(\"hi world\") { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of interpolation -> Text failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "interpolation-Text checks must pass");
}

/// TEXT.R3a: the rounded-out stdlib `Text` API — `trim`, `rfind`, `slice`
/// (copies), and `split` into a `Vec[Text]` — all pure C+ stdlib (no compiler
/// change). Builds, runs, and the owned pieces + the `Vec[Text]` drop clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_slice_rfind_trim_split() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         import \"stdlib/option\" as option;\n\
         import \"stdlib/vec\" as vec;\n\
         fn main() -> i32 {\n\
             let s: text::Text = \"  hello,world,foo  \";\n\
             var score: i32 = 0;\n\
             let t: text::Text = s.trim();\n\
             if t.len() == (15 as usize) { score = score +% 1; }\n\
             match t.rfind(\",\") {\n\
                 option::Option[usize]::Some(i) => { if i == (11 as usize) { score = score +% 1; } }\n\
                 option::Option[usize]::None => { }\n\
             }\n\
             match t.slice(0 as usize, 5 as usize) {\n\
                 option::Option[text::Text]::Some(sl) => { if sl.starts_with(\"hello\") { score = score +% 1; } }\n\
                 option::Option[text::Text]::None => { }\n\
             }\n\
             let parts: vec::Vec[text::Text] = t.split(\",\");\n\
             if parts.len() == (3 as usize) { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(
        st.success(),
        "cpc build of Text slice/rfind/trim/split failed"
    );
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(
        run.code(),
        Some(4),
        "slice/rfind/trim/split checks must pass"
    );
}

/// TEXT.R3b: `Text::c_str` builds an owning, NUL-terminated `CString` for C FFI.
/// A real libc `strlen` round-trip confirms the terminator; an interior NUL is
/// rejected with `None`. The `CString` frees its buffer on drop (ASan-clean).
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_c_str_round_trips_through_libc() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         import \"stdlib/option\" as option;\n\
         extern fn strlen(s: *u8) -> usize;\n\
         fn main() -> i32 {\n\
             var score: i32 = 0;\n\
             let t: text::Text = \"hello\";\n\
             match t.c_str() {\n\
                 option::Option[text::CString]::Some(cs) => {\n\
                     if { strlen(cs.as_ptr()) } == (5 as usize) { score = score +% 1; }\n\
                     if cs.len() == (5 as usize) { score = score +% 1; }\n\
                 }\n\
                 option::Option[text::CString]::None => { }\n\
             }\n\
             let withnul: text::Text = \"a\\0b\";\n\
             match withnul.c_str() {\n\
                 option::Option[text::CString]::Some(cs2) => { let _ = cs2.len(); }\n\
                 option::Option[text::CString]::None => { score = score +% 1; }\n\
             }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of Text::c_str failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(
        run.code(),
        Some(3),
        "c_str strlen round-trip + interior-NUL checks"
    );
}

/// TEXT.R3b: `.to_string()` produces an owned `Text` (when `stdlib/text` is
/// imported) — consistent with interpolation. Builds, runs, drops clean.
#[test]
#[cfg(target_os = "macos")]
fn stdlib_text_to_string_produces_owned_text() {
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    setup_text_project(
        &dir,
        "import \"stdlib/text\" as text;\n\
         fn main() -> i32 {\n\
             let n: i32 = 42;\n\
             let s: text::Text = n.to_text();\n\
             let b: text::Text = true.to_text();\n\
             var score: i32 = 0;\n\
             if s.len() == (2 as usize) { score = score +% 1; }\n\
             if s.starts_with(\"42\") { score = score +% 1; }\n\
             if b.starts_with(\"true\") { score = score +% 1; }\n\
             return score;\n\
         }\n",
    );
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc");
    assert!(st.success(), "cpc build of n.to_string() -> Text failed");
    let run = Command::new(dir.join("target/debug/textt"))
        .status()
        .expect("run");
    assert_eq!(run.code(), Some(3), "to_string -> Text checks must pass");
}

/// The full heap surface — Text (lang string), Vec, to_text, interpolation
/// lengths — emits 32-bit-correct IR that esp-clang's verifier accepts and
/// compiles to a Xtensa object. This is the oracle from the development
/// loop, kept as a regression gate. Skips (loudly) without esp-clang.
#[test]
fn target_esp32_text_and_vec_compile_to_xtensa_object() {
    let Some(esp_clang) = esp_clang_for_test() else {
        eprintln!("skipping: esp-clang not installed");
        return;
    };
    let cpc = env!("CARGO_BIN_EXE_cpc");
    let dir = tempdir();
    std::fs::write(
        dir.join("Cplus.toml"),
        "[package]\nname = \"heaptest\"\n\n[[bin]]\nname = \"heaptest\"\npath = \"src/main.cplus\"\n\n[dependencies]\nstdlib = \"*\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    symlink_dir(&root.join("vendor/stdlib"), &dir.join("vendor/stdlib"));
    std::fs::write(
        dir.join("src/main.cplus"),
        "import \"stdlib/text\" as text;\n\
         import \"stdlib/vec\" as vec;\n\
         fn main() -> i32 {\n\
             let t: text::Text = \"esp32 heap\".to_text();\n\
             let n: usize = t.len();\n\
             var v: vec::Vec[i32] = vec::new::[i32]();\n\
             v.push(40);\n\
             v.push(2);\n\
             let a: i32 = vec::at_copy::[i32](v, 0 as usize);\n\
             let b: i32 = vec::at_copy::[i32](v, 1 as usize);\n\
             return (n as i32) + a + b - 52;\n\
         }\n",
    )
    .unwrap();
    let out = Command::new(cpc)
        .arg("--target")
        .arg("esp32-xtensa")
        .arg("--emit-ll-project")
        .current_dir(&dir)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "emit-ll-project for esp32 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ll = dir.join("heap.ll");
    std::fs::write(&ll, &out.stdout).unwrap();
    let obj = dir.join("heap.o");
    let cc = Command::new(&esp_clang)
        .arg("-Wno-override-module")
        .arg("-target")
        .arg("xtensa-esp32-elf")
        .arg("-c")
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("invoke esp-clang");
    assert!(
        cc.status.success(),
        "esp-clang must verify + compile the 32-bit heap IR: {}",
        String::from_utf8_lossy(&cc.stderr)
    );
    assert!(obj.is_file());

    // Behavior check on the host: same program, host target, must run clean
    // (exit 0 — the arithmetic checks Text len and Vec contents).
    let st = Command::new(cpc)
        .arg("build")
        .current_dir(&dir)
        .status()
        .expect("invoke cpc build");
    assert!(st.success(), "host build of the heap program failed");
    let run = Command::new(dir.join("target/debug/heaptest"))
        .status()
        .expect("run heaptest");
    assert_eq!(run.code(), Some(0), "heap program must compute correctly");
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
