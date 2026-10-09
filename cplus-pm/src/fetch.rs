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
        let cache_dir = cache_root.join(slot_key(repo)).join(slot_key(&tag));
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
            // A slot is reused only if it was cloned from THIS remote (audit
            // F01). With injective keys a mismatch means the URL behind a repo
            // changed (an override, a mirror). Either way the cache is ours and
            // disposable, so the stale clone is dropped and fetched again
            // rather than trusted.
            if self.origin_url().as_deref() == Some(self.repo_url.as_str()) {
                return Ok(&self.source_dir);
            }
            fs::remove_dir_all(&self.source_dir).map_err(|source| FetchError::Io {
                path: self.source_dir.clone(),
                source,
            })?;
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

    /// The commit the checkout is at (`git rev-parse HEAD`) — what the
    /// release tag resolved to. Recorded in the vendor stamp and the store's
    /// tag record (D8) so "which bytes is this?" stays answerable and a
    /// moved tag is detectable.
    pub fn head_sha(&self) -> Result<String, FetchError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.source_dir)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map_err(|source| FetchError::Io {
                path: self.source_dir.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(FetchError::Git {
                command: format!("git -C {} rev-parse HEAD", self.source_dir.display()),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// `remote.origin.url` as git recorded it at clone time — the URL given
    /// to `git clone`, before any `insteadOf` rewrite.
    fn origin_url(&self) -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.source_dir)
            .arg("config")
            .arg("--get")
            .arg("remote.origin.url")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

/// A filesystem name for `value` that no OTHER value shares (audit F01).
///
/// The cache slot and the D8 tag record are keyed by the repo string, so the
/// mapping is an identity and must be injective. The old one mapped every
/// character outside `[A-Za-z0-9._-]` to `_` and kept `_`, so
/// `gitlab.com/acme/my_lib`, `gitlab.com/acme_my/lib` and
/// `gitlab.com/acme/my/lib` were one slot: whichever was fetched first was
/// installed under the others' names, and its commit became their "true"
/// tag record.
///
/// Escaping instead of replacing: `[A-Za-z0-9.-]` stays as written, `_`
/// becomes `__`, and any other byte becomes `_` plus two hex digits (`/` is
/// `_2f`). Decoding is unambiguous, so two inputs can't meet. The result is
/// still readable: `gitlab.com_2facme_2fmy__lib`.
pub(crate) fn slot_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' => out.push(byte as char),
            b'_' => out.push_str("__"),
            _ => out.push_str(&format!("_{byte:02x}")),
        }
    }
    out
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
            PathBuf::from(".pkgcache/github.com_2fnetdur_2fcplus/v0.0.26/source")
        );
    }

    #[test]
    fn repos_that_used_to_collide_get_distinct_slots() {
        // The three spellings audit F01 put in one slot.
        let keys = [
            slot_key("gitlab.com/acme/my_lib"),
            slot_key("gitlab.com/acme_my/lib"),
            slot_key("gitlab.com/acme/my/lib"),
        ];
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[0], keys[2]);
        assert_ne!(keys[1], keys[2]);
        // And the escape's own characters cannot be forged: a literal `_2f`
        // is not a `/`.
        assert_ne!(slot_key("a/b"), slot_key("a_2fb"));
        assert_ne!(slot_key("a__b"), slot_key("a_b"));
    }

    fn repo_with(dir: &Path, marker: &str) {
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        fs::create_dir_all(dir).unwrap();
        git(&["init", "-q"]);
        fs::write(dir.join("WHO"), marker).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        git(&["tag", "v1.0.0"]);
    }

    #[test]
    fn a_slot_cloned_from_another_remote_is_fetched_again() {
        // The slot is keyed by the repo NAME; the clone in it must also have
        // come from the URL being asked for, or it is not reused.
        let tmp = tempfile::TempDir::new().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        repo_with(&first, "FIRST");
        repo_with(&second, "SECOND");
        let cache = tmp.path().join("cache");

        let a = Checkout::new("gitlab.com/acme/lib", first.to_string_lossy(), "v1.0.0", &cache);
        let src = a.ensure().unwrap().to_path_buf();
        assert_eq!(fs::read_to_string(src.join("WHO")).unwrap(), "FIRST");

        let b = Checkout::new("gitlab.com/acme/lib", second.to_string_lossy(), "v1.0.0", &cache);
        let src = b.ensure().unwrap().to_path_buf();
        assert_eq!(fs::read_to_string(src.join("WHO")).unwrap(), "SECOND");

        // And the same remote is reused, not re-cloned.
        let marker = src.join("REUSED");
        fs::write(&marker, "x").unwrap();
        b.ensure().unwrap();
        assert!(marker.is_file());
    }

    #[test]
    fn a_key_is_one_path_component() {
        for v in ["github.com/x/y", "../../etc", "/abs", "a\\b", "v1.0.0"] {
            let k = slot_key(v);
            assert!(!k.contains('/') && !k.contains('\\') && k != ".." && k != ".", "{v} -> {k}");
        }
        assert_eq!(slot_key("v1.0.0"), "v1.0.0");
    }

    #[test]
    fn same_repo_and_tag_share_one_checkout() {
        let cache = Path::new(".pkgcache");
        let a = Checkout::new("github.com/x/y", "u", "v1", cache);
        let b = Checkout::new("github.com/x/y", "u", "v1", cache);
        assert_eq!(a.source_dir, b.source_dir);
    }
}
