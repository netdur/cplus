//! End-to-end: platform-scoped dependencies (`[<platform>.dependencies]`).
//!
//! A dep scoped to another platform must vanish from the build entirely (no
//! presence check, no `[link]` splice, no import resolution); a dep scoped to
//! the ACTIVE platform must behave exactly like a base `[dependencies]` entry.
//! Fixtures are built per-test in a tempdir; every test computes the host's
//! platform name so the suite passes on any OS.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cpc() -> &'static str {
    env!("CARGO_BIN_EXE_cpc")
}

/// The manifest platform name of the machine the tests run on — must mirror
/// `cplus_core::target::platform_name(HOST.os)`.
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Any platform that is NOT the host — deps scoped here must be skipped.
fn other_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "linux"
    } else {
        "macos"
    }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A minimal dep-free vendor package with one module exporting `answer()`.
fn write_vendor_pkg(project: &Path, name: &str, answer: i32) {
    write(
        &project.join(format!("vendor/{name}/Cplus.toml")),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.0.1\"\n"),
    );
    write(
        &project.join(format!("vendor/{name}/src/util.cplus")),
        &format!("fn answer() -> i32 {{\n    return {answer};\n}}\n"),
    );
}

fn build(project: &Path) -> std::process::Output {
    Command::new(cpc())
        .current_dir(project)
        .arg("build")
        // Hermetic from any populated per-user store (~/.cplus): these
        // fixtures assert on missing/absent vendor packages, which the
        // store fallback could otherwise rescue.
        .env("CPLUS_HOME", project.join(".no-store"))
        .output()
        .expect("run cpc build")
}

fn run_built(project: &Path) -> i32 {
    let bin = project.join("target/debug/app");
    let status = Command::new(&bin)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", bin.display()));
    status.code().expect("exit code")
}

#[test]
fn deps_scoped_to_another_platform_are_skipped_entirely() {
    // `ghostlib` is declared for the other platform and does NOT exist in
    // vendor/ — pre-platform-deps this failed the presence check (E0854)
    // and spliced foreign link flags. Now the build must succeed.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 7);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [dependencies]\nherelib = \"*\"\n\n\
             [{}.dependencies]\nghostlib = \"*\"\n",
            other_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "build must skip the off-platform dep:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run_built(project), 7);
}

#[test]
fn deps_scoped_to_the_active_platform_participate() {
    // Scoped to the HOST platform: behaves exactly like a base dep.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 42);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [{}.dependencies]\nherelib = \"*\"\n",
            host_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "an active-platform dep must resolve:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run_built(project), 42);
}

#[test]
fn importing_an_off_platform_dep_is_a_targeted_e0866() {
    // The package IS declared (for the other platform) and IS in vendor/ —
    // importing it here must say "not available on platform", not the
    // misleading E0852 "not a declared dependency".
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "gatedlib", 1);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [{}.dependencies]\ngatedlib = \"*\"\n",
            other_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "import \"gatedlib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(!out.status.success(), "off-platform import must fail");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("E0866"), "expected E0866, got:\n{all}");
    assert!(
        all.contains("platform") && all.contains("gatedlib"),
        "diagnostic should name the platform gate and the package:\n{all}"
    );
    assert!(
        !all.contains("E0852"),
        "must not report the dep as undeclared:\n{all}"
    );
}

#[test]
fn conflicting_dependency_declarations_fail_with_e0869() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 1);
    write(
        &project.join("Cplus.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             [dependencies]\nherelib = \"*\"\n\n\
             [{}.dependencies]\nherelib = \"*\"\n",
            host_platform()
        ),
    );
    write(
        &project.join("src/main.cplus"),
        "fn main() -> i32 {\n    return 0;\n}\n",
    );

    let out = build(project);
    assert!(!out.status.success(), "duplicate declaration must fail");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("E0869"), "expected E0869, got:\n{all}");
}

/// `[android.maven]` is a real table, not an unknown key: `deny_unknown_fields`
/// would otherwise make a valid manifest a hard parse error, and a
/// SILENTLY-accepted one would drop the coordinate on the floor.
#[test]
fn android_maven_coordinates_parse_and_build() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n         [android.maven]\n\"com.google.android.gms:play-services-maps\" = \"19.0.0\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "fn main() -> i32 {\n    return 0;\n}\n",
    );
    let out = build(project);
    assert!(
        out.status.success(),
        "a manifest with [android.maven] must build:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The three ways an `[android.maven]` entry is wrong, each E0877. Validated
/// by the compiler because it is the one tool that reads every manifest, and
/// a coordinate that is quietly dropped is a class missing at runtime on a
/// device.
#[test]
fn malformed_maven_coordinates_fail_with_e0877() {
    for (section, key, version) in [
        // Maven is an Android ecosystem; the table anywhere else is a mistake.
        ("ios", "com.x:y", "1.0"),
        // The version is the VALUE, not a third field in the key.
        ("android", "com.x:y:1.0", "1.0"),
        ("android", "com.x", "1.0"),
        // Exact pins only — there is no version solver here, on purpose.
        ("android", "com.x:y", "*"),
        ("android", "com.x:y", ""),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(
            &project.join("Cplus.toml"),
            &format!(
                "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n                 [{section}.maven]\n\"{key}\" = \"{version}\"\n"
            ),
        );
        write(
            &project.join("src/main.cplus"),
            "fn main() -> i32 {\n    return 0;\n}\n",
        );
        let out = build(project);
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "`[{section}.maven] {key} = {version}` must fail");
        assert!(
            all.contains("E0877"),
            "expected E0877 for `[{section}.maven] {key} = {version}`, got:\n{all}"
        );
    }
}

#[test]
fn platform_suffix_module_shadows_the_base_file() {
    // The `<name>_<platform>.cplus` override is keyed off the ACTIVE
    // platform's name (same vocabulary as the manifest sections) — a
    // `util_macos.cplus` sibling now shadows `util.cplus` on macOS the same
    // way `util_linux.cplus` always did on Linux.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_vendor_pkg(project, "herelib", 1); // base answer() -> 1
    write(
        &project.join(format!(
            "vendor/herelib/src/util_{}.cplus",
            host_platform()
        )),
        "fn answer() -> i32 {\n    return 2;\n}\n",
    );
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nherelib = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"herelib/util\" as util;\n\nfn main() -> i32 {\n    return util::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "build failed:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        run_built(project),
        2,
        "the _{} override should shadow the base module",
        host_platform()
    );
}

#[test]
fn prebuilt_slice_excludes_the_packages_test_root() {
    // `src/test_main.cplus` is a package's TEST entry — `cpc test` compiles it
    // directly (`Manifest::test_entry`) — and it has no business in the
    // library. Importing it into the synthesized entry made the archive's LINK
    // REQUIREMENTS a function of what the TESTS import: `inspector`'s suite
    // imports its macOS overlay in order to exercise it, so the iOS slice
    // referenced the `appkit` package — a `[macos.dependencies]`, not linked
    // for iOS — and a facet app on the simulator failed to link on 59 symbols
    // named after a package it never asked for (2026-08-22).
    //
    // The test root below calls a C symbol nothing defines. Compiled into the
    // slice that is an undefined reference the app link cannot satisfy — a
    // package is ONE object, so pulling any of it pulls all of it. Left out, the
    // app builds and runs.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        &project.join("vendor/plat/Cplus.toml"),
        "[package]\nname = \"plat\"\nversion = \"0.0.1\"\n\n[build]\nprebuild = true\n",
    );
    write(
        &project.join("vendor/plat/src/eng.cplus"),
        "fn answer() -> i32 {\n    return 4;\n}\n",
    );
    write(
        &project.join("vendor/plat/src/test_main.cplus"),
        "import \"./eng\" as eng;\n\n         extern fn plat_nothing_defines_this_v1();\n\n         fn reaches_outside_the_library() {\n    { plat_nothing_defines_this_v1(); }\n    return;\n}\n\n         #[test]\nfn the_suite_calls_it() {\n    reaches_outside_the_library();\n    assert eng::answer() == 4;\n}\n",
    );
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nplat = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"plat/eng\" as eng;\n\nfn main() -> i32 {\n    return eng::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "a package's test root reached the library slice:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run_built(project), 4, "the library itself still links and runs");
}

#[test]
fn prebuilt_slice_excludes_foreign_platform_variants() {
    // A `[build] prebuild = true` package with two platform-variant module
    // families, every file exporting a C symbol of the same name as its
    // siblings. The synthesized library entry must compile exactly the set
    // an app build resolves — the active variant where one exists, the base
    // where none does, and NEVER a foreign variant, whose duplicate export
    // fails the merged module (stdlib's reactor_{linux,windows} vs reactor
    // was the live case, 2026-08-16).
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        &project.join("vendor/plat/Cplus.toml"),
        "[package]\nname = \"plat\"\nversion = \"0.0.1\"\n\n[build]\nprebuild = true\n",
    );
    // Family 1: base + ACTIVE variant + foreign variant. The active variant
    // is what both the slice and the app must use.
    write(
        &project.join("vendor/plat/src/eng.cplus"),
        "export extern fn plat_eng_probe_v1() {}\n\nfn answer() -> i32 {\n    return 1;\n}\n",
    );
    write(
        &project.join(format!("vendor/plat/src/eng_{}.cplus", host_platform())),
        "export extern fn plat_eng_probe_v1() {}\n\nfn answer() -> i32 {\n    return 2;\n}\n",
    );
    write(
        &project.join(format!("vendor/plat/src/eng_{}.cplus", other_platform())),
        "export extern fn plat_eng_probe_v1() {}\n\nfn answer() -> i32 {\n    return 3;\n}\n",
    );
    // Family 2: base + foreign variant only (stdlib's exact reactor shape
    // on macOS). The base must stay in the slice.
    write(
        &project.join("vendor/plat/src/pump.cplus"),
        "export extern fn plat_pump_probe_v1() {}\n\nfn answer() -> i32 {\n    return 5;\n}\n",
    );
    write(
        &project.join(format!("vendor/plat/src/pump_{}.cplus", other_platform())),
        "export extern fn plat_pump_probe_v1() {}\n\nfn answer() -> i32 {\n    return 7;\n}\n",
    );
    write(
        &project.join("Cplus.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [dependencies]\nplat = \"*\"\n",
    );
    write(
        &project.join("src/main.cplus"),
        "import \"plat/eng\" as eng;\nimport \"plat/pump\" as pump;\n\n\
         fn main() -> i32 {\n    return eng::answer() * 10 + pump::answer();\n}\n",
    );

    let out = build(project);
    assert!(
        out.status.success(),
        "prebuild with platform variants failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // eng resolves to the active variant (2), pump to its base (5).
    assert_eq!(run_built(project), 25, "slice must match the app-resolved set");
    // The slice really was built (prebuild ran, not source fallback).
    let lib = project.join("vendor/plat/lib");
    assert!(lib.join("include").is_dir(), "headers were generated");
}
