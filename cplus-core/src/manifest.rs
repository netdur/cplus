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
    /// Phase 2 (v0.0.2) — package system MVP. Vendor packages declare
    /// their linker requirements in a top-level `[link]` table; the
    /// consumer's build driver walks the dep graph and forwards each
    /// dep's `[link]` to its own clang invocation. Consumers typically
    /// don't populate this directly — they use `[[bin]] frameworks`/
    /// `libs` for their own binary's link surface. Both sources of
    /// link args are merged at build time.
    pub link: Option<LinkSpec>,
    /// Phase 2 (v0.0.2) — consumer's declared dependencies. Each entry
    /// names a directory expected to exist at `vendor/<name>/` with a
    /// matching `Cplus.toml`. Version strings parse but are unused at
    /// resolution time (MVP). Empty for vendor packages and standalone
    /// programs.
    pub dependencies: Vec<Dependency>,
    /// Directory containing the manifest file. All bin `path` entries are
    /// resolved relative to this directory.
    pub root: PathBuf,
}

/// Phase 2 (v0.0.2) — top-level `[link]` table on a vendor package's
/// `Cplus.toml`. Declares the linker requirements the package wants its
/// consumer to honor when building anything that depends on it.
///
/// The manifest is the single source of truth: the build driver
/// verifies the filesystem matches what's declared, and refuses to
/// link anything else. See plan.md §"Phase 2 — Manifest = single
/// source of truth" for the E0860-E0863 error codes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkSpec {
    /// macOS / iOS frameworks. Each entry becomes `-framework <name>`
    /// on the link line. Platform-gated by clang.
    pub frameworks: Vec<String>,
    /// System libraries — expected on the consumer's machine. Each
    /// entry becomes `-l<name>` on the link line.
    pub libs: Vec<String>,
    /// Bundled binaries — shipped by THIS package, located at
    /// `src/lib/<host-triple>/<basename>`. Each entry is a basename
    /// (no path component); the file must exist for every triple in
    /// `triples`. Missing file → E0860; orphan file → E0861.
    pub bundled: Vec<String>,
    /// Host triples this package's bundled binaries are built for.
    /// Required when `bundled` is non-empty (E0863); the consumer's
    /// host must appear here (E0862) or the package can't link.
    pub triples: Vec<String>,
    /// v0.0.9 Phase 8 (cpc-gaps G-001): prebuilt `.o` files to append
    /// to the link line for any target produced from this manifest.
    /// Paths are resolved relative to the manifest directory. Useful
    /// for embedding hand-written C, assembly-generated `incbin`
    /// blobs (Metal shader libraries, etc.), or any other prebuilt
    /// object the C+ binary needs to link against.
    ///
    /// cpc doesn't run a build script — the user is responsible for
    /// producing each `.o` out-of-band (typical pattern: a Makefile
    /// invokes `clang -c foo.s -o foo.o` before `cpc build`). cpc
    /// validates each entry exists at link time and fails with
    /// E0864 if any is missing.
    pub extra_objects: Vec<PathBuf>,
}

/// Phase 2 (v0.0.2) — one entry in `[dependencies]`. Today carries
/// just (name, version-string). Resolution is presence-check only:
/// `cpc build` verifies `vendor/<name>/Cplus.toml` exists and is
/// valid. SemVer resolution is forward-compat work for `cpc fetch`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub version: String,
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
    /// Phase 2 (E0857): `[dependencies]` key fails the lowercase-ident
    /// rule. Dep names must match `[a-z][a-z0-9_]*` so the import path's
    /// first segment is unambiguous.
    InvalidDependencyName { path: PathBuf, found: String },
    /// Phase 2 (E0863): `[link].bundled` is non-empty but `[link].triples`
    /// is empty. The build driver can't know what hosts the bundled
    /// binaries are built for without a declared triples list.
    BundledRequiresTriples { path: PathBuf },
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
            ManifestError::InvalidDependencyName { path, found } => {
                write!(f, "manifest {}: dependency name `{found}` must be a lowercase identifier (`[a-z][a-z0-9_]*`)", path.display())
            }
            ManifestError::BundledRequiresTriples { path } => {
                write!(f, "manifest {}: `[link].bundled` is non-empty but `[link].triples` is empty — declare the host triples the binaries are built for", path.display())
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
            | ManifestError::UnsupportedCrateType { path, .. }
            | ManifestError::InvalidDependencyName { path, .. }
            | ManifestError::BundledRequiresTriples { path } => path.clone(),
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
            ManifestError::InvalidDependencyName { found, .. } => (
                "E0857",
                format!("dependency name `{found}` must match `[a-z][a-z0-9_]*` (no dots, slashes, or uppercase — the first segment of an import path must be unambiguous)"),
            ),
            ManifestError::BundledRequiresTriples { .. } => (
                "E0863",
                "`[link].bundled` is non-empty but `[link].triples` is empty — declare the host triples your bundled binaries are built for (e.g. `triples = [\"aarch64-apple-darwin\"]`)".to_string(),
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
    /// Phase 2: top-level `[link]` table on a vendor package's manifest.
    #[serde(default)]
    link: Option<RawLinkSpec>,
    /// Phase 2: `[dependencies]` table — `name = "version-string"` pairs.
    /// Toml's `serde` integration deserializes this as a string-keyed map.
    /// Iteration order matches insertion order via `BTreeMap` (lexicographic
    /// — fine for MVP; consumers shouldn't depend on dep ordering).
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawLinkSpec {
    #[serde(default)]
    frameworks: Vec<String>,
    #[serde(default)]
    libs: Vec<String>,
    #[serde(default)]
    bundled: Vec<String>,
    #[serde(default)]
    triples: Vec<String>,
    /// v0.0.9 Phase 8 (cpc-gaps G-001): kebab-case key `extra-objects`
    /// matching the rest of the manifest's multi-word field naming.
    #[serde(default, rename = "extra-objects")]
    extra_objects: Vec<String>,
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

    // Phase 2: convert raw `[link]` to LinkSpec + enforce
    // bundled-requires-triples. The pure-source-package case (no [link]
    // table at all) yields `link = None`; an empty [link] table still
    // round-trips as `Some(LinkSpec::default())` which is harmless.
    let link = match raw.link {
        None => None,
        Some(rl) => {
            if !rl.bundled.is_empty() && rl.triples.is_empty() {
                return Err(ManifestError::BundledRequiresTriples {
                    path: manifest_path.to_path_buf(),
                });
            }
            // v0.0.9 Phase 8 (cpc-gaps G-001): resolve each extra-object
            // path relative to the manifest directory. We don't check
            // file existence at parse time — that happens at link time
            // (E0864) so the diagnostic carries the full link context.
            let extra_objects: Vec<PathBuf> = rl
                .extra_objects
                .into_iter()
                .map(|p| root.join(p))
                .collect();
            Some(LinkSpec {
                frameworks: rl.frameworks,
                libs: rl.libs,
                bundled: rl.bundled,
                triples: rl.triples,
                extra_objects,
            })
        }
    };

    // Phase 2: validate every dep name against the lowercase-ident rule
    // so the first segment of an import path is unambiguous. Iterate in
    // BTreeMap order so any failure is deterministic.
    let mut dependencies: Vec<Dependency> = Vec::with_capacity(raw.dependencies.len());
    for (dep_name, dep_version) in raw.dependencies {
        if !is_valid_dep_name(&dep_name) {
            return Err(ManifestError::InvalidDependencyName {
                path: manifest_path.to_path_buf(),
                found: dep_name,
            });
        }
        dependencies.push(Dependency {
            name: dep_name,
            version: dep_version,
        });
    }

    Ok(Manifest {
        package: Package { name, version, edition },
        bins,
        lib,
        link,
        dependencies,
        root,
    })
}

/// Phase 2: dep names must match `[a-z][a-z0-9_]*` so the first segment
/// of an import path (e.g. `stdlib/io`) is an unambiguous identifier.
/// Rejects `Stdlib`, `stdlib/vec`, `std.lib`, and the empty string.
fn is_valid_dep_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => return false,
        Some(c) if !c.is_ascii_lowercase() => return false,
        _ => {}
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

    // ---- Phase 2 Slice 2A: [dependencies] + top-level [link] ----

    #[test]
    fn dependencies_table_parses() {
        let text = r#"
            [package]
            name = "consumer"

            [dependencies]
            stdlib = "*"
            tiny   = "0.1.0"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 2);
        // BTreeMap → lexicographic order.
        assert_eq!(m.dependencies[0].name, "stdlib");
        assert_eq!(m.dependencies[0].version, "*");
        assert_eq!(m.dependencies[1].name, "tiny");
        assert_eq!(m.dependencies[1].version, "0.1.0");
    }

    #[test]
    fn dependencies_absent_yields_empty_vec() {
        let text = r#"
            [package]
            name = "x"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn invalid_dep_name_uppercase_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            Stdlib = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { ref found, .. } if found == "Stdlib"),
            "expected InvalidDependencyName for `Stdlib`, got: {err:?}");
    }

    #[test]
    fn invalid_dep_name_slash_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "stdlib/vec" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { .. }),
            "expected InvalidDependencyName for `stdlib/vec`, got: {err:?}");
    }

    #[test]
    fn invalid_dep_name_dot_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "std.lib" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { .. }));
    }

    #[test]
    fn invalid_dep_name_leading_digit_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "1stplace" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { .. }));
    }

    #[test]
    fn dep_name_with_underscore_and_digit_accepted() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            stdlib_v2 = "*"
            tiny0     = "*"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 2);
    }

    #[test]
    fn top_level_link_table_parses() {
        let text = r#"
            [package]
            name = "appkit"

            [link]
            frameworks = ["Cocoa"]
            libs       = ["objc"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.frameworks, vec!["Cocoa".to_string()]);
        assert_eq!(link.libs, vec!["objc".to_string()]);
        assert!(link.bundled.is_empty());
        assert!(link.triples.is_empty());
    }

    #[test]
    fn top_level_link_with_bundled_and_triples_parses() {
        let text = r#"
            [package]
            name = "curl_bindings"

            [link]
            bundled = ["curl.a"]
            triples = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]
            libs    = ["z"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.bundled, vec!["curl.a".to_string()]);
        assert_eq!(link.triples.len(), 2);
        assert_eq!(link.libs, vec!["z".to_string()]);
    }

    #[test]
    fn bundled_without_triples_rejected_e0863() {
        // The manifest-as-truth principle: a package that ships
        // binaries must declare which triples it supports.
        let text = r#"
            [package]
            name = "x"

            [link]
            bundled = ["foo.a"]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::BundledRequiresTriples { .. }));
    }

    #[test]
    fn triples_without_bundled_accepted() {
        // A package that DECLARES triples but ships no binaries is
        // weird but harmless — maybe they intend to add binaries later.
        // We don't reject this; the Slice 2C build-driver path is the
        // arbiter of whether anything actually gets linked.
        let text = r#"
            [package]
            name = "x"

            [link]
            triples = ["aarch64-apple-darwin"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert!(link.bundled.is_empty());
        assert_eq!(link.triples, vec!["aarch64-apple-darwin".to_string()]);
    }

    #[test]
    fn empty_link_table_parses_as_some_default() {
        let text = r#"
            [package]
            name = "x"

            [link]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert!(link.frameworks.is_empty());
        assert!(link.libs.is_empty());
        assert!(link.bundled.is_empty());
        assert!(link.triples.is_empty());
        assert!(link.extra_objects.is_empty());
    }

    // ---- v0.0.9 Phase 8 (cpc-gaps G-001): [link] extra-objects ----

    #[test]
    fn link_extra_objects_parses_kebab_case() {
        let text = r#"
            [package]
            name = "x"

            [link]
            extra-objects = ["build/metallib.o", "build/shader_blob.o"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert_eq!(link.extra_objects.len(), 2);
        // Paths resolve relative to the manifest directory.
        assert!(link.extra_objects[0].ends_with("build/metallib.o"));
        assert!(link.extra_objects[1].ends_with("build/shader_blob.o"));
    }

    #[test]
    fn link_extra_objects_absent_defaults_empty() {
        // A [link] table with only frameworks/libs entries must still
        // produce an empty extra_objects vec (the backward-compat path).
        let text = r#"
            [package]
            name = "x"

            [link]
            frameworks = ["Cocoa"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert!(link.extra_objects.is_empty());
        assert_eq!(link.frameworks, vec!["Cocoa".to_string()]);
    }

    #[test]
    fn link_extra_objects_paths_anchor_to_manifest_root() {
        // Verify that the resolved path is the manifest dir joined with
        // the relative entry — not, e.g., the process CWD.
        let dir = std::env::temp_dir().join("cpc-test-extra-objects");
        let _ = std::fs::create_dir_all(&dir);
        let text = r#"
            [package]
            name = "x"

            [link]
            extra-objects = ["foo.o"]
        "#;
        let m = parse_in(&dir, text).unwrap();
        let link = m.link.unwrap();
        // The resolved path should start with the manifest's `root`
        // (which is the canonicalized form of `dir`).
        assert!(
            link.extra_objects[0].starts_with(&m.root),
            "expected {} to start with {}",
            link.extra_objects[0].display(),
            m.root.display()
        );
    }

    #[test]
    fn is_valid_dep_name_unit_cases() {
        assert!(super::is_valid_dep_name("stdlib"));
        assert!(super::is_valid_dep_name("tiny0"));
        assert!(super::is_valid_dep_name("a_b_c"));
        assert!(!super::is_valid_dep_name(""));
        assert!(!super::is_valid_dep_name("Stdlib"));
        assert!(!super::is_valid_dep_name("0std"));
        assert!(!super::is_valid_dep_name("std-lib"));
        assert!(!super::is_valid_dep_name("std.lib"));
        assert!(!super::is_valid_dep_name("std/lib"));
        assert!(!super::is_valid_dep_name(" std"));
    }
}
