//! End-to-end: `--timings` — the phase table and the per-package roll-up.
//!
//! The phase table answers "where does a compile go"; the package table
//! answers "which package am I paying for". Fixtures are built per-test in a
//! tempdir, so every row here is produced by a real prebuild.

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

/// A minimal vendor package with one module exporting `answer()`. `build` is
/// spliced into its manifest so a caller can turn prebuild off.
fn write_vendor_pkg(vendor_root: &Path, name: &str, answer: i32, build: &str) {
    write(
        &vendor_root.join(format!("vendor/{name}/Cplus.toml")),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.0.1\"\n{build}"),
    );
    write(
        &vendor_root.join(format!("vendor/{name}/src/util.cplus")),
        &format!("fn answer() -> i32 {{\n    return {answer};\n}}\n"),
    );
}

/// An app that returns `herelib::answer()`.
fn write_app(project: &Path) {
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nherelib = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );
}

fn build(project: &Path, args: &[&str]) -> String {
    let out = Command::new(cpc())
        .current_dir(project)
        .arg("build")
        .args(args)
        // Hermetic from any populated per-user store (~/.cplus).
        .env("CPLUS_HOME", project.join(".no-store"))
        .output()
        .expect("run cpc build");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "build failed:\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    stderr
}

#[test]
fn the_phase_table_names_every_phase_and_a_total() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7, "");
    write_app(project);

    let err = build(project, &["--timings"]);
    for row in [
        "cpc timings:",
        "resolve+sema+borrowck",
        "codegen",
        "prune",
        "clang + link",
        "measured total",
    ] {
        assert!(err.contains(row), "no `{row}` row in:\n{err}");
    }
}

#[test]
fn a_prebuilt_dependency_gets_a_row_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7, "");
    write_app(project);

    let err = build(project, &["--timings"]);
    let table = err
        .split_once("cpc timings by package:")
        .unwrap_or_else(|| panic!("no package table in:\n{err}"))
        .1;
    // Compiled now, so it is `prebuilt` — the dependency's own cost, not the
    // project's, and the project keeps a row of its own beside it.
    assert!(
        table.lines().any(|l| l.contains("herelib") && l.contains("prebuilt")),
        "herelib is not reported as prebuilt:\n{table}"
    );
    assert!(
        table.lines().any(|l| l.contains("app") && l.contains("this project")),
        "no project row:\n{table}"
    );
    assert!(table.contains("wall clock"), "no wall clock row:\n{table}");
}

#[test]
fn a_second_build_reports_the_dependency_as_up_to_date() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7, "");
    write_app(project);

    build(project, &["--timings"]);
    // Nothing changed: the fingerprint matches and the slice is reused. The
    // row must still appear — a warm build with no dependency rows at all
    // would read as a build that has no dependencies.
    let err = build(project, &["--timings"]);
    let table = err.split_once("cpc timings by package:").expect("package table").1;
    assert!(
        table.lines().any(|l| l.contains("herelib") && l.contains("up to date")),
        "a reused slice is not reported as up to date:\n{table}"
    );
}

#[test]
fn a_dependency_that_opts_out_of_prebuild_is_reported_as_inlined() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7, "\n[build]\nprebuild = false\n");
    write_app(project);

    let err = build(project, &["--timings"]);
    let table = err.split_once("cpc timings by package:").expect("package table").1;
    // It is compiled from source inside the project's own compile, so it has
    // no separable cost — but it must still be named, with what CAN be said
    // about it: how many of its modules this build swallowed.
    assert!(
        table
            .lines()
            .any(|l| l.contains("herelib") && l.contains("compiled inside `app`") && l.contains("1 modules")),
        "herelib is not reported as inlined:\n{table}"
    );
    assert!(
        !table.lines().any(|l| l.contains("herelib") && l.contains("prebuilt")),
        "a package that opted out of prebuild must not claim a prebuild row:\n{table}"
    );
}

#[test]
fn a_package_under_vendor_is_not_inlined_into_itself() {
    // A vendor package built from inside itself (`cd vendor/events && cpc
    // test`) sits at `.../vendor/<name>/src/` exactly like a dependency
    // would. Matching on the path alone reported it as inlined into itself,
    // one row above its own project row.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("vendor/mypkg");
    write_vendor_pkg(&project, "herelib", 7, "\n[build]\nprebuild = false\n");
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"mypkg\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nherelib = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let err = build(&project, &["--timings"]);
    let table = err.split_once("cpc timings by package:").expect("package table").1;
    assert!(
        !table
            .lines()
            .any(|l| l.trim_start().starts_with("mypkg") && l.contains("compiled inside")),
        "mypkg is reported as a dependency of itself:\n{table}"
    );
    assert!(
        table.lines().any(|l| l.contains("mypkg") && l.contains("this project")),
        "no project row:\n{table}"
    );
    assert!(
        table.lines().any(|l| l.contains("herelib") && l.contains("compiled inside")),
        "the real inlined dependency went missing:\n{table}"
    );
}

#[test]
fn without_the_flag_nothing_is_measured() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7, "");
    write_app(project);

    let err = build(project, &[]);
    assert!(
        !err.contains("cpc timings"),
        "timing output leaked into a build that did not ask for it:\n{err}"
    );
}
