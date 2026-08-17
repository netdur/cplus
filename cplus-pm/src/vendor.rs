//! Materialize a project's dependencies — into the per-user store by
//! default, into the project's `vendor/` on request.
//!
//! `install` reads the project manifest and walks the dependency graph
//! breadth-first: for each package it fetches the pinned repo (one cached
//! clone per repo+tag), copies the package subtree to its destination, and
//! then walks that package's own dependencies. Bare-name deps (`stdlib =
//! "*"`) resolve to siblings in the same checkout — or, at the root, to the
//! toolchain's own packages when a [`ToolchainContext`] is supplied (D15).
//!
//! Destination (D16): the store tier (`~/.cplus/<tier>/vendor/<name>`) by
//! default, shared by every project on the machine; `local: true` targets
//! `<project>/vendor/<name>` instead. A pin that disagrees with what the
//! store already holds is vendored into the project instead of thrashing
//! the shared copy — divergence creates locality, agreement shares.
//!
//! There is no version conflict resolution: the first time a name is seen
//! wins (the root manifest is processed first, so the root wins what it
//! names), and a losing request is reported in the install report's
//! warnings (D9), never silently dropped.

use crate::fetch::{Checkout, FetchError};
use crate::manifest::{is_valid_dep_name, Manifest, ManifestError, MANIFEST_NAME};
use crate::spec::{DepSpec, SpecError};
use crate::store::{self, Store, ToolchainContext};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// A provenance stamp written into each vendored package. Line 1 records the
/// pin it was installed from (`<repo>@<version> <subpath>`); line 2 records
/// the commit the release tag resolved to (D8). The pin line is the source
/// of truth for "is this already installed?"; a missing sha line marks a
/// pre-D8 install and triggers one refetch.
const VENDOR_STAMP: &str = ".cplus-vendor";

#[derive(Debug)]
pub enum VendorError {
    Manifest(ManifestError),
    Spec { name: String, source: SpecError },
    /// A bare root dependency with no [`ToolchainContext`] to resolve it.
    RootSiblingDependency { name: String },
    /// Global install (the default) needs the toolchain version to name the
    /// store tier.
    NoToolchainContext,
    /// No store root: no `--store`, no `$CPLUS_HOME`, no home directory.
    NoStoreRoot,
    /// A release tag no longer points at the commit it was first seen at
    /// (D8). Never accommodated silently.
    TagMoved {
        repo: String,
        tag: String,
        recorded: String,
        actual: String,
    },
    Fetch(FetchError),
    MissingPackageDir { name: String, path: PathBuf },
    Io { path: PathBuf, source: std::io::Error },
    NotInstalled { name: String },
    InvalidName { name: String },
}

/// Where a package landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// The shared store tier: `~/.cplus/<tier>/vendor/<name>`.
    Store,
    /// The project's own `vendor/<name>` (`--local`, or a divergent pin).
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub repo: String,
    pub version: String,
    /// `true` if fetched and copied this run; `false` if already present at
    /// this pin and left untouched.
    pub fresh: bool,
    pub location: Location,
}

/// What an install did: the packages, plus everything worth saying out loud
/// (losing version requests, divergent pins vendored locally).
#[derive(Debug, Default)]
pub struct InstallReport {
    pub packages: Vec<Resolved>,
    pub warnings: Vec<String>,
}

/// One package waiting to be vendored, with everything needed to locate it.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub(crate) name: String,
    pub(crate) repo: String,
    pub(crate) repo_url: String,
    pub(crate) tag: String,
    pub(crate) version: String,
    /// Package directory within the checkout, e.g. `vendor/stdlib`.
    pub(crate) subpath: String,
    /// Directory holding this package's siblings, e.g. `vendor`.
    pub(crate) sibling_root: String,
    /// Who asked for this pin — for the D9 conflict warning.
    pub(crate) declared_by: String,
}

/// Where to install to and fetch from.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Store root override. Default: `$CPLUS_HOME`, else `~/.cplus`.
    pub store_root: Option<PathBuf>,
    /// Clone cache override. Default: `<store root>/cache`.
    pub cache_root: Option<PathBuf>,
    /// Override every clone URL (e.g. a local repo path for offline installs).
    pub repo_url_override: Option<String>,
    /// The running toolchain's identity — supplied by `cpc pm`, or by flags
    /// on the standalone binary. Names the store tier and resolves bare
    /// root deps.
    pub toolchain: Option<ToolchainContext>,
    /// Install into `<project>/vendor/` instead of the store (D16).
    pub local: bool,
}

impl InstallOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn resolved_store_root(&self) -> Option<PathBuf> {
        self.store_root.clone().or_else(store::default_root)
    }
}

pub fn vendor_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("vendor")
}

/// Resolve `<project>/Cplus.toml`'s dependencies (transitively) and
/// materialize them — in the store tier by default, in `<project>/vendor/`
/// with `local`. Incremental: a package already present at the pinned
/// version (stamp match) is left untouched; its transitive deps are still
/// walked so a missing sibling is discovered.
pub fn install(
    project_dir: &Path,
    options: &InstallOptions,
) -> Result<InstallReport, VendorError> {
    let manifest = Manifest::load_dir(project_dir).map_err(VendorError::Manifest)?;
    let mut report = InstallReport::default();
    if manifest.deps.is_empty() {
        return Ok(report);
    }

    let ctx = options.toolchain.as_ref();
    let store_root = options.resolved_store_root();
    // Global install writes into a tier named after the toolchain version;
    // without the context there is no tier to write into.
    let global_store: Option<Store> = if options.local {
        None
    } else {
        let ctx = ctx.ok_or(VendorError::NoToolchainContext)?;
        let root = store_root.clone().ok_or(VendorError::NoStoreRoot)?;
        Some(Store::new(root, &ctx.version))
    };
    let cache_root = options
        .cache_root
        .clone()
        .or_else(|| store_root.as_ref().map(|r| r.join("cache")))
        .ok_or(VendorError::NoStoreRoot)?;
    let local_vendor = vendor_dir(project_dir);

    let mut queue: VecDeque<Pending> = VecDeque::new();
    for (name, value) in &manifest.deps {
        queue.push_back(root_pending(name, value, ctx)?);
    }

    // First seen wins; later different pins are reported, not resolved (D9).
    let mut winners: HashMap<String, Pending> = HashMap::new();

    while let Some(dep) = queue.pop_front() {
        if let Some(winner) = winners.get(&dep.name) {
            if (&winner.repo, &winner.version, &winner.subpath)
                != (&dep.repo, &dep.version, &dep.subpath)
            {
                report.warnings.push(format!(
                    "{}: installed {}@{} (via {}); {} wanted {}@{}",
                    dep.name,
                    winner.repo,
                    winner.version,
                    winner.declared_by,
                    dep.declared_by,
                    dep.repo,
                    dep.version,
                ));
            }
            continue;
        }

        let pin = stamp_pin(&dep);

        // Destination: the store unless `local` was asked for — or unless
        // the store already holds this name from a DIFFERENT pin, in which
        // case this project's copy goes local and the shared one is left
        // alone (D16).
        let (dest, location) = match &global_store {
            None => (local_vendor.join(&dep.name), Location::Local),
            Some(store) => {
                let store_dest = store.vendor_dir().join(&dep.name);
                match read_stamp(&store_dest) {
                    Some((stored_pin, _)) if stored_pin != pin => {
                        report.warnings.push(format!(
                            "{}: store has {} but this project pins {}@{} — vendoring locally into {}",
                            dep.name,
                            stored_pin,
                            dep.repo,
                            dep.version,
                            local_vendor.join(&dep.name).display(),
                        ));
                        (local_vendor.join(&dep.name), Location::Local)
                    }
                    _ => (store_dest, Location::Store),
                }
            }
        };

        // Present at this exact pin (sha recorded, manifest loads)? Leave it
        // byte-for-byte; still walk its deps below. A missing sha line is a
        // pre-D8 stamp: reinstall once to record provenance.
        let present = match read_stamp(&dest) {
            Some((stored_pin, Some(_))) if stored_pin == pin => Manifest::load_dir(&dest).ok(),
            _ => None,
        };

        let sub_manifest = if let Some(m) = present {
            report.packages.push(Resolved {
                name: dep.name.clone(),
                repo: dep.repo.clone(),
                version: dep.version.clone(),
                fresh: false,
                location,
            });
            m
        } else {
            let repo_url = options
                .repo_url_override
                .clone()
                .unwrap_or_else(|| dep.repo_url.clone());
            let checkout = Checkout::new(&dep.repo, repo_url, &dep.tag, &cache_root);
            let source_root = checkout.ensure().map_err(VendorError::Fetch)?.to_path_buf();
            let sha = checkout.head_sha().map_err(VendorError::Fetch)?;
            // D8: a release tag is immutable. The first time a tag is seen
            // its commit is recorded (beside the tiers, so a purged cache
            // does not forget); a later fetch that resolves differently is
            // an incident, never accommodated.
            if let Some(root) = &store_root {
                check_tag_record(root, &dep, &sha)?;
            }

            let package_src = join_subpath(&source_root, &dep.subpath);
            if !package_src.join(MANIFEST_NAME).is_file() {
                return Err(VendorError::MissingPackageDir {
                    name: dep.name.clone(),
                    path: package_src,
                });
            }

            if dest.exists() {
                fs::remove_dir_all(&dest).map_err(|source| VendorError::Io {
                    path: dest.clone(),
                    source,
                })?;
            }
            copy_tree(&package_src, &dest)?;
            write_stamp(&dest, &pin, &sha)?;

            report.packages.push(Resolved {
                name: dep.name.clone(),
                repo: dep.repo.clone(),
                version: dep.version.clone(),
                fresh: true,
                location,
            });
            Manifest::load_dir(&dest).map_err(VendorError::Manifest)?
        };

        winners.insert(dep.name.clone(), dep.clone());

        // Walk the package's own dependencies (present or fresh alike).
        // Duplicates are queued anyway: the pop-side check is what compares
        // pins and reports a losing request instead of hiding it.
        for (name, value) in &sub_manifest.deps {
            queue.push_back(transitive_pending(name, value, &dep)?);
        }
    }

    report.packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(report)
}

/// Remove a package's directory from `<project>/vendor/`. The store is
/// shared across projects and is not touched by remove.
pub fn remove(project_dir: &Path, name: &str) -> Result<(), VendorError> {
    // `name` comes straight from the CLI and is joined onto `vendor/`, so it
    // must be a single, contained path component — never `../..` or an
    // absolute path that would delete outside `vendor/`.
    if !is_valid_dep_name(name) {
        return Err(VendorError::InvalidName {
            name: name.to_string(),
        });
    }
    let dest = vendor_dir(project_dir).join(name);
    if !dest.exists() {
        return Err(VendorError::NotInstalled {
            name: name.to_string(),
        });
    }
    fs::remove_dir_all(&dest).map_err(|source| VendorError::Io { path: dest, source })?;
    Ok(())
}

/// A project's direct dependency: a pinned tree-URL, or — with a toolchain
/// context — a bare name resolved to the toolchain's own packages at the
/// toolchain's version (D15). Without context a bare root dep has no repo.
pub(crate) fn root_pending(
    name: &str,
    value: &str,
    ctx: Option<&ToolchainContext>,
) -> Result<Pending, VendorError> {
    match DepSpec::parse(value).map_err(|source| VendorError::Spec {
        name: name.to_string(),
        source,
    })? {
        DepSpec::Pinned(p) => Ok(Pending {
            name: name.to_string(),
            repo: p.repo.clone(),
            repo_url: p.repo_url(),
            tag: p.tag(),
            version: p.version.clone(),
            sibling_root: p.sibling_root(),
            subpath: p.subpath,
            declared_by: "the root manifest".to_string(),
        }),
        DepSpec::Sibling { version } => match ctx {
            Some(ctx) => {
                // `*` means the toolchain's version; an explicit bare
                // version is an explicit pin within the toolchain repo.
                let version = version.unwrap_or_else(|| ctx.version.clone());
                Ok(Pending {
                    name: name.to_string(),
                    repo: ctx.repo.clone(),
                    repo_url: format!("https://{}.git", ctx.repo),
                    tag: format!("v{version}"),
                    version,
                    subpath: join_names(&ctx.package_root, name),
                    sibling_root: ctx.package_root.clone(),
                    declared_by: "the root manifest".to_string(),
                })
            }
            None => Err(VendorError::RootSiblingDependency {
                name: name.to_string(),
            }),
        },
    }
}

/// A dependency declared *inside* an already-resolved package. A sibling
/// inherits the parent's repo/tag and lives beside it; a pinned URL fetches
/// from wherever it points.
fn transitive_pending(name: &str, value: &str, parent: &Pending) -> Result<Pending, VendorError> {
    let declared_by = format!("package `{}`", parent.name);
    match DepSpec::parse(value).map_err(|source| VendorError::Spec {
        name: name.to_string(),
        source,
    })? {
        DepSpec::Sibling { .. } => Ok(Pending {
            name: name.to_string(),
            repo: parent.repo.clone(),
            repo_url: parent.repo_url.clone(),
            tag: parent.tag.clone(),
            version: parent.version.clone(),
            subpath: join_names(&parent.sibling_root, name),
            sibling_root: parent.sibling_root.clone(),
            declared_by,
        }),
        DepSpec::Pinned(p) => Ok(Pending {
            name: name.to_string(),
            repo: p.repo.clone(),
            repo_url: p.repo_url(),
            tag: p.tag(),
            version: p.version.clone(),
            sibling_root: p.sibling_root(),
            subpath: p.subpath,
            declared_by,
        }),
    }
}

pub(crate) fn join_names(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn join_subpath(root: &Path, subpath: &str) -> PathBuf {
    if subpath.is_empty() {
        root.to_path_buf()
    } else {
        root.join(subpath)
    }
}

/// The pin line identifying what a package was installed from.
fn stamp_pin(dep: &Pending) -> String {
    format!("{}@{} {}", dep.repo, dep.version, dep.subpath)
        .trim_end()
        .to_string()
}

/// Read a stamp: `(pin line, commit sha)`. The sha is `None` for stamps
/// written before D8 — which is exactly what forces their one refetch.
fn read_stamp(dest: &Path) -> Option<(String, Option<String>)> {
    let text = fs::read_to_string(dest.join(VENDOR_STAMP)).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let pin = lines.next()?.to_string();
    let sha = lines.next().map(str::to_string);
    Some((pin, sha))
}

fn write_stamp(dest: &Path, pin: &str, sha: &str) -> Result<(), VendorError> {
    let path = dest.join(VENDOR_STAMP);
    fs::write(&path, format!("{pin}\n{sha}\n"))
        .map_err(|source| VendorError::Io { path, source })
}

/// D8: compare a fresh checkout's commit against the first-seen record for
/// its tag; record it on first sight. The record lives under the store root
/// (`<root>/tags/`), surviving cache deletion.
pub(crate) fn check_tag_record(store_root: &Path, dep: &Pending, sha: &str) -> Result<(), VendorError> {
    let path = store::tag_record(store_root, &dep.repo, &dep.tag);
    match fs::read_to_string(&path) {
        Ok(recorded) => {
            let recorded = recorded.trim();
            if recorded != sha {
                return Err(VendorError::TagMoved {
                    repo: dep.repo.clone(),
                    tag: dep.tag.clone(),
                    recorded: recorded.to_string(),
                    actual: sha.to_string(),
                });
            }
            Ok(())
        }
        Err(_) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| VendorError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&path, format!("{sha}\n"))
                .map_err(|source| VendorError::Io { path, source })
        }
    }
}

/// Copy a directory tree from `src` into `dest`, skipping any `.git`.
fn copy_tree(src: &Path, dest: &Path) -> Result<(), VendorError> {
    fs::create_dir_all(dest).map_err(|source| VendorError::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    let entries = fs::read_dir(src).map_err(|source| VendorError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| VendorError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        if entry.file_name() == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source| VendorError::Io {
            path: from.clone(),
            source,
        })?;
        // Skip symlinks: a fetched package must not be able to pull an external
        // file into `vendor/` (following a link to `/etc/...`) or redirect the
        // copy walk out of its own tree via a symlinked directory.
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|source| VendorError::Io {
                path: from.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

impl fmt::Display for VendorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VendorError::Manifest(source) => source.fmt(f),
            VendorError::Spec { source, .. } => source.fmt(f),
            VendorError::RootSiblingDependency { name } => write!(
                f,
                "dependency `{name}` has no source URL; pin it as `…/tree/<ref>/<path>@<version>`, or supply the toolchain context (`cpc pm` does automatically; the standalone binary takes --toolchain-repo/--toolchain-version)"
            ),
            VendorError::NoToolchainContext => write!(
                f,
                "global install needs the toolchain version to name the store tier; run through `cpc pm`, pass --toolchain-version, or install with --local"
            ),
            VendorError::NoStoreRoot => write!(
                f,
                "no store location: pass --store (or --cache), or set $CPLUS_HOME / $HOME"
            ),
            VendorError::TagMoved {
                repo,
                tag,
                recorded,
                actual,
            } => write!(
                f,
                "tag `{tag}` of {repo} moved from {recorded} to {actual}; a release tag is immutable — if this is deliberate, delete the record under the store's tags/ directory and reinstall"
            ),
            VendorError::Fetch(source) => source.fmt(f),
            VendorError::MissingPackageDir { name, path } => write!(
                f,
                "package `{name}` was not found in the fetched repo at {}",
                path.display()
            ),
            VendorError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            VendorError::NotInstalled { name } => {
                write!(f, "`{name}` is not installed in vendor/")
            }
            VendorError::InvalidName { name } => write!(
                f,
                "invalid package name `{name}`: a package name must be a lowercase identifier ([a-z][a-z0-9_]*)"
            ),
        }
    }
}

impl std::error::Error for VendorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    // Build a throwaway git repo laid out like the C+ monorepo (packages under
    // vendor/), tag it, and install a consumer project against it via a local
    // repo-url override — proving the fetch/copy/transitive walk end to end
    // without touching the network.
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

    fn monorepo(dir: &Path) {
        git(dir, &["init", "-q"]);
        write(
            &dir.join("vendor/stdlib/Cplus.toml"),
            "[package]\nname = \"stdlib\"\nversion = \"0.0.26\"\n",
        );
        write(&dir.join("vendor/stdlib/src/lib/io.cplus"), "// io\n");
        write(
            &dir.join("vendor/objc/Cplus.toml"),
            "[package]\nname = \"objc\"\nversion = \"0.0.26\"\n",
        );
        write(
            &dir.join("vendor/appkit/Cplus.toml"),
            "[package]\nname = \"appkit\"\nversion = \"0.0.26\"\n\n[dependencies]\nstdlib = \"*\"\nobjc = \"*\"\n",
        );
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", "v0.0.26"]);
        git(dir, &["tag", "v0.0.26"]);
    }

    fn ctx() -> ToolchainContext {
        ToolchainContext {
            repo: "github.com/netdur/cplus".to_string(),
            version: "0.0.26".to_string(),
            package_root: "vendor".to_string(),
        }
    }

    /// Options wired to a throwaway store and a local fixture repo.
    fn options(repo: &Path, store: &Path) -> InstallOptions {
        InstallOptions {
            store_root: Some(store.to_path_buf()),
            cache_root: None,
            repo_url_override: Some(repo.to_string_lossy().into_owned()),
            toolchain: Some(ctx()),
            local: false,
        }
    }

    // A project pinning appkit (which pulls stdlib + objc), a fixture repo,
    // and a fresh store.
    fn appkit_project() -> (TempDir, TempDir, TempDir, InstallOptions) {
        let repo = TempDir::new().unwrap();
        monorepo(repo.path());
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nappkit = \"https://github.com/netdur/cplus/tree/main/vendor/appkit@0.0.26\"\n",
        );
        let store = TempDir::new().unwrap();
        let options = options(repo.path(), store.path());
        (repo, project, store, options)
    }

    fn store_vendor(store: &TempDir) -> PathBuf {
        store.path().join("v0.0.26").join("vendor")
    }

    #[test]
    fn global_install_lands_in_the_store_not_the_project() {
        let (_repo, project, store, options) = appkit_project();

        let report = install(project.path(), &options).unwrap();
        let names: Vec<&str> = report.packages.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["appkit", "objc", "stdlib"]);
        assert!(report.packages.iter().all(|i| i.fresh));
        assert!(report
            .packages
            .iter()
            .all(|i| i.location == Location::Store));
        assert!(report.warnings.is_empty());

        // Everything is in the store tier; the project grew no vendor/.
        let sv = store_vendor(&store);
        assert!(sv.join("appkit/Cplus.toml").is_file());
        assert!(sv.join("objc/Cplus.toml").is_file());
        assert!(sv.join("stdlib/src/lib/io.cplus").is_file());
        assert!(!project.path().join("vendor").exists());
        // The stamp records pin + commit.
        let (pin, sha) = read_stamp(&sv.join("stdlib")).unwrap();
        assert_eq!(pin, "github.com/netdur/cplus@0.0.26 vendor/stdlib");
        assert_eq!(sha.unwrap().len(), 40);
    }

    #[test]
    fn local_install_lands_in_the_project() {
        let (_repo, project, store, mut options) = appkit_project();
        options.local = true;

        let report = install(project.path(), &options).unwrap();
        assert!(report
            .packages
            .iter()
            .all(|i| i.location == Location::Local));
        assert!(project.path().join("vendor/appkit/Cplus.toml").is_file());
        assert!(project.path().join("vendor/stdlib/Cplus.toml").is_file());
        assert!(!store_vendor(&store).join("appkit").exists());
    }

    #[test]
    fn bare_root_dep_resolves_through_the_toolchain_context() {
        // D15: `stdlib = "*"` at the root = the toolchain's package at the
        // toolchain's version.
        let repo = TempDir::new().unwrap();
        monorepo(repo.path());
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n",
        );
        let store = TempDir::new().unwrap();
        let options = options(repo.path(), store.path());

        let report = install(project.path(), &options).unwrap();
        assert_eq!(report.packages.len(), 1);
        assert_eq!(report.packages[0].version, "0.0.26");
        assert!(store_vendor(&store)
            .join("stdlib/src/lib/io.cplus")
            .is_file());
    }

    #[test]
    fn bare_root_dep_without_context_is_rejected() {
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n",
        );
        let store = TempDir::new().unwrap();
        let options = InstallOptions {
            store_root: Some(store.path().to_path_buf()),
            local: true, // isolate the bare-dep error from NoToolchainContext
            ..InstallOptions::default()
        };
        let error = install(project.path(), &options).unwrap_err();
        assert!(matches!(error, VendorError::RootSiblingDependency { .. }));
    }

    #[test]
    fn global_install_without_context_is_rejected() {
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26\"\n",
        );
        let store = TempDir::new().unwrap();
        let options = InstallOptions {
            store_root: Some(store.path().to_path_buf()),
            ..InstallOptions::default()
        };
        let error = install(project.path(), &options).unwrap_err();
        assert!(matches!(error, VendorError::NoToolchainContext));
    }

    #[test]
    fn remove_rejects_a_traversal_or_absolute_name() {
        // `remove NAME` joins NAME onto vendor/, so a traversal/absolute name
        // must be rejected before any deletion (it never reaches remove_dir_all).
        let project = TempDir::new().unwrap();
        for bad in ["../../etc", "/tmp", "a/b", ".."] {
            let err = remove(project.path(), bad).unwrap_err();
            assert!(
                matches!(err, VendorError::InvalidName { .. }),
                "`{bad}` should be rejected"
            );
        }
    }

    #[test]
    fn already_present_store_packages_are_left_untouched() {
        let (_repo, project, store, options) = appkit_project();
        install(project.path(), &options).unwrap();

        // A second project sharing the store refetches nothing.
        let project2 = TempDir::new().unwrap();
        write(
            &project2.path().join("Cplus.toml"),
            "[package]\nname = \"app2\"\nversion = \"0.0.1\"\n\n[dependencies]\nappkit = \"https://github.com/netdur/cplus/tree/main/vendor/appkit@0.0.26\"\n",
        );
        let marker = store_vendor(&store).join("stdlib/LOCAL.txt");
        fs::write(&marker, "edited").unwrap();

        let again = install(project2.path(), &options).unwrap();
        assert!(
            again.packages.iter().all(|r| !r.fresh),
            "nothing should be refetched"
        );
        assert!(marker.is_file(), "up-to-date package was wiped");
    }

    #[test]
    fn a_missing_package_is_reinstalled_others_untouched() {
        let (_repo, project, store, options) = appkit_project();
        install(project.path(), &options).unwrap();

        let sv = store_vendor(&store);
        let marker = sv.join("stdlib/LOCAL.txt");
        fs::write(&marker, "edited").unwrap();
        fs::remove_dir_all(sv.join("objc")).unwrap();

        let again = install(project.path(), &options).unwrap();
        let fresh: Vec<&str> = again
            .packages
            .iter()
            .filter(|r| r.fresh)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(fresh, vec!["objc"]);
        assert!(sv.join("objc/Cplus.toml").is_file());
        assert!(marker.is_file(), "stdlib should have been left alone");
    }

    #[test]
    fn a_missing_or_sha_less_stamp_triggers_reinstall() {
        let (_repo, project, store, options) = appkit_project();
        install(project.path(), &options).unwrap();
        let sv = store_vendor(&store);

        // No stamp at all (a hand-copied dir).
        fs::remove_file(sv.join("stdlib").join(VENDOR_STAMP)).unwrap();
        // A pre-D8 stamp: pin line only, no recorded commit.
        let (pin, _) = read_stamp(&sv.join("objc")).unwrap();
        fs::write(sv.join("objc").join(VENDOR_STAMP), format!("{pin}\n")).unwrap();

        let again = install(project.path(), &options).unwrap();
        let mut fresh: Vec<&str> = again
            .packages
            .iter()
            .filter(|r| r.fresh)
            .map(|r| r.name.as_str())
            .collect();
        fresh.sort();
        assert_eq!(fresh, vec!["objc", "stdlib"]);
        // The migration reinstall recorded the sha.
        assert!(read_stamp(&sv.join("objc")).unwrap().1.is_some());
    }

    #[test]
    fn a_divergent_pin_goes_local_and_leaves_the_store_alone() {
        // Two projects, same store, different stdlib pins: the store keeps
        // the first, the second project gets a local vendor copy (D16).
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q"]);
        write(
            &repo.path().join("vendor/stdlib/Cplus.toml"),
            "[package]\nname = \"stdlib\"\nversion = \"0.0.0\"\n",
        );
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v26\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r26"]);
        git(repo.path(), &["tag", "v0.0.26"]);
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v27\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r27"]);
        git(repo.path(), &["tag", "v0.0.27"]);

        let store = TempDir::new().unwrap();
        let manifest = |v: &str| {
            format!(
                "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@{v}\"\n"
            )
        };
        let a = TempDir::new().unwrap();
        write(&a.path().join("Cplus.toml"), &manifest("0.0.26"));
        let b = TempDir::new().unwrap();
        write(&b.path().join("Cplus.toml"), &manifest("0.0.27"));
        let opts = options(repo.path(), store.path());

        install(a.path(), &opts).unwrap();
        let report = install(b.path(), &opts).unwrap();

        let sv = store_vendor(&store);
        assert_eq!(
            fs::read_to_string(sv.join("stdlib/src/lib/io.cplus")).unwrap(),
            "// v26\n",
            "the store keeps the first-installed pin"
        );
        let entry = &report.packages[0];
        assert_eq!(entry.location, Location::Local);
        assert_eq!(
            fs::read_to_string(b.path().join("vendor/stdlib/src/lib/io.cplus")).unwrap(),
            "// v27\n"
        );
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("vendoring locally"));
    }

    #[test]
    fn a_moved_tag_is_a_hard_error() {
        // D8: the store remembers what v0.0.26 pointed at; re-tagging and
        // clearing the cache must be detected, not silently absorbed.
        let (repo, project, store, options) = appkit_project();
        install(project.path(), &options).unwrap();

        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// evil\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "retag"]);
        git(repo.path(), &["tag", "-f", "v0.0.26"]);

        // Force a refetch: drop the cache and one store package.
        fs::remove_dir_all(store.path().join("cache")).unwrap();
        fs::remove_dir_all(store_vendor(&store).join("stdlib")).unwrap();

        let err = install(project.path(), &options).unwrap_err();
        assert!(
            matches!(err, VendorError::TagMoved { ref tag, .. } if tag == "v0.0.26"),
            "got: {err:?}"
        );
    }

    #[test]
    fn a_losing_version_request_is_reported() {
        // D9: root wins what it names; the transitive pin that lost is
        // named in the warnings, and install continues.
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q"]);
        write(
            &repo.path().join("vendor/stdlib/Cplus.toml"),
            "[package]\nname = \"stdlib\"\nversion = \"0.0.26\"\n",
        );
        write(
            &repo.path().join("vendor/appkit/Cplus.toml"),
            "[package]\nname = \"appkit\"\nversion = \"0.0.26\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@9.9.9\"\n",
        );
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "v0.0.26"]);
        git(repo.path(), &["tag", "v0.0.26"]);

        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nappkit = \"*\"\nstdlib = \"*\"\n",
        );
        let store = TempDir::new().unwrap();
        let opts = options(repo.path(), store.path());

        let report = install(project.path(), &opts).unwrap();
        assert_eq!(report.packages.len(), 2, "install completed");
        assert_eq!(report.warnings.len(), 1);
        assert!(
            report.warnings[0].contains("stdlib")
                && report.warnings[0].contains("9.9.9")
                && report.warnings[0].contains("appkit"),
            "warning names the loser: {}",
            report.warnings[0]
        );
        // The winner (the root's 0.0.26) is what's installed.
        let (pin, _) = read_stamp(&store_vendor(&store).join("stdlib")).unwrap();
        assert!(pin.contains("@0.0.26"));
    }

    #[test]
    fn changing_the_pinned_version_refetches_that_version() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q"]);
        write(
            &repo.path().join("vendor/stdlib/Cplus.toml"),
            "[package]\nname = \"stdlib\"\nversion = \"0.0.0\"\n",
        );
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v26\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r26"]);
        git(repo.path(), &["tag", "v0.0.26"]);
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v27\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r27"]);
        git(repo.path(), &["tag", "v0.0.27"]);

        // Local mode: the pin change must replace the project's copy.
        let project = TempDir::new().unwrap();
        let manifest = |v: &str| {
            format!(
                "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@{v}\"\n"
            )
        };
        write(&project.path().join("Cplus.toml"), &manifest("0.0.26"));
        let store = TempDir::new().unwrap();
        let mut opts = options(repo.path(), store.path());
        opts.local = true;

        install(project.path(), &opts).unwrap();
        let io = project.path().join("vendor/stdlib/src/lib/io.cplus");
        assert_eq!(fs::read_to_string(&io).unwrap(), "// v26\n");

        write(&project.path().join("Cplus.toml"), &manifest("0.0.27"));
        let again = install(project.path(), &opts).unwrap();
        assert!(again.packages[0].fresh);
        assert_eq!(fs::read_to_string(&io).unwrap(), "// v27\n");
    }
}
