//! End-to-end: the per-user store (D16 in `cplus-pm/docs/decisions.md`).
//!
//! `cpc pm install` populates `$CPLUS_HOME/<tier>/vendor/` — here a temp
//! directory standing in for `~/.cplus` — and `cpc build` resolves imports
//! and links from it. The project itself never grows a `vendor/` directory:
//! that is the store working as designed. The fixture monorepo is tagged
//! `v<toolchain version>` so the bare `dep = "*"` form (D15) resolves
//! through the toolchain context `cpc pm` supplies automatically.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

/// The store tier the running toolchain uses — lockstep with
/// `cplus_pm::store::tier` (exact version pre-1.0, major.minor after).
fn tier() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let major = version.split('.').next().unwrap_or("0");
    if major != "0" {
        let minor = version.split('.').nth(1).unwrap_or("0");
        format!("v{major}.{minor}")
    } else {
        format!("v{version}")
    }
}

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
fn build_resolves_a_store_installed_package() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("cplus");
    let store = temp.path().join("home");
    let project = temp.path().join("app");

    // A monorepo with one package, tagged with the running toolchain's
    // version — what `mathy = "*"` resolves to through the context.
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write(
        &repo.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\nversion = \"0.0.1\"\n",
    );
    write(
        &repo.join("vendor/mathy/src/util.cplus"),
        "fn answer() -> i32 {\n    return 41;\n}\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "release"]);
    git(&repo, &["tag", &format!("v{}", env!("CARGO_PKG_VERSION"))]);

    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nmathy = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"mathy/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    // `cpc pm install` — global by default: the store fills, the project
    // stays clean. `--repo-url` keeps it offline.
    let out = Command::new(cpc())
        .args(["pm", "install", "--repo-url"])
        .arg(&repo)
        .current_dir(&project)
        .env("CPLUS_HOME", &store)
        .output()
        .expect("run cpc pm install");
    assert!(
        out.status.success(),
        "pm install failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let store_pkg = store.join(tier()).join("vendor/mathy");
    assert!(store_pkg.join("Cplus.toml").is_file());
    assert!(store_pkg.join(".cplus-vendor").is_file());
    assert!(!project.join("vendor").exists(), "project must stay clean");

    // `cpc build` resolves the import and links from the store.
    let out = Command::new(cpc())
        .arg("build")
        .current_dir(&project)
        .env("CPLUS_HOME", &store)
        .output()
        .expect("run cpc build");
    assert!(
        out.status.success(),
        "build against the store failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let status = Command::new(project.join("target/debug/app"))
        .status()
        .expect("run built binary");
    assert_eq!(status.code(), Some(41), "the store-resolved module ran");

    // A local vendor copy of the same package must WIN over the store —
    // local-first resolution is what makes dev mode and divergence work.
    write(
        &project.join("vendor/mathy/Cplus.toml"),
        "[package]\nname = \"mathy\"\nversion = \"0.0.1\"\n",
    );
    write(
        &project.join("vendor/mathy/src/util.cplus"),
        "fn answer() -> i32 {\n    return 7;\n}\n",
    );
    let out = Command::new(cpc())
        .arg("build")
        .current_dir(&project)
        .env("CPLUS_HOME", &store)
        .output()
        .expect("run cpc build");
    assert!(
        out.status.success(),
        "build with local override failed:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = Command::new(project.join("target/debug/app"))
        .status()
        .expect("run built binary");
    assert_eq!(status.code(), Some(7), "the project's vendor/ wins");
}
