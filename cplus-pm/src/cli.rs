//! The `cplus-pm` command-line dispatcher, as a library entry point.
//!
//! Lifted out of `main.rs` so it can back BOTH the standalone `cplus-pm`
//! binary AND the unified `cpc pm ...` subcommand (the package manager is part
//! of the one toolchain that ships via Homebrew). [`run`] takes the argument
//! list (without the program name) and returns `Ok(())` or a ready-to-print
//! error message.

use crate::manifest::{Manifest, MANIFEST_NAME};
use crate::vendor::{self, InstallOptions};
use std::path::PathBuf;

const USAGE: &str = "\
cpc pm - manage C+ packages in a project's vendor/ folder

usage:
  cpc pm install [DIR]           resolve DIR/Cplus.toml deps into DIR/vendor/
                                  DIR defaults to the current directory
                                  flags: --cache DIR, --repo-url URL
  cpc pm update [DIR]            re-resolve and refresh DIR/vendor/ (= install)
                                  flags: --cache DIR, --repo-url URL
  cpc pm remove DIR NAME          delete DIR/vendor/NAME
  cpc pm manifest [DIR]           parse DIR/Cplus.toml and print normalized JSON
  cpc pm -h | --help              show this message

(also available as the standalone `cplus-pm` command)
";

/// Run one package-manager command. `args` excludes the program name.
pub fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("install") => install_cmd(&args[1..]),
        Some("update") => update_cmd(&args[1..]),
        Some("remove") => remove_cmd(&args[1..]),
        Some("manifest") => manifest_cmd(&args[1..]),
        Some(command) => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    }
}

fn install_cmd(args: &[String]) -> Result<(), String> {
    let (positional, options) = parse_install_args(args)?;
    if positional.len() > 1 {
        return Err(format!("install takes at most one project DIR\n\n{USAGE}"));
    }
    // No DIR means the current directory, matching `cpc build`.
    let project_dir = PathBuf::from(positional.first().map(String::as_str).unwrap_or("."));
    let resolved = vendor::install(&project_dir, &options).map_err(|err| err.to_string())?;
    for pkg in &resolved {
        let verb = if pkg.fresh { "installed" } else { "present " };
        println!("{verb} {} ({}@{})", pkg.name, pkg.repo, pkg.version);
    }
    match resolved.iter().filter(|p| p.fresh).count() {
        _ if resolved.is_empty() => println!("no dependencies to install"),
        0 => println!("all {} dependencies already present", resolved.len()),
        n => println!("installed {n} of {} dependencies", resolved.len()),
    }
    Ok(())
}

fn update_cmd(args: &[String]) -> Result<(), String> {
    // Update re-resolves and refreshes vendor/ — same materialization as install.
    install_cmd(args)
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

fn parse_install_args(args: &[String]) -> Result<(Vec<String>, InstallOptions), String> {
    let mut positional = Vec::new();
    let mut options = InstallOptions::new(".pkgcache");
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cache" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--cache requires a directory".to_string())?;
                options.cache_root = PathBuf::from(value);
            }
            "--repo-url" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--repo-url requires a URL or local git path".to_string())?;
                options.repo_url_override = Some(value.clone());
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok((Vec::new(), options));
            }
            _ => positional.push(arg.clone()),
        }
    }

    Ok((positional, options))
}
