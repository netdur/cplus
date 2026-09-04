//! Read the parts of a `Cplus.toml` the package manager cares about.
//!
//! `cpc pm` shares the one manifest format the rest of the toolchain uses
//! (`cpc init` writes it, `cpc build` consumes it). This crate stays standalone
//! (no dependency on the compiler crates), so it re-reads the same file with a
//! deliberately narrow view: the package's `name`/`version` and the
//! `[dependencies]` table. Every other table (`[library]`, `[link]`,
//! `[build]`, `[profile]`, platform `entry` keys, …) is ignored — building
//! packages is `cpc build`'s job, not this tool's.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The manifest filename, shared with `cpc init` / `cpc build`.
pub const MANIFEST_NAME: &str = "Cplus.toml";

/// The platform section names, in lockstep with the compiler's
/// `target::PLATFORMS`. Used for `[<platform>.dependencies]` merging here
/// and for `cpc pm add`'s target-set detection.
pub const PLATFORMS: [&str; 7] = [
    "macos", "linux", "windows", "ios", "android", "esp32", "wasm",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// `[dependencies]` plus every `[<platform>.dependencies]` section,
    /// merged: package name → raw spec string. The string is a git tree-URL
    /// (`…/tree/<ref>/<subpath>@<version>`) for a pinned dependency, or a
    /// bare version / `*` for a monorepo sibling. See [`crate::spec`].
    ///
    /// Platform sections are merged in deliberately — `vendor/` is committed
    /// and must build on every OS the manifest supports, so install fetches
    /// the union; the compiler's build driver is what filters by platform.
    pub deps: BTreeMap<String, String>,
    /// `[android.maven]` — third-party Maven/AAR coordinates, keyed
    /// `group:artifact` → version (D18). Android-only by construction: the
    /// compiler rejects the table under any other platform, so this is a
    /// flat map rather than one per platform.
    pub maven: BTreeMap<String, String>,
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
    /// A `[dependencies]` key that is not a valid package name. Rejected at
    /// load so a name like `../evil` or `/etc` never reaches the filesystem.
    InvalidDependencyName {
        path: PathBuf,
        name: String,
    },
    /// A `[<platform>.maven]` key that is not a `group:artifact`
    /// coordinate, or the table on a platform that has no Maven (D18).
    InvalidMavenCoordinate {
        path: PathBuf,
        platform: String,
        key: String,
        message: String,
    },
    /// One dependency name declared twice with incompatible meaning: in
    /// `[dependencies]` AND a `[<platform>.dependencies]` section, or in two
    /// platform sections with different spec strings. Kept in lockstep with
    /// the compiler's E0869 — the pm has no conflict resolver by design, so
    /// one package must resolve to one spec.
    ConflictingDependency {
        path: PathBuf,
        name: String,
        message: String,
    },
}

/// The package-name rule, kept in lockstep with the compiler's
/// `cplus-core::manifest::is_valid_dep_name`: a lowercase identifier
/// (`[a-z][a-z0-9_]*`). It is also the package manager's security boundary —
/// a valid name is a single path component, so `vendor/<name>` can never
/// contain `/`, `..`, or an absolute prefix that would escape `vendor/`.
pub(crate) fn is_valid_dep_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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
        // Reject bad dependency names before they reach `vendor/<name>` in a
        // fetch/copy/remove. This mirrors the compiler's own manifest loader
        // (which the package manager had drifted from) and, because a valid
        // name is a single path component, is what keeps install/remove inside
        // `vendor/`. Covers transitive deps too: each vendored package's own
        // manifest is loaded through here before its deps are walked.
        for name in raw.dependencies.keys() {
            if !is_valid_dep_name(name) {
                return Err(ManifestError::InvalidDependencyName {
                    path: root.join(MANIFEST_NAME),
                    name: name.clone(),
                });
            }
        }
        let mut deps = raw.dependencies;
        // Merge every `[<platform>.dependencies]` section in: install fetches
        // the union of all platforms (vendor/ must build anywhere), so the pm
        // only cares WHICH packages exist, not where they're active. The
        // duplicate rules mirror the compiler's E0869: base + section is a
        // conflict; two sections may share a dep only with an identical spec.
        let mut maven: BTreeMap<String, String> = BTreeMap::new();
        let sections: [(&str, Option<RawPlatformSection>); 7] = [
            ("macos", raw.macos),
            ("linux", raw.linux),
            ("windows", raw.windows),
            ("ios", raw.ios),
            ("android", raw.android),
            ("esp32", raw.esp32),
            ("wasm", raw.wasm),
        ];
        // name → the platform section that first declared it (base deps are
        // absent here), so base-vs-section and spec drift are told apart.
        let mut scoped_origin: BTreeMap<String, &str> = BTreeMap::new();
        for (platform, section) in sections {
            let Some(section) = section else { continue };
            // `[<platform>.maven]`. Android is the only platform with a
            // Maven ecosystem; the table anywhere else is a mistake worth
            // naming rather than a silently ignored key (the compiler
            // raises E0877 for the same shape).
            for (key, version) in section.maven {
                if platform != "android" {
                    return Err(ManifestError::InvalidMavenCoordinate {
                        path: root.join(MANIFEST_NAME),
                        platform: platform.to_string(),
                        key,
                        message: "Maven artifacts are an Android-only ecosystem — use `[android.maven]`".to_string(),
                    });
                }
                if key.split(':').count() != 2 || key.split(':').any(|p| p.trim().is_empty()) {
                    return Err(ManifestError::InvalidMavenCoordinate {
                        path: root.join(MANIFEST_NAME),
                        platform: platform.to_string(),
                        key,
                        message: "expected a `\"group:artifact\" = \"version\"` entry".to_string(),
                    });
                }
                if version.trim().is_empty() {
                    return Err(ManifestError::InvalidMavenCoordinate {
                        path: root.join(MANIFEST_NAME),
                        platform: platform.to_string(),
                        key,
                        message: "version is empty — Maven coordinates are exact pins (D2), never `*`".to_string(),
                    });
                }
                maven.insert(key, version);
            }
            for (name, spec) in section.dependencies {
                if !is_valid_dep_name(&name) {
                    return Err(ManifestError::InvalidDependencyName {
                        path: root.join(MANIFEST_NAME),
                        name,
                    });
                }
                match deps.get(&name) {
                    None => {
                        deps.insert(name.clone(), spec);
                        scoped_origin.insert(name, platform);
                    }
                    Some(_) if !scoped_origin.contains_key(&name) => {
                        return Err(ManifestError::ConflictingDependency {
                            path: root.join(MANIFEST_NAME),
                            name,
                            message: format!(
                                "declared in `[dependencies]` (all platforms) and again in `[{platform}.dependencies]` — remove one"
                            ),
                        });
                    }
                    Some(existing) if *existing != spec => {
                        return Err(ManifestError::ConflictingDependency {
                            path: root.join(MANIFEST_NAME),
                            name: name.clone(),
                            message: format!(
                                "declared with spec `{existing}` in `[{}.dependencies]` and spec `{spec}` in `[{platform}.dependencies]` — one package, one spec",
                                scoped_origin[&name],
                            ),
                        });
                    }
                    Some(_) => {} // same spec in another platform section: merged
                }
            }
        }
        Ok(Self {
            name: raw.package.name,
            version: raw.package.version,
            deps,
            maven,
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
    // `[<platform>.dependencies]` sections — one field per platform the
    // compiler's `target::PLATFORMS` knows, kept in lockstep. The pm stays
    // lenient about everything else in the file ([[bin]], [link], …), but
    // platform deps must be READ, not skipped: skipping them would leave
    // vendor/ missing the packages another OS's build needs.
    #[serde(default)]
    macos: Option<RawPlatformSection>,
    #[serde(default)]
    linux: Option<RawPlatformSection>,
    #[serde(default)]
    windows: Option<RawPlatformSection>,
    #[serde(default)]
    ios: Option<RawPlatformSection>,
    #[serde(default)]
    android: Option<RawPlatformSection>,
    #[serde(default)]
    esp32: Option<RawPlatformSection>,
    #[serde(default)]
    wasm: Option<RawPlatformSection>,
}

/// One `[<platform>.*]` table, narrowed to the key the pm cares about.
/// Other keys under the platform table are ignored here (the compiler's
/// loader is the strict one).
#[derive(Debug, Deserialize)]
struct RawPlatformSection {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    /// `[android.maven]` — `"group:artifact" = "version"` (D18).
    #[serde(default)]
    maven: BTreeMap<String, String>,
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
            ManifestError::InvalidDependencyName { path, name } => write!(
                f,
                "invalid dependency name `{name}` in {}: a package name must be a lowercase identifier ([a-z][a-z0-9_]*)",
                path.display()
            ),
            ManifestError::InvalidMavenCoordinate {
                path,
                platform,
                key,
                message,
            } => write!(
                f,
                "invalid `[{platform}.maven]` entry `{key}` in {}: {message}",
                path.display()
            ),
            ManifestError::ConflictingDependency {
                path,
                name,
                message,
            } => write!(
                f,
                "dependency `{name}` in {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_cpc_init_manifest() {
        // Exactly what `cpc init` writes (entry defaults to src/main.cplus;
        // there is no target section). Unknown keys/tables are ignored.
        let manifest = Manifest::parse(
            r#"
[package]
name    = "Inspect"
version = "0.0.1"
edition = "2026"

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
    fn platform_sections_merge_into_the_dep_map() {
        // The pm fetches the union of all platforms: vendor/ is committed
        // and must build on every OS the manifest supports.
        let manifest = Manifest::parse(
            r#"
[package]
name    = "app"
version = "0.0.1"

[dependencies]
stdlib = "*"
facet  = "*"

[macos.dependencies]
facet_appkit = "*"

[linux.dependencies]
facet_gtk = "*"
"#,
        )
        .unwrap();
        assert_eq!(manifest.deps.len(), 4);
        assert_eq!(manifest.deps["facet_appkit"], "*");
        assert_eq!(manifest.deps["facet_gtk"], "*");
    }

    #[test]
    fn same_dep_in_two_platform_sections_with_same_spec_is_fine() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "app"
version = "0.0.1"

[macos.dependencies]
objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.26"

[ios.dependencies]
objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.26"
"#,
        )
        .unwrap();
        assert_eq!(manifest.deps.len(), 1);
    }

    #[test]
    fn dep_in_base_and_platform_section_is_a_conflict() {
        let error = Manifest::parse(
            r#"
[package]
name = "app"
version = "0.0.1"

[dependencies]
objc = "*"

[macos.dependencies]
objc = "*"
"#,
        )
        .unwrap_err();
        assert!(
            matches!(error, ManifestError::ConflictingDependency { ref name, .. } if name == "objc"),
            "got: {error:?}"
        );
    }

    #[test]
    fn spec_drift_across_platform_sections_is_a_conflict() {
        let error = Manifest::parse(
            r#"
[package]
name = "app"
version = "0.0.1"

[macos.dependencies]
objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.26"

[ios.dependencies]
objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.25"
"#,
        )
        .unwrap_err();
        assert!(
            matches!(error, ManifestError::ConflictingDependency { .. }),
            "got: {error:?}"
        );
    }

    #[test]
    fn platform_section_dep_names_are_validated_too() {
        let error = Manifest::parse(
            "[package]\nname = \"app\"\nversion = \"0.0.1\"\n\n[linux.dependencies]\n\"../evil\" = \"*\"\n",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidDependencyName { .. }
        ));
    }

    #[test]
    fn android_maven_coordinates_are_read() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "maps"
version = "0.0.1"

[android.dependencies]
jni = "*"

[android.maven]
"com.google.android.gms:play-services-maps" = "19.0.0"
"androidx.annotation:annotation" = "1.1.0"
"#,
        )
        .unwrap();
        assert_eq!(manifest.deps["jni"], "*");
        assert_eq!(
            manifest.maven["com.google.android.gms:play-services-maps"],
            "19.0.0"
        );
        assert_eq!(manifest.maven.len(), 2);
    }

    #[test]
    fn maven_outside_android_is_rejected() {
        // Maven is an Android ecosystem. The table elsewhere would be a
        // silently-ignored key, which is how a dependency goes missing.
        let error = Manifest::parse(
            "[package]\nname = \"p\"\nversion = \"0.0.1\"\n\n[ios.maven]\n\"com.x:y\" = \"1.0\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(error, ManifestError::InvalidMavenCoordinate { ref platform, .. } if platform == "ios"),
            "got: {error:?}"
        );
    }

    #[test]
    fn a_malformed_maven_coordinate_is_rejected() {
        for (key, version) in [
            ("com.x", "1.0"),          // no artifact
            ("com.x:y:1.0", "1.0"),    // version in the key
            ("com.x:", "1.0"),         // empty artifact
            (":y", "1.0"),             // empty group
            ("com.x:y", ""),           // no version
            ("com.x:y", "*"),          // a wildcard is not a Maven version…
        ] {
            let src = format!(
                "[package]\nname = \"p\"\nversion = \"0.0.1\"\n\n[android.maven]\n\"{key}\" = \"{version}\"\n"
            );
            let parsed = Manifest::parse(&src);
            if version == "*" {
                // …but that one is caught when the coordinate is resolved,
                // not here: `*` is a legal-looking string. Assert the shape
                // check passes so the failure lands with a Maven message.
                assert!(parsed.is_ok(), "`{key}` = `{version}`");
                continue;
            }
            assert!(
                matches!(parsed, Err(ManifestError::InvalidMavenCoordinate { .. })),
                "`{key}` = `{version}` should be rejected"
            );
        }
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

    #[test]
    fn traversal_and_absolute_dependency_names_are_rejected() {
        // Quoted TOML keys can hold arbitrary strings; a path-traversal or
        // absolute name would escape `vendor/` on install, so it is rejected
        // at load before any fetch/copy touches the filesystem.
        for bad in ["../outside", "/etc", "a/b", "..", "UPPER", "has space", ""] {
            let src = format!(
                "[package]\nname = \"p\"\nversion = \"0.0.1\"\n\n[dependencies]\n\"{bad}\" = \"*\"\n"
            );
            assert!(
                matches!(
                    Manifest::parse(&src),
                    Err(ManifestError::InvalidDependencyName { .. })
                ),
                "`{bad}` should be rejected as an invalid dependency name"
            );
        }
    }

    #[test]
    fn valid_dependency_names_are_accepted() {
        assert!(is_valid_dep_name("stdlib"));
        assert!(is_valid_dep_name("agent_appkit"));
        assert!(is_valid_dep_name("gtk4"));
        assert!(!is_valid_dep_name("../x"));
        assert!(!is_valid_dep_name("/x"));
        assert!(!is_valid_dep_name("4gtk")); // must start with a letter
        assert!(!is_valid_dep_name("Stdlib"));
        assert!(!is_valid_dep_name(""));
    }
}
