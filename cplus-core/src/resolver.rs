//! Multi-file resolver (Phase 4 slice 4A).
//!
//! Walks the import graph from the entry file, parses every reached
//! `.cplus` file, and produces a single combined `Program` for sema/codegen
//! to chew on. Item names are qualified with a per-file prefix (the
//! "file id") so that two files can define an item with the same source
//! name without colliding in the merged symbol table. The entry binary's
//! `fn main()` is the one exception — it stays un-prefixed so the linker
//! finds it as `@main`.
//!
//! See `docs/design/phase4-modules.md` §8 for the slice plan and §8.1/§8.2
//! for the AST/codegen-level intent.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::lexer::Span;

/// A per-file unit after parsing, before resolution.
#[derive(Debug, Clone)]
pub struct FileUnit {
    /// Stable, dot-separated identifier derived from the file's path
    /// relative to the manifest root (`src/foo/bar.cplus` → `src.foo.bar`).
    /// Items declared in this file are mangled `{file_id}.{name}`.
    pub file_id: String,
    /// Absolute, canonicalized path on disk. The import-graph walk uses
    /// this as the deduplication key — two `import` declarations resolving
    /// to the same canonical path are the same file.
    pub canonical_path: PathBuf,
    /// Source text — kept on the unit so sema/diagnostics can read spans.
    pub source: String,
    /// AST as the parser produced it. Imports are still attached; the
    /// resolver consumes them when rewriting and the merged Program drops
    /// them.
    pub program: Program,
}

#[derive(Debug)]
pub enum ResolveError {
    /// The import string did not resolve to an existing file. (E0401.)
    ImportNotFound {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
        resolved: PathBuf,
    },
    /// Two `import` declarations in the same file share an `as` prefix. (E0405.)
    DuplicatePrefix {
        file: PathBuf,
        prefix: String,
        first_span: Span,
        second_span: Span,
    },
    /// A `prefix::...` path references an unknown prefix. (E0402-adjacent —
    /// surfaced at sema time normally; we catch it during rewrite so the
    /// error mentions the prefix specifically.)
    UnknownPrefix {
        file: PathBuf,
        span: Span,
        prefix: String,
    },
    /// Cyclic import dependency. (E0404 — wired in slice 4C.)
    Cycle {
        chain: Vec<PathBuf>,
    },
    /// Cross-file reference to a non-`pub` item. (E0403, slice 4B.)
    /// `kind` distinguishes the surface form so the message can use the
    /// right phrasing — function, struct, enum, method, or field.
    PrivateAccess {
        file: PathBuf,
        span: Span,
        kind: PrivateKind,
        owner: String,    // for methods: the type name; for fields: the struct; else the file id
        name: String,     // the item being denied
    },
    /// Generic I/O error while reading a `.cplus` file the import graph
    /// reaches. Distinct from `ImportNotFound`: the file exists but
    /// couldn't be read (permission denied, etc.).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A parser error in a non-entry file. Wrapped so the caller can
    /// attribute it to the right source.
    Parse {
        path: PathBuf,
        source: crate::parser::ParseError,
    },
    /// A lexer error in a non-entry file.
    Lex {
        path: PathBuf,
        source: crate::lexer::LexError,
    },
    /// Phase 2 (E0852): a vendor import's first segment isn't a declared
    /// dependency in `Cplus.toml`. Example: `import "stdlib/io"` when
    /// `[dependencies]` contains no `stdlib` entry.
    UnknownPackage {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
        package: String,
    },
    /// Phase 2 (E0853): a bare path that isn't `./`/`../`-prefixed AND
    /// whose first segment isn't a declared dependency. Either the user
    /// forgot the `./` (local-file case) or forgot the dependency
    /// declaration (vendor-package case). The diagnostic suggests both.
    BareImport {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
    },
    /// Phase 2: the import path carries a `.cplus` extension. Slice 2B
    /// canonicalizes import paths to extension-less form so the same
    /// string works for both local and vendor modes. The migration is
    /// mechanical: drop the trailing `.cplus`.
    StaleExtension {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
    },
    /// Phase 2: a vendor import contains a `..` segment that would
    /// escape `vendor/<pkg>/src/`. Security: a package can't reach
    /// outside its own directory via static imports.
    VendorEscape {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum PrivateKind { Function, Struct, Enum, Method, Interface, TypeAlias }

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::ImportNotFound { importing_file, requested, resolved, .. } => {
                write!(f, "[E0401] {}: import `{requested}` not found (resolved to {})",
                    importing_file.display(), resolved.display())
            }
            ResolveError::DuplicatePrefix { file, prefix, .. } => {
                write!(f, "[E0405] {}: duplicate import prefix `{prefix}`", file.display())
            }
            ResolveError::UnknownPrefix { file, prefix, .. } => {
                write!(f, "[E0402] {}: unknown import prefix `{prefix}`", file.display())
            }
            ResolveError::Cycle { chain } => {
                let chain_str: Vec<String> = chain.iter().map(|p| p.display().to_string()).collect();
                write!(f, "[E0404] cyclic import: {}", chain_str.join(" -> "))
            }
            ResolveError::PrivateAccess { file, kind, owner, name, .. } => {
                let what = match kind {
                    PrivateKind::Function => "function",
                    PrivateKind::Struct => "struct",
                    PrivateKind::Enum => "enum",
                    PrivateKind::Method => "method",
                    PrivateKind::Interface => "interface",
                    PrivateKind::TypeAlias => "type alias",
                };
                match kind {
                    PrivateKind::Method => write!(
                        f,
                        "[E0403] {}: {what} `{owner}::{name}` is private (mark it `pub` in its declaration to export)",
                        file.display(),
                    ),
                    _ => write!(
                        f,
                        "[E0403] {}: {what} `{name}` (in module `{owner}`) is private (mark it `pub` in its declaration to export)",
                        file.display(),
                    ),
                }
            }
            ResolveError::Io { path, source } => {
                write!(f, "I/O error reading {}: {source}", path.display())
            }
            ResolveError::Parse { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            ResolveError::Lex { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            ResolveError::UnknownPackage { importing_file, requested, package, .. } => {
                write!(
                    f,
                    "[E0852] {}: import `{requested}` — first segment `{package}` is not a declared dependency in `Cplus.toml`",
                    importing_file.display(),
                )
            }
            ResolveError::BareImport { importing_file, requested, .. } => {
                write!(
                    f,
                    "[E0853] {}: bare import `{requested}` — paths must start with `./`/`../` for file-relative or match a declared `[dependencies]` entry",
                    importing_file.display(),
                )
            }
            ResolveError::StaleExtension { importing_file, requested, .. } => {
                write!(
                    f,
                    "[E0858] {}: import `{requested}` has a `.cplus` extension — drop the extension (Phase 2 imports are extension-less)",
                    importing_file.display(),
                )
            }
            ResolveError::VendorEscape { importing_file, requested, .. } => {
                write!(
                    f,
                    "[E0859] {}: vendor import `{requested}` contains `..` — packages cannot reach outside their own `src/` directory",
                    importing_file.display(),
                )
            }
        }
    }
}

/// Public entry point: read the entry binary and every transitively-imported
/// file, then produce a single merged `Program` for the existing
/// sema/codegen pipeline. The `manifest_root` is used to derive file ids
/// (relative-to-root, dot-separated).
///
/// On failure returns a `LoadFailure` carrying both the error and the
/// per-file source map collected up to the failure point — so the driver
/// can render the diagnostic with the right path / line/col / source
/// snippet via `LoadFailure::to_diagnostic`.
pub fn load_project(
    entry_path: &Path,
    manifest_root: &Path,
) -> Result<LoadedProject, LoadFailure> {
    load_project_with_mode(entry_path, manifest_root, false)
}

/// Phase 5 Slice 5.A: like `load_project` but allows the caller to mark
/// this project as a library target. When `is_lib = true`, top-level
/// items in the entry file keep unqualified names so a C consumer can
/// link against them by their source-level identifier.
pub fn load_project_with_mode(
    entry_path: &Path,
    manifest_root: &Path,
    is_lib: bool,
) -> Result<LoadedProject, LoadFailure> {
    // Pre-2B compat: `None` → single-file mode (file-relative imports).
    load_project_full(entry_path, manifest_root, is_lib, None)
}

/// Phase 2 Slice 2B: full-fledged entry point taking the consumer's
/// declared `[dependencies]` names. Vendor imports (`stdlib/io` etc.)
/// resolve under `<manifest_root>/vendor/<name>/src/`; imports whose
/// first segment isn't in `deps` fail with E0852/E0853 depending on
/// shape. Source-only callers that don't know about deps yet can pass
/// `&[]` to get the pre-Slice-2B behavior (everything is local-relative
/// and `.cplus` extensions are still allowed for backward compat).
pub fn load_project_full(
    entry_path: &Path,
    manifest_root: &Path,
    is_lib: bool,
    deps: Option<&[String]>,
) -> Result<LoadedProject, LoadFailure> {
    // `None` = legacy single-file mode (no manifest); bare imports fall
    // through to file-relative for backward compat. `Some([])` = project
    // mode with no deps; the strict vendor rules apply so bare paths
    // immediately surface E0853 instead of silently scanning for files.
    let (dep_set, project_mode): (BTreeSet<String>, bool) = match deps {
        None => (BTreeSet::new(), false),
        Some(d) => (d.iter().cloned().collect(), true),
    };
    let loader_deps_snapshot = dep_set.clone();
    let mut loader = Loader::with_deps(manifest_root.to_path_buf(), dep_set);
    loader.project_mode = project_mode;
    let entry_file_id = match loader.load_recursive(entry_path, None, None) {
        Ok(id) => id,
        Err(e) => return Err(LoadFailure::new(e, &loader)),
    };
    let LoaderState { files, edges } = loader.into_state();

    if let Err(e) = detect_cycle(&entry_file_id, &edges, &files) {
        let sources = files.iter()
            .map(|(_, u)| (u.canonical_path.clone(), u.source.clone()))
            .collect();
        return Err(LoadFailure { error: e, sources });
    }

    // Snapshot per-file (path, source) before `merge` consumes `files`.
    let file_sources: std::collections::BTreeMap<String, (PathBuf, String)> = files
        .iter()
        .map(|(fid, u)| (fid.clone(), (u.canonical_path.clone(), u.source.clone())))
        .collect();
    // Also keyed by canonical path for the failure path.
    let sources_by_path: std::collections::BTreeMap<PathBuf, String> = files
        .iter()
        .map(|(_, u)| (u.canonical_path.clone(), u.source.clone()))
        .collect();

    let merged = match merge(files, &entry_file_id, is_lib, manifest_root, &loader_deps_snapshot, project_mode) {
        Ok(p) => p,
        Err(e) => return Err(LoadFailure { error: e, sources: sources_by_path }),
    };
    Ok(LoadedProject {
        program: merged,
        entry_file_id,
        files: file_sources,
    })
}

/// Bundle a `ResolveError` with the per-file source map collected so far
/// — needed to render the error as a structured `Diagnostic` with proper
/// line/column attribution and source-snippet context.
#[derive(Debug)]
pub struct LoadFailure {
    pub error: ResolveError,
    /// Canonicalized path → file source. Populated for every file the
    /// loader had successfully parsed before the error fired. Empty if
    /// the error happened before any file was read (e.g. entry doesn't
    /// exist).
    pub sources: std::collections::BTreeMap<PathBuf, String>,
}

impl LoadFailure {
    fn new(error: ResolveError, loader: &Loader) -> Self {
        let sources = loader.files.iter()
            .map(|(_, u)| (u.canonical_path.clone(), u.source.clone()))
            .collect();
        Self { error, sources }
    }

    /// Path of the file the primary diagnostic span belongs to, if any.
    /// Used by the driver to pick the right source for `render_human`'s
    /// snippet line.
    pub fn primary_path(&self) -> Option<&Path> {
        match &self.error {
            ResolveError::ImportNotFound { importing_file, .. } => Some(importing_file),
            ResolveError::DuplicatePrefix { file, .. } => Some(file),
            ResolveError::UnknownPrefix { file, .. } => Some(file),
            ResolveError::PrivateAccess { file, .. } => Some(file),
            ResolveError::Cycle { chain } => chain.first().map(|p| p.as_path()),
            ResolveError::Parse { path, .. } => Some(path),
            ResolveError::Lex { path, .. } => Some(path),
            ResolveError::Io { path, .. } => Some(path),
            ResolveError::UnknownPackage { importing_file, .. } => Some(importing_file),
            ResolveError::BareImport { importing_file, .. } => Some(importing_file),
            ResolveError::StaleExtension { importing_file, .. } => Some(importing_file),
            ResolveError::VendorEscape { importing_file, .. } => Some(importing_file),
        }
    }

    /// Source of the file `primary_path()` points to, if we have it.
    pub fn primary_source(&self) -> Option<&str> {
        let p = self.primary_path()?;
        // Try canonical first, then fall back to the raw path.
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        self.sources.get(&canon).map(|s| s.as_str())
            .or_else(|| self.sources.get(p).map(|s| s.as_str()))
    }

    /// Render this failure as a structured `Diagnostic`. Routes the span
    /// through the primary file's line-map so JSON/short/human renderers
    /// all see the right (file, line, col).
    pub fn to_diagnostic(&self) -> crate::diagnostics::Diagnostic {
        use crate::diagnostics::{Applicability, DiagCode, Diagnostic, LineMap, Position, Severity, SourceSpan, Suggestion};

        // Helper: build a SourceSpan for `(path, span)` using whatever
        // source we have. If no source is available (rare — file went
        // missing between read and error), fall back to a degenerate
        // position-only span.
        let span_in = |path: &Path, span: Span| -> SourceSpan {
            if let Some(src) = self.sources.get(path) {
                let lm = LineMap::new(src);
                lm.span(&path.to_path_buf(), span, src)
            } else {
                SourceSpan {
                    file: path.to_path_buf(),
                    start: Position { line: 1, col: 1, byte: span.start },
                    end: Position { line: 1, col: 1, byte: span.end },
                }
            }
        };
        // Helper for errors whose "primary location" is just a path with
        // no useful span (manifest entry missing, I/O errors before a
        // file is read).
        let pathless_span = |path: &Path| -> SourceSpan {
            SourceSpan {
                file: path.to_path_buf(),
                start: Position { line: 1, col: 1, byte: 0 },
                end: Position { line: 1, col: 1, byte: 0 },
            }
        };

        let mut suggestions: Vec<Suggestion> = Vec::new();
        let mut notes: Vec<String> = Vec::new();

        let (code, message, primary): (&'static str, String, SourceSpan) = match &self.error {
            ResolveError::ImportNotFound { importing_file, import_span, requested, resolved } => {
                // Did-you-mean: scan the importing file's directory tree
                // for `.cplus` files and suggest the closest basename.
                if let Some(close) = closest_cplus(importing_file, requested) {
                    suggestions.push(Suggestion {
                        description: format!("did you mean `{close}`?"),
                        span: span_in(importing_file, *import_span),
                        replacement: format!("\"{close}\""),
                        applicability: Applicability::MaybeIncorrect,
                    });
                }
                notes.push(format!("resolved to `{}`", resolved.display()));
                (
                    "E0401",
                    format!("imported file `{requested}` not found"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::DuplicatePrefix { file, prefix, first_span, second_span } => {
                notes.push("each `import` must use a distinct `as` name".to_string());
                let primary = span_in(file, *second_span);
                // Point at the first import as well, via a note (Label
                // would also fit but we keep it simple).
                let first = span_in(file, *first_span);
                notes.push(format!("first import at {}:{}:{}", first.file.display(), first.start.line, first.start.col));
                (
                    "E0405",
                    format!("duplicate import prefix `{prefix}`"),
                    primary,
                )
            }
            ResolveError::UnknownPrefix { file, span, prefix } => {
                (
                    "E0402",
                    format!("unknown import prefix `{prefix}`"),
                    span_in(file, *span),
                )
            }
            ResolveError::Cycle { chain } => {
                let chain_str: Vec<String> = chain.iter().map(|p| p.display().to_string()).collect();
                notes.push(format!("cycle: {}", chain_str.join(" -> ")));
                let primary = chain.first().map(|p| pathless_span(p))
                    .unwrap_or_else(|| pathless_span(Path::new("<unknown>")));
                (
                    "E0404",
                    "cyclic import dependency".to_string(),
                    primary,
                )
            }
            ResolveError::PrivateAccess { file, span, kind, owner, name } => {
                let what = match kind {
                    PrivateKind::Function => "function",
                    PrivateKind::Struct => "struct",
                    PrivateKind::Enum => "enum",
                    PrivateKind::Method => "method",
                    PrivateKind::Interface => "interface",
                    PrivateKind::TypeAlias => "type alias",
                };
                let msg = match kind {
                    PrivateKind::Method => format!(
                        "{what} `{owner}::{name}` is private (mark it `pub` in its declaration to export)",
                    ),
                    _ => format!(
                        "{what} `{name}` is private (mark it `pub` in `{owner}` to export)",
                    ),
                };
                ("E0403", msg, span_in(file, *span))
            }
            ResolveError::Io { path, source } => {
                ("E0401", format!("I/O error reading `{}`: {source}", path.display()), pathless_span(path))
            }
            ResolveError::Parse { path, source } => {
                // Parse / lex errors in non-entry files already carry their
                // own structured shape; rewrap minimally for now. (A full
                // wrap would re-export ParseError's E01xx codes; this stays
                // a 4C polish item that comes for free with proper sema-side
                // structured handling. Use the parse error's span.)
                let primary = span_in(path, source.span);
                ("E01XX", format!("{source}"), primary)
            }
            ResolveError::Lex { path, source } => {
                let primary = span_in(path, source.span);
                ("E00XX", format!("{source}"), primary)
            }
            ResolveError::UnknownPackage { importing_file, import_span, requested, package } => {
                notes.push(format!(
                    "add `{package} = \"*\"` to `[dependencies]` in `Cplus.toml`, or change the import to `./{requested}` for a file-relative path"
                ));
                (
                    "E0852",
                    format!("import `{requested}`: first segment `{package}` is not a declared dependency"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::BareImport { importing_file, import_span, requested } => {
                suggestions.push(Suggestion {
                    description: "use `./` for a file-relative import".to_string(),
                    span: span_in(importing_file, *import_span),
                    replacement: format!("\"./{requested}\""),
                    applicability: Applicability::MaybeIncorrect,
                });
                notes.push(
                    "or add the package to `[dependencies]` in `Cplus.toml` if you intended a vendor import".to_string()
                );
                (
                    "E0853",
                    format!("bare import `{requested}` is not `./`/`../`-prefixed and `{requested}`'s first segment isn't a declared dependency"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::StaleExtension { importing_file, import_span, requested } => {
                let stripped = requested.trim_end_matches(".cplus");
                suggestions.push(Suggestion {
                    description: "drop the `.cplus` extension".to_string(),
                    span: span_in(importing_file, *import_span),
                    replacement: format!("\"{stripped}\""),
                    applicability: Applicability::MachineApplicable,
                });
                (
                    "E0858",
                    format!("import `{requested}` has a `.cplus` extension — Phase 2 imports are extension-less"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::VendorEscape { importing_file, import_span, requested } => {
                notes.push(
                    "packages cannot reach files outside their own `src/` directory via static imports".to_string()
                );
                (
                    "E0859",
                    format!("vendor import `{requested}` contains `..`"),
                    span_in(importing_file, *import_span),
                )
            }
        };

        Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message,
            primary,
            labels: Vec::new(),
            notes,
            suggestions,
        }
    }
}

/// Scan the directory tree rooted at `importing_file`'s parent for any
/// `.cplus` files whose basename is close (edit distance ≤ 2 or one of
/// them is a strict prefix of the other) to `requested`'s basename.
/// Returns the closest match if any. Used to power the E0401 did-you-mean
/// suggestion. Bounded: we scan only the immediate directory of the
/// importing file plus one level down — this catches the
/// "math.cplus" vs "maths.cplus" typo without spelunking into the project
/// tree.
fn closest_cplus(importing_file: &Path, requested: &str) -> Option<String> {
    let want = Path::new(requested);
    let want_basename = want.file_name()?.to_string_lossy().to_string();
    // Phase 2: import paths are extension-less. Strip a stale `.cplus`
    // (if the user wrote it) so the edit-distance comparison against
    // on-disk basenames (which keep the extension) is symmetric.
    let want_stem = want_basename.strip_suffix(".cplus").unwrap_or(&want_basename).to_string();
    let dir = importing_file.parent()?;
    let mut candidates: Vec<(String, String)> = Vec::new();
    push_cplus_files(dir, dir, &mut candidates, 0);
    let mut best: Option<(usize, String)> = None;
    for (rel, basename) in &candidates {
        // Compare stem-to-stem so the distance reflects what the user
        // would re-type, not the on-disk extension.
        let basename_stem = basename.strip_suffix(".cplus").unwrap_or(basename);
        let d = edit_distance(&want_stem, basename_stem);
        if d > 2 { continue; }
        match &best {
            None => best = Some((d, rel.clone())),
            Some((bd, _)) if d < *bd => best = Some((d, rel.clone())),
            _ => {}
        }
    }
    best.map(|(_, rel)| rel)
}

fn push_cplus_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && depth < 1 {
            push_cplus_files(root, &p, out, depth + 1);
        } else if p.is_file() {
            let Some(ext) = p.extension() else { continue; };
            if ext != "cplus" { continue; }
            let basename = p.file_name().unwrap().to_string_lossy().to_string();
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string();
            out.push((rel, basename));
        }
    }
}

/// Classic dynamic-programming Levenshtein distance. Bounded by string
/// length; suggestion candidates are short basenames so this is cheap.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i-1] == b[j-1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j-1] + 1).min(prev[j-1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[derive(Debug)]
pub struct LoadedProject {
    pub program: Program,
    pub entry_file_id: String,
    /// Per-file context for downstream diagnostics (slice 4C). The
    /// resolver has already read each file's source while parsing; we
    /// hand it off to sema so cross-file errors can render with the
    /// originating file's path + line/col rather than the entry file's.
    /// Keyed on the same file id the resolver bakes into qualified item
    /// names (`src.math`, etc.), so sema looks up by `current_file`
    /// without further plumbing.
    pub files: std::collections::BTreeMap<String, (PathBuf, String)>,
}

// ---------------- internals ----------------

struct Loader {
    manifest_root: PathBuf,
    files: BTreeMap<String, FileUnit>,           // file_id → unit
    by_canonical: BTreeMap<PathBuf, String>,     // canonical_path → file_id
    edges: BTreeMap<String, Vec<String>>,        // file_id → imported file_ids
    /// Phase 2 Slice 2B: declared dependencies (consumer's `[dependencies]`).
    /// Used to classify vendor imports — bare paths whose first segment is
    /// in this set resolve under `vendor/<name>/src/`; others fail with
    /// E0852/E0853 depending on shape.
    deps: BTreeSet<String>,
    /// Phase 2 Slice 2B: `true` when a `Cplus.toml` exists and the
    /// caller has threaded its (possibly empty) `[dependencies]` list.
    /// Drives strict vendor-mode classification — bare imports become
    /// E0853 instead of falling through to file-relative resolution.
    /// `false` for single-file mode (`cpc FILE.cplus -o BIN`).
    project_mode: bool,
}

struct LoaderState {
    files: BTreeMap<String, FileUnit>,
    edges: BTreeMap<String, Vec<String>>,
}

impl Loader {
    fn new(manifest_root: PathBuf) -> Self {
        Self::with_deps(manifest_root, BTreeSet::new())
    }

    fn with_deps(manifest_root: PathBuf, deps: BTreeSet<String>) -> Self {
        Self {
            manifest_root,
            files: BTreeMap::new(),
            by_canonical: BTreeMap::new(),
            edges: BTreeMap::new(),
            deps,
            project_mode: false,
        }
    }

    fn into_state(self) -> LoaderState {
        LoaderState {
            files: self.files,
            edges: self.edges,
        }
    }

    /// Load `path` and, recursively, anything it imports.
    /// `importing_file` + `import_span` are used to attribute "not found"
    /// errors to the import site that triggered the load (None for the
    /// entry binary itself).
    fn load_recursive(
        &mut self,
        path: &Path,
        importing_file: Option<&Path>,
        import_span: Option<(Span, String)>,
    ) -> Result<String, ResolveError> {
        // Canonicalize; if it doesn't exist, attribute to the importing site.
        let canonical = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => {
                return Err(ResolveError::ImportNotFound {
                    importing_file: importing_file.map(|p| p.to_path_buf())
                        .unwrap_or_else(|| path.to_path_buf()),
                    import_span: import_span.as_ref().map(|(s, _)| *s).unwrap_or(Span::new(0, 0)),
                    requested: import_span.map(|(_, r)| r).unwrap_or_else(|| path.display().to_string()),
                    resolved: path.to_path_buf(),
                });
            }
        };
        if let Some(file_id) = self.by_canonical.get(&canonical) {
            return Ok(file_id.clone());
        }

        let raw_source = std::fs::read_to_string(&canonical).map_err(|e| ResolveError::Io {
            path: canonical.clone(),
            source: e,
        })?;
        // Slice 5DOC: doctest extraction runs per-file before lexing so
        // synthesized `#[test]` functions become part of the loaded unit
        // and participate in `attrs::discover_tests` later. Files without
        // doctest fences are unchanged.
        let source = crate::doctest::extract(&raw_source);
        let tokens = crate::lexer::tokenize(&source).map_err(|e| ResolveError::Lex {
            path: canonical.clone(),
            source: e,
        })?;
        let program = crate::parser::parse(tokens).map_err(|e| ResolveError::Parse {
            path: canonical.clone(),
            source: e,
        })?;

        let file_id = derive_file_id(&canonical, &self.manifest_root);

        self.by_canonical.insert(canonical.clone(), file_id.clone());
        let unit = FileUnit {
            file_id: file_id.clone(),
            canonical_path: canonical.clone(),
            source,
            program: program.clone(),
        };
        self.files.insert(file_id.clone(), unit);
        self.edges.insert(file_id.clone(), Vec::new());

        // Recurse into imports.
        let import_dir = canonical.parent().map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        for imp in &program.imports {
            let target_path = self.classify_import_path(&imp.path, &import_dir, &canonical, imp.span)?;
            let target_id = self.load_recursive(
                &target_path,
                Some(&canonical),
                Some((imp.span, imp.path.clone())),
            )?;
            self.edges.get_mut(&file_id).unwrap().push(target_id);
        }

        Ok(file_id)
    }

    fn classify_import_path(
        &self,
        path_str: &str,
        import_dir: &Path,
        importing_canonical: &Path,
        span: Span,
    ) -> Result<PathBuf, ResolveError> {
        classify_import_path(
            path_str, import_dir, importing_canonical, span,
            &self.manifest_root, &self.deps, self.project_mode,
        )
    }
}

/// Phase 2 Slice 2B: classify an `import "..."` path string and map it
/// to a filesystem path. Three shapes:
///
/// - `./foo` or `../foo` → file-relative under `import_dir`.
/// - `<dep>/...` where `<dep>` ∈ `deps` → vendor;
///   resolves to `<manifest_root>/vendor/<dep>/src/<rest>.cplus`.
/// - Anything else → E0853 (bare path not matching any rule). If the
///   first segment looks like a dep name but isn't declared, E0852 fires
///   instead with the more specific "did you forget a `[dependencies]`
///   entry?" diagnostic.
///
/// Phase 2 import paths are extension-less; a trailing `.cplus` fires
/// E0858. `..` segments inside a vendor path fire E0859 (security).
///
/// Backward compat: when `deps` is empty (pre-Slice-2B callers passing
/// `&[]`), bare paths fall through to file-relative behavior and the
/// `.cplus` extension is permitted. This is the single-file
/// `cpc FILE.cplus -o BIN` path that doesn't have a manifest.
fn classify_import_path(
    path_str: &str,
    import_dir: &Path,
    importing_canonical: &Path,
    span: Span,
    manifest_root: &Path,
    deps: &BTreeSet<String>,
    project_mode: bool,
) -> Result<PathBuf, ResolveError> {
    let extensionless = if let Some(stripped) = path_str.strip_suffix(".cplus") {
        if project_mode {
            return Err(ResolveError::StaleExtension {
                importing_file: importing_canonical.to_path_buf(),
                import_span: span,
                requested: path_str.to_string(),
            });
        }
        stripped.to_string()
    } else {
        path_str.to_string()
    };

    if extensionless.starts_with("./") || extensionless.starts_with("../") {
        let mut p = import_dir.join(&extensionless);
        if p.extension().is_none() {
            p.set_extension("cplus");
        }
        return Ok(p);
    }

    if !project_mode {
        // Pre-Slice-2B compat: single-file mode (no manifest). Treat as
        // file-relative so older callers keep working.
        let mut p = import_dir.join(&extensionless);
        if p.extension().is_none() {
            p.set_extension("cplus");
        }
        return Ok(p);
    }

    let mut segments = extensionless.split('/');
    let first = segments.next().unwrap_or("");
    let rest: Vec<&str> = segments.collect();

    if first.is_empty() {
        return Err(ResolveError::BareImport {
            importing_file: importing_canonical.to_path_buf(),
            import_span: span,
            requested: path_str.to_string(),
        });
    }

    if !deps.contains(first) {
        return Err(if rest.is_empty() {
            ResolveError::BareImport {
                importing_file: importing_canonical.to_path_buf(),
                import_span: span,
                requested: path_str.to_string(),
            }
        } else {
            ResolveError::UnknownPackage {
                importing_file: importing_canonical.to_path_buf(),
                import_span: span,
                requested: path_str.to_string(),
                package: first.to_string(),
            }
        });
    }

    if rest.iter().any(|seg| *seg == ".." || seg.is_empty()) {
        return Err(ResolveError::VendorEscape {
            importing_file: importing_canonical.to_path_buf(),
            import_span: span,
            requested: path_str.to_string(),
        });
    }

    let mut p = manifest_root.to_path_buf();
    p.push("vendor");
    p.push(first);
    p.push("src");
    for seg in &rest {
        p.push(seg);
    }
    p.set_extension("cplus");
    Ok(p)
}

fn derive_file_id(canonical: &Path, manifest_root: &Path) -> String {
    // Try to express canonical as a path relative to manifest_root. If that
    // fails (file lives outside the project — e.g. a vendor symlink resolving
    // outside the consumer's tree), fall back to the basename chain — better
    // than nothing.
    let canonical_root = std::fs::canonicalize(manifest_root)
        .unwrap_or_else(|_| manifest_root.to_path_buf());
    let rel = canonical.strip_prefix(&canonical_root).unwrap_or(canonical);
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => {
                let mut s = s.to_string_lossy().to_string();
                if let Some(stripped) = s.strip_suffix(".cplus") {
                    s = stripped.to_string();
                }
                parts.push(s);
            }
            std::path::Component::ParentDir => parts.push("up".to_string()),
            _ => {}
        }
    }
    let joined = if parts.is_empty() {
        "root".to_string()
    } else {
        parts.join(".")
    };
    // Sanitize for LLVM identifier shape. Path components can contain any
    // POSIX-filename byte; LLVM `define @<name>` only accepts a narrow set.
    // Keep `[A-Za-z0-9_.]` (dot is our segment separator); map everything
    // else to `_`. Notably this catches `+` in directory names — the C+
    // project literally lives at a path containing `+`, and without this
    // step every fallback file_id would be unlinkable.
    joined
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

fn detect_cycle(
    entry: &str,
    edges: &BTreeMap<String, Vec<String>>,
    files: &BTreeMap<String, FileUnit>,
) -> Result<(), ResolveError> {
    // Standard DFS with white/gray/black colors.
    let mut state: BTreeMap<String, u8> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();
    return dfs(entry, edges, &mut state, &mut stack, files);

    fn dfs(
        node: &str,
        edges: &BTreeMap<String, Vec<String>>,
        state: &mut BTreeMap<String, u8>,
        stack: &mut Vec<String>,
        files: &BTreeMap<String, FileUnit>,
    ) -> Result<(), ResolveError> {
        match state.get(node).copied().unwrap_or(0) {
            1 => {
                // Gray: cycle. Build the chain.
                let cut = stack.iter().position(|n| n == node).unwrap_or(0);
                let chain: Vec<PathBuf> = stack[cut..]
                    .iter()
                    .chain(std::iter::once(&node.to_string()))
                    .map(|id| files.get(id).map(|f| f.canonical_path.clone())
                        .unwrap_or_else(|| PathBuf::from(id)))
                    .collect();
                Err(ResolveError::Cycle { chain })
            }
            2 => Ok(()),
            _ => {
                state.insert(node.to_string(), 1);
                stack.push(node.to_string());
                if let Some(children) = edges.get(node) {
                    for c in children {
                        dfs(c, edges, state, stack, files)?;
                    }
                }
                stack.pop();
                state.insert(node.to_string(), 2);
                Ok(())
            }
        }
    }
}

// ----- merge / rewrite -----

fn merge(
    files: BTreeMap<String, FileUnit>,
    entry_file_id: &str,
    is_lib_entry: bool,
    manifest_root: &Path,
    deps: &BTreeSet<String>,
    project_mode: bool,
) -> Result<Program, ResolveError> {
    // Pre-pass: collect each file's local item names (used by the
    // rewriter to qualify unqualified references) AND its **public**
    // surface (slice 4B: gates cross-file access via E0403).
    //
    // `local_items` is everything declared at top level; `pub_items` is
    // the subset that's exported. `pub_methods[file_id][type_name]` is the
    // set of methods marked `pub` on that type — separately tracked
    // because methods live inside `impl` blocks. Enum variants inherit the
    // enum's `pub` (no per-variant flag), so the variant gate just
    // re-checks `pub_items`.
    let mut local_items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut local_enums: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut local_structs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pub_items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pub_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut item_kind: BTreeMap<String, BTreeMap<String, ItemKindTag>> = BTreeMap::new();
    for (fid, unit) in &files {
        let mut all: BTreeSet<String> = BTreeSet::new();
        let mut enums: BTreeSet<String> = BTreeSet::new();
        let mut structs: BTreeSet<String> = BTreeSet::new();
        let mut pubs: BTreeSet<String> = BTreeSet::new();
        let mut methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut kinds: BTreeMap<String, ItemKindTag> = BTreeMap::new();
        for it in &unit.program.items {
            match &it.kind {
                ItemKind::Function(f) => {
                    // Slice 10.FFI.1: extern fns are never local-qualified —
                    // they bind to a literal external C symbol. Skip the
                    // `all` insertion so call-site rewriting doesn't try
                    // to prefix the name. Pub is rejected by the parser
                    // for extern fns; nothing to record there.
                    if f.is_extern { continue; }
                    all.insert(f.name.name.clone());
                    kinds.insert(f.name.name.clone(), ItemKindTag::Function);
                    if f.is_pub { pubs.insert(f.name.name.clone()); }
                }
                ItemKind::Enum(e) => {
                    all.insert(e.name.name.clone());
                    enums.insert(e.name.name.clone());
                    kinds.insert(e.name.name.clone(), ItemKindTag::Enum);
                    if e.is_pub { pubs.insert(e.name.name.clone()); }
                }
                ItemKind::Struct(s) => {
                    all.insert(s.name.name.clone());
                    structs.insert(s.name.name.clone());
                    kinds.insert(s.name.name.clone(), ItemKindTag::Struct);
                    if s.is_pub { pubs.insert(s.name.name.clone()); }
                }
                ItemKind::Impl(b) => {
                    let entry = methods.entry(b.target.name.clone()).or_default();
                    for m in &b.methods {
                        if m.is_pub { entry.insert(m.name.name.clone()); }
                    }
                }
                // Slice 7GEN.3: interface declarations register as items.
                // Cross-file `impl Interface for Type` blocks reference
                // the interface by name; pub-status gates cross-file use.
                ItemKind::Interface(i) => {
                    all.insert(i.name.name.clone());
                    kinds.insert(i.name.name.clone(), ItemKindTag::Interface);
                    if i.is_pub { pubs.insert(i.name.name.clone()); }
                }
                // Phase 11 polish: aliases register as ordinary type-level
                // names so cross-file `pub use` lookups + import-alias
                // rewrites apply.
                ItemKind::TypeAlias(a) => {
                    all.insert(a.name.name.clone());
                    kinds.insert(a.name.name.clone(), ItemKindTag::TypeAlias);
                    if a.is_pub { pubs.insert(a.name.name.clone()); }
                }
            }
        }
        local_items.insert(fid.clone(), all);
        local_enums.insert(fid.clone(), enums);
        local_structs.insert(fid.clone(), structs);
        pub_items.insert(fid.clone(), pubs);
        pub_methods.insert(fid.clone(), methods);
        item_kind.insert(fid.clone(), kinds);
    }

    let mut merged_items: Vec<Item> = Vec::new();
    for (fid, unit) in &files {
        // Build the per-file import-alias → target-file-id map. Detect
        // duplicate prefixes (E0405) here so each file reports its own.
        let mut imports_map: BTreeMap<String, String> = BTreeMap::new();
        let mut first_span_for: BTreeMap<String, Span> = BTreeMap::new();
        for imp in &unit.program.imports {
            if let Some(first) = first_span_for.get(&imp.as_name.name) {
                return Err(ResolveError::DuplicatePrefix {
                    file: unit.canonical_path.clone(),
                    prefix: imp.as_name.name.clone(),
                    first_span: *first,
                    second_span: imp.as_name.span,
                });
            }
            // Resolve to the target file's file_id. The loader already
            // recorded each canonical path → file_id; redo the resolution
            // here through the same Phase 2 classifier so vendor and
            // local imports map to the same files the loader saw.
            let import_dir = unit.canonical_path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let target_path = classify_import_path(
                &imp.path, &import_dir, &unit.canonical_path, imp.span,
                manifest_root, deps, project_mode,
            )?;
            let target_canon = std::fs::canonicalize(&target_path)
                .unwrap_or(target_path);
            // Find the file_id whose canonical_path equals target_canon.
            let target_id = files.iter()
                .find(|(_, u)| u.canonical_path == target_canon)
                .map(|(id, _)| id.clone());
            if let Some(target_id) = target_id {
                imports_map.insert(imp.as_name.name.clone(), target_id);
                first_span_for.insert(imp.as_name.name.clone(), imp.as_name.span);
            }
            // If the loader didn't find the file we'd already have returned
            // ImportNotFound during load; here being missing means we hit
            // some race condition — just skip and let sema report the
            // dangling prefix.
        }

        let ctx = RewriteCtx {
            self_file_id: fid.clone(),
            self_file_path: unit.canonical_path.clone(),
            entry_file_id: entry_file_id.to_string(),
            is_lib_entry,
            imports: imports_map,
            local_items: local_items.get(fid).cloned().unwrap_or_default(),
            local_enums: local_enums.clone(),
            local_structs: local_structs.clone(),
            pub_items: pub_items.clone(),
            pub_methods: pub_methods.clone(),
            item_kind: item_kind.clone(),
        };

        for it in &unit.program.items {
            let rewritten = rewrite_item(it, &ctx)?;
            merged_items.push(rewritten);
        }
    }

    Ok(Program { imports: Vec::new(), items: merged_items })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKindTag { Function, Struct, Enum, Interface, TypeAlias }

struct RewriteCtx {
    self_file_id: String,
    self_file_path: PathBuf,
    entry_file_id: String,
    /// Phase 5 Slice 5.A: this project's root file is the entry of a
    /// `[lib]` target. Top-level items in `entry_file_id` skip mangling
    /// so C consumers can link against `pub fn add` as the bare `_add`
    /// symbol. Files imported by the entry stay qualified normally —
    /// they're not part of the public C ABI.
    is_lib_entry: bool,
    /// Map of `as`-prefix → target file id.
    imports: BTreeMap<String, String>,
    /// Top-level item names declared in this file.
    local_items: BTreeSet<String>,
    /// All files' enums and structs, indexed by file_id. Used to
    /// distinguish "Type::Variant" enum paths from "Type::method" assoc
    /// calls after a path prefix has been resolved.
    #[allow(dead_code)]
    local_enums: BTreeMap<String, BTreeSet<String>>,
    #[allow(dead_code)]
    local_structs: BTreeMap<String, BTreeSet<String>>,
    /// Per-file public surface (slice 4B). `pub_items[file_id]` is the set
    /// of top-level item names marked `pub`; `pub_methods[file_id][type]`
    /// is the set of pub methods on that type. Used to gate cross-file
    /// access (E0403). Same-file access ignores these.
    pub_items: BTreeMap<String, BTreeSet<String>>,
    pub_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// `item_kind[file_id][name]` tags each top-level item as Function /
    /// Struct / Enum. Used to pick the right error phrasing for E0403 and
    /// to decide if a 3-segment path is `Enum::Variant` (variants inherit
    /// the enum's pub) vs `Struct::method` (per-method pub check).
    item_kind: BTreeMap<String, BTreeMap<String, ItemKindTag>>,
}

impl RewriteCtx {
    /// Qualified name for an item `name` declared in this file. The entry
    /// binary's `main` keeps its bare name so the linker entry point works.
    ///
    /// Phase 5 Slice 5.A: when this is a library target's entry file,
    /// every top-level name skips qualification — the bare names ARE the
    /// public ABI. Internal helpers also stay unqualified for MVP; Slice
    /// 5.B will mark non-`pub` items with `internal` linkage so they
    /// don't leak as exported symbols.
    fn qualify_local(&self, name: &str) -> String {
        if name == "main" && self.self_file_id == self.entry_file_id {
            return "main".to_string();
        }
        if self.is_lib_entry && self.self_file_id == self.entry_file_id {
            return name.to_string();
        }
        format!("{}.{}", self.self_file_id, name)
    }

    /// Qualified name for an item `name` declared in file `target_id`.
    fn qualify_external(&self, target_id: &str, name: &str) -> String {
        if name == "main" && target_id == self.entry_file_id {
            return "main".to_string();
        }
        if self.is_lib_entry && target_id == self.entry_file_id {
            return name.to_string();
        }
        format!("{target_id}.{name}")
    }

    /// Check that top-level `name` is `pub` in `target_id` (cross-file).
    /// Same-file access is never blocked. Returns an E0403 if the item
    /// isn't pub. The `kind` is best-effort — looked up from item_kind;
    /// defaults to Function when unknown so the diagnostic still names
    /// something.
    fn check_pub_item(&self, target_id: &str, name: &str, span: Span) -> Result<(), ResolveError> {
        if target_id == self.self_file_id {
            return Ok(());
        }
        let kind = self.item_kind.get(target_id)
            .and_then(|m| m.get(name))
            .copied()
            .map(|k| match k {
                ItemKindTag::Function => PrivateKind::Function,
                ItemKindTag::Struct => PrivateKind::Struct,
                ItemKindTag::Enum => PrivateKind::Enum,
                ItemKindTag::Interface => PrivateKind::Interface,
                ItemKindTag::TypeAlias => PrivateKind::TypeAlias,
            })
            .unwrap_or(PrivateKind::Function);
        let is_pub = self.pub_items.get(target_id)
            .map(|s| s.contains(name))
            .unwrap_or(false);
        if !is_pub {
            return Err(ResolveError::PrivateAccess {
                file: self.self_file_path.clone(),
                span,
                kind,
                owner: target_id.to_string(),
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// Check that method `method` on type `type_name` is `pub` in
    /// `target_id` (cross-file). Same-file access is never blocked.
    fn check_pub_method(
        &self,
        target_id: &str,
        type_name: &str,
        method: &str,
        span: Span,
    ) -> Result<(), ResolveError> {
        if target_id == self.self_file_id {
            return Ok(());
        }
        let is_pub = self.pub_methods.get(target_id)
            .and_then(|m| m.get(type_name))
            .map(|s| s.contains(method))
            .unwrap_or(false);
        if !is_pub {
            return Err(ResolveError::PrivateAccess {
                file: self.self_file_path.clone(),
                span,
                kind: PrivateKind::Method,
                owner: type_name.to_string(),
                name: method.to_string(),
            });
        }
        Ok(())
    }

    /// Is the named local item an enum?
    fn external_is_enum(&self, target_id: &str, name: &str) -> bool {
        matches!(
            self.item_kind.get(target_id).and_then(|m| m.get(name)),
            Some(ItemKindTag::Enum)
        )
    }
}

fn rewrite_item(item: &Item, ctx: &RewriteCtx) -> Result<Item, ResolveError> {
    let kind = match &item.kind {
        ItemKind::Function(f) => ItemKind::Function(rewrite_fn(f, ctx)?),
        ItemKind::Enum(e) => {
            let mut e = e.clone();
            e.name.name = ctx.qualify_local(&e.name.name);
            for v in &mut e.variants {
                for p in &mut v.payload {
                    rewrite_type(p, ctx)?;
                }
            }
            ItemKind::Enum(e)
        }
        ItemKind::Struct(s) => {
            let mut s = s.clone();
            s.name.name = ctx.qualify_local(&s.name.name);
            for f in &mut s.fields {
                rewrite_type(&mut f.ty, ctx)?;
            }
            ItemKind::Struct(s)
        }
        ItemKind::Impl(b) => {
            let mut b = b.clone();
            // impl target must live in same file (4A doesn't enforce yet —
            // sema's normal "unknown type" error will surface if the user
            // tries `impl ForeignType {}`). Qualify against locals.
            if ctx.local_items.contains(&b.target.name) {
                b.target.name = ctx.qualify_local(&b.target.name);
            }
            for m in &mut b.methods {
                let new_method = rewrite_method(m, ctx)?;
                *m = new_method;
            }
            // Slice 7GEN.3: qualify the interface name if local.
            if let Some(iface) = &mut b.interface_name {
                if ctx.local_items.contains(&iface.name) {
                    iface.name = ctx.qualify_local(&iface.name);
                }
            }
            ItemKind::Impl(b)
        }
        // Slice 7GEN.3: interface declarations. Qualify the name and
        // rewrite types in each method signature. Self stays as
        // `Path("Self")` — sema handles the substitution at
        // impl-resolution.
        ItemKind::Interface(i) => {
            let mut i = i.clone();
            i.name.name = ctx.qualify_local(&i.name.name);
            for m in &mut i.methods {
                for p in &mut m.params {
                    rewrite_type(&mut p.ty, ctx)?;
                }
                if let Some(rt) = &mut m.return_type {
                    rewrite_type(rt, ctx)?;
                }
            }
            ItemKind::Interface(i)
        }
        // Phase 11 polish: type aliases. Qualify the alias name and
        // rewrite its target so cross-file paths in the target resolve.
        ItemKind::TypeAlias(a) => {
            let mut a = a.clone();
            a.name.name = ctx.qualify_local(&a.name.name);
            rewrite_type(&mut a.target, ctx)?;
            ItemKind::TypeAlias(a)
        }
    };
    Ok(Item { kind, span: item.span, origin_file: Some(ctx.self_file_id.clone()) })
}

fn rewrite_fn(f: &Function, ctx: &RewriteCtx) -> Result<Function, ResolveError> {
    let mut f = f.clone();
    let local_scope = HashSet::new();
    // Slice 10.FFI.1: extern fns keep their literal C symbol name —
    // the whole point of the FFI declaration is to bind a specific
    // external symbol. Param/return type rewriting still happens
    // (in case they reference C+ structs / type aliases).
    if !f.is_extern {
        f.name.name = ctx.qualify_local(&f.name.name);
    }
    for p in &mut f.params {
        rewrite_type(&mut p.ty, ctx)?;
    }
    if let Some(rt) = &mut f.return_type {
        rewrite_type(rt, ctx)?;
    }
    // Body: parameters and `self` are in scope.
    let mut scope = local_scope;
    for p in &f.params {
        scope.insert(p.name.name.clone());
    }
    rewrite_block(&mut f.body, ctx, &mut scope)?;
    Ok(f)
}

fn rewrite_method(m: &Method, ctx: &RewriteCtx) -> Result<Method, ResolveError> {
    let mut m = m.clone();
    // Method name stays bare — it's joined with the (already-qualified)
    // type name at codegen time.
    for p in &mut m.params {
        rewrite_type(&mut p.ty, ctx)?;
    }
    if let Some(rt) = &mut m.return_type {
        rewrite_type(rt, ctx)?;
    }
    let mut scope: HashSet<String> = HashSet::new();
    if m.receiver.is_some() {
        scope.insert("self".to_string());
    }
    for p in &m.params {
        scope.insert(p.name.name.clone());
    }
    rewrite_block(&mut m.body, ctx, &mut scope)?;
    Ok(m)
}

fn rewrite_type(ty: &mut Type, ctx: &RewriteCtx) -> Result<(), ResolveError> {
    match &mut ty.kind {
        TypeKind::Path(s) => {
            *s = rewrite_type_name(s, ty.span, ctx)?;
        }
        TypeKind::Array { elem, .. } => rewrite_type(elem, ctx)?,
        // Slice 6BC.5: region annotations are transparent for resolver
        // qualification — `borrow A prefix::T` rewrites to
        // `borrow A <qualified>` by recursing into the inner type.
        TypeKind::Borrowed { inner, .. } => rewrite_type(inner, ctx)?,
        // Slice 7GEN.5c: `prefix::Pair[i32, bool]` — qualify the generic
        // name + recurse into each arg (args may themselves reference
        // qualified types).
        TypeKind::Generic { name, args } => {
            *name = rewrite_type_name(name, ty.span, ctx)?;
            for a in args.iter_mut() { rewrite_type(a, ctx)?; }
        }
        TypeKind::RawPtr(inner) => rewrite_type(inner, ctx)?,
        // Slice 11.FN_PTR: function pointer types — recurse into each
        // param type and the return type so cross-file references in
        // signature components are qualified.
        TypeKind::FnPtr { params, return_type } => {
            for p in params.iter_mut() { rewrite_type(p, ctx)?; }
            if let Some(rt) = return_type.as_mut() { rewrite_type(rt, ctx)?; }
        }
        TypeKind::Slice(inner) => rewrite_type(inner, ctx)?,
    }
    Ok(())
}

fn rewrite_type_name(s: &str, span: Span, ctx: &RewriteCtx) -> Result<String, ResolveError> {
    // Cross-file: `prefix::Type` (and only that shape — types can't be
    // 3-segment because there's no Type::Variant in type position).
    if let Some((prefix, rest)) = s.split_once("::") {
        let Some(target_id) = ctx.imports.get(prefix) else {
            return Err(ResolveError::UnknownPrefix {
                file: ctx.self_file_path.clone(),
                span,
                prefix: prefix.to_string(),
            });
        };
        // Slice 4B: the referenced type must be `pub` in the target file.
        ctx.check_pub_item(target_id, rest, span)?;
        return Ok(ctx.qualify_external(target_id, rest));
    }
    // Unqualified: if it names a local item, qualify it; otherwise leave
    // alone (primitive, builtin, generic param, etc.).
    if ctx.local_items.contains(s) {
        return Ok(ctx.qualify_local(s));
    }
    Ok(s.to_string())
}

fn rewrite_block(b: &mut Block, ctx: &RewriteCtx, scope: &mut HashSet<String>) -> Result<(), ResolveError> {
    // Save the scope so locals declared inside this block don't leak out.
    let snapshot = scope.clone();
    for s in &mut b.stmts {
        rewrite_stmt(s, ctx, scope)?;
    }
    if let Some(tail) = &mut b.tail {
        rewrite_expr(tail, ctx, scope)?;
    }
    *scope = snapshot;
    Ok(())
}

fn rewrite_stmt(s: &mut Stmt, ctx: &RewriteCtx, scope: &mut HashSet<String>) -> Result<(), ResolveError> {
    match &mut s.kind {
        StmtKind::Let { name, ty, init, .. } => {
            if let Some(t) = ty {
                rewrite_type(t, ctx)?;
            }
            if let Some(e) = init {
                rewrite_expr(e, ctx, scope)?;
            }
            scope.insert(name.name.clone());
        }
        StmtKind::Return(opt) => {
            if let Some(e) = opt { rewrite_expr(e, ctx, scope)?; }
        }
        StmtKind::While { cond, body } => {
            rewrite_expr(cond, ctx, scope)?;
            rewrite_block(body, ctx, scope)?;
        }
        StmtKind::For(fl) => match fl {
            ForLoop::CStyle { init, cond, update, body } => {
                let snapshot = scope.clone();
                if let Some(init) = init {
                    rewrite_stmt(init, ctx, scope)?;
                }
                if let Some(c) = cond { rewrite_expr(c, ctx, scope)?; }
                for u in update { rewrite_expr(u, ctx, scope)?; }
                rewrite_block(body, ctx, scope)?;
                *scope = snapshot;
            }
            ForLoop::Range { var, iter, body } => {
                rewrite_expr(iter, ctx, scope)?;
                let snapshot = scope.clone();
                scope.insert(var.name.clone());
                rewrite_block(body, ctx, scope)?;
                *scope = snapshot;
            }
        }
        StmtKind::Expr(e) => rewrite_expr(e, ctx, scope)?,
        StmtKind::Defer(e) => rewrite_expr(e, ctx, scope)?,
        StmtKind::IfLet { pattern, scrutinee, body, else_body } => {
            rewrite_expr(scrutinee, ctx, scope)?;
            let snapshot = scope.clone();
            rewrite_pattern(pattern, ctx, scope)?;
            rewrite_block(body, ctx, scope)?;
            *scope = snapshot;
            if let Some(eb) = else_body {
                rewrite_block(eb, ctx, scope)?;
            }
        }
        StmtKind::Break | StmtKind::Continue => {
            // Pure control-flow markers — nothing to rewrite.
        }
        StmtKind::Assert(e) => rewrite_expr(e, ctx, scope)?,
        StmtKind::Loop(body) => {
            rewrite_block(body, ctx, scope)?;
        }
        StmtKind::WhileLet { pattern, scrutinee, body } => {
            rewrite_expr(scrutinee, ctx, scope)?;
            // Bindings from the loop pattern live inside the body only.
            let snapshot = scope.clone();
            rewrite_pattern(pattern, ctx, scope)?;
            rewrite_block(body, ctx, scope)?;
            *scope = snapshot;
        }
        StmtKind::GuardLet { pattern, scrutinee, complement, else_body } => {
            rewrite_expr(scrutinee, ctx, scope)?;
            // Else block runs in a scope that has NEITHER the pattern's
            // bindings (it didn't match) nor the post-statement scope.
            // Run the else-body walk in a snapshotted scope.
            {
                let snapshot = scope.clone();
                let mut inner = snapshot.clone();
                if let Some(cp) = complement {
                    rewrite_pattern(cp, ctx, &mut inner)?;
                }
                rewrite_block(else_body, ctx, &mut inner)?;
                let _ = snapshot;
            }
            // Add the pattern's bindings to the *enclosing* scope so the
            // continuation sees them.
            rewrite_pattern(pattern, ctx, scope)?;
        }
    }
    Ok(())
}

fn rewrite_expr(e: &mut Expr, ctx: &RewriteCtx, scope: &mut HashSet<String>) -> Result<(), ResolveError> {
    match &mut e.kind {
        ExprKind::IntLit(_, _) | ExprKind::FloatLit(_, _) | ExprKind::BoolLit(_) | ExprKind::StrLit(_) => {}
        ExprKind::InterpStr { parts } => {
            for p in parts {
                if let crate::ast::InterpStrPart::Expr(inner) = p {
                    rewrite_expr(inner, ctx, scope)?;
                }
            }
        }
        ExprKind::Ident(name) => {
            // Don't touch shadowed locals. Don't touch `self`.
            if scope.contains(name) || name == "self" {
                return Ok(());
            }
            // Built-in intrinsics stay un-prefixed.
            if is_builtin(name) {
                return Ok(());
            }
            if ctx.local_items.contains(name) {
                *name = ctx.qualify_local(name);
            }
        }
        ExprKind::Block(b) => rewrite_block(b, ctx, scope)?,
        ExprKind::Unsafe(b) => rewrite_block(b, ctx, scope)?,
        ExprKind::Await(inner) => rewrite_expr(inner, ctx, scope)?,
        ExprKind::If { cond, then, else_branch } => {
            rewrite_expr(cond, ctx, scope)?;
            rewrite_block(then, ctx, scope)?;
            if let Some(eb) = else_branch { rewrite_expr(eb, ctx, scope)?; }
        }
        ExprKind::Call { callee, args, type_args } => {
            rewrite_expr(callee, ctx, scope)?;
            for a in args { rewrite_expr(a, ctx, scope)?; }
            // v0.0.3 Slice 1P.3: turbofish type-args carry their own type
            // references (`foo::[mod::T, other::U](...)`); qualify them
            // the same way as types in declared positions.
            for ta in type_args.iter_mut() { rewrite_type(ta, ctx)?; }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, ctx, scope)?;
            rewrite_expr(rhs, ctx, scope)?;
        }
        ExprKind::Unary { operand, .. } => rewrite_expr(operand, ctx, scope)?,
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start { rewrite_expr(s, ctx, scope)?; }
            if let Some(en) = end { rewrite_expr(en, ctx, scope)?; }
        }
        ExprKind::Assign { target, value, .. } => {
            rewrite_expr(target, ctx, scope)?;
            rewrite_expr(value, ctx, scope)?;
        }
        ExprKind::Cast { expr, ty } => {
            rewrite_expr(expr, ctx, scope)?;
            rewrite_type(ty, ctx)?;
        }
        ExprKind::Path { segments } => {
            // Rewrite according to length:
            //   1 segment  : already an Ident — shouldn't happen as Path.
            //   2 segments : either Enum::Variant (local enum) or
            //                prefix::Item (cross-file, single name).
            //   3 segments : prefix::Type::method or prefix::Enum::Variant.
            //   4+         : not yet (will become E0312 in sema).
            if segments.len() == 2 {
                let first = &segments[0].name;
                if let Some(target_id) = ctx.imports.get(first) {
                    // prefix::Item — collapse to single ident.
                    let item_name = segments[1].name.clone();
                    let item_span = segments[1].span;
                    // Slice 4B: cross-file pub gate.
                    ctx.check_pub_item(target_id, &item_name, item_span)?;
                    let qualified = ctx.qualify_external(target_id, &item_name);
                    e.kind = ExprKind::Ident(qualified);
                    return Ok(());
                }
                // Local enum: qualify the first segment if it names a local
                // item (so the rewritten path matches the qualified enum).
                if ctx.local_items.contains(first) {
                    segments[0].name = ctx.qualify_local(first);
                }
            } else if segments.len() == 3 {
                let first = &segments[0].name;
                if let Some(target_id) = ctx.imports.get(first) {
                    let type_name = segments[1].name.clone();
                    let method_or_variant = segments[2].name.clone();
                    let type_span = segments[1].span;
                    let leaf_span = segments[2].span;
                    // Slice 4B: the type itself must be `pub` to be
                    // referenced cross-file at all.
                    ctx.check_pub_item(target_id, &type_name, type_span)?;
                    // If the type is an enum, variants inherit the enum's
                    // pub (no per-variant flag) — the `check_pub_item`
                    // above covers it. If the type is a struct, the method
                    // also needs its own `pub`.
                    if !ctx.external_is_enum(target_id, &type_name) {
                        ctx.check_pub_method(target_id, &type_name, &method_or_variant, leaf_span)?;
                    }
                    let new_type_name = ctx.qualify_external(target_id, &type_name);
                    segments.remove(0);
                    segments[0].name = new_type_name;
                    segments[0].span = type_span;
                    segments[1].name = method_or_variant;
                    segments[1].span = leaf_span;
                    return Ok(());
                }
                return Err(ResolveError::UnknownPrefix {
                    file: ctx.self_file_path.clone(),
                    span: segments[0].span,
                    prefix: first.clone(),
                });
            }
        }
        ExprKind::StructLit { name, fields } => {
            if ctx.local_items.contains(&name.name) {
                name.name = ctx.qualify_local(&name.name);
            } else if let Some((prefix, rest)) = name.name.clone().split_once("::").map(|(a, b)| (a.to_string(), b.to_string())) {
                if let Some(target_id) = ctx.imports.get(&prefix) {
                    // Slice 4B: cross-file struct literal requires the
                    // struct to be `pub`. Field-pub is enforced by sema
                    // (it has the field-level info after resolver-rewrite).
                    ctx.check_pub_item(target_id, &rest, name.span)?;
                    name.name = ctx.qualify_external(target_id, &rest);
                }
            }
            for f in fields {
                rewrite_expr(&mut f.value, ctx, scope)?;
            }
        }
        // Slice 7GEN.5c: rewrite the generic name (cross-file qualification)
        // + recurse into type args + field exprs. The pattern mirrors
        // `StructLit`, but resolver doesn't know about generic instantiation
        // names — those are synthesized by sema and live only post-mono.
        ExprKind::GenericStructLit { name, type_args, fields } => {
            if ctx.local_items.contains(&name.name) {
                name.name = ctx.qualify_local(&name.name);
            } else if let Some((prefix, rest)) = name.name.clone().split_once("::").map(|(a, b)| (a.to_string(), b.to_string())) {
                if let Some(target_id) = ctx.imports.get(&prefix) {
                    ctx.check_pub_item(target_id, &rest, name.span)?;
                    name.name = ctx.qualify_external(target_id, &rest);
                }
            }
            for ta in type_args.iter_mut() { rewrite_type(ta, ctx)?; }
            for f in fields { rewrite_expr(&mut f.value, ctx, scope)?; }
        }
        ExprKind::Field { receiver, .. } => rewrite_expr(receiver, ctx, scope)?,
        ExprKind::ArrayLit { elements } => {
            for el in elements { rewrite_expr(el, ctx, scope)?; }
        }
        // v0.0.3 1P.1: qualify the enum_name for cross-module generic enum
        // constructors (`mod::Enum[T, E]::Variant(args)`). Pattern mirrors
        // GenericStructLit above. Also rewrite type-args + arg expressions.
        ExprKind::GenericEnumCall { enum_name, type_args, args, .. } => {
            if ctx.local_items.contains(&enum_name.name) {
                enum_name.name = ctx.qualify_local(&enum_name.name);
            } else if let Some((prefix, rest)) = enum_name.name.clone().split_once("::").map(|(a, b)| (a.to_string(), b.to_string())) {
                if let Some(target_id) = ctx.imports.get(&prefix) {
                    ctx.check_pub_item(target_id, &rest, enum_name.span)?;
                    enum_name.name = ctx.qualify_external(target_id, &rest);
                }
            }
            for ta in type_args.iter_mut() { rewrite_type(ta, ctx)?; }
            for el in args { rewrite_expr(el, ctx, scope)?; }
        }
        ExprKind::Index { receiver, index } => {
            rewrite_expr(receiver, ctx, scope)?;
            rewrite_expr(index, ctx, scope)?;
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_expr(scrutinee, ctx, scope)?;
            for arm in arms {
                let snapshot = scope.clone();
                rewrite_pattern(&mut arm.pattern, ctx, scope)?;
                rewrite_expr(&mut arm.body, ctx, scope)?;
                *scope = snapshot;
            }
        }
    }
    Ok(())
}

fn rewrite_pattern(p: &mut Pattern, ctx: &RewriteCtx, scope: &mut HashSet<String>) -> Result<(), ResolveError> {
    match &mut p.kind {
        PatternKind::Wildcard => {}
        PatternKind::Binding(ident) => {
            scope.insert(ident.name.clone());
        }
        PatternKind::Variant { enum_name, type_args, payload, .. } => {
            // Three shapes (slice 4-end completes the cross-file case):
            //   `Variant`                  — `enum_name = "EnumName"`        (local)
            //   `Enum::Variant`            — `enum_name = "EnumName"`        (local; payload captured)
            //   `prefix::Enum::Variant`    — `enum_name = "prefix::Enum"`    (cross-file)
            //   `Option[i32]::Variant`     — generic-enum pattern (slice 7GEN.5e); type_args walked below.
            if let Some((prefix, rest)) = enum_name.name.clone()
                .split_once("::").map(|(a, b)| (a.to_string(), b.to_string()))
            {
                // Cross-file: rewrite to the qualified enum name.
                if let Some(target_id) = ctx.imports.get(&prefix) {
                    ctx.check_pub_item(target_id, &rest, enum_name.span)?;
                    enum_name.name = ctx.qualify_external(target_id, &rest);
                }
            } else if ctx.local_items.contains(&enum_name.name) {
                enum_name.name = ctx.qualify_local(&enum_name.name);
            }
            for ta in type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
            for sub in payload {
                rewrite_pattern(sub, ctx, scope)?;
            }
        }
    }
    Ok(())
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "println"
            | "i8" | "i16" | "i32" | "i64"
            | "u8" | "u16" | "u32" | "u64"
            | "isize" | "usize"
            | "f32" | "f64"
            | "bool"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmpdir() -> PathBuf {
        // v0.0.3 Phase 2: secure random tempdir via `tempfile` crate. The
        // TempDir auto-cleans on drop; we leak it via `Box::leak` so the
        // returned `PathBuf` outlives the test's scope (it gets passed
        // into helper fns that run after this returns).
        let _ = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = tempfile::Builder::new()
            .prefix("cpc-resolver-")
            .tempdir()
            .expect("tempdir creation");
        let leaked: &'static tempfile::TempDir = Box::leak(Box::new(dir));
        leaked.path().to_path_buf()
    }

    #[test]
    fn derive_file_id_basics() {
        let root = PathBuf::from("/tmp/proj");
        assert_eq!(derive_file_id(Path::new("/tmp/proj/src/main.cplus"), &root), "src.main");
        assert_eq!(derive_file_id(Path::new("/tmp/proj/src/util/strings.cplus"), &root), "src.util.strings");
    }

    #[test]
    fn single_file_no_imports() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let main = dir.join("src/main.cplus");
        fs::write(&main, "fn main() -> i32 { return 0; }").unwrap();
        let p = load_project(&main, &dir).unwrap();
        // `main` stays bare in the entry file.
        let names: Vec<String> = p.program.items.iter().map(|it| match &it.kind {
            ItemKind::Function(f) => f.name.name.clone(),
            _ => String::new(),
        }).collect();
        assert!(names.contains(&"main".to_string()));
    }

    #[test]
    fn import_and_call_resolves() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/math.cplus"), "pub fn square(n: i32) -> i32 { return n * n; }").unwrap();
        let main_src = r#"
            import "math.cplus" as math;
            fn main() -> i32 { return math::square(7); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let p = load_project(&main, &dir).unwrap();
        // `math::square` should have been rewritten to qualified Ident.
        let main_fn = p.program.items.iter().find_map(|it| match &it.kind {
            ItemKind::Function(f) if f.name.name == "main" => Some(f),
            _ => None,
        }).unwrap();
        // Inspect the call expr in main's body.
        let return_expr = match &main_fn.body.stmts[0].kind {
            StmtKind::Return(Some(e)) => e,
            _ => panic!("expected return stmt"),
        };
        let callee = match &return_expr.kind {
            ExprKind::Call { callee, .. } => callee,
            other => panic!("expected Call, got {other:?}"),
        };
        match &callee.kind {
            ExprKind::Ident(name) => assert_eq!(name, "src.math.square"),
            other => panic!("expected Ident, got {other:?}"),
        }
        // `square` itself should have been qualified.
        let square = p.program.items.iter().find_map(|it| match &it.kind {
            ItemKind::Function(f) if f.name.name == "src.math.square" => Some(f),
            _ => None,
        });
        assert!(square.is_some(), "expected qualified `src.math.square` in merged program");
    }

    #[test]
    fn import_not_found_errors() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let main_src = r#"
            import "missing.cplus" as m;
            fn main() -> i32 { return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::ImportNotFound { .. }));
    }

    #[test]
    fn duplicate_prefix_errors() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.cplus"), "fn one() -> i32 { return 1; }").unwrap();
        fs::write(dir.join("src/b.cplus"), "fn two() -> i32 { return 2; }").unwrap();
        let main_src = r#"
            import "a.cplus" as m;
            import "b.cplus" as m;
            fn main() -> i32 { return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::DuplicatePrefix { .. }));
    }

    #[test]
    fn cycle_detected() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.cplus"), r#"
            import "b.cplus" as b;
            fn from_a() -> i32 { return 1; }
        "#).unwrap();
        fs::write(dir.join("src/b.cplus"), r#"
            import "a.cplus" as a;
            fn from_b() -> i32 { return 2; }
        "#).unwrap();
        let main_src = r#"
            import "a.cplus" as a;
            fn main() -> i32 { return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::Cycle { .. }));
    }

    #[test]
    fn cross_file_private_fn_rejected_with_e0403() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        // No `pub` — private to math.cplus.
        fs::write(dir.join("src/math.cplus"), "fn square(n: i32) -> i32 { return n * n; }").unwrap();
        let main_src = r#"
            import "math.cplus" as math;
            fn main() -> i32 { return math::square(7); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::PrivateAccess { kind: PrivateKind::Function, .. }),
            "expected PrivateAccess Function, got {err:?}");
    }

    #[test]
    fn cross_file_private_struct_rejected_with_e0403() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/geom.cplus"),
            "struct Point { x: i32, y: i32 }\n").unwrap();
        // Slice 4C: cross-file struct literal `g::Point { ... }` now
        // parses, so the E0403 check fires on the construction site.
        let main_src = r#"
            import "geom.cplus" as g;
            fn main() -> i32 { let p = g::Point { x: 1, y: 2 }; return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::PrivateAccess { kind: PrivateKind::Struct, .. }),
            "expected PrivateAccess Struct, got {err:?}");
    }

    #[test]
    fn cross_file_public_struct_private_method_rejected() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        // Struct is pub but method isn't.
        fs::write(dir.join("src/geom.cplus"), r#"
            pub struct Point { pub x: i32, pub y: i32 }
            impl Point {
                fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }
            }
        "#).unwrap();
        let main_src = r#"
            import "geom.cplus" as g;
            fn main() -> i32 { let p: g::Point = g::Point::new(1, 2); return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::PrivateAccess { kind: PrivateKind::Method, .. }),
            "expected PrivateAccess Method, got {err:?}");
    }

    #[test]
    fn same_file_private_access_allowed() {
        // A private item is freely callable from within its file. Sanity
        // check that the pub gate doesn't fire on same-file refs.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let main_src = r#"
            fn helper(n: i32) -> i32 { return n + 1; }
            fn main() -> i32 { return helper(41); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        load_project(&main, &dir).expect("same-file access should not trigger E0403");
    }

    #[test]
    fn cross_file_variant_pattern_in_match_resolves() {
        // Slice 4-end carry-forward from 4A: `prefix::Enum::Variant(...)`
        // patterns inside `match` now parse and resolve to the qualified
        // enum.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/colors.cplus"),
            "pub enum Color { Red, Green(i32), Blue }\n").unwrap();
        let main_src = r#"
            import "colors.cplus" as c;
            fn name(co: c::Color) -> i32 {
                return match co {
                    c::Color::Red => 0,
                    c::Color::Green(v) => v,
                    c::Color::Blue => 2,
                };
            }
            fn main() -> i32 { return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let project = load_project(&main, &dir).expect("project loads");
        // Walk to `name` fn's match arms and confirm the enum_name was
        // rewritten to `src.colors.Color`.
        let name_fn = project.program.items.iter().find_map(|it| match &it.kind {
            ItemKind::Function(f) if f.name.name == "src.main.name" => Some(f),
            _ => None,
        }).expect("found name fn");
        let return_expr = match &name_fn.body.stmts[0].kind {
            StmtKind::Return(Some(e)) => e,
            _ => panic!("expected return"),
        };
        let arms = match &return_expr.kind {
            ExprKind::Match { arms, .. } => arms,
            _ => panic!("expected match"),
        };
        for arm in arms {
            if let PatternKind::Variant { enum_name, .. } = &arm.pattern.kind {
                assert_eq!(enum_name.name, "src.colors.Color",
                    "expected qualified enum name; got `{}`", enum_name.name);
            }
        }
    }

    #[test]
    fn cross_file_pub_enum_variants_are_accessible() {
        // `pub enum` exports variants automatically (no per-variant pub).
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/colors.cplus"),
            "pub enum Color { Red, Green, Blue }\n").unwrap();
        let main_src = r#"
            import "colors.cplus" as c;
            fn main() -> i32 {
                let r: c::Color = c::Color::Red;
                return 0;
            }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        load_project(&main, &dir).expect("pub enum variants should be reachable");
    }

    #[test]
    fn cross_file_private_enum_rejected() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/colors.cplus"),
            "enum Color { Red, Green, Blue }\n").unwrap();
        let main_src = r#"
            import "colors.cplus" as c;
            fn main() -> i32 { let r: c::Color = c::Color::Red; return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(matches!(err.error, ResolveError::PrivateAccess { kind: PrivateKind::Enum, .. }),
            "expected PrivateAccess Enum, got {err:?}");
    }

    #[test]
    fn cross_file_struct_and_method_resolve() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/geom.cplus"), r#"
            pub struct Point { pub x: i32, pub y: i32 }
            impl Point {
                pub fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }
            }
        "#).unwrap();
        let main_src = r#"
            import "geom.cplus" as g;
            fn main() -> i32 {
                let p: g::Point = g::Point::new(3, 4);
                return p.x;
            }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let p = load_project(&main, &dir).unwrap();
        // The struct should be `src.geom.Point`.
        let has_struct = p.program.items.iter().any(|it| match &it.kind {
            ItemKind::Struct(s) => s.name.name == "src.geom.Point",
            _ => false,
        });
        assert!(has_struct);
        // The impl block target should also be `src.geom.Point`.
        let has_impl = p.program.items.iter().any(|it| match &it.kind {
            ItemKind::Impl(b) => b.target.name == "src.geom.Point",
            _ => false,
        });
        assert!(has_impl);
    }
}
