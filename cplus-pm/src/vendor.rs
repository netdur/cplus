//! Populate a project's `vendor/` from its `Cplus.toml`.
//!
//! `install` reads the project manifest, and for each `[dependencies]` entry
//! fetches the pinned repo, copies the named package's subtree into
//! `vendor/<name>/`, then walks that package's own dependencies transitively.
//! Bare-name deps (`stdlib = "*"`) resolve to siblings in the same checkout, so
//! a whole monorepo's worth of packages materializes from one clone. Placement
//! is keyed by the dependency name (the `[dependencies]` key), which is exactly
//! what `cpc build` looks up under `vendor/<name>/`.
//!
//! There is no version conflict resolution: the first time a name is seen wins,
//! and everything in a monorepo is pinned to the same tag anyway.

use crate::fetch::{Checkout, FetchError};
use crate::manifest::{Manifest, ManifestError, MANIFEST_NAME};
use crate::spec::{DepSpec, SpecError};
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// A provenance stamp written into each vendored package recording the pin it
/// was installed from (`<repo>@<version> <subpath>`). It is the source of truth
/// for "is this already installed?": a package's own `[package].version` is its
/// independent version, not the git-tag pin, so we cannot compare against that.
const VENDOR_STAMP: &str = ".cplus-vendor";

#[derive(Debug)]
pub enum VendorError {
    Manifest(ManifestError),
    Spec { name: String, source: SpecError },
    RootSiblingDependency { name: String },
    Fetch(FetchError),
    MissingPackageDir { name: String, path: PathBuf },
    Io { path: PathBuf, source: std::io::Error },
    NotInstalled { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub repo: String,
    pub version: String,
    /// `true` if fetched and copied this run; `false` if it was already present
    /// in `vendor/` at this version and left untouched.
    pub fresh: bool,
}

/// One package waiting to be vendored, with everything needed to locate it.
#[derive(Debug, Clone)]
struct Pending {
    name: String,
    repo: String,
    repo_url: String,
    tag: String,
    version: String,
    /// Package directory within the checkout, e.g. `vendor/stdlib`.
    subpath: String,
    /// Directory holding this package's siblings, e.g. `vendor`.
    sibling_root: String,
}

/// Where to fetch from and cache to.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub cache_root: PathBuf,
    /// Override every clone URL (e.g. a local repo path for offline installs).
    pub repo_url_override: Option<String>,
}

impl InstallOptions {
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            repo_url_override: None,
        }
    }
}

pub fn vendor_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("vendor")
}

/// Resolve `<project>/Cplus.toml`'s dependencies (transitively) and make
/// `<project>/vendor/` match the manifest. Incremental: a package already
/// present at the pinned version is left untouched; only missing packages (and
/// ones whose installed version differs from the pin) are fetched and copied.
/// Transitive dependencies are walked even under already-present packages, so a
/// missing sibling is still discovered and installed.
pub fn install(
    project_dir: &Path,
    options: &InstallOptions,
) -> Result<Vec<Resolved>, VendorError> {
    let manifest = Manifest::load_dir(project_dir).map_err(VendorError::Manifest)?;
    let vendor = vendor_dir(project_dir);

    let mut queue: VecDeque<Pending> = VecDeque::new();
    for (name, value) in &manifest.deps {
        queue.push_back(root_pending(name, value)?);
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut resolved: Vec<Resolved> = Vec::new();

    while let Some(dep) = queue.pop_front() {
        if !seen.insert(dep.name.clone()) {
            continue;
        }

        let dest = vendor.join(&dep.name);

        // Already installed from this exact pin? Keep it as-is; we still read
        // its manifest below to walk transitive deps. Otherwise fetch & copy.
        // A missing package (no stamp), or one installed from a different pin
        // (the `@version` changed), fails the match and is (re)installed.
        let stamp = stamp_line(&dep);
        let present = if read_stamp(&dest).as_deref() == Some(stamp.as_str()) {
            Manifest::load_dir(&dest).ok()
        } else {
            None
        };

        let sub_manifest = if let Some(manifest) = present {
            resolved.push(Resolved {
                name: dep.name.clone(),
                repo: dep.repo.clone(),
                version: dep.version.clone(),
                fresh: false,
            });
            manifest
        } else {
            let repo_url = options
                .repo_url_override
                .clone()
                .unwrap_or_else(|| dep.repo_url.clone());
            let checkout = Checkout::new(&dep.repo, repo_url, &dep.tag, &options.cache_root);
            let source_root = checkout.ensure().map_err(VendorError::Fetch)?;

            let package_src = join_subpath(source_root, &dep.subpath);
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
            write_stamp(&dest, &stamp)?;

            resolved.push(Resolved {
                name: dep.name.clone(),
                repo: dep.repo.clone(),
                version: dep.version.clone(),
                fresh: true,
            });
            Manifest::load_dir(&dest).map_err(VendorError::Manifest)?
        };

        // Walk the package's own dependencies (present or freshly installed).
        for (name, value) in &sub_manifest.deps {
            if seen.contains(name) {
                continue;
            }
            queue.push_back(transitive_pending(name, value, &dep)?);
        }
    }

    resolved.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(resolved)
}

/// Remove a package's directory from `<project>/vendor/`.
pub fn remove(project_dir: &Path, name: &str) -> Result<(), VendorError> {
    let dest = vendor_dir(project_dir).join(name);
    if !dest.exists() {
        return Err(VendorError::NotInstalled {
            name: name.to_string(),
        });
    }
    fs::remove_dir_all(&dest).map_err(|source| VendorError::Io { path: dest, source })?;
    Ok(())
}

/// A project's direct dependency: must be a pinned tree-URL (it names where the
/// package comes from). A bare sibling has no repo to fetch from at the root.
fn root_pending(name: &str, value: &str) -> Result<Pending, VendorError> {
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
        }),
        DepSpec::Sibling { .. } => Err(VendorError::RootSiblingDependency {
            name: name.to_string(),
        }),
    }
}

/// A dependency declared *inside* an already-resolved package. A sibling
/// inherits the parent's repo/tag and lives beside it; a pinned URL fetches
/// from wherever it points.
fn transitive_pending(name: &str, value: &str, parent: &Pending) -> Result<Pending, VendorError> {
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
        }),
        DepSpec::Pinned(p) => Ok(Pending {
            name: name.to_string(),
            repo: p.repo.clone(),
            repo_url: p.repo_url(),
            tag: p.tag(),
            version: p.version.clone(),
            sibling_root: p.sibling_root(),
            subpath: p.subpath,
        }),
    }
}

fn join_names(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn join_subpath(root: &Path, subpath: &str) -> PathBuf {
    if subpath.is_empty() {
        root.to_path_buf()
    } else {
        root.join(subpath)
    }
}

/// The provenance line identifying the pin a package was installed from.
fn stamp_line(dep: &Pending) -> String {
    format!("{}@{} {}", dep.repo, dep.version, dep.subpath)
}

fn read_stamp(dest: &Path) -> Option<String> {
    fs::read_to_string(dest.join(VENDOR_STAMP))
        .ok()
        .map(|s| s.trim().to_string())
}

fn write_stamp(dest: &Path, stamp: &str) -> Result<(), VendorError> {
    let path = dest.join(VENDOR_STAMP);
    fs::write(&path, format!("{stamp}\n")).map_err(|source| VendorError::Io { path, source })
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
                "dependency `{name}` has no source URL; a project's direct dependency must be pinned as `…/tree/<ref>/<path>@<version>`"
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

    #[test]
    fn installs_a_pinned_dep_and_its_transitive_siblings() {
        let repo = TempDir::new().unwrap();
        monorepo(repo.path());
        let repo_url = repo.path().to_string_lossy().into_owned();

        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nappkit = \"https://github.com/netdur/cplus/tree/main/vendor/appkit@0.0.26\"\n",
        );

        let cache = TempDir::new().unwrap();
        let mut options = InstallOptions::new(cache.path());
        options.repo_url_override = Some(repo_url);

        let installed = install(project.path(), &options).unwrap();
        let names: Vec<&str> = installed.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["appkit", "objc", "stdlib"]);
        // First run: every package was freshly fetched.
        assert!(installed.iter().all(|i| i.fresh));

        // appkit pulled in stdlib + objc; the leaf file came across too.
        assert!(project.path().join("vendor/appkit/Cplus.toml").is_file());
        assert!(project.path().join("vendor/objc/Cplus.toml").is_file());
        assert!(project
            .path()
            .join("vendor/stdlib/src/lib/io.cplus")
            .is_file());
    }

    #[test]
    fn root_sibling_dependency_is_rejected() {
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"*\"\n",
        );
        let cache = TempDir::new().unwrap();
        let error = install(project.path(), &InstallOptions::new(cache.path())).unwrap_err();
        assert!(matches!(error, VendorError::RootSiblingDependency { .. }));
    }

    // A project pinning appkit (which pulls stdlib + objc), against a local repo.
    fn appkit_project() -> (TempDir, TempDir, InstallOptions) {
        let repo = TempDir::new().unwrap();
        monorepo(repo.path());
        let project = TempDir::new().unwrap();
        write(
            &project.path().join("Cplus.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nappkit = \"https://github.com/netdur/cplus/tree/main/vendor/appkit@0.0.26\"\n",
        );
        let cache = TempDir::new().unwrap();
        let mut options = InstallOptions::new(cache.path());
        options.repo_url_override = Some(repo.path().to_string_lossy().into_owned());
        (repo, project, options)
    }

    #[test]
    fn already_present_packages_are_left_untouched() {
        let (_repo, project, options) = appkit_project();
        install(project.path(), &options).unwrap();

        // A local edit inside an installed package must survive a re-install.
        let marker = project.path().join("vendor/stdlib/LOCAL.txt");
        fs::write(&marker, "edited").unwrap();

        let again = install(project.path(), &options).unwrap();
        assert!(again.iter().all(|r| !r.fresh), "nothing should be refetched");
        assert!(marker.is_file(), "up-to-date package was wiped");
    }

    #[test]
    fn a_missing_package_is_reinstalled_others_untouched() {
        let (_repo, project, options) = appkit_project();
        install(project.path(), &options).unwrap();

        // Mark stdlib, then delete objc: only objc should come back.
        let marker = project.path().join("vendor/stdlib/LOCAL.txt");
        fs::write(&marker, "edited").unwrap();
        fs::remove_dir_all(project.path().join("vendor/objc")).unwrap();

        let again = install(project.path(), &options).unwrap();
        let fresh: Vec<&str> = again
            .iter()
            .filter(|r| r.fresh)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(fresh, vec!["objc"]);
        assert!(project.path().join("vendor/objc/Cplus.toml").is_file());
        assert!(marker.is_file(), "stdlib should have been left alone");
    }

    #[test]
    fn a_missing_stamp_triggers_reinstall() {
        // The stamp is what marks a package as installed; without it (e.g. a
        // hand-copied vendor dir) the package is treated as needing install.
        let (_repo, project, options) = appkit_project();
        install(project.path(), &options).unwrap();
        fs::remove_file(project.path().join("vendor/stdlib/.cplus-vendor")).unwrap();

        let again = install(project.path(), &options).unwrap();
        let fresh: Vec<&str> = again
            .iter()
            .filter(|r| r.fresh)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(fresh, vec!["stdlib"]);
    }

    #[test]
    fn changing_the_pinned_version_refetches_that_version() {
        // A monorepo with two releases whose stdlib differs.
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q"]);
        let stdlib_manifest = "[package]\nname = \"stdlib\"\nversion = \"0.0.0\"\n";
        write(&repo.path().join("vendor/stdlib/Cplus.toml"), stdlib_manifest);
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v26\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r26"]);
        git(repo.path(), &["tag", "v0.0.26"]);
        write(&repo.path().join("vendor/stdlib/src/lib/io.cplus"), "// v27\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-qm", "r27"]);
        git(repo.path(), &["tag", "v0.0.27"]);

        let project = TempDir::new().unwrap();
        let manifest = |v: &str| {
            format!(
                "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[dependencies]\nstdlib = \"https://github.com/netdur/cplus/tree/main/vendor/stdlib@{v}\"\n"
            )
        };
        write(&project.path().join("Cplus.toml"), &manifest("0.0.26"));
        let cache = TempDir::new().unwrap();
        let mut options = InstallOptions::new(cache.path());
        options.repo_url_override = Some(repo.path().to_string_lossy().into_owned());

        install(project.path(), &options).unwrap();
        let io = project.path().join("vendor/stdlib/src/lib/io.cplus");
        assert_eq!(fs::read_to_string(&io).unwrap(), "// v26\n");

        // Re-pin to 0.0.27 → the changed pin forces a refetch of the new tree.
        write(&project.path().join("Cplus.toml"), &manifest("0.0.27"));
        let again = install(project.path(), &options).unwrap();
        assert!(again.iter().find(|r| r.name == "stdlib").unwrap().fresh);
        assert_eq!(fs::read_to_string(&io).unwrap(), "// v27\n");
    }
}
