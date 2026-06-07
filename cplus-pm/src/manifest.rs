use crate::id::{PackageId, PackageIdError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub package: Package,
    pub deps: Deps,
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Package {
    pub id: PackageId,
    pub version: String,
    pub license: Option<String>,
    pub language: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Deps {
    pub public: BTreeMap<String, Dependency>,
    pub private: BTreeMap<String, Dependency>,
    pub build: BTreeMap<String, Dependency>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Dependency {
    Constraint(String),
    LocalPath { path: String },
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
    PackageId {
        path: PathBuf,
        source: PackageIdError,
    },
    MissingPackage {
        path: PathBuf,
    },
    UnsupportedSchemaVersion {
        path: PathBuf,
        version: u32,
    },
}

impl Manifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        Self::parse_with_root(&source, manifest_root(path)?).map_err(|error| match error {
            ManifestError::Parse { source, .. } => ManifestError::Parse {
                path: path.to_path_buf(),
                source,
            },
            ManifestError::PackageId { source, .. } => ManifestError::PackageId {
                path: path.to_path_buf(),
                source,
            },
            ManifestError::MissingPackage { .. } => ManifestError::MissingPackage {
                path: path.to_path_buf(),
            },
            ManifestError::UnsupportedSchemaVersion { version, .. } => {
                ManifestError::UnsupportedSchemaVersion {
                    path: path.to_path_buf(),
                    version,
                }
            }
            ManifestError::Io { .. } => error,
        })
    }

    pub fn parse(source: &str) -> Result<Self, ManifestError> {
        Self::parse_with_root(source, PathBuf::new())
    }

    pub fn parse_with_root(source: &str, root: PathBuf) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(source).map_err(|source| ManifestError::Parse {
            path: root.join("pkg.toml"),
            source,
        })?;
        let raw_package = raw.package.ok_or_else(|| ManifestError::MissingPackage {
            path: root.join("pkg.toml"),
        })?;
        let schema_version = raw.schema_version.unwrap_or(1);
        if schema_version != 1 {
            return Err(ManifestError::UnsupportedSchemaVersion {
                path: root.join("pkg.toml"),
                version: schema_version,
            });
        }

        Ok(Self {
            schema_version,
            package: Package {
                id: PackageId::new(&raw_package.id).map_err(|source| ManifestError::PackageId {
                    path: root.join("pkg.toml"),
                    source,
                })?,
                version: raw_package.version,
                license: raw_package.license,
                language: raw_package.language,
            },
            deps: raw.deps.unwrap_or_default().into(),
            root,
        })
    }
}

fn manifest_root(path: &Path) -> Result<PathBuf, ManifestError> {
    Ok(path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?)
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    #[serde(rename = "manifest-version")]
    schema_version: Option<u32>,
    package: Option<RawPackage>,
    deps: Option<RawDeps>,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    id: String,
    version: String,
    license: Option<String>,
    language: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawDeps {
    #[serde(default)]
    public: BTreeMap<String, RawDependency>,
    #[serde(default)]
    private: BTreeMap<String, RawDependency>,
    #[serde(default)]
    build: BTreeMap<String, RawDependency>,
}

impl From<RawDeps> for Deps {
    fn from(raw: RawDeps) -> Self {
        Self {
            public: raw.public.into_iter().map(convert_dep).collect(),
            private: raw.private.into_iter().map(convert_dep).collect(),
            build: raw.build.into_iter().map(convert_dep).collect(),
        }
    }
}

fn convert_dep((id, dep): (String, RawDependency)) -> (String, Dependency) {
    let dep = match dep {
        RawDependency::Constraint(version) => Dependency::Constraint(version),
        RawDependency::LocalPath { path } => Dependency::LocalPath { path },
    };
    (id, dep)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDependency {
    Constraint(String),
    LocalPath { path: String },
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
            ManifestError::PackageId { path, source } => {
                write!(f, "invalid package id in {}: {source}", path.display())
            }
            ManifestError::MissingPackage { path } => {
                write!(f, "{} is missing required [package] table", path.display())
            }
            ManifestError::UnsupportedSchemaVersion { path, version } => write!(
                f,
                "{} uses unsupported manifest-version {version}",
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
    fn parses_manifest_sketch() {
        // Unknown tables (an older manifest's [api]/[build]/etc.) are ignored —
        // the PM only reads identity + deps.
        let manifest = Manifest::parse(
            r#"
[package]
id = "github.com/sled/tools/parser"
version = "2.1.0"
license = "MIT OR Apache-2.0"
language = "c11"

[build]
command = "./scripts/build.sh"

[deps.public]
"github.com/sled/tools/types" = "^1.4"

[deps.private]
"github.com/madler/zlib" = "^1.3"
"#,
        )
        .unwrap();

        assert_eq!(
            manifest.package.id.to_string(),
            "github.com/sled/tools/parser"
        );
        assert_eq!(manifest.package.version, "2.1.0");
        assert_eq!(manifest.package.language, "c11");
        assert_eq!(
            manifest.deps.public["github.com/sled/tools/types"],
            Dependency::Constraint("^1.4".to_string())
        );
    }

    #[test]
    fn parses_local_path_dependency() {
        let manifest = Manifest::parse(
            r#"
[package]
id = "github.com/sled/tools/parser"
version = "2.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/types" = { path = "../types" }
"#,
        )
        .unwrap();

        assert_eq!(
            manifest.deps.public["github.com/sled/tools/types"],
            Dependency::LocalPath {
                path: "../types".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let error = Manifest::parse(
            r#"
manifest-version = 2

[package]
id = "github.com/sled/tools/parser"
version = "2.1.0"
language = "c11"
"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ManifestError::UnsupportedSchemaVersion { version: 2, .. }
        ));
    }
}
