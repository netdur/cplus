//! Fetch a repo once, at a repo-wide version tag, into a shared cache.
//!
//! A C+ monorepo tags the whole repo `v<version>`; the many packages inside it
//! (`vendor/stdlib`, `vendor/json`, …) share that one tag. So the unit we clone
//! is the *repo at a tag*, not a per-package tree, and every dependency drawn
//! from the same `(repo, version)` reuses one checkout.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A clone of one repo at one tag, cached under `<cache>/<repo>/<tag>/source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub repo_url: String,
    pub tag: String,
    pub source_dir: PathBuf,
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
}

impl Checkout {
    /// Plan a checkout. `repo` (`host/owner/repo`) names the cache slot;
    /// `repo_url` is what git actually clones (may be a local path override).
    pub fn new(
        repo: &str,
        repo_url: impl Into<String>,
        tag: impl Into<String>,
        cache_root: &Path,
    ) -> Self {
        let tag = tag.into();
        let cache_dir = cache_root.join(sanitize(repo)).join(sanitize(&tag));
        Self {
            repo_url: repo_url.into(),
            tag,
            source_dir: cache_dir.join("source"),
        }
    }

    /// Ensure the checkout exists on disk, cloning it if absent, and return its
    /// source directory. Idempotent: a present checkout is reused as-is.
    pub fn ensure(&self) -> Result<&Path, FetchError> {
        if self.source_dir.exists() {
            return Ok(&self.source_dir);
        }
        if let Some(parent) = self.source_dir.parent() {
            fs::create_dir_all(parent).map_err(|source| FetchError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.git_clone()?;
        Ok(&self.source_dir)
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
            .arg(&self.source_dir)
            .output()
            .map_err(|source| FetchError::Io {
                path: self.source_dir.clone(),
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
                self.source_dir.display()
            ),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect()
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            FetchError::Git { command, stderr } => {
                write!(f, "`{command}` failed: {}", stderr.trim())
            }
        }
    }
}

impl std::error::Error for FetchError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_under_repo_and_tag() {
        let checkout = Checkout::new(
            "github.com/netdur/cplus",
            "https://github.com/netdur/cplus.git",
            "v0.0.26",
            Path::new(".pkgcache"),
        );
        assert_eq!(
            checkout.source_dir,
            PathBuf::from(".pkgcache/github.com_netdur_cplus/v0.0.26/source")
        );
    }

    #[test]
    fn same_repo_and_tag_share_one_checkout() {
        let cache = Path::new(".pkgcache");
        let a = Checkout::new("github.com/x/y", "u", "v1", cache);
        let b = Checkout::new("github.com/x/y", "u", "v1", cache);
        assert_eq!(a.source_dir, b.source_dir);
    }
}
