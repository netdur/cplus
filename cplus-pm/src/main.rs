use cplus_pm::fetch::FetchPlan;
use cplus_pm::id::PackageId;
use cplus_pm::manifest::Manifest;
use cplus_pm::resolve::{direct_dependency, resolve_graph, write_lockfile, ResolveOptions};
use cplus_pm::vendor;
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
cplus-pm - manage C+ packages in a project's vendor/ folder

usage:
  cplus-pm install DIR              resolve deps and place them in DIR/vendor/
                                    flags: --cache DIR, --repo-url URL
  cplus-pm remove DIR NAME          delete DIR/vendor/NAME
  cplus-pm update DIR               re-resolve and refresh DIR/vendor/
                                    flags: --cache DIR, --repo-url URL
  cplus-pm manifest [PATH]          parse pkg.toml and print normalized JSON
  cplus-pm resolve PATH             resolve transitive deps and print lockfile JSON
                                    flags: --cache DIR, --repo-url URL
  cplus-pm lock PATH [OUT]          resolve transitive deps and write pkg.lock
                                    flags: --cache DIR, --repo-url URL
  cplus-pm fetch ID VERSION         fetch one tagged package into a local cache
                                    flags: --cache DIR, --repo-url URL
  cplus-pm fetch-dep PATH DEP_ID    resolve one direct dep from PATH and fetch it
                                    flags: --cache DIR, --repo-url URL
  cplus-pm tag ID VERSION           print the canonical git tag for ID/VERSION
  cplus-pm -h | --help              show this message
";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("install") => install_cmd(&args[1..]),
        Some("remove") => remove_cmd(&args[1..]),
        Some("update") => update_cmd(&args[1..]),
        Some("manifest") => manifest_cmd(&args[1..]),
        Some("fetch") => fetch_cmd(&args[1..]),
        Some("fetch-dep") => fetch_dep_cmd(&args[1..]),
        Some("resolve") => resolve_cmd(&args[1..]),
        Some("lock") => lock_cmd(&args[1..]),
        Some("tag") => tag_cmd(&args[1..]),
        Some(command) => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    }
}

fn install_cmd(args: &[String]) -> Result<(), String> {
    let (positional, options) = parse_manifest_resolve_args(args)?;
    if positional.len() != 1 {
        return Err(format!("install requires a project DIR\n\n{USAGE}"));
    }
    let project_dir = PathBuf::from(&positional[0]);
    let installed = vendor::install(&project_dir, &options).map_err(|err| err.to_string())?;
    for pkg in &installed {
        println!("installed {} ({}@{})", pkg.name, pkg.id, pkg.version);
    }
    if installed.is_empty() {
        println!("no dependencies to install");
    }
    Ok(())
}

fn update_cmd(args: &[String]) -> Result<(), String> {
    // Update re-resolves (picking the newest version within each constraint) and
    // refreshes vendor/ — the same materialization as install.
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
        return Err(format!("manifest accepts at most one PATH\n\n{USAGE}"));
    }

    let path = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("pkg.toml"));
    let manifest = Manifest::load(&path).map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&manifest).map_err(|err| err.to_string())?;
    println!("{json}");
    Ok(())
}

fn fetch_cmd(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut cache = PathBuf::from(".pkgcache");
    let mut repo_url = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cache" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--cache requires a directory".to_string())?;
                cache = PathBuf::from(value);
            }
            "--repo-url" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--repo-url requires a URL or local git path".to_string())?;
                repo_url = Some(value.clone());
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(arg.clone()),
        }
    }

    if positional.len() != 2 {
        return Err(format!("fetch requires ID and VERSION\n\n{USAGE}"));
    }

    let id = PackageId::new(&positional[0]).map_err(|err| err.to_string())?;
    let plan = match repo_url {
        Some(repo_url) => FetchPlan::with_repo_url(id, &positional[1], repo_url, cache),
        None => FetchPlan::new(id, &positional[1], cache),
    };
    let package_dir = plan.fetch().map_err(|err| err.to_string())?;

    println!("{}", package_dir.display());
    Ok(())
}

fn fetch_dep_cmd(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut cache = PathBuf::from(".pkgcache");
    let mut repo_url = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cache" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--cache requires a directory".to_string())?;
                cache = PathBuf::from(value);
            }
            "--repo-url" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--repo-url requires a URL or local git path".to_string())?;
                repo_url = Some(value.clone());
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(arg.clone()),
        }
    }

    if positional.len() != 2 {
        return Err(format!("fetch-dep requires PATH and DEP_ID\n\n{USAGE}"));
    }

    let manifest = Manifest::load(&positional[0]).map_err(|err| err.to_string())?;
    let dep = direct_dependency(&manifest, &positional[1]).map_err(|err| err.to_string())?;
    let receipt = dep
        .fetch(cache, repo_url.as_deref())
        .map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&receipt).map_err(|err| err.to_string())?;

    println!("{json}");
    Ok(())
}

fn resolve_cmd(args: &[String]) -> Result<(), String> {
    let (positional, options) = parse_manifest_resolve_args(args)?;
    if positional.len() != 1 {
        return Err(format!("resolve requires PATH\n\n{USAGE}"));
    }

    let manifest = Manifest::load(&positional[0]).map_err(|err| err.to_string())?;
    let graph = resolve_graph(&manifest, &options).map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&graph.lockfile).map_err(|err| err.to_string())?;
    println!("{json}");
    Ok(())
}

fn lock_cmd(args: &[String]) -> Result<(), String> {
    let (positional, options) = parse_manifest_resolve_args(args)?;
    if positional.is_empty() || positional.len() > 2 {
        return Err(format!("lock requires PATH and optional OUT\n\n{USAGE}"));
    }

    let manifest_path = PathBuf::from(&positional[0]);
    let out = positional.get(1).map(PathBuf::from).unwrap_or_else(|| {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("pkg.lock")
    });
    let manifest = Manifest::load(&manifest_path).map_err(|err| err.to_string())?;
    write_lockfile(&manifest, &options, &out).map_err(|err| err.to_string())?;

    println!("{}", out.display());
    Ok(())
}

fn parse_manifest_resolve_args(args: &[String]) -> Result<(Vec<String>, ResolveOptions), String> {
    let mut positional = Vec::new();
    let mut options = ResolveOptions::new(".pkgcache");
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

fn tag_cmd(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err(format!("tag requires ID and VERSION\n\n{USAGE}"));
    }

    let id = PackageId::new(&args[0]).map_err(|err| err.to_string())?;
    println!("{}", id.tag_for_version(&args[1]));
    Ok(())
}
