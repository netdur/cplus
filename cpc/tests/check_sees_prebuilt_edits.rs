//! End-to-end: `cpc check` must see an edit to a PREBUILT dependency's source.
//!
//! The defect this pins (bugs/prebuilt-slice-hides-a-source-edit-from-
//! consumers.md): a consumer type-checks against the dependency's generated
//! `lib/include/`, and only a build regenerated it — `ensure_one_slice` does
//! headers and archive together, and `cpc check` never reaches it. So after an
//! item was added to the dependency, `cpc check` in the consumer said "no item
//! named `X`" while every older item in the same module resolved, and nothing
//! in the message pointed at a cache.

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

fn run(project: &Path, verb: &str) -> std::process::Output {
    Command::new(cpc())
        .current_dir(project)
        .arg(verb)
        // Hermetic from any populated per-user store (~/.cplus).
        .env("CPLUS_HOME", project.join(".no-store"))
        .output()
        .expect("run cpc")
}

fn ok(out: &std::process::Output, what: &str) {
    assert!(
        out.status.success(),
        "{what}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_sees_an_item_added_to_a_prebuilt_dependency() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        &project.join("vendor/plat/Cplus.toml"),
        "[package]\nname = \"plat\"\nversion = \"0.0.1\"\n\n[build]\nprebuild = true\n",
    );
    let eng = project.join("vendor/plat/src/eng.cplus");
    write(&eng, "fn answer() -> i32 {\n    return 4;\n}\n");
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nplat = \"*\"\n",
    );
    let main = project.join("src/main.cplus");
    write(
        &main,
        "import \"plat/eng\" as eng;\n\nfn main() -> i32 {\n    return eng::answer();\n}\n",
    );

    // A build leaves a slice and its headers behind: the state the bug needs.
    ok(&run(project, "build"), "first build");
    assert!(
        project.join("vendor/plat/lib/include/eng.cplus").is_file(),
        "fixture: the build should have generated plat's headers"
    );

    // Edit the dependency, use the new item, and only CHECK.
    write(
        &eng,
        "fn answer() -> i32 {\n    return 4;\n}\n\nfn doubled() -> i32 {\n    return 8;\n}\n",
    );
    write(
        &main,
        "import \"plat/eng\" as eng;\n\nfn main() -> i32 {\n    return eng::answer() + eng::doubled();\n}\n",
    );
    ok(
        &run(project, "check"),
        "`cpc check` read plat's headers from before the edit",
    );

    // And the refresh tracks removal too: the item is gone again, so is the
    // name — the headers follow the source, not merely grow.
    write(&eng, "fn answer() -> i32 {\n    return 4;\n}\n");
    let out = run(project, "check");
    assert!(
        !out.status.success(),
        "`cpc check` accepted a call to an item the dependency no longer has"
    );
}
