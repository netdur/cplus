//! Manage packages in a project's `vendor/` folder: install, remove, update.
//!
//! This is the whole point of the tool. `install` resolves the project's
//! dependencies, fetches each at its pinned version (verified by content hash),
//! and copies it into `<project>/vendor/<name>/`; `remove` deletes a package's
//! directory; `update` re-resolves and refreshes. The lockfile (`pkg.lock`) is
//! rewritten so the result is reproducible.

use crate::lockfile::LockfileError;
use crate::manifest::{Manifest, ManifestError};
use crate::resolve::{resolve_graph, ResolveError, ResolveOptions};
use serde::Serialize;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum VendorError {
    Manifest(ManifestError),
    Resolve(ResolveError),
    Lockfile(LockfileError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NotInstalled {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Installed {
    pub name: String,
    pub id: String,
    pub version: String,
}

pub fn vendor_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("vendor")
}

/// Resolve the project's dependency graph, fetch each package, and place it under
/// `<project>/vendor/<name>/`, then write `<project>/pkg.lock`. Idempotent:
/// re-running refreshes `vendor/` to match the current resolution (this is also
/// what `update` does).
pub fn install(
    project_dir: &Path,
    options: &ResolveOptions,
) -> Result<Vec<Installed>, VendorError> {
    let manifest = Manifest::load(project_dir.join("pkg.toml")).map_err(VendorError::Manifest)?;
    let graph = resolve_graph(&manifest, options).map_err(VendorError::Resolve)?;

    let mut installed = Vec::new();
    for pkg in &graph.packages {
        // The root package itself is not vendored.
        if pkg.source == "local" {
            continue;
        }
        let name = pkg.id.import_name().to_string();
        let dest = vendor_dir(project_dir).join(&name);
        if dest.exists() {
            fs::remove_dir_all(&dest).map_err(|source| VendorError::Io {
                path: dest.clone(),
                source,
            })?;
        }
        copy_tree(&pkg.package_dir, &dest)?;
        installed.push(Installed {
            name,
            id: pkg.id.to_string(),
            version: pkg.version.clone(),
        });
    }

    graph
        .lockfile
        .write(project_dir.join("pkg.lock"))
        .map_err(VendorError::Lockfile)?;

    installed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(installed)
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

/// Copy a directory tree from `src` into `dest`, skipping any `.git` (the cached
/// checkout's git metadata is not part of the package).
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
            VendorError::Resolve(source) => source.fmt(f),
            VendorError::Lockfile(source) => source.fmt(f),
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
