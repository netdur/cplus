use crate::fetch::{FetchError, FetchPlan};
use crate::hash::{source_tree_hash, HashError};
use crate::id::{PackageId, PackageIdError};
use crate::lockfile::{LockedPackage, Lockfile, LockfileError};
use crate::manifest::{Dependency, Manifest, ManifestError};
use crate::solver::{solve_manifest, SolverError, ROOT_PACKAGE};
use crate::version::VersionError;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Public,
    Private,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectDependency {
    pub kind: DependencyKind,
    pub id: PackageId,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct FetchReceipt {
    pub dependency: DirectDependency,
    pub plan: FetchPlan,
    pub package_dir: PathBuf,
    pub fetched_manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct ResolveOptions {
    pub cache_root: PathBuf,
    pub repo_url_override: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedGraph {
    pub lockfile: Lockfile,
    pub packages: Vec<ResolvedPackage>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub version: String,
    pub package_dir: PathBuf,
    pub source: String,
    pub hash: String,
    pub deps: Vec<String>,
}

#[derive(Debug)]
pub enum ResolveError {
    MissingDependency {
        id: String,
    },
    DependencyId {
        id: String,
        source: PackageIdError,
    },
    LocalPathDependency {
        id: String,
        path: String,
    },
    UnsupportedConstraint {
        id: String,
        constraint: String,
    },
    NoMatchingVersion {
        id: PackageId,
        constraint: String,
    },
    VersionConflict {
        id: PackageId,
        existing: String,
        requested: String,
    },
    Fetch(FetchError),
    Manifest(ManifestError),
    Hash(HashError),
    Lockfile(LockfileError),
    Solver(SolverError),
    Version(VersionError),
    FetchedManifestMismatch {
        expected_id: PackageId,
        expected_version: String,
        found_id: PackageId,
        found_version: String,
    },
}

impl DirectDependency {
    pub fn fetch(
        &self,
        cache_root: impl AsRef<Path>,
        repo_url_override: Option<&str>,
    ) -> Result<FetchReceipt, ResolveError> {
        let plan = match repo_url_override {
            Some(repo_url) => FetchPlan::with_repo_url(
                self.id.clone(),
                self.version.clone(),
                repo_url,
                cache_root,
            ),
            None => FetchPlan::new(self.id.clone(), self.version.clone(), cache_root),
        };
        let package_dir = plan.fetch().map_err(ResolveError::Fetch)?;
        let fetched_manifest =
            Manifest::load(package_dir.join("pkg.toml")).map_err(ResolveError::Manifest)?;

        if fetched_manifest.package.id != self.id
            || fetched_manifest.package.version != self.version
        {
            return Err(ResolveError::FetchedManifestMismatch {
                expected_id: self.id.clone(),
                expected_version: self.version.clone(),
                found_id: fetched_manifest.package.id.clone(),
                found_version: fetched_manifest.package.version.clone(),
            });
        }

        Ok(FetchReceipt {
            dependency: self.clone(),
            plan,
            package_dir,
            fetched_manifest,
        })
    }
}

impl ResolveOptions {
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            repo_url_override: None,
        }
    }

    pub fn with_repo_url_override(mut self, repo_url: impl Into<String>) -> Self {
        self.repo_url_override = Some(repo_url.into());
        self
    }
}

pub fn resolve_graph(
    root_manifest: &Manifest,
    options: &ResolveOptions,
) -> Result<ResolvedGraph, ResolveError> {
    let solution = solve_manifest(root_manifest, options).map_err(ResolveError::Solver)?;
    let root_hash = source_tree_hash(&root_manifest.root).map_err(ResolveError::Hash)?;
    let root_dep_refs = dep_refs_from_manifest(root_manifest, &solution.selected)?;

    let mut resolved_packages = BTreeMap::<String, ResolvedPackage>::new();
    for (id_key, version) in &solution.selected {
        if id_key == ROOT_PACKAGE {
            continue;
        }

        let id = PackageId::new(id_key).map_err(|source| ResolveError::DependencyId {
            id: id_key.clone(),
            source,
        })?;
        let dep = DirectDependency {
            kind: DependencyKind::Public,
            id,
            version: version.clone(),
        };
        let receipt = dep.fetch(&options.cache_root, options.repo_url_override.as_deref())?;
        let hash = source_tree_hash(&receipt.package_dir).map_err(ResolveError::Hash)?;
        let dep_refs = dep_refs_from_manifest(&receipt.fetched_manifest, &solution.selected)?;

        let source = format!("git+{}#{}", receipt.plan.repo_url, receipt.plan.tag);
        resolved_packages.insert(
            id_key.clone(),
            ResolvedPackage {
                id: dep.id.clone(),
                version: dep.version.clone(),
                package_dir: receipt.package_dir,
                source,
                hash,
                deps: dep_refs,
            },
        );
    }

    let mut packages = Vec::new();
    packages.push(ResolvedPackage {
        id: root_manifest.package.id.clone(),
        version: root_manifest.package.version.clone(),
        package_dir: root_manifest.root.clone(),
        source: "local".to_string(),
        hash: root_hash,
        deps: root_dep_refs,
    });
    packages.extend(resolved_packages.into_values());

    let locked = packages
        .iter()
        .map(|pkg| LockedPackage {
            id: pkg.id.to_string(),
            version: pkg.version.clone(),
            source: pkg.source.clone(),
            hash: pkg.hash.clone(),
            deps: pkg.deps.clone(),
        })
        .collect();

    Ok(ResolvedGraph {
        lockfile: Lockfile::new(locked),
        packages,
    })
}

fn dep_refs_from_manifest(
    manifest: &Manifest,
    selected: &BTreeMap<String, String>,
) -> Result<Vec<String>, ResolveError> {
    let mut refs = Vec::new();
    for deps in [
        &manifest.deps.public,
        &manifest.deps.private,
        &manifest.deps.build,
    ] {
        for dep_id in deps.keys() {
            if let Some(version) = selected.get(dep_id) {
                let id = PackageId::new(dep_id).map_err(|source| ResolveError::DependencyId {
                    id: dep_id.clone(),
                    source,
                })?;
                refs.push(LockedPackage::dep_ref(&id, version));
            }
        }
    }
    refs.sort();
    Ok(refs)
}

pub fn write_lockfile(
    root_manifest: &Manifest,
    options: &ResolveOptions,
    path: impl AsRef<Path>,
) -> Result<ResolvedGraph, ResolveError> {
    let graph = resolve_graph(root_manifest, options)?;
    graph.lockfile.write(path).map_err(ResolveError::Lockfile)?;
    Ok(graph)
}

pub fn direct_dependency(
    manifest: &Manifest,
    dependency_id: &str,
) -> Result<DirectDependency, ResolveError> {
    for (kind, deps) in [
        (DependencyKind::Public, &manifest.deps.public),
        (DependencyKind::Private, &manifest.deps.private),
        (DependencyKind::Build, &manifest.deps.build),
    ] {
        if let Some(dep) = deps.get(dependency_id) {
            let id =
                PackageId::new(dependency_id).map_err(|source| ResolveError::DependencyId {
                    id: dependency_id.to_string(),
                    source,
                })?;
            let version = exact_version(dependency_id, dep)?;
            return Ok(DirectDependency { kind, id, version });
        }
    }

    Err(ResolveError::MissingDependency {
        id: dependency_id.to_string(),
    })
}

fn exact_version(id: &str, dep: &Dependency) -> Result<String, ResolveError> {
    match dep {
        Dependency::Constraint(raw) => {
            let version = raw
                .trim()
                .strip_prefix('=')
                .unwrap_or_else(|| raw.trim())
                .strip_prefix('v')
                .unwrap_or_else(|| raw.trim().strip_prefix('=').unwrap_or_else(|| raw.trim()))
                .trim();

            if is_exact_semver(version) {
                Ok(version.to_string())
            } else {
                Err(ResolveError::UnsupportedConstraint {
                    id: id.to_string(),
                    constraint: raw.clone(),
                })
            }
        }
        Dependency::LocalPath { path } => Err(ResolveError::LocalPathDependency {
            id: id.to_string(),
            path: path.clone(),
        }),
    }
}

fn is_exact_semver(version: &str) -> bool {
    let core = version
        .split_once('-')
        .map(|(core, _)| core)
        .unwrap_or(version)
        .split_once('+')
        .map(|(core, _)| core)
        .unwrap_or(version);
    let mut parts = core.split('.');

    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if is_numeric_identifier(major)
                && is_numeric_identifier(minor)
                && is_numeric_identifier(patch)
    )
}

fn is_numeric_identifier(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::MissingDependency { id } => {
                write!(f, "manifest does not declare a direct dependency on `{id}`")
            }
            ResolveError::DependencyId { id, source } => {
                write!(f, "dependency id `{id}` is invalid: {source}")
            }
            ResolveError::LocalPathDependency { id, path } => write!(
                f,
                "dependency `{id}` is a local path dependency ({path}); v0.1 fetches tagged git dependencies only"
            ),
            ResolveError::UnsupportedConstraint { id, constraint } => write!(
                f,
                "dependency `{id}` uses constraint `{constraint}`; v0.1 requires an exact version such as `1.2.3`"
            ),
            ResolveError::NoMatchingVersion { id, constraint } => {
                write!(f, "no version of `{id}` matches constraint {constraint}")
            }
            ResolveError::VersionConflict {
                id,
                existing,
                requested,
            } => write!(
                f,
                "version conflict for `{id}`: already resolved {existing}, but another dependency requested {requested}"
            ),
            ResolveError::Fetch(source) => source.fmt(f),
            ResolveError::Manifest(source) => source.fmt(f),
            ResolveError::Hash(source) => source.fmt(f),
            ResolveError::Lockfile(source) => source.fmt(f),
            ResolveError::Solver(source) => source.fmt(f),
            ResolveError::Version(source) => source.fmt(f),
            ResolveError::FetchedManifestMismatch {
                expected_id,
                expected_version,
                found_id,
                found_version,
            } => write!(
                f,
                "fetched manifest mismatch: expected {expected_id}@{expected_version}, found {found_id}@{found_version}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
[package]
id = "github.com/app/root"
version = "0.1.0"
language = "c11"

[deps.public]
"github.com/sled/tools/parser" = "2.1.0"

[deps.private]
"github.com/madler/zlib" = "^1.3"

[deps.build]
"github.com/westes/flex" = { path = "../flex" }
"#,
        )
        .unwrap()
    }

    #[test]
    fn resolves_exact_direct_dependency() {
        let dep = direct_dependency(&manifest(), "github.com/sled/tools/parser").unwrap();

        assert_eq!(dep.kind, DependencyKind::Public);
        assert_eq!(dep.id.to_string(), "github.com/sled/tools/parser");
        assert_eq!(dep.version, "2.1.0");
    }

    #[test]
    fn rejects_range_constraint_for_v0_1() {
        let error = direct_dependency(&manifest(), "github.com/madler/zlib").unwrap_err();

        assert!(matches!(error, ResolveError::UnsupportedConstraint { .. }));
    }

    #[test]
    fn rejects_local_path_dependency_for_v0_1_fetch() {
        let error = direct_dependency(&manifest(), "github.com/westes/flex").unwrap_err();

        assert!(matches!(error, ResolveError::LocalPathDependency { .. }));
    }
}
