//! `Cplus.toml` manifest loader.
//!
//! Schema:
//!
//! ```toml
//! [package]
//! name    = "myapp"
//! version = "0.1.0"
//! edition = "2026"
//! entry   = "src/main.cplus"     # optional — this default applies when the file exists
//!
//! [dependencies]                 # the portable tier
//! stdlib = "*"
//!
//! [ios]                          # per-platform: entry override + scoped deps
//! entry = "src/main_ios.cplus"
//! [ios.dependencies]
//! facet_uikit = "*"
//!
//! [library]                      # a C-ABI product library (rare)
//! kind  = "staticlib"            # or "cdylib" / "both"; default "staticlib"
//! entry = "src/lib.cplus"        # omitted → the whole of src/ is archived
//! ```
//!
//! Only `[package]` is required. What a build produces is decided by the
//! TARGET, not the manifest: an entry on a self-linked platform (macos,
//! linux, windows) becomes an executable; on an external-builder platform
//! (ios, android, esp32) it becomes `lib<name>.a` + a C header, and Xcode /
//! Gradle / ESP-IDF owns the final link. A package with no entry at all is a
//! library: `cpc build` archives its whole `src/` tree.
//!
//! The Cargo-shaped `[[bin]]` / `[lib]` sections were REMOVED in v0.0.28;
//! they parse into a targeted E0408 with the migration in the message.

use crate::diagnostics::{
    Applicability, DiagCode, Diagnostic, Position, Severity, SourceSpan, Suggestion,
};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub package: Package,
    /// The package-level app entry: `[package] entry = "..."`, or the
    /// conventional default `src/main.cplus` when that file exists, no
    /// `[library]` is declared, AND no platform section names an entry (a
    /// declared platform entry scopes the app deliberately — the default
    /// must not leak into platforms the author left out). `None` = no
    /// package-level entry (the package may still be an app through
    /// `platform_entries`).
    pub entry: Option<PathBuf>,
    /// True when an `entry` key was actually WRITTEN somewhere (package
    /// level or a platform section) — as opposed to the src/main.cplus
    /// default being picked up. Drives the "no entry for platform X" error:
    /// a package that declared entries is an app everywhere, and building it
    /// for a platform it names no entry for is a hard error, never a silent
    /// fall-through to library mode.
    pub entry_declared: bool,
    /// Per-platform entry overrides: `[<platform>] entry = "..."`. Keys are
    /// `crate::target::PLATFORMS` names.
    pub platform_entries: std::collections::BTreeMap<String, PathBuf>,
    /// A C-ABI product library: the `[library]` section, or the target
    /// synthesized by `[build] prebuild = true`. When present, `cpc build`
    /// produces `.a` and/or `.dylib`/`.so` instead of an executable, and the
    /// codegen path skips the test-driver `@main` injection.
    pub lib: Option<LibTarget>,
    /// Phase 2 (v0.0.2) — package system MVP. Vendor packages declare
    /// their linker requirements in a top-level `[link]` table; the
    /// consumer's build driver walks the dep graph and forwards each
    /// dep's `[link]` to its own clang invocation. Consumers typically
    /// don't populate this directly — they use `[[bin]] frameworks`/
    /// `libs` for their own binary's link surface. Both sources of
    /// link args are merged at build time.
    pub link: Option<LinkSpec>,
    /// Phase 2 (v0.0.2) — consumer's declared dependencies. Each entry
    /// names a directory expected to exist at `vendor/<name>/` with a
    /// matching `Cplus.toml`. Version strings parse but are unused at
    /// resolution time (MVP). Empty for vendor packages and standalone
    /// programs.
    pub dependencies: Vec<Dependency>,
    /// Directory containing the manifest file. All bin `path` entries are
    /// resolved relative to this directory.
    pub root: PathBuf,
    /// Optional `[build]` table — how this package is consumed. See `BuildSpec`.
    pub build: BuildSpec,
    /// v0.0.12 realtime Phase 8: optional `[profile.realtime]` table. When
    /// present, the build/check driver synthesizes the corresponding contract
    /// attributes (`#[no_alloc]`, `#[no_block]`, `#[max_stack(N)]`) onto every
    /// function defined in *this* package (dependencies are exempt), turning
    /// the per-function opt-in into a project-wide CI gate.
    pub realtime_profile: Option<RealtimeProfile>,
}

/// v0.0.12 realtime Phase 8 — parsed `[profile.realtime]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeProfile {
    /// Deny heap allocation (synthesizes `#[no_alloc]`).
    pub deny_alloc: bool,
    /// Deny blocking primitives (synthesizes `#[no_block]`).
    pub deny_block: bool,
    /// Reject calls to externs not known/marked non-allocating/non-blocking.
    /// Subsumed by `deny_alloc`/`deny_block` (both already reject unknown
    /// externs); kept as an explicit knob for clarity and forward use.
    pub deny_unknown_extern: bool,
    /// Per-function stack budget in bytes (synthesizes `#[max_stack(N)]`).
    pub stack_limit: Option<u64>,
}

/// Phase 2 (v0.0.2) — top-level `[link]` table on a vendor package's
/// `Cplus.toml`. Declares the linker requirements the package wants its
/// consumer to honor when building anything that depends on it.
///
/// The manifest is the single source of truth: the build driver
/// verifies the filesystem matches what's declared, and refuses to
/// link anything else. See plan.md §"Phase 2 — Manifest = single
/// source of truth" for the E0860-E0863 error codes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkSpec {
    /// macOS / iOS frameworks. Each entry becomes `-framework <name>`
    /// on the link line. Platform-gated by clang.
    pub frameworks: Vec<String>,
    /// System libraries — expected on the consumer's machine. Each
    /// entry becomes `-l<name>` on the link line.
    pub libs: Vec<String>,
    /// Binaries shipped by THIS package, located at `lib/<triple>/<basename>`.
    /// Each entry is a basename, no path component.
    ///
    /// The triple is never declared — the build derives it from the target it
    /// is building for, and the directory's existence is the whole signal: no
    /// `lib/<triple>/` means this package ships nothing for you, so it is
    /// compiled from `src/` like any source package. When the directory IS
    /// there, the manifest is truth inside it: a declared file that is missing
    /// is E0860, an undeclared binary sitting in it is E0861.
    pub bundled: Vec<String>,
    /// v0.0.9 Phase 8 (cpc-gaps G-001): prebuilt `.o` files to append
    /// to the link line for any target produced from this manifest.
    /// Paths are resolved relative to the manifest directory. Useful
    /// for embedding hand-written C, assembly-generated `incbin`
    /// blobs (Metal shader libraries, etc.), or any other prebuilt
    /// object the C+ binary needs to link against.
    ///
    /// cpc doesn't run a build script — the user is responsible for
    /// producing each `.o` out-of-band (typical pattern: a Makefile
    /// invokes `clang -c foo.s -o foo.o` before `cpc build`). cpc
    /// validates each entry exists at link time and fails with
    /// E0864 if any is missing.
    pub extra_objects: Vec<PathBuf>,
    /// Library search directories. Each becomes both `-L<dir>` (so the
    /// linker resolves `libs` that live outside the default search path,
    /// e.g. `/usr/local/cuda/lib64`) and `-Wl,-rpath,<dir>` (so the
    /// dynamic loader finds the same `.so` at runtime without the user
    /// setting `LD_LIBRARY_PATH`). Relative entries resolve against the
    /// manifest directory; absolute system paths pass through unchanged.
    pub search_paths: Vec<String>,
}

/// Phase 2 (v0.0.2) — one entry in `[dependencies]` or a
/// `[<platform>.dependencies]` section. Carries (name, version-string,
/// platforms). Resolution is presence-check only: `cpc build` verifies
/// `vendor/<name>/Cplus.toml` exists and is valid. SemVer resolution is
/// forward-compat work for `cpc fetch`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    /// Platforms this dep applies to (`crate::target::PLATFORMS` names,
    /// from the `[<platform>.dependencies]` section(s) that declared it).
    /// Empty = all platforms (declared in plain `[dependencies]`).
    pub platforms: Vec<String>,
}

impl Dependency {
    /// Whether this dep participates in a build for `platform`
    /// (a `crate::target::active_platform()` name).
    pub fn active_on(&self, platform: &str) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|p| p == platform)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibTarget {
    pub name: String,
    pub path: PathBuf,
    pub crate_type: CrateType,
    /// True when this target was synthesized — by `[build] prebuild = true`,
    /// or by a `[library]` section with no `entry` key — rather than naming
    /// an entry file on purpose.
    ///
    /// The two mean different things by "the entry". An explicit entry names
    /// one file on purpose — that import tree IS the library, and its
    /// top-level names are the public C ABI (spelled bare). A synthesized
    /// target means all of `src/`: `cpc headers` generates a declaration file
    /// per module there, so any module the entry doesn't happen to import
    /// would be declared to consumers with nothing defining it. The build
    /// driver compiles a synthesized entry importing every module to keep the
    /// archive and the headers describing the same package.
    pub synthesized: bool,
    /// Same shape as `BinTarget.frameworks` / `.libs`: linker flags
    /// forwarded as `-framework <name>` / `-l<name>`. Today these flags
    /// are baked into the produced `.dylib` (the C consumer doesn't have
    /// to re-state them) or recorded in the `.a` archive's metadata for
    /// the consumer's link line (a future polish).
    pub frameworks: Vec<String>,
    pub libs: Vec<String>,
}

/// The `[build]` table — how a package is consumed by anything that depends on
/// it. Both keys are booleans and both default to `false`, so a package that
/// says nothing keeps today's behaviour: consumers compile it from `src/` on
/// every build.
///
/// ```toml
/// [build]
/// prebuild = true    # compile me once, reuse the archive
/// dev      = true    # I'm being worked on — compile me from src/, ignore binaries
/// ```
///
/// `prebuild` is the cache: the first consumer build compiles the package into
/// `lib/<triple>/<name>.a`, generates `lib/include/`, and records a fingerprint
/// next to the archive. Later builds link the archive instead of recompiling.
/// A package that declares it needs no `[lib]` table — one is synthesized, so
/// `prebuild = true` is the whole opt-in.
///
/// `dev` is the escape hatch, and it wins over everything: no headers, no
/// archive, no fingerprint, `src/` straight through, whether the binaries come
/// from `prebuild` or from an author-shipped `[link].bundled`. It is declared
/// in the manifest of the package being worked on, not in each consumer, so
/// flipping it applies to every app that depends on it. Every build restates
/// it on stderr — a manifest knob that changes what gets compiled must not be
/// able to rot silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildSpec {
    pub prebuild: bool,
    pub dev: bool,
}

/// `prebuild` defaults to TRUE (decision 2026-08-16): a package is a built
/// artifact unless it says otherwise. `prebuild = false` opts a package out
/// (e.g. one that is generic-only and gains nothing from an archive);
/// `dev = true` overrides everything while the package is being worked on.
/// The content fingerprint is what makes the default safe — an edited
/// source can never be shadowed by a stale archive.
impl Default for BuildSpec {
    fn default() -> Self {
        Self {
            prebuild: true,
            dev: false,
        }
    }
}

impl BuildSpec {
    /// Does this package's public surface resolve through `lib/include/`?
    ///
    /// `bundled` is passed in because author-shipped binaries live in `[link]`,
    /// not `[build]`: either source of binaries puts consumers on headers, and
    /// `dev` overrides both.
    pub fn resolves_through_headers(&self, ships_bundled: bool) -> bool {
        !self.dev && (self.prebuild || ships_bundled)
    }
}

/// `[library] kind` — what artifact(s) a C-ABI product library produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrateType {
    /// `libNAME.a` archive. Linked statically by the consumer. The default.
    Staticlib,
    /// `libNAME.dylib` (macOS) / `libNAME.so` (Linux). Linked dynamically.
    Cdylib,
    /// Produce both `.a` and `.dylib`/`.so`.
    Both,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
}

impl Manifest {
    /// The app entry a build for `platform` uses: the platform section's
    /// `entry` when one is declared, else the package-level entry. `None` on
    /// a platform the manifest names no entry for.
    pub fn entry_for(&self, platform: &str) -> Option<PathBuf> {
        if let Some(p) = self.platform_entries.get(platform) {
            return Some(p.clone());
        }
        self.entry.clone()
    }

    /// Whether this package is an application — any entry, declared or the
    /// src/main.cplus default. An app with no entry for the ACTIVE platform
    /// is a build error, never a silent library build.
    pub fn is_app(&self) -> bool {
        self.entry.is_some() || !self.platform_entries.is_empty()
    }

    /// The platforms an entry is declared or available for, for the
    /// "no entry for platform X" diagnostic.
    pub fn entry_platforms(&self) -> Vec<String> {
        let mut out: Vec<String> = self.platform_entries.keys().cloned().collect();
        if self.entry.is_some() {
            out.insert(0, "every platform (package-level entry)".to_string());
        }
        out
    }

    /// The source-entry ladder shared by `cpc check`, `cpc graph`/`query`/
    /// `mcp` and the LSP — the file whose import tree best describes the
    /// package. Returns `(path, is_lib_like)`, where `is_lib_like` selects
    /// the bare (C-ABI) spelling of the entry's top-level names.
    ///
    ///   1. the app entry for `platform`,
    ///   2. the `[library]` target,
    ///   3. the `src/test_main.cplus` convention (a library package's test
    ///      root imports the whole surface, so it is the widest tree),
    ///   4. the `src/<package>.cplus` root-module fallback.
    pub fn resolve_source_entry(&self, platform: &str) -> Option<(PathBuf, bool)> {
        if let Some(e) = self.entry_for(platform) {
            if e.is_file() {
                return Some((e, false));
            }
        }
        if let Some(lt) = &self.lib {
            if lt.path.is_file() {
                return Some((lt.path.clone(), !lt.synthesized));
            }
        }
        let test_main = self.root.join("src").join("test_main.cplus");
        if test_main.is_file() {
            return Some((test_main, false));
        }
        let root_module = self
            .root
            .join("src")
            .join(format!("{}.cplus", self.package.name));
        if root_module.is_file() {
            return Some((root_module, true));
        }
        None
    }

    /// The `cpc test` entry ladder. Same rungs as `resolve_source_entry`,
    /// but `src/test_main.cplus` comes FIRST: a package with a dedicated
    /// test root means it, even when the package is also an app.
    pub fn test_entry(&self, platform: &str) -> Option<(PathBuf, bool)> {
        let test_main = self.root.join("src").join("test_main.cplus");
        if test_main.is_file() {
            return Some((test_main, false));
        }
        self.resolve_source_entry(platform)
    }
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    MissingField {
        path: PathBuf,
        field: &'static str,
    },
    UnsupportedEdition {
        path: PathBuf,
        found: String,
    },
    /// (E0408): a removed Cargo-shaped target section (`[[bin]]` / `[lib]`)
    /// appears in the manifest. The error carries the migration.
    LegacyTargetSection {
        path: PathBuf,
        section: &'static str,
        hint: &'static str,
    },
    /// (E0408): an app entry and a `[library]` section in one manifest. A
    /// package is an application or a C-ABI product library, not both.
    EntryAndLibraryConflict {
        path: PathBuf,
    },
    /// (E0412): `[library] kind` value not in `{staticlib, cdylib, both}`.
    UnsupportedCrateType {
        path: PathBuf,
        found: String,
    },
    /// Phase 2 (E0857): `[dependencies]` key fails the lowercase-ident
    /// rule. Dep names must match `[a-z][a-z0-9_]*` so the import path's
    /// first segment is unambiguous.
    InvalidDependencyName {
        path: PathBuf,
        found: String,
    },
    /// v0.0.20 (E0865): a `${VAR}` reference in `[link].search-paths` or
    /// `[link].extra-objects` could not be expanded — the variable is unset
    /// and no `:-default` fallback was given (or the `${...}` is malformed).
    /// Lets vendor manifests point at an external SDK via the environment
    /// instead of a hardcoded absolute path.
    EnvExpansion {
        path: PathBuf,
        entry: String,
        message: String,
    },
    /// (E0869): one dependency name declared in more than one place with
    /// incompatible meaning — in `[dependencies]` (all platforms) AND a
    /// `[<platform>.dependencies]` section, or in two platform sections
    /// with different version specs. The package manager has no conflict
    /// resolver by design, so one package must resolve to one spec.
    /// (The one legal duplicate: the same spec in several platform
    /// sections, which merges into one dep active on all of them.)
    ConflictingDependency {
        path: PathBuf,
        name: String,
        message: String,
    },
    /// (E0868): an `entry` / `[library] entry` path resolves outside the
    /// package directory (absolute, or a `..` chain). A package's source
    /// targets must live inside its own tree — the same containment the
    /// import paths enforce (E0859/E0914); a hostile vendored manifest must
    /// not be able to point compilation at arbitrary host files. `[link]`
    /// paths are exempt: search paths and `${VAR}`-expanded extra objects
    /// legitimately name external SDK locations.
    TargetPathEscapes {
        path: PathBuf,
        target: String,
        requested: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io { path, source } => {
                write!(f, "reading manifest {}: {source}", path.display())
            }
            ManifestError::Parse { path, message } => {
                write!(f, "parsing manifest {}: {message}", path.display())
            }
            ManifestError::MissingField { path, field } => {
                write!(
                    f,
                    "manifest {} is missing required field `{field}`",
                    path.display()
                )
            }
            ManifestError::UnsupportedEdition { path, found } => {
                write!(
                    f,
                    "manifest {}: unsupported edition `{found}` (only `2026` is currently valid)",
                    path.display()
                )
            }
            ManifestError::LegacyTargetSection { path, section, hint } => {
                write!(f, "manifest {}: the `{section}` section was removed — {hint}", path.display())
            }
            ManifestError::EntryAndLibraryConflict { path } => {
                write!(f, "manifest {}: cannot declare both an app `entry` and `[library]` (a package is an application or a C-ABI library, not both)", path.display())
            }
            ManifestError::UnsupportedCrateType { path, found } => {
                write!(f, "manifest {}: unsupported `[library] kind` value `{found}` (must be one of `staticlib`, `cdylib`, `both`)", path.display())
            }
            ManifestError::InvalidDependencyName { path, found } => {
                write!(f, "manifest {}: dependency name `{found}` must be a lowercase identifier (`[a-z][a-z0-9_]*`)", path.display())
            }
            ManifestError::EnvExpansion {
                path,
                entry,
                message,
            } => {
                write!(
                    f,
                    "manifest {}: cannot expand `{entry}`: {message}",
                    path.display()
                )
            }
            ManifestError::ConflictingDependency { path, name, message } => {
                write!(
                    f,
                    "manifest {}: dependency `{name}` {message}",
                    path.display()
                )
            }
            ManifestError::TargetPathEscapes {
                path,
                target,
                requested,
            } => {
                write!(
                    f,
                    "manifest {}: `{target}` path `{requested}` resolves outside the package directory",
                    path.display()
                )
            }
        }
    }
}

impl ManifestError {
    /// Render this error as a structured `Diagnostic`. Manifest issues
    /// don't have meaningful byte spans (the TOML parser would but we
    /// don't thread its spans through yet); the primary location is a
    /// position-zero anchor at the manifest file path. E0406 covers
    /// parse / missing-field / bad-edition; I/O issues use E0407 to
    /// stay consistent with the slice-4A allocation.
    pub fn to_diagnostic(&self) -> Diagnostic {
        let path = match self {
            ManifestError::Io { path, .. }
            | ManifestError::Parse { path, .. }
            | ManifestError::MissingField { path, .. }
            | ManifestError::UnsupportedEdition { path, .. }
            | ManifestError::LegacyTargetSection { path, .. }
            | ManifestError::EntryAndLibraryConflict { path }
            | ManifestError::UnsupportedCrateType { path, .. }
            | ManifestError::InvalidDependencyName { path, .. }
            | ManifestError::EnvExpansion { path, .. }
            | ManifestError::ConflictingDependency { path, .. }
            | ManifestError::TargetPathEscapes { path, .. } => path.clone(),
        };
        let primary = SourceSpan {
            file: path.clone(),
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
        };
        let mut suggestions: Vec<Suggestion> = Vec::new();
        let (code, message) = match self {
            ManifestError::Io { path, source } => (
                "E0407",
                format!("could not read manifest `{}`: {source}", path.display()),
            ),
            ManifestError::Parse { message, .. } => (
                "E0406",
                format!("malformed `Cplus.toml`: {message}"),
            ),
            ManifestError::MissingField { field, .. } => (
                "E0406",
                format!("manifest is missing required field `{field}`"),
            ),
            ManifestError::UnsupportedEdition { found, .. } => {
                // Machine-applicable: bump it to "2026".
                suggestions.push(Suggestion {
                    description: "use the current edition".to_string(),
                    span: primary.clone(),
                    replacement: "edition = \"2026\"".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                });
                (
                    "E0406",
                    format!("unsupported edition `{found}` (only `2026` is currently valid)"),
                )
            }
            ManifestError::LegacyTargetSection { section, hint, .. } => (
                "E0408",
                format!("the `{section}` section was removed — {hint}"),
            ),
            ManifestError::EntryAndLibraryConflict { .. } => (
                "E0408",
                "cannot declare both an app `entry` and `[library]` in one manifest \
                 (a package is an application or a C-ABI library — split into two packages if you need both)".to_string(),
            ),
            ManifestError::UnsupportedCrateType { found, .. } => (
                "E0412",
                format!("unsupported `[library] kind` value `{found}` (expected one of `staticlib`, `cdylib`, `both`)"),
            ),
            ManifestError::InvalidDependencyName { found, .. } => (
                "E0857",
                format!("dependency name `{found}` must match `[a-z][a-z0-9_]*` (no dots, slashes, or uppercase — the first segment of an import path must be unambiguous)"),
            ),
            ManifestError::EnvExpansion { entry, message, .. } => (
                "E0865",
                format!("cannot expand `{entry}` in `[link]`: {message}"),
            ),
            ManifestError::ConflictingDependency { name, message, .. } => (
                "E0869",
                format!("dependency `{name}` {message}"),
            ),
            ManifestError::TargetPathEscapes {
                target, requested, ..
            } => (
                "E0868",
                format!(
                    "`{target}` path `{requested}` resolves outside the package directory — \
                     source targets must live inside the package tree"
                ),
            ),
        };
        Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message,
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions,
        }
    }
}

/// Lexically resolve `.` / `..` in `joined` (no filesystem access — the
/// target file may not exist yet at parse time) and report whether the
/// result leaves `root`. `root` is already canonicalized by the caller, so
/// a prefix compare is meaningful. An absolute `path` key survives
/// `root.join` unchanged and fails the prefix check unless it happens to
/// point back inside the package.
fn target_path_escapes(root: &Path, joined: &Path) -> bool {
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    !out.starts_with(root)
}

/// On-disk schema. Kept distinct from the public `Manifest` so we can apply
/// defaults and validation before exposing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    package: RawPackage,
    /// REMOVED sections, kept as opaque values so any legacy shape parses
    /// far enough to get the targeted E0408 migration error instead of a
    /// generic serde "unknown field".
    #[serde(default, rename = "bin")]
    bin: Option<toml::Value>,
    #[serde(default)]
    lib: Option<toml::Value>,
    /// `[library]` — a C-ABI product library (staticlib / cdylib / both).
    #[serde(default)]
    library: Option<RawLibrary>,
    /// Phase 2: top-level `[link]` table on a vendor package's manifest.
    #[serde(default)]
    link: Option<RawLinkSpec>,
    /// `[build]` table — consumption policy. See `BuildSpec`.
    #[serde(default)]
    build: Option<RawBuildSpec>,
    /// Phase 2: `[dependencies]` table — `name = "version-string"` pairs.
    /// Toml's `serde` integration deserializes this as a string-keyed map.
    /// Iteration order matches insertion order via `BTreeMap` (lexicographic
    /// — fine for MVP; consumers shouldn't depend on dep ordering).
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, String>,
    /// v0.0.12 realtime Phase 8: `[profile.<name>]` tables. Only `realtime`
    /// is recognized today; unknown profile names are ignored.
    #[serde(default)]
    profile: Option<RawProfiles>,
    /// Platform-scoped sections: `[<platform>.dependencies]` declares deps
    /// that exist only on that platform (a facet backend, an OS binding).
    /// One field per `crate::target::PLATFORMS` name; `deny_unknown_fields`
    /// makes a misspelled platform a hard parse error. Kept in lockstep
    /// with `cplus-pm`'s manifest reader.
    #[serde(default)]
    macos: Option<RawPlatformSection>,
    #[serde(default)]
    linux: Option<RawPlatformSection>,
    #[serde(default)]
    windows: Option<RawPlatformSection>,
    #[serde(default)]
    ios: Option<RawPlatformSection>,
    #[serde(default)]
    android: Option<RawPlatformSection>,
    #[serde(default)]
    esp32: Option<RawPlatformSection>,
    #[serde(default)]
    wasm: Option<RawPlatformSection>,
}

/// One `[<platform>.*]` table: an optional `entry` override plus scoped
/// `dependencies`. `deny_unknown_fields` reserves the rest of the namespace
/// (a future `[<platform>.link]` arrives as a feature, not a
/// silently-ignored key).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlatformSection {
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfiles {
    #[serde(default)]
    realtime: Option<RawRealtimeProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRealtimeProfile {
    // Kebab-case keys to match the rest of the manifest (`extra-objects`,
    // `search-paths`, `crate-type`). These drive the project-wide
    // `#[no_alloc]`/`#[no_block]` safety gate — a silently-dropped mis-cased
    // key (the snake_case spelling that used to be required) meant a CI gate
    // the author believed was on was actually off. With `deny_unknown_fields`
    // the old snake spelling is now a hard parse error, not a silent drop.
    #[serde(default, rename = "deny-alloc")]
    deny_alloc: bool,
    #[serde(default, rename = "deny-block")]
    deny_block: bool,
    #[serde(default, rename = "deny-unknown-extern")]
    deny_unknown_extern: bool,
    #[serde(default, rename = "stack-limit")]
    stack_limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLinkSpec {
    #[serde(default)]
    frameworks: Vec<String>,
    #[serde(default)]
    libs: Vec<String>,
    #[serde(default)]
    bundled: Vec<String>,
    /// v0.0.9 Phase 8 (cpc-gaps G-001): kebab-case key `extra-objects`
    /// matching the rest of the manifest's multi-word field naming.
    #[serde(default, rename = "extra-objects")]
    extra_objects: Vec<String>,
    #[serde(default, rename = "search-paths")]
    search_paths: Vec<String>,
}

/// On-disk `[build]`. `deny_unknown_fields` makes a misspelled key a hard
/// parse error rather than a silently-ignored line — a build policy that
/// quietly does nothing is worse than one that refuses to load.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuildSpec {
    /// Defaults to true (2026-08-16): prebuild is the norm, `prebuild =
    /// false` is the opt-out, `dev = true` the development override.
    #[serde(default = "default_prebuild")]
    prebuild: bool,
    #[serde(default)]
    dev: bool,
}

fn default_prebuild() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLibrary {
    name: Option<String>,
    /// Explicit entry: this file's import tree IS the library and its
    /// top-level names are the public C ABI. Omitted: the whole of `src/`
    /// is archived (synthesized entry, qualified names).
    entry: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    frameworks: Vec<String>,
    #[serde(default)]
    libs: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
    name: Option<String>,
    version: Option<String>,
    edition: Option<String>,
    /// The app entry. Defaults to `src/main.cplus` when that file exists.
    #[serde(default)]
    entry: Option<String>,
}

/// Load and validate a `Cplus.toml` file. The returned `Manifest`'s
/// `root` field holds the manifest directory; `bins[].path` entries are
/// absolute paths derived from the manifest's location.
pub fn load(manifest_path: &Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| ManifestError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })?;
    parse(&text, manifest_path)
}

pub fn parse(text: &str, manifest_path: &Path) -> Result<Manifest, ManifestError> {
    let raw: RawManifest = toml::from_str(text).map_err(|e| ManifestError::Parse {
        path: manifest_path.to_path_buf(),
        message: e.to_string(),
    })?;

    let name = raw.package.name.ok_or(ManifestError::MissingField {
        path: manifest_path.to_path_buf(),
        field: "package.name",
    })?;
    let version = raw.package.version.unwrap_or_else(|| "0.0.0".to_string());
    let edition = raw.package.edition.unwrap_or_else(|| "2026".to_string());
    if edition != "2026" {
        return Err(ManifestError::UnsupportedEdition {
            path: manifest_path.to_path_buf(),
            found: edition,
        });
    }

    // Resolve `root` to an absolute path so downstream consumers (file-id
    // derivation, target-dir creation) don't have to second-guess CWD.
    // `manifest_path.parent()` on a bare `Cplus.toml` is `Some("")`, which
    // canonicalize() rejects — handle that explicitly.
    let parent = manifest_path.parent().filter(|p| !p.as_os_str().is_empty());
    let root = match parent {
        Some(p) => std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    // The removed Cargo-shaped sections get a targeted migration error, not
    // a serde "unknown field". Checked before anything else so a legacy
    // manifest fails on the section, never on some downstream consequence.
    if raw.bin.is_some() {
        return Err(ManifestError::LegacyTargetSection {
            path: manifest_path.to_path_buf(),
            section: "[[bin]]",
            hint: "delete it (`src/main.cplus` is the default entry), or set \
                   `entry = \"...\"` under `[package]` / a `[<platform>]` section; \
                   `frameworks`/`libs` move to the `[link]` table",
        });
    }
    if raw.lib.is_some() {
        return Err(ManifestError::LegacyTargetSection {
            path: manifest_path.to_path_buf(),
            section: "[lib]",
            hint: "for an external-builder app (iOS/Android), name the entry in that \
                   platform's section (`[ios] entry = \"...\"`) — the target decides the \
                   artifact; for a C-ABI product library use `[library]` with \
                   `kind = \"staticlib\"|\"cdylib\"|\"both\"` and an optional `entry`",
        });
    }

    let build = raw
        .build
        .map(|b| BuildSpec {
            prebuild: b.prebuild,
            dev: b.dev,
        })
        .unwrap_or_default();

    let lib = match &raw.library {
        None => None,
        Some(rl) => {
            let lib_name = rl.name.clone().unwrap_or_else(|| name.clone());
            let crate_type = match rl.kind.as_deref() {
                None | Some("staticlib") => CrateType::Staticlib,
                Some("cdylib") => CrateType::Cdylib,
                Some("both") => CrateType::Both,
                Some(other) => {
                    return Err(ManifestError::UnsupportedCrateType {
                        path: manifest_path.to_path_buf(),
                        found: other.to_string(),
                    })
                }
            };
            let (lib_path, synthesized) = match &rl.entry {
                Some(p) => {
                    let joined = root.join(p);
                    if target_path_escapes(&root, &joined) {
                        return Err(ManifestError::TargetPathEscapes {
                            path: manifest_path.to_path_buf(),
                            target: "[library] entry".to_string(),
                            requested: p.clone(),
                        });
                    }
                    (joined, false)
                }
                // No entry: the whole of src/ is the library. Same root-module
                // preference the prebuild synthesis uses.
                None => {
                    let named = root.join("src").join(format!("{name}.cplus"));
                    let path = if named.is_file() {
                        named
                    } else {
                        root.join("src").join("lib.cplus")
                    };
                    (path, true)
                }
            };
            Some(LibTarget {
                name: lib_name,
                path: lib_path,
                crate_type,
                synthesized,
                frameworks: rl.frameworks.clone(),
                libs: rl.libs.clone(),
            })
        }
    };

    // `prebuild` synthesizes a staticlib target: a package compiled once and
    // reused is a library by definition, so it needs no `[library]` section.
    // An explicit `[library]` still wins. The entry is `src/<name>.cplus`
    // when that exists (a package usually names its root module after
    // itself, as stdlib does), otherwise the conventional `src/lib.cplus`.
    //
    // Gated on the package NOT being an app: with prebuild defaulting to
    // true (2026-08-16), an ungated synthesis would hand every scaffolded
    // app a library target — and the entry default below treats "a library
    // is declared" as "not a zero-config app", so `cpc build` would archive
    // instead of producing the executable. An app is never consumed as a
    // dependency, so prebuild simply has no meaning for it.
    let package_is_app = raw.package.entry.is_some()
        || [
            &raw.macos,
            &raw.linux,
            &raw.windows,
            &raw.ios,
            &raw.android,
            &raw.esp32,
            &raw.wasm,
        ]
        .iter()
        .any(|s| s.as_ref().is_some_and(|s| s.entry.is_some()))
        || root.join("src").join("main.cplus").is_file();
    let lib = match (lib, build.prebuild && !package_is_app) {
        (None, true) => {
            let named = root.join("src").join(format!("{name}.cplus"));
            let path = if named.is_file() {
                named
            } else {
                root.join("src").join("lib.cplus")
            };
            Some(LibTarget {
                name: name.clone(),
                path,
                crate_type: CrateType::Staticlib,
                synthesized: true,
                frameworks: Vec::new(),
                libs: Vec::new(),
            })
        }
        (other, _) => other,
    };

    // App entries. `[package] entry` names one on purpose; the bare
    // `src/main.cplus` convention keeps the zero-config app working — but
    // only when NO platform section declares an entry (see below): declared
    // platform entries mean the author scoped the app deliberately, and a
    // platform they left out must be E0413, not a silent default. A library
    // package (explicit or prebuild-synthesized) is never an app — declaring
    // both is E0408.
    let package_entry_declared = raw.package.entry.is_some();
    let entry: Option<PathBuf> = match &raw.package.entry {
        Some(p) => {
            let joined = root.join(p);
            if target_path_escapes(&root, &joined) {
                return Err(ManifestError::TargetPathEscapes {
                    path: manifest_path.to_path_buf(),
                    target: "[package] entry".to_string(),
                    requested: p.clone(),
                });
            }
            Some(joined)
        }
        None => None,
    };

    // Phase 2: convert raw `[link]` to LinkSpec. The pure-source-package case (no [link]
    // table at all) yields `link = None`; an empty [link] table still
    // round-trips as `Some(LinkSpec::default())` which is harmless.
    let link = match raw.link {
        None => None,
        Some(rl) => {
            // v0.0.20: expand `${VAR}` / `${VAR:-default}` in path entries
            // before resolving, so a vendor binding can point at an external
            // SDK via the environment instead of a hardcoded absolute path.
            // v0.0.9 Phase 8 (cpc-gaps G-001): resolve each extra-object
            // path relative to the manifest directory. We don't check
            // file existence at parse time — that happens at link time
            // (E0864) so the diagnostic carries the full link context.
            let extra_objects: Vec<PathBuf> = expand_link_entries(rl.extra_objects, manifest_path)?
                .into_iter()
                .map(|p| root.join(p))
                .collect();
            // Resolve each search path against the manifest dir. `join` is
            // a no-op for absolute inputs (the common case — system SDK
            // dirs like /usr/local/cuda/lib64), so this only rewrites
            // relative entries.
            let search_paths: Vec<String> = expand_link_entries(rl.search_paths, manifest_path)?
                .into_iter()
                .map(|p| root.join(p).to_string_lossy().into_owned())
                .collect();
            Some(LinkSpec {
                frameworks: rl.frameworks,
                libs: rl.libs,
                bundled: rl.bundled,
                extra_objects,
                search_paths,
            })
        }
    };

    // Phase 2: validate every dep name against the lowercase-ident rule
    // so the first segment of an import path is unambiguous. Iterate in
    // BTreeMap order so any failure is deterministic.
    let mut dep_map: std::collections::BTreeMap<String, Dependency> =
        std::collections::BTreeMap::new();
    for (dep_name, dep_version) in raw.dependencies {
        if !is_valid_dep_name(&dep_name) {
            return Err(ManifestError::InvalidDependencyName {
                path: manifest_path.to_path_buf(),
                found: dep_name,
            });
        }
        dep_map.insert(
            dep_name.clone(),
            Dependency {
                name: dep_name,
                version: dep_version,
                platforms: Vec::new(),
            },
        );
    }
    // Platform-scoped sections, in PLATFORMS order (deterministic). The
    // same dep may appear in several platform sections with the SAME spec
    // (merged: active on all of them); anything else is E0869 — one
    // package, one spec, because the package manager has no conflict
    // resolver by design.
    let sections: [(&str, Option<RawPlatformSection>); 7] = [
        ("macos", raw.macos),
        ("linux", raw.linux),
        ("windows", raw.windows),
        ("ios", raw.ios),
        ("android", raw.android),
        ("esp32", raw.esp32),
        ("wasm", raw.wasm),
    ];
    let mut platform_entries: std::collections::BTreeMap<String, PathBuf> =
        std::collections::BTreeMap::new();
    for (platform, section) in sections {
        let Some(section) = section else { continue };
        if let Some(p) = &section.entry {
            let joined = root.join(p);
            if target_path_escapes(&root, &joined) {
                return Err(ManifestError::TargetPathEscapes {
                    path: manifest_path.to_path_buf(),
                    target: format!("[{platform}] entry"),
                    requested: p.clone(),
                });
            }
            platform_entries.insert(platform.to_string(), joined);
        }
        for (dep_name, dep_version) in section.dependencies {
            if !is_valid_dep_name(&dep_name) {
                return Err(ManifestError::InvalidDependencyName {
                    path: manifest_path.to_path_buf(),
                    found: dep_name,
                });
            }
            match dep_map.get_mut(&dep_name) {
                None => {
                    dep_map.insert(
                        dep_name.clone(),
                        Dependency {
                            name: dep_name,
                            version: dep_version,
                            platforms: vec![platform.to_string()],
                        },
                    );
                }
                Some(existing) if existing.platforms.is_empty() => {
                    return Err(ManifestError::ConflictingDependency {
                        path: manifest_path.to_path_buf(),
                        name: dep_name,
                        message: format!(
                            "is declared in `[dependencies]` (all platforms) and again in `[{platform}.dependencies]` — remove one"
                        ),
                    });
                }
                Some(existing) if existing.version != dep_version => {
                    return Err(ManifestError::ConflictingDependency {
                        path: manifest_path.to_path_buf(),
                        name: dep_name,
                        message: format!(
                            "is declared with spec `{}` (for {}) and spec `{dep_version}` in `[{platform}.dependencies]` — platform sections may share a dep only with an identical spec",
                            existing.version,
                            existing.platforms.join(", "),
                        ),
                    });
                }
                Some(existing) => existing.platforms.push(platform.to_string()),
            }
        }
    }
    let dependencies: Vec<Dependency> = dep_map.into_values().collect();

    // An app entry and a `[library]` cannot coexist — the declared kinds
    // contradict. (The src/main.cplus DEFAULT never conflicts: it is only
    // picked up when no library target exists.)
    let entry_declared = package_entry_declared || !platform_entries.is_empty();
    if lib.is_some() && entry_declared {
        return Err(ManifestError::EntryAndLibraryConflict {
            path: manifest_path.to_path_buf(),
        });
    }

    // The src/main.cplus default: only for a package that declared nothing —
    // no library target and no platform-scoped entry.
    let entry = match entry {
        Some(e) => Some(e),
        None => {
            let default = root.join("src").join("main.cplus");
            if lib.is_none() && platform_entries.is_empty() && default.is_file() {
                Some(default)
            } else {
                None
            }
        }
    };

    let realtime_profile = raw
        .profile
        .and_then(|p| p.realtime)
        .map(|r| RealtimeProfile {
            deny_alloc: r.deny_alloc,
            deny_block: r.deny_block,
            deny_unknown_extern: r.deny_unknown_extern,
            stack_limit: r.stack_limit,
        });

    Ok(Manifest {
        package: Package {
            name,
            version,
            edition,
        },
        entry,
        entry_declared,
        platform_entries,
        lib,
        link,
        dependencies,
        root,
        build,
        realtime_profile,
    })
}

/// v0.0.20: expand `${VAR}` / `${VAR:-default}` references in a manifest
/// `[link]` path entry against `lookup` (the process environment, in
/// production). Lets a vendor binding point at an external SDK via the
/// environment instead of baking one workstation's absolute path into the
/// manifest. Plain text passes through untouched; a bare `$` not followed by
/// `{` is literal. Returns `Err(message)` on a malformed `${...}` (no closing
/// `}`) or an unset variable with no `:-default` fallback.
///
/// `lookup` is injected (rather than calling `std::env` directly) so the
/// expansion logic is unit-testable without mutating process-global state.
fn expand_env_vars(input: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(dollar) = rest.find("${") {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 2..];
        let close = after
            .find('}')
            .ok_or_else(|| "unterminated `${` (missing closing `}`)".to_string())?;
        let spec = &after[..close];
        // `${VAR}` or `${VAR:-fallback}` (shell-style default).
        let (var, default) = match spec.split_once(":-") {
            Some((v, d)) => (v.trim(), Some(d)),
            None => (spec.trim(), None),
        };
        if var.is_empty() {
            return Err("empty variable name in `${...}`".to_string());
        }
        match lookup(var) {
            Some(val) => out.push_str(&val),
            None => match default {
                Some(d) => out.push_str(d),
                None => {
                    return Err(format!(
                        "environment variable `{var}` is not set (set it, or supply a fallback with `${{{var}:-/default/path}}`)"
                    ))
                }
            },
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Expand every entry of a `[link]` path list against the process
/// environment, surfacing the offending entry in the error so the
/// diagnostic is actionable.
fn expand_link_entries(
    entries: Vec<String>,
    manifest_path: &Path,
) -> Result<Vec<String>, ManifestError> {
    let lookup = |k: &str| std::env::var(k).ok();
    entries
        .into_iter()
        .map(|entry| match expand_env_vars(&entry, &lookup) {
            Ok(expanded) => Ok(expanded),
            Err(message) => Err(ManifestError::EnvExpansion {
                path: manifest_path.to_path_buf(),
                entry,
                message,
            }),
        })
        .collect()
}

/// Phase 2: dep names must match `[a-z][a-z0-9_]*` so the first segment
/// of an import path (e.g. `stdlib/io`) is an unambiguous identifier.
/// Rejects `Stdlib`, `stdlib/vec`, `std.lib`, and the empty string.
fn is_valid_dep_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => return false,
        Some(c) if !c.is_ascii_lowercase() => return false,
        _ => {}
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_in(dir: &Path, text: &str) -> Result<Manifest, ManifestError> {
        // Tests pin the manifest's "directory" via the path passed to
        // parse(); we want to verify bin paths anchor relative to that
        // directory. Use a real existing dir so canonicalize succeeds
        // predictably; std::env::temp_dir() exists on every platform.
        parse(text, &dir.join("Cplus.toml"))
    }

    #[test]
    fn no_profile_table_means_none() {
        let text = r#"
            [package]
            name = "hello"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert!(m.realtime_profile.is_none());
    }

    #[test]
    fn profile_realtime_parsed() {
        let text = r#"
            [package]
            name = "rt"

            [profile.realtime]
            deny-alloc = true
            deny-block = true
            deny-unknown-extern = true
            stack-limit = 4096
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let p = m.realtime_profile.expect("profile parsed");
        assert!(p.deny_alloc);
        assert!(p.deny_block);
        assert!(p.deny_unknown_extern);
        assert_eq!(p.stack_limit, Some(4096));
    }

    #[test]
    fn profile_realtime_snake_case_key_is_rejected() {
        // The old snake_case spelling is now a hard parse error (not a silent
        // drop that leaves the safety gate off).
        let text = r#"
            [package]
            name = "rt"

            [profile.realtime]
            deny_alloc = true
        "#;
        assert!(
            parse_in(&std::env::temp_dir(), text).is_err(),
            "snake_case `deny_alloc` must be rejected under kebab-case + deny_unknown_fields"
        );
    }

    #[test]
    fn unknown_manifest_key_is_rejected() {
        // A typo'd package key (`verison`) must be a hard error, not silently
        // ignored.
        let text = r#"
            [package]
            name = "x"
            verison = "1.0"
        "#;
        assert!(
            parse_in(&std::env::temp_dir(), text).is_err(),
            "unknown manifest key must be rejected"
        );
    }

    #[test]
    fn profile_realtime_defaults_when_fields_omitted() {
        let text = r#"
            [package]
            name = "rt"

            [profile.realtime]
            deny-alloc = true
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let p = m.realtime_profile.expect("profile parsed");
        assert!(p.deny_alloc);
        assert!(!p.deny_block);
        assert!(!p.deny_unknown_extern);
        assert_eq!(p.stack_limit, None);
    }

    /// A fresh directory under the system temp dir, for tests that need
    /// real files (the src/main.cplus default is existence-gated).
    fn fresh_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cpc-manifest-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn assert_entry_relpath(m: &Manifest, expected_rel: &str) {
        let actual = m
            .entry
            .as_ref()
            .expect("expected a package-level entry")
            .strip_prefix(&m.root)
            .expect("entry path should sit under manifest root");
        assert_eq!(actual, Path::new(expected_rel));
    }

    #[test]
    fn minimum_package_only_is_a_library() {
        // No entry key and no src/main.cplus on disk: a library package —
        // and since prebuild defaults to true (2026-08-16), it carries the
        // synthesized staticlib target without writing a word.
        let text = r#"
            [package]
            name = "hello"
        "#;
        let dir = fresh_dir("min");
        let m = parse_in(&dir, text).unwrap();
        assert_eq!(m.package.name, "hello");
        assert_eq!(m.package.version, "0.0.0");
        assert_eq!(m.package.edition, "2026");
        assert!(m.entry.is_none());
        assert!(!m.entry_declared);
        assert!(!m.is_app());
        let lib = m.lib.expect("prebuild-by-default synthesizes the lib target");
        assert!(lib.synthesized);
        assert!(m.build.prebuild, "prebuild is the default");
        // An explicit opt-out yields the old shape.
        let m = parse_in(
            &fresh_dir("min-optout"),
            "[package]\nname = \"hello\"\n\n[build]\nprebuild = false\n",
        )
        .unwrap();
        assert!(!m.build.prebuild);
        assert!(m.lib.is_none());
    }

    #[test]
    fn src_main_default_makes_an_app() {
        // The zero-config app: src/main.cplus on disk, no entry key.
        let text = r#"
            [package]
            name = "hello"
        "#;
        let dir = fresh_dir("default-entry");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.cplus"), "fn main() -> i32 { return 0; }").unwrap();
        let m = parse_in(&dir, text).unwrap();
        assert!(m.is_app());
        assert!(!m.entry_declared, "the default is picked up, not declared");
        assert_entry_relpath(&m, "src/main.cplus");
        // Every platform resolves to the package-level entry.
        assert!(m.entry_for("macos").is_some());
        assert!(m.entry_for("ios").is_some());
    }

    #[test]
    fn explicit_package_entry() {
        let text = r#"
            [package]
            name = "hello"
            version = "0.1.0"
            edition = "2026"
            entry = "src/entry.cplus"
        "#;
        let dir = fresh_dir("pkg-entry");
        let m = parse_in(&dir, text).unwrap();
        assert!(m.is_app());
        assert!(m.entry_declared);
        assert_entry_relpath(&m, "src/entry.cplus");
    }

    #[test]
    fn platform_entry_overrides_package_entry() {
        let text = r#"
            [package]
            name = "gallery"
            entry = "src/main.cplus"

            [ios]
            entry = "src/main_ios.cplus"
            [ios.dependencies]
            facet_uikit = "*"
        "#;
        let dir = fresh_dir("plat-entry");
        let m = parse_in(&dir, text).unwrap();
        assert!(m.is_app());
        assert!(m.entry_declared);
        assert!(m.entry_for("macos").unwrap().ends_with("src/main.cplus"));
        assert!(m.entry_for("ios").unwrap().ends_with("src/main_ios.cplus"));
    }

    #[test]
    fn platform_only_entry_has_no_other_platforms() {
        // An iOS-only app: building it for macos must see None (the driver
        // turns that into E0413, never a silent library build).
        let text = r#"
            [package]
            name = "gallery_ios"

            [ios]
            entry = "src/main.cplus"
        "#;
        let dir = fresh_dir("ios-only");
        let m = parse_in(&dir, text).unwrap();
        assert!(m.is_app());
        assert!(m.entry_for("ios").is_some());
        assert!(m.entry_for("macos").is_none());
    }

    #[test]
    fn platform_entry_suppresses_the_src_main_default() {
        // The iOS-gallery shape: `[ios] entry = "src/main.cplus"` names the
        // SAME file the default would pick up. A platform the author left
        // out must resolve to None (→ E0413 in the driver), never to the
        // default — the declared scoping is the intent.
        let text = r#"
            [package]
            name = "gallery_ios"

            [ios]
            entry = "src/main.cplus"
        "#;
        let dir = fresh_dir("suppress-default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.cplus"), "// ios entry\n").unwrap();
        let m = parse_in(&dir, text).unwrap();
        assert!(m.entry_for("ios").is_some());
        assert!(
            m.entry_for("macos").is_none(),
            "the default must not leak into undeclared platforms"
        );
    }

    #[test]
    fn entry_escaping_the_package_rejected_e0868() {
        let text = r#"
            [package]
            name = "evil"
            entry = "../../outside.cplus"
        "#;
        let err = parse_in(&fresh_dir("escape"), text).unwrap_err();
        assert!(matches!(err, ManifestError::TargetPathEscapes { .. }));
        let text2 = r#"
            [package]
            name = "evil"

            [ios]
            entry = "/etc/passwd"
        "#;
        let err2 = parse_in(&fresh_dir("escape2"), text2).unwrap_err();
        assert!(matches!(err2, ManifestError::TargetPathEscapes { .. }));
    }

    #[test]
    fn missing_name_errors() {
        let text = r#"
            [package]
            version = "0.1.0"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::MissingField {
                field: "package.name",
                ..
            }
        ));
    }

    #[test]
    fn unsupported_edition_errors() {
        let text = r#"
            [package]
            name = "x"
            edition = "2018"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedEdition { .. }));
    }

    #[test]
    fn malformed_toml_errors() {
        let text = "[[[ not valid";
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    #[test]
    fn legacy_bin_section_rejected_with_migration_e0408() {
        // The removed Cargo shape must fail with the migration in the
        // message, whatever its body contains.
        let text = r#"
            [package]
            name = "appkit_hello"

            [[bin]]
            name = "appkit_hello"
            path = "src/main.cplus"
            frameworks = ["Cocoa", "Foundation"]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::LegacyTargetSection { section: "[[bin]]", .. }),
            "expected LegacyTargetSection, got: {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0408"));
    }

    #[test]
    fn legacy_lib_section_rejected_with_migration_e0408() {
        let text = r#"
            [package]
            name = "gallery_ios"

            [lib]
            crate-type = "staticlib"
            path = "src/main.cplus"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::LegacyTargetSection { section: "[lib]", .. }),
            "expected LegacyTargetSection, got: {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0408"));
    }

    #[test]
    fn library_section_defaults_to_synthesized_staticlib() {
        // `[library]` with no keys: kind = staticlib, entry synthesized
        // (whole of src/, root-module preference).
        let text = r#"
            [package]
            name = "mathlib"

            [library]
        "#;
        let m = parse_in(&fresh_dir("lib-default"), text).unwrap();
        assert!(!m.is_app());
        let lib = m.lib.expect("expected a library target");
        assert_eq!(lib.name, "mathlib");
        assert_eq!(lib.crate_type, CrateType::Staticlib);
        assert!(lib.synthesized, "no entry key means the whole src/ tree");
        assert!(lib.path.ends_with("src/lib.cplus"));
    }

    #[test]
    fn library_explicit_entry_is_the_c_abi_surface() {
        let text = r#"
            [package]
            name = "mathlib"

            [library]
            entry = "src/lib.cplus"
            kind  = "both"
        "#;
        let m = parse_in(&fresh_dir("lib-entry"), text).unwrap();
        let lib = m.lib.unwrap();
        assert!(!lib.synthesized, "an explicit entry is deliberate");
        assert_eq!(lib.crate_type, CrateType::Both);
        assert!(lib.path.ends_with("src/lib.cplus"));
    }

    #[test]
    fn library_kind_values_parse() {
        let text = r#"
            [package]
            name = "x"

            [library]
            kind = "cdylib"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.lib.unwrap().crate_type, CrateType::Cdylib);

        let text2 = r#"
            [package]
            name = "x"

            [library]
            kind = "staticlib"
        "#;
        let m2 = parse_in(&std::env::temp_dir(), text2).unwrap();
        assert_eq!(m2.lib.unwrap().crate_type, CrateType::Staticlib);
    }

    #[test]
    fn library_rejects_unknown_kind_e0412() {
        let text = r#"
            [package]
            name = "x"

            [library]
            kind = "rlib"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::UnsupportedCrateType { .. }),
            "expected UnsupportedCrateType, got: {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0412"));
    }

    #[test]
    fn entry_and_library_together_emit_e0408() {
        let text = r#"
            [package]
            name = "x"
            entry = "src/main.cplus"

            [library]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::EntryAndLibraryConflict { .. }),
            "expected EntryAndLibraryConflict, got: {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0408"));
    }

    #[test]
    fn library_ignores_src_main_default() {
        // A `[library]` package that happens to have src/main.cplus on disk
        // stays a library — the DEFAULT never conflicts, only a declared key.
        let text = r#"
            [package]
            name = "x"

            [library]
        "#;
        let dir = fresh_dir("lib-with-main");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.cplus"), "// probe\n").unwrap();
        let m = parse_in(&dir, text).unwrap();
        assert!(m.lib.is_some());
        assert!(!m.is_app());
        assert!(m.entry.is_none());
    }

    #[test]
    fn library_section_carries_frameworks_and_libs() {
        // A library can declare its own linker flags — baked into the
        // .dylib (or recorded for the consumer's link line in .a).
        let text = r#"
            [package]
            name = "uikit_wrapper"

            [library]
            frameworks = ["UIKit"]
            libs = ["objc"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let lib = m.lib.unwrap();
        assert_eq!(lib.frameworks, vec!["UIKit".to_string()]);
        assert_eq!(lib.libs, vec!["objc".to_string()]);
    }

    // ---- Phase 2 Slice 2A: [dependencies] + top-level [link] ----

    #[test]
    fn dependencies_table_parses() {
        let text = r#"
            [package]
            name = "consumer"

            [dependencies]
            stdlib = "*"
            tiny   = "0.1.0"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 2);
        // BTreeMap → lexicographic order.
        assert_eq!(m.dependencies[0].name, "stdlib");
        assert_eq!(m.dependencies[0].version, "*");
        assert_eq!(m.dependencies[1].name, "tiny");
        assert_eq!(m.dependencies[1].version, "0.1.0");
    }

    #[test]
    fn dependencies_absent_yields_empty_vec() {
        let text = r#"
            [package]
            name = "x"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn invalid_dep_name_uppercase_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            Stdlib = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::InvalidDependencyName { ref found, .. } if found == "Stdlib"),
            "expected InvalidDependencyName for `Stdlib`, got: {err:?}"
        );
    }

    #[test]
    fn invalid_dep_name_slash_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "stdlib/vec" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::InvalidDependencyName { .. }),
            "expected InvalidDependencyName for `stdlib/vec`, got: {err:?}"
        );
    }

    #[test]
    fn invalid_dep_name_dot_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "std.lib" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { .. }));
    }

    #[test]
    fn invalid_dep_name_leading_digit_rejected_e0857() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            "1stplace" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDependencyName { .. }));
    }

    #[test]
    fn dep_name_with_underscore_and_digit_accepted() {
        let text = r#"
            [package]
            name = "x"

            [dependencies]
            stdlib_v2 = "*"
            tiny0     = "*"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 2);
    }

    // ---- platform-scoped dependency sections ----

    #[test]
    fn platform_sections_parse_and_scope_deps() {
        let text = r#"
            [package]
            name = "app"

            [dependencies]
            stdlib = "*"
            facet  = "*"

            [macos.dependencies]
            facet_appkit = "*"
            appkit       = "*"

            [linux.dependencies]
            facet_gtk = "*"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 5);
        let by_name = |n: &str| m.dependencies.iter().find(|d| d.name == n).unwrap();
        assert!(by_name("stdlib").platforms.is_empty(), "base deps are platform-free");
        assert_eq!(by_name("facet_appkit").platforms, vec!["macos".to_string()]);
        assert_eq!(by_name("appkit").platforms, vec!["macos".to_string()]);
        assert_eq!(by_name("facet_gtk").platforms, vec!["linux".to_string()]);
        // active_on: base deps everywhere, scoped deps only on their platform.
        assert!(by_name("stdlib").active_on("linux"));
        assert!(by_name("facet_appkit").active_on("macos"));
        assert!(!by_name("facet_appkit").active_on("linux"));
        assert!(!by_name("facet_gtk").active_on("macos"));
    }

    #[test]
    fn same_dep_in_two_platform_sections_merges_when_specs_match() {
        // objc exists on both Apple platforms; one dep, two platforms.
        let text = r#"
            [package]
            name = "app"

            [macos.dependencies]
            objc = "*"

            [ios.dependencies]
            objc = "*"
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.dependencies.len(), 1);
        let d = &m.dependencies[0];
        assert_eq!(d.name, "objc");
        assert_eq!(d.platforms, vec!["macos".to_string(), "ios".to_string()]);
        assert!(d.active_on("macos") && d.active_on("ios") && !d.active_on("linux"));
    }

    #[test]
    fn dep_in_base_and_platform_section_rejected_e0869() {
        let text = r#"
            [package]
            name = "app"

            [dependencies]
            objc = "*"

            [macos.dependencies]
            objc = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::ConflictingDependency { ref name, .. } if name == "objc"),
            "expected ConflictingDependency, got: {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0869"));
    }

    #[test]
    fn same_dep_with_different_specs_across_sections_rejected_e0869() {
        let text = r#"
            [package]
            name = "app"

            [macos.dependencies]
            objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.26"

            [ios.dependencies]
            objc = "https://github.com/netdur/cplus/tree/main/vendor/objc@0.0.25"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::ConflictingDependency { .. }),
            "two specs for one package must be rejected (no conflict resolver by design): {err:?}"
        );
        assert_eq!(err.to_diagnostic().code, DiagCode("E0869"));
    }

    #[test]
    fn invalid_dep_name_in_platform_section_rejected_e0857() {
        let text = r#"
            [package]
            name = "app"

            [linux.dependencies]
            "Gtk4" = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::InvalidDependencyName { ref found, .. } if found == "Gtk4"),
            "platform sections must apply the same name rule: {err:?}"
        );
    }

    #[test]
    fn unknown_platform_section_is_a_parse_error() {
        // `[macbook.dependencies]` — not a platform. deny_unknown_fields
        // turns it into a hard parse error, not a silently dead section.
        let text = r#"
            [package]
            name = "app"

            [macbook.dependencies]
            facet_appkit = "*"
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }), "got: {err:?}");
    }

    #[test]
    fn platform_section_keys_other_than_dependencies_are_reserved() {
        // `[macos.link]` is future work; today it must fail loudly rather
        // than be ignored (a silently-dropped link table means missing
        // frameworks at runtime, not build time).
        let text = r#"
            [package]
            name = "app"

            [macos.link]
            frameworks = ["AppKit"]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }), "got: {err:?}");
    }

    #[test]
    fn empty_platform_section_is_harmless() {
        let text = r#"
            [package]
            name = "app"

            [wasm]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn top_level_link_table_parses() {
        let text = r#"
            [package]
            name = "appkit"

            [link]
            frameworks = ["Cocoa"]
            libs       = ["objc"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.frameworks, vec!["Cocoa".to_string()]);
        assert_eq!(link.libs, vec!["objc".to_string()]);
        assert!(link.bundled.is_empty());
    }

    #[test]
    fn top_level_link_with_bundled_parses() {
        let text = r#"
            [package]
            name = "curl_bindings"

            [link]
            bundled = ["curl.a"]
            libs    = ["z"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.bundled, vec!["curl.a".to_string()]);
        assert_eq!(link.libs, vec!["z".to_string()]);
    }

    #[test]
    fn bundled_needs_no_triples_declaration() {
        // The triple is derived from what's being built, never declared, so
        // `bundled` alone is a complete statement. A `triples` key is now an
        // unknown field — the manifest refuses it rather than ignoring it.
        let text = r#"
            [package]
            name = "x"

            [link]
            bundled = ["foo.a"]
            triples = ["aarch64-apple-darwin"]
        "#;
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse { .. }),
            "expected a parse error naming `triples`, got {err:?}"
        );
    }

    #[test]
    fn link_search_paths_parse() {
        // `search-paths` carries library dirs (-> -L / -rpath). Absolute
        // entries pass through unchanged; relative ones resolve against the
        // manifest dir.
        let root = std::env::temp_dir();
        // An absolute path passes through unchanged. "Absolute" is
        // platform-specific: a Unix path like `/usr/local/cuda/lib64` has no
        // drive and is NOT absolute on Windows (it would be resolved relative
        // to the manifest dir), so use a drive-qualified path there.
        let abs = if cfg!(windows) {
            "C:/cuda/lib64"
        } else {
            "/usr/local/cuda/lib64"
        };
        let text = format!(
            "
            [package]
            name = \"cuda\"

            [link]
            libs         = [\"cudart\", \"cublas\"]
            search-paths = [\"{abs}\", \"vendored/lib\"]
        "
        );
        let m = parse_in(&root, &text).unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.libs, vec!["cudart".to_string(), "cublas".to_string()]);
        assert_eq!(link.search_paths.len(), 2);
        assert_eq!(link.search_paths[0], abs);
        // Compare against the manifest's own canonicalized `root` (not the
        // raw `temp_dir()`): `parse` canonicalizes the manifest dir, and on
        // macOS `/tmp` is a symlink to `/private/tmp`, so the raw temp path
        // would not match the resolved one.
        assert_eq!(
            link.search_paths[1],
            m.root.join("vendored/lib").to_string_lossy()
        );
    }

    // ---- v0.0.20: `${VAR}` / `${VAR:-default}` expansion in `[link]` paths ----

    #[test]
    fn expand_env_vars_plain_text_passthrough() {
        let none = |_: &str| None;
        assert_eq!(
            expand_env_vars("/usr/local/lib", &none).unwrap(),
            "/usr/local/lib"
        );
        // A bare `$` not followed by `{` is literal — only `${...}` is special.
        assert_eq!(
            expand_env_vars("/opt/$HOME/lib", &none).unwrap(),
            "/opt/$HOME/lib"
        );
    }

    #[test]
    fn expand_env_vars_substitutes_set_variable() {
        let lookup = |k: &str| (k == "SDK").then(|| "/opt/sdk".to_string());
        assert_eq!(
            expand_env_vars("${SDK}/lib", &lookup).unwrap(),
            "/opt/sdk/lib"
        );
        // Multiple references in one entry all expand.
        assert_eq!(
            expand_env_vars("${SDK}/lib:${SDK}/bin", &lookup).unwrap(),
            "/opt/sdk/lib:/opt/sdk/bin"
        );
    }

    #[test]
    fn expand_env_vars_default_used_when_unset() {
        let none = |_: &str| None;
        assert_eq!(
            expand_env_vars("${SDK:-/usr/local/cuda/lib64}", &none).unwrap(),
            "/usr/local/cuda/lib64"
        );
        // A set value wins over the `:-default`.
        let set = |k: &str| (k == "SDK").then(|| "/opt/sdk".to_string());
        assert_eq!(
            expand_env_vars("${SDK:-/fallback}", &set).unwrap(),
            "/opt/sdk"
        );
    }

    #[test]
    fn expand_env_vars_unset_no_default_errors() {
        let none = |_: &str| None;
        let err = expand_env_vars("${MISSING_SDK}/lib", &none).unwrap_err();
        assert!(
            err.contains("MISSING_SDK"),
            "error names the variable: {err}"
        );
    }

    #[test]
    fn expand_env_vars_malformed_errors() {
        let none = |_: &str| None;
        // Unterminated `${`.
        assert!(expand_env_vars("${SDK/lib", &none).is_err());
        assert!(expand_env_vars("${", &none).is_err());
        // Empty variable name.
        assert!(expand_env_vars("${}", &none).is_err());
        assert!(expand_env_vars("${:-x}", &none).is_err());
    }

    #[test]
    fn link_search_paths_expand_env_var_through_parse() {
        // End-to-end: `${VAR}` in search-paths is expanded against the process
        // environment during parse(). Unique var name avoids cross-test races.
        // "Absolute" is platform-specific (see link_search_paths_parse), so an
        // absolute value passes through root.join unchanged on each OS.
        let var = "CPLUS_TEST_LLAMA_LIB_X1";
        let abs = if cfg!(windows) {
            "C:/llama/build/bin"
        } else {
            "/opt/llama/build/bin"
        };
        std::env::set_var(var, abs);
        let text = format!(
            "
            [package]
            name = \"llama_cpp\"

            [link]
            libs         = [\"llama\"]
            search-paths = [\"${{{var}}}\"]
        "
        );
        let parsed = parse_in(&std::env::temp_dir(), &text);
        std::env::remove_var(var);
        let m = parsed.unwrap();
        let link = m.link.expect("expected [link]");
        assert_eq!(link.search_paths[0], abs);
    }

    #[test]
    fn link_search_paths_env_default_fallback_through_parse() {
        // `${VAR:-default}` with VAR unset resolves to the default — this is
        // how vendor/cuda keeps `/usr/local/cuda/lib64` as a sane default
        // while staying overridable via the environment.
        let abs = if cfg!(windows) {
            "C:/cuda/lib64"
        } else {
            "/usr/local/cuda/lib64"
        };
        let text = format!(
            "
            [package]
            name = \"cuda\"

            [link]
            search-paths = [\"${{CPLUS_UNSET_CUDA_LIB_Z7:-{abs}}}\"]
        "
        );
        let m = parse_in(&std::env::temp_dir(), &text).unwrap();
        let link = m.link.unwrap();
        assert_eq!(link.search_paths[0], abs);
    }

    #[test]
    fn link_search_paths_unset_env_var_rejected_e0865() {
        // No default + unset var → E0865 at parse time, naming the offending
        // entry so the user knows which variable to set.
        let text = "
            [package]
            name = \"x\"

            [link]
            search-paths = [\"${CPLUS_DEFINITELY_UNSET_VAR_Q9}/lib\"]
        ";
        let err = parse_in(&std::env::temp_dir(), text).unwrap_err();
        match &err {
            ManifestError::EnvExpansion { entry, .. } => {
                assert!(entry.contains("CPLUS_DEFINITELY_UNSET_VAR_Q9"));
            }
            other => panic!("expected EnvExpansion, got {other:?}"),
        }
        assert_eq!(err.to_diagnostic().code, DiagCode("E0865"));
    }

    #[test]
    fn link_extra_objects_also_expand_env_vars() {
        // extra-objects are paths too, so they get the same treatment.
        let var = "CPLUS_TEST_OBJ_DIR_K3";
        let abs = if cfg!(windows) {
            "C:/objs"
        } else {
            "/opt/objs"
        };
        std::env::set_var(var, abs);
        let text = format!(
            "
            [package]
            name = \"x\"

            [link]
            extra-objects = [\"${{{var}}}/startup.o\"]
        "
        );
        let parsed = parse_in(&std::env::temp_dir(), &text);
        std::env::remove_var(var);
        let m = parsed.unwrap();
        let link = m.link.unwrap();
        assert_eq!(
            link.extra_objects[0].to_string_lossy(),
            format!("{abs}/startup.o")
        );
    }

    #[test]
    fn bundled_alone_is_a_complete_declaration() {
        // A package that ships binaries names the files and nothing else: the
        // triple that locates them comes from the build, and whether they are
        // present at all is answered by the directory, not the manifest.
        let text = r#"
            [package]
            name = "x"

            [link]
            bundled = ["foo.a"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        assert_eq!(m.link.unwrap().bundled, vec!["foo.a".to_string()]);
    }

    #[test]
    fn empty_link_table_parses_as_some_default() {
        let text = r#"
            [package]
            name = "x"

            [link]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert!(link.frameworks.is_empty());
        assert!(link.libs.is_empty());
        assert!(link.bundled.is_empty());
        assert!(link.extra_objects.is_empty());
    }

    // ---- v0.0.9 Phase 8 (cpc-gaps G-001): [link] extra-objects ----

    #[test]
    fn link_extra_objects_parses_kebab_case() {
        let text = r#"
            [package]
            name = "x"

            [link]
            extra-objects = ["build/metallib.o", "build/shader_blob.o"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert_eq!(link.extra_objects.len(), 2);
        // Paths resolve relative to the manifest directory.
        assert!(link.extra_objects[0].ends_with("build/metallib.o"));
        assert!(link.extra_objects[1].ends_with("build/shader_blob.o"));
    }

    #[test]
    fn link_extra_objects_absent_defaults_empty() {
        // A [link] table with only frameworks/libs entries must still
        // produce an empty extra_objects vec (the backward-compat path).
        let text = r#"
            [package]
            name = "x"

            [link]
            frameworks = ["Cocoa"]
        "#;
        let m = parse_in(&std::env::temp_dir(), text).unwrap();
        let link = m.link.unwrap();
        assert!(link.extra_objects.is_empty());
        assert_eq!(link.frameworks, vec!["Cocoa".to_string()]);
    }

    #[test]
    fn link_extra_objects_paths_anchor_to_manifest_root() {
        // Verify that the resolved path is the manifest dir joined with
        // the relative entry — not, e.g., the process CWD.
        let dir = std::env::temp_dir().join("cpc-test-extra-objects");
        let _ = std::fs::create_dir_all(&dir);
        let text = r#"
            [package]
            name = "x"

            [link]
            extra-objects = ["foo.o"]
        "#;
        let m = parse_in(&dir, text).unwrap();
        let link = m.link.unwrap();
        // The resolved path should start with the manifest's `root`
        // (which is the canonicalized form of `dir`).
        assert!(
            link.extra_objects[0].starts_with(&m.root),
            "expected {} to start with {}",
            link.extra_objects[0].display(),
            m.root.display()
        );
    }

    #[test]
    fn is_valid_dep_name_unit_cases() {
        assert!(super::is_valid_dep_name("stdlib"));
        assert!(super::is_valid_dep_name("tiny0"));
        assert!(super::is_valid_dep_name("a_b_c"));
        assert!(!super::is_valid_dep_name(""));
        assert!(!super::is_valid_dep_name("Stdlib"));
        assert!(!super::is_valid_dep_name("0std"));
        assert!(!super::is_valid_dep_name("std-lib"));
        assert!(!super::is_valid_dep_name("std.lib"));
        assert!(!super::is_valid_dep_name("std/lib"));
        assert!(!super::is_valid_dep_name(" std"));
    }

    #[test]
    fn package_entry_escaping_package_is_rejected_e0868() {
        // An `entry` with a `..` chain (or absolute path) must not point
        // compilation at files outside the package tree — the same
        // containment the import paths enforce (E0859/E0914).
        for bad in ["../../outside/main.cplus", "/etc/hosts"] {
            let text =
                format!("[package]\nname = \"esc\"\nentry = \"{bad}\"\n");
            let err = parse_in(&std::env::temp_dir(), &text)
                .expect_err("escaping entry must be rejected");
            assert_eq!(err.to_diagnostic().code, DiagCode("E0868"), "path: {bad}");
        }
    }

    #[test]
    fn library_entry_escaping_package_is_rejected_e0868() {
        let text =
            "[package]\nname = \"esc\"\n\n[library]\nentry = \"../sibling/lib.cplus\"\n";
        let err = parse_in(&std::env::temp_dir(), text)
            .expect_err("escaping library entry must be rejected");
        assert_eq!(err.to_diagnostic().code, DiagCode("E0868"));
    }

    #[test]
    fn in_tree_target_paths_are_allowed() {
        // Nested and `./`-relative in-tree paths must keep working; only a
        // path that actually leaves the package root is rejected.
        let text =
            "[package]\nname = \"ok\"\nentry = \"./tools/../src/main.cplus\"\n";
        let m = parse_in(&std::env::temp_dir(), text).expect("in-tree entry parses");
        assert!(m.entry.unwrap().ends_with("src/main.cplus"));
    }
}
