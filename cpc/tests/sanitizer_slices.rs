//! End-to-end: `--asan` / `--tsan` must reach PREBUILT DEPENDENCIES, not just
//! the entry package.
//!
//! The defect this pins (bugs/, 2026-08-22): `prebuild_fingerprint` hashed the
//! compiler version, the triple, debug-vs-release and the source digest — but
//! not the sanitizer set. So `cpc build --tsan` asked for a slice, the
//! fingerprint answered "current", and the UNINSTRUMENTED archive from the
//! last ordinary build was linked. The entry package was instrumented and the
//! runtime was live, so a race in the entry package reported correctly and a
//! race inside `stdlib` reported nothing — a silence that reads as clean.
//! `build_lib_project` compounded it by passing a hardcoded `&[]` to codegen
//! and omitting `-fsanitize=` from its `clang -c`.
//!
//! Fourth defect in this family (`cpc build` v0.0.3, the test LINK step, then
//! `closed/cpc-test-asan-does-not-instrument.md` for test codegen), which is
//! why the assertions below are on the ARCHIVE's bytes rather than on a
//! sanitizer report: a report can be absent for many reasons, but an archive
//! either carries the instrumentation or it does not.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// One app + one `prebuild = true` vendor dep it calls into.
fn fixture(project: &Path) {
    write(
        &project.join("vendor/plat/Cplus.toml"),
        "[package]\nname = \"plat\"\nversion = \"0.0.1\"\n\n[build]\nprebuild = true\n",
    );
    // A load and a store through a local: enough shape for ASan's
    // instrumentation pass to emit `__asan_` references.
    write(
        &project.join("vendor/plat/src/eng.cplus"),
        "fn answer() -> i32 {\n    var a: [i32; 4] = [1, 2, 3, 4];\n    var t: i32 = 0;\n    var i: usize = 0 as usize;\n    while i < (4 as usize) { t = t + a[i]; i = i + (1 as usize); }\n    return t;\n}\n",
    );
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nplat = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"plat/eng\" as eng;\n\nfn main() -> i32 {\n    return eng::answer();\n}\n",
    );
}

fn build(project: &Path, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(cpc());
    cmd.current_dir(project).arg("build");
    for a in extra {
        cmd.arg(a);
    }
    // Hermetic from any populated per-user store (~/.cplus).
    cmd.env("CPLUS_HOME", project.join(".no-store"))
        .output()
        .expect("run cpc build")
}

fn ok(out: &std::process::Output, what: &str) {
    assert!(
        out.status.success(),
        "{what}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The one slice this fixture produces. The path is part of the contract —
/// `collect_dep_link_args` names the same file on the link line.
fn slice(project: &Path) -> PathBuf {
    let libdir = project.join("vendor/plat/lib");
    let triple = fs::read_dir(&libdir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", libdir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "include"))
        .expect("a lib/<triple>/ slice directory");
    triple.join("libplat.a")
}

/// Sanitizer instrumentation leaves its runtime calls in the archive's symbol
/// table, so the names appear literally in its bytes. Reading them this way
/// keeps the test free of `nm` and works for Mach-O and ELF alike.
fn mentions(archive: &Path, needle: &str) -> bool {
    let bytes = fs::read(archive)
        .unwrap_or_else(|e| panic!("reading {}: {e}", archive.display()));
    bytes
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn asan_instruments_prebuilt_dependency_slices() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    fixture(project);

    // Control: an ordinary build's slice carries nothing.
    ok(&build(project, &[]), "plain build");
    let a = slice(project);
    assert!(
        !mentions(&a, "__asan_"),
        "control failed: a plain build's slice already mentions __asan_"
    );

    // The bug: this used to reuse the archive above, untouched.
    let out = build(project, &["--asan"]);
    ok(&out, "--asan build");
    assert!(
        mentions(&a, "__asan_"),
        "`cpc build --asan` linked an UNINSTRUMENTED dependency slice — \
         a heap bug inside any vendored package is invisible"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("prebuilding `plat`"),
        "the sanitizer set must invalidate the slice fingerprint"
    );

    // Round-trip: flipping back must restore an uninstrumented slice, not
    // leave the instrumented one stamped current — an app built without
    // `--asan` that links an ASan archive fails to resolve the runtime.
    ok(&build(project, &[]), "plain rebuild after --asan");
    assert!(
        !mentions(&a, "__asan_"),
        "dropping --asan left the instrumented slice in place"
    );

    // And the key must be a KEY, not a permanent miss: a second identical
    // sanitizer build reuses its slice.
    ok(&build(project, &["--asan"]), "--asan rebuild");
    let again = build(project, &["--asan"]);
    ok(&again, "--asan repeat");
    assert!(
        !String::from_utf8_lossy(&again.stderr).contains("prebuilding `plat`"),
        "a repeated --asan build must hit the cache, not rebuild every time"
    );
}

#[test]
fn tsan_instruments_prebuilt_dependency_slices() {
    // TSan is the case the report was written from: the reason a race in
    // `stdlib/reactor.cplus` could be neither confirmed nor ruled out.
    if cfg!(windows) {
        return; // no TSan runtime on MSVC
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    fixture(project);

    ok(&build(project, &[]), "plain build");
    let a = slice(project);
    assert!(!mentions(&a, "__tsan_"), "control: plain slice is clean");

    ok(&build(project, &["--tsan"]), "--tsan build");
    assert!(
        mentions(&a, "__tsan_"),
        "`cpc build --tsan` linked an UNINSTRUMENTED dependency slice — \
         every race inside a vendored package reads as clean"
    );
}

#[test]
fn different_sanitizer_sets_are_different_slices() {
    // `--asan` and `--tsan` are not interchangeable, and the two runtimes are
    // mutually exclusive at link time: reusing one for the other produces an
    // archive that either fails to link or is instrumented for the wrong tool.
    if cfg!(windows) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    fixture(project);

    ok(&build(project, &["--asan"]), "--asan build");
    let a = slice(project);
    assert!(mentions(&a, "__asan_"));

    ok(&build(project, &["--tsan"]), "--tsan build");
    assert!(
        mentions(&a, "__tsan_") && !mentions(&a, "__asan_"),
        "switching sanitizers must rebuild the slice for the new set"
    );
}
