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

// ---- cpc skill: per-package skills ----
//
// The language reference cannot teach a package's correct use, and package
// misuse COMPILES — so a dependency may ship its own `SKILL.md` and `cpc skill`
// appends it. These pin the three behaviours: it is picked up, `--lang-only`
// suppresses it, and a dep without one changes nothing.

/// Build a throwaway project whose `vendor/<name>/` holds a package with the
/// given `SKILL.md` body (or none, when `skill` is `None`).
fn project_with_dep(dir: &Path, dep: &str, skill: Option<&str>) {
    std::fs::write(
        dir.join("Cplus.toml"),
        format!(
            "[package]\nname = \"host\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n[dependencies]\n{dep} = \"*\"\n"
        ),
    )
    .unwrap();
    let vd = dir.join("vendor").join(dep);
    std::fs::create_dir_all(vd.join("src")).unwrap();
    std::fs::write(
        vd.join("Cplus.toml"),
        format!("[package]\nname = \"{dep}\"\nversion = \"0.0.1\"\nedition = \"2026\"\n"),
    )
    .unwrap();
    if let Some(body) = skill {
        std::fs::write(vd.join("SKILL.md"), body).unwrap();
    }
}

#[test]
fn skill_appends_a_dependencys_own_skill() {
    let dir = tempfile::tempdir().unwrap();
    project_with_dep(dir.path(), "widgets", Some("# WIDGETS SKILL\n\nrule one.\n"));

    let out = Command::new(cpc())
        .arg("skill")
        .current_dir(dir.path())
        .output()
        .expect("run cpc skill");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SKILL — writing C+ source"), "language reference must still lead");
    assert!(s.contains("# WIDGETS SKILL"), "dependency skill must be appended:\n{s}");
    assert!(
        s.contains("package skill: widgets"),
        "the appended skill must say where it came from"
    );
}

#[test]
fn skill_lang_only_suppresses_package_skills() {
    let dir = tempfile::tempdir().unwrap();
    project_with_dep(dir.path(), "widgets", Some("# WIDGETS SKILL\n"));

    let out = Command::new(cpc())
        .arg("skill")
        .arg("--lang-only")
        .current_dir(dir.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SKILL — writing C+ source"));
    assert!(!s.contains("# WIDGETS SKILL"), "--lang-only must print the language reference alone");
}

#[test]
fn skill_is_unchanged_when_no_dependency_ships_one() {
    let dir = tempfile::tempdir().unwrap();
    project_with_dep(dir.path(), "widgets", None);

    let with = Command::new(cpc()).arg("skill").current_dir(dir.path()).output().unwrap();
    let bare = Command::new(cpc()).arg("skill").arg("--lang-only").current_dir(dir.path()).output().unwrap();
    assert!(with.status.success() && bare.status.success());
    assert_eq!(
        String::from_utf8_lossy(&with.stdout),
        String::from_utf8_lossy(&bare.stdout),
        "a dep with no SKILL.md must add nothing"
    );
}

#[test]
fn skill_write_reports_which_package_skills_it_bundled() {
    let dir = tempfile::tempdir().unwrap();
    project_with_dep(dir.path(), "widgets", Some("# WIDGETS SKILL\n"));
    let dest = dir.path().join("OUT.md");

    let out = Command::new(cpc())
        .arg("skill").arg("--write").arg(&dest)
        .current_dir(dir.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("widgets"), "should name the bundled skill:\n{said}");
    let body = std::fs::read_to_string(&dest).unwrap();
    assert!(body.contains("# WIDGETS SKILL"), "the written file must carry it too");
}

// ---- cpc init: the iOS scaffold ----
//
// iOS has no console and no window a `fn main` could open, so `--platform ios`
// scaffolds a facet APP (a screen + the runtime) and the Xcode-side shell,
// rather than the hello-world every self-linked platform gets. These pin the
// shape; `cpc build --target ios-*` proving it LINKS is a separate, heavier
// check that needs the facet packages in the store.

#[test]
fn init_ios_scaffolds_the_app_and_the_xcode_shell() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .arg("init").arg("--platform").arg("ios").arg("myapp")
        .current_dir(dir.path())
        .output()
        .expect("run cpc init");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let root = dir.path().join("myapp");

    // The app, the entry, and the Xcode side.
    for f in ["src/app.cplus", "src/main.cplus", "ios/main.m", "ios/Info.plist"] {
        assert!(root.join(f).is_file(), "missing {f}");
    }

    // The entry is the external-builder shape, and main.m calls exactly it.
    let entry = std::fs::read_to_string(root.join("src/main.cplus")).unwrap();
    assert!(entry.contains("export extern fn myapp_main()"), "entry:\n{entry}");
    assert!(entry.contains("app::run()"), "the entry must go through the shared app");
    let m = std::fs::read_to_string(root.join("ios/main.m")).unwrap();
    assert!(m.contains("return myapp_main();"), "main.m must call the exported symbol:\n{m}");
    assert!(m.contains("#import \"myapp.h\""), "main.m must include the generated header:\n{m}");

    // facet_uikit synthesizes its own delegate, so the plist must name neither
    // a storyboard nor a scene manifest — either one takes the window away.
    let plist = std::fs::read_to_string(root.join("ios/Info.plist")).unwrap();
    assert!(!plist.contains("UIMainStoryboardFile"), "plist must not name a storyboard");
    assert!(!plist.contains("UIApplicationSceneManifest"), "plist must not name a scene manifest");
    assert!(plist.contains("LSRequiresIPhoneOS"), "plist:\n{plist}");

    // The manifest carries the backend closure — the resolver checks every
    // import against this one flat set, and `webkit` is not optional.
    let toml = std::fs::read_to_string(root.join("Cplus.toml")).unwrap();
    assert!(toml.contains("[ios.dependencies]"), "toml:\n{toml}");
    for dep in ["facet", "facet_runtime", "facet_uikit", "uikit", "objc", "quartzcore", "webkit"] {
        assert!(toml.contains(dep), "manifest must declare `{dep}`:\n{toml}");
    }

    // The app is a facet component, written the way the facet skill says.
    let app = std::fs::read_to_string(root.join("src/app.cplus")).unwrap();
    assert!(app.contains("component::Component"), "app must implement Component");
    assert!(app.contains("on_click: this.on_tap"), "handlers bind as METHODS:\n{app}");
    assert!(!app.contains("#addr_of(this) as *u8"), "a scaffold must not hand-roll the ctx slot");
}

#[test]
fn init_ios_alongside_a_desktop_platform_yields_main_cplus() {
    // The iOS entry must NOT claim `src/main.cplus` when a self-linked platform
    // is also named: building for that platform would then report the iOS entry
    // as unreachable (W0005), and a fresh scaffold must not warn on first build.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .arg("init").arg("--platform").arg("ios").arg("--platform").arg("macos").arg("both")
        .current_dir(dir.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let root = dir.path().join("both");

    let toml = std::fs::read_to_string(root.join("Cplus.toml")).unwrap();
    assert!(toml.contains("entry = \"src/main_ios.cplus\""), "ios yields main.cplus:\n{toml}");
    assert!(toml.contains("entry = \"src/main.cplus\""), "macos takes it:\n{toml}");
    assert!(root.join("src/main_ios.cplus").is_file());
    assert!(root.join("src/main.cplus").is_file());
    assert!(toml.contains("[macos.dependencies]"), "the desktop backend closure too:\n{toml}");

    // One app, two doors.
    let desktop = std::fs::read_to_string(root.join("src/main.cplus")).unwrap();
    assert!(desktop.contains("fn main() -> i32"), "desktop entry is a real main:\n{desktop}");
    assert!(desktop.contains("app::run()"), "and shares the app");
    assert_eq!(
        root.join("src/app.cplus").is_file(),
        true,
        "the app is shared, not duplicated per platform"
    );
}

#[test]
fn init_ios_alone_keeps_main_cplus_and_no_desktop_entry() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .arg("init").arg("--platform").arg("ios").arg("solo")
        .current_dir(dir.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let root = dir.path().join("solo");
    let toml = std::fs::read_to_string(root.join("Cplus.toml")).unwrap();
    assert!(toml.contains("entry = \"src/main.cplus\""), "nothing to yield to:\n{toml}");
    assert!(!toml.contains("[macos.dependencies]"), "no desktop backend when none was asked for");
    assert!(!root.join("src/main_ios.cplus").exists());
}

#[test]
fn init_without_ios_stays_a_hello_world() {
    // The macOS/host scaffold is deliberately unchanged: no facet, no ios/.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .arg("init").arg("--platform").arg("macos").arg("desk")
        .current_dir(dir.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let root = dir.path().join("desk");
    assert!(!root.join("ios").exists(), "no ios/ for a desktop-only project");
    assert!(!root.join("src/app.cplus").exists(), "no facet app either");
    let main = std::fs::read_to_string(root.join("src/main.cplus")).unwrap();
    assert!(main.contains("hello from C+"), "main:\n{main}");
    let toml = std::fs::read_to_string(root.join("Cplus.toml")).unwrap();
    assert!(!toml.contains("facet"), "a hello-world must not pull in facet:\n{toml}");
}

// ---- cpc explain ----

#[test]
fn explain_prints_cause_fix_and_example_for_a_known_code() {
    let out = Command::new(cpc())
        .arg("explain").arg("E0502")
        .output().expect("run cpc explain");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("E0502"), "should name the code:\n{s}");
    assert!(s.contains("Cause") && s.contains("Fix"), "should show cause + fix:\n{s}");
    // The .md web-docs convention is surfaced so an agent can go deeper.
    assert!(s.contains(".md"), "should point at the markdown docs:\n{s}");
}

#[test]
fn explain_normalizes_and_is_case_insensitive() {
    // `e502` must resolve to the canonical `E0502`.
    let out = Command::new(cpc())
        .arg("explain").arg("e502")
        .output().expect("run");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("E0502"));
}

#[test]
fn explain_unknown_code_fails_cleanly() {
    let out = Command::new(cpc())
        .arg("explain").arg("E9999")
        .output().expect("run");
    assert!(!out.status.success(), "unknown code must exit non-zero");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E9999") && err.contains("--list"),
        "should name the code and point to --list:\n{err}");
}

#[test]
fn explain_list_enumerates_every_code() {
    let out = Command::new(cpc())
        .arg("explain").arg("--list")
        .output().expect("run");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    // First code present, and a plausible catalog size (>100 codes).
    assert!(s.contains("E0001"), "list should include E0001:\n{}", &s[..s.len().min(400)]);
    assert!(s.matches("  E").count() > 100, "catalog should list >100 codes");
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
    // v0.0.28: no target section — src/main.cplus is the default entry.
    assert!(!manifest.contains("[[bin]]"), "legacy [[bin]] must not be scaffolded: {manifest}");
    // D15: a bare `*` is the toolchain's own package at the toolchain's
    // version — `cpc pm` supplies the context, so no URL is scaffolded.
    assert!(
        manifest.contains("stdlib = \"*\""),
        "stdlib should be the bare toolchain dep: {manifest}"
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
fn pm_manifest_routes_to_package_manager() {
    // `cpc pm ...` dispatches to the same `cplus_pm::cli::run` that backs the
    // standalone `cplus-pm` binary. `manifest` is the offline command (reads a
    // Cplus.toml, prints JSON — no network): a valid manifest must round-trip
    // its name and version. (Replaces the retired `pm tag` probe: the rebuilt
    // pm takes versions from git tags on the dependency repo, so there is no
    // tag subcommand anymore.)
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cplus.toml"),
        "[package]\nname = \"probe\"\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    let out = Command::new(cpc())
        .args(["pm", "manifest"])
        .arg(dir.path())
        .output()
        .expect("cpc pm manifest");
    assert!(
        out.status.success(),
        "manifest of a valid project should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"probe\""), "manifest JSON should carry the name: {s}");
    assert!(s.contains("\"1.2.3\""), "manifest JSON should carry the version: {s}");
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

#[test]
fn init_platform_scopes_the_app() {
    // `--platform ios`: the entry lives in the [ios] section (E0413
    // everywhere else), and the entry file has the external-builder shape —
    // an `export extern fn <name>_main` the app shell calls, not `fn main`.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .args(["init", "--platform", "ios", "phoneapp"])
        .output().expect("run cpc init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));

    let proj = dir.path().join("phoneapp");
    let manifest = read(&proj.join("Cplus.toml"));
    assert!(manifest.contains("[ios]"), "manifest: {manifest}");
    assert!(manifest.contains("entry = \"src/main.cplus\""), "manifest: {manifest}");
    let main = read(&proj.join("src/main.cplus"));
    assert!(main.contains("export extern fn phoneapp_main() -> i32"), "main: {main}");
    assert!(!main.contains("\nfn main"), "no fn main in an external-builder entry: {main}");
    // The next-steps point at the simulator target, not a bare cpc build.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ios-arm64-simulator"), "{stdout}");

    // The scoped app refuses to build for the host: E0413, never a guess.
    let build = Command::new(cpc())
        .current_dir(&proj)
        .arg("build")
        .env("CPLUS_HOME", proj.join(".no-store"))
        .output().expect("run cpc build");
    assert!(!build.status.success());
    let all = format!("{}{}",
        String::from_utf8_lossy(&build.stdout), String::from_utf8_lossy(&build.stderr));
    assert!(all.contains("E0413"), "expected E0413, got: {all}");
}

#[test]
fn init_two_platforms_get_their_own_entries() {
    // First-named platform owns src/main.cplus; the second gets
    // src/main_<p>.cplus — the entry SHAPES differ, so the files must too.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .args(["init", "--platform", "macos", "--platform", "ios", "both"])
        .output().expect("run cpc init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));

    let proj = dir.path().join("both");
    let manifest = read(&proj.join("Cplus.toml"));
    assert!(manifest.contains("[macos]\nentry = \"src/main.cplus\""), "{manifest}");
    assert!(manifest.contains("[ios]\nentry = \"src/main_ios.cplus\""), "{manifest}");
    let mac_main = read(&proj.join("src/main.cplus"));
    assert!(mac_main.contains("fn main() -> i32"), "{mac_main}");
    let ios_main = read(&proj.join("src/main_ios.cplus"));
    assert!(ios_main.contains("export extern fn both_main() -> i32"), "{ios_main}");
}

#[test]
fn init_rejects_an_unknown_platform() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(cpc())
        .current_dir(dir.path())
        .args(["init", "--platform", "amiga", "x"])
        .output().expect("run cpc init");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown platform `amiga`"), "{stderr}");
    assert!(stderr.contains("macos"), "the error lists the valid names: {stderr}");
    assert!(!dir.path().join("x").exists(), "nothing scaffolded on error");
}
