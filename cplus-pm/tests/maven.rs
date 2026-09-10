//! End-to-end: `[android.maven]` coordinates, through the same `cli::run`
//! dispatcher the `cpc pm` subcommand uses.
//!
//! A fixture Maven repo on disk, served over `file://`, exercises the real
//! download path — `curl` fetches, the cache is filled, AARs are exploded
//! with `unzip` — with no network. `--m2` keeps it out of `~/.cplus`.

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Publish a POM into a fixture Maven repo.
fn pom(repo: &Path, group: &str, artifact: &str, version: &str, body: &str) {
    let dir = repo
        .join(group.replace('.', "/"))
        .join(artifact)
        .join(version);
    write(
        &dir.join(format!("{artifact}-{version}.pom")),
        &format!(
            r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <groupId>{group}</groupId><artifactId>{artifact}</artifactId><version>{version}</version>
  {body}
</project>"#
        ),
    );
}

/// Publish an AAR: a real zip holding a manifest fragment, a `classes.jar`,
/// a `res/` and a `jni/` — the four things a build consumes.
fn aar(repo: &Path, group: &str, artifact: &str, version: &str) {
    let dir = repo
        .join(group.replace('.', "/"))
        .join(artifact)
        .join(version);
    let staging = dir.join("staging");
    write(
        &staging.join("AndroidManifest.xml"),
        &format!("<manifest package=\"{group}.{artifact}\"/>\n"),
    );
    write(&staging.join("classes.jar"), "not really a jar, but a file\n");
    write(&staging.join("res/values/values.xml"), "<resources/>\n");
    write(&staging.join("jni/arm64-v8a/lib.so"), "\0elf\n");
    let out = dir.join(format!("{artifact}-{version}.aar"));
    let status = Command::new("zip")
        .arg("-q")
        .arg("-r")
        .arg(&out)
        .arg(".")
        .current_dir(&staging)
        .status()
        .unwrap();
    assert!(status.success(), "zip failed");
    fs::remove_dir_all(&staging).unwrap();
}

/// Publish a plain jar (code only, nothing to explode).
fn jar(repo: &Path, group: &str, artifact: &str, version: &str) {
    let dir = repo
        .join(group.replace('.', "/"))
        .join(artifact)
        .join(version);
    write(
        &dir.join(format!("{artifact}-{version}.jar")),
        "class bytes\n",
    );
}

/// `app` (an AAR) depends on `lib` (a jar) at a version only its BOM knows.
fn fixture_repo(repo: &Path) {
    pom(
        repo,
        "com.k",
        "bom",
        "1.0",
        "<dependencyManagement><dependencies>
           <dependency><groupId>com.k</groupId><artifactId>lib</artifactId><version>2.0</version></dependency>
         </dependencies></dependencyManagement>",
    );
    pom(
        repo,
        "com.x",
        "app",
        "1.0",
        "<packaging>aar</packaging>
         <dependencyManagement><dependencies>
           <dependency><groupId>com.k</groupId><artifactId>bom</artifactId><version>1.0</version><type>pom</type><scope>import</scope></dependency>
         </dependencies></dependencyManagement>
         <dependencies>
           <dependency><groupId>com.k</groupId><artifactId>lib</artifactId></dependency>
           <dependency><groupId>com.x</groupId><artifactId>testonly</artifactId><version>1.0</version><scope>test</scope></dependency>
         </dependencies>",
    );
    aar(repo, "com.x", "app", "1.0");
    pom(repo, "com.k", "lib", "2.0", "");
    jar(repo, "com.k", "lib", "2.0");
}

/// A project pinning `com.x:app`, plus the flags that keep the run offline
/// and out of the real store.
fn project(dir: &Path, extra: &str) {
    write(
        &dir.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n[android.maven]\n\"com.x:app\" = \"1.0\"\n{extra}"
        ),
    );
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn flags(repo: &Path, m2: &Path, store: &Path) -> Vec<String> {
    args(&[
        "--m2",
        &m2.to_string_lossy(),
        "--maven-repo",
        &format!("file://{}", repo.display()),
        "--store",
        &store.to_string_lossy(),
        "--toolchain-repo",
        "github.com/netdur/cplus",
        "--toolchain-version",
        "0.0.27",
    ])
}

#[test]
fn install_materializes_the_closure_and_explodes_the_aars() {
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    project(dir.path(), "");

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    cplus_pm::cli::run(argv).expect("install");

    // The repo layout is real, so `d8` can be pointed at it directly.
    let app = m2.path().join("com/x/app/1.0");
    assert!(app.join("app-1.0.aar").is_file(), "the AAR was downloaded");
    assert!(app.join("app-1.0.pom").is_file(), "the POM was cached");
    // The AAR is exploded beside it: an archive is not a classpath entry.
    assert!(app.join("app-1.0/classes.jar").is_file());
    assert!(app.join("app-1.0/AndroidManifest.xml").is_file());
    assert!(app.join("app-1.0/res/values/values.xml").is_file());
    assert!(app.join("app-1.0/jni/arm64-v8a/lib.so").is_file());
    // The BOM-versioned transitive jar came too — without <scope>import</scope>
    // it would have no version and vanish silently.
    assert!(m2.path().join("com/k/lib/2.0/lib-2.0.jar").is_file());
    // A `test`-scoped dependency is not shipped, and was never fetched.
    assert!(!m2.path().join("com/x/testonly").exists());
}

#[test]
fn install_is_incremental_and_the_queries_are_offline() {
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    project(dir.path(), "");

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    cplus_pm::cli::run(argv).expect("install");

    // Take the remote away. Everything a BUILD asks for must still answer:
    // a `d8` invocation that reaches the network is a build that fails
    // differently on a plane.
    let gone = tempfile::tempdir().unwrap();
    for what in ["list", "classpath", "manifests", "res", "jni"] {
        let mut argv = args(&["maven", what, &dir.path().to_string_lossy()]);
        argv.extend(flags(gone.path(), m2.path(), store.path()));
        cplus_pm::cli::run(argv).unwrap_or_else(|e| panic!("maven {what}: {e}"));
    }
}

#[test]
fn the_build_queries_never_fetch_a_package() {
    // `cpc pm maven classpath` runs inside a build.sh. If it went through
    // `install` it could start a git clone mid-build; instead it walks only
    // the rungs already on disk. An UNRESOLVABLE C+ dependency must
    // therefore not stop it — the Maven side is still answerable.
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    project(dir.path(), "");

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    cplus_pm::cli::run(argv).expect("install");

    // Now declare a git dependency that exists nowhere. `install` would have
    // to fetch it and fail; the query must not care.
    project(
        dir.path(),
        "\n[dependencies]\nnosuch = \"https://github.com/nobody/nothing/tree/main/vendor/nosuch@9.9.9\"\n",
    );
    let mut argv = args(&["maven", "classpath", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    cplus_pm::cli::run(argv).expect("classpath must not need the git dep");

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    assert!(
        cplus_pm::cli::run(argv).is_err(),
        "install, by contrast, does have to fetch it"
    );
}

#[test]
fn an_unreachable_coordinate_installs_nothing() {
    // A missing transitive artifact is a NoClassDefFoundError on a device —
    // the worst place to learn it. Install refuses the whole closure rather
    // than materializing the part that resolved.
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    // `app` also wants something the repo does not publish.
    pom(
        repo.path(),
        "com.x",
        "app",
        "1.0",
        "<dependencies>
           <dependency><groupId>com.x</groupId><artifactId>gone</artifactId><version>9.9</version></dependency>
         </dependencies>",
    );
    project(dir.path(), "");

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    let error = cplus_pm::cli::run(argv).unwrap_err();
    assert!(error.contains("com.x:gone:9.9"), "{error}");
    assert!(error.contains("incomplete"), "{error}");
    assert!(
        !m2.path().join("com/x/app/1.0/app-1.0.aar").is_file(),
        "nothing should be materialized from an incomplete closure"
    );
}

#[test]
fn a_vendored_package_brings_its_own_maven_coordinates() {
    // The real shape: a C+ binding package (a `maps` wrapping play-services)
    // declares the AAR, and a project that never names a coordinate still
    // needs it materialized.
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    write(
        &dir.path().join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[android.dependencies]\nmaps = \"*\"\n",
    );
    // An unstamped `vendor/maps` is rung one of the ladder the build walks,
    // so install reads ITS manifest rather than fetching a release copy.
    write(
        &dir.path().join("vendor/maps/Cplus.toml"),
        "[package]\nname = \"maps\"\nversion = \"0.0.1\"\n\n[android.maven]\n\"com.x:app\" = \"1.0\"\n",
    );

    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(repo.path(), m2.path(), store.path()));
    cplus_pm::cli::run(argv).expect("install");
    assert!(m2.path().join("com/x/app/1.0/app-1.0/classes.jar").is_file());
}

#[test]
fn add_maven_writes_the_pin_and_is_idempotent() {
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    write(
        &dir.path().join("Cplus.toml"),
        "# keep me\n[package]\nname = \"app\"\nversion = \"0.0.1\"\n",
    );

    let run = |coord: &str| {
        let mut argv = args(&["add", &dir.path().to_string_lossy(), "--maven", coord]);
        argv.extend(flags(repo.path(), m2.path(), store.path()));
        cplus_pm::cli::run(argv)
    };
    run("com.x:app:1.0").expect("add");
    let text = fs::read_to_string(dir.path().join("Cplus.toml")).unwrap();
    assert!(text.contains("[android.maven]"), "{text}");
    assert!(text.contains(r#""com.x:app" = "1.0""#), "{text}");
    assert!(text.contains("# keep me"), "comments survive: {text}");
    assert!(m2.path().join("com/x/app/1.0/app-1.0/classes.jar").is_file());

    // Again: no second entry, no rewrite.
    run("com.x:app:1.0").expect("re-add");
    assert_eq!(
        fs::read_to_string(dir.path().join("Cplus.toml")).unwrap(),
        text
    );

    // A coordinate that does not resolve leaves NO line behind — resolution
    // happens before the manifest is touched.
    assert!(run("com.x:nope:1.0").is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Cplus.toml")).unwrap(),
        text
    );
}

#[test]
fn a_malformed_coordinate_is_refused_before_any_fetch() {
    let repo = tempfile::tempdir().unwrap();
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n",
    );
    for bad in ["com.x:app", "../../etc:app:1.0", "com.x:app:.."] {
        let mut argv = args(&["add", &dir.path().to_string_lossy(), "--maven", bad]);
        argv.extend(flags(repo.path(), m2.path(), store.path()));
        let error = cplus_pm::cli::run(argv).unwrap_err();
        assert!(error.contains("coordinate"), "`{bad}`: {error}");
    }
    assert!(!m2.path().join("com").exists(), "nothing was fetched");
}

#[test]
fn maven_outside_android_names_the_fix() {
    let m2 = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[ios.maven]\n\"com.x:app\" = \"1.0\"\n",
    );
    let mut argv = args(&["install", &dir.path().to_string_lossy()]);
    argv.extend(flags(Path::new("/nowhere"), m2.path(), store.path()));
    let error = cplus_pm::cli::run(argv).unwrap_err();
    assert!(error.contains("[android.maven]"), "{error}");
}
