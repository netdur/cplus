//! The `cplus-pm` command-line dispatcher, as a library entry point.
//!
//! Lifted out of `main.rs` so it can back BOTH the standalone `cplus-pm`
//! binary AND the unified `cpc pm ...` subcommand (the package manager is
//! part of the one toolchain that ships via Homebrew). `cpc pm` calls
//! [`run_with_toolchain`] with its own identity (repo + version), which is
//! what makes bare `stdlib = "*"` deps and the store tier work (D15); the
//! standalone binary calls [`run`] and takes the same identity from flags.

use crate::manifest::{Manifest, MANIFEST_NAME};
use crate::store::ToolchainContext;
use crate::vendor::{self, InstallOptions, Location};
use std::path::PathBuf;

const USAGE: &str = "\
cpc pm - manage C+ packages: the per-user store, or a project's vendor/

usage:
  cpc pm install [DIR]           resolve DIR/Cplus.toml deps into the store
                                  (~/.cplus/<tier>/vendor/); DIR defaults to
                                  the current directory
  cpc pm update [DIR]            re-resolve and refresh (= install)
  cpc pm add DIR NAME [SPEC]     write NAME and its declared closure into
                                  DIR/Cplus.toml (platform sections mapped to
                                  the project's target platforms), then install.
                                  SPEC defaults to '*' (the toolchain package);
                                  a tree-URL pins a third-party package.
                                  extra flag: --platform P (repeatable) extends
                                  the target set beyond declared+host
  cpc pm add DIR --maven G:A:V    add a third-party Maven/AAR coordinate to
                                  [android.maven], print what its closure
                                  costs, and download it. No Gradle.
  cpc pm remove DIR NAME          delete DIR/vendor/NAME (the project copy;
                                  the shared store is never touched)
  cpc pm manifest [DIR]           parse DIR/Cplus.toml and print normalized JSON
  cpc pm maven [WHAT] [DIR]       what the Maven closure gives an Android
                                  build, resolved offline from the local repo.
                                  WHAT: list (default — the priced closure),
                                  classpath (jars for `d8`), manifests, res,
                                  jni, root (the local repo path)
  cpc pm maven price G:A:V        what one coordinate WOULD cost: resolve and
                                  download its closure, print the artifact
                                  count and the megabytes. No project, no
                                  manifest touched. Run this BEFORE taking on
                                  an AAR dependency — see plans/aar.md
  cpc pm -h | --help              show this message

install/update/add flags:
  --local                install into DIR/vendor/ instead of the store
  --store DIR            store root (default $CPLUS_HOME, else ~/.cplus)
  --cache DIR            clone cache (default <store>/cache)
  --repo-url URL         override every clone URL (local path = offline)
  --toolchain-repo R     toolchain monorepo, e.g. github.com/netdur/cplus
  --toolchain-version V  toolchain version — names the store tier
                          (`cpc pm` supplies both automatically)
  --maven G:A:V          (add only) add a Maven coordinate instead of a
                          C+ package
  --m2 DIR               local Maven repo (default <store>/m2)
  --maven-repo URL       remote Maven repo, repeatable, in order
                          (default: Google's Maven, then Maven Central)
  --maven-offline        never download a Maven artifact; resolve from the
                          local repo or fail

(also available as the standalone `cplus-pm` command)
";

/// Run one package-manager command with no toolchain identity of its own
/// (the standalone binary; flags may still supply one).
pub fn run(args: Vec<String>) -> Result<(), String> {
    run_with_toolchain(args, None)
}

/// Run one package-manager command. `args` excludes the program name.
/// `toolchain` is the caller's identity (`cpc pm` passes its own);
/// `--toolchain-repo`/`--toolchain-version` flags override it.
pub fn run_with_toolchain(
    args: Vec<String>,
    toolchain: Option<ToolchainContext>,
) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("install") => install_cmd(&args[1..], toolchain),
        Some("update") => update_cmd(&args[1..], toolchain),
        Some("add") => add_cmd(&args[1..], toolchain),
        Some("remove") => remove_cmd(&args[1..]),
        Some("manifest") => manifest_cmd(&args[1..]),
        Some("maven") => maven_cmd(&args[1..], toolchain),
        Some(command) => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    }
}

fn install_cmd(args: &[String], toolchain: Option<ToolchainContext>) -> Result<(), String> {
    // `None` means `-h`/`--help` was handled (usage printed): do NOT fall
    // through and install the current directory.
    let Parsed {
        positional, options, ..
    } = match parse_install_args(args, toolchain)? {
        Some(parsed) => parsed,
        None => return Ok(()),
    };
    if positional.len() > 1 {
        return Err(format!("install takes at most one project DIR\n\n{USAGE}"));
    }
    // No DIR means the current directory, matching `cpc build`.
    let project_dir = PathBuf::from(positional.first().map(String::as_str).unwrap_or("."));
    let report = vendor::install(&project_dir, &options).map_err(|err| err.to_string())?;
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    for pkg in &report.packages {
        let verb = if pkg.fresh { "installed" } else { "present " };
        let place = match pkg.location {
            Location::Store => "store",
            Location::Local => "vendor/",
        };
        println!("{verb} {} ({}@{}) -> {place}", pkg.name, pkg.repo, pkg.version);
    }
    match report.packages.iter().filter(|p| p.fresh).count() {
        _ if report.packages.is_empty() => println!("no dependencies to install"),
        0 => println!("all {} dependencies already present", report.packages.len()),
        n => println!("installed {n} of {} dependencies", report.packages.len()),
    }
    Ok(())
}

fn update_cmd(args: &[String], toolchain: Option<ToolchainContext>) -> Result<(), String> {
    // Pre-1.0, update re-runs the install materialization (D6); the tier is
    // exact, so there is nothing to advance within it (D14).
    install_cmd(args, toolchain)
}

/// `add DIR NAME [SPEC]` — write the dependency + its declared closure into
/// the manifest (D17), then materialize with the same install path.
fn add_cmd(args: &[String], toolchain: Option<ToolchainContext>) -> Result<(), String> {
    let Parsed {
        positional,
        options,
        platforms,
        maven,
    } = match parse_install_args(args, toolchain)? {
        Some(parsed) => parsed,
        None => return Ok(()),
    };
    if let Some(coordinate) = maven {
        return add_maven_cmd(&positional, &coordinate, &options);
    }
    if positional.len() < 2 || positional.len() > 3 {
        return Err(format!("add requires DIR and NAME (and an optional SPEC)\n\n{USAGE}"));
    }
    let project_dir = PathBuf::from(&positional[0]);
    let name = &positional[1];
    let spec = positional.get(2).map(String::as_str);
    let report = crate::add::add(&project_dir, name, spec, &platforms, &options)
        .map_err(|err| err.to_string())?;
    for e in &report.entries {
        let section = match &e.platform {
            Some(p) => format!("[{p}.dependencies]"),
            None => "[dependencies]".to_string(),
        };
        match &e.action {
            crate::add::AddAction::Added => {
                println!("added    {} = \"{}\"  in {section}", e.name, e.spec)
            }
            crate::add::AddAction::Present => println!("present  {}  in {section}", e.name),
            crate::add::AddAction::KeptDifferent { existing } => println!(
                "kept     {} = \"{existing}\"  in {section} (add wanted \"{}\")",
                e.name, e.spec
            ),
        }
    }
    println!("target platforms: {}", report.platforms.join(", "));
    let installed = vendor::install(&project_dir, &options).map_err(|err| err.to_string())?;
    for warning in &installed.warnings {
        eprintln!("warning: {warning}");
    }
    let fresh = installed.packages.iter().filter(|p| p.fresh).count();
    println!("installed {fresh} of {} dependencies", installed.packages.len());
    report_maven(&installed);
    Ok(())
}

/// `add DIR --maven G:A:V` — pin a third-party Maven/AAR coordinate in
/// `[android.maven]`, print what its closure costs, and download it (D18).
fn add_maven_cmd(
    positional: &[String],
    coordinate: &str,
    options: &InstallOptions,
) -> Result<(), String> {
    if positional.len() != 1 {
        return Err(format!("add --maven requires DIR\n\n{USAGE}"));
    }
    let project_dir = PathBuf::from(&positional[0]);
    let report = crate::add::add_maven(&project_dir, coordinate, options)
        .map_err(|err| err.to_string())?;
    match &report.action {
        crate::add::AddAction::Added => println!(
            "added    \"{}\" = \"{}\"  in [android.maven]",
            report.coord.ga(),
            report.coord.version
        ),
        crate::add::AddAction::Present => {
            println!("present  {}  in [android.maven]", report.coord.ga())
        }
        crate::add::AddAction::KeptDifferent { existing } => println!(
            "kept     \"{}\" = \"{existing}\"  in [android.maven] (add wanted \"{}\")",
            report.coord.ga(),
            report.coord.version
        ),
    }
    // The resolved closure belongs to the coordinate that was ASKED for. If
    // the manifest kept a different pin, printing it would price a closure
    // this project does not have — the install report below prints the real
    // one either way.
    if !matches!(report.action, crate::add::AddAction::KeptDifferent { .. }) {
        println!("closure: {} artifacts", report.closure.order.len());
        if !report.closure.bom_imports.is_empty() {
            println!(
                "BOM imports followed: {}",
                report.closure.bom_imports.join(", ")
            );
        }
    }
    let installed = vendor::install(&project_dir, options).map_err(|err| err.to_string())?;
    for warning in &installed.warnings {
        eprintln!("warning: {warning}");
    }
    report_maven(&installed);
    Ok(())
}

/// Say where this resolver and a Gradle build would disagree, and only
/// there. Nearest-wins vs highest-wins is the documented divergence
/// (`plans/aar.md` §2), but it only BITES when the nearest version is the
/// older one — a CameraX closure asks for kotlin-stdlib eight times and
/// agrees with Gradle every time.
fn report_conflicts(closure: &vendor::MavenClosure) {
    for conflict in closure.divergent() {
        eprintln!(
            "warning: {} resolved to {} (Maven nearest-wins) but {} was also requested \u{2014} Gradle would take the higher one; pin it in [android.maven] to match",
            conflict.ga,
            conflict.kept,
            conflict.dropped.join(", "),
        );
    }
    let agreed = closure.conflicts.len() - closure.divergent().count();
    if agreed > 0 {
        println!("{agreed} version conflicts resolved to the highest version (as Gradle would)");
    }
}

/// The Maven side of an install report: what landed, and what it costs.
/// The byte total is the number the decision to take an AAR turns on
/// (`plans/aar.md` §3 — 178x the dex for one feature).
fn report_maven(installed: &vendor::InstallReport) {
    if installed.maven.is_empty() {
        return;
    }
    let mut total = 0u64;
    let mut fresh = 0usize;
    for artifact in &installed.maven {
        total += artifact.bytes;
        fresh += usize::from(artifact.fresh);
        println!(
            "{} {:>4} {:>8} KB  {}{}",
            if artifact.fresh { "fetched " } else { "present " },
            artifact.kind.ext(),
            artifact.bytes / 1024,
            artifact.coord,
            // A facade AAR: no code of its own, its variant is in the
            // closure. Worth showing so nobody hunts for a missing jar.
            if artifact.classes.is_none() { "  (no code)" } else { "" },
        );
    }
    println!(
        "maven: {} artifacts, {:.1} MB ({fresh} fetched this run)",
        installed.maven.len(),
        total as f64 / 1048576.0
    );
    if let Some(closure) = &installed.maven_closure {
        report_conflicts(closure);
    }
}

/// `cpc pm maven [WHAT] [DIR]` — what the resolved Maven closure gives an
/// Android build.
///
/// ALWAYS OFFLINE. These are build inputs: a `d8` invocation that reaches
/// the network is a build that fails differently on a plane. Everything is
/// resolved from the local Maven repo `install` filled; a gap says so and
/// names `cpc pm install`.
fn maven_cmd(args: &[String], toolchain: Option<ToolchainContext>) -> Result<(), String> {
    const WHAT: [&str; 7] = [
        "list",
        "classpath",
        "manifests",
        "res",
        "jni",
        "root",
        "price",
    ];
    let Parsed {
        positional,
        mut options,
        ..
    } = match parse_install_args(args, toolchain)? {
        Some(parsed) => parsed,
        None => return Ok(()),
    };
    // `price` is the one that may reach out: its whole job is to fetch a
    // closure nobody has yet, to find out what it weighs.
    options.maven_offline = positional.first().map(String::as_str) != Some("price");
    let (what, dir) = match positional.split_first() {
        Some((first, rest)) if WHAT.contains(&first.as_str()) => (first.as_str(), rest.first()),
        Some((first, _)) if first.starts_with('-') => {
            return Err(format!("unknown flag `{first}`\n\n{USAGE}"))
        }
        Some((dir, rest)) if rest.is_empty() => ("list", Some(dir)),
        None => ("list", None),
        Some(_) => return Err(format!("maven takes at most WHAT and DIR\n\n{USAGE}")),
    };
    let registry = options.registry().map_err(|err| err.to_string())?;
    if what == "root" {
        println!("{}", registry.root.display());
        return Ok(());
    }
    if what == "price" {
        let coordinate = dir.ok_or_else(|| {
            format!("maven price needs a `group:artifact:version` coordinate\n\n{USAGE}")
        })?;
        return price_cmd(&registry, coordinate);
    }

    let project_dir = PathBuf::from(dir.map(String::as_str).unwrap_or("."));
    // NOT `install`: these are build inputs, and a `d8` line that can start a
    // git clone is a build that behaves differently on a plane.
    let (artifacts, closure) =
        vendor::maven_artifacts(&project_dir, &options).map_err(|err| err.to_string())?;
    if artifacts.is_empty() {
        // Not an error: a project with no `[android.maven]` prints nothing,
        // so a build script can interpolate this command unconditionally.
        if what == "list" {
            println!("no Maven dependencies");
        }
        return Ok(());
    }
    match what {
        "list" => {
            let mut total = 0u64;
            for artifact in &artifacts {
                total += artifact.bytes;
                println!(
                    "  {:>4} {:>8} KB  {}{}",
                    artifact.kind.ext(),
                    artifact.bytes / 1024,
                    artifact.coord,
                    if artifact.classes.is_none() {
                        "  (no code)"
                    } else {
                        ""
                    },
                );
            }
            println!(
                "maven: {} artifacts, {:.1} MB",
                artifacts.len(),
                total as f64 / 1048576.0
            );
            report_conflicts(&closure);
            Ok(())
        }
        "classpath" => print_paths(artifacts.iter().map(|a| a.classes.clone())),
        "manifests" => print_paths(artifacts.iter().map(|a| a.manifest.clone())),
        "res" => print_paths(artifacts.iter().map(|a| a.res.clone())),
        "jni" => print_paths(artifacts.iter().map(|a| a.jni.clone())),
        other => Err(format!("unknown `maven` query `{other}`\n\n{USAGE}")),
    }
}

/// `cpc pm maven price G:A:V` — what one coordinate would cost, before
/// anyone commits to it.
///
/// The measurement in `plans/aar.md` is the argument this exists to make
/// cheap: CameraX is 35 artifacts and 8.2 MB, which dexes to 178x facet's
/// whole dex for one feature. That number is what the decision turns on, and
/// it is printable in thirty seconds — so print it first.
fn price_cmd(registry: &crate::maven::Registry, coordinate: &str) -> Result<(), String> {
    let coord = crate::maven::Coord::parse(coordinate).map_err(|err| err.to_string())?;
    let closure = crate::maven::resolve(registry, std::slice::from_ref(&coord))
        .map_err(|err| err.to_string())?;
    let (artifacts, missing) =
        crate::maven::materialize_lenient(registry, &closure).map_err(|err| err.to_string())?;
    let mut total = 0u64;
    for artifact in &artifacts {
        total += artifact.bytes;
        println!(
            "  {:>4} {:>8} KB  {}{}",
            artifact.kind.ext(),
            artifact.bytes / 1024,
            artifact.coord,
            if artifact.classes.is_none() {
                "  (no code)"
            } else {
                ""
            },
        );
    }
    println!(
        "closure: {} artifacts, {:.1} MB",
        artifacts.len(),
        total as f64 / 1048576.0
    );
    if !closure.bom_imports.is_empty() {
        println!("BOM imports followed: {}", closure.bom_imports.join(", "));
    }
    report_conflicts(&closure);
    for entry in closure.unresolved.iter().chain(&missing) {
        eprintln!("UNRESOLVED: {} — {}", entry.what, entry.reason);
    }
    Ok(())
}

fn print_paths(paths: impl Iterator<Item = Option<PathBuf>>) -> Result<(), String> {
    for path in paths.flatten() {
        println!("{}", path.display());
    }
    Ok(())
}

fn remove_cmd(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err(format!("remove requires DIR and NAME\n\n{USAGE}"));
    }
    let project_dir = PathBuf::from(&args[0]);
    vendor::remove(&project_dir, &args[1]).map_err(|err| err.to_string())?;
    println!("removed {}", args[1]);
    Ok(())
}

fn manifest_cmd(args: &[String]) -> Result<(), String> {
    if args.len() > 1 {
        return Err(format!("manifest accepts at most one DIR\n\n{USAGE}"));
    }

    // Accept either a project directory or a direct path to a Cplus.toml.
    let arg = args.first().map(PathBuf::from).unwrap_or_default();
    let manifest = if arg.is_file() {
        Manifest::load(&arg)
    } else {
        Manifest::load(arg.join(MANIFEST_NAME))
    }
    .map_err(|err| err.to_string())?;

    let json = serde_json::json!({
        "name": manifest.name,
        "version": manifest.version,
        "dependencies": manifest.deps,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json).map_err(|err| err.to_string())?
    );
    Ok(())
}

/// The parsed form of an `install`/`update`/`add`/`maven` command line.
#[derive(Debug, Default)]
struct Parsed {
    positional: Vec<String>,
    options: InstallOptions,
    /// `--platform` values: extra target platforms for `add`'s closure.
    platforms: Vec<String>,
    /// `--maven G:A:V`: add a Maven coordinate rather than a C+ package.
    maven: Option<String>,
}

/// Parse `install`/`update` arguments. Returns `Ok(None)` when `-h`/`--help`
/// was handled (usage already printed) so the caller stops instead of
/// installing; `Ok(Some(..))` carries the parsed positional args + options.
fn parse_install_args(
    args: &[String],
    toolchain: Option<ToolchainContext>,
) -> Result<Option<Parsed>, String> {
    let mut positional = Vec::new();
    let mut platforms = Vec::new();
    let mut maven = None;
    let mut maven_repos: Vec<String> = Vec::new();
    let mut options = InstallOptions::new();
    options.toolchain = toolchain;
    let mut iter = args.iter();

    let take = |flag: &str, iter: &mut std::slice::Iter<'_, String>| {
        iter.next()
            .cloned()
            .ok_or_else(|| format!("{flag} requires a value"))
    };

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--local" => options.local = true,
            "--platform" => platforms.push(take("--platform", &mut iter)?),
            "--maven" => maven = Some(take("--maven", &mut iter)?),
            "--m2" => options.m2_root = Some(PathBuf::from(take("--m2", &mut iter)?)),
            "--maven-repo" => maven_repos.push(take("--maven-repo", &mut iter)?),
            "--maven-offline" => options.maven_offline = true,
            "--store" => options.store_root = Some(PathBuf::from(take("--store", &mut iter)?)),
            "--cache" => options.cache_root = Some(PathBuf::from(take("--cache", &mut iter)?)),
            "--repo-url" => options.repo_url_override = Some(take("--repo-url", &mut iter)?),
            "--toolchain-repo" => {
                let repo = take("--toolchain-repo", &mut iter)?;
                options.toolchain = Some(match options.toolchain.take() {
                    Some(mut t) => {
                        t.repo = repo;
                        t
                    }
                    None => ToolchainContext {
                        repo,
                        version: String::new(),
                        package_root: "vendor".to_string(),
                    },
                });
            }
            "--toolchain-version" => {
                let version = take("--toolchain-version", &mut iter)?;
                options.toolchain = Some(match options.toolchain.take() {
                    Some(mut t) => {
                        t.version = version;
                        t
                    }
                    None => ToolchainContext {
                        repo: String::new(),
                        version,
                        package_root: "vendor".to_string(),
                    },
                });
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            _ => positional.push(arg.clone()),
        }
    }

    // A half-supplied identity would fetch from an empty repo URL or name an
    // empty tier — refuse it before it becomes a confusing git error.
    if let Some(t) = &options.toolchain {
        if t.repo.is_empty() || t.version.is_empty() {
            return Err(
                "--toolchain-repo and --toolchain-version must be supplied together".to_string(),
            );
        }
    }

    if !maven_repos.is_empty() {
        options.maven_repos = Some(maven_repos);
    }
    Ok(Some(Parsed {
        positional,
        options,
        platforms,
        maven,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_help_flag_stops_before_installing() {
        // `--help` / `-h` prints usage and yields None, so install_cmd returns
        // without falling through to `install .`.
        assert!(parse_install_args(&["--help".to_string()], None)
            .unwrap()
            .is_none());
        assert!(parse_install_args(&["-h".to_string()], None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn install_parses_positional_dir_and_flags() {
        let Parsed {
            positional, options, ..
        } = parse_install_args(
            &[
                "mydir".to_string(),
                "--local".to_string(),
                "--repo-url".to_string(),
                "file:///x".to_string(),
                "--store".to_string(),
                "/tmp/store".to_string(),
            ],
            None,
        )
        .unwrap()
        .expect("normal args parse to Some");
        assert_eq!(positional, vec!["mydir".to_string()]);
        assert!(options.local);
        assert_eq!(options.repo_url_override.as_deref(), Some("file:///x"));
        assert_eq!(options.store_root.as_deref(), Some("/tmp/store".as_ref()));
    }

    #[test]
    fn toolchain_flags_build_and_override_the_context() {
        // Flags alone build a context…
        let Parsed { options, .. } = parse_install_args(
            &[
                "--toolchain-repo".to_string(),
                "github.com/x/y".to_string(),
                "--toolchain-version".to_string(),
                "0.0.9".to_string(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        let t = options.toolchain.unwrap();
        assert_eq!(t.repo, "github.com/x/y");
        assert_eq!(t.version, "0.0.9");
        assert_eq!(t.package_root, "vendor");

        // …and override the caller-supplied one field-wise.
        let base = ToolchainContext {
            repo: "github.com/netdur/cplus".to_string(),
            version: "0.0.27".to_string(),
            package_root: "vendor".to_string(),
        };
        let Parsed { options, .. } = parse_install_args(
            &["--toolchain-version".to_string(), "0.0.9".to_string()],
            Some(base),
        )
        .unwrap()
        .unwrap();
        let t = options.toolchain.unwrap();
        assert_eq!(t.repo, "github.com/netdur/cplus");
        assert_eq!(t.version, "0.0.9");
    }

    #[test]
    fn maven_flags_parse() {
        let Parsed {
            maven,
            options,
            positional,
            ..
        } = parse_install_args(
            &[
                ".".to_string(),
                "--maven".to_string(),
                "com.x:y:1.0".to_string(),
                "--m2".to_string(),
                "/tmp/m2".to_string(),
                "--maven-repo".to_string(),
                "file:///a".to_string(),
                "--maven-repo".to_string(),
                "file:///b".to_string(),
                "--maven-offline".to_string(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(positional, vec![".".to_string()]);
        assert_eq!(maven.as_deref(), Some("com.x:y:1.0"));
        assert_eq!(options.m2_root.as_deref(), Some("/tmp/m2".as_ref()));
        // Repo ORDER is load-bearing: androidx is on Google's Maven and
        // Central answers 404 for it.
        assert_eq!(
            options.maven_repos.as_deref(),
            Some(["file:///a".to_string(), "file:///b".to_string()].as_slice())
        );
        assert!(options.maven_offline);
    }

    #[test]
    fn a_half_supplied_toolchain_identity_is_rejected() {
        let err = parse_install_args(
            &["--toolchain-version".to_string(), "0.0.9".to_string()],
            None,
        )
        .unwrap_err();
        assert!(err.contains("together"));
    }
}
