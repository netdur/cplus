//! End-to-end: `cpc pm add` (D17) through the same `cli::run` dispatcher.
//!
//! A fixture monorepo carries a `ui` package with a base dep and two
//! platform-scoped backends; `add` must write ui + its closure into the
//! project manifest with platform sections mapped to the TARGET SET
//! (declared platforms, else the host, extended by --platform), preserve
//! the manifest's comments, stay idempotent, and then install.

use std::fs;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A monorepo: `ui` with one base dep and one backend per desktop platform.
fn monorepo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q"]);
    write(
        &repo.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\nversion = \"0.0.26\"\n",
    );
    write(
        &repo.join("vendor/macish/Cplus.toml"),
        "[package]\nname = \"macish\"\nversion = \"0.0.26\"\n",
    );
    write(
        &repo.join("vendor/winish/Cplus.toml"),
        "[package]\nname = \"winish\"\nversion = \"0.0.26\"\n",
    );
    write(
        &repo.join("vendor/gtkish/Cplus.toml"),
        "[package]\nname = \"gtkish\"\nversion = \"0.0.26\"\n",
    );
    write(
        &repo.join("vendor/ui/Cplus.toml"),
        "[package]\nname = \"ui\"\nversion = \"0.0.26\"\n\n\
         [dependencies]\nstdlib = \"*\"\n\n\
         [macos.dependencies]\nmacish = \"*\"\n\n\
         [windows.dependencies]\nwinish = \"*\"\n\n\
         [linux.dependencies]\ngtkish = \"*\"\n",
    );
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "release"]);
    git(repo, &["tag", "v0.0.26"]);
}

fn run_add(project: &Path, repo: &Path, store: &Path, extra: &[&str]) -> Result<(), String> {
    let mut args: Vec<String> = vec![
        "add".into(),
        project.to_string_lossy().into_owned(),
        "ui".into(),
        "--repo-url".into(),
        repo.to_string_lossy().into_owned(),
        "--store".into(),
        store.to_string_lossy().into_owned(),
        "--toolchain-repo".into(),
        "github.com/netdur/cplus".into(),
        "--toolchain-version".into(),
        "0.0.26".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    cplus_pm::cli::run(args)
}

#[test]
fn add_writes_the_closure_for_the_host_and_installs() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, store, project) = (
        temp.path().join("cplus"),
        temp.path().join("store"),
        temp.path().join("app"),
    );
    monorepo(&repo);
    write(
        &project.join("Cplus.toml"),
        "# my app\n[package]\nname    = \"app\"   # the name\nversion = \"0.0.1\"\nedition = \"2026\"\n",
    );

    run_add(&project, &repo, &store, &[]).expect("add failed");

    let manifest = fs::read_to_string(project.join("Cplus.toml")).unwrap();
    // Comments survive the surgical edit.
    assert!(manifest.contains("# my app"), "{manifest}");
    assert!(manifest.contains("# the name"), "{manifest}");
    // The package and its base closure land in [dependencies].
    assert!(manifest.contains("ui = \"*\""), "{manifest}");
    assert!(manifest.contains("stdlib = \"*\""), "{manifest}");
    // Exactly the HOST platform's section is written — an undeclared
    // project targets the machine it stands on, nothing else.
    let host = cplus_pm::add::host_platform();
    assert!(
        manifest.contains(&format!("[{host}.dependencies]")),
        "{manifest}"
    );
    for other in ["macos", "windows", "linux"] {
        if other != host {
            assert!(
                !manifest.contains(&format!("[{other}.dependencies]")),
                "foreign platform section written: {manifest}"
            );
        }
    }
    // And the install ran: the store tier has the package.
    assert!(store
        .join("v0.0.26/vendor/ui/Cplus.toml")
        .is_file());
    assert!(store
        .join("v0.0.26/vendor/stdlib/Cplus.toml")
        .is_file());
}

#[test]
fn add_is_idempotent_and_platform_flag_extends() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, store, project) = (
        temp.path().join("cplus"),
        temp.path().join("store"),
        temp.path().join("app"),
    );
    monorepo(&repo);
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n",
    );

    run_add(&project, &repo, &store, &[]).expect("first add");
    // Extend to linux next month: only the missing section is filled.
    run_add(&project, &repo, &store, &["--platform", "linux"]).expect("second add");
    run_add(&project, &repo, &store, &["--platform", "linux"]).expect("third add");

    let manifest = fs::read_to_string(project.join("Cplus.toml")).unwrap();
    assert!(manifest.contains("[linux.dependencies]"), "{manifest}");
    assert!(manifest.contains("gtkish = \"*\""), "{manifest}");
    // Idempotent: one line per dep, however many times add ran.
    assert_eq!(manifest.matches("ui = ").count(), 1, "{manifest}");
    assert_eq!(manifest.matches("gtkish = ").count(), 1, "{manifest}");
    assert_eq!(manifest.matches("stdlib = ").count(), 1, "{manifest}");
}

#[test]
fn a_declared_platform_wins_over_the_host() {
    // D17 rule 1: an iOS-only project on a desktop host gets ui's iOS
    // closure (none here) and NO desktop section at all — packages follow
    // the target, never the host.
    let temp = tempfile::tempdir().unwrap();
    let (repo, store, project) = (
        temp.path().join("cplus"),
        temp.path().join("store"),
        temp.path().join("app"),
    );
    monorepo(&repo);
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n[ios]\nentry = \"src/main.cplus\"\n",
    );

    run_add(&project, &repo, &store, &[]).expect("add failed");
    let manifest = fs::read_to_string(project.join("Cplus.toml")).unwrap();
    assert!(manifest.contains("ui = \"*\""), "{manifest}");
    for desktop in ["macos", "windows", "linux"] {
        assert!(
            !manifest.contains(&format!("[{desktop}.dependencies]")),
            "host leaked into a platform-scoped project: {manifest}"
        );
    }
}

#[test]
fn a_third_party_add_pins_its_siblings_by_url() {
    // The added package's bare siblings must become pinned URLs at ITS
    // repo — writing `*` into the project would re-resolve them against
    // the toolchain repo, a different package entirely.
    let temp = tempfile::tempdir().unwrap();
    let (repo, store, project) = (
        temp.path().join("acme"),
        temp.path().join("store"),
        temp.path().join("app"),
    );
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write(
        &repo.join("parser/Cplus.toml"),
        "[package]\nname = \"parser\"\nversion = \"1.0.0\"\n\n[dependencies]\nlex = \"*\"\n",
    );
    write(
        &repo.join("lex/Cplus.toml"),
        "[package]\nname = \"lex\"\nversion = \"1.0.0\"\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "release"]);
    git(&repo, &["tag", "v1.0.0"]);
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n",
    );

    let result = cplus_pm::cli::run(vec![
        "add".into(),
        project.to_string_lossy().into_owned(),
        "parser".into(),
        "https://github.com/acme/tools/tree/main/parser@1.0.0".into(),
        "--repo-url".into(),
        repo.to_string_lossy().into_owned(),
        "--store".into(),
        store.to_string_lossy().into_owned(),
        "--toolchain-repo".into(),
        "github.com/netdur/cplus".into(),
        "--toolchain-version".into(),
        "0.0.26".into(),
    ]);
    assert!(result.is_ok(), "add failed: {result:?}");

    let manifest = fs::read_to_string(project.join("Cplus.toml")).unwrap();
    assert!(
        manifest.contains("parser = \"https://github.com/acme/tools/tree/main/parser@1.0.0\""),
        "{manifest}"
    );
    assert!(
        manifest.contains("lex = \"https://github.com/acme/tools/tree/main/lex@1.0.0\""),
        "sibling must be pinned at the parent's repo: {manifest}"
    );
    assert!(store.join("v0.0.26/vendor/parser/Cplus.toml").is_file());
    assert!(store.join("v0.0.26/vendor/lex/Cplus.toml").is_file());
}
