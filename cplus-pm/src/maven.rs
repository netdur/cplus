//! Resolve a Maven/AAR dependency closure, and put it where a build can
//! reach it. The Android side of "third-party SDKs without Gradle".
//!
//! The Android toolchain ships no dependency resolver: there is no `mvn`,
//! no `cs`, and Gradle itself is downloaded by the wrapper rather than
//! installed. Gradle IS the resolver. But resolution over pinned coordinates
//! is reading XML, and an AAR is a zip — so this is that, and the closure
//! then dexes with stock `d8`. `plans/aar.md` is the measurement that
//! settled it (CameraX: 35 artifacts, 8.2 MB, one dex, 9.5s, no Gradle).
//!
//! WHERE THIS DIFFERS FROM GRADLE, both real and both inherited from
//! `tools/mvnresolve.py`, the spike this module supersedes:
//!
//!   * Conflict resolution is NEAREST-WINS, which is Maven's rule. Gradle
//!     uses HIGHEST-WINS. Two paths to different versions of one artifact
//!     resolve differently here, and this side can pick the older.
//!   * `.module` Gradle metadata is ignored. AndroidX publishes it and its
//!     POM is a compatibility shim that says so in a comment, so variant
//!     selection does not happen. Fine for plain AAR consumption, wrong for
//!     anything shipping platform-specific variants.
//!
//! Handled, because each was silent until it was not: `<parent>` chains,
//! `<properties>` interpolation, `<dependencyManagement>`, BOM
//! `<scope>import</scope>` (without it the coroutines artifacts have no
//! version and vanish), and hard version ranges `[1.2.3]` meaning exactly
//! that version.
//!
//! NOT handled: soft/open ranges `[1.0,2.0)`, classifiers, exclusions,
//! mirrors. Each fails LOUDLY — it lands in [`Closure::unresolved`], and
//! install refuses a closure with any unresolved entry rather than
//! materializing a partial one.
//!
//! TWO REPOS, and the order matters: androidx lives on Google's Maven, NOT
//! on Central, which answers 404 for every `androidx.*` coordinate and looks
//! exactly like a bad version number.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Google's Maven, then Central. Order matters (see the module note).
pub const DEFAULT_REPOS: [&str; 2] = [
    "https://dl.google.com/dl/android/maven2",
    "https://repo1.maven.org/maven2",
];

/// How many times `${...}` interpolation is re-run over one string.
/// Properties can reference properties; four passes is what the spike used
/// and no real POM has needed more.
const INTERP_PASSES: usize = 4;

// ---- coordinates ------------------------------------------------------------

/// A `group:artifact:version` Maven coordinate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Coord {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

impl Coord {
    /// Parse `group:artifact:version`.
    pub fn parse(value: &str) -> Result<Self, MavenError> {
        let parts: Vec<&str> = value.trim().split(':').collect();
        if parts.len() != 3 {
            return Err(MavenError::BadCoordinate {
                value: value.to_string(),
                reason: "expected `group:artifact:version`".to_string(),
            });
        }
        Self::new(parts[0], parts[1], parts[2])
    }

    /// Parse the `group:artifact` half plus a separate version — the shape a
    /// manifest holds (`"com.x:y" = "1.0.0"`).
    pub fn from_ga(ga: &str, version: &str) -> Result<Self, MavenError> {
        let parts: Vec<&str> = ga.trim().split(':').collect();
        if parts.len() != 2 {
            return Err(MavenError::BadCoordinate {
                value: ga.to_string(),
                reason: "expected `group:artifact`".to_string(),
            });
        }
        Self::new(parts[0], parts[1], version)
    }

    pub fn new(group: &str, artifact: &str, version: &str) -> Result<Self, MavenError> {
        let coord = Self {
            group: group.trim().to_string(),
            artifact: artifact.trim().to_string(),
            version: version.trim().to_string(),
        };
        // Every one of these becomes a path component under the local repo,
        // so this is a containment boundary, not a politeness check: the
        // strings arrive from a manifest AND from downloaded POMs, and a
        // `..` segment would write outside the store.
        for segment in coord.group.split('.') {
            check_segment(segment, &coord)?;
        }
        check_segment(&coord.artifact, &coord)?;
        check_segment(&coord.version, &coord)?;
        Ok(coord)
    }

    /// `group:artifact` — the identity a version attaches to.
    pub fn ga(&self) -> String {
        format!("{}:{}", self.group, self.artifact)
    }

    /// Repo-relative path of one file for this coordinate:
    /// `com/x/y/1.0/y-1.0.<ext>`.
    pub fn path(&self, ext: &str) -> String {
        format!(
            "{}/{}/{}/{}-{}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.artifact,
            self.version,
            ext
        )
    }

    /// Directory holding this coordinate's files, under a repo root.
    pub fn dir(&self, root: &Path) -> PathBuf {
        let mut dir = root.to_path_buf();
        for segment in self.group.split('.') {
            dir.push(segment);
        }
        dir.push(&self.artifact);
        dir.push(&self.version);
        dir
    }
}

fn check_segment(segment: &str, coord: &Coord) -> Result<(), MavenError> {
    let ok = !segment.is_empty()
        && segment
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    if ok {
        return Ok(());
    }
    Err(MavenError::BadCoordinate {
        value: format!("{}:{}:{}", coord.group, coord.artifact, coord.version),
        reason: format!(
            "`{segment}` is not a usable path component \
             (letters, digits, `.`, `_`, `-`, `+`; must not start with `.`)"
        ),
    })
}

impl fmt::Display for Coord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.group, self.artifact, self.version)
    }
}

// ---- the local repo ---------------------------------------------------------

/// The local Maven repo (`~/.cplus/m2` by default) plus the remotes it fills
/// itself from.
///
/// It is laid out as a REAL Maven repo — `com/x/y/1.0/y-1.0.aar` — so `d8`,
/// `aapt2` and anything else can be pointed straight at it, and so the cache
/// the `tools/mvnresolve.py` spike wrote is the same cache this reads.
///
/// Not tier-scoped, unlike the package store: a Maven coordinate is
/// immutable, so `1.6.2` means the same bytes to every toolchain version.
#[derive(Debug, Clone)]
pub struct Registry {
    pub root: PathBuf,
    pub repos: Vec<String>,
    /// Refuse the network: a missing artifact is an error, not a download.
    /// What `maven classpath` uses, so a build never reaches out.
    pub offline: bool,
}

impl Registry {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            repos: DEFAULT_REPOS.iter().map(|s| s.to_string()).collect(),
            offline: false,
        }
    }

    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Local path of a repo-relative file, whether or not it exists.
    pub fn local(&self, path: &str) -> PathBuf {
        let mut out = self.root.clone();
        for segment in path.split('/') {
            out.push(segment);
        }
        out
    }

    /// The bytes of one repo-relative file: cache first, then each remote in
    /// order. `Ok(None)` means every remote answered "not there" — a POM that
    /// legitimately does not exist (a parent outside both repos) reads as a
    /// missing file, not a failure.
    pub fn blob(&self, path: &str) -> Result<Option<Vec<u8>>, MavenError> {
        let local = self.local(path);
        if local.is_file() {
            let bytes = fs::read(&local).map_err(|source| MavenError::Io {
                path: local.clone(),
                source,
            })?;
            return Ok(Some(bytes));
        }
        if self.offline {
            return Ok(None);
        }
        for repo in &self.repos {
            let url = format!("{}/{}", repo.trim_end_matches('/'), path);
            if download(&url, &local)? {
                let bytes = fs::read(&local).map_err(|source| MavenError::Io {
                    path: local.clone(),
                    source,
                })?;
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    /// Local path of a file, fetching it if absent. `Ok(None)` = not in any
    /// repo.
    pub fn file(&self, path: &str) -> Result<Option<PathBuf>, MavenError> {
        Ok(self.blob(path)?.map(|_| self.local(path)))
    }
}

/// Fetch one URL to `dest`. `Ok(false)` = the server said no (404 and
/// friends) — try the next repo. Downloads land on a `.part` sibling and are
/// renamed, so an interrupted fetch never leaves a truncated artifact in the
/// cache that every later run would trust.
fn download(url: &str, dest: &Path) -> Result<bool, MavenError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|source| MavenError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("tmp")
    ));
    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("--location")
        .arg("--max-time")
        .arg("120")
        .arg("--output")
        .arg(&part)
        .arg("--")
        .arg(url)
        .output()
        .map_err(|source| MavenError::NoCurl { source })?;
    if !output.status.success() {
        let _ = fs::remove_file(&part);
        return Ok(false);
    }
    fs::rename(&part, dest).map_err(|source| MavenError::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    Ok(true)
}

// ---- POM XML ----------------------------------------------------------------

/// One XML element, narrowed to what a POM needs: a name, its text, and its
/// children. Attributes are skipped — no POM field this resolver reads lives
/// in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub name: String,
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Trimmed text of a direct child, `None` when absent or empty.
    fn text_of(&self, name: &str) -> Option<String> {
        let text = self.child(name)?.text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// Every descendant with this name, document order. `<dependency>` lives
    /// under `<dependencies>` in one place and `<dependencyManagement>` in
    /// another, and a BOM's are two levels down — the walk is what makes one
    /// accessor serve all three.
    fn descendants<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for child in &self.children {
            if child.name == name {
                out.push(child);
            }
            child.descendants(name, out);
        }
    }

    fn find_all(&self, name: &str) -> Vec<&Element> {
        let mut out = Vec::new();
        self.descendants(name, &mut out);
        out
    }
}

/// Parse a POM. Deliberately small: elements, text, comments, CDATA, the
/// prolog and a doctype. A namespace PREFIX is stripped from tag names, so a
/// `<pom:dependency>` reads the same as the usual default-namespaced
/// `<dependency>`.
pub(crate) fn parse_xml(bytes: &[u8]) -> Result<Element, MavenError> {
    let text = String::from_utf8_lossy(bytes);
    let source: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    while at < source.len() {
        if source[at] != '<' {
            // Character data belongs to the open element.
            let start = at;
            while at < source.len() && source[at] != '<' {
                at += 1;
            }
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&decode_entities(
                    &source[start..at].iter().collect::<String>(),
                ));
            }
            continue;
        }
        // `<!--` comment, `<![CDATA[`, `<!DOCTYPE`, `<?...?>`
        if starts_with(&source, at, "<!--") {
            at = skip_to(&source, at + 4, "-->", "comment")? + 3;
            continue;
        }
        if starts_with(&source, at, "<![CDATA[") {
            let end = skip_to(&source, at + 9, "]]>", "CDATA section")?;
            if let Some(top) = stack.last_mut() {
                top.text
                    .push_str(&source[at + 9..end].iter().collect::<String>());
            }
            at = end + 3;
            continue;
        }
        if starts_with(&source, at, "<!") || starts_with(&source, at, "<?") {
            at = skip_to(&source, at + 2, ">", "declaration")? + 1;
            continue;
        }
        // A closing tag: `</name>`
        if starts_with(&source, at, "</") {
            let end = skip_to(&source, at + 2, ">", "closing tag")?;
            let name = local_name(&source[at + 2..end].iter().collect::<String>());
            let done = stack.pop().ok_or(MavenError::BadXml {
                reason: format!("closing tag `</{name}>` with nothing open"),
            })?;
            if done.name != name {
                return Err(MavenError::BadXml {
                    reason: format!("`</{name}>` closes `<{}>`", done.name),
                });
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(done),
                None => root = Some(done),
            }
            at = end + 1;
            continue;
        }
        // An opening (or self-closing) tag.
        let end = skip_to(&source, at + 1, ">", "tag")?;
        let raw: String = source[at + 1..end].iter().collect();
        let self_closing = raw.trim_end().ends_with('/');
        let raw = raw.trim_end().trim_end_matches('/');
        let name = local_name(raw.split_whitespace().next().unwrap_or(""));
        if name.is_empty() {
            return Err(MavenError::BadXml {
                reason: "empty tag name".to_string(),
            });
        }
        let element = Element {
            name,
            text: String::new(),
            children: Vec::new(),
        };
        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.children.push(element),
                None => root = Some(element),
            }
        } else {
            stack.push(element);
        }
        at = end + 1;
    }

    if let Some(open) = stack.last() {
        return Err(MavenError::BadXml {
            reason: format!("`<{}>` is never closed", open.name),
        });
    }
    root.ok_or(MavenError::BadXml {
        reason: "no root element".to_string(),
    })
}

fn starts_with(source: &[char], at: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(i, c)| source.get(at + i) == Some(&c))
}

/// Index of the next `needle` at or after `from`. An unterminated construct
/// is an error rather than a silent truncation of the rest of the file.
fn skip_to(source: &[char], from: usize, needle: &str, what: &str) -> Result<usize, MavenError> {
    let mut at = from;
    while at < source.len() {
        if starts_with(source, at, needle) {
            return Ok(at);
        }
        at += 1;
    }
    Err(MavenError::BadXml {
        reason: format!("unterminated {what}"),
    })
}

/// `pom:dependency` -> `dependency`. The usual POM has a DEFAULT namespace
/// and therefore no prefix at all; this is for the rare one that does.
fn local_name(raw: &str) -> String {
    match raw.rsplit_once(':') {
        Some((_, local)) => local.trim().to_string(),
        None => raw.trim().to_string(),
    }
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- resolution -------------------------------------------------------------

/// A POM plus everything inherited down its `<parent>` chain.
struct Chain {
    /// The POMs, NEAREST FIRST. Only the nearest one's `<dependencies>` are
    /// the artifact's own; the rest contribute properties and management.
    poms: Vec<Element>,
    /// Merged `<properties>`, nearest wins.
    props: BTreeMap<String, String>,
    /// Merged `<dependencyManagement>` versions, nearest wins.
    mgmt: BTreeMap<String, String>,
}

/// Why one coordinate did not make it into the closure. Every one of these
/// is reported; none is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub what: String,
    pub reason: String,
}

/// One `group:artifact` that was requested at more than one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub ga: String,
    /// The version that won — the first one seen (nearest-wins).
    pub kept: String,
    /// The versions that lost, sorted.
    pub dropped: Vec<String>,
    /// True when a DROPPED version looks newer than the kept one. This is
    /// the whole Gradle divergence made visible: Gradle takes the highest,
    /// this takes the nearest, and only here do the two disagree. Everything
    /// else is a conflict Gradle would have resolved the same way.
    pub divergent: bool,
}

/// The result of walking one root coordinate's dependencies.
#[derive(Debug, Default, Clone)]
pub struct Closure {
    /// Every artifact, breadth-first from the roots — the order `d8` is
    /// happy to receive and a human can read.
    pub order: Vec<Coord>,
    /// BOMs whose `<dependencyManagement>` was imported, deduplicated.
    pub bom_imports: Vec<String>,
    pub unresolved: Vec<Unresolved>,
    /// Every `group:artifact` requested at more than one version, with the
    /// nearest-wins outcome. Reported, never silent.
    pub conflicts: Vec<Conflict>,
}

impl Closure {
    pub fn is_complete(&self) -> bool {
        self.unresolved.is_empty()
    }

    /// The conflicts where nearest-wins kept an OLDER version than something
    /// else asked for — the only ones where this resolver and a Gradle build
    /// actually disagree.
    pub fn divergent(&self) -> impl Iterator<Item = &Conflict> {
        self.conflicts.iter().filter(|c| c.divergent)
    }
}

/// Walk `roots` breadth-first and return every artifact in the closure.
///
/// NEAREST-WINS: the first version seen for a `group:artifact` is the one
/// kept, and later requests for another version are recorded in
/// [`Closure::conflicts`]. Roots are processed in order, so a root's own pin
/// always beats a transitive one — the same "the root wins what it names"
/// rule the git side uses (D9).
pub fn resolve(registry: &Registry, roots: &[Coord]) -> Result<Closure, MavenError> {
    let mut closure = Closure::default();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    // ga -> the versions that lost. Aggregated rather than reported per
    // occurrence: one CameraX resolve asks for kotlin-stdlib eight times,
    // and eight identical warnings bury the one that matters.
    let mut dropped: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut boms: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut queue: std::collections::VecDeque<Coord> = roots.iter().cloned().collect();

    while let Some(coord) = queue.pop_front() {
        if let Some(kept) = seen.get(&coord.ga()) {
            if *kept != coord.version {
                dropped
                    .entry(coord.ga())
                    .or_default()
                    .insert(coord.version.clone());
            }
            continue;
        }
        seen.insert(coord.ga(), coord.version.clone());
        closure.order.push(coord.clone());

        let chain = match chain(registry, &coord, &mut boms)? {
            Some(chain) => chain,
            None => {
                closure.unresolved.push(Unresolved {
                    what: coord.to_string(),
                    reason: if registry.offline {
                        "not in the local Maven repo (offline) — run `cpc pm install`".to_string()
                    } else {
                        "no POM in any repo".to_string()
                    },
                });
                continue;
            }
        };

        let Some(deps) = chain.poms[0].child("dependencies") else {
            continue;
        };
        for dep in deps.children.iter().filter(|c| c.name == "dependency") {
            // `provided`, `test` and `system` are not shipped; `optional`
            // means the consumer opts in explicitly, which is a coordinate
            // of its own in the manifest.
            let scope = dep.text_of("scope").unwrap_or_else(|| "compile".to_string());
            if scope != "compile" && scope != "runtime" {
                continue;
            }
            if dep.text_of("optional").as_deref() == Some("true") {
                continue;
            }
            let (Some(group), Some(artifact)) = (dep.text_of("groupId"), dep.text_of("artifactId"))
            else {
                closure.unresolved.push(Unresolved {
                    what: format!("a <dependency> of {coord}"),
                    reason: "no groupId/artifactId".to_string(),
                });
                continue;
            };
            let group = interpolate(&group, &chain.props);
            let artifact = interpolate(&artifact, &chain.props);
            let declared = dep.text_of("version").map(|v| interpolate(&v, &chain.props));
            let version = match declared.or_else(|| chain.mgmt.get(&format!("{group}:{artifact}")).cloned()) {
                Some(version) => version,
                None => {
                    // No version anywhere. This is the BOM failure mode: drop
                    // `<scope>import</scope>` handling and the coroutines
                    // artifacts land here silently.
                    closure.unresolved.push(Unresolved {
                        what: format!("{group}:{artifact}"),
                        reason: format!("no version (declared by {coord}, and no <dependencyManagement> entry)"),
                    });
                    continue;
                }
            };
            let version = match unwrap_range(&version) {
                Ok(version) => version,
                Err(reason) => {
                    closure.unresolved.push(Unresolved {
                        what: format!("{group}:{artifact}:{version}"),
                        reason,
                    });
                    continue;
                }
            };
            match Coord::new(&group, &artifact, &version) {
                Ok(coord) => queue.push_back(coord),
                Err(error) => closure.unresolved.push(Unresolved {
                    what: format!("{group}:{artifact}:{version}"),
                    reason: error.to_string(),
                }),
            }
        }
    }

    closure.bom_imports = boms.into_iter().collect();
    for (ga, versions) in dropped {
        let kept = seen[&ga].clone();
        let divergent = versions.iter().any(|v| is_newer(v, &kept));
        closure.conflicts.push(Conflict {
            ga,
            kept,
            dropped: versions.into_iter().collect(),
            divergent,
        });
    }
    Ok(closure)
}

/// Is `candidate` a newer version than `kept`?
///
/// Dot-separated segments compared as numbers where both are numbers, as
/// text otherwise; a missing segment is lower (`1.2` < `1.2.1`). This is not
/// full Maven version ordering — it exists to answer one question, "would
/// Gradle's highest-wins have picked something else here?", and it errs
/// toward YES: `1.0.0-alpha01` compares above `1.0.0` under the textual
/// fallback, so a pre-release is reported rather than hidden.
fn is_newer(candidate: &str, kept: &str) -> bool {
    let mut a = candidate.split('.');
    let mut b = kept.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return false,
            (Some(_), None) => return true,
            (None, Some(_)) => return false,
            (Some(x), Some(y)) if x == y => continue,
            (Some(x), Some(y)) => {
                return match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x > y,
                    _ => x > y,
                }
            }
        }
    }
}

/// Load a POM and its `<parent>` chain, merging properties and
/// `<dependencyManagement>` nearest-first. `Ok(None)` when the coordinate's
/// own POM is missing.
fn chain(
    registry: &Registry,
    coord: &Coord,
    boms: &mut std::collections::BTreeSet<String>,
) -> Result<Option<Chain>, MavenError> {
    let mut chain = Chain {
        poms: Vec::new(),
        props: BTreeMap::new(),
        mgmt: BTreeMap::new(),
    };
    let mut at = Some(coord.clone());
    // A malformed POM whose <parent> points back at itself would spin here.
    let mut guard_count = 0;
    while let Some(current) = at.take() {
        guard_count += 1;
        if guard_count > 64 {
            return Err(MavenError::ParentChainTooDeep {
                coord: coord.to_string(),
            });
        }
        let Some(bytes) = registry.blob(&current.path("pom"))? else {
            break;
        };
        let pom = parse_xml(&bytes)?;

        if let Some(props) = pom.child("properties") {
            for property in &props.children {
                chain
                    .props
                    .entry(property.name.clone())
                    .or_insert_with(|| property.text.trim().to_string());
            }
        }
        if let Some(managed) = pom.child("dependencyManagement") {
            for dep in managed.find_all("dependency") {
                let (Some(group), Some(artifact)) =
                    (dep.text_of("groupId"), dep.text_of("artifactId"))
                else {
                    continue;
                };
                let group = interpolate(&group, &chain.props);
                let artifact = interpolate(&artifact, &chain.props);
                let version = dep.text_of("version").map(|v| interpolate(&v, &chain.props));
                // A BOM: its whole <dependencyManagement> is pulled in.
                // Without this, artifacts whose version lives only in a BOM
                // (kotlinx-coroutines) have no version and vanish.
                if dep.text_of("scope").as_deref() == Some("import") {
                    let Some(version) = version else { continue };
                    let bom = Coord::new(&group, &artifact, &version)?;
                    boms.insert(bom.to_string());
                    if let Some(bytes) = registry.blob(&bom.path("pom"))? {
                        let bom_pom = parse_xml(&bytes)?;
                        let bom_props = properties_of(&bom_pom);
                        for bd in bom_pom.find_all("dependency") {
                            let (Some(bg), Some(ba), Some(bv)) = (
                                bd.text_of("groupId"),
                                bd.text_of("artifactId"),
                                bd.text_of("version"),
                            ) else {
                                continue;
                            };
                            chain
                                .mgmt
                                .entry(format!(
                                    "{}:{}",
                                    interpolate(&bg, &bom_props),
                                    interpolate(&ba, &bom_props)
                                ))
                                .or_insert_with(|| interpolate(&bv, &bom_props));
                        }
                    }
                    continue;
                }
                if let Some(version) = version {
                    chain
                        .mgmt
                        .entry(format!("{group}:{artifact}"))
                        .or_insert(version);
                }
            }
        }

        let parent = pom.child("parent").and_then(|p| {
            let group = p.text_of("groupId")?;
            let artifact = p.text_of("artifactId")?;
            let version = p.text_of("version")?;
            Some((group, artifact, version))
        });
        chain.poms.push(pom);
        if let Some((group, artifact, version)) = parent {
            at = Some(Coord::new(&group, &artifact, &version)?);
        }
    }

    if chain.poms.is_empty() {
        return Ok(None);
    }
    // `${project.*}` refers to the artifact being resolved, not to whichever
    // POM in the chain declared the reference.
    chain
        .props
        .entry("project.version".to_string())
        .or_insert_with(|| coord.version.clone());
    chain
        .props
        .entry("project.groupId".to_string())
        .or_insert_with(|| coord.group.clone());
    chain
        .props
        .entry("project.artifactId".to_string())
        .or_insert_with(|| coord.artifact.clone());
    Ok(Some(chain))
}

fn properties_of(pom: &Element) -> BTreeMap<String, String> {
    let mut props = BTreeMap::new();
    if let Some(table) = pom.child("properties") {
        for property in &table.children {
            props.insert(property.name.clone(), property.text.trim().to_string());
        }
    }
    if let Some(version) = pom.text_of("version") {
        props.insert("project.version".to_string(), version);
    }
    props
}

/// Expand `${property}` references. Properties may name properties, so this
/// runs a few passes and then stops — an unexpanded `${...}` survives into
/// the coordinate and fails visibly rather than looping.
fn interpolate(value: &str, props: &BTreeMap<String, String>) -> String {
    let mut out = value.trim().to_string();
    for _ in 0..INTERP_PASSES {
        if !out.contains("${") {
            break;
        }
        for (key, replacement) in props {
            out = out.replace(&format!("${{{key}}}"), replacement);
        }
    }
    out
}

/// `[1.6.2]` is a HARD requirement meaning exactly that version — unwrap it.
/// Anything else in bracket form is a real range: this resolver has no
/// version solver, so it says so instead of guessing.
fn unwrap_range(version: &str) -> Result<String, String> {
    let version = version.trim();
    if !version.starts_with('[') && !version.starts_with('(') {
        return Ok(version.to_string());
    }
    let inner = version
        .trim_start_matches(['[', '('])
        .trim_end_matches([']', ')']);
    if version.starts_with('[') && version.ends_with(']') && !inner.contains(',') {
        return Ok(inner.trim().to_string());
    }
    Err(format!(
        "version range `{version}` needs a solver — pin the exact version instead"
    ))
}

// ---- materializing ----------------------------------------------------------

/// What kind of binary a coordinate publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An Android library: a zip holding `classes.jar`, `res/`, a manifest
    /// fragment and sometimes `jni/<abi>/*.so`.
    Aar,
    /// A plain JVM jar — code only, nothing to explode.
    Jar,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Aar => "aar",
            Kind::Jar => "jar",
        }
    }
}

/// One artifact on disk, with the parts a build actually consumes.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub coord: Coord,
    pub kind: Kind,
    /// The downloaded `.aar` / `.jar`.
    pub file: PathBuf,
    pub bytes: u64,
    /// Code for `d8`: an AAR's exploded `classes.jar`, or the jar itself.
    ///
    /// `None` for an AAR that ships no code. That is a REAL shape, not a
    /// corrupt download: a Kotlin-multiplatform artifact publishes a facade
    /// whose AAR holds only a manifest, and its POM — the compatibility shim
    /// Gradle is told to ignore — declares the platform variant
    /// (`tracing-android`) as an ordinary `compile` dependency. So the code
    /// arrives through the closure, and the facade contributes its manifest
    /// fragment. This is why ignoring `.module` metadata survives plain AAR
    /// consumption (`plans/aar.md` §2).
    pub classes: Option<PathBuf>,
    /// The AAR's `AndroidManifest.xml` fragment, when it has one. Merging
    /// these is the half this does not do yet — but a build cannot merge
    /// what it cannot find, so they are named.
    pub manifest: Option<PathBuf>,
    /// The AAR's `res/`, when non-empty.
    pub res: Option<PathBuf>,
    /// The AAR's `jni/`, when it ships native libraries.
    pub jni: Option<PathBuf>,
    /// `true` if this run downloaded it.
    pub fresh: bool,
}

/// Download every artifact in `closure` and explode the AARs.
///
/// An AAR is a zip; `classes.jar` inside it is what `d8` wants, so a
/// closure that is only downloaded is not yet usable. Explosion lands beside
/// the archive in `<artifact>-<version>/`, which keeps the surrounding
/// directory a valid Maven repo.
pub fn materialize(registry: &Registry, closure: &Closure) -> Result<Vec<Artifact>, MavenError> {
    let mut out = Vec::new();
    for coord in &closure.order {
        out.push(materialize_one(registry, coord)?);
    }
    Ok(out)
}

/// [`materialize`], but a coordinate that publishes no binary is REPORTED
/// rather than fatal. What pricing wants: the question "what would this
/// cost?" still has a useful answer when one artifact of thirty-five is
/// missing, and the missing one is named. Real I/O failures still propagate.
pub fn materialize_lenient(
    registry: &Registry,
    closure: &Closure,
) -> Result<(Vec<Artifact>, Vec<Unresolved>), MavenError> {
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for coord in &closure.order {
        match materialize_one(registry, coord) {
            Ok(artifact) => out.push(artifact),
            Err(MavenError::NoArtifact { coord, .. }) => missing.push(Unresolved {
                what: coord,
                reason: "no .aar or .jar published".to_string(),
            }),
            Err(other) => return Err(other),
        }
    }
    Ok((out, missing))
}

fn materialize_one(registry: &Registry, coord: &Coord) -> Result<Artifact, MavenError> {
    // AAR first: an Android library also publishes a `.jar` of nothing in
    // some repos, and the AAR is the one carrying resources.
    for kind in [Kind::Aar, Kind::Jar] {
        let path = coord.path(kind.ext());
        let fresh = !registry.local(&path).is_file();
        let Some(file) = registry.file(&path)? else {
            continue;
        };
        let bytes = fs::metadata(&file)
            .map_err(|source| MavenError::Io {
                path: file.clone(),
                source,
            })?
            .len();
        if kind == Kind::Jar {
            return Ok(Artifact {
                coord: coord.clone(),
                kind,
                classes: Some(file.clone()),
                file,
                bytes,
                manifest: None,
                res: None,
                jni: None,
                fresh,
            });
        }
        let exploded = coord
            .dir(&registry.root)
            .join(format!("{}-{}", coord.artifact, coord.version));
        explode(&file, &exploded)?;
        let some_if = |path: PathBuf| path.exists().then_some(path);
        return Ok(Artifact {
            coord: coord.clone(),
            kind,
            classes: some_if(exploded.join("classes.jar")),
            file,
            bytes,
            manifest: some_if(exploded.join("AndroidManifest.xml")),
            res: some_if(exploded.join("res")).filter(|p| has_entries(p)),
            jni: some_if(exploded.join("jni")).filter(|p| has_entries(p)),
            fresh,
        });
    }
    Err(MavenError::NoArtifact {
        coord: coord.to_string(),
        offline: registry.offline,
    })
}

/// Does a directory hold anything? An AAR that declares no resources still
/// ships `res/values/values.xml` at 63 bytes, so "the directory exists" is
/// not the question a build wants answered.
fn has_entries(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Unzip an AAR. Idempotent: an already-exploded archive is left alone, so a
/// repeat install does no work. An AAR always ships a manifest, so that is
/// the marker — `classes.jar` is not, because a code-less facade has none.
fn explode(archive: &Path, into: &Path) -> Result<(), MavenError> {
    if into.join("AndroidManifest.xml").is_file() {
        return Ok(());
    }
    fs::create_dir_all(into).map_err(|source| MavenError::Io {
        path: into.to_path_buf(),
        source,
    })?;
    let output = Command::new("unzip")
        .arg("-q")
        .arg("-o")
        .arg(archive)
        .arg("-d")
        .arg(into)
        .output()
        .map_err(|source| MavenError::NoUnzip { source })?;
    if !output.status.success() {
        return Err(MavenError::Explode {
            archive: archive.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    // Every AAR has a manifest — `classes.jar` it may legitimately lack (see
    // `Artifact::classes`). No manifest means the download is not the file
    // its extension claims, and every later step would be confusing.
    if !into.join("AndroidManifest.xml").is_file() {
        return Err(MavenError::Explode {
            archive: archive.to_path_buf(),
            stderr: "no AndroidManifest.xml inside — not an AAR".to_string(),
        });
    }
    Ok(())
}

// ---- errors -----------------------------------------------------------------

#[derive(Debug)]
pub enum MavenError {
    BadCoordinate {
        value: String,
        reason: String,
    },
    BadXml {
        reason: String,
    },
    ParentChainTooDeep {
        coord: String,
    },
    /// No `.aar` and no `.jar` for a coordinate whose POM resolved.
    NoArtifact {
        coord: String,
        offline: bool,
    },
    Explode {
        archive: PathBuf,
        stderr: String,
    },
    NoCurl {
        source: std::io::Error,
    },
    NoUnzip {
        source: std::io::Error,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The closure has entries that could not be resolved. Install refuses
    /// rather than materializing a partial one — a missing transitive
    /// artifact is a `NoClassDefFoundError` at runtime, which is the worst
    /// place to learn it.
    Incomplete {
        entries: Vec<Unresolved>,
    },
}

impl fmt::Display for MavenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MavenError::BadCoordinate { value, reason } => {
                write!(f, "invalid Maven coordinate `{value}`: {reason}")
            }
            MavenError::BadXml { reason } => write!(f, "malformed POM: {reason}"),
            MavenError::ParentChainTooDeep { coord } => write!(
                f,
                "`{coord}` has a <parent> chain over 64 deep (a cycle?)"
            ),
            MavenError::NoArtifact { coord, offline } => {
                write!(f, "no .aar or .jar published for `{coord}`")?;
                if *offline {
                    write!(f, " in the local repo (offline — run `cpc pm install`)")?;
                }
                Ok(())
            }
            MavenError::Explode { archive, stderr } => {
                write!(f, "failed to unpack {}: {stderr}", archive.display())
            }
            MavenError::NoCurl { source } => write!(
                f,
                "cannot run `curl` to fetch Maven artifacts: {source}"
            ),
            MavenError::NoUnzip { source } => {
                write!(f, "cannot run `unzip` to unpack an AAR: {source}")
            }
            MavenError::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            MavenError::Incomplete { entries } => {
                writeln!(f, "the Maven closure is incomplete:")?;
                for entry in entries {
                    writeln!(f, "  {} — {}", entry.what, entry.reason)?;
                }
                write!(
                    f,
                    "nothing was installed; pin the missing coordinates explicitly \
                     in [android.maven], or drop the dependency"
                )
            }
        }
    }
}

impl std::error::Error for MavenError {}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- coordinates --------------------------------------------------------

    #[test]
    fn coordinate_paths_follow_the_maven_layout() {
        let coord = Coord::parse("androidx.camera:camera-core:1.6.2").unwrap();
        assert_eq!(coord.group, "androidx.camera");
        assert_eq!(coord.artifact, "camera-core");
        assert_eq!(coord.ga(), "androidx.camera:camera-core");
        assert_eq!(
            coord.path("pom"),
            "androidx/camera/camera-core/1.6.2/camera-core-1.6.2.pom"
        );
        assert_eq!(
            coord.dir(Path::new("/m2")),
            PathBuf::from("/m2/androidx/camera/camera-core/1.6.2")
        );
    }

    #[test]
    fn group_artifact_and_version_split_separately() {
        let coord = Coord::from_ga("com.google.android.gms:play-services-maps", "19.0.0").unwrap();
        assert_eq!(coord.to_string(), "com.google.android.gms:play-services-maps:19.0.0");
    }

    #[test]
    fn traversal_in_a_coordinate_is_rejected() {
        // Every part becomes a path component under the store, and versions
        // arrive from DOWNLOADED POMs as well as from the manifest.
        for bad in [
            "..:x:1.0",
            "a..b:x:1.0",
            "com.x:..:1.0",
            "com.x:y:..",
            "com.x:y:../../etc",
            "com/x:y:1.0",
            "com.x:y:1.0/../..",
            ":y:1.0",
            "com.x::1.0",
            "com.x:y:",
        ] {
            assert!(
                matches!(Coord::parse(bad), Err(MavenError::BadCoordinate { .. })),
                "`{bad}` should be rejected"
            );
        }
        assert!(Coord::parse("com.x:y-z_1.2+w:1.0.0-beta01").is_ok());
    }

    #[test]
    fn a_coordinate_needs_all_three_parts() {
        assert!(matches!(
            Coord::parse("com.x:y"),
            Err(MavenError::BadCoordinate { .. })
        ));
        assert!(matches!(
            Coord::parse("com.x:y:1.0:extra"),
            Err(MavenError::BadCoordinate { .. })
        ));
    }

    // ---- XML ----------------------------------------------------------------

    #[test]
    fn parses_a_pom_with_the_shapes_that_actually_appear() {
        let pom = parse_xml(
            br#"<?xml version="1.0" encoding="UTF-8"?>
            <!-- generated, do not edit -->
            <project xmlns="http://maven.apache.org/POM/4.0.0">
              <groupId>com.x</groupId>
              <artifactId>y</artifactId>
              <version>1.0</version>
              <name><![CDATA[Y & friends]]></name>
              <description>a &amp; b &#65;</description>
              <properties>
                <kotlin.version>1.9.0</kotlin.version>
              </properties>
              <dependencies>
                <dependency>
                  <groupId>com.z</groupId>
                  <artifactId>z</artifactId>
                  <version>${kotlin.version}</version>
                </dependency>
                <dependency/>
              </dependencies>
            </project>"#,
        )
        .unwrap();
        assert_eq!(pom.name, "project");
        assert_eq!(pom.text_of("groupId").as_deref(), Some("com.x"));
        assert_eq!(pom.text_of("name").as_deref(), Some("Y & friends"));
        assert_eq!(pom.text_of("description").as_deref(), Some("a & b A"));
        assert_eq!(pom.find_all("dependency").len(), 2);
        assert_eq!(
            pom.child("properties").unwrap().text_of("kotlin.version"),
            Some("1.9.0".to_string())
        );
    }

    #[test]
    fn a_namespace_prefix_is_stripped_from_tag_names() {
        let pom = parse_xml(
            br#"<pom:project xmlns:pom="http://maven.apache.org/POM/4.0.0">
                  <pom:artifactId>y</pom:artifactId>
                </pom:project>"#,
        )
        .unwrap();
        assert_eq!(pom.name, "project");
        assert_eq!(pom.text_of("artifactId").as_deref(), Some("y"));
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_silent_truncation() {
        assert!(matches!(
            parse_xml(b"<project><a></b></project>"),
            Err(MavenError::BadXml { .. })
        ));
        assert!(matches!(
            parse_xml(b"<project><a>"),
            Err(MavenError::BadXml { .. })
        ));
        assert!(matches!(
            parse_xml(b"<!-- unterminated"),
            Err(MavenError::BadXml { .. })
        ));
        assert!(matches!(parse_xml(b"   "), Err(MavenError::BadXml { .. })));
    }

    // ---- versions -----------------------------------------------------------

    #[test]
    fn a_hard_range_is_exactly_that_version() {
        assert_eq!(unwrap_range("[1.6.2]").unwrap(), "1.6.2");
        assert_eq!(unwrap_range("1.6.2").unwrap(), "1.6.2");
    }

    #[test]
    fn an_open_range_says_so_instead_of_guessing() {
        for range in ["[1.0,2.0)", "(1.0,2.0]", "[1.0,)", "[1.0,2.0]"] {
            let error = unwrap_range(range).unwrap_err();
            assert!(error.contains("solver"), "{range}: {error}");
        }
    }

    #[test]
    fn interpolation_expands_nested_properties_and_stops() {
        let props: BTreeMap<String, String> = [
            ("a".to_string(), "${b}".to_string()),
            ("b".to_string(), "1.0".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(interpolate("${a}", &props), "1.0");
        // An unknown property survives verbatim and fails as a coordinate,
        // rather than looping or silently becoming empty.
        assert_eq!(interpolate("${nope}", &props), "${nope}");
    }

    // ---- resolution, over a fixture repo ------------------------------------

    /// Write one POM into a local repo laid out the Maven way.
    fn pom(root: &Path, coord: &str, body: &str) {
        let coord = Coord::parse(coord).unwrap();
        let path = root.join(coord.path("pom").replace('/', std::path::MAIN_SEPARATOR_STR));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(r#"<project xmlns="http://maven.apache.org/POM/4.0.0">{body}</project>"#),
        )
        .unwrap();
    }

    fn offline(root: &Path) -> Registry {
        Registry::new(root.to_path_buf()).offline(true)
    }

    #[test]
    fn walks_a_transitive_closure_breadth_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:app:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>lib</artifactId><version>2.0</version></dependency>
             </dependencies>",
        );
        pom(
            root,
            "com.x:lib:2.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>leaf</artifactId><version>3.0</version></dependency>
             </dependencies>",
        );
        pom(root, "com.x:leaf:3.0", "");

        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert!(closure.is_complete(), "{:?}", closure.unresolved);
        assert_eq!(
            closure.order.iter().map(Coord::to_string).collect::<Vec<_>>(),
            ["com.x:app:1.0", "com.x:lib:2.0", "com.x:leaf:3.0"]
        );
    }

    #[test]
    fn test_and_provided_scopes_and_optionals_stay_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:app:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>junit</artifactId><version>1.0</version><scope>test</scope></dependency>
               <dependency><groupId>com.x</groupId><artifactId>api</artifactId><version>1.0</version><scope>provided</scope></dependency>
               <dependency><groupId>com.x</groupId><artifactId>maybe</artifactId><version>1.0</version><optional>true</optional></dependency>
               <dependency><groupId>com.x</groupId><artifactId>rt</artifactId><version>1.0</version><scope>runtime</scope></dependency>
             </dependencies>",
        );
        pom(root, "com.x:rt:1.0", "");
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert_eq!(
            closure.order.iter().map(Coord::to_string).collect::<Vec<_>>(),
            ["com.x:app:1.0", "com.x:rt:1.0"]
        );
    }

    #[test]
    fn a_parent_chain_supplies_properties_and_managed_versions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:parent:1.0",
            "<properties><lib.version>2.0</lib.version></properties>
             <dependencyManagement><dependencies>
               <dependency><groupId>com.x</groupId><artifactId>managed</artifactId><version>${lib.version}</version></dependency>
             </dependencies></dependencyManagement>",
        );
        pom(
            root,
            "com.x:app:1.0",
            "<parent><groupId>com.x</groupId><artifactId>parent</artifactId><version>1.0</version></parent>
             <dependencies>
               <dependency><groupId>com.x</groupId><artifactId>managed</artifactId></dependency>
               <dependency><groupId>com.x</groupId><artifactId>interp</artifactId><version>${lib.version}</version></dependency>
             </dependencies>",
        );
        pom(root, "com.x:managed:2.0", "");
        pom(root, "com.x:interp:2.0", "");
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert!(closure.is_complete(), "{:?}", closure.unresolved);
        assert_eq!(
            closure.order.iter().map(Coord::to_string).collect::<Vec<_>>(),
            ["com.x:app:1.0", "com.x:managed:2.0", "com.x:interp:2.0"]
        );
    }

    #[test]
    fn a_bom_import_supplies_versions_that_exist_nowhere_else() {
        // The coroutines failure mode: drop <scope>import</scope> and these
        // artifacts have no version and vanish from the closure silently.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.k:bom:1.9.0",
            "<dependencyManagement><dependencies>
               <dependency><groupId>com.k</groupId><artifactId>coroutines</artifactId><version>1.9.0</version></dependency>
             </dependencies></dependencyManagement>",
        );
        pom(
            root,
            "com.x:app:1.0",
            "<dependencyManagement><dependencies>
               <dependency><groupId>com.k</groupId><artifactId>bom</artifactId><version>1.9.0</version><type>pom</type><scope>import</scope></dependency>
             </dependencies></dependencyManagement>
             <dependencies>
               <dependency><groupId>com.k</groupId><artifactId>coroutines</artifactId></dependency>
             </dependencies>",
        );
        pom(root, "com.k:coroutines:1.9.0", "");
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert!(closure.is_complete(), "{:?}", closure.unresolved);
        assert_eq!(closure.bom_imports, ["com.k:bom:1.9.0"]);
        assert!(closure
            .order
            .iter()
            .any(|c| c.to_string() == "com.k:coroutines:1.9.0"));
    }

    #[test]
    fn nearest_wins_and_the_loser_is_named() {
        // Maven's rule, NOT Gradle's highest-wins. Documented divergence,
        // so it is pinned by a test rather than left to be rediscovered.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:app:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>shared</artifactId><version>1.0</version></dependency>
               <dependency><groupId>com.x</groupId><artifactId>other</artifactId><version>1.0</version></dependency>
             </dependencies>",
        );
        pom(
            root,
            "com.x:other:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>shared</artifactId><version>9.0</version></dependency>
             </dependencies>",
        );
        pom(root, "com.x:shared:1.0", "");
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert!(closure
            .order
            .iter()
            .any(|c| c.to_string() == "com.x:shared:1.0"));
        assert!(!closure
            .order
            .iter()
            .any(|c| c.to_string() == "com.x:shared:9.0"));
        assert_eq!(closure.conflicts.len(), 1, "{:?}", closure.conflicts);
        assert_eq!(closure.conflicts[0].ga, "com.x:shared");
        assert_eq!(closure.conflicts[0].kept, "1.0");
        assert_eq!(closure.conflicts[0].dropped, ["9.0"]);
        // Nearest kept the OLDER one, so Gradle's highest-wins would differ:
        // exactly the case worth a warning.
        assert!(closure.conflicts[0].divergent);
        assert_eq!(closure.divergent().count(), 1);
    }

    #[test]
    fn a_conflict_that_already_kept_the_highest_is_not_divergent() {
        // Nearest-wins picked the newest anyway — Gradle would land in the
        // same place, so there is nothing to warn about. Eight of these per
        // CameraX resolve is why the report aggregates.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:app:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>shared</artifactId><version>2.0</version></dependency>
               <dependency><groupId>com.x</groupId><artifactId>other</artifactId><version>1.0</version></dependency>
             </dependencies>",
        );
        pom(
            root,
            "com.x:other:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>shared</artifactId><version>1.0</version></dependency>
             </dependencies>",
        );
        pom(root, "com.x:shared:2.0", "");
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert_eq!(closure.conflicts.len(), 1);
        assert!(!closure.conflicts[0].divergent);
        assert_eq!(closure.divergent().count(), 0);
    }

    #[test]
    fn version_comparison_answers_would_gradle_differ() {
        assert!(is_newer("2.0", "1.9"));
        assert!(is_newer("1.10", "1.9"));   // numeric, not lexicographic
        assert!(is_newer("23.0.0", "13.0"));
        assert!(is_newer("1.2.1", "1.2"));  // a longer version is newer
        assert!(!is_newer("1.2", "1.2.1"));
        assert!(!is_newer("1.0", "1.0"));
        assert!(!is_newer("1.7.10", "2.1.20"));
        // A pre-release compares ABOVE its release under the textual
        // fallback. Deliberate: the answer errs toward reporting.
        assert!(is_newer("1.0.0-alpha01", "1.0.0"));
    }

    #[test]
    fn a_missing_pom_and_a_versionless_dep_are_both_reported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:app:1.0",
            "<dependencies>
               <dependency><groupId>com.x</groupId><artifactId>gone</artifactId><version>1.0</version></dependency>
               <dependency><groupId>com.x</groupId><artifactId>nover</artifactId></dependency>
               <dependency><groupId>com.x</groupId><artifactId>ranged</artifactId><version>[1.0,2.0)</version></dependency>
             </dependencies>",
        );
        let closure = resolve(&offline(root), &[Coord::parse("com.x:app:1.0").unwrap()]).unwrap();
        assert!(!closure.is_complete());
        let reported: Vec<String> = closure
            .unresolved
            .iter()
            .map(|u| format!("{} — {}", u.what, u.reason))
            .collect();
        assert!(
            reported.iter().any(|r| r.contains("nover") && r.contains("no version")),
            "{reported:?}"
        );
        assert!(
            reported.iter().any(|r| r.contains("ranged") && r.contains("solver")),
            "{reported:?}"
        );
        // Offline, so the reason names the local repo and the fix rather
        // than implying the coordinate does not exist upstream.
        assert!(
            reported
                .iter()
                .any(|r| r.contains("gone") && r.contains("offline") && r.contains("cpc pm install")),
            "{reported:?}"
        );
    }

    #[test]
    fn a_parent_cycle_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        pom(
            root,
            "com.x:a:1.0",
            "<parent><groupId>com.x</groupId><artifactId>b</artifactId><version>1.0</version></parent>",
        );
        pom(
            root,
            "com.x:b:1.0",
            "<parent><groupId>com.x</groupId><artifactId>a</artifactId><version>1.0</version></parent>",
        );
        assert!(matches!(
            resolve(&offline(root), &[Coord::parse("com.x:a:1.0").unwrap()]),
            Err(MavenError::ParentChainTooDeep { .. })
        ));
    }

    #[test]
    fn offline_never_reaches_the_network() {
        // No fixture at all: an offline registry must report a miss rather
        // than try (and, on a machine with a network, succeed).
        let dir = tempfile::tempdir().unwrap();
        let registry = offline(dir.path());
        assert!(registry
            .blob("androidx/core/core/1.3.2/core-1.3.2.pom")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_local_repo_url_fills_the_cache() {
        // `file://` exercises the real download path — one repo answers, the
        // cache is written, and a second read is served locally.
        let remote = tempfile::tempdir().unwrap();
        pom(remote.path(), "com.x:app:1.0", "<packaging>jar</packaging>");
        let cache = tempfile::tempdir().unwrap();
        let mut registry = Registry::new(cache.path().to_path_buf());
        registry.repos = vec![
            format!("file://{}", remote.path().display()),
        ];
        let path = "com/x/app/1.0/app-1.0.pom";
        assert!(registry.blob(path).unwrap().is_some());
        assert!(cache.path().join(path).is_file());
        // Now offline: the cached copy answers.
        let offline = Registry::new(cache.path().to_path_buf()).offline(true);
        assert!(offline.blob(path).unwrap().is_some());
        // And a coordinate the remote never had stays a miss, with no
        // `.part` file left behind.
        assert!(registry.blob("com/x/gone/1.0/gone-1.0.pom").unwrap().is_none());
        assert!(!cache.path().join("com/x/gone/1.0").exists()
            || fs::read_dir(cache.path().join("com/x/gone/1.0"))
                .unwrap()
                .next()
                .is_none());
    }
}
