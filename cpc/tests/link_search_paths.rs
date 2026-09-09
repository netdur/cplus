//! End-to-end: a package's OWN `[link] search-paths` must reach its own tests.
//!
//! `splice_plain_link_args` has emitted `-L<dir>` (and the matching `-rpath`)
//! for a DEPENDENCY's `[link]` since the table existed. The `cpc test` path had
//! a second, hand-rolled copy of that splice for the package UNDER TEST, and it
//! had drifted: `frameworks`, `libs` and `extra-objects` were spliced and
//! `search-paths` was dropped. So a package whose own library is only findable
//! through a `-L` linked fine for every consumer and could not run one test of
//! its own — `ld: library '...' not found`, naming a file that is right there.
//!
//! The fixture builds its own static library rather than naming a system one,
//! so the check is about the search path and nothing else, on any host.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A one-function static library in `<project>/libs`, which is NOT a directory
/// any linker searches by default — reaching it is the whole point.
fn build_static_lib(project: &Path, name: &str, answer: i32) -> bool {
    let libs = project.join("libs");
    fs::create_dir_all(&libs).unwrap();
    let c = project.join("cpc_test_link.c");
    write(
        &c,
        &format!("int cpc_test_link_answer(void) {{ return {answer}; }}\n"),
    );
    let obj = project.join("cpc_test_link.o");
    let compiled = Command::new("clang")
        .args(["-c", c.to_str().unwrap(), "-o", obj.to_str().unwrap()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !compiled {
        return false;
    }
    Command::new("ar")
        .arg("rcs")
        .arg(libs.join(format!("lib{name}.a")))
        .arg(&obj)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn a_packages_own_search_path_reaches_its_own_tests() {
    let tmp = std::env::temp_dir().join(format!("cpc_link_sp_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // No clang/ar on this machine: the fixture cannot be built, and a check
    // that silently passes without its subject is worse than one that is not
    // run. Skipped loudly rather than asserted vacuously.
    if !build_static_lib(&tmp, "cpctestlink", 42) {
        eprintln!("skipping: could not build the fixture archive (clang/ar)");
        return;
    }

    // `search-paths` is RELATIVE here, which resolves against the manifest dir
    // — so the fixture is location-independent and the test needs no absolute
    // path of its own.
    write(
        &tmp.join("Cplus.toml"),
        "[package]\n\
         name    = \"lsp\"\n\
         version = \"0.0.1\"\n\
         edition = \"2026\"\n\
         \n\
         [link]\n\
         libs         = [\"cpctestlink\"]\n\
         search-paths = [\"libs\"]\n",
    );
    // The extern is CALLED by the check, so this fails if the archive is found
    // and not linked as well as if it is never found at all.
    write(
        &tmp.join("src/main.cplus"),
        "extern fn cpc_test_link_answer() -> i32;\n\
         \n\
         fn main() -> i32 { return 0 as i32; }\n\
         \n\
         #[test]\n\
         fn the_archive_on_the_search_path_is_linked() {\n\
         \x20   assert { cpc_test_link_answer() } == (42 as i32);\n\
         }\n",
    );

    let out = Command::new(cpc())
        .arg("test")
        .current_dir(&tmp)
        .output()
        .expect("run cpc test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`cpc test` failed.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed") || stderr.contains("1 passed"),
        "expected the one check to run.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
