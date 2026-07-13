//! End-to-end: platform-scoped dependencies (`[<platform>.dependencies]`).
//!
//! A dep scoped to another platform must vanish from the build entirely (no
//! presence check, no `[link]` splice, no import resolution); a dep scoped to
//! the ACTIVE platform must behave exactly like a base `[dependencies]` entry.
//! Fixtures are built per-test in a tempdir; every test computes the host's
//! platform name so the suite passes on any OS.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

/// The manifest platform name of the machine the tests run on — must mirror
/// `cplus_core::target::platform_name(HOST.os)`.
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Any platform that is NOT the host — deps scoped here must be skipped.
fn other_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "linux"
    } else {
        "macos"
    }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A minimal dep-free vendor package with one module exporting `answer()`.
fn write_vendor_pkg(project: &Path, name: &str, answer: i32) {
    write(
        &project.join(format!("vendor/{name}/Cplus.toml")),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.0.1\"\n"),
    );
    write(
        &project.join(format!("vendor/{name}/src/util.cplus")),
        &format!("fn answer() -> i32 {{\n    return {answer};\n}}\n"),
    );
}

fn build(project: &Path) -> std::process::Output {
    Command::new(cpc())
        .current_dir(project)
        .arg("build")
        .output()
        .expect("run cpc build")
}

fn run_built(project: &Path) -> i32 {
    let bin = project.join("target/debug/app");
    let status = Command::new(&bin)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", bin.display()));
    status.code().expect("exit code")
}

#[test]
fn deps_scoped_to_another_platform_are_skipped_entirely() {
    // `ghostlib` is declared for the other platform and does NOT exist in
    // vendor/ — pre-platform-deps this failed the presence check (E0854)
    // and spliced foreign link flags. Now the build must succeed.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
             [dependencies]\nherelib = \"*\"\n\n\
             [{}.dependencies]\nghostlib = \"*\"\n",
            other_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "build must skip the off-platform dep:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run_built(project), 7);
}

#[test]
fn deps_scoped_to_the_active_platform_participate() {
    // Scoped to the HOST platform: behaves exactly like a base dep.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 42);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
             [{}.dependencies]\nherelib = \"*\"\n",
            host_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "an active-platform dep must resolve:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run_built(project), 42);
}

#[test]
fn importing_an_off_platform_dep_is_a_targeted_e0866() {
    // The package IS declared (for the other platform) and IS in vendor/ —
    // importing it here must say "not available on platform", not the
    // misleading E0852 "not a declared dependency".
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "gatedlib", 1);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
             [{}.dependencies]\ngatedlib = \"*\"\n",
            other_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"gatedlib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(!out.status.success(), "off-platform import must fail");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("E0866"), "expected E0866, got:\n{all}");
    assert!(
        all.contains("platform") && all.contains("gatedlib"),
        "diagnostic should name the platform gate and the package:\n{all}"
    );
    assert!(
        !all.contains("E0852"),
        "must not report the dep as undeclared:\n{all}"
    );
}

#[test]
fn conflicting_dependency_declarations_fail_with_e0869() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 1);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
             [dependencies]\nherelib = \"*\"\n\n\
             [{}.dependencies]\nherelib = \"*\"\n",
            host_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "fn main() -> i32 {\n    return 0;\n}\n",
    );

    let out = build(project);
    assert!(!out.status.success(), "duplicate declaration must fail");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("E0869"), "expected E0869, got:\n{all}");
}

#[test]
fn platform_suffix_module_shadows_the_base_file() {
    // The `<name>_<platform>.cplus` override is keyed off the ACTIVE
    // platform's name (same vocabulary as the manifest sections) — a
    // `util_macos.cplus` sibling now shadows `util.cplus` on macOS the same
    // way `util_linux.cplus` always did on Linux.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 1); // base answer() -> 1
    write(
        &project.join(format!(
            "vendor/herelib/src/util_{}.cplus",
            host_platform()
        )),
        "fn answer() -> i32 {\n    return 2;\n}\n",
    );
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
         [dependencies]\nherelib = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "build failed:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        run_built(project),
        2,
        "the _{} override should shadow the base module",
        host_platform()
    );
}
