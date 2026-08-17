//! The per-user store: `~/.cplus/`, one package set per toolchain tier.
//!
//! Decisions D7/D13/D16 (`docs/decisions.md`). The store is durable state —
//! builds resolve against it — so it lives in a home dotdir, not a cache
//! directory the OS may purge. Layout:
//!
//! ```text
//! ~/.cplus/
//!   cache/                      # disposable git clones — safe to delete
//!   tags/<repo>/<tag>          # first-seen commit per release tag (D8)
//!   v0.0.27/vendor/<name>/     # the store tier: one package set per line
//! ```
//!
//! The tier is the compatibility line of the RUNNING toolchain: the exact
//! version pre-1.0 (`v0.0.27` — every release its own universe), and
//! `major.minor` post-1.0 (`v1.2` — patch fixes float within it, D14).
//! `cplus-core` derives the same tier path for import resolution; the two
//! must stay in lockstep (`cplus-core::resolver::store_vendor_dir`).

use std::path::PathBuf;

/// What the toolchain knows and this crate deliberately doesn't (D15): where
/// toolchain packages come from. `cpc pm` fills this from its own constants;
/// the standalone binary takes it from flags. Without it, bare `*` root deps
/// cannot resolve and global install has no tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainContext {
    /// `host/owner/repo` of the toolchain monorepo, e.g. `github.com/netdur/cplus`.
    pub repo: String,
    /// The running toolchain's version, e.g. `0.0.27`.
    pub version: String,
    /// Directory inside the repo that holds the packages, e.g. `vendor`.
    pub package_root: String,
}

/// Environment override for the store root (also what tests use). The
/// default is `$HOME/.cplus`.
pub const HOME_ENV: &str = "CPLUS_HOME";

/// The store root: `$CPLUS_HOME`, else `$HOME/.cplus` (`%USERPROFILE%` on
/// Windows). `None` when no home directory can be determined.
pub fn default_root() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(HOME_ENV) {
        return Some(PathBuf::from(v));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cplus"))
}

/// The tier directory name for a toolchain version: the compatibility line.
/// Pre-1.0 every release is its own universe (`v0.0.27`); from 1.0 the line
/// is `major.minor` (`v1.2`) and only patch fixes move within it (D14).
pub fn tier(toolchain_version: &str) -> String {
    let mut parts = toolchain_version.split('.');
    let major = parts.next().unwrap_or("0");
    if major.trim() != "0" {
        let minor = parts.next().unwrap_or("0");
        return format!("v{}.{}", major.trim(), minor.trim());
    }
    format!("v{}", toolchain_version.trim())
}

/// Paths of one store rooted at `root` for one toolchain tier.
#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
    pub tier: String,
}

impl Store {
    pub fn new(root: PathBuf, toolchain_version: &str) -> Self {
        Self {
            root,
            tier: tier(toolchain_version),
        }
    }

    /// The tier's package set: `<root>/<tier>/vendor/`.
    pub fn vendor_dir(&self) -> PathBuf {
        self.root.join(&self.tier).join("vendor")
    }

    /// The clone cache: `<root>/cache/`. The only truly disposable part.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

}

/// The first-seen commit record for a release tag (D8):
/// `<root>/tags/<repo>/<tag>`. Lives beside the tiers, not in `cache/`, so
/// deleting the cache does not forget what a tag pointed at. Takes the root
/// rather than a [`Store`] because tag records are tier-independent.
pub fn tag_record(root: &std::path::Path, repo: &str, tag: &str) -> PathBuf {
    root.join("tags")
        .join(crate::fetch::sanitize(repo))
        .join(crate::fetch::sanitize(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_1_0_tier_is_the_exact_version() {
        assert_eq!(tier("0.0.27"), "v0.0.27");
        assert_eq!(tier("0.9.1"), "v0.9.1");
    }

    #[test]
    fn post_1_0_tier_is_major_minor() {
        assert_eq!(tier("1.2.3"), "v1.2");
        assert_eq!(tier("1.2.9"), "v1.2");
        assert_eq!(tier("2.0.0"), "v2.0");
    }

    #[test]
    fn store_paths() {
        let s = Store::new(PathBuf::from("/tmp/home"), "0.0.27");
        assert_eq!(s.vendor_dir(), PathBuf::from("/tmp/home/v0.0.27/vendor"));
        assert_eq!(s.cache_dir(), PathBuf::from("/tmp/home/cache"));
        assert_eq!(
            tag_record(&s.root, "github.com/netdur/cplus", "v0.0.27"),
            PathBuf::from("/tmp/home/tags/github.com_netdur_cplus/v0.0.27")
        );
    }
}
