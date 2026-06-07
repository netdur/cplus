use serde::Serialize;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PackageId {
    origin: String,
    path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageIdError {
    Empty,
    MissingHost,
    InvalidPathSegment(String),
}

impl PackageId {
    pub fn new(value: &str) -> Result<Self, PackageIdError> {
        value.parse()
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The local import / `vendor/` directory name: the leaf of the subpath
    /// (`parser`), or the repo name when there is no subpath. This is what C+
    /// imports resolve against (`import "parser/..."` -> `vendor/parser`).
    pub fn import_name(&self) -> &str {
        let source = self.path.as_deref().unwrap_or(&self.origin);
        source.rsplit('/').next().unwrap_or(source)
    }

    pub fn repo_url(&self) -> String {
        format!("https://{}.git", self.origin)
    }

    pub fn tag_for_version(&self, version: &str) -> String {
        match self.path() {
            Some(path) => format!("{path}/v{version}"),
            None => format!("v{version}"),
        }
    }
}

impl FromStr for PackageId {
    type Err = PackageIdError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(PackageIdError::Empty);
        }

        let trimmed = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed)
            .trim_end_matches(".git")
            .trim_end_matches('/');

        let segments: Vec<&str> = trimmed.split('/').filter(|part| !part.is_empty()).collect();
        if segments.len() < 3 {
            return Err(PackageIdError::MissingHost);
        }

        for segment in &segments {
            if segment.contains('\\') || *segment == "." || *segment == ".." {
                return Err(PackageIdError::InvalidPathSegment((*segment).to_string()));
            }
        }

        let origin = segments[0..3].join("/");
        let path = if segments.len() > 3 {
            Some(segments[3..].join("/"))
        } else {
            None
        };

        Ok(Self { origin, path })
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{}/{}", self.origin, path),
            None => f.write_str(&self.origin),
        }
    }
}

impl fmt::Display for PackageIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageIdError::Empty => f.write_str("package id is empty"),
            PackageIdError::MissingHost => {
                f.write_str("package id must include host, owner, and repo")
            }
            PackageIdError::InvalidPathSegment(segment) => {
                write!(f, "package id contains invalid path segment `{segment}`")
            }
        }
    }
}

impl std::error::Error for PackageIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_root_package_id() {
        let id = PackageId::new("github.com/sled/tools").unwrap();

        assert_eq!(id.origin(), "github.com/sled/tools");
        assert_eq!(id.path(), None);
        assert_eq!(id.repo_url(), "https://github.com/sled/tools.git");
        assert_eq!(id.tag_for_version("1.2.3"), "v1.2.3");
        assert_eq!(id.to_string(), "github.com/sled/tools");
    }

    #[test]
    fn parses_subdir_package_id() {
        let id = PackageId::new("https://github.com/sled/tools/parser").unwrap();

        assert_eq!(id.origin(), "github.com/sled/tools");
        assert_eq!(id.path(), Some("parser"));
        assert_eq!(id.tag_for_version("2.1.0"), "parser/v2.1.0");
        assert_eq!(id.to_string(), "github.com/sled/tools/parser");
    }
}
