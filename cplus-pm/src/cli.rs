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
  cpc pm remove DIR NAME          delete DIR/vendor/NAME (the project copy;
                                  the shared store is never touched)
  cpc pm manifest [DIR]           parse DIR/Cplus.toml and print normalized JSON
  cpc pm -h | --help              show this message

install/update/add flags:
  --local                install into DIR/vendor/ instead of the store
  --store DIR            store root (default $CPLUS_HOME, else ~/.cplus)
  --cache DIR            clone cache (default <store>/cache)
  --repo-url URL         override every clone URL (local path = offline)
  --toolchain-repo R     toolchain monorepo, e.g. github.com/netdur/cplus
  --toolchain-version V  toolchain version — names the store tier
                          (`cpc pm` supplies both automatically)

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
        Some(command) => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    }
}

fn install_cmd(args: &[String], toolchain: Option<ToolchainContext>) -> Result<(), String> {
    // `None` means `-h`/`--help` was handled (usage printed): do NOT fall
    // through and install the current directory.
    let (positional, options, _platforms) = match parse_install_args(args, toolchain)? {
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
    let (positional, options, platforms) = match parse_install_args(args, toolchain)? {
        Some(parsed) => parsed,
        None => return Ok(()),
    };
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

/// Parse `install`/`update` arguments. Returns `Ok(None)` when `-h`/`--help`
/// was handled (usage already printed) so the caller stops instead of
/// installing; `Ok(Some(..))` carries the parsed positional args + options.
fn parse_install_args(
    args: &[String],
    toolchain: Option<ToolchainContext>,
) -> Result<Option<(Vec<String>, InstallOptions, Vec<String>)>, String> {
    let mut positional = Vec::new();
    let mut platforms = Vec::new();
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

    Ok(Some((positional, options, platforms)))
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
        let (positional, options, _) = parse_install_args(
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
        let (_, options, _) = parse_install_args(
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
        let (_, options, _) = parse_install_args(
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
    fn a_half_supplied_toolchain_identity_is_rejected() {
        let err = parse_install_args(
            &["--toolchain-version".to_string(), "0.0.9".to_string()],
            None,
        )
        .unwrap_err();
        assert!(err.contains("together"));
    }
}
