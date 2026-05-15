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
    /// Phase 5 (v0.0.2) — C ABI export: optional library target. Mutually
    /// exclusive with `[[bin]]` (E0408). When present, `cpc build` produces
    /// `.a` and/or `.dylib`/`.so` instead of an executable, and the codegen
    /// path skips the test-driver `@main` injection.
    pub lib: Option<LibTarget>,
    /// Directory containing the manifest file. All bin `path` entries are
    /// resolved relative to this directory.
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibTarget {
    pub name: String,
    pub path: PathBuf,
    pub crate_type: CrateType,
    /// Same shape as `BinTarget.frameworks` / `.libs`: linker flags
    /// forwarded as `-framework <name>` / `-l<name>`. Today these flags
    /// are baked into the produced `.dylib` (the C consumer doesn't have
    /// to re-state them) or recorded in the `.a` archive's metadata for
    /// the consumer's link line (a future polish).
    pub frameworks: Vec<String>,
    pub libs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrateType {
    /// `libNAME.a` archive. Linked statically by the consumer.
    Staticlib,
    /// `libNAME.dylib` (macOS) / `libNAME.so` (Linux). Linked dynamically.
    Cdylib,
    /// Produce both `.a` and `.dylib`/`.so`. Default in v1.
    Both,
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
    /// v0.0.2 (AppKit-via-Cplus.toml): platform frameworks to pass to the
    /// linker. Each entry becomes `-framework <name>` (macOS / iOS only).
    /// Example: `frameworks = ["Cocoa", "Foundation"]`.
    pub frameworks: Vec<String>,
    /// v0.0.2: shared libraries to pass to the linker. Each entry becomes
    /// `-l<name>` (cross-platform). Example: `libs = ["objc", "z"]`.
    pub libs: Vec<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, message: String },
    MissingField { path: PathBuf, field: &'static str },
    UnsupportedEdition { path: PathBuf, found: String },
    /// Phase 5 (E0408): both `[[bin]]` and `[lib]` declared. A manifest is
    /// either a binary target or a library target, not both. Users with a
    /// genuine "executable + library" need two manifests today.
    BinAndLibConflict { path: PathBuf },
    /// Phase 5 (E0412): `crate-type` value not in `{staticlib, cdylib, both}`.
    UnsupportedCrateType { path: PathBuf, found: String },
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
            ManifestError::BinAndLibConflict { path } => {
                write!(f, "manifest {}: cannot declare both `[[bin]]` and `[lib]` (a manifest is either an executable or a library)", path.display())
            }
            ManifestError::UnsupportedCrateType { path, found } => {
                write!(f, "manifest {}: unsupported `crate-type` value `{found}` (must be one of `staticlib`, `cdylib`, `both`)", path.display())
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
            | ManifestError::UnsupportedEdition { path, .. }
            | ManifestError::BinAndLibConflict { path }
            | ManifestError::UnsupportedCrateType { path, .. } => path.clone(),
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
            ManifestError::BinAndLibConflict { .. } => (
                "E0408",
                "cannot declare both `[[bin]]` and `[lib]` in one manifest \
                 (a manifest is either an executable or a library — split into two crates if you need both)".to_string(),
            ),
            ManifestError::UnsupportedCrateType { found, .. } => (
                "E0412",
                format!("unsupported `crate-type` value `{found}` (expected one of `staticlib`, `cdylib`, `both`)"),
            ),
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
    #[serde(default)]
    lib: Option<RawLib>,
}

#[derive(Debug, Deserialize)]
struct RawLib {
    name: Option<String>,
    path: Option<String>,
    #[serde(default, rename = "crate-type")]
    crate_type: Option<String>,
    #[serde(default)]
    frameworks: Vec<String>,
    #[serde(default)]
    libs: Vec<String>,
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
    #[serde(default)]
    frameworks: Vec<String>,
    #[serde(default)]
    libs: Vec<String>,
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

    // Phase 5: `[lib]` and `[[bin]]` are mutually exclusive. Detect
    // explicit dual-presence here — the default-bin auto-injection that
    // follows is suppressed when `[lib]` is present (so an absent
    // `[[bin]]` block in a lib manifest doesn't conflict).
    if raw.lib.is_some() && !raw.bin.is_empty() {
        return Err(ManifestError::BinAndLibConflict {
            path: manifest_path.to_path_buf(),
        });
    }

    let lib = match raw.lib {
        None => None,
        Some(rl) => {
            let lib_name = rl.name.clone().unwrap_or_else(|| name.clone());
            let lib_path = rl.path
                .map(|p| root.join(p))
                .unwrap_or_else(|| root.join("src").join("lib.cplus"));
            let crate_type = match rl.crate_type.as_deref() {
                None | Some("both") => CrateType::Both,
                Some("staticlib")   => CrateType::Staticlib,
                Some("cdylib")      => CrateType::Cdylib,
                Some(other) => return Err(ManifestError::UnsupportedCrateType {
                    path: manifest_path.to_path_buf(),
                    found: other.to_string(),
                }),
            };
            Some(LibTarget {
                name: lib_name,
                path: lib_path,
                crate_type,
                frameworks: rl.frameworks,
                libs: rl.libs,
            })
        }
    };

    // Bin targets — only auto-injected when neither `[lib]` nor `[[bin]]`
    // was declared. When `[lib]` is present, the bin list stays empty.
    let bins = if lib.is_some() {
        Vec::new()
    } else if raw.bin.is_empty() {
        vec![BinTarget {
            name: name.clone(),
            path: root.join("src").join("main.cplus"),
            frameworks: Vec::new(),
            libs: Vec::new(),
        }]
    } else {
        raw.bin.into_iter().map(|b| {
            let bin_name = b.name.clone().unwrap_or_else(|| name.clone());
            let bin_path = b.path
                .map(|p| root.join(p))
                .unwrap_or_else(|| root.join("src").join("main.cplus"));
            BinTarget {
                name: bin_name,
                path: bin_path,
                frameworks: b.frameworks,
                libs: b.libs,
            }
        }).collect()
    };

    Ok(Manifest {
        package: Package { name, version, edition },
        bins,
        lib,
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
    fn frameworks_and_libs_parse() {
        let text = r#"
            [package]
            name = "appkit_hello"

            [[bin]]
            name = "appkit_hello"
            path = "src/main.cplus"
            frameworks = ["Cocoa", "Foundation"]
            libs = ["objc", "z"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.bins[0].frameworks, vec!["Cocoa".to_string(), "Foundation".to_string()]);
        assert_eq!(m.bins[0].libs, vec!["objc".to_string(), "z".to_string()]);
    }

    #[test]
    fn frameworks_and_libs_default_empty_when_absent() {
        // Backward-compat: existing manifests with no frameworks/libs key
        // continue to parse and produce empty vectors.
        let text = r#"
            [package]
            name = "simple"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert!(m.bins[0].frameworks.is_empty());
        assert!(m.bins[0].libs.is_empty());
    }

    #[test]
    fn lib_section_parses_with_defaults() {
        // Phase 5: `[lib]` declares a library target. Defaults: name =
        // package.name, path = src/lib.cplus, crate-type = both.
        let text = r#"
            [package]
            name = "mathlib"

            [lib]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let lib = m.lib.expect("expected a lib target");
        assert_eq!(lib.name, "mathlib");
        assert_eq!(lib.crate_type, CrateType::Both);
        assert!(lib.path.ends_with("src/lib.cplus"));
        // `[lib]` suppresses the default-bin auto-injection.
        assert!(m.bins.is_empty(), "bins should be empty when only [lib] present");
    }

    #[test]
    fn lib_section_respects_crate_type() {
        let text = r#"
            [package]
            name = "x"

            [lib]
            crate-type = "cdylib"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.lib.unwrap().crate_type, CrateType::Cdylib);

        let text2 = r#"
            [package]
            name = "x"

            [lib]
            crate-type = "staticlib"
        "#;
        let m2 = parse_in(&std::env::temp_dir(), text2).unwrap();
        assert_eq!(m2.lib.unwrap().crate_type, CrateType::Staticlib);
    }

    #[test]
    fn lib_section_rejects_unknown_crate_type() {
        let text = r#"
            [package]
            name = "x"

            [lib]
            crate-type = "rlib"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedCrateType { .. }),
            "expected UnsupportedCrateType, got: {err:?}");
    }

    #[test]
    fn lib_and_bin_together_emits_e0408() {
        let text = r#"
            [package]
            name = "x"

            [[bin]]
            name = "exe"

            [lib]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::BinAndLibConflict { .. }),
            "expected BinAndLibConflict, got: {err:?}");
    }

    #[test]
    fn lib_section_carries_frameworks_and_libs() {
        // Library can declare its own linker flags — baked into the
        // .dylib (or recorded for the consumer's link line in .a).
        let text = r#"
            [package]
            name = "uikit_wrapper"

            [lib]
            frameworks = ["UIKit"]
            libs = ["objc"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let lib = m.lib.unwrap();
        assert_eq!(lib.frameworks, vec!["UIKit".to_string()]);
        assert_eq!(lib.libs, vec!["objc".to_string()]);
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
