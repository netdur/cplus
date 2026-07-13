//! End-to-end: `cpc pm install` against a git monorepo, through the same
//! `cli::run` dispatcher the `cpc pm` subcommand uses.
//!
//! Builds a throwaway repo laid out like the C+ monorepo (packages under
//! `vendor/`, one repo-wide tag), points a consumer project's `Cplus.toml` at
//! it, and installs — verifying the pinned dep and its transitive siblings land
//! in the project's `vendor/`. A local `--repo-url` keeps it offline.

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

#[test]
fn install_populates_vendor_transitively_via_cli() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let cache = temp.path().join("cache");
    let project = temp.path().join("app");

    // A monorepo: json depends on stdlib; both live under vendor/.
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
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
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "release"]);
    git(&repo, &["tag", "v0.0.26"]);

    // A consumer project pinning json by tree-URL.
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
         [dependencies]\njson = \"https://github.com/netdur/cplus/tree/main/vendor/json@0.0.26\"\n",
    );

    let result = cplus_pm::cli::run(vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
        "--repo-url".into(),
        repo.to_string_lossy().into_owned(),
        "--cache".into(),
        cache.to_string_lossy().into_owned(),
    ]);
    assert!(result.is_ok(), "install failed: {result:?}");

    // Pinned dep + its transitive sibling both landed, with sources.
    assert!(project.join("vendor/json/Cplus.toml").is_file());
    assert!(project.join("vendor/json/src/lib/json.cplus").is_file());
    assert!(project.join("vendor/stdlib/Cplus.toml").is_file());
    assert!(project.join("vendor/stdlib/src/lib/io.cplus").is_file());
    // .git metadata from the checkout is not copied into vendor/.
    assert!(!project.join("vendor/json/.git").exists());
}

#[test]
fn install_fetches_platform_scoped_deps_on_every_host() {
    // vendor/ is committed and must build on every OS the manifest supports,
    // so install fetches the UNION of `[dependencies]` and every
    // `[<platform>.dependencies]` section — platform filtering is the build
    // driver's job, not the fetcher's. Covers the transitive case too: a
    // platform-scoped package's own platform-scoped deps are walked.
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let cache = temp.path().join("cache");
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
         [[bin]]\nname = \"app\"\npath = \"src/main.cplus\"\n\n\
         [dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26\"\n\n\
         [linux.dependencies]\nfacet_gtk = \"https://github.com/netdur/cplus/tree/main/vendor/facet_gtk@0.0.26\"\n",
    );

    let result = cplus_pm::cli::run(vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
        "--repo-url".into(),
        repo.to_string_lossy().into_owned(),
        "--cache".into(),
        cache.to_string_lossy().into_owned(),
    ]);
    assert!(result.is_ok(), "install failed: {result:?}");

    // Base dep, linux-scoped dep, AND its linux-scoped transitive all land —
    // on macOS/Windows hosts too.
    assert!(project.join("vendor/stdlib/Cplus.toml").is_file());
    assert!(project.join("vendor/facet_gtk/Cplus.toml").is_file());
    assert!(project.join("vendor/gtk4/Cplus.toml").is_file());
}

#[test]
fn install_reports_a_clear_error_for_an_unpinned_root_dep() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("app");
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n",
    );

    let err = cplus_pm::cli::run(vec![
        "install".into(),
        project.to_string_lossy().into_owned(),
        "--cache".into(),
        temp.path().join("cache").to_string_lossy().into_owned(),
    ])
    .unwrap_err();
    assert!(err.contains("stdlib"), "unexpected error: {err}");
}
