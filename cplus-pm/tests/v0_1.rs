use cplus_pm::manifest::Manifest;
use cplus_pm::resolve::{direct_dependency, ResolveOptions};
use cplus_pm::vendor;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn fetches_one_direct_dependency_from_a_tagged_git_repo() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let root = temp.path().join("root");

    fs::create_dir_all(repo.join("parser")).unwrap();
    fs::create_dir_all(repo.join("types")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(
        repo.join("parser/pkg.toml"),
        r#"
[package]
id = "github.com/sled/tools/parser"
version = "2.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/types" = "1.4.2"
"#,
    )
    .unwrap();
    fs::write(
        repo.join("types/pkg.toml"),
        r#"
[package]
id = "github.com/sled/tools/types"
version = "1.4.2"
language = "c11"
"#,
    )
    .unwrap();
    fs::write(
        repo.join("parser/source.c"),
        "int parser(void) { return 1; }\n",
    )
    .unwrap();
    fs::write(
        repo.join("types/source.c"),
        "int types(void) { return 1; }\n",
    )
    .unwrap();
    git(&repo, &["init"]);
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Cplus PM Test",
            "-c",
            "user.email=cplus-pm@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    );
    git(&repo, &["tag", "parser/v2.1.0"]);
    git(&repo, &["tag", "types/v1.4.2"]);

    fs::write(
        root.join("pkg.toml"),
        r#"
[package]
id = "github.com/app/root"
version = "0.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/parser" = "^2.0"
"#,
    )
    .unwrap();

    fs::write(
        root.join("pkg-exact.toml"),
        r#"
[package]
id = "github.com/app/root"
version = "0.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/parser" = "2.1.0"
"#,
    )
    .unwrap();

    let exact_manifest = Manifest::load(root.join("pkg-exact.toml")).unwrap();
    let dep = direct_dependency(&exact_manifest, "github.com/sled/tools/parser").unwrap();
    let receipt = dep.fetch(&cache, Some(repo.to_str().unwrap())).unwrap();

    assert_eq!(receipt.plan.tag, "parser/v2.1.0");
    assert!(receipt.package_dir.join("pkg.toml").exists());
    assert!(receipt.package_dir.join("source.c").exists());
    assert_eq!(
        receipt.fetched_manifest.package.id.to_string(),
        "github.com/sled/tools/parser"
    );
    assert_eq!(receipt.fetched_manifest.package.version, "2.1.0");

    let output = Command::new(env!("CARGO_BIN_EXE_cplus-pm"))
        .arg("fetch-dep")
        .arg(root.join("pkg-exact.toml"))
        .arg("github.com/sled/tools/parser")
        .arg("--cache")
        .arg(&cache)
        .arg("--repo-url")
        .arg(&repo)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "fetch-dep failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"tag\": \"parser/v2.1.0\""));

    let lock_path = root.join("pkg.lock");
    let output = Command::new(env!("CARGO_BIN_EXE_cplus-pm"))
        .arg("lock")
        .arg(root.join("pkg.toml"))
        .arg(&lock_path)
        .arg("--cache")
        .arg(&cache)
        .arg("--repo-url")
        .arg(&repo)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "lock failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let lock = fs::read_to_string(lock_path).unwrap();
    assert!(lock.contains("github.com/app/root"));
    assert!(lock.contains("github.com/sled/tools/parser"));
    assert!(lock.contains("github.com/sled/tools/types"));
    assert!(lock.contains("sha256:"));
    assert!(lock.contains("github.com/sled/tools/types@1.4.2"));
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn installs_and_removes_a_dependency_in_vendor() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let project = temp.path().join("project");

    // A dependency repo: the `parser` package, tagged parser/v2.1.0.
    fs::create_dir_all(repo.join("parser")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        repo.join("parser/pkg.toml"),
        r#"
[package]
id = "github.com/sled/tools/parser"
version = "2.1.0"
language = "c11"
"#,
    )
    .unwrap();
    fs::write(
        repo.join("parser/parser.cplus"),
        "pub fn parser() -> i32 { return 1; }\n",
    )
    .unwrap();
    git(&repo, &["init"]);
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Cplus PM Test",
            "-c",
            "user.email=cplus-pm@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    );
    git(&repo, &["tag", "parser/v2.1.0"]);

    // A project that depends on it.
    fs::write(
        project.join("pkg.toml"),
        r#"
[package]
id = "github.com/app/root"
version = "0.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/parser" = "2.1.0"
"#,
    )
    .unwrap();

    let options = ResolveOptions::new(&cache).with_repo_url_override(repo.to_str().unwrap());

    // install: resolves + fetches + places into project/vendor/parser/, writes lock.
    let installed = vendor::install(&project, &options).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "parser");
    assert_eq!(installed[0].version, "2.1.0");
    assert!(project.join("vendor/parser/pkg.toml").exists());
    assert!(project.join("vendor/parser/parser.cplus").exists());
    assert!(project.join("pkg.lock").exists());
    // The cached checkout's .git must not be copied into vendor/.
    assert!(!project.join("vendor/parser/.git").exists());

    // remove: deletes the package directory.
    vendor::remove(&project, "parser").unwrap();
    assert!(!project.join("vendor/parser").exists());

    // removing again is an error.
    assert!(vendor::remove(&project, "parser").is_err());
}
