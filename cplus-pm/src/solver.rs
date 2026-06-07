use crate::fetch::{list_remote_versions, FetchError, FetchPlan};
use crate::id::PackageId;
use crate::manifest::{Dependency, Manifest, ManifestError};
use crate::resolve::ResolveOptions;
use pubgrub::{
    resolve, DefaultStringReporter, OfflineDependencyProvider, PubGrubError, Ranges, Reporter as _,
    SemanticVersion,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

type PgRange = Ranges<SemanticVersion>;

pub const ROOT_PACKAGE: &str = "__root__";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PubGrubSolution {
    pub selected: BTreeMap<String, String>,
    pub report: Option<String>,
}

#[derive(Debug)]
pub enum SolverError {
    PackageId {
        raw: String,
        source: crate::id::PackageIdError,
    },
    Version {
        raw: String,
        message: String,
    },
    Constraint {
        raw: String,
    },
    Fetch(FetchError),
    Manifest(ManifestError),
    NoSolution(String),
}

pub fn solve_manifest(
    root_manifest: &Manifest,
    options: &ResolveOptions,
) -> Result<PubGrubSolution, SolverError> {
    let mut index = PubGrubIndex::new();
    index.add_manifest(
        ROOT_PACKAGE.to_string(),
        &root_manifest.package.version,
        root_manifest,
    )?;

    let mut seen = BTreeSet::new();
    for request in manifest_dependencies(root_manifest)? {
        index_package_versions(&mut index, request.id, options, &mut seen)?;
    }

    let solution = match resolve(
        &index.provider,
        ROOT_PACKAGE.to_string(),
        pg_version(&root_manifest.package.version)?,
    ) {
        Ok(solution) => solution,
        Err(PubGrubError::NoSolution(mut tree)) => {
            tree.collapse_no_versions();
            return Err(SolverError::NoSolution(DefaultStringReporter::report(
                &tree,
            )));
        }
        Err(err) => return Err(SolverError::NoSolution(err.to_string())),
    };

    let mut selected = BTreeMap::new();
    for (package, version) in solution {
        selected.insert(package, version.to_string());
    }

    Ok(PubGrubSolution {
        selected,
        report: None,
    })
}

#[derive(Debug)]
struct PubGrubIndex {
    provider: OfflineDependencyProvider<String, PgRange>,
}

impl PubGrubIndex {
    fn new() -> Self {
        Self {
            provider: OfflineDependencyProvider::new(),
        }
    }

    fn add_manifest(
        &mut self,
        package: String,
        version: &str,
        manifest: &Manifest,
    ) -> Result<(), SolverError> {
        let deps = manifest_dependencies(manifest)?
            .into_iter()
            .map(|dep| Ok((dep.id.to_string(), range_from_constraint(&dep.constraint)?)))
            .collect::<Result<Vec<_>, SolverError>>()?;
        self.provider
            .add_dependencies(package, pg_version(version)?, deps);
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct SolverDependency {
    id: PackageId,
    constraint: String,
}

fn index_package_versions(
    index: &mut PubGrubIndex,
    id: PackageId,
    options: &ResolveOptions,
    seen: &mut BTreeSet<String>,
) -> Result<(), SolverError> {
    let key = id.to_string();
    if !seen.insert(key.clone()) {
        return Ok(());
    }

    let versions = list_remote_versions(&id, options.repo_url_override.as_deref())
        .map_err(SolverError::Fetch)?;
    for version in versions {
        let plan = match options.repo_url_override.as_deref() {
            Some(repo_url) => {
                FetchPlan::with_repo_url(id.clone(), version.clone(), repo_url, &options.cache_root)
            }
            None => FetchPlan::new(id.clone(), version.clone(), &options.cache_root),
        };
        let package_dir = plan.fetch().map_err(SolverError::Fetch)?;
        let manifest =
            Manifest::load(package_dir.join("pkg.toml")).map_err(SolverError::Manifest)?;

        index.add_manifest(key.clone(), &version, &manifest)?;
        for dep in manifest_dependencies(&manifest)? {
            index_package_versions(index, dep.id, options, seen)?;
        }
    }

    Ok(())
}

fn manifest_dependencies(manifest: &Manifest) -> Result<Vec<SolverDependency>, SolverError> {
    let mut out = Vec::new();
    for deps in [
        &manifest.deps.public,
        &manifest.deps.private,
        &manifest.deps.build,
    ] {
        for (raw_id, dep) in deps {
            let id = PackageId::new(raw_id).map_err(|source| SolverError::PackageId {
                raw: raw_id.clone(),
                source,
            })?;
            let Dependency::Constraint(constraint) = dep else {
                continue;
            };
            out.push(SolverDependency {
                id,
                constraint: constraint.clone(),
            });
        }
    }
    Ok(out)
}

pub fn range_from_constraint(raw: &str) -> Result<PgRange, SolverError> {
    let trimmed = raw.trim();
    if let Some(version) = trimmed.strip_prefix('^') {
        return caret_range(version);
    }
    if let Some(version) = trimmed.strip_prefix('=') {
        return Ok(Ranges::singleton(pg_version(strip_v(version))?));
    }
    if let Some(version) = trimmed.strip_prefix(">=") {
        return Ok(Ranges::higher_than(pg_version(strip_v(version))?));
    }
    if let Some(version) = trimmed.strip_prefix('>') {
        return Ok(Ranges::strictly_higher_than(pg_version(strip_v(version))?));
    }
    if let Some(version) = trimmed.strip_prefix("<=") {
        return Ok(Ranges::lower_than(pg_version(strip_v(version))?));
    }
    if let Some(version) = trimmed.strip_prefix('<') {
        return Ok(Ranges::strictly_lower_than(pg_version(strip_v(version))?));
    }

    Ok(Ranges::singleton(pg_version(strip_v(trimmed))?))
}

fn caret_range(raw: &str) -> Result<PgRange, SolverError> {
    let (major, minor, patch) = parse_partial_version(raw)?;
    let lower = SemanticVersion::new(major, minor, patch);
    let upper = if major > 0 {
        SemanticVersion::new(major + 1, 0, 0)
    } else if minor > 0 {
        SemanticVersion::new(0, minor + 1, 0)
    } else {
        SemanticVersion::new(0, 0, patch + 1)
    };
    Ok(Ranges::between(lower, upper))
}

fn parse_partial_version(raw: &str) -> Result<(u32, u32, u32), SolverError> {
    let parts = strip_v(raw)
        .split('.')
        .map(|part| {
            part.parse::<u32>().map_err(|source| SolverError::Version {
                raw: raw.to_string(),
                message: source.to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    match parts.as_slice() {
        [major] => Ok((*major, 0, 0)),
        [major, minor] => Ok((*major, *minor, 0)),
        [major, minor, patch] => Ok((*major, *minor, *patch)),
        _ => Err(SolverError::Version {
            raw: raw.to_string(),
            message: "expected major, major.minor, or major.minor.patch".to_string(),
        }),
    }
}

pub fn pg_version(raw: &str) -> Result<SemanticVersion, SolverError> {
    SemanticVersion::from_str(strip_v(raw)).map_err(|source| SolverError::Version {
        raw: raw.to_string(),
        message: source.to_string(),
    })
}

fn strip_v(raw: &str) -> &str {
    raw.trim().strip_prefix('v').unwrap_or_else(|| raw.trim())
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolverError::PackageId { raw, source } => {
                write!(f, "invalid dependency id `{raw}`: {source}")
            }
            SolverError::Version { raw, message } => {
                write!(f, "invalid version `{raw}`: {message}")
            }
            SolverError::Constraint { raw } => write!(f, "unsupported version constraint `{raw}`"),
            SolverError::Fetch(source) => source.fmt(f),
            SolverError::Manifest(source) => source.fmt(f),
            SolverError::NoSolution(report) => write!(f, "{report}"),
        }
    }
}

impl std::error::Error for SolverError {}

#[cfg(test)]
mod tests {
    use super::*;
    use pubgrub::OfflineDependencyProvider;

    #[test]
    fn converts_caret_constraints_to_pubgrub_ranges() {
        let range = range_from_constraint("^1.2").unwrap();

        assert!(range.contains(&pg_version("1.2.0").unwrap()));
        assert!(range.contains(&pg_version("1.9.9").unwrap()));
        assert!(!range.contains(&pg_version("2.0.0").unwrap()));
    }

    #[test]
    fn runs_real_pubgrub_conflict_resolution() {
        let mut provider = OfflineDependencyProvider::<String, PgRange>::new();
        provider.add_dependencies(
            "root".to_string(),
            pg_version("1.0.0").unwrap(),
            [
                ("foo".to_string(), range_from_constraint("^1.0").unwrap()),
                ("bar".to_string(), range_from_constraint("^1.0").unwrap()),
            ],
        );
        provider.add_dependencies(
            "foo".to_string(),
            pg_version("1.1.0").unwrap(),
            [("bar".to_string(), range_from_constraint("^2.0").unwrap())],
        );
        provider.add_dependencies("foo".to_string(), pg_version("1.0.0").unwrap(), []);
        provider.add_dependencies("bar".to_string(), pg_version("1.0.0").unwrap(), []);
        provider.add_dependencies("bar".to_string(), pg_version("2.0.0").unwrap(), []);

        let solution =
            resolve(&provider, "root".to_string(), pg_version("1.0.0").unwrap()).unwrap();

        assert_eq!(
            solution.get(&"foo".to_string()).unwrap().to_string(),
            "1.0.0"
        );
        assert_eq!(
            solution.get(&"bar".to_string()).unwrap().to_string(),
            "1.0.0"
        );
    }
}
