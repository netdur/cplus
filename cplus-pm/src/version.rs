use semver::{Version, VersionReq};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionConstraint {
    raw: String,
    req: VersionReq,
}

#[derive(Debug)]
pub enum VersionError {
    InvalidConstraint { raw: String, message: String },
    InvalidVersion { raw: String, message: String },
}

impl VersionConstraint {
    pub fn parse(raw: &str) -> Result<Self, VersionError> {
        let normalized = normalize_req(raw);
        let req =
            VersionReq::parse(&normalized).map_err(|source| VersionError::InvalidConstraint {
                raw: raw.to_string(),
                message: source.to_string(),
            })?;
        Ok(Self {
            raw: raw.to_string(),
            req,
        })
    }

    pub fn exact_version(&self) -> Option<String> {
        let trimmed = self
            .raw
            .trim()
            .strip_prefix('=')
            .unwrap_or_else(|| self.raw.trim())
            .trim()
            .strip_prefix('v')
            .unwrap_or_else(|| {
                self.raw
                    .trim()
                    .strip_prefix('=')
                    .unwrap_or_else(|| self.raw.trim())
                    .trim()
            })
            .trim();

        if Version::parse(trimmed).is_ok() {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    pub fn matches(&self, version: &str) -> Result<bool, VersionError> {
        let version = Version::parse(version).map_err(|source| VersionError::InvalidVersion {
            raw: version.to_string(),
            message: source.to_string(),
        })?;
        Ok(self.req.matches(&version))
    }
}

pub fn select_highest_matching<'a>(
    versions: impl IntoIterator<Item = &'a str>,
    constraint: &VersionConstraint,
) -> Result<Option<String>, VersionError> {
    let mut parsed = Vec::new();
    for raw in versions {
        let version = Version::parse(raw).map_err(|source| VersionError::InvalidVersion {
            raw: raw.to_string(),
            message: source.to_string(),
        })?;
        if constraint.req.matches(&version) {
            parsed.push(version);
        }
    }
    parsed.sort();
    Ok(parsed.pop().map(|version| version.to_string()))
}

fn normalize_req(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(version) = trimmed.strip_prefix('v') {
        return version.to_string();
    }
    if let Some(version) = trimmed.strip_prefix("=v") {
        return format!("={version}");
    }
    trimmed.to_string()
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionError::InvalidConstraint { raw, message } => {
                write!(f, "invalid version constraint `{raw}`: {message}")
            }
            VersionError::InvalidVersion { raw, message } => {
                write!(f, "invalid version `{raw}`: {message}")
            }
        }
    }
}

impl std::error::Error for VersionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_exact_versions() {
        assert_eq!(
            VersionConstraint::parse("=v1.2.3").unwrap().exact_version(),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            VersionConstraint::parse("^1.2").unwrap().exact_version(),
            None
        );
    }

    #[test]
    fn selects_highest_matching_version() {
        let constraint = VersionConstraint::parse("^1.2").unwrap();
        let selected = select_highest_matching(["1.2.0", "1.4.1", "2.0.0"], &constraint).unwrap();

        assert_eq!(selected, Some("1.4.1".to_string()));
    }
}
