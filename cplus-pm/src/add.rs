//! `cpc pm add` — write a dependency and its declared closure into the
//! project manifest, then install (D17 in `docs/decisions.md`).
//!
//! The package's own manifest is the source of truth: `add facet` fetches
//! facet (one cached clone, same as install), reads ITS `[dependencies]`
//! and `[<platform>.dependencies]`, and writes the package plus its closure
//! into the project's matching sections — so the manifest stays the
//! complete, readable bill of materials without a documentation hunt.
//!
//! Platform sections are written for the project's TARGET SET, never a
//! guess: every platform the project manifest already mentions (a platform
//! entry or an existing section) — the project said so; the HOST when it
//! mentions none — the one target a fresh project certainly has; and
//! `--platform` extends the set explicitly. An iOS-only project on a macOS
//! host gets the `[ios.dependencies]` closure and no `[macos.*]` at all.
//!
//! Idempotent and surgical: existing entries are left byte-for-byte alone
//! (a differing spec is reported, never overwritten — the manifest is the
//! user's file), comments and formatting survive (`toml_edit`), and running
//! `add` again with another `--platform` fills only the missing sections.

use crate::fetch::Checkout;
use crate::manifest::{is_valid_dep_name, MANIFEST_NAME, PLATFORMS};
use crate::spec::DepSpec;
use crate::store;
use crate::vendor::{self, InstallOptions, VendorError};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

/// What happened to one manifest line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddAction {
    /// Written by this run.
    Added,
    /// Already present with the same spec — untouched.
    Present,
    /// Already present with a DIFFERENT spec — kept, reported.
    KeptDifferent { existing: String },
}

#[derive(Debug, Clone)]
pub struct AddEntry {
    /// `None` = the base `[dependencies]` table.
    pub platform: Option<String>,
    pub name: String,
    pub spec: String,
    pub action: AddAction,
}

#[derive(Debug, Default)]
pub struct AddReport {
    pub entries: Vec<AddEntry>,
    /// The target platform set the closure was expanded for.
    pub platforms: Vec<String>,
}

#[derive(Debug)]
pub enum AddError {
    InvalidName { name: String },
    UnknownPlatform { platform: String },
    Io { path: std::path::PathBuf, source: std::io::Error },
    ManifestSyntax { message: String },
    Vendor(VendorError),
}

/// Package `name`'s manifest text from somewhere on this machine, or `None`
/// when only a fetch can supply it.
///
/// The rungs, in the order `cpc build`'s `vendor_dir_for` walks them:
///
///   1. `<project>/vendor/<name>/`   — what the build links against;
///   2. `<project>/../<name>/`       — the vendor-package self-test case,
///      where sibling packages sit beside the project rather than under a
///      `vendor/` of its own. `cpc pm add . <sibling>` run from inside
///      `vendor/<pkg>` means this one;
///   3. `<store>/<tier>/vendor/<name>/` — the per-user store (D16).
///
/// A rung is taken only when the manifest is actually readable there, so a
/// half-materialized directory falls through to the next rather than failing
/// the command.
fn local_package_manifest(
    project_dir: &Path,
    name: &str,
    options: &InstallOptions,
) -> Option<String> {
    let mut candidates: Vec<std::path::PathBuf> = vec![vendor::vendor_dir(project_dir).join(name)];
    if let Some(parent) = project_dir.parent() {
        candidates.push(parent.join(name));
    }
    if let (Some(root), Some(tc)) = (options.resolved_store_root(), options.toolchain.as_ref()) {
        candidates.push(store::Store::new(root, &tc.version).vendor_dir().join(name));
    }
    candidates
        .into_iter()
        .find_map(|dir| fs::read_to_string(dir.join(MANIFEST_NAME)).ok())
}

/// The host's manifest platform name — the fallback target of a project
/// that declares no platform.
pub fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// Add `name` (at `spec`, default `"*"`) to `<project>/Cplus.toml` with its
/// declared closure, expanded for the project's target set plus
/// `extra_platforms`. Returns what was written; the caller runs `install`
/// afterwards to materialize.
pub fn add(
    project_dir: &Path,
    name: &str,
    spec: Option<&str>,
    extra_platforms: &[String],
    options: &InstallOptions,
) -> Result<AddReport, AddError> {
    if !is_valid_dep_name(name) {
        return Err(AddError::InvalidName {
            name: name.to_string(),
        });
    }
    let manifest_path = project_dir.join(MANIFEST_NAME);
    let text = fs::read_to_string(&manifest_path).map_err(|source| AddError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| AddError::ManifestSyntax {
            message: format!("{e}"),
        })?;

    // The target set (D17): declared platforms, else the host; flags extend.
    let mut platforms: Vec<String> = PLATFORMS
        .iter()
        .filter(|p| doc.get(**p).is_some())
        .map(|s| s.to_string())
        .collect();
    for p in extra_platforms {
        if !PLATFORMS.contains(&p.as_str()) {
            return Err(AddError::UnknownPlatform {
                platform: p.clone(),
            });
        }
        if !platforms.contains(p) {
            platforms.push(p.clone());
        }
    }
    if platforms.is_empty() {
        platforms.push(host_platform().to_string());
    }

    // Read the package's OWN manifest — the closure comes out of it.
    //
    // THE SAME LADDER THE BUILD WALKS, and that is the whole point. `cpc build`
    // resolves a dependency from `<project>/vendor/<name>` first, then a
    // sibling `<project>/../<name>`, then the store; `add` went straight to a
    // git checkout of the toolchain repo at the pinned tag. Two resolvers, two
    // universes — so in a checkout whose `vendor/` is AHEAD of the last release
    // (which the toolchain repo's own tree always is), `add` failed for exactly
    // the packages the build had just compiled:
    //
    //     $ cpc pm add . facet_agent
    //     error: failed to access ~/.cplus/cache/…/v0.0.27/source/vendor/
    //            facet_agent/Cplus.toml: No such file or directory
    //
    // while `cpc build` in the same project resolved `facet_agent` without
    // complaint. The tool whose job is "write the dependency closure into the
    // manifest" was the one that could not see the closure.
    //
    // Fetching stays the LAST rung, not the first: a project that depends on a
    // package it does not have locally still gets it.
    let raw_spec = spec.unwrap_or("*").to_string();
    let dep = vendor::root_pending(name, &raw_spec, options.toolchain.as_ref())
        .map_err(AddError::Vendor)?;
    let store_root = options.resolved_store_root();
    // A PINNED spec names a specific repo, ref and version, and the local rungs
    // cannot honour any of that — a directory called `foo` beside the project
    // is not evidence that it is the `foo` at that URL. So a pin always fetches;
    // the ladder is for the bare and sibling specs, which are exactly the ones
    // that mean "the package this toolchain ships".
    let local_first = !matches!(DepSpec::parse(&raw_spec), Ok(DepSpec::Pinned(_)));
    let pkg_text = match local_first
        .then(|| local_package_manifest(project_dir, name, options))
        .flatten()
    {
        Some(text) => text,
        None => {
            let cache_root = options
                .cache_root
                .clone()
                .or_else(|| store_root.as_ref().map(|r| r.join("cache")))
                .ok_or(AddError::Vendor(VendorError::NoStoreRoot))?;
            let repo_url = options
                .repo_url_override
                .clone()
                .unwrap_or_else(|| dep.repo_url.clone());
            let checkout = Checkout::new(&dep.repo, repo_url, &dep.tag, &cache_root);
            let source_root = checkout
                .ensure()
                .map_err(|e| AddError::Vendor(VendorError::Fetch(e)))?
                .to_path_buf();
            let sha = checkout
                .head_sha()
                .map_err(|e| AddError::Vendor(VendorError::Fetch(e)))?;
            if let Some(root) = &store_root {
                vendor::check_tag_record(root, &dep, &sha).map_err(AddError::Vendor)?;
            }
            let pkg_dir = vendor::join_subpath(&source_root, &dep.subpath);
            fs::read_to_string(pkg_dir.join(MANIFEST_NAME)).map_err(|source| AddError::Io {
                path: pkg_dir.join(MANIFEST_NAME),
                source,
            })?
        }
    };
    let (base_deps, platform_deps) = split_sections(&pkg_text)?;

    // Was the added package itself pinned by URL? Its bare siblings then
    // become pinned URLs at the same repo/ref/version — writing `*` into the
    // project would re-resolve them against the TOOLCHAIN repo, which is a
    // different package entirely.
    let pinned_parent = match DepSpec::parse(&raw_spec) {
        Ok(DepSpec::Pinned(p)) => Some(p),
        _ => None,
    };
    let mut report = AddReport {
        platforms: platforms.clone(),
        ..Default::default()
    };

    let rewrite_kv = |key: &str, value: &str| -> String {
        match (&pinned_parent, DepSpec::parse(value)) {
            (Some(p), Ok(DepSpec::Sibling { .. })) => format!(
                "https://{}/tree/{}/{}@{}",
                p.repo,
                p.git_ref,
                vendor::join_names(&p.sibling_root(), key),
                p.version
            ),
            _ => value.to_string(),
        }
    };

    // The package's own line, then its base closure, then the target
    // platforms' closures.
    upsert(&mut doc, None, name, &raw_spec, &mut report.entries);
    for (dep_name, value) in &base_deps {
        let spec = rewrite_kv(dep_name, value);
        upsert(&mut doc, None, dep_name, &spec, &mut report.entries);
    }
    for platform in &platforms {
        if let Some(deps) = platform_deps.get(platform.as_str()) {
            for (dep_name, value) in deps {
                let spec = rewrite_kv(dep_name, value);
                upsert(
                    &mut doc,
                    Some(platform),
                    dep_name,
                    &spec,
                    &mut report.entries,
                );
            }
        }
    }

    fs::write(&manifest_path, doc.to_string()).map_err(|source| AddError::Io {
        path: manifest_path,
        source,
    })?;
    Ok(report)
}

/// Read a package manifest's dependency sections SEPARATELY (the pm's
/// `Manifest` merges them — add must map platform to platform).
type DepMap = BTreeMap<String, String>;
fn split_sections(text: &str) -> Result<(DepMap, BTreeMap<&'static str, DepMap>), AddError> {
    let value: toml::Value = toml::from_str(text).map_err(|e| AddError::ManifestSyntax {
        message: format!("{e}"),
    })?;
    let table_of = |v: Option<&toml::Value>| -> DepMap {
        v.and_then(|t| t.as_table())
            .map(|t| {
                t.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let base = table_of(value.get("dependencies"));
    let mut per_platform = BTreeMap::new();
    for p in PLATFORMS {
        let deps = table_of(value.get(p).and_then(|t| t.get("dependencies")));
        if !deps.is_empty() {
            per_platform.insert(p, deps);
        }
    }
    Ok((base, per_platform))
}

/// Write one dependency line unless it already exists. Existing entries are
/// never modified — the manifest is the user's file; a differing spec is
/// reported and kept.
fn upsert(
    doc: &mut toml_edit::DocumentMut,
    platform: Option<&str>,
    name: &str,
    spec: &str,
    entries: &mut Vec<AddEntry>,
) {
    use toml_edit::{Item, Table};
    let table: &mut Table = match platform {
        None => {
            if doc.get("dependencies").is_none() {
                doc.insert("dependencies", Item::Table(Table::new()));
            }
            doc["dependencies"].as_table_mut().expect("dependencies is a table")
        }
        Some(p) => {
            if doc.get(p).is_none() {
                let mut t = Table::new();
                // Implicit: render `[macos.dependencies]` without a bare
                // `[macos]` header appearing above it.
                t.set_implicit(true);
                doc.insert(p, Item::Table(t));
            }
            let pt = doc[p].as_table_mut().expect("platform is a table");
            if pt.get("dependencies").is_none() {
                pt.insert("dependencies", Item::Table(Table::new()));
            }
            pt["dependencies"].as_table_mut().expect("platform dependencies is a table")
        }
    };
    let action = match table.get(name).and_then(|i| i.as_str()) {
        Some(existing) if existing == spec => AddAction::Present,
        Some(existing) => AddAction::KeptDifferent {
            existing: existing.to_string(),
        },
        None => {
            table.insert(name, toml_edit::value(spec));
            AddAction::Added
        }
    };
    entries.push(AddEntry {
        platform: platform.map(str::to_string),
        name: name.to_string(),
        spec: spec.to_string(),
        action,
    });
}

impl fmt::Display for AddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddError::InvalidName { name } => write!(
                f,
                "invalid package name `{name}`: a package name must be a lowercase identifier ([a-z][a-z0-9_]*)"
            ),
            AddError::UnknownPlatform { platform } => write!(
                f,
                "unknown platform `{platform}`; one of: {}",
                PLATFORMS.join(", ")
            ),
            AddError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            AddError::ManifestSyntax { message } => {
                write!(f, "manifest is not valid TOML: {message}")
            }
            AddError::Vendor(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AddError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_preserves_comments_and_is_idempotent() {
        let text = "# my app\n[package]\nname = \"app\"  # the name\n\n[dependencies]\n# keep me\nstdlib = \"*\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        let mut entries = Vec::new();
        upsert(&mut doc, None, "facet", "*", &mut entries);
        upsert(&mut doc, None, "stdlib", "*", &mut entries);
        upsert(&mut doc, Some("ios"), "facet_uikit", "*", &mut entries);
        let out = doc.to_string();
        assert!(out.contains("# my app"), "{out}");
        assert!(out.contains("# keep me"), "{out}");
        assert!(out.contains("# the name"), "{out}");
        assert!(out.contains("facet = \"*\""), "{out}");
        assert!(out.contains("[ios.dependencies]"), "{out}");
        assert!(!out.contains("[ios]\n"), "no bare platform header: {out}");
        assert_eq!(entries[0].action, AddAction::Added);
        assert_eq!(entries[1].action, AddAction::Present);
        // A differing spec is kept and reported, never overwritten.
        let mut entries = Vec::new();
        upsert(&mut doc, None, "stdlib", "0.0.9", &mut entries);
        assert!(matches!(
            entries[0].action,
            AddAction::KeptDifferent { ref existing } if existing == "*"
        ));
        assert!(doc.to_string().contains("stdlib = \"*\""));
    }

    #[test]
    fn split_sections_keeps_platforms_separate() {
        let (base, per) = split_sections(
            "[package]\nname = \"ui\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n\n[linux.dependencies]\ngtkish = \"*\"\n",
        )
        .unwrap();
        assert_eq!(base["stdlib"], "*");
        assert_eq!(per["linux"]["gtkish"], "*");
        assert!(per.get("macos").is_none());
    }
}
