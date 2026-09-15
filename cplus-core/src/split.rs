//! Partition a library's IR into one module per source file, so the archive
//! has one member per module instead of one for the whole package.
//!
//! `cpc` compiles a package to one LLVM module, and it used to archive that as
//! one object. A static archive is pulled in a MEMBER at a time, so resolving
//! any symbol from a one-member archive dragged in every module the package
//! had. Measured: `examples/facet_gallery_android` imports `stdlib/option` and
//! `stdlib/vec` and linked all 24,618 lines of stdlib — `process` and `pty`
//! included, and with them the `posix_spawn` family bionic introduced at API
//! 28, which raised every Android app's floor from 24 to 28 for a code path no
//! facet app can reach. The same mechanism put a source-mode backend's JNI
//! entry points into a prebuilt facade's object, where the app's own copy then
//! collided with them
//! (bugs/closed/a-package-is-one-object-so-a-consumer-links-all-of-it.md).
//!
//! `-ffunction-sections` with `--gc-sections` does not help, and it is worth
//! ruling out by name because it is the first thing anyone tries: section
//! garbage collection runs AFTER symbol resolution, and once a member is
//! loaded every undefined symbol in it must resolve. It can shrink an output;
//! it cannot rescue a link.
//!
//! Like `prune.rs`, this is a text pass over the emitted IR rather than a walk
//! over the AST, for the same reason: on the emitted text a reference is a
//! literal `@name`, so the analysis cannot miss a reference kind — a call, a
//! fn-pointer, drop glue, a static initialiser — that an AST walk would have to
//! enumerate. The failure mode of a miss is an undefined symbol at link time,
//! never wrong behaviour.
//!
//! ## The partition
//!
//! Codegen precedes each item of a library build with `; cpc-home: <file id>`,
//! so every definition knows the module it came from. The symbol name says so
//! for `<pkg>.src.<module>.<item>` and says nothing for a blessed `impl f32`
//! method (`f32.sqrt`) or an `export extern fn` (its bare C name), which is why
//! the marker exists.
//!
//! Every definition is one of three things:
//!
//! - A ROOT: externally visible and concrete — a module's own functions, the
//!   ones a consumer's header declares. Defined in its home module's piece and
//!   DECLARED in any other piece that references it, so a cross-module
//!   reference is an undefined symbol the linker resolves by loading that
//!   module's member. That is exactly the granularity an archive should have.
//! - An INSTANCE: a monomorphized generic — `__` in the name, the mangling
//!   grammar's separator, which E0917 keeps out of user identifiers. It is
//!   `weak_odr` and COPIED into every piece that reaches it: C++'s answer for a
//!   template instantiated in two translation units. Placing it in the
//!   template's module instead would make `vec.o` carry `Vec[Child]::push`,
//!   whose drop of `Child` references `process` — and the API floor is back.
//! - LOCAL: `internal` or `private` — helpers, constants, trampolines. Copied
//!   into every piece that reaches them; the linker never sees the copies.
//!
//! Globals are copied where referenced: a private constant duplicates
//! harmlessly and a `weak_odr` static merges. Types, `declare`s and metadata
//! are shared by every piece. A definition with no home and nothing reaching
//! it lands in a residual piece named after the package, so nothing is dropped.
//!
//! ## The hazard, named
//!
//! An `internal` MUTABLE global copied into two pieces is two variables. That
//! happens only when an instance or an internal helper that touches it is
//! reached from another module, and it is not new: a consumer compiling a
//! generic module from its verbatim header already has its own copy of that
//! module's private state. [`split_by_home`] reports every such global in its
//! `notes` so the situation is visible rather than silent. The compiler's own
//! per-thread slots make the choice out loud: the reactor slot is `internal`
//! because one module is its only user; the cancel slot is `linkonce_odr
//! hidden` because thread trampolines everywhere write it.

use std::collections::{BTreeSet, HashMap, HashSet};

/// The comment codegen writes before each item of a library build. `-` after
/// it means "no home": what follows is compiler-synthesized.
pub const HOME_MARKER: &str = "; cpc-home: ";

pub fn write_home_marker(out: &mut String, home: Option<&str>) {
    out.push_str(HOME_MARKER);
    out.push_str(home.unwrap_or("-"));
    out.push('\n');
}

/// One object's worth of IR.
pub struct Piece {
    /// The module's file id (`stdlib.src.vec`), or the package name for the
    /// residual piece.
    pub module: String,
    pub ir: String,
}

pub struct Split {
    pub pieces: Vec<Piece>,
    /// Observations worth printing under `CPC_VERBOSE`: internal mutable
    /// globals that landed in more than one piece.
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Linkage {
    /// `internal` / `private`: invisible outside its object, so copied.
    Local,
    /// `weak_odr` / `linkonce_odr` and kin: equivalent copies merge.
    Mergeable,
    /// Plain external: exactly one definition in the whole link.
    External,
}

enum Kind {
    /// Types, metadata, `declare`s, target lines, comments: every piece.
    Shared,
    /// A home marker: read, then dropped.
    Marker,
    Define {
        name: String,
        home: Option<usize>,
        root: bool,
    },
    Global {
        name: String,
        mutable_local: bool,
    },
    /// `$name = comdat any` (COFF): travels with the definition it names.
    Comdat {
        name: String,
    },
    /// `module asm`: the home module's only, or it would be assembled once
    /// per piece.
    Asm {
        home: Option<usize>,
    },
}

struct Entity {
    kind: Kind,
    /// Line range, inclusive.
    start: usize,
    end: usize,
}

/// Partition `ir` by the home markers in it. `residual` names the piece that
/// takes definitions with no home. At least one piece is always returned, so
/// an archive is never empty.
pub fn split_by_home(ir: &str, residual: &str) -> Split {
    let lines: Vec<&str> = ir.lines().collect();
    let mut modules: Vec<String> = Vec::new();
    let mut module_ids: HashMap<String, usize> = HashMap::new();
    let mut entities: Vec<Entity> = Vec::new();
    let mut home: Option<usize> = None;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if let Some(h) = line.strip_prefix(HOME_MARKER) {
            home = if h == "-" {
                None
            } else {
                Some(*module_ids.entry(h.to_string()).or_insert_with(|| {
                    modules.push(h.to_string());
                    modules.len() - 1
                }))
            };
            entities.push(Entity {
                kind: Kind::Marker,
                start: i,
                end: i,
            });
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("define ") {
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "}" {
                j += 1;
            }
            if j >= lines.len() {
                // Unterminated — not a shape codegen emits. Leave the tail
                // shared rather than lose it.
                entities.push(Entity {
                    kind: Kind::Shared,
                    start: i,
                    end: lines.len() - 1,
                });
                break;
            }
            let name = first_symbol(rest).unwrap_or("").to_string();
            let head = &rest[..rest.find('@').unwrap_or(rest.len())];
            let linkage = linkage_of(head);
            let root = linkage == Linkage::External
                || (linkage == Linkage::Mergeable && !name.contains("__"));
            entities.push(Entity {
                kind: Kind::Define { name, home, root },
                start: i,
                end: j,
            });
            i = j + 1;
            continue;
        }
        if line.starts_with('@') {
            let name = first_symbol(line).unwrap_or("").to_string();
            let head = &line[..line.find(" global ").map(|p| p + 8).unwrap_or(0)];
            let mutable_local = !head.is_empty()
                && (head.contains(" internal ") || head.contains(" private "));
            entities.push(Entity {
                kind: Kind::Global {
                    name,
                    mutable_local,
                },
                start: i,
                end: i,
            });
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('$') {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            entities.push(Entity {
                kind: Kind::Comdat { name },
                start: i,
                end: i,
            });
            i += 1;
            continue;
        }
        if line.starts_with("module asm") {
            entities.push(Entity {
                kind: Kind::Asm { home },
                start: i,
                end: i,
            });
            i += 1;
            continue;
        }
        entities.push(Entity {
            kind: Kind::Shared,
            start: i,
            end: i,
        });
        i += 1;
    }
    let residual_id = modules.len();
    modules.push(residual.to_string());

    // Name → entity, for definitions and globals; and the names the shared
    // preamble already declares, so a cut never declares one twice.
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    let mut declared: HashSet<&str> = HashSet::new();
    for (idx, e) in entities.iter().enumerate() {
        match &e.kind {
            Kind::Define { name, .. } | Kind::Global { name, .. } => {
                by_name.insert(name.as_str(), idx);
            }
            Kind::Shared => {
                if let Some(rest) = lines[e.start].strip_prefix("declare ") {
                    if let Some(n) = first_symbol(rest) {
                        declared.insert(n);
                    }
                }
            }
            _ => {}
        }
    }
    // Edges: entity → the definitions and globals its text names.
    let refs: Vec<Vec<usize>> = entities
        .iter()
        .enumerate()
        .map(|(idx, e)| {
            let mut out: Vec<usize> = Vec::new();
            if !matches!(e.kind, Kind::Define { .. } | Kind::Global { .. }) {
                return out;
            }
            for l in e.start..=e.end {
                for name in symbol_refs(lines[l]) {
                    if let Some(&t) = by_name.get(name) {
                        if t != idx && !out.contains(&t) {
                            out.push(t);
                        }
                    }
                }
            }
            out
        })
        .collect();
    let home_of = |e: &Entity| -> Option<usize> {
        match &e.kind {
            Kind::Define { home, .. } | Kind::Asm { home } => Some(home.unwrap_or(residual_id)),
            _ => None,
        }
    };

    let mut pieces: Vec<Piece> = Vec::new();
    // Which pieces each internal mutable global landed in — the hazard report.
    let mut landings: HashMap<usize, Vec<String>> = HashMap::new();
    for m in 0..modules.len() {
        let roots: Vec<usize> = entities
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, Kind::Define { root: true, .. }) && home_of(e) == Some(m))
            .map(|(idx, _)| idx)
            .collect();
        let has_asm = entities
            .iter()
            .any(|e| matches!(e.kind, Kind::Asm { .. }) && home_of(e) == Some(m));
        // The residual piece exists when something needs it — or when nothing
        // else does, so the archive has a member.
        if roots.is_empty() && !has_asm && !(m == residual_id && pieces.is_empty()) {
            continue;
        }
        let mut included: HashSet<usize> = HashSet::new();
        let mut cuts: BTreeSet<usize> = BTreeSet::new();
        let mut stack: Vec<usize> = roots;
        while let Some(idx) = stack.pop() {
            if !included.insert(idx) {
                continue;
            }
            for &r in &refs[idx] {
                let e = &entities[r];
                let cut = matches!(e.kind, Kind::Define { root: true, .. }) && home_of(e) != Some(m);
                if cut {
                    cuts.insert(r);
                } else if !included.contains(&r) {
                    stack.push(r);
                }
            }
        }
        let included_names: HashSet<&str> = included
            .iter()
            .filter_map(|&idx| match &entities[idx].kind {
                Kind::Define { name, .. } | Kind::Global { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let mut out = String::with_capacity(ir.len() / modules.len().max(1) + 4096);
        for (idx, e) in entities.iter().enumerate() {
            let emit = match &e.kind {
                Kind::Shared => true,
                Kind::Marker => false,
                Kind::Define { .. } | Kind::Global { .. } => included.contains(&idx),
                Kind::Comdat { name } => included_names.contains(name.as_str()),
                Kind::Asm { .. } => home_of(e) == Some(m),
            };
            if !emit {
                continue;
            }
            if let Kind::Global {
                mutable_local: true,
                ..
            } = &e.kind
            {
                landings.entry(idx).or_default().push(modules[m].clone());
            }
            for l in e.start..=e.end {
                out.push_str(lines[l]);
                out.push('\n');
            }
        }
        for c in cuts {
            let Kind::Define { name, .. } = &entities[c].kind else {
                continue;
            };
            if declared.contains(name.as_str()) {
                continue;
            }
            match declare_from_define(lines[entities[c].start]) {
                Some(d) => {
                    out.push_str(&d);
                    out.push('\n');
                }
                None => {
                    // Not a shape codegen emits. Copying the definition keeps
                    // the piece linkable — every root is mergeable or external,
                    // and only the former can be copied — so say so loudly in
                    // a debug build and take the copy in release.
                    debug_assert!(false, "cannot derive a declaration from: {}", lines[entities[c].start]);
                    for l in entities[c].start..=entities[c].end {
                        out.push_str(lines[l]);
                        out.push('\n');
                    }
                }
            }
        }
        pieces.push(Piece {
            module: modules[m].clone(),
            ir: out,
        });
    }
    let mut notes: Vec<String> = Vec::new();
    let mut shared: Vec<(usize, Vec<String>)> = landings.into_iter().filter(|(_, v)| v.len() > 1).collect();
    shared.sort_by_key(|(idx, _)| *idx);
    for (idx, where_) in shared {
        if let Kind::Global { name, .. } = &entities[idx].kind {
            notes.push(format!(
                "internal global `@{name}` is a separate variable in each of {} objects ({})",
                where_.len(),
                where_.join(", ")
            ));
        }
    }
    Split { pieces, notes }
}

/// The linkage keywords a `define` may carry between `define` and its return
/// type. A declaration may not carry these, so a cut strips them.
const LINKAGES: &[&str] = &[
    "private",
    "internal",
    "available_externally",
    "linkonce",
    "weak",
    "common",
    "appending",
    "extern_weak",
    "linkonce_odr",
    "weak_odr",
    "external",
];

fn linkage_of(head: &str) -> Linkage {
    for w in head.split_whitespace() {
        match w {
            "internal" | "private" => return Linkage::Local,
            "weak_odr" | "linkonce_odr" | "weak" | "linkonce" | "common" => {
                return Linkage::Mergeable
            }
            _ => {}
        }
    }
    Linkage::External
}

/// `define <linkage> <attrs> <ret> @name(<params>) <fn attrs> {` →
/// `declare <attrs> <ret> @name(<param types and attrs>)`.
///
/// Derived from the definition's own header so the two cannot disagree — the
/// same reason a body-less `fn` in a header reuses the definition's signature
/// emission. Only what a declaration may carry survives: linkage goes, the
/// function attributes and everything after the parameter list go, and the
/// parameter names go because they name nothing in a declaration.
pub fn declare_from_define(header: &str) -> Option<String> {
    let rest = header.strip_prefix("define ")?;
    let at = rest.find('@')?;
    let head: Vec<&str> = rest[..at]
        .split_whitespace()
        .filter(|t| !LINKAGES.contains(t))
        .collect();
    let tail = &rest[at..];
    let open = tail.find('(')?;
    let mut depth = 0usize;
    let mut close = None;
    for (i, b) in tail.bytes().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let name = &tail[..open];
    let params = strip_param_names(&tail[open + 1..close]);
    Some(format!("declare {} {}({})", head.join(" "), name, params))
}

/// Drop the `%name` each parameter ends with, keeping its type and attributes.
fn strip_param_names(params: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut depth = 0usize;
    let mut last = 0usize;
    for (i, b) in params.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&params[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&params[last..]);
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| match p.rsplit_once(' ') {
            Some((rest, name)) if name.starts_with('%') => rest.trim_end().to_string(),
            _ => p.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first `@symbol` in `s`, if any. Same character class `prune.rs` uses.
fn first_symbol(s: &str) -> Option<&str> {
    let at = s.find('@')?;
    let rest = &s[at + 1..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
        .unwrap_or(rest.len());
    if end == 0 {
        None
    } else {
        Some(&rest[..end])
    }
}

/// Every `@symbol` occurring in `s`.
fn symbol_refs(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let rest = &s[i + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
                .unwrap_or(rest.len());
            if end > 0 {
                out.push(&rest[..end]);
                i += 1 + end;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece<'a>(s: &'a Split, module: &str) -> &'a str {
        &s.pieces
            .iter()
            .find(|p| p.module == module)
            .unwrap_or_else(|| panic!("no piece `{module}`; have {:?}", s.pieces.iter().map(|p| &p.module).collect::<Vec<_>>()))
            .ir
    }

    // Two modules. `a.f` calls `b.g` (a root of b), an instance `b.inst__i32`,
    // and through it b's internal helper; both use a private constant.
    const TWO: &str = "\
%pkg.src.b.T = type { i32 }
declare ptr @malloc(i64)
@.str.0 = private unnamed_addr constant [3 x i8] c\"hi\\00\", align 1
@pkg.src.b.COUNT = weak_odr global i32 0, align 4
@pkg.src.b._SEEN = internal global i32 0, align 4
; cpc-home: pkg.src.a
define weak_odr i32 @pkg.src.a.f(i32 noundef %0) \"frame-pointer\"=\"non-leaf\" {
entry:
  %1 = call i32 @pkg.src.b.g(i32 %0)
  %2 = call i32 @pkg.src.b.inst__i32(ptr @.str.0)
  ret i32 %1
}
; cpc-home: pkg.src.b
define weak_odr i32 @pkg.src.b.g(i32 noundef %0) \"frame-pointer\"=\"non-leaf\" {
entry:
  %1 = load i32, ptr @pkg.src.b.COUNT
  ret i32 %1
}
define weak_odr i32 @pkg.src.b.inst__i32(ptr %0) \"frame-pointer\"=\"non-leaf\" {
entry:
  %1 = call i32 @pkg.src.b._h()
  ret i32 %1
}
define internal i32 @pkg.src.b._h() \"frame-pointer\"=\"non-leaf\" {
entry:
  %1 = load i32, ptr @pkg.src.b._SEEN
  ret i32 %1
}
define weak_odr i32 @pkg.src.b.unused() \"frame-pointer\"=\"non-leaf\" {
entry:
  ret i32 0
}
; cpc-home: -
";

    #[test]
    fn a_root_is_defined_at_home_and_declared_where_referenced() {
        let s = split_by_home(TWO, "pkg");
        let a = piece(&s, "pkg.src.a");
        let b = piece(&s, "pkg.src.b");
        assert!(a.contains("define weak_odr i32 @pkg.src.a.f("), "{a}");
        assert!(!a.contains("define weak_odr i32 @pkg.src.b.g("), "{a}");
        assert!(a.contains("declare i32 @pkg.src.b.g(i32 noundef)\n"), "{a}");
        assert!(b.contains("define weak_odr i32 @pkg.src.b.g("), "{b}");
        assert!(!b.contains("@pkg.src.a.f"), "{b}");
        assert!(!b.contains("declare i32 @pkg.src.b.g"), "b defines g, it must not declare it too: {b}");
    }

    #[test]
    fn an_instance_and_what_it_reaches_are_copied_not_declared() {
        let s = split_by_home(TWO, "pkg");
        let a = piece(&s, "pkg.src.a");
        assert!(a.contains("define weak_odr i32 @pkg.src.b.inst__i32("), "{a}");
        assert!(a.contains("define internal i32 @pkg.src.b._h("), "{a}");
        assert!(a.contains("@.str.0 = private"), "{a}");
        assert!(!a.contains("declare i32 @pkg.src.b.inst__i32"), "{a}");
        // Nothing in `b`'s own roots reaches the instance, so `b` does not
        // carry it: an instance lives where it is used.
        let b = piece(&s, "pkg.src.b");
        assert!(!b.contains("@pkg.src.b.inst__i32"), "{b}");
        assert!(b.contains("@pkg.src.b.unused"), "a root nothing references is still its module's: {b}");
    }

    #[test]
    fn shared_lines_reach_every_piece_and_markers_none() {
        let s = split_by_home(TWO, "pkg");
        for p in &s.pieces {
            assert!(p.ir.contains("%pkg.src.b.T = type { i32 }"), "{}", p.module);
            assert!(p.ir.contains("declare ptr @malloc(i64)"), "{}", p.module);
            assert!(!p.ir.contains(HOME_MARKER), "{}", p.module);
        }
        assert_eq!(s.pieces.len(), 2, "no residual piece when nothing is homeless");
    }

    #[test]
    fn a_global_goes_where_it_is_referenced() {
        let s = split_by_home(TWO, "pkg");
        let a = piece(&s, "pkg.src.a");
        let b = piece(&s, "pkg.src.b");
        assert!(b.contains("@pkg.src.b.COUNT = weak_odr global"), "{b}");
        assert!(!a.contains("@pkg.src.b.COUNT ="), "a never names COUNT: {a}");
        // `_SEEN` is reached only through the instance, which only `a` uses —
        // one landing, so nothing to report.
        assert!(a.contains("@pkg.src.b._SEEN = internal global"), "{a}");
        assert!(!b.contains("@pkg.src.b._SEEN ="), "{b}");
        assert!(s.notes.is_empty(), "{:?}", s.notes);
    }

    #[test]
    fn an_internal_mutable_global_in_two_pieces_is_reported() {
        let ir = "\
@pkg.src.b._SEEN = internal global i32 0, align 4
; cpc-home: pkg.src.a
define weak_odr i32 @pkg.src.a.f() {
entry:
  %1 = call i32 @pkg.src.b.peek__i32()
  ret i32 %1
}
; cpc-home: pkg.src.b
define weak_odr i32 @pkg.src.b.g() {
entry:
  %1 = call i32 @pkg.src.b.peek__i32()
  ret i32 %1
}
define weak_odr i32 @pkg.src.b.peek__i32() {
entry:
  %1 = load i32, ptr @pkg.src.b._SEEN
  ret i32 %1
}
";
        let s = split_by_home(ir, "pkg");
        assert_eq!(s.notes.len(), 1, "{:?}", s.notes);
        assert!(s.notes[0].contains("@pkg.src.b._SEEN"), "{}", s.notes[0]);
        assert!(s.notes[0].contains("pkg.src.a") && s.notes[0].contains("pkg.src.b"), "{}", s.notes[0]);
    }

    #[test]
    fn an_export_is_a_root_whatever_its_name_and_a_homeless_root_is_residual() {
        // A JNI entry point's bare C name carries no module; the marker does.
        // A `__`-named EXTERNAL definition is an export, not an instance.
        // A homeless external definition lands in the residual piece.
        let ir = "\
; cpc-home: app.src.entry
define void @Java_cplus_facet_FacetActivity_nativeCreateView(ptr %0) {
entry:
  ret void
}
define void @__cplus_thread_create_v1(ptr %0) {
entry:
  ret void
}
; cpc-home: -
define void @orphan_export() {
entry:
  ret void
}
define linkonce_odr hidden ptr @__cplus_cancel_get_slot() {
entry:
  ret ptr null
}
";
        let s = split_by_home(ir, "app");
        let entry = piece(&s, "app.src.entry");
        assert!(entry.contains("define void @Java_cplus_facet_FacetActivity_nativeCreateView("), "{entry}");
        assert!(entry.contains("define void @__cplus_thread_create_v1("), "{entry}");
        let res = piece(&s, "app");
        assert!(res.contains("define void @orphan_export("), "{res}");
        assert!(!res.contains("@Java_"), "{res}");
        // A linkonce helper nothing reaches is not a root anywhere.
        for p in &s.pieces {
            assert!(!p.ir.contains("@__cplus_cancel_get_slot"), "{}", p.module);
        }
    }

    #[test]
    fn a_package_with_no_roots_still_yields_one_piece() {
        let s = split_by_home("%t = type { i32 }\ndeclare void @f()\n", "pkg");
        assert_eq!(s.pieces.len(), 1);
        assert_eq!(s.pieces[0].module, "pkg");
        assert!(s.pieces[0].ir.contains("declare void @f()"));
    }

    #[test]
    fn module_asm_is_assembled_once_in_its_home() {
        let ir = "\
; cpc-home: pkg.src.a
module asm \".globl marker\"
define weak_odr void @pkg.src.a.f() {
entry:
  ret void
}
; cpc-home: pkg.src.b
define weak_odr void @pkg.src.b.g() {
entry:
  ret void
}
";
        let s = split_by_home(ir, "pkg");
        assert!(piece(&s, "pkg.src.a").contains("module asm"));
        assert!(!piece(&s, "pkg.src.b").contains("module asm"));
    }

    #[test]
    fn a_declaration_keeps_the_signature_and_drops_the_rest() {
        let def = "define weak_odr void @stdlib.src.arc.new__i32(ptr sret(%enum.40) noalias nonnull noundef writable dereferenceable(16) align 8 %0, i32 noundef %1) \"frame-pointer\"=\"non-leaf\" {";
        assert_eq!(
            declare_from_define(def).as_deref(),
            Some("declare void @stdlib.src.arc.new__i32(ptr sret(%enum.40) noalias nonnull noundef writable dereferenceable(16) align 8, i32 noundef)")
        );
        let ext = "define zeroext i8 @poll(ptr %fds, i32 %nfds, i32 %timeout) \"frame-pointer\"=\"non-leaf\" cold {";
        assert_eq!(
            declare_from_define(ext).as_deref(),
            Some("declare zeroext i8 @poll(ptr, i32, i32)")
        );
        let none = "define %stdlib.src.future.Future__unit @__async_x() \"frame-pointer\"=\"non-leaf\" presplitcoroutine {";
        assert_eq!(
            declare_from_define(none).as_deref(),
            Some("declare %stdlib.src.future.Future__unit @__async_x()")
        );
        assert_eq!(declare_from_define("declare void @f()"), None);
    }

    #[test]
    fn a_shared_declare_is_not_declared_again_by_a_cut() {
        // `b.g` is defined by the IR AND declared in the preamble (an export
        // the preamble had already declared). The cut in `a` must not add a
        // second declaration.
        let ir = "\
declare i32 @pkg.src.b.g(i32)
; cpc-home: pkg.src.a
define weak_odr i32 @pkg.src.a.f(i32 %0) {
entry:
  %1 = call i32 @pkg.src.b.g(i32 %0)
  ret i32 %1
}
; cpc-home: pkg.src.b
define weak_odr i32 @pkg.src.b.g(i32 %0) {
entry:
  ret i32 %0
}
";
        let s = split_by_home(ir, "pkg");
        let a = piece(&s, "pkg.src.a");
        assert_eq!(a.matches("declare i32 @pkg.src.b.g(").count(), 1, "{a}");
    }
}
