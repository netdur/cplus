//! Read the parts of a `Cplus.toml` the package manager cares about.
//!
//! `cpc pm` shares the one manifest format the rest of the toolchain uses
//! (`cpc init` writes it, `cpc build` consumes it). This crate stays standalone
//! (no dependency on the compiler crates), so it re-reads the same file with a
//! deliberately narrow view: the package's `name`/`version` and the
//! `[dependencies]` table. Every other table (`[[bin]]`, `[lib]`, `[link]`,
//! `[profile]`, …) is ignored — building packages is `cpc build`'s job, not
//! this tool's.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The manifest filename, shared with `cpc init` / `cpc build`.
pub const MANIFEST_NAME: &str = "Cplus.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// `[dependencies]`: package name → raw spec string. The string is a git
    /// tree-URL (`…/tree/<ref>/<subpath>@<version>`) for a pinned dependency,
    /// or a bare version / `*` for a monorepo sibling. See [`crate::spec`].
    pub deps: BTreeMap<String, String>,
    /// Directory the manifest lives in (used to place `vendor/`).
    pub root: PathBuf,
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl Manifest {
    /// Read and parse the `Cplus.toml` at `path`. `root` is taken from the
    /// file's parent directory (canonicalized) so `vendor/` lands beside it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let root = manifest_root(path)?;
        Self::parse_with_root(&source, root).map_err(|error| match error {
            ManifestError::Parse { source, .. } => ManifestError::Parse {
                path: path.to_path_buf(),
                source,
            },
            other => other,
        })
    }

    /// Load a project's manifest given its directory (`<dir>/Cplus.toml`).
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, ManifestError> {
        Self::load(dir.as_ref().join(MANIFEST_NAME))
    }

    pub fn parse(source: &str) -> Result<Self, ManifestError> {
        Self::parse_with_root(source, PathBuf::new())
    }

    pub fn parse_with_root(source: &str, root: PathBuf) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(source).map_err(|source| ManifestError::Parse {
            path: root.join(MANIFEST_NAME),
            source,
        })?;
        Ok(Self {
            name: raw.package.name,
            version: raw.package.version,
            deps: raw.dependencies,
            root,
        })
    }
}

fn manifest_root(path: &Path) -> Result<PathBuf, ManifestError> {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: String,
    version: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            ManifestError::Parse { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_cpc_init_manifest() {
        // Exactly what `cpc init` writes. Unknown tables ([[bin]]) are ignored.
        let manifest = Manifest::parse(
            r#"
[package]
name    = "Inspect"
version = "0.0.1"
edition = "2026"

[[bin]]
name = "Inspect"
path = "src/main.cplus"

[dependencies]
stdlib = "https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26"
"#,
        )
        .unwrap();

        assert_eq!(manifest.name, "Inspect");
        assert_eq!(manifest.version, "0.0.1");
        assert_eq!(
            manifest.deps["stdlib"],
            "https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26"
        );
    }

    #[test]
    fn parses_a_vendor_package_manifest_with_bare_deps() {
        // A vendored package's own manifest declares siblings as bare names.
        let manifest = Manifest::parse(
            r#"
[package]
name = "appkit"
version = "0.0.26"

[dependencies]
stdlib     = "*"
objc       = "*"
quartzcore = "*"

[link]
frameworks = ["AppKit"]
"#,
        )
        .unwrap();

        assert_eq!(manifest.name, "appkit");
        assert_eq!(manifest.deps.len(), 3);
        assert_eq!(manifest.deps["objc"], "*");
    }

    #[test]
    fn dependencies_absent_yields_empty_map() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "leaf"
version = "0.0.1"
"#,
        )
        .unwrap();
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn missing_package_table_is_a_parse_error() {
        let error = Manifest::parse("[dependencies]\nstdlib = \"*\"\n").unwrap_err();
        assert!(matches!(error, ManifestError::Parse { .. }));
    }
}
