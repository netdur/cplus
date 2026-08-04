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
    Cycle { chain: Vec<PathBuf> },
    /// Cross-file reference to a non-`pub` item. (E0403, slice 4B.)
    /// `kind` distinguishes the surface form so the message can use the
    /// right phrasing — function, struct, enum, method, or field.
    PrivateAccess {
        file: PathBuf,
        span: Span,
        kind: PrivateKind,
        owner: String, // for methods: the type name; for fields: the struct; else the file id
        name: String,  // the item being denied
    },
    /// v0.0.12 G-030-bonus (llama.cplus G-029 bonus): cross-file reference
    /// to a name that doesn't exist at all in the target module. Pre-fix,
    /// the resolver lumped this into PrivateAccess with the misleading
    /// message "function X is private (mark it `pub` ...)" — but there's
    /// nothing to mark `pub` because the name isn't there. Distinct
    /// variant + clean message.
    UnknownItem {
        file: PathBuf,
        span: Span,
        owner: String, // file id of the target module
        name: String,
    },
    /// A `Type::method` path names a method the type does not have. The
    /// METHOD twin of `UnknownItem` — v0.0.12 split unknown from private for
    /// top-level items so a typo stopped being reported as a privacy
    /// violation, and the method path was never given the same split
    /// (reports/bug-22).
    UnknownMethod {
        file: PathBuf,
        span: Span,
        owner: String, // the type name
        name: String,
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
    /// v0.0.21 embedded profile (E0866): the import names a stdlib
    /// module the selected target's package profile excludes (POSIX
    /// mechanisms absent on the target). Failing at resolve time gives
    /// the profile story instead of an IR-verifier error.
    TargetGatedModule {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
        target_name: &'static str,
    },
    /// (E0866, platform-deps flavor): the import names a package the
    /// manifest declares only for OTHER platforms via
    /// `[<platform>.dependencies]`. Same family as `TargetGatedModule` —
    /// "this import doesn't exist where you're building" — but the gate is
    /// the consumer's own manifest, so the fix-notes differ.
    PlatformGatedPackage {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
        package: String,
        /// Comma-joined platform list the dep was declared for.
        platforms: String,
        /// The active platform the build is for.
        active: &'static str,
    },
    /// Phase 2: a vendor import contains a `..` segment that would
    /// escape `vendor/<pkg>/src/`. Security: a package can't reach
    /// outside its own directory via static imports.
    VendorEscape {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
    },
    /// A file-relative import (`./x` / `../x`) whose `..` chain resolves to a
    /// file OUTSIDE the project tree. Security: symmetric with `VendorEscape`
    /// — neither import kind may pull source from outside `manifest_root`.
    RelativeEscape {
        importing_file: PathBuf,
        import_span: Span,
        requested: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum PrivateKind {
    Function,
    Struct,
    Enum,
    Method,
    Interface,
    TypeAlias,
    /// v0.0.9 Phase 4: module-scope `const NAME`. Same cross-file
    /// visibility gate as a function; a leading `_` (module-private)
    /// triggers E0403 on cross-file access.
    Const,
    /// v0.0.9 Phase 4: module-scope `static NAME`. Same visibility gate.
    Static,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::ImportNotFound {
                importing_file,
                requested,
                resolved,
                ..
            } => {
                write!(
                    f,
                    "[E0401] {}: import `{requested}` not found (resolved to {})",
                    importing_file.display(),
                    resolved.display()
                )
            }
            ResolveError::DuplicatePrefix { file, prefix, .. } => {
                write!(
                    f,
                    "[E0405] {}: duplicate import prefix `{prefix}`",
                    file.display()
                )
            }
            ResolveError::UnknownPrefix { file, prefix, .. } => {
                write!(
                    f,
                    "[E0402] {}: unknown import prefix `{prefix}`",
                    file.display()
                )
            }
            ResolveError::Cycle { chain } => {
                let chain_str: Vec<String> =
                    chain.iter().map(|p| p.display().to_string()).collect();
                write!(f, "[E0404] cyclic import: {}", chain_str.join(" -> "))
            }
            ResolveError::PrivateAccess {
                file,
                kind,
                owner,
                name,
                ..
            } => {
                let what = match kind {
                    PrivateKind::Function => "function",
                    PrivateKind::Struct => "struct",
                    PrivateKind::Enum => "enum",
                    PrivateKind::Method => "method",
                    PrivateKind::Interface => "interface",
                    PrivateKind::TypeAlias => "type alias",
                    PrivateKind::Const => "const",
                    PrivateKind::Static => "static",
                };
                match kind {
                    PrivateKind::Method => write!(
                        f,
                        "[E0403] {}: {what} `{owner}::{name}` is private (its leading `_` marks it module-private; drop the `_` to export)",
                        file.display(),
                    ),
                    _ => write!(
                        f,
                        "[E0403] {}: {what} `{name}` (in module `{owner}`) is private (its leading `_` marks it module-private; drop the `_` to export)",
                        file.display(),
                    ),
                }
            }
            ResolveError::UnknownItem {
                file, owner, name, ..
            } => {
                write!(
                    f,
                    "[E0405] {}: no item named `{name}` in module `{owner}`",
                    file.display(),
                )
            }
            ResolveError::UnknownMethod {
                file, owner, name, ..
            } => {
                write!(
                    f,
                    "[E0405] {}: no method named `{name}` on `{owner}`",
                    file.display(),
                )
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
            ResolveError::UnknownPackage {
                importing_file,
                requested,
                package,
                ..
            } => {
                write!(
                    f,
                    "[E0852] {}: import `{requested}` — first segment `{package}` is not a declared dependency in `Cplus.toml`",
                    importing_file.display(),
                )
            }
            ResolveError::BareImport {
                importing_file,
                requested,
                ..
            } => {
                write!(
                    f,
                    "[E0853] {}: bare import `{requested}` — paths must start with `./`/`../` for file-relative or match a declared `[dependencies]` entry",
                    importing_file.display(),
                )
            }
            ResolveError::TargetGatedModule {
                importing_file,
                requested,
                target_name,
                ..
            } => {
                write!(
                    f,
                    "[E0866] {}: import `{requested}` is not available on target `{target_name}` (excluded from this target's package profile)",
                    importing_file.display(),
                )
            }
            ResolveError::PlatformGatedPackage {
                importing_file,
                requested,
                package,
                platforms,
                active,
                ..
            } => {
                write!(
                    f,
                    "[E0866] {}: import `{requested}` is not available on platform `{active}` — `Cplus.toml` declares `{package}` for `{platforms}` only",
                    importing_file.display(),
                )
            }
            ResolveError::StaleExtension {
                importing_file,
                requested,
                ..
            } => {
                write!(
                    f,
                    "[E0858] {}: import `{requested}` has a `.cplus` extension — drop the extension (Phase 2 imports are extension-less)",
                    importing_file.display(),
                )
            }
            ResolveError::VendorEscape {
                importing_file,
                requested,
                ..
            } => {
                write!(
                    f,
                    "[E0859] {}: vendor import `{requested}` contains `..` — packages cannot reach outside their own `src/` directory",
                    importing_file.display(),
                )
            }
            ResolveError::RelativeEscape {
                importing_file,
                requested,
                ..
            } => {
                write!(
                    f,
                    "[E0914] {}: relative import `{requested}` resolves outside the project directory — imports may not reach beyond the project root",
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
pub fn load_project(entry_path: &Path, manifest_root: &Path) -> Result<LoadedProject, LoadFailure> {
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
    load_project_full(entry_path, manifest_root, is_lib, None, BTreeMap::new())
}

/// v0.0.14 LSP dirty-buffer overlay: like `load_project`, but `overlays`
/// (canonical-path → unsaved buffer text) replace the on-disk contents of those
/// files. Lets the LSP serve graph/type-at/value-refs/goto-def from in-editor
/// edits before save. Mirrors `load_project`'s dep/mode handling otherwise.
pub fn load_project_with_overlays(
    entry_path: &Path,
    manifest_root: &Path,
    overlays: BTreeMap<PathBuf, String>,
) -> Result<LoadedProject, LoadFailure> {
    load_project_full(entry_path, manifest_root, false, None, overlays)
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
    overlays: BTreeMap<PathBuf, String>,
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
    // Resolve the project's own package name once, before any file is loaded:
    // every file id derived below is qualified with it.
    loader.load_package_name();
    let own_package = loader.package_name.clone();
    loader.overlays = overlays;
    let entry_file_id = match loader.load_recursive(entry_path, None, None) {
        Ok(id) => id,
        Err(e) => return Err(LoadFailure::new(e, &loader)),
    };
    let LoaderState { files, edges } = loader.into_state();

    if let Err(e) = detect_cycle(&entry_file_id, &edges, &files) {
        let sources = files.values().map(|u| (u.canonical_path.clone(), u.source.clone()))
            .collect();
        return Err(LoadFailure { error: e, sources });
    }

    // Snapshot per-file (path, source) before `merge` consumes `files`.
    let file_sources: std::collections::BTreeMap<String, (PathBuf, String)> = files
        .iter()
        .map(|(fid, u)| (fid.clone(), (u.canonical_path.clone(), u.source.clone())))
        .collect();
    // Also keyed by canonical path for the failure path.
    let sources_by_path: std::collections::BTreeMap<PathBuf, String> = files.values().map(|u| (u.canonical_path.clone(), u.source.clone()))
        .collect();

    let merged = match merge(
        files,
        &entry_file_id,
        is_lib,
        manifest_root,
        &loader_deps_snapshot,
        own_package.as_deref(),
        project_mode,
    ) {
        Ok(p) => p,
        Err(e) => {
            return Err(LoadFailure {
                error: e,
                sources: sources_by_path,
            })
        }
    };
    Ok(LoadedProject {
        program: merged,
        entry_file_id,
        files: file_sources,
        imports: edges,
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
        // Start with raw_sources (every file the loader has *read*, even if
        // lex/parse failed) so the failing file's source is always
        // available for span rendering. Then overlay loader.files (which
        // for successfully-parsed files carries the post-doctest source
        // that matches the spans the parser produced).
        let mut sources: std::collections::BTreeMap<PathBuf, String> = loader.raw_sources.clone();
        for u in loader.files.values() {
            sources.insert(u.canonical_path.clone(), u.source.clone());
        }
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
            ResolveError::UnknownItem { file, .. } => Some(file),
            ResolveError::UnknownMethod { file, .. } => Some(file),
            ResolveError::Cycle { chain } => chain.first().map(|p| p.as_path()),
            ResolveError::Parse { path, .. } => Some(path),
            ResolveError::Lex { path, .. } => Some(path),
            ResolveError::Io { path, .. } => Some(path),
            ResolveError::UnknownPackage { importing_file, .. } => Some(importing_file),
            ResolveError::BareImport { importing_file, .. } => Some(importing_file),
            ResolveError::TargetGatedModule { importing_file, .. } => Some(importing_file),
            ResolveError::PlatformGatedPackage { importing_file, .. } => Some(importing_file),
            ResolveError::StaleExtension { importing_file, .. } => Some(importing_file),
            ResolveError::VendorEscape { importing_file, .. } => Some(importing_file),
            ResolveError::RelativeEscape { importing_file, .. } => Some(importing_file),
        }
    }

    /// Source of the file `primary_path()` points to, if we have it.
    pub fn primary_source(&self) -> Option<&str> {
        let p = self.primary_path()?;
        // Try canonical first, then fall back to the raw path.
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        self.sources
            .get(&canon)
            .map(|s| s.as_str())
            .or_else(|| self.sources.get(p).map(|s| s.as_str()))
    }

    /// Render this failure as a structured `Diagnostic`. Routes the span
    /// through the primary file's line-map so JSON/short/human renderers
    /// all see the right (file, line, col).
    pub fn to_diagnostic(&self) -> crate::diagnostics::Diagnostic {
        use crate::diagnostics::{
            Applicability, DiagCode, Diagnostic, LineMap, Position, Severity, SourceSpan,
            Suggestion,
        };

        // Helper: build a SourceSpan for `(path, span)` using whatever
        // source we have. If no source is available (rare — file went
        // missing between read and error), fall back to a degenerate
        // position-only span.
        let span_in = |path: &Path, span: Span| -> SourceSpan {
            if let Some(src) = self.sources.get(path) {
                let lm = LineMap::new(src);
                lm.span(path, span, src)
            } else {
                SourceSpan {
                    file: path.to_path_buf(),
                    start: Position {
                        line: 1,
                        col: 1,
                        byte: span.start,
                    },
                    end: Position {
                        line: 1,
                        col: 1,
                        byte: span.end,
                    },
                }
            }
        };
        // Helper for errors whose "primary location" is just a path with
        // no useful span (manifest entry missing, I/O errors before a
        // file is read).
        let pathless_span = |path: &Path| -> SourceSpan {
            SourceSpan {
                file: path.to_path_buf(),
                start: Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
                end: Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
            }
        };

        let mut suggestions: Vec<Suggestion> = Vec::new();
        let mut notes: Vec<String> = Vec::new();

        let (code, message, primary): (&'static str, String, SourceSpan) = match &self.error {
            ResolveError::ImportNotFound {
                importing_file,
                import_span,
                requested,
                resolved,
            } => {
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
            ResolveError::DuplicatePrefix {
                file,
                prefix,
                first_span,
                second_span,
            } => {
                notes.push("each `import` must use a distinct `as` name".to_string());
                let primary = span_in(file, *second_span);
                // Point at the first import as well, via a note (Label
                // would also fit but we keep it simple).
                let first = span_in(file, *first_span);
                notes.push(format!(
                    "first import at {}:{}:{}",
                    first.file.display(),
                    first.start.line,
                    first.start.col
                ));
                (
                    "E0405",
                    format!("duplicate import prefix `{prefix}`"),
                    primary,
                )
            }
            ResolveError::UnknownPrefix { file, span, prefix } => (
                "E0402",
                format!("unknown import prefix `{prefix}`"),
                span_in(file, *span),
            ),
            ResolveError::Cycle { chain } => {
                let chain_str: Vec<String> =
                    chain.iter().map(|p| p.display().to_string()).collect();
                notes.push(format!("cycle: {}", chain_str.join(" -> ")));
                let primary = chain
                    .first()
                    .map(|p| pathless_span(p))
                    .unwrap_or_else(|| pathless_span(Path::new("<unknown>")));
                ("E0404", "cyclic import dependency".to_string(), primary)
            }
            ResolveError::PrivateAccess {
                file,
                span,
                kind,
                owner,
                name,
            } => {
                let what = match kind {
                    PrivateKind::Function => "function",
                    PrivateKind::Struct => "struct",
                    PrivateKind::Enum => "enum",
                    PrivateKind::Method => "method",
                    PrivateKind::Interface => "interface",
                    PrivateKind::TypeAlias => "type alias",
                    PrivateKind::Const => "const",
                    PrivateKind::Static => "static",
                };
                let msg = match kind {
                    PrivateKind::Method => format!(
                        "{what} `{owner}::{name}` is private (its leading `_` marks it module-private; drop the `_` to export)",
                    ),
                    _ => format!(
                        "{what} `{name}` is private (its leading `_` marks it module-private in `{owner}`; drop the `_` to export)",
                    ),
                };
                ("E0403", msg, span_in(file, *span))
            }
            ResolveError::UnknownItem {
                file,
                span,
                owner,
                name,
            } => {
                let msg = format!("no item named `{name}` in module `{owner}`");
                ("E0405", msg, span_in(file, *span))
            }
            ResolveError::UnknownMethod {
                file,
                span,
                owner,
                name,
            } => {
                let msg = format!("no method named `{name}` on `{owner}`");
                ("E0405", msg, span_in(file, *span))
            }
            ResolveError::Io { path, source } => (
                "E0401",
                format!("I/O error reading `{}`: {source}", path.display()),
                pathless_span(path),
            ),
            ResolveError::Parse { path, source } => {
                // Carry the ORIGINAL parse error's code/message/span through, so
                // a syntax error in an imported file reports the same code as
                // the identical error in the entry file (was a placeholder
                // `E01XX`). `from_parse` owns the ParseErrorKind → code map.
                let src = self.sources.get(path).map(String::as_str).unwrap_or("");
                let lm = LineMap::new(src);
                return crate::diagnostics::from_parse(source, path, &lm, src);
            }
            ResolveError::Lex { path, source } => {
                // Same for a lex error in an imported file — route through the
                // canonical `from_lex` map instead of a placeholder `E00XX`.
                let src = self.sources.get(path).map(String::as_str).unwrap_or("");
                let lm = LineMap::new(src);
                return crate::diagnostics::from_lex(source, path, &lm, src);
            }
            ResolveError::UnknownPackage {
                importing_file,
                import_span,
                requested,
                package,
            } => {
                notes.push(format!(
                    "add `{package} = \"*\"` to `[dependencies]` in `Cplus.toml`, or change the import to `./{requested}` for a file-relative path"
                ));
                (
                    "E0852",
                    format!("import `{requested}`: first segment `{package}` is not a declared dependency"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::BareImport {
                importing_file,
                import_span,
                requested,
            } => {
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
            ResolveError::TargetGatedModule {
                importing_file,
                import_span,
                requested,
                target_name,
            } => {
                notes.push(
                    "the module relies on POSIX mechanisms (pthreads, kqueue/epoll, process environment) the target does not provide".to_string(),
                );
                notes.push(
                    "see `vendor/espidf` for the embedded equivalents (esp_timer, task sleep, GPIO, console)".to_string(),
                );
                (
                    "E0866",
                    format!("import `{requested}` is not available on target `{target_name}`"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::PlatformGatedPackage {
                importing_file,
                import_span,
                requested,
                package,
                platforms,
                active,
            } => {
                notes.push(format!(
                    "`Cplus.toml` declares `{package}` for `{platforms}` only — the current build is for `{active}`"
                ));
                notes.push(format!(
                    "confine the import to a platform-specific module (a `<name>_<platform>.cplus` sibling shadows `<name>.cplus` on that platform), or declare `{package}` for `{active}` too"
                ));
                (
                    "E0866",
                    format!("import `{requested}` is not available on platform `{active}`"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::StaleExtension {
                importing_file,
                import_span,
                requested,
            } => {
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
            ResolveError::VendorEscape {
                importing_file,
                import_span,
                requested,
            } => {
                notes.push(
                    "packages cannot reach files outside their own `src/` directory via static imports".to_string()
                );
                (
                    "E0859",
                    format!("vendor import `{requested}` contains `..`"),
                    span_in(importing_file, *import_span),
                )
            }
            ResolveError::RelativeEscape {
                importing_file,
                import_span,
                requested,
            } => {
                notes.push(
                    "a relative import may not resolve outside the project root — the same containment the vendor import path enforces".to_string()
                );
                (
                    "E0914",
                    format!("relative import `{requested}` resolves outside the project directory"),
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
    let want_stem = want_basename
        .strip_suffix(".cplus")
        .unwrap_or(&want_basename)
        .to_string();
    let dir = importing_file.parent()?;
    let mut candidates: Vec<(String, String)> = Vec::new();
    push_cplus_files(dir, dir, &mut candidates, 0);
    let mut best: Option<(usize, String)> = None;
    for (rel, basename) in &candidates {
        // Compare stem-to-stem so the distance reflects what the user
        // would re-type, not the on-disk extension.
        let basename_stem = basename.strip_suffix(".cplus").unwrap_or(basename);
        let d = edit_distance(&want_stem, basename_stem);
        if d > 2 {
            continue;
        }
        match &best {
            None => best = Some((d, rel.clone())),
            Some((bd, _)) if d < *bd => best = Some((d, rel.clone())),
            _ => {}
        }
    }
    best.map(|(_, rel)| rel)
}

fn push_cplus_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && depth < 1 {
            push_cplus_files(root, &p, out, depth + 1);
        } else if p.is_file() {
            let Some(ext) = p.extension() else {
                continue;
            };
            if ext != "cplus" {
                continue;
            }
            let basename = p.file_name().unwrap().to_string_lossy().to_string();
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            out.push((rel, basename));
        }
    }
}

/// Classic dynamic-programming Levenshtein distance. Bounded by string
/// length; suggestion candidates are short basenames so this is cheap.
use crate::diagnostics::edit_distance;

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
    /// EXT.2: the import graph, `file_id → the file ids it imports directly`.
    /// The merged `Program` drops per-file `import` declarations, but an
    /// extension method is only visible where the module that wrote it was
    /// imported — so sema needs the edges the merge walked. Every loaded file
    /// has an entry (empty when it imports nothing).
    pub imports: std::collections::BTreeMap<String, Vec<String>>,
}

// ---------------- internals ----------------

struct Loader {
    manifest_root: PathBuf,
    files: BTreeMap<String, FileUnit>, // file_id → unit
    /// v0.0.12 G-026: per-file source snapshot, populated the moment a
    /// file is read — *before* lex/parse. If lex or parse fails, the
    /// LoadFailure can still attach a real source so the diagnostic
    /// renders a proper line/column span instead of falling back to 1:1.
    raw_sources: BTreeMap<PathBuf, String>,
    by_canonical: BTreeMap<PathBuf, String>, // canonical_path → file_id
    edges: BTreeMap<String, Vec<String>>,    // file_id → imported file_ids
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
    /// v0.0.14 LSP dirty-buffer overlay: canonical-path → unsaved buffer text.
    /// When a file being loaded has an entry here, its in-editor (unsaved)
    /// contents are used instead of the on-disk bytes, so graph/type-at/
    /// value-refs reflect edits before save. Empty for the compile path.
    overlays: BTreeMap<PathBuf, String>,
    /// This project's own package name, from `[package].name`.
    ///
    /// A module's identity must not depend on WHO is compiling it. When stdlib
    /// builds itself, `src/text.cplus` is inside its own root and would derive
    /// `src.text`; when a consumer builds it, the same file is outside their
    /// root and derives `stdlib.src.text`. The two disagree by exactly the
    /// package prefix, so an archive built by the package exports
    /// `_src.text.from_str` while every consumer emits calls to
    /// `_stdlib.src.text.from_str` — and the link fails. Qualifying a
    /// package's own files with its name makes the identity the same either
    /// way.
    package_name: Option<String>,
}

struct LoaderState {
    files: BTreeMap<String, FileUnit>,
    edges: BTreeMap<String, Vec<String>>,
}

impl Loader {
    fn with_deps(manifest_root: PathBuf, deps: BTreeSet<String>) -> Self {
        Self {
            manifest_root,
            files: BTreeMap::new(),
            raw_sources: BTreeMap::new(),
            by_canonical: BTreeMap::new(),
            edges: BTreeMap::new(),
            deps,
            project_mode: false,
            overlays: BTreeMap::new(),
            package_name: None,
        }
    }

    /// Read `[package].name` from the project's manifest, once.
    /// The package name comes from the real manifest parser.
    ///
    /// It used to come from a hand-rolled line scan whose value was
    /// `v.trim().trim_matches('"').trim()` — which keeps a trailing comment. A
    /// manifest reading `name = "tomly" # the app` gave the resolver the
    /// package identity `tomly # the app`, sanitized into the linker symbol
    /// `_tomly____the_app.src.util.helper` (reports/bug-18). The package name
    /// is the mangled-symbol prefix, the self-import rule's input, and the
    /// prebuilt-archive linkage key, so the resolver's idea of the package
    /// disagreed with every other subsystem's.
    ///
    /// A malformed or absent manifest leaves the name unset, exactly as the
    /// scanner's early returns did — the resolver is not the place that
    /// reports manifest errors.
    fn load_package_name(&mut self) {
        let path = self.manifest_root.join("Cplus.toml");
        if let Ok(m) = crate::manifest::load(&path) {
            if !m.package.name.is_empty() {
                self.package_name = Some(m.package.name);
            }
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
                    importing_file: importing_file
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| path.to_path_buf()),
                    import_span: import_span
                        .as_ref()
                        .map(|(s, _)| *s)
                        .unwrap_or(Span::new(0, 0)),
                    requested: import_span
                        .map(|(_, r)| r)
                        .unwrap_or_else(|| path.display().to_string()),
                    resolved: path.to_path_buf(),
                });
            }
        };
        if let Some(file_id) = self.by_canonical.get(&canonical) {
            return Ok(file_id.clone());
        }

        // v0.0.14 LSP dirty-buffer overlay: prefer unsaved in-editor contents
        // for this file when the caller supplied them; else read from disk.
        let raw_source = match self.overlays.get(&canonical) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(&canonical).map_err(|e| ResolveError::Io {
                path: canonical.clone(),
                source: e,
            })?,
        };
        // Slice 5DOC: doctest extraction runs per-file before lexing so
        // synthesized `#[test]` functions become part of the loaded unit
        // and participate in `attrs::discover_tests` later. Files without
        // doctest fences are unchanged.
        let source = crate::doctest::extract(&raw_source);
        // v0.0.12 G-026: register the source BEFORE lex/parse so a failure
        // in either step lands in LoadFailure with a real source attached.
        // Without this the diagnostic falls back to a 1:1 span when the
        // first parse error hits the entry file.
        self.raw_sources.insert(canonical.clone(), source.clone());
        // v0.0.22 file-aware spans: derive + intern the file id BEFORE
        // lexing so every span this file produces carries it. Diagnostics
        // and monomorphization route by `span.file` from here on; the
        // per-item `origin_file` strings remain as the fallback for
        // synthesized (file-less) spans.
        let file_id = derive_file_id_qualified(
            &canonical,
            &self.manifest_root,
            self.package_name.as_deref(),
        );
        let file_idx = crate::lexer::intern_file(&file_id);
        let tokens =
            crate::lexer::tokenize_with_file(&source, file_idx).map_err(|e| ResolveError::Lex {
                path: canonical.clone(),
                source: e,
            })?;
        let program = crate::parser::parse(tokens).map_err(|e| ResolveError::Parse {
            path: canonical.clone(),
            source: e,
        })?;

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
        let import_dir = canonical
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        for imp in &program.imports {
            let target_path =
                self.classify_import_path(&imp.path, &import_dir, &canonical, imp.span)?;
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
            path_str,
            import_dir,
            importing_canonical,
            span,
            &self.manifest_root,
            &self.deps,
            self.package_name.as_deref(),
            self.project_mode,
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
    own_package: Option<&str>,
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
        // Security: a relative import must stay within the project tree, the
        // same boundary the vendor path enforces via E0859. Without this a
        // `../../../../etc/...` chain resolved and loaded arbitrary on-disk
        // `.cplus` files into the build. Only enforced in project mode
        // (single-file mode has no project boundary, and sibling `../` imports
        // are expected there).
        if project_mode && relative_import_escapes_root(import_dir, &extensionless, manifest_root) {
            return Err(ResolveError::RelativeEscape {
                importing_file: importing_canonical.to_path_buf(),
                import_span: span,
                requested: path_str.to_string(),
            });
        }
        let mut p = import_dir.join(&extensionless);
        if p.extension().is_none() {
            p.set_extension("cplus");
        }
        // Relative imports reach the reactor too (executor/time/net do
        // `import "./reactor"`), so platform selection must apply here.
        return Ok(platform_override(p));
    }

    if !project_mode {
        // Pre-Slice-2B compat: single-file mode (no manifest). Treat as
        // file-relative so older callers keep working.
        let mut p = import_dir.join(&extensionless);
        if p.extension().is_none() {
            p.set_extension("cplus");
        }
        return Ok(platform_override(p));
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

    // A package may name ITSELF. `appkit_ext.cplus` writes `import
    // "appkit/appkit"`, which resolves through `vendor/appkit/` for every
    // consumer — but inside appkit's own build there is no `vendor/appkit`,
    // and a package is not its own dependency. Both spellings must mean the
    // same module, or a package with modules that cross-reference by name can
    // never be compiled on its own. Resolve to this project's `src/`.
    if own_package == Some(first) && !rest.is_empty() {
        let mut p = manifest_root.join("src");
        for seg in &rest {
            p.push(seg);
        }
        p.set_extension("cplus");
        if p.is_file() {
            return Ok(platform_override(p));
        }
    }

    if !deps.contains(first) {
        // Declared, but only for other platforms (`[<platform>.dependencies]`)?
        // Report the platform gate, not "not a declared dependency" — the
        // dep IS declared; it just doesn't exist where this build is going.
        if let Some(platforms) = crate::target::platform_gated_dep(first) {
            return Err(ResolveError::PlatformGatedPackage {
                importing_file: importing_canonical.to_path_buf(),
                import_span: span,
                requested: path_str.to_string(),
                package: first.to_string(),
                platforms,
                active: crate::target::active_platform(),
            });
        }
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

    // v0.0.21 embedded profile: the selected target may exclude stdlib
    // modules whose mechanism it lacks. The gate lives here (not sema)
    // so the error points at the offending `import` line. Gated modules'
    // stdlib-internal consumers are themselves in the list, so relative
    // imports inside the package (`./reactor` from executor) can't
    // bypass it.
    if first == "stdlib" {
        if let Some(module) = rest.first() {
            let tgt = crate::target::active_target();
            if tgt.unsupported_stdlib.contains(module) {
                return Err(ResolveError::TargetGatedModule {
                    importing_file: importing_canonical.to_path_buf(),
                    import_span: span,
                    requested: path_str.to_string(),
                    target_name: tgt.name,
                });
            }
        }
    }

    // Binary / mixed mode: when a package has binaries for consumers, its
    // public surface is the generated header in `lib/include/`, not `src/`.
    // The consumer must not have to know which — `import "stdlib/text"` is the
    // same line either way — so the choice is made here, once.
    //
    // Gated on the MANIFEST, never on the mere presence of the directory:
    // `cpc headers` may leave `lib/include/` behind in a package that still
    // ships as source, and compiling against declarations with no archive to
    // link would fail at link time with undefined symbols.
    //
    // Per-module fallback below is what makes MIXED mode work: a generic
    // module has no header of its own (it ships verbatim), so it misses here
    // and resolves from `src/` like any source module.
    let pkg_root = manifest_root.join("vendor").join(first);
    if package_resolves_through_headers(&pkg_root) {
        let mut hdr = pkg_root.join("lib").join("include");
        for seg in &rest {
            hdr.push(seg);
        }
        hdr.set_extension("cplus");
        if hdr.is_file() {
            return Ok(platform_override(hdr));
        }
    }

    let mut p = manifest_root.to_path_buf();
    p.push("vendor");
    p.push(first);
    p.push("src");
    for seg in &rest {
        p.push(seg);
    }
    p.set_extension("cplus");
    // Vendor-package self-test fallback: when run from inside a vendor
    // package (e.g. `cpc test` in vendor/uuid/), the package's own
    // manifest_root has no vendor/ subdir, but sibling vendor packages
    // live at `<manifest_root>/../<dep>/`. Try that layout if the
    // primary path doesn't resolve. This keeps `cpc test` working in
    // package directories without requiring per-package vendor symlinks.
    if !p.is_file() {
        if let Some(parent) = manifest_root.parent() {
            let mut alt = parent.to_path_buf();
            alt.push(first);
            alt.push("src");
            for seg in &rest {
                alt.push(seg);
            }
            alt.set_extension("cplus");
            if alt.is_file() {
                return Ok(platform_override(alt));
            }
        }
    }
    Ok(platform_override(p))
}

/// True iff resolving the relative import `rel` against `import_dir` escapes
/// the importing file's own PACKAGE tree.
///
/// The boundary is the nearest ancestor of `import_dir` that holds a
/// `Cplus.toml` (the importing file's package root), falling back to
/// `manifest_root`. This is deliberately the *package* root, not the consumer's
/// `manifest_root`: a dependency's source lives OUTSIDE the consumer root (the
/// loader resolves `stdlib` from `<root>/../stdlib/`), and its own internal
/// `import "./option"` must still resolve — while a `../../etc/...` chain that
/// leaves the package is rejected, symmetric with the vendor path's E0859.
///
/// Canonicalizes the anchors (they exist on disk). This helper is only reached
/// in project mode, so if the boundary can't be established it FAILS CLOSED —
/// returns `true` (treat as an escape, reject the import) rather than letting an
/// unverifiable path through. `import_dir` is `canonical.parent()` of a file we
/// just read, so canonicalize here re-canonicalizes an already-canonical path
/// and effectively never fails on a real build; the fail-closed branches guard
/// only the pathological cases (broken symlink component, racy/partial checkout,
/// a project with no discoverable root) where "no boundary" must NOT mean "no
/// enforcement." Single-file mode never calls this (the caller gates on
/// `project_mode`).
fn relative_import_escapes_root(import_dir: &Path, rel: &str, manifest_root: &Path) -> bool {
    let base = match import_dir.canonicalize() {
        Ok(b) => b,
        Err(_) => return true,
    };
    let root = {
        let mut cur: Option<&Path> = Some(base.as_path());
        let mut found: Option<PathBuf> = None;
        while let Some(d) = cur {
            if d.join("Cplus.toml").is_file() {
                found = Some(d.to_path_buf());
                break;
            }
            cur = d.parent();
        }
        match found.or_else(|| manifest_root.canonicalize().ok()) {
            Some(r) => r,
            None => return true,
        }
    };
    let mut parts: Vec<std::ffi::OsString> = base
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(std::ffi::OsString::from(other)),
        }
    }
    let resolved: PathBuf = parts.iter().collect();
    !resolved.starts_with(&root)
}

/// Platform-specific source override. Some modules have an OS-specific
/// implementation that can't share one source file (e.g. the async reactor:
/// kqueue on macOS, epoll on Linux, and C+ has no in-source `cfg`).
/// Convention: a sibling `<name>_<platform>.cplus` next to `<name>.cplus`
/// shadows the base file on that platform. When present for the current
/// target, it is loaded in place of the base — transparently, since these
/// modules export the same public symbols.
///
/// The suffix comes from `target::active_platform()` — the SELECTED target's
/// platform, not the compiler host's OS — so a `--target ios-arm64` build
/// from a Mac looks for `_ios` files, never `_macos` ones. (Pre-platform-deps
/// this keyed off `cfg!(target_os)`, which conflated host and target.) The
/// platform names are the same vocabulary `[<platform>.dependencies]` uses.
fn platform_override(p: PathBuf) -> PathBuf {
    let os_suffix = format!("_{}", crate::target::active_platform());
    // Only base `.cplus` files participate; never double-suffix.
    let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
        return p;
    };
    if stem.ends_with(os_suffix.as_str()) {
        return p;
    }
    let candidate = p.with_file_name(format!("{stem}{os_suffix}.cplus"));
    if candidate.is_file() {
        candidate
    } else {
        p
    }
}

/// The file id becomes the mangled-symbol prefix for every item in the file,
/// so it must be a function of the file's position INSIDE its package — never
/// of where the package happens to sit on this machine.
///
/// A dependency reached through a `vendor/` symlink canonicalizes outside the
/// consumer's manifest root, so `strip_prefix` fails for it. Before 2026-07-26
/// the fallback was the whole absolute path, which put
/// `Users.adel.Workspace.C_.vendor.appkit.src.appkit.<item>` into ~18k symbols:
/// builds were not reproducible across machines, the developer's home path
/// leaked into every shipped binary, and a prebuilt `.a` could never link
/// anywhere but the exact path that produced it. `vendor` anchoring fixes all
/// three — the id becomes `appkit.src.appkit.<item>` wherever the tree lives.
///
/// Both real package layouts end in `<package>/src/<module>`: the normal
/// `<root>/vendor/<pkg>/src/...` and the package-self-test sibling fallback
/// (`<root>/../<pkg>/src/...`, which canonicalizes back under `vendor/` too).

/// Does this package's public surface resolve through `lib/include/`?
///
/// True when the package has binaries for consumers to link — either its own
/// (`[build] prebuild = true`) or the author's (`[link] bundled = [...]`) — and
/// false whenever `[build] dev = true`, which is the point of that key: the
/// package is being worked on, so consumers compile its `src/` no matter what
/// binaries are lying around.
///
/// Read straight from the file rather than threaded through: import resolution
/// does not otherwise carry dependency manifests, and this runs once per
/// imported module. A package with no manifest, or one that fails to parse, is
/// treated as source-only — the conservative answer, since it keeps the build
/// compiling from `src/` rather than silently switching to headers.
/// The decision comes from the manifest's own `BuildSpec`.
///
/// This was a second hand-rolled scan, whose final line was a verbatim copy of
/// `BuildSpec::resolves_through_headers` — one decision stated in two places,
/// reading the file two different ways. The stated excuse ("parsing the whole
/// manifest here would pull the manifest module into the resolver's dependency
/// surface") does not hold: it is the same crate (reports/bug-18).
fn package_resolves_through_headers(pkg_root: &Path) -> bool {
    let Ok(m) = crate::manifest::load(&pkg_root.join("Cplus.toml")) else {
        return false;
    };
    let ships_bundled = m.link.as_ref().is_some_and(|l| !l.bundled.is_empty());
    m.build.resolves_through_headers(ships_bundled)
}

/// `derive_file_id`, but qualified with the project's own package name when the
/// file lives inside it.
///
/// This is what makes a module's identity independent of who compiles it — see
/// `Loader::package_name`. Without it, a package's own build and a consumer's
/// build of the same file produce symbol names that differ by the package
/// prefix, and a prebuilt archive can never link.
fn derive_file_id_qualified(
    canonical: &Path,
    manifest_root: &Path,
    package_name: Option<&str>,
) -> String {
    let base = derive_file_id(canonical, manifest_root);
    let Some(pkg) = package_name else {
        return base;
    };
    // Only files INSIDE this project need qualifying. A dependency was already
    // re-rooted at its own package boundary by `package_relative`, so it
    // carries its package name already — prefixing again would give
    // `iris.stdlib.src.text`. `vendor/` is a dependency wherever it sits: a
    // real subdirectory is still not this project's code.
    let canonical_root =
        std::fs::canonicalize(manifest_root).unwrap_or_else(|_| manifest_root.to_path_buf());
    let own = matches!(canonical.strip_prefix(&canonical_root), Ok(r) if !path_is_under_vendor(r));
    if !own {
        return base;
    }
    let pkg = sanitize_id(pkg);
    if base.is_empty() || base == "root" {
        pkg
    } else {
        format!("{pkg}.{base}")
    }
}

/// Map arbitrary text to the `[A-Za-z0-9_.]` shape LLVM accepts in a symbol.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn derive_file_id(canonical: &Path, manifest_root: &Path) -> String {
    // A generated header stands in for its source module and must share its id.
    let rewritten = header_path_as_source(canonical);
    let canonical: &Path = rewritten.as_deref().unwrap_or(canonical);
    let canonical_root =
        std::fs::canonicalize(manifest_root).unwrap_or_else(|_| manifest_root.to_path_buf());
    let rel: &Path = match canonical.strip_prefix(&canonical_root) {
        // Inside the project, and not under `vendor/`: the path relative to
        // the root is already machine-independent.
        Ok(r) if !path_is_under_vendor(r) => r,
        // A dependency. Anchor on the package boundary instead of the path we
        // happen to have reached it by — including when `vendor/` is a real
        // directory inside this project rather than a symlink out of it. Get
        // this wrong and the same module compiles to `app.vendor.mathy.src.x`
        // here and `mathy.src.x` in the package's own build, so a prebuilt
        // archive never links.
        _ => package_relative(canonical).unwrap_or(canonical),
    };
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
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Rewrite a `lib/include/` path to the `src/` path it stands for.
///
/// A header and the source it was generated from MUST produce the same file id,
/// because the id becomes the mangled-symbol prefix. The archive was compiled
/// from `src/text.cplus` as `stdlib.src.text.*`; a consumer compiling against
/// `lib/include/text.cplus` would otherwise derive `stdlib.lib.include.text.*`
/// and fail to link against symbols that exist under a different name.
///
/// Doing it here means every id consumer — mangling, diagnostics, monomorphize
/// routing — sees one identity for a module regardless of which form of it was
/// read.
fn header_path_as_source(p: &Path) -> Option<PathBuf> {
    let comps: Vec<std::path::Component<'_>> = p.components().collect();
    // ... /<pkg>/lib/include/<rest...>
    let i = comps
        .iter()
        .rposition(|c| comp_name(c) == Some("include"))?;
    if i == 0 || comp_name(&comps[i - 1]) != Some("lib") {
        return None;
    }
    let mut out = PathBuf::new();
    for c in &comps[..i - 1] {
        out.push(c.as_os_str());
    }
    out.push("src");
    for c in &comps[i + 1..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

/// Re-root an out-of-project path at its package boundary, so the resulting id
/// is identical on every machine.
///
/// Preference order:
///   1. everything after the last `vendor` component — `appkit/src/appkit`
///   2. else `<parent-of-src>/src/<rest>` — covers a package laid out without
///      a `vendor` ancestor
///   3. else `None`, and the caller keeps its existing behaviour
///
/// Returning a path (not a string) keeps the component walk, `.cplus`
/// stripping and LLVM sanitisation in one place in the caller.
/// Does this project-relative path run through a `vendor/` directory?
///
/// The marker of a dependency. Checked against the path relative to the
/// manifest root, never the absolute one, so a project that merely happens to
/// live under a directory called `vendor` is not mistaken for its own dep.
fn path_is_under_vendor(rel: &Path) -> bool {
    rel.components().any(|c| comp_name(&c) == Some("vendor"))
}

fn package_relative(canonical: &Path) -> Option<&Path> {
    let comps: Vec<std::path::Component<'_>> = canonical.components().collect();

    // 1. after the last `vendor`
    if let Some(i) = comps.iter().rposition(|c| comp_name(c) == Some("vendor")) {
        if i + 1 < comps.len() {
            return Some(sub_path(canonical, comps.len() - (i + 1)));
        }
    }
    // 2. the package dir immediately above `src`
    if let Some(i) = comps.iter().rposition(|c| comp_name(c) == Some("src")) {
        if i >= 1 {
            return Some(sub_path(canonical, comps.len() - (i - 1)));
        }
    }
    None
}

fn comp_name<'a>(c: &std::path::Component<'a>) -> Option<&'a str> {
    match c {
        std::path::Component::Normal(s) => s.to_str(),
        _ => None,
    }
}

/// The last `n` components of `p`, as a borrowed sub-path.
fn sub_path(p: &Path, n: usize) -> &Path {
    let mut cur = p;
    while cur.components().count() > n {
        cur = strip_first_component(cur);
    }
    cur
}

/// Drop the leading component of `p` (e.g. `/a/b/c` -> `a/b/c` -> `b/c`).
fn strip_first_component(p: &Path) -> &Path {
    let mut it = p.components();
    match it.next() {
        Some(_) => it.as_path(),
        None => p,
    }
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
                    .map(|id| {
                        files
                            .get(id)
                            .map(|f| f.canonical_path.clone())
                            .unwrap_or_else(|| PathBuf::from(id))
                    })
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

/// v0.0.24 #10: visibility is name-based — a top-level item or method is
/// exported (public to importers) unless its name starts with `_`, which marks
/// it module-private (the Dart model: public by default, `_` to hide). Replaces
/// the old `pub`-flag gate.
fn exported_name(name: &str) -> bool {
    !name.starts_with('_')
}

/// An `extern fn` whose C+ NAME stays bare/global instead of being module-scoped:
///  - `export extern fn …` — an exported C-ABI symbol/definition whose whole purpose
///    is to expose a specific bare linker symbol (e.g. the `stdlib_reactor_*` runtime
///    helpers defined in C+ and called by the compiler's emitted code), and
///  - `__cplus_*` — compiler-runtime ABI reached by `#name` intrinsics from any module
///    (`#reactor_get_state` -> `__cplus_reactor_get_state`).
///
/// A plain (non-`export`) `extern fn` FFI import (`free`, `read`, `write`) is the
/// scoped case: private to its declaring module, so importers don't inherit libc
/// names bare into their namespace.
fn extern_stays_global(f: &Function) -> bool {
    f.is_extern
        && (f.is_pub
            || f.name
                .name
                .starts_with(crate::mangling::RUNTIME_ABI_PREFIX))
}

fn merge(
    files: BTreeMap<String, FileUnit>,
    entry_file_id: &str,
    is_lib_entry: bool,
    manifest_root: &Path,
    deps: &BTreeSet<String>,
    own_package: Option<&str>,
    project_mode: bool,
) -> Result<Program, ResolveError> {
    // Pre-pass: collect each file's local item names (used by the
    // rewriter to qualify unqualified references) AND its **public**
    // surface (slice 4B: gates cross-file access via E0403).
    //
    // `local_items` is everything declared at top level; `pub_items` is
    // the subset that's exported. `pub_methods[file_id][type_name]` is the
    // set of exported (non-`_`-prefixed) methods on that type — separately
    // tracked because methods live inside `impl` blocks. Enum variants
    // inherit the enum's visibility (no per-variant marker), so the variant
    // gate just re-checks `pub_items`.
    let mut local_items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pub_items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pub_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    // Existence, separately from exportedness — the same `local_items` /
    // `pub_items` pairing the item side already has. Without it a name that is
    // simply ABSENT is indistinguishable from one that is present and private,
    // so a typo was reported as a privacy violation (reports/bug-22).
    let mut all_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut item_kind: BTreeMap<String, BTreeMap<String, ItemKindTag>> = BTreeMap::new();
    for (fid, unit) in &files {
        let mut all: BTreeSet<String> = BTreeSet::new();
        let mut pubs: BTreeSet<String> = BTreeSet::new();
        let mut methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut all_meths: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut kinds: BTreeMap<String, ItemKindTag> = BTreeMap::new();
        for it in &unit.program.items {
            match &it.kind {
                ItemKind::Function(f) => {
                    // Compiler-runtime externs (`__cplus_*`) stay GLOBAL and bare —
                    // they are the runtime ABI, reached by `#name` intrinsics from any
                    // module (`#reactor_get_state` -> `__cplus_reactor_get_state`),
                    // not user-facing names. Skip qualification entirely.
                    if extern_stays_global(f) {
                        continue;
                    }
                    all.insert(f.name.name.clone());
                    kinds.insert(f.name.name.clone(), ItemKindTag::Function);
                    // `extern fn` is MODULE-PRIVATE: it is qualified within its
                    // declaring module (so a local can shadow it and, crucially, an
                    // importer does NOT get libc names like `free`/`read`/`write`
                    // dumped bare into its namespace), but it is never exported —
                    // not even as `pkg::free`. Only the linker symbol it binds is
                    // global; the C+ *name* is a private implementation detail. A
                    // regular fn exports by name (Dart model: public unless
                    // `_`-prefixed).
                    if !f.is_extern && exported_name(&f.name.name) {
                        pubs.insert(f.name.name.clone());
                    }
                }
                ItemKind::Enum(e) => {
                    all.insert(e.name.name.clone());
                    kinds.insert(e.name.name.clone(), ItemKindTag::Enum);
                    if exported_name(&e.name.name) {
                        pubs.insert(e.name.name.clone());
                    }
                }
                ItemKind::Struct(s) => {
                    all.insert(s.name.name.clone());
                    kinds.insert(s.name.name.clone(), ItemKindTag::Struct);
                    if exported_name(&s.name.name) {
                        pubs.insert(s.name.name.clone());
                    }
                }
                ItemKind::Impl(b) => {
                    let entry = methods.entry(b.target.name.clone()).or_default();
                    let all_entry = all_meths.entry(b.target.name.clone()).or_default();
                    for m in &b.methods {
                        all_entry.insert(m.name.name.clone());
                        if exported_name(&m.name.name) {
                            entry.insert(m.name.name.clone());
                        }
                    }
                }
                // Slice 7GEN.3: interface declarations register as items.
                // Cross-file `impl Type: Interface` blocks reference
                // the interface by name; pub-status gates cross-file use.
                ItemKind::Interface(i) => {
                    all.insert(i.name.name.clone());
                    kinds.insert(i.name.name.clone(), ItemKindTag::Interface);
                    if exported_name(&i.name.name) {
                        pubs.insert(i.name.name.clone());
                    }
                }
                // Phase 11 polish: aliases register as ordinary type-level
                // names so cross-file re-export lookups + import-alias
                // rewrites apply.
                ItemKind::TypeAlias(a) => {
                    all.insert(a.name.name.clone());
                    kinds.insert(a.name.name.clone(), ItemKindTag::TypeAlias);
                    if exported_name(&a.name.name) {
                        pubs.insert(a.name.name.clone());
                    }
                }
                // v0.0.9 Phase 4: const/static register as value-level
                // names. Cross-file `prefix::FOO` path expressions go
                // through the same pub gate as functions; the rewriter
                // collapses a `prefix::FOO` path to the qualified ident
                // and the path-expression sema treats it as a value.
                ItemKind::Const(c) => {
                    all.insert(c.name.name.clone());
                    kinds.insert(c.name.name.clone(), ItemKindTag::Const);
                    if exported_name(&c.name.name) {
                        pubs.insert(c.name.name.clone());
                    }
                }
                ItemKind::Static(s) => {
                    all.insert(s.name.name.clone());
                    kinds.insert(s.name.name.clone(), ItemKindTag::Static);
                    if exported_name(&s.name.name) {
                        pubs.insert(s.name.name.clone());
                    }
                }
                // v0.0.15: module-scope `#asm("...")` declares no name —
                // nothing to record in the local/pub symbol tables.
                ItemKind::ModuleAsm(_) => {}
            }
        }
        local_items.insert(fid.clone(), all);
        pub_items.insert(fid.clone(), pubs);
        pub_methods.insert(fid.clone(), methods);
        all_methods.insert(fid.clone(), all_meths);
        item_kind.insert(fid.clone(), kinds);
    }

    let mut imports_by_file: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (fid, unit) in &files {
        let mut imports_map: BTreeMap<String, String> = BTreeMap::new();
        let mut first_span_for: BTreeMap<String, Span> = BTreeMap::new();
        for imp in &unit.program.imports {
            // STRM v3 (2026-08-01): `as _` binds no name — skip the alias
            // maps entirely (so `_::x` never resolves) and exempt it from
            // duplicate-prefix detection (any number of discard imports is
            // legal). The file itself was already pulled into the build by
            // the loader's path walk, which is alias-independent.
            if imp.as_name.name == "_" {
                continue;
            }
            if let Some(first) = first_span_for.get(&imp.as_name.name) {
                return Err(ResolveError::DuplicatePrefix {
                    file: unit.canonical_path.clone(),
                    prefix: imp.as_name.name.clone(),
                    first_span: *first,
                    second_span: imp.as_name.span,
                });
            }
            let import_dir = unit
                .canonical_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let target_path = classify_import_path(
                &imp.path,
                &import_dir,
                &unit.canonical_path,
                imp.span,
                manifest_root,
                deps,
                own_package,
                project_mode,
            )?;
            let target_canon = std::fs::canonicalize(&target_path).unwrap_or(target_path);
            if let Some(target_id) = files
                .iter()
                .find(|(_, u)| u.canonical_path == target_canon)
                .map(|(id, _)| id.clone())
            {
                imports_map.insert(imp.as_name.name.clone(), target_id);
                first_span_for.insert(imp.as_name.name.clone(), imp.as_name.span);
            }
        }
        imports_by_file.insert(fid.clone(), imports_map);
    }

    let mut alias_targets: BTreeMap<String, BTreeMap<String, AliasTarget>> = BTreeMap::new();
    for (fid, unit) in &files {
        let Some(imports) = imports_by_file.get(fid) else {
            continue;
        };
        for it in &unit.program.items {
            let ItemKind::TypeAlias(a) = &it.kind else {
                continue;
            };
            if let Some(target) =
                resolve_alias_target(fid, &a.target, imports, &local_items, &item_kind)
            {
                alias_targets
                    .entry(fid.clone())
                    .or_default()
                    .insert(a.name.name.clone(), target);
            }
        }
    }

    let mut merged_items: Vec<Item> = Vec::new();
    for (fid, unit) in &files {
        let imports_map = imports_by_file.get(fid).cloned().unwrap_or_default();

        let ctx = RewriteCtx {
            self_file_id: fid.clone(),
            self_file_path: unit.canonical_path.clone(),
            entry_file_id: entry_file_id.to_string(),
            is_lib_entry,
            imports: imports_map,
            local_items: local_items.get(fid).cloned().unwrap_or_default(),
            pub_items: pub_items.clone(),
            pub_methods: pub_methods.clone(),
            all_methods: all_methods.clone(),
            item_kind: item_kind.clone(),
            alias_targets: alias_targets.clone(),
        };

        for it in &unit.program.items {
            let rewritten = rewrite_item(it, &ctx)?;
            merged_items.push(rewritten);
        }
    }

    Ok(Program {
        imports: Vec::new(),
        items: merged_items,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKindTag {
    Function,
    Struct,
    Enum,
    Interface,
    TypeAlias,
    /// v0.0.9 Phase 4: module-scope `const NAME: Ty = LIT;`. Lowered to
    /// the inlined literal at every use site; same cross-file pub gate
    /// as `Function`.
    Const,
    /// v0.0.9 Phase 4: module-scope `static NAME: Ty = LIT;`.
    /// Survives to codegen as an LLVM global; same cross-file pub gate
    /// as `Function`.
    Static,
}

#[derive(Debug, Clone)]
struct AliasTarget {
    target_id: String,
    name: String,
    kind: ItemKindTag,
}

struct RewriteCtx {
    self_file_id: String,
    self_file_path: PathBuf,
    entry_file_id: String,
    /// Phase 5 Slice 5.A: this project's root file is the entry of a
    /// `[lib]` target. Top-level items in `entry_file_id` skip mangling
    /// so C consumers can link against an exported `fn add` as the bare
    /// `_add` symbol. Files imported by the entry stay qualified normally —
    /// they're not part of the public C ABI.
    is_lib_entry: bool,
    /// Map of `as`-prefix → target file id.
    imports: BTreeMap<String, String>,
    /// Top-level item names declared in this file.
    local_items: BTreeSet<String>,
    /// Per-file public surface (slice 4B). `pub_items[file_id]` is the set
    /// of exported top-level item names (no leading `_`);
    /// `pub_methods[file_id][type]` is the set of exported methods on that
    /// type. Used to gate cross-file access (E0403). Same-file access
    /// ignores these.
    pub_items: BTreeMap<String, BTreeSet<String>>,
    pub_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// Every method name per type, exported or not — the method twin of
    /// `local_items`. Paired with `pub_methods` so a cross-file method access
    /// can tell "no such method" from "private method" (reports/bug-22).
    all_methods: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// `item_kind[file_id][name]` tags each top-level item as Function /
    /// Struct / Enum. Used to pick the right error phrasing for E0403 and
    /// to decide if a 3-segment path is `Enum::Variant` (variants inherit
    /// the enum's visibility) vs `Struct::method` (per-method visibility
    /// check).
    item_kind: BTreeMap<String, BTreeMap<String, ItemKindTag>>,
    /// Public type aliases can act as small facade re-exports. This maps
    /// `file_id::Alias` to the concrete type item it names.
    alias_targets: BTreeMap<String, BTreeMap<String, AliasTarget>>,
}

impl RewriteCtx {
    /// Qualified name for an item `name` declared in this file. The entry
    /// binary's `main` keeps its bare name so the linker entry point works.
    ///
    /// Phase 5 Slice 5.A: when this is a library target's entry file,
    /// every top-level name skips qualification — the bare names ARE the
    /// public ABI. Internal helpers also stay unqualified for MVP; Slice
    /// 5.B will mark module-private (`_`-prefixed) items with `internal`
    /// linkage so they don't leak as exported symbols.
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

    /// Check that top-level `name` is exported (non-`_`-prefixed) in
    /// `target_id` (cross-file). Same-file access is never blocked. Returns
    /// an E0403 if the item is module-private. The `kind` is best-effort —
    /// looked up from item_kind; defaults to Function when unknown so the
    /// diagnostic still names something.
    fn check_pub_item(&self, target_id: &str, name: &str, span: Span) -> Result<(), ResolveError> {
        if target_id == self.self_file_id {
            return Ok(());
        }
        // v0.0.12 G-030-bonus: separate "name doesn't exist in module"
        // from "name exists but isn't pub". Pre-fix both lumped into
        // PrivateAccess with the same "mark it `pub`" message, which
        // was misleading when the name truly wasn't there.
        let existing_kind = self
            .item_kind
            .get(target_id)
            .and_then(|m| m.get(name))
            .copied();
        let Some(raw_kind) = existing_kind else {
            return Err(ResolveError::UnknownItem {
                file: self.self_file_path.clone(),
                span,
                owner: target_id.to_string(),
                name: name.to_string(),
            });
        };
        let kind = match raw_kind {
            ItemKindTag::Function => PrivateKind::Function,
            ItemKindTag::Struct => PrivateKind::Struct,
            ItemKindTag::Enum => PrivateKind::Enum,
            ItemKindTag::Interface => PrivateKind::Interface,
            ItemKindTag::TypeAlias => PrivateKind::TypeAlias,
            ItemKindTag::Const => PrivateKind::Const,
            ItemKindTag::Static => PrivateKind::Static,
        };
        let is_pub = self
            .pub_items
            .get(target_id)
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

    /// v0.0.22 DSL.3: contextual builder lookup. Given a builder
    /// context path and a bare item name, return the qualified path
    /// segments to use (`[alias, name]`) if `name` is a top-level item
    /// of the context package — or `None` if there is no such member
    /// (bare name then falls through to normal/local resolution, which
    /// usually means a "no such identifier" sema error).
    ///
    /// Only single-segment contexts (`@view`) are resolved this way;
    /// multi-segment contexts (`@ui::view`) return `None` and require
    /// the item to be written qualified, matching the path-resolution
    /// limits noted in DSL.2. Membership uses `item_kind` (existence,
    /// pub or not) so a private member still rewrites and then earns the
    /// precise PrivateAccess error from the ordinary path rewrite, not a
    /// vaguer "unknown identifier".
    fn builder_context_member(&self, context: &[Ident], name: &str) -> Option<Vec<Ident>> {
        let [alias] = context else {
            return None;
        };
        let target_id = self.imports.get(&alias.name)?;
        let is_member = self
            .item_kind
            .get(target_id)
            .is_some_and(|m| m.contains_key(name));
        if !is_member {
            return None;
        }
        Some(vec![
            alias.clone(),
            Ident {
                name: name.to_string(),
                span: alias.span,
            },
        ])
    }

    /// Check that method `method` on type `type_name` is exported
    /// (non-`_`-prefixed) in `target_id` (cross-file). Same-file access is
    /// never blocked.
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
        // Three-way, mirroring the item side's UnknownItem/PrivateAccess split:
        // absent, present-but-private, or exported. `unwrap_or(false)` on the
        // pub-only table conflated the first two, so `Gadget::nonexistent(g)`
        // was reported as "is private (drop the `_` to export)" — advice about
        // a `_` that isn't there, on a method that doesn't exist
        // (reports/bug-22).
        let exists = self
            .all_methods
            .get(target_id)
            .and_then(|m| m.get(type_name))
            .is_some_and(|s| s.contains(method));
        if !exists {
            return Err(ResolveError::UnknownMethod {
                file: self.self_file_path.clone(),
                span,
                owner: type_name.to_string(),
                name: method.to_string(),
            });
        }
        let is_pub = self
            .pub_methods
            .get(target_id)
            .and_then(|m| m.get(type_name))
            .is_some_and(|s| s.contains(method));
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
        if let Some(target) = self.resolve_alias_target(target_id, name) {
            return target.kind == ItemKindTag::Enum;
        }
        matches!(
            self.item_kind.get(target_id).and_then(|m| m.get(name)),
            Some(ItemKindTag::Enum)
        )
    }

    /// issue-09: resolve ONE possibly-prefixed item name to its qualified
    /// form. Every syntactic position that can name a type or an item — a
    /// struct literal, a generic struct literal, a generic enum call, a
    /// variant pattern, a type reference — asks this.
    ///
    /// It used to be inlined at seven arms with divergent behavior: a type
    /// reference to an unknown prefix was E0402 here in the resolver, while
    /// the same typo in a struct literal fell silently through to sema's
    /// E0303 ("unknown type"), which names a type the user never wrote. Two
    /// error universes for one mistake. And the alias-facade hop
    /// (`resolve_alias_target`) was consulted for a plain struct literal but
    /// not for the generic one, so a re-exported generic type resolved in one
    /// spelling and not the other.
    ///
    /// `Ok(None)` means "not prefixed, not local" — a primitive, a builtin, a
    /// generic parameter: leave the name alone.
    fn resolve_item_name(&self, name: &str, span: Span) -> Result<Option<String>, ResolveError> {
        if let Some((prefix, rest)) = name.split_once("::") {
            let Some(target_id) = self.imports.get(prefix) else {
                return Err(ResolveError::UnknownPrefix {
                    file: self.self_file_path.clone(),
                    span,
                    prefix: prefix.to_string(),
                });
            };
            // Slice 4B: a cross-file reference requires the item to be
            // exported (non-`_`). Field-level visibility is sema's.
            self.check_pub_item(target_id, rest, span)?;
            // A module may re-export another module's type; follow the facade
            // so every spelling lands on the declaring module.
            return Ok(Some(match self.resolve_alias_target(target_id, rest) {
                Some(t) => self.qualify_external(&t.target_id, &t.name),
                None => self.qualify_external(target_id, rest),
            }));
        }
        if self.local_items.contains(name) {
            return Ok(Some(self.qualify_local(name)));
        }
        Ok(None)
    }

    fn resolve_alias_target(&self, target_id: &str, name: &str) -> Option<AliasTarget> {
        let mut current_id = target_id.to_string();
        let mut current_name = name.to_string();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        loop {
            if !seen.insert((current_id.clone(), current_name.clone())) {
                return None;
            }
            let target = self
                .alias_targets
                .get(&current_id)
                .and_then(|m| m.get(&current_name))
                .cloned()?;
            if target.kind != ItemKindTag::TypeAlias {
                return Some(target);
            }
            current_id = target.target_id;
            current_name = target.name;
        }
    }
}

fn resolve_alias_target(
    fid: &str,
    target: &Type,
    imports: &BTreeMap<String, String>,
    local_items: &BTreeMap<String, BTreeSet<String>>,
    item_kind: &BTreeMap<String, BTreeMap<String, ItemKindTag>>,
) -> Option<AliasTarget> {
    let TypeKind::Path(path) = &target.kind else {
        return None;
    };
    let (target_id, name) = if let Some((prefix, rest)) = path.split_once("::") {
        (imports.get(prefix)?.clone(), rest.to_string())
    } else if local_items
        .get(fid)
        .map(|s| s.contains(path))
        .unwrap_or(false)
    {
        (fid.to_string(), path.clone())
    } else {
        return None;
    };
    let kind = *item_kind.get(&target_id)?.get(&name)?;
    Some(AliasTarget {
        target_id,
        name,
        kind,
    })
}

fn rewrite_item(item: &Item, ctx: &RewriteCtx) -> Result<Item, ResolveError> {
    let kind = match &item.kind {
        ItemKind::Function(f) => ItemKind::Function(rewrite_fn(f, ctx)?),
        ItemKind::Enum(e) => {
            let mut e = e.clone();
            e.name.name = ctx.qualify_local(&e.name.name);
            qualify_bounds(&mut e.generic_params, ctx)?;
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
            qualify_bounds(&mut s.generic_params, ctx)?;
            for f in &mut s.fields {
                rewrite_type(&mut f.ty, ctx)?;
            }
            ItemKind::Struct(s)
        }
        ItemKind::Impl(b) => {
            let mut b = b.clone();
            // The target: a local name qualifies against this file; an
            // import-alias path (`impl core::Handle` — EXT.1 same-package
            // extension) resolves through the same machinery as type names,
            // so pub-gating and unknown-prefix errors match type references.
            // A bare foreign name stays as-is and surfaces as sema's E0325;
            // the package rule itself (E0387) is sema's, where both origins
            // are known.
            if ctx.local_items.contains(&b.target.name) {
                b.target.name = ctx.qualify_local(&b.target.name);
            } else if b.target.name.contains("::") {
                b.target.name = rewrite_type_name(&b.target.name, b.target.span, ctx)?;
            }
            qualify_bounds(&mut b.target_generic_params, ctx)?;
            for m in &mut b.methods {
                let new_method = rewrite_method(m, ctx)?;
                *m = new_method;
            }
            // Slice 7GEN.3: qualify the interface name if local; since
            // 2026-07-06 an import-alias path (`impl T: mod::Interface`)
            // also resolves, through the same machinery as type names.
            if let Some(iface) = &mut b.interface_name {
                iface.name = rewrite_type_name(&iface.name, iface.span, ctx)?;
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
        // v0.0.9 Phase 4: const items. Qualify the name and rewrite
        // the declared type. Initializer is a literal (no cross-file
        // path references possible inside it) — but for forward-
        // compatibility we still walk it in case a later slice admits
        // const-of-const initializers.
        ItemKind::Const(c) => {
            let mut c = c.clone();
            c.name.name = ctx.qualify_local(&c.name.name);
            rewrite_type(&mut c.ty, ctx)?;
            let mut scope: HashSet<String> = HashSet::new();
            rewrite_expr(&mut c.value, ctx, &mut scope)?;
            ItemKind::Const(c)
        }
        // v0.0.9 Phase 4: static items. Same shape as const — qualify
        // the name and rewrite the declared type.
        ItemKind::Static(s) => {
            let mut s = s.clone();
            s.name.name = ctx.qualify_local(&s.name.name);
            rewrite_type(&mut s.ty, ctx)?;
            let mut scope: HashSet<String> = HashSet::new();
            rewrite_expr(&mut s.value, ctx, &mut scope)?;
            ItemKind::Static(s)
        }
        // v0.0.15: module-scope `#asm("...")` is raw assembly with no names or
        // types — cross-file resolution has nothing to qualify; pass it through.
        ItemKind::ModuleAsm(ma) => ItemKind::ModuleAsm(ma.clone()),
    };
    Ok(Item {
        kind,
        span: item.span,
        origin_file: Some(ctx.self_file_id.clone()),
    })
}

/// Interface-bound qualification (2026-07-06): generic-param bounds
/// (`[B: Backend]`, `[C: mod::Component]`) resolve through the same
/// machinery as type names — import-alias paths rewrite to the target
/// module's qualified name (with the `_`-privacy check), local names
/// qualify to this module, and everything else (the blessed bounds:
/// Copy/Send/Sync/Hash/Eq/Ord/...) passes through bare. Interface
/// DECLARATIONS are module-qualified, so an unrewritten bound fails
/// the package-mode bound check with E0502 — single-file mode
/// qualifies nothing, which is why the mismatch only surfaced in
/// packages.
fn qualify_bounds(params: &mut [GenericParam], ctx: &RewriteCtx) -> Result<(), ResolveError> {
    for gp in params {
        for b in &mut gp.bounds {
            b.name = rewrite_type_name(&b.name, b.span, ctx)?;
        }
    }
    Ok(())
}

fn rewrite_fn(f: &Function, ctx: &RewriteCtx) -> Result<Function, ResolveError> {
    let mut f = f.clone();
    let local_scope = HashSet::new();
    // The C+ NAME is module-qualified even for `extern fn`, so it is scoped to
    // this module (a local can shadow it; importers don't see it bare). The
    // literal C SYMBOL it binds is preserved separately: sema sets the extern's
    // `link_name` to the bare declared name (recovered from the last path
    // segment) when no explicit `#[link_name]` is present, so codegen still emits
    // `@free`, not `@<module>.free`. Compiler-runtime externs (`__cplus_*`) are the
    // exception — they stay bare/global (the runtime ABI reached by `#name`).
    if !extern_stays_global(&f) {
        f.name.name = ctx.qualify_local(&f.name.name);
    }
    qualify_bounds(&mut f.generic_params, ctx)?;
    for p in &mut f.params {
        rewrite_type(&mut p.ty, ctx)?;
        // A default value is an expression too — resolve its import aliases,
        // module-qualified enum paths, and free-fn names, exactly as the body
        // is. Without this, `o: T = mod::Enum[..]::None` / a module fn default
        // reached sema unresolved (E0303 "unknown generic enum" / E0300).
        // Defaults are spliced at call sites before params bind, so they see
        // module scope only — a fresh, empty local scope.
        if let Some(def) = &mut p.default {
            let mut default_scope = HashSet::new();
            rewrite_expr(&mut **def, ctx, &mut default_scope)?;
        }
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
    qualify_bounds(&mut m.generic_params, ctx)?;
    for p in &mut m.params {
        rewrite_type(&mut p.ty, ctx)?;
        // Resolve default-value expressions (see rewrite_fn for the rationale).
        if let Some(def) = &mut p.default {
            let mut default_scope = HashSet::new();
            rewrite_expr(&mut **def, ctx, &mut default_scope)?;
        }
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
        // qualification — recurse into the inner type so a `prefix::T`
        // inside is qualified. (The surface `borrow REGION T` syntax was
        // retired in v0.0.24 #9; this variant only persists for any
        // already-parsed inner types.)
        TypeKind::Borrowed { inner, .. } => rewrite_type(inner, ctx)?,
        // Slice 7GEN.5c: `prefix::Pair[i32, bool]` — qualify the generic
        // name + recurse into each arg (args may themselves reference
        // qualified types).
        TypeKind::Generic { name, args } => {
            *name = rewrite_type_name(name, ty.span, ctx)?;
            for a in args.iter_mut() {
                rewrite_type(a, ctx)?;
            }
        }
        TypeKind::RawPtr(inner) => rewrite_type(inner, ctx)?,
        // Slice 11.FN_PTR: function pointer types — recurse into each
        // param type and the return type so cross-file references in
        // signature components are qualified.
        TypeKind::FnPtr {
            params,
            return_type,
            ..
        } => {
            for p in params.iter_mut() {
                rewrite_type(p, ctx)?;
            }
            if let Some(rt) = return_type.as_mut() {
                rewrite_type(rt, ctx)?;
            }
        }
        TypeKind::Slice(inner) => rewrite_type(inner, ctx)?,
        // v0.0.5 Phase 3 Slice 3B: tuple element types may themselves
        // reference cross-file types — recurse into each.
        TypeKind::Tuple(elems) => {
            for t in elems.iter_mut() {
                rewrite_type(t, ctx)?;
            }
        }
    }
    Ok(())
}

fn rewrite_type_name(s: &str, span: Span, ctx: &RewriteCtx) -> Result<String, ResolveError> {
    // Cross-file: `prefix::Type` (and only that shape — types can't be
    // 3-segment because there's no Type::Variant in type position).
    // Unqualified and not a local item: a primitive, a builtin or a generic
    // parameter — leave it alone.
    Ok(ctx.resolve_item_name(s, span)?.unwrap_or_else(|| s.to_string()))
}

fn rewrite_block(
    b: &mut Block,
    ctx: &RewriteCtx,
    scope: &mut HashSet<String>,
) -> Result<(), ResolveError> {
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

fn rewrite_stmt(
    s: &mut Stmt,
    ctx: &RewriteCtx,
    scope: &mut HashSet<String>,
) -> Result<(), ResolveError> {
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
        StmtKind::LetDestructure { fields, init, .. } => {
            rewrite_expr(init, ctx, scope)?;
            for f in fields {
                scope.insert(f.name.clone());
            }
        }
        StmtKind::Return(opt) => {
            if let Some(e) = opt {
                rewrite_expr(e, ctx, scope)?;
            }
        }
        StmtKind::While { cond, body, .. } => {
            rewrite_expr(cond, ctx, scope)?;
            rewrite_block(body, ctx, scope)?;
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                let snapshot = scope.clone();
                if let Some(init) = init {
                    rewrite_stmt(init, ctx, scope)?;
                }
                if let Some(c) = cond {
                    rewrite_expr(c, ctx, scope)?;
                }
                for u in update {
                    rewrite_expr(u, ctx, scope)?;
                }
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
        },
        StmtKind::Expr(e) => rewrite_expr(e, ctx, scope)?,
        StmtKind::Defer(e) => rewrite_expr(e, ctx, scope)?,
        StmtKind::IfLet {
            pattern,
            scrutinee,
            body,
            else_body,
            ..
        } => {
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
        StmtKind::Loop(body, _) => {
            rewrite_block(body, ctx, scope)?;
        }
        StmtKind::WhileLet {
            pattern,
            scrutinee,
            body,
            ..
        } => {
            rewrite_expr(scrutinee, ctx, scope)?;
            // Bindings from the loop pattern live inside the body only.
            let snapshot = scope.clone();
            rewrite_pattern(pattern, ctx, scope)?;
            rewrite_block(body, ctx, scope)?;
            *scope = snapshot;
        }
        StmtKind::GuardLet {
            pattern,
            scrutinee,
            complement,
            else_body,
            ..
        } => {
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

/// v0.0.22 DSL.3: rewrite bare item names inside a builder-block entry
/// expression to contextual paths (`text` → `view::text`). Applies the
/// precedence locals → same-file top-level → contextual: a name in
/// `locals` or `ctx.local_items` is left bare (normal resolution wins);
/// otherwise, if it is a member of the context package, it becomes a
/// two-segment `Path` that the ordinary path rewrite then resolves.
///
/// Walks the direct expression structure (calls, operators, indexing,
/// field receivers, literals' elements) but stops at nested
/// block-introducing constructs — `Block`, `If`, `Match`, and
/// nested `@`-blocks — which own their own scopes (and, for nested
/// builder blocks, their own context). Names inside those must be
/// written qualified; that is consistent with DSL.3's scope (item
/// constructors and modifier operands) and avoids tracking inner-block
/// bindings here.
/// v0.0.22 DSL.3/DSL.4: apply contextual lookup across a builder body.
/// Walks entries in source order, tracking block-level locals (each `let`
/// binding extends them for following entries). Leaf item exprs and
/// modifier operands are contextualized; `if`/`for` bodies recurse (the
/// `for` var binds inside its body). A container item expr is itself a
/// builder block — its bare names resolve in the SAME enclosing context,
/// so we just propagate the context into its (currently empty) `context`
/// field and leave its own entries to its later arm.
fn contextualize_entries(
    entries: &mut [BuilderEntry],
    context: &[Ident],
    locals: &HashSet<String>,
    ctx: &RewriteCtx,
) {
    let mut locals = locals.clone();
    for entry in entries {
        match entry {
            BuilderEntry::Let(s) => {
                if let StmtKind::Let {
                    init: Some(init), ..
                } = &mut s.kind
                {
                    contextualize_builder_idents(init, context, &locals, ctx);
                }
                if let StmtKind::Let { name, .. } = &s.kind {
                    locals.insert(name.name.clone());
                }
            }
            BuilderEntry::Item { expr, modifiers } => {
                // Container element (possibly at the head of a same-line
                // postfix chain, `hstack { ... }.gap(8.0)`): inherit the
                // enclosing context so its later arm builds `ctx::name`
                // and resolves its children in `ctx`. Its own entries are
                // contextualized then.
                fill_container_context(expr, context);
                // DSL.5: a container's arguments (`card(title: t) { ... }`)
                // belong to the ENCLOSING scope — sibling `let`s are
                // visible, the container's own entries are not — so they
                // contextualize here, not on the re-entry that handles the
                // container's children.
                contextualize_container_args(expr, context, &locals, ctx);
                if !matches!(expr.kind, ExprKind::BuilderBlock { .. }) {
                    // Chain args and ordinary items contextualize here;
                    // the walk stops at the container head (it owns its
                    // own scope/context).
                    contextualize_builder_idents(expr, context, &locals, ctx);
                }
                for m in modifiers {
                    // The modifier name itself (`m.name`) is a field/method on
                    // the current item, never a contextual lookup — only its
                    // operands are.
                    match &mut m.kind {
                        BuilderModifierKind::Assign(v) => {
                            contextualize_builder_idents(v, context, &locals, ctx)
                        }
                        BuilderModifierKind::Call(args) => {
                            for a in args {
                                contextualize_builder_idents(a, context, &locals, ctx);
                            }
                        }
                    }
                }
            }
            BuilderEntry::If { cond, then, else_ } => {
                contextualize_builder_idents(cond, context, &locals, ctx);
                contextualize_entries(then, context, &locals, ctx);
                if let Some(eb) = else_ {
                    contextualize_entries(eb, context, &locals, ctx);
                }
            }
            BuilderEntry::For { var, iter, body } => {
                contextualize_builder_idents(iter, context, &locals, ctx);
                let mut body_locals = locals.clone();
                body_locals.insert(var.name.clone());
                contextualize_entries(body, context, &body_locals, ctx);
            }
        }
    }
}

/// v0.0.22 DSL.4: a bare container element may sit at the head of a
/// same-line postfix chain (`hstack { ... }.gap(8.0)`). Descend through
/// `Call`/`Field`/`Index` receivers to that head and give an
/// empty-context container the enclosing block's context. A root
/// `@`-block head keeps its own context (its `context` is never empty).
fn fill_container_context(e: &mut Expr, context: &[Ident]) {
    match &mut e.kind {
        ExprKind::BuilderBlock {
            context: cctx,
            container: Some(_),
            ..
        } => {
            if cctx.is_empty() {
                *cctx = context.to_vec();
            }
        }
        ExprKind::Call { callee, .. } => fill_container_context(callee, context),
        ExprKind::Field { receiver, .. } => fill_container_context(receiver, context),
        ExprKind::Index { receiver, .. } => fill_container_context(receiver, context),
        _ => {}
    }
}

/// DSL.5: contextualize a container element's arguments, descending
/// through a postfix chain to the container head the same way
/// `fill_container_context` does. The arguments live in the enclosing
/// entry's scope; the container's own entries are handled on re-entry.
fn contextualize_container_args(
    e: &mut Expr,
    context: &[Ident],
    locals: &HashSet<String>,
    ctx: &RewriteCtx,
) {
    match &mut e.kind {
        ExprKind::BuilderBlock {
            container: Some(_),
            container_args,
            ..
        } => {
            for a in container_args {
                contextualize_builder_idents(a, context, locals, ctx);
            }
        }
        ExprKind::Call { callee, .. } => contextualize_container_args(callee, context, locals, ctx),
        ExprKind::Field { receiver, .. } => {
            contextualize_container_args(receiver, context, locals, ctx)
        }
        ExprKind::Index { receiver, .. } => {
            contextualize_container_args(receiver, context, locals, ctx)
        }
        _ => {}
    }
}

fn contextualize_builder_idents(
    e: &mut Expr,
    context: &[Ident],
    locals: &HashSet<String>,
    ctx: &RewriteCtx,
) {
    match &mut e.kind {
        ExprKind::Ident(name) => {
            if locals.contains(name) || ctx.local_items.contains(name) {
                return;
            }
            if let Some(segments) = ctx.builder_context_member(context, name) {
                e.kind = ExprKind::Path { segments };
            }
        }
        ExprKind::Call { callee, args, .. } => {
            contextualize_builder_idents(callee, context, locals, ctx);
            for a in args {
                contextualize_builder_idents(a, context, locals, ctx);
            }
        }
        ExprKind::Field { receiver, .. } => {
            contextualize_builder_idents(receiver, context, locals, ctx)
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            contextualize_builder_idents(lhs, context, locals, ctx);
            contextualize_builder_idents(rhs, context, locals, ctx);
        }
        ExprKind::Unary { operand, .. } => {
            contextualize_builder_idents(operand, context, locals, ctx)
        }
        ExprKind::Index { receiver, index } => {
            contextualize_builder_idents(receiver, context, locals, ctx);
            contextualize_builder_idents(index, context, locals, ctx);
        }
        ExprKind::Cast { expr, .. } => contextualize_builder_idents(expr, context, locals, ctx),
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                contextualize_builder_idents(s, context, locals, ctx);
            }
            if let Some(en) = end {
                contextualize_builder_idents(en, context, locals, ctx);
            }
        }
        ExprKind::ArrayLit { elements }
        | ExprKind::TupleLit { elements }
        | ExprKind::GenericEnumCall { args: elements, .. } => {
            for el in elements {
                contextualize_builder_idents(el, context, locals, ctx);
            }
        }
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            for f in fields {
                contextualize_builder_idents(&mut f.value, context, locals, ctx);
            }
        }
        ExprKind::ArrayFill { fill, .. } => {
            contextualize_builder_idents(fill, context, locals, ctx)
        }
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                contextualize_builder_idents(a, context, locals, ctx);
            }
        }
        ExprKind::InterpStr { parts } => {
            for p in parts {
                if let InterpStrPart::Expr(inner) = p {
                    contextualize_builder_idents(inner, context, locals, ctx);
                }
            }
        }
        // Leaves, already-qualified paths, and nested scope/context
        // boundaries: nothing to rewrite here.
        _ => {}
    }
}

fn rewrite_expr(
    e: &mut Expr,
    ctx: &RewriteCtx,
    scope: &mut HashSet<String>,
) -> Result<(), ResolveError> {
    match &mut e.kind {
        ExprKind::IntLit(_, _)
        | ExprKind::FloatLit(_, _)
        | ExprKind::BoolLit(_)
        | ExprKind::StrLit(_)
        | ExprKind::CStrLit(_)
        | ExprKind::IncludeBytes { .. }
        | ExprKind::IncludeStr { .. }
        | ExprKind::EnvVar { .. } => {}
        ExprKind::Intrinsic {
            type_args,
            args,
            ret_ty,
            ..
        } => {
            for t in type_args {
                rewrite_type(t, ctx)?;
            }
            if let Some(t) = ret_ty {
                rewrite_type(t, ctx)?;
            }
            for a in args {
                rewrite_expr(a, ctx, scope)?;
            }
        }
        ExprKind::Asm { operands, .. } => {
            for op in operands {
                rewrite_expr(&mut op.value, ctx, scope)?;
            }
        }
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
        // v0.0.22 DSL.2/DSL.3: first apply contextual lookup (DSL.3) —
        // rewrite a bare item name `text` to `ctx::text` when it is a
        // member of the context package and is not shadowed by a local
        // or a same-file top-level item (locals → normal → contextual).
        // Then desugar (DSL.2) to the ordinary `Builder::new`/`add`/
        // `finish` block BEFORE alias rewriting, so both the synthesized
        // protocol paths and the contextual item paths are rewritten
        // like any user-written path. `let` entries get ordinary block
        // scoping.
        ExprKind::BuilderBlock { .. } => {
            if let ExprKind::BuilderBlock { context, body, .. } = &mut e.kind {
                // For a root `@`-block this is its own context path; for a
                // container element the parent's pass already filled it
                // (the inherited context). Block-level locals start from the
                // outer scope; `let` entries extend them in source order.
                let context = context.clone();
                let locals: HashSet<String> = scope.clone();
                contextualize_entries(&mut body.entries, &context, &locals, ctx);
            }
            crate::lower::desugar_builder_block(e);
            // Re-dispatch on the freshly synthesized Block. Container item
            // exprs are still `BuilderBlock` nodes (their context was set
            // by `contextualize_entries`); this recursion reaches each one
            // and runs the same arm — contextual lookup + desugar.
            rewrite_expr(e, ctx, scope)?;
        }
        ExprKind::Block(b) => rewrite_block(b, ctx, scope)?,
        ExprKind::Await(inner) => rewrite_expr(inner, ctx, scope)?,
        ExprKind::Yield(inner) => rewrite_expr(inner, ctx, scope)?,
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            rewrite_expr(cond, ctx, scope)?;
            rewrite_block(then, ctx, scope)?;
            if let Some(eb) = else_branch {
                rewrite_expr(eb, ctx, scope)?;
            }
        }
        ExprKind::Call {
            callee,
            args,
            type_args,
            arg_labels: _,
        } => {
            rewrite_expr(callee, ctx, scope)?;
            for a in args {
                rewrite_expr(a, ctx, scope)?;
            }
            // v0.0.3 Slice 1P.3: turbofish type-args carry their own type
            // references (`foo::[mod::T, other::U](...)`); qualify them
            // the same way as types in declared positions.
            for ta in type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
        }
        ExprKind::FnRef { callee, type_args } => {
            rewrite_expr(callee, ctx, scope)?;
            for ta in type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, ctx, scope)?;
            rewrite_expr(rhs, ctx, scope)?;
        }
        ExprKind::Unary { operand, .. } => rewrite_expr(operand, ctx, scope)?,
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_expr(s, ctx, scope)?;
            }
            if let Some(en) = end {
                rewrite_expr(en, ctx, scope)?;
            }
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
                    // Slice 4B: the type itself must be exported (non-`_`)
                    // to be referenced cross-file at all.
                    ctx.check_pub_item(target_id, &type_name, type_span)?;
                    let resolved_alias = ctx.resolve_alias_target(target_id, &type_name);
                    let (actual_target_id, actual_type_name) = match &resolved_alias {
                        Some(target) => (target.target_id.as_str(), target.name.as_str()),
                        None => (target_id.as_str(), type_name.as_str()),
                    };
                    // If the type is an enum, variants inherit the enum's
                    // visibility (no per-variant marker) — the `check_pub_item`
                    // above covers it. If the type is a struct, the method
                    // also needs to be exported (non-`_`) in its own right.
                    if !ctx.external_is_enum(actual_target_id, actual_type_name) {
                        ctx.check_pub_method(
                            actual_target_id,
                            actual_type_name,
                            &method_or_variant,
                            leaf_span,
                        )?;
                    }
                    let new_type_name = ctx.qualify_external(actual_target_id, actual_type_name);
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
            if let Some(q) = ctx.resolve_item_name(&name.name, name.span)? {
                name.name = q;
            }
            for f in fields {
                rewrite_expr(&mut f.value, ctx, scope)?;
            }
        }
        // v0.0.24 de-Rust: the type-inferred literal has no type name to
        // qualify (the type is resolved from the expected type at sema time),
        // so just recurse into the field values.
        ExprKind::InferredStructLit { fields } => {
            for f in fields {
                rewrite_expr(&mut f.value, ctx, scope)?;
            }
        }
        // Slice 7GEN.5c: rewrite the generic name (cross-file qualification)
        // + recurse into type args + field exprs. The pattern mirrors
        // `StructLit`, but resolver doesn't know about generic instantiation
        // names — those are synthesized by sema and live only post-mono.
        ExprKind::GenericStructLit {
            name,
            type_args,
            fields,
        } => {
            if let Some(q) = ctx.resolve_item_name(&name.name, name.span)? {
                name.name = q;
            }
            for ta in type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
            for f in fields {
                rewrite_expr(&mut f.value, ctx, scope)?;
            }
        }
        ExprKind::Field { receiver, .. } => rewrite_expr(receiver, ctx, scope)?,
        ExprKind::ArrayFill { fill, .. } => rewrite_expr(fill, ctx, scope)?,
        ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
            for el in elements {
                rewrite_expr(el, ctx, scope)?;
            }
        }
        // v0.0.3 1P.1: qualify the enum_name for cross-module generic enum
        // constructors (`mod::Enum[T, E]::Variant(args)`). Pattern mirrors
        // GenericStructLit above. Also rewrite type-args + arg expressions.
        ExprKind::GenericEnumCall {
            enum_name,
            type_args,
            method_type_args,
            args,
            ..
        } => {
            if let Some(q) = ctx.resolve_item_name(&enum_name.name, enum_name.span)? {
                enum_name.name = q;
            }
            for ta in type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
            for ta in method_type_args.iter_mut() {
                rewrite_type(ta, ctx)?;
            }
            for el in args {
                rewrite_expr(el, ctx, scope)?;
            }
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

fn rewrite_pattern(
    p: &mut Pattern,
    ctx: &RewriteCtx,
    scope: &mut HashSet<String>,
) -> Result<(), ResolveError> {
    match &mut p.kind {
        // A literal pattern declares no name. It is desugared away by `lower`,
        // but the resolver walks the AST BEFORE lower runs, so this arm is
        // reachable and must simply do nothing.
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Binding(ident) => {
            scope.insert(ident.name.clone());
        }
        PatternKind::Variant {
            enum_name,
            type_args,
            payload,
            ..
        } => {
            // Three shapes (slice 4-end completes the cross-file case):
            //   `Variant`                  — `enum_name = "EnumName"`        (local)
            //   `Enum::Variant`            — `enum_name = "EnumName"`        (local; payload captured)
            //   `prefix::Enum::Variant`    — `enum_name = "prefix::Enum"`    (cross-file)
            //   `Option[i32]::Variant`     — generic-enum pattern (slice 7GEN.5e); type_args walked below.
            if let Some(q) = ctx.resolve_item_name(&enum_name.name, enum_name.span)? {
                enum_name.name = q;
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
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "isize"
            | "usize"
            | "f32"
            | "f64"
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
        assert_eq!(
            derive_file_id(Path::new("/tmp/proj/src/main.cplus"), &root),
            "src.main"
        );
        assert_eq!(
            derive_file_id(Path::new("/tmp/proj/src/util/strings.cplus"), &root),
            "src.util.strings"
        );
    }

    // A dependency reached through a `vendor/` symlink canonicalizes OUTSIDE the
    // consumer's manifest root. The id must still be package-relative: it lands
    // in every mangled symbol, so an absolute path here means non-reproducible
    // builds, the developer's home directory baked into shipped binaries, and a
    // prebuilt archive that only links on the machine that produced it.
    // A generated header stands in for its source module. If their ids differ,
    // the consumer emits calls to `stdlib.lib.include.text.find` while the
    // archive defines `stdlib.src.text.find`, and the link fails.
    #[test]
    fn a_header_gets_the_same_file_id_as_the_source_it_replaces() {
        let root = PathBuf::from("/proj");
        let from_src = derive_file_id(Path::new("/x/vendor/stdlib/src/text.cplus"), &root);
        let from_hdr =
            derive_file_id(Path::new("/x/vendor/stdlib/lib/include/text.cplus"), &root);
        assert_eq!(from_src, "stdlib.src.text");
        assert_eq!(from_hdr, from_src, "header id must match its source module");
    }

    #[test]
    fn a_nested_header_module_maps_back_correctly() {
        let root = PathBuf::from("/proj");
        assert_eq!(
            derive_file_id(Path::new("/x/vendor/p/lib/include/sub/mod.cplus"), &root),
            derive_file_id(Path::new("/x/vendor/p/src/sub/mod.cplus"), &root),
        );
    }

    // `lib/` alone is the binary-slice directory; only `lib/include/` is headers.
    #[test]
    fn a_binary_slice_path_is_not_mistaken_for_a_header() {
        let root = PathBuf::from("/proj");
        let id = derive_file_id(Path::new("/x/vendor/p/lib/aarch64-apple-darwin/p.a"), &root);
        assert!(!id.contains("src"), "binary slice must not be rewritten: {id}");
    }

    #[test]
    fn dependency_outside_the_root_is_package_relative_not_absolute() {
        let root = PathBuf::from("/Users/someone/Workspace/iris");
        let dep = Path::new("/Users/someone/Workspace/C+/vendor/appkit/src/appkit.cplus");
        assert_eq!(derive_file_id(dep, &root), "appkit.src.appkit");
    }

    #[test]
    fn package_relative_id_is_identical_from_any_checkout_location() {
        // Same package, three different machines / checkout paths, one id.
        let ids: Vec<String> = [
            "/Users/adel/Workspace/C+/vendor/facet/src/runtime.cplus",
            "/home/ci/build/deps/vendor/facet/src/runtime.cplus",
            "/completely/elsewhere/vendor/facet/src/runtime.cplus",
        ]
        .iter()
        .map(|p| derive_file_id(Path::new(p), &PathBuf::from("/some/consumer")))
        .collect();
        assert_eq!(ids, vec!["facet.src.runtime"; 3], "ids diverged: {ids:?}");
    }

    #[test]
    fn nested_module_under_a_dependency_keeps_its_subpath() {
        let root = PathBuf::from("/proj");
        assert_eq!(
            derive_file_id(
                Path::new("/x/vendor/facet_appkit/src/backend/ns/view.cplus"),
                &root
            ),
            "facet_appkit.src.backend.ns.view"
        );
    }

    // No `vendor` ancestor: fall back to the package dir above `src`.
    #[test]
    fn out_of_root_without_vendor_anchors_above_src() {
        let root = PathBuf::from("/proj");
        assert_eq!(
            derive_file_id(Path::new("/elsewhere/mypkg/src/thing.cplus"), &root),
            "mypkg.src.thing"
        );
    }

    // The `+` in the C+ project path used to be the reason the sanitiser exists;
    // with vendor anchoring it should not appear in an id at all.
    #[test]
    fn vendor_anchoring_removes_the_host_path_entirely() {
        let id = derive_file_id(
            Path::new("/Users/adel/Workspace/C+/vendor/stdlib/src/vec.cplus"),
            &PathBuf::from("/Users/adel/Workspace/iris"),
        );
        assert_eq!(id, "stdlib.src.vec");
        assert!(!id.contains("Users"), "host path leaked: {id}");
        assert!(!id.contains('_'), "sanitiser fired unexpectedly: {id}");
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
        let names: Vec<String> = p
            .program
            .items
            .iter()
            .map(|it| match &it.kind {
                ItemKind::Function(f) => f.name.name.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(names.contains(&"main".to_string()));
    }

    #[test]
    fn relative_import_escaping_project_is_rejected_e0914() {
        // Security: a `../..`-chain relative import that resolves outside the
        // package must be rejected, symmetric with the vendor `..` guard.
        let dir = tmpdir();
        // Plant a file OUTSIDE the project tree (as a sibling of `dir`).
        let outside = dir.parent().unwrap().join(format!(
            "cpc-outside-{}",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.cplus"), "fn leaked() -> i32 { return 42; }").unwrap();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"proj\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let main = dir.join("src/main.cplus");
        // Enough `..` to climb out of the project and into the sibling dir.
        let rel = format!(
            "../../{}/secret",
            outside.file_name().unwrap().to_str().unwrap()
        );
        fs::write(
            &main,
            format!("import \"{rel}\" as outside;\nfn main() -> i32 {{ return outside::leaked(); }}"),
        )
        .unwrap();
        // Project mode (`Some(deps)`) — the containment applies there, not in
        // legacy single-file mode.
        let err = load_project_full(&main, &dir, false, Some(&[]), BTreeMap::new()).unwrap_err();
        let diag = err.to_diagnostic();
        assert_eq!(diag.code.0, "E0914", "expected E0914, got {}", diag.code.0);
    }

    #[test]
    fn relative_import_within_project_is_allowed() {
        // A `./` and an in-tree `../` import that stay inside the package must
        // still resolve — the containment check must not over-reject.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"proj\"").unwrap();
        fs::create_dir_all(dir.join("src/util")).unwrap();
        fs::write(
            dir.join("src/main.cplus"),
            "import \"./util/helper\" as helper;\nfn main() -> i32 { return helper::help(); }",
        )
        .unwrap();
        fs::write(
            dir.join("src/util/helper.cplus"),
            "import \"../sibling\" as s;\nfn help() -> i32 { return s::val(); }",
        )
        .unwrap();
        fs::write(dir.join("src/sibling.cplus"), "fn val() -> i32 { return 3; }").unwrap();
        let main = dir.join("src/main.cplus");
        assert!(
            load_project_full(&main, &dir, false, Some(&[]), BTreeMap::new()).is_ok(),
            "in-tree relative imports should resolve"
        );
    }

    #[test]
    fn relative_escape_check_fails_closed_when_boundary_unverifiable() {
        // Security posture: if the package boundary can't be established
        // (import_dir doesn't canonicalize, no Cplus.toml ancestor, no
        // canonicalizable manifest root), the helper must treat the import as
        // an escape — "no boundary" must not mean "no enforcement" in project
        // mode. Force the condition with a non-existent import_dir.
        let missing = PathBuf::from("/no/such/dir/that/exists/anywhere");
        assert!(
            relative_import_escapes_root(&missing, "../x", &missing),
            "unverifiable boundary must fail CLOSED (escape = true)"
        );
    }

    #[test]
    fn overlay_replaces_on_disk_source() {
        // v0.0.14 LSP dirty-buffer overlay: an overlay entry for a file's
        // canonical path supplies its (unsaved) contents instead of disk.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let main = dir.join("src/main.cplus");
        fs::write(&main, "fn main() -> i32 { return 1; }").unwrap();
        let canon = std::fs::canonicalize(&main).unwrap();

        let mut overlays = BTreeMap::new();
        overlays.insert(canon, "fn main() -> i32 { return 2; }".to_string());
        let p = load_project_with_overlays(&main, &dir, overlays).unwrap();
        let src = p
            .files
            .values()
            .find(|(path, _)| path.ends_with("main.cplus"))
            .map(|(_, s)| s.clone())
            .unwrap();
        assert!(src.contains("return 2"), "overlay source not used: {src}");

        // Without an overlay, the on-disk content is used.
        let p2 = load_project(&main, &dir).unwrap();
        let src2 = p2
            .files
            .values()
            .find(|(path, _)| path.ends_with("main.cplus"))
            .map(|(_, s)| s.clone())
            .unwrap();
        assert!(src2.contains("return 1"), "disk source expected: {src2}");
    }

    #[test]
    fn import_and_call_resolves() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/math.cplus"),
            "fn square(n: i32) -> i32 { return n * n; }",
        )
        .unwrap();
        let main_src = r#"
            import "math.cplus" as math;
            fn main() -> i32 { return math::square(7); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let p = load_project(&main, &dir).unwrap();
        // `math::square` should have been rewritten to qualified Ident.
        let main_fn = p
            .program
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "main" => Some(f),
                _ => None,
            })
            .unwrap();
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
            ExprKind::Ident(name) => assert_eq!(name, "x.src.math.square"),
            other => panic!("expected Ident, got {other:?}"),
        }
        // `square` itself should have been qualified.
        let square = p.program.items.iter().find_map(|it| match &it.kind {
            ItemKind::Function(f) if f.name.name == "x.src.math.square" => Some(f),
            _ => None,
        });
        assert!(
            square.is_some(),
            "expected qualified `src.math.square` in merged program"
        );
    }

    #[test]
    fn param_default_expr_is_resolved() {
        // Regression (2026-07-08): rewrite_fn/rewrite_method rewrote param TYPES
        // but not param DEFAULTS, so an imported name used as a default value
        // (`= math::square`) reached sema unresolved and fired E0300/E0303.
        // The default expression must be rewritten like any other expression.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/math.cplus"),
            "fn square(n: i32) -> i32 { return n * n; }",
        )
        .unwrap();
        let main_src = r#"
            import "math.cplus" as math;
            fn apply(f: fn(i32) -> i32 = math::square) -> i32 { return f(3); }
            fn main() -> i32 { return apply(); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let p = load_project(&main, &dir).unwrap();
        // Non-`main` entry-file fns are qualified (`src.main.apply`).
        let apply_fn = p
            .program
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "x.src.main.apply" => Some(f),
                _ => None,
            })
            .unwrap();
        let default = apply_fn.params[0]
            .default
            .as_ref()
            .expect("default present");
        // Unresolved it would still read `math::square`; resolved it is the
        // qualified free-fn Ident, exactly like a call callee.
        match &default.kind {
            ExprKind::Ident(name) => assert_eq!(name, "x.src.math.square"),
            other => panic!("expected qualified Ident default, got {other:?}"),
        }
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
        fs::write(
            dir.join("src/a.cplus"),
            r#"
            import "b.cplus" as b;
            fn from_a() -> i32 { return 1; }
        "#,
        )
        .unwrap();
        fs::write(
            dir.join("src/b.cplus"),
            r#"
            import "a.cplus" as a;
            fn from_b() -> i32 { return 2; }
        "#,
        )
        .unwrap();
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
        // Leading `_` — private to math.cplus (v0.0.24 #10 name-based privacy).
        fs::write(
            dir.join("src/math.cplus"),
            "fn _square(n: i32) -> i32 { return n * n; }",
        )
        .unwrap();
        let main_src = r#"
            import "math.cplus" as math;
            fn main() -> i32 { return math::_square(7); }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(
            matches!(
                err.error,
                ResolveError::PrivateAccess {
                    kind: PrivateKind::Function,
                    ..
                }
            ),
            "expected PrivateAccess Function, got {err:?}"
        );
    }

    #[test]
    fn cross_file_private_struct_rejected_with_e0403() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/geom.cplus"),
            "struct _Point { x: i32, y: i32 }\n",
        )
        .unwrap();
        // Slice 4C: cross-file struct literal `g::_Point { ... }` now
        // parses, so the E0403 check fires on the construction site.
        let main_src = r#"
            import "geom.cplus" as g;
            fn main() -> i32 { let p = g::_Point { x: 1, y: 2 }; return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(
            matches!(
                err.error,
                ResolveError::PrivateAccess {
                    kind: PrivateKind::Struct,
                    ..
                }
            ),
            "expected PrivateAccess Struct, got {err:?}"
        );
    }

    #[test]
    fn cross_file_public_struct_private_method_rejected() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        // Struct is public but the `_new` method is module-private.
        fs::write(
            dir.join("src/geom.cplus"),
            r#"
            struct Point { x: i32, y: i32 }
            impl Point {
                fn _new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }
            }
        "#,
        )
        .unwrap();
        let main_src = r#"
            import "geom.cplus" as g;
            fn main() -> i32 { let p: g::Point = g::Point::_new(1, 2); return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(
            matches!(
                err.error,
                ResolveError::PrivateAccess {
                    kind: PrivateKind::Method,
                    ..
                }
            ),
            "expected PrivateAccess Method, got {err:?}"
        );
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
        fs::write(
            dir.join("src/colors.cplus"),
            "enum Color { Red, Green(i32), Blue }\n",
        )
        .unwrap();
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
        let name_fn = project
            .program
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "x.src.main.name" => Some(f),
                _ => None,
            })
            .expect("found name fn");
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
                assert_eq!(
                    enum_name.name, "x.src.colors.Color",
                    "expected qualified enum name; got `{}`",
                    enum_name.name
                );
            }
        }
    }

    #[test]
    fn cross_file_pub_enum_variants_are_accessible() {
        // An exported enum (non-`_` name) exports its variants automatically
        // (no per-variant marker).
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/colors.cplus"),
            "enum Color { Red, Green, Blue }\n",
        )
        .unwrap();
        let main_src = r#"
            import "colors.cplus" as c;
            fn main() -> i32 {
                let r: c::Color = c::Color::Red;
                return 0;
            }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        load_project(&main, &dir).expect("enum variants should be reachable");
    }

    #[test]
    fn cross_file_private_enum_rejected() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/colors.cplus"),
            "enum _Color { Red, Green, Blue }\n",
        )
        .unwrap();
        let main_src = r#"
            import "colors.cplus" as c;
            fn main() -> i32 { let r: c::_Color = c::_Color::Red; return 0; }
        "#;
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let err = load_project(&main, &dir).unwrap_err();
        assert!(
            matches!(
                err.error,
                ResolveError::PrivateAccess {
                    kind: PrivateKind::Enum,
                    ..
                }
            ),
            "expected PrivateAccess Enum, got {err:?}"
        );
    }

    #[test]
    fn cross_file_struct_and_method_resolve() {
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/geom.cplus"),
            r#"
            struct Point { x: i32, y: i32 }
            impl Point {
                fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }
            }
        "#,
        )
        .unwrap();
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
            ItemKind::Struct(s) => s.name.name == "x.src.geom.Point",
            _ => false,
        });
        assert!(has_struct);
        // The impl block target should also be `src.geom.Point`.
        let has_impl = p.program.items.iter().any(|it| match &it.kind {
            ItemKind::Impl(b) => b.target.name == "x.src.geom.Point",
            _ => false,
        });
        assert!(has_impl);
    }

    #[test]
    fn impl_target_alias_path_rewrites_to_qualified_name() {
        // EXT.1: `impl g::Point` in a sibling file resolves through the same
        // machinery as a type reference — the merged AST carries the
        // qualified target, so sema sees one name for the type and both of
        // its impl blocks.
        let dir = tmpdir();
        fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/geom.cplus"),
            "struct Point { x: i32, y: i32 }\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/ext.cplus"),
            "import \"geom.cplus\" as g;\nimpl g::Point { fn sum(this) -> i32 { return this.x + this.y; } }\n",
        )
        .unwrap();
        let main_src = "import \"geom.cplus\" as g;\nimport \"ext.cplus\" as e;\nfn main() -> i32 { let p: g::Point = g::Point { x: 3, y: 4 }; return p.sum(); }\n";
        let main = dir.join("src/main.cplus");
        fs::write(&main, main_src).unwrap();
        let p = load_project(&main, &dir).unwrap();
        let impl_targets: Vec<String> = p
            .program
            .items
            .iter()
            .filter_map(|it| match &it.kind {
                ItemKind::Impl(b) => Some(b.target.name.clone()),
                _ => None,
            })
            .collect();
        assert!(
            impl_targets.iter().any(|t| t == "x.src.geom.Point"),
            "alias-path impl target must rewrite to the qualified type name; got {impl_targets:?}"
        );
    }
}
