use crate::id::PackageId;
use serde::Serialize;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchPlan {
    pub id: PackageId,
    pub version: String,
    pub repo_url: String,
    pub tag: String,
    pub cache_dir: PathBuf,
    pub checkout_dir: PathBuf,
    pub package_dir: PathBuf,
}

#[derive(Debug)]
pub enum FetchError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Git {
        command: String,
        stderr: String,
    },
    MissingPackageDir {
        path: PathBuf,
    },
    InvalidTag {
        raw: String,
    },
}

impl FetchPlan {
    pub fn new(id: PackageId, version: impl Into<String>, cache_root: impl AsRef<Path>) -> Self {
        let repo_url = id.repo_url();
        Self::with_repo_url(id, version, repo_url, cache_root)
    }

    pub fn with_repo_url(
        id: PackageId,
        version: impl Into<String>,
        repo_url: impl Into<String>,
        cache_root: impl AsRef<Path>,
    ) -> Self {
        let version = version.into();
        let tag = id.tag_for_version(&version);
        let repo_url = repo_url.into();
        let cache_dir = cache_root
            .as_ref()
            .join(sanitize_path(id.origin()))
            .join(sanitize_path(&tag));
        let checkout_dir = cache_dir.join("source");
        let package_dir = match id.path() {
            Some(path) => checkout_dir.join(path),
            None => checkout_dir.clone(),
        };

        Self {
            id,
            version,
            repo_url,
            tag,
            cache_dir,
            checkout_dir,
            package_dir,
        }
    }

    pub fn fetch(&self) -> Result<PathBuf, FetchError> {
        if self.package_dir.join("pkg.toml").exists() {
            return Ok(self.package_dir.clone());
        }

        fs::create_dir_all(&self.cache_dir).map_err(|source| FetchError::Io {
            path: self.cache_dir.clone(),
            source,
        })?;

        if !self.checkout_dir.exists() {
            self.git_clone()?;
        }

        if !self.package_dir.join("pkg.toml").exists() {
            return Err(FetchError::MissingPackageDir {
                path: self.package_dir.clone(),
            });
        }

        Ok(self.package_dir.clone())
    }

    fn git_clone(&self) -> Result<(), FetchError> {
        let output = Command::new("git")
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .arg("--branch")
            .arg(&self.tag)
            .arg("--")
            .arg(&self.repo_url)
            .arg(&self.checkout_dir)
            .output()
            .map_err(|source| FetchError::Io {
                path: self.checkout_dir.clone(),
                source,
            })?;

        if output.status.success() {
            return Ok(());
        }

        Err(FetchError::Git {
            command: format!(
                "git clone --depth 1 --branch {} -- {} {}",
                self.tag,
                self.repo_url,
                self.checkout_dir.display()
            ),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

pub fn list_remote_versions(
    id: &PackageId,
    repo_url_override: Option<&str>,
) -> Result<Vec<String>, FetchError> {
    let repo_url = repo_url_override
        .map(str::to_string)
        .unwrap_or_else(|| id.repo_url());
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("--tags")
        .arg("--")
        .arg(&repo_url)
        .output()
        .map_err(|source| FetchError::Io {
            path: PathBuf::from(&repo_url),
            source,
        })?;

    if !output.status.success() {
        return Err(FetchError::Git {
            command: format!("git ls-remote --tags -- {repo_url}"),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let mut versions = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(raw_ref) = line.split_whitespace().nth(1) else {
            continue;
        };
        let Some(tag) = raw_ref
            .strip_prefix("refs/tags/")
            .and_then(|tag| tag.strip_suffix("^{}").or(Some(tag)))
        else {
            continue;
        };
        if let Some(version) = version_from_tag(id, tag)? {
            versions.push(version);
        }
    }
    versions.sort();
    versions.dedup();
    Ok(versions)
}

fn version_from_tag(id: &PackageId, tag: &str) -> Result<Option<String>, FetchError> {
    match id.path() {
        Some(path) => {
            let Some(rest) = tag
                .strip_prefix(path)
                .and_then(|rest| rest.strip_prefix("/v"))
            else {
                return Ok(None);
            };
            if rest.contains('/') {
                return Err(FetchError::InvalidTag {
                    raw: tag.to_string(),
                });
            }
            Ok(Some(rest.to_string()))
        }
        None => Ok(tag.strip_prefix('v').map(str::to_string)),
    }
}

fn sanitize_path(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect()
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            FetchError::Git { command, stderr } => {
                write!(f, "`{command}` failed: {}", stderr.trim())
            }
            FetchError::MissingPackageDir { path } => write!(
                f,
                "fetched source does not contain a pkg.toml at {}",
                path.display()
            ),
            FetchError::InvalidTag { raw } => write!(f, "invalid package tag `{raw}`"),
        }
    }
}

impl std::error::Error for FetchError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_root_fetch() {
        let plan = FetchPlan::new(
            PackageId::new("github.com/sled/tools").unwrap(),
            "1.2.3",
            ".pkgcache",
        );

        assert_eq!(plan.tag, "v1.2.3");
        assert_eq!(plan.repo_url, "https://github.com/sled/tools.git");
        assert_eq!(
            plan.package_dir,
            PathBuf::from(".pkgcache/github.com_sled_tools/v1.2.3/source")
        );
    }

    #[test]
    fn plans_subdir_fetch() {
        let plan = FetchPlan::new(
            PackageId::new("github.com/sled/tools/parser").unwrap(),
            "2.1.0",
            ".pkgcache",
        );

        assert_eq!(plan.tag, "parser/v2.1.0");
        assert_eq!(
            plan.package_dir,
            PathBuf::from(".pkgcache/github.com_sled_tools/parser_v2.1.0/source/parser")
        );
    }

    #[test]
    fn supports_explicit_repo_url() {
        let plan = FetchPlan::with_repo_url(
            PackageId::new("github.com/sled/tools/parser").unwrap(),
            "2.1.0",
            "/tmp/tools.git",
            ".pkgcache",
        );

        assert_eq!(plan.repo_url, "/tmp/tools.git");
        assert_eq!(plan.tag, "parser/v2.1.0");
    }

    #[test]
    fn extracts_versions_from_root_and_subdir_tags() {
        let root = PackageId::new("github.com/sled/tools").unwrap();
        let subdir = PackageId::new("github.com/sled/tools/parser").unwrap();

        assert_eq!(
            version_from_tag(&root, "v1.2.3").unwrap(),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            version_from_tag(&subdir, "parser/v2.1.0").unwrap(),
            Some("2.1.0".to_string())
        );
        assert_eq!(version_from_tag(&subdir, "lexer/v2.1.0").unwrap(), None);
    }
}
