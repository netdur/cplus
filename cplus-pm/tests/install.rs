//! End-to-end: `cpc pm install` against a git monorepo, through the same
//! `cli::run` dispatcher the `cpc pm` subcommand uses.
//!
//! Builds a throwaway repo laid out like the C+ monorepo (packages under
//! `vendor/`, one repo-wide tag), points a consumer project's `Cplus.toml` at
//! it, and installs — verifying the pinned dep and its transitive siblings
//! land in the store tier (the default) or the project's `vendor/`
//! (`--local`). A local `--repo-url` keeps it offline; `--store` keeps it
//! out of the real `~/.cplus`.

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

/// A monorepo where json depends on stdlib; both live under vendor/.
fn monorepo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q"]);
    write(
        &repo.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\nversion = \"0.0.26\"\n",
    );
    write(&repo.join("vendor/stdlib/src/lib/io.cplus"), "// io\n");
    write(
        &repo.join("vendor/json/Cplus.toml"),
        "[package]\nname = \"json\"\nversion = \"0.0.26\"\n\n[dependencies]\nstdlib = \"*\"\n",
    );
    write(&repo.join("vendor/json/src/lib/json.cplus"), "// json\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "release"]);
    git(repo, &["tag", "v0.0.26"]);
}

fn run_install(project: &Path, repo: &Path, store: &Path, extra: &[&str]) -> Result<(), String> {
    let mut args: Vec<String> = vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
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
fn install_populates_the_store_transitively_via_cli() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let store = temp.path().join("store");
    let project = temp.path().join("app");
    monorepo(&repo);

    // A consumer project: bare deps resolved through the toolchain flags.
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\njson = \"*\"\n",
    );

    let result = run_install(&project, &repo, &store, &[]);
    assert!(result.is_ok(), "install failed: {result:?}");

    // Pinned dep + its transitive sibling landed in the store tier, with
    // sources; the project itself grew no vendor/.
    let sv = store.join("v0.0.26/vendor");
    assert!(sv.join("json/Cplus.toml").is_file());
    assert!(sv.join("json/src/lib/json.cplus").is_file());
    assert!(sv.join("stdlib/Cplus.toml").is_file());
    assert!(sv.join("stdlib/src/lib/io.cplus").is_file());
    assert!(!project.join("vendor").exists());
    // .git metadata from the checkout is not copied.
    assert!(!sv.join("json/.git").exists());
    // The clone cache and the tag record live under the store root.
    assert!(store.join("cache").is_dir());
    assert!(store
        .join("tags/github.com_netdur_cplus/v0.0.26")
        .is_file());
}

#[test]
fn install_local_populates_the_project_vendor() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let store = temp.path().join("store");
    let project = temp.path().join("app");
    monorepo(&repo);
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\njson = \"https://github.com/netdur/cplus/tree/main/vendor/json@0.0.26\"\n",
    );

    let result = run_install(&project, &repo, &store, &["--local"]);
    assert!(result.is_ok(), "install failed: {result:?}");

    assert!(project.join("vendor/json/Cplus.toml").is_file());
    assert!(project.join("vendor/stdlib/Cplus.toml").is_file());
    assert!(!store.join("v0.0.26/vendor/json").exists());
}

#[test]
fn install_fetches_platform_scoped_deps_on_every_host() {
    // The store must serve every OS the manifest supports, so install
    // fetches the UNION of `[dependencies]` and every
    // `[<platform>.dependencies]` section — platform filtering is the build
    // driver's job, not the fetcher's. Covers the transitive case too: a
    // platform-scoped package's own platform-scoped deps are walked.
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let store = temp.path().join("store");
    let project = temp.path().join("app");

    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write(
        &repo.join("vendor/stdlib/Cplus.toml"),
        "[package]\nname = \"stdlib\"\nversion = \"0.0.26\"\n",
    );
    write(&repo.join("vendor/stdlib/src/lib/io.cplus"), "// io\n");
    // facet_gtk is linux-only for consumers AND has its own linux-scoped
    // transitive sibling (gtk4).
    write(
        &repo.join("vendor/facet_gtk/Cplus.toml"),
        "[package]\nname = \"facet_gtk\"\nversion = \"0.0.26\"\n\n[linux.dependencies]\ngtk4 = \"*\"\n",
    );
    write(&repo.join("vendor/facet_gtk/src/lib/gtk.cplus"), "// gtk\n");
    write(
        &repo.join("vendor/gtk4/Cplus.toml"),
        "[package]\nname = \"gtk4\"\nversion = \"0.0.26\"\n",
    );
    write(&repo.join("vendor/gtk4/src/lib/gtk4.cplus"), "// gtk4\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "release"]);
    git(&repo, &["tag", "v0.0.26"]);

    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nstdlib = \"*\"\n\n\
         [linux.dependencies]\nfacet_gtk = \"*\"\n",
    );

    let result = run_install(&project, &repo, &store, &[]);
    assert!(result.is_ok(), "install failed: {result:?}");

    // Base dep, linux-scoped dep, AND its linux-scoped transitive all land —
    // on macOS/Windows hosts too.
    let sv = store.join("v0.0.26/vendor");
    assert!(sv.join("stdlib/Cplus.toml").is_file());
    assert!(sv.join("facet_gtk/Cplus.toml").is_file());
    assert!(sv.join("gtk4/Cplus.toml").is_file());
}

#[test]
fn install_reports_a_clear_error_for_an_unpinned_root_dep_without_context() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("app");
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n",
    );

    // No toolchain flags: a bare root dep has nowhere to come from. --local
    // isolates that error from the store-tier requirement.
    let err = cplus_pm::cli::run(vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
        "--local".into(),
        "--store".into(),
        temp.path().join("store").to_string_lossy().into_owned(),
    ])
    .unwrap_err();
    assert!(err.contains("stdlib"), "unexpected error: {err}");
}

#[test]
fn global_install_without_toolchain_version_names_the_fix() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("app");
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26\"\n",
    );

    let err = cplus_pm::cli::run(vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
        "--store".into(),
        temp.path().join("store").to_string_lossy().into_owned(),
    ])
    .unwrap_err();
    assert!(
        err.contains("--toolchain-version") && err.contains("--local"),
        "unexpected error: {err}"
    );
}
