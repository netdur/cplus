//! Parse a `[dependencies]` value into where the package comes from.
//!
//! Two forms appear in practice:
//!
//! * A **pinned** git tree-URL, as `cpc init` writes for a project's direct
//!   dependencies:
//!   `https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26`
//!   — repo `github.com/netdur/cplus`, subpath `vendor/stdlib`, version
//!   `0.0.26`. The whole repo is one unit tagged `v<version>`; the subpath
//!   selects which package inside it.
//!
//! * A **sibling**, as a vendored package declares its own transitive deps:
//!   `stdlib = "*"`. No URL — it names a package that lives beside it in the
//!   same checkout (`…/vendor/stdlib`), so it inherits the parent's repo and
//!   version. There is no version solving; the monorepo is at one tag.

use std::fmt;

/// The `/tree/<ref>/` marker in a GitHub folder URL.
const TREE_MARKER: &str = "/tree/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepSpec {
    Pinned(Pinned),
    Sibling { version: Option<String> },
}

/// A dependency pinned to a subpath of a git repo at a repo-wide version tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    /// `host/owner/repo`, e.g. `github.com/netdur/cplus`.
    pub repo: String,
    /// The branch/ref named in the tree-URL (e.g. `main`). Informational: the
    /// fetch pins by the version tag, not this ref.
    pub git_ref: String,
    /// Package directory within the repo, e.g. `vendor/stdlib` (may be empty
    /// for the repo root).
    pub subpath: String,
    /// Exact version, e.g. `0.0.26`.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    Empty,
    MissingVersion { value: String },
    MissingTreeMarker { value: String },
    MissingRepo { value: String },
    /// The `/tree/<ref>/` subpath escapes the checkout (absolute, or a `..`
    /// parent component), so `source_root.join(subpath)` would point outside
    /// the cloned repo. Rejected before any fetch/copy.
    UnsafeSubpath { value: String },
}

/// A pinned subpath is joined onto the cloned repo root, so it must stay
/// inside it: reject absolute paths and any `..` parent component. (`.` and
/// normal segments are fine.)
fn subpath_is_contained(subpath: &str) -> bool {
    use std::path::{Component, Path};
    !Path::new(subpath).components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

impl DepSpec {
    /// Parse a raw `[dependencies]` value. `name` is the map key (used only for
    /// error messages).
    pub fn parse(value: &str) -> Result<Self, SpecError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SpecError::Empty);
        }

        // Sibling: a bare version / wildcard, no URL.
        if !value.contains(TREE_MARKER) && !value.contains("://") {
            let version = if value == "*" {
                None
            } else {
                Some(value.to_string())
            };
            return Ok(DepSpec::Sibling { version });
        }

        Ok(DepSpec::Pinned(Pinned::parse(value)?))
    }
}

impl Pinned {
    fn parse(value: &str) -> Result<Self, SpecError> {
        // Split the trailing `@<version>`.
        let (url, version) = value
            .rsplit_once('@')
            .ok_or_else(|| SpecError::MissingVersion {
                value: value.to_string(),
            })?;
        let version = version.trim();
        if version.is_empty() {
            return Err(SpecError::MissingVersion {
                value: value.to_string(),
            });
        }

        // Normalize the URL: drop scheme and any trailing `/`.
        let url = url
            .trim()
            .strip_prefix("https://")
            .or_else(|| url.trim().strip_prefix("http://"))
            .unwrap_or_else(|| url.trim())
            .trim_end_matches('/');

        // Split `<repo>/tree/<ref>/<subpath>`. `.git` rides on the repo part
        // (`…/cplus.git/tree/…`), so strip it there rather than off the tail.
        let (repo, rest) = url
            .split_once(TREE_MARKER)
            .ok_or_else(|| SpecError::MissingTreeMarker {
                value: value.to_string(),
            })?;
        let repo = repo.trim_matches('/').trim_end_matches(".git");
        if repo.split('/').filter(|s| !s.is_empty()).count() < 3 {
            return Err(SpecError::MissingRepo {
                value: value.to_string(),
            });
        }

        // `rest` is `<ref>/<subpath...>` (subpath may be empty).
        let (git_ref, subpath) = match rest.split_once('/') {
            Some((r, sub)) => (r, sub.trim_matches('/')),
            None => (rest, ""),
        };

        // The subpath is joined onto the checkout root; a `..` or absolute
        // component would let it point outside the cloned repo.
        if !subpath_is_contained(subpath) {
            return Err(SpecError::UnsafeSubpath {
                value: value.to_string(),
            });
        }

        Ok(Pinned {
            repo: repo.to_string(),
            git_ref: git_ref.to_string(),
            subpath: subpath.to_string(),
            version: version.to_string(),
        })
    }

    /// The https clone URL for the repo.
    pub fn repo_url(&self) -> String {
        format!("https://{}.git", self.repo)
    }

    /// The repo-wide git tag for this version (`v<version>`).
    pub fn tag(&self) -> String {
        format!("v{}", self.version)
    }

    /// Directory holding sibling packages: the parent of `subpath` (e.g. the
    /// `vendor` dir when `subpath` is `vendor/stdlib`). Transitive bare-name
    /// deps resolve to `<sibling_root>/<name>`.
    pub fn sibling_root(&self) -> String {
        match self.subpath.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => String::new(),
        }
    }
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::Empty => f.write_str("dependency spec is empty"),
            SpecError::MissingVersion { value } => write!(
                f,
                "dependency `{value}` is missing a pinned `@<version>` suffix"
            ),
            SpecError::MissingTreeMarker { value } => write!(
                f,
                "dependency `{value}` is not a `…/tree/<ref>/<path>` git URL"
            ),
            SpecError::MissingRepo { value } => write!(
                f,
                "dependency `{value}` does not name a host/owner/repo"
            ),
            SpecError::UnsafeSubpath { value } => write!(
                f,
                "dependency `{value}` has a subpath that escapes the repo (absolute or `..`)"
            ),
        }
    }
}

impl std::error::Error for SpecError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(value: &str) -> Pinned {
        match DepSpec::parse(value).unwrap() {
            DepSpec::Pinned(p) => p,
            other => panic!("expected pinned, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_cpc_init_tree_url() {
        let p = pinned("https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26");
        assert_eq!(p.repo, "github.com/netdur/cplus");
        assert_eq!(p.git_ref, "main");
        assert_eq!(p.subpath, "vendor/stdlib");
        assert_eq!(p.version, "0.0.26");
        assert_eq!(p.repo_url(), "https://github.com/netdur/cplus.git");
        assert_eq!(p.tag(), "v0.0.26");
        assert_eq!(p.sibling_root(), "vendor");
    }

    #[test]
    fn bare_star_is_a_versionless_sibling() {
        assert_eq!(
            DepSpec::parse("*").unwrap(),
            DepSpec::Sibling { version: None }
        );
    }

    #[test]
    fn bare_version_is_a_pinned_sibling_version() {
        assert_eq!(
            DepSpec::parse("0.0.26").unwrap(),
            DepSpec::Sibling {
                version: Some("0.0.26".to_string())
            }
        );
    }

    #[test]
    fn tolerates_dot_git_and_trailing_slash() {
        let p = pinned("https://github.com/netdur/cplus.git/tree/main/vendor/json@1.2.3");
        assert_eq!(p.repo, "github.com/netdur/cplus");
        assert_eq!(p.subpath, "vendor/json");
        assert_eq!(p.version, "1.2.3");
    }

    #[test]
    fn subpathless_tree_url_targets_repo_root() {
        let p = pinned("https://github.com/acme/lib/tree/main@2.0.0");
        assert_eq!(p.repo, "github.com/acme/lib");
        assert_eq!(p.git_ref, "main");
        assert_eq!(p.subpath, "");
        assert_eq!(p.sibling_root(), "");
    }

    #[test]
    fn pinned_url_without_version_is_rejected() {
        let err = DepSpec::parse("https://github.com/netdur/cplus/tree/main/vendor/stdlib")
            .unwrap_err();
        assert!(matches!(err, SpecError::MissingVersion { .. }));
    }

    #[test]
    fn url_without_repo_is_rejected() {
        let err = DepSpec::parse("https://github.com/tree/main/x@1.0.0").unwrap_err();
        assert!(matches!(err, SpecError::MissingRepo { .. }));
    }

    #[test]
    fn empty_is_rejected() {
        assert_eq!(DepSpec::parse("   ").unwrap_err(), SpecError::Empty);
    }

    #[test]
    fn subpath_escaping_the_repo_is_rejected() {
        // A `..` component in the tree-URL subpath would let install copy from
        // (or point package_src) outside the cloned checkout.
        for bad in [
            "https://github.com/o/r/tree/main/../../../etc@1.0.0",
            "https://github.com/o/r/tree/main/vendor/../../secret@1.0.0",
        ] {
            assert!(
                matches!(DepSpec::parse(bad), Err(SpecError::UnsafeSubpath { .. })),
                "`{bad}` should be rejected"
            );
        }
        // A normal nested subpath is still fine.
        assert!(DepSpec::parse("https://github.com/o/r/tree/main/vendor/stdlib@1.0.0").is_ok());
    }
}
