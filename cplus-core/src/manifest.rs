//! `Cplus.toml` manifest loader (Phase 4 slice 4A).
//!
//! Schema (see `docs/design/phase4-modules.md` §5):
//!
//! ```toml
//! [package]
//! name    = "myapp"
//! version = "0.1.0"
//! edition = "2026"
//!
//! [[bin]]
//! name = "myapp"
//! path = "src/main.cplus"
//! ```
//!
//! Only `[package]` is required. If `[[bin]]` is omitted, defaults are
//! `name = package.name`, `path = "src/main.cplus"`.

use crate::diagnostics::{
    Applicability, DiagCode, Diagnostic, Position, Severity, SourceSpan, Suggestion,
};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub package: Package,
    pub bins: Vec<BinTarget>,
    /// Directory containing the manifest file. All bin `path` entries are
    /// resolved relative to this directory.
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinTarget {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum ManifestError {
    Io { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, message: String },
    MissingField { path: PathBuf, field: &'static str },
    UnsupportedEdition { path: PathBuf, found: String },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io { path, source } => {
                write!(f, "reading manifest {}: {source}", path.display())
            }
            ManifestError::Parse { path, message } => {
                write!(f, "parsing manifest {}: {message}", path.display())
            }
            ManifestError::MissingField { path, field } => {
                write!(f, "manifest {} is missing required field `{field}`", path.display())
            }
            ManifestError::UnsupportedEdition { path, found } => {
                write!(f, "manifest {}: unsupported edition `{found}` (only `2026` is currently valid)", path.display())
            }
        }
    }
}

impl ManifestError {
    /// Render this error as a structured `Diagnostic`. Manifest issues
    /// don't have meaningful byte spans (the TOML parser would but we
    /// don't thread its spans through yet); the primary location is a
    /// position-zero anchor at the manifest file path. E0406 covers
    /// parse / missing-field / bad-edition; I/O issues use E0407 to
    /// stay consistent with the slice-4A allocation.
    pub fn to_diagnostic(&self) -> Diagnostic {
        let path = match self {
            ManifestError::Io { path, .. }
            | ManifestError::Parse { path, .. }
            | ManifestError::MissingField { path, .. }
            | ManifestError::UnsupportedEdition { path, .. } => path.clone(),
        };
        let primary = SourceSpan {
            file: path.clone(),
            start: Position { line: 1, col: 1, byte: 0 },
            end: Position { line: 1, col: 1, byte: 0 },
        };
        let mut suggestions: Vec<Suggestion> = Vec::new();
        let (code, message) = match self {
            ManifestError::Io { path, source } => (
                "E0407",
                format!("could not read manifest `{}`: {source}", path.display()),
            ),
            ManifestError::Parse { message, .. } => (
                "E0406",
                format!("malformed `Cplus.toml`: {message}"),
            ),
            ManifestError::MissingField { field, .. } => (
                "E0406",
                format!("manifest is missing required field `{field}`"),
            ),
            ManifestError::UnsupportedEdition { found, .. } => {
                // Machine-applicable: bump it to "2026".
                suggestions.push(Suggestion {
                    description: "use the current edition".to_string(),
                    span: primary.clone(),
                    replacement: "edition = \"2026\"".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                });
                (
                    "E0406",
                    format!("unsupported edition `{found}` (only `2026` is currently valid)"),
                )
            }
        };
        Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message,
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions,
        }
    }
}

/// On-disk schema. Kept distinct from the public `Manifest` so we can apply
/// defaults and validation before exposing.
#[derive(Debug, Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default, rename = "bin")]
    bin: Vec<RawBin>,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: Option<String>,
    version: Option<String>,
    edition: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawBin {
    name: Option<String>,
    path: Option<String>,
}

/// Load and validate a `Cplus.toml` file. The returned `Manifest`'s
/// `root` field holds the manifest directory; `bins[].path` entries are
/// absolute paths derived from the manifest's location.
pub fn load(manifest_path: &Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| ManifestError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })?;
    parse(&text, manifest_path)
}

pub fn parse(text: &str, manifest_path: &Path) -> Result<Manifest, ManifestError> {
    let raw: RawManifest = toml::from_str(text).map_err(|e| ManifestError::Parse {
        path: manifest_path.to_path_buf(),
        message: e.to_string(),
    })?;

    let name = raw.package.name.ok_or(ManifestError::MissingField {
        path: manifest_path.to_path_buf(),
        field: "package.name",
    })?;
    let version = raw.package.version.unwrap_or_else(|| "0.0.0".to_string());
    let edition = raw.package.edition.unwrap_or_else(|| "2026".to_string());
    if edition != "2026" {
        return Err(ManifestError::UnsupportedEdition {
            path: manifest_path.to_path_buf(),
            found: edition,
        });
    }

    // Resolve `root` to an absolute path so downstream consumers (file-id
    // derivation, target-dir creation) don't have to second-guess CWD.
    // `manifest_path.parent()` on a bare `Cplus.toml` is `Some("")`, which
    // canonicalize() rejects — handle that explicitly.
    let parent = manifest_path.parent().filter(|p| !p.as_os_str().is_empty());
    let root = match parent {
        Some(p) => std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let bins = if raw.bin.is_empty() {
        vec![BinTarget {
            name: name.clone(),
            path: root.join("src").join("main.cplus"),
        }]
    } else {
        raw.bin.into_iter().map(|b| {
            let bin_name = b.name.clone().unwrap_or_else(|| name.clone());
            let bin_path = b.path
                .map(|p| root.join(p))
                .unwrap_or_else(|| root.join("src").join("main.cplus"));
            BinTarget { name: bin_name, path: bin_path }
        }).collect()
    };

    Ok(Manifest {
        package: Package { name, version, edition },
        bins,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_in(dir: &Path, text: &str) -> Result<Manifest, ManifestError> {
        // Tests pin the manifest's "directory" via the path passed to
        // parse(); we want to verify bin paths anchor relative to that
        // directory. Use a real existing dir so canonicalize succeeds
        // predictably; std::env::temp_dir() exists on every platform.
        parse(text, &dir.join("Cplus.toml"))
    }

    fn assert_bin_relpath(m: &Manifest, idx: usize, expected_rel: &str) {
        let actual = m.bins[idx].path.strip_prefix(&m.root)
            .expect("bin path should sit under manifest root");
        assert_eq!(actual, Path::new(expected_rel));
    }

    #[test]
    fn minimum_package_only() {
        let text = r#"
            [package]
            name = "hello"
        "#;
        let dir = std::env::temp_dir();
        let m = parse_in(&dir, text).unwrap();
        assert_eq!(m.package.name, "hello");
        assert_eq!(m.package.version, "0.0.0");
        assert_eq!(m.package.edition, "2026");
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "hello");
        assert_bin_relpath(&m, 0, "src/main.cplus");
    }

    #[test]
    fn explicit_bin_entry() {
        let text = r#"
            [package]
            name = "hello"
            version = "0.1.0"
            edition = "2026"

            [[bin]]
            name = "hello-bin"
            path = "src/entry.cplus"
        "#;
        let dir = std::env::temp_dir();
        let m = parse_in(&dir, text).unwrap();
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "hello-bin");
        assert_bin_relpath(&m, 0, "src/entry.cplus");
    }

    #[test]
    fn missing_name_errors() {
        let text = r#"
            [package]
            version = "0.1.0"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField { field: "package.name", .. }));
    }

    #[test]
    fn unsupported_edition_errors() {
        let text = r#"
            [package]
            name = "x"
            edition = "2018"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedEdition { .. }));
    }

    #[test]
    fn malformed_toml_errors() {
        let text = "[[[ not valid";
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    #[test]
    fn bin_name_defaults_to_package_name() {
        let text = r#"
            [package]
            name = "myapp"

            [[bin]]
            path = "src/main.cplus"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.bins[0].name, "myapp");
    }
}
