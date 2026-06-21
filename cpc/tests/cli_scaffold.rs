//! Tests for the project-DX subcommands: `cpc skill` and `cpc init`.

use std::path::Path;
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

// ---- cpc skill ----

#[test]
fn skill_prints_the_reference() {
    let out = Command::new(cpc()).arg("skill").output().expect("run cpc skill");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SKILL — writing C+ source"), "unexpected skill output");
    assert!(s.len() > 1000, "skill reference seems too short");
}

#[test]
fn skill_write_creates_file_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("SKILL.md");

    let w = Command::new(cpc())
        .arg("skill").arg("--write").arg(&dest)
        .output().expect("run");
    assert!(w.status.success());
    assert!(dest.exists());
    let body = std::fs::read_to_string(&dest).unwrap();
    assert!(body.contains("SKILL — writing C+ source"));

    // Second write without --force must fail (no clobber).
    let again = Command::new(cpc())
        .arg("skill").arg("--write").arg(&dest)
        .output().expect("run");
    assert!(!again.status.success(), "overwrite without --force must fail");

    // --force overwrites.
    let forced = Command::new(cpc())
        .arg("skill").arg("--write").arg(&dest).arg("--force")
        .output().expect("run");
    assert!(forced.status.success());
}

// ---- cpc init ----

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|_| panic!("missing {}", p.display()))
}

#[test]
fn init_scaffolds_a_named_project() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .arg("init").arg("myapp")
        .output().expect("run cpc init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));

    let proj = dir.path().join("myapp");
    let manifest = read(&proj.join("Cplus.toml"));
    assert!(manifest.contains("name    = \"myapp\""), "manifest: {manifest}");
    assert!(manifest.contains("[[bin]]"));
    assert!(manifest.contains("path = \"src/main.cplus\""));
    // stdlib is the Go-style path@version source, pinned to this toolchain.
    assert!(manifest.contains("vendor/stdlib@"), "stdlib should use path@version: {manifest}");
    assert!(
        manifest.contains(&format!("@{}\"", env!("CARGO_PKG_VERSION"))),
        "stdlib should be pinned to the cpc version: {manifest}"
    );

    let main = read(&proj.join("src/main.cplus"));
    assert!(main.contains("fn main() -> i32"));
    assert!(main.contains("io::println"));

    assert!(proj.join(".gitignore").exists());
    // The fresh project ships the agent reference.
    assert!(read(&proj.join("SKILL.md")).contains("SKILL — writing C+ source"));
}

#[test]
fn init_refuses_existing_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cplus.toml"), "[package]\nname=\"x\"\nversion=\"0.0.1\"\n").unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .arg("init")
        .output().expect("run");
    assert!(!out.status.success(), "init must refuse to clobber an existing Cplus.toml");
    assert!(String::from_utf8_lossy(&out.stderr).contains("already exists"));
}

#[test]
fn init_accepts_a_path_and_names_from_the_leaf() {
    // `cpc init a/b` is a path (cargo-like): scaffold into a/b/, name = `b`.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .arg("init").arg("nested/proj")
        .output().expect("run");
    assert!(out.status.success(), "a path arg should be accepted: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = read(&dir.path().join("nested/proj/Cplus.toml"));
    assert!(manifest.contains("name    = \"proj\""), "package name should be the leaf: {manifest}");
}

#[test]
fn init_dot_scaffolds_in_place() {
    // `cpc init .` scaffolds the current directory; name = the directory's name.
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("myproj");
    std::fs::create_dir(&proj).unwrap();
    let out = Command::new(cpc())
        .current_dir(&proj)
        .arg("init").arg(".")
        .output().expect("run cpc init .");
    assert!(out.status.success(), "init . failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(read(&proj.join("Cplus.toml")).contains("name    = \"myproj\""));
    // No `cd .` noise in the in-place case.
    assert!(!String::from_utf8_lossy(&out.stdout).contains("cd ."), "should not suggest `cd .`");
}

#[test]
fn init_absolute_path_creates_nested() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a/b/app"); // absolute, nested, doesn't exist yet
    let out = Command::new(cpc())
        .arg("init").arg(&target)
        .output().expect("run cpc init <abs>");
    assert!(out.status.success(), "absolute path init failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(read(&target.join("Cplus.toml")).contains("name    = \"app\""), "name = leaf");
    assert!(target.join("src/main.cplus").exists());
}

#[test]
fn init_rejects_invalid_leaf_name() {
    // An invalid character in the *leaf* (not a path separator) is rejected.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .arg("init").arg("bad name!")
        .output().expect("run");
    assert!(!out.status.success(), "an invalid leaf name must be rejected");
}

// ---- cpc pm (unified package manager) ----

#[test]
fn pm_help_is_the_package_manager() {
    let out = Command::new(cpc()).arg("pm").arg("--help").output().expect("run cpc pm --help");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("manage C+ packages"), "got: {s}");
    assert!(s.contains("install"), "usage should list `install`");
}

#[test]
fn pm_tag_routes_to_package_manager() {
    // `cpc pm ...` dispatches to the same `cplus_pm::cli::run` that backs the
    // standalone `cplus-pm` binary. `tag` is pure (no fs/network): a valid
    // package id must produce a tag referencing the version.
    let id = "github.com/netdur/cplus";
    let out = Command::new(cpc()).args(["pm", "tag", id, "1.2.3"]).output().expect("cpc pm tag");
    assert!(
        out.status.success(),
        "tag of a valid id should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.2.3"), "tag should reference the version");
}

#[test]
fn pm_unknown_command_fails() {
    let out = Command::new(cpc()).args(["pm", "definitely-not-a-command"]).output().expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
}

#[test]
fn init_manifest_parses_and_builds_front_end() {
    // The scaffolded main.cplus imports stdlib, so a full build needs deps;
    // but we can at least confirm the generated Cplus.toml is well-formed by
    // having `cpc build` get past manifest parsing (it will then fail on the
    // missing vendored stdlib, not on a malformed manifest).
    let dir = tempfile::tempdir().unwrap();
    assert!(Command::new(cpc())
        .current_dir(dir.path())
        .arg("init").arg("p")
        .status().unwrap().success());

    let out = Command::new(cpc())
        .current_dir(dir.path().join("p"))
        .arg("build")
        .output().expect("run cpc build");
    let err = String::from_utf8_lossy(&out.stderr);
    // Must NOT be a manifest/TOML parse error — any failure should be about the
    // missing dependency, proving the generated manifest is valid.
    assert!(
        !err.to_lowercase().contains("toml") && !err.contains("Cplus.toml: parse"),
        "generated manifest failed to parse: {err}"
    );
}
