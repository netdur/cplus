use crate::id::PackageId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const LOCKFILE_VERSION: u32 = 1;
pub const GENERATED_BY: &str = "cplus-pm 1.0.0";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Lockfile {
    pub version: u32,
    pub generated_by: String,
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LockedPackage {
    pub id: String,
    pub version: String,
    pub source: String,
    pub hash: String,
    #[serde(default)]
    pub deps: Vec<String>,
}

#[derive(Debug)]
pub enum LockfileError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Encode {
        source: toml::ser::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    UnsupportedVersion {
        path: PathBuf,
        version: u32,
    },
}

impl Lockfile {
    pub fn new(mut package: Vec<LockedPackage>) -> Self {
        package.sort_by(|a, b| (&a.id, &a.version).cmp(&(&b.id, &b.version)));
        Self {
            version: LOCKFILE_VERSION,
            generated_by: GENERATED_BY.to_string(),
            package,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, LockfileError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| LockfileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let lockfile: Lockfile =
            toml::from_str(&source).map_err(|source| LockfileError::Decode {
                path: path.to_path_buf(),
                source,
            })?;
        if lockfile.version != LOCKFILE_VERSION {
            return Err(LockfileError::UnsupportedVersion {
                path: path.to_path_buf(),
                version: lockfile.version,
            });
        }
        Ok(lockfile)
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), LockfileError> {
        let path = path.as_ref();
        let encoded =
            toml::to_string_pretty(self).map_err(|source| LockfileError::Encode { source })?;
        fs::write(path, encoded).map_err(|source| LockfileError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl LockedPackage {
    pub fn dep_ref(id: &PackageId, version: &str) -> String {
        format!("{id}@{version}")
    }
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockfileError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            LockfileError::Encode { source } => write!(f, "failed to encode lockfile: {source}"),
            LockfileError::Decode { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
            LockfileError::UnsupportedVersion { path, version } => write!(
                f,
                "{} uses unsupported lockfile version {version}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LockfileError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_human_readable_lockfile() {
        let lock = Lockfile::new(vec![LockedPackage {
            id: "github.com/sled/tools/parser".to_string(),
            version: "2.1.0".to_string(),
            source: "git+https://github.com/sled/tools.git#parser/v2.1.0".to_string(),
            hash: "sha256:abc".to_string(),
            deps: vec![],
        }]);

        let encoded = toml::to_string_pretty(&lock).unwrap();

        assert!(encoded.contains("version = 1"));
        assert!(encoded.contains("[[package]]"));
        assert!(encoded.contains("github.com/sled/tools/parser"));
    }
}
