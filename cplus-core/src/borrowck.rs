//! Phase 5 — borrow checker (slices 5BC.1, 5BC.2a, 5BC.2b).
//!
//! Design note: [`docs/compiler/design/phase5-borrow-shared.md`](../../docs/compiler/design/phase5-borrow-shared.md).
//!
//! ## What this module produces
//!
//! - `analyze(prog)` returns a `ProgramAnalysis` — per-function place-state
//!   snapshots, used by unit tests to assert on analyzer behavior.
//! - `check(prog, file, src)` returns a `Vec<Diagnostic>` and is the
//!   pipeline entry — wired into `cpc build` / `cpc check` / `cpc-lsp`
//!   after sema. Diagnostics emitted here render alongside sema's.
//!
//! ## What's active
//!
//! - **5BC.1**: place-state machinery — `Place { root, projections }`,
//!   four-variant `PlaceState`, snapshots at `entry` / `after stmt N` /
//!   `exit`, plus a stable `dump()` for snapshot tests.
//! - **5BC.2a**:
//!   - `CopyOracle` mirrors sema's struct + enum `Copy` fixpoint.
//!   - Per-binding type tracking — parameter types, annotated lets,
//!     `this` (impl target), for-range loop var (synthesized i32).
//!     Unannotated lets stay `Unknown`, skip Copy-gated diagnostics.
//!   - Owned → Moved transitions are Copy-gated; `take x: i32` is a
//!     bit-copy and leaves the source Owned.
//!   - **E0370** — move-and-shared-borrow of the same place in one call.
//! - **5BC.2b**:
//!   - **Flow-sensitive branch merging** at `if`/`else`, `match` arms,
//!     and loop bodies. State is snapshotted before each branch, each
//!     branch walked independently from the snapshot, then states
//!     intersected per-place at the join via `PlaceState::merge`. The
//!     `MaybePartial` state appears here for the first time. Branches
//!     that diverge (`return`/`break`/`continue` on every path) are
//!     excluded from the merge — the survivor's state carries forward.
//!   - Bindings introduced inside a branch are scope-local: they don't
//!     leak past the branch's closing brace.
//!   - Loop bodies merge with pre-loop state (the body might not run, so
//!     any move inside the body is conservatively `MaybePartial` after
//!     the loop).
//!   - **E0371** — use of possibly-moved binding (fires on a bare
//!     `ExprKind::Ident` read whose place is currently in `MaybePartial`
//!     state, when the binding is provably non-Copy). Today this rarely
//!     fires in practice because sema's linear E0335 is overly
//!     conservative on branched moves and rejects them first (the
//!     pipeline bails before borrowck runs). The machinery here is
//!     infrastructure for later precision — eventually replacing sema's
//!     E0335 with borrowck's flow-sensitive equivalent will surface the
//!     value. See plan.md §3 Phase 5 sequencing note.
//!
//! ## What's deferred
//!
//! - **5BC.3** / **5BC.4**: return-borrow tracking + lifetime elision
//!   (single-param E1, self-method E2, multi-param E3) → E0372 / E0373.
//! - **5BC.5**: partial-place tracking through field / index projections
//!   → E0374.
//! - Method-call move detection (`x.consume()` where `consume` takes
//!   `take this`).
//! - Sema integration for fully-typed binding-type lookup.
//! - Replacing sema's linear E0335 with borrowck's flow-sensitive
//!   tracking (would let E0371 actually fire in user-visible cases).

use crate::ast::*;
use crate::diagnostics::{Applicability, DiagCode, Diagnostic, LineMap, Severity, Suggestion};
use crate::lexer::Span;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Place / PlaceState (5BC.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Place {
    pub root: String,
    pub projections: Vec<Projection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Projection {
    Field(String),
    Index(u64),
    AnyIndex,
}

impl Place {
    pub fn root(name: impl Into<String>) -> Self {
        Place {
            root: name.into(),
            projections: Vec::new(),
        }
    }

    pub fn canonical(&self) -> String {
        let mut s = self.root.clone();
        for p in &self.projections {
            match p {
                Projection::Field(f) => {
                    s.push('.');
                    s.push_str(f);
                }
                Projection::Index(n) => {
                    s.push('[');
                    s.push_str(&n.to_string());
                    s.push(']');
                }
                Projection::AnyIndex => s.push_str("[*]"),
            }
        }
        s
    }

    /// Slice 6BC.3: how two places overlap. `Disjoint` covers both
    /// "different roots" and "same root, divergent projections"
    /// (`buf.left` vs `buf.right`). `Same` means the canonical paths
    /// match exactly. `Contains` means `self.projections` is a strict
    /// prefix of `other.projections` (so `self` is the *larger* place
    /// — a borrow of `self` includes `other`); `Contained` is the
    /// inverse.
    pub fn overlap(&self, other: &Place) -> PlaceOverlap {
        if self.root != other.root {
            return PlaceOverlap::Disjoint;
        }
        let a = &self.projections;
        let b = &other.projections;
        if a == b {
            return PlaceOverlap::Same;
        }
        if a.len() < b.len() && b.starts_with(a) {
            return PlaceOverlap::Contains;
        }
        if b.len() < a.len() && a.starts_with(b) {
            return PlaceOverlap::Contained;
        }
        PlaceOverlap::Disjoint
    }

    /// True iff this place and `other` are aliasing — Same / Contains /
    /// Contained all conflict. Convenience over `overlap`.
    pub fn conflicts_with(&self, other: &Place) -> bool {
        !matches!(self.overlap(other), PlaceOverlap::Disjoint)
    }
}

/// Slice 6BC.3: the result of comparing two places. Used by the borrow
/// checker to decide which conflict diagnostic fires when a sibling
/// argument claims a place that overlaps another claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceOverlap {
    /// Same canonical path — exact aliasing.
    Same,
    /// `self.projections` is a strict prefix of `other.projections`.
    /// `self` is the *larger* (parent) borrow; `other` is a sub-place.
    Contains,
    /// `other.projections` is a strict prefix of `self.projections`.
    /// `other` is the parent; `self` is the sub-place.
    Contained,
    /// No aliasing — different roots, or same root but divergent
    /// projections (e.g. `buf.left` vs `buf.right`).
    Disjoint,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaceState {
    Owned,
    BorrowedShared(u32),
    /// Slice 6BC.1: exactly one exclusive borrower. Conflicts with every
    /// other access (reads, writes, moves, shared borrows, additional
    /// exclusive borrows). The payload names the borrowing binding so the
    /// diagnostic can point at it; cross-statement tracking (5BC.2) wires
    /// this into `let r = f(ref x);` in a future slice.
    BorrowedExclusive(String),
    Moved,
    MaybePartial,
}

impl PlaceState {
    pub fn merge(&self, other: &PlaceState) -> PlaceState {
        use PlaceState::*;
        match (self, other) {
            (Owned, Owned) => Owned,
            (Moved, Moved) => Moved,
            (Owned, Moved) | (Moved, Owned) => MaybePartial,
            (MaybePartial, _) | (_, MaybePartial) => MaybePartial,
            (BorrowedShared(a), BorrowedShared(b)) => BorrowedShared(*a.max(b)),
            // Slice 6BC.1: exclusive-borrow merge rules per design note §5.1.
            // Same borrower on both branches → still exclusive. Different
            // borrowers, or exclusive on one branch and anything-else on
            // the other → MaybePartial (callers fire E0371 on reads).
            (BorrowedExclusive(a), BorrowedExclusive(b)) if a == b => BorrowedExclusive(a.clone()),
            (BorrowedExclusive(_), BorrowedExclusive(_)) => MaybePartial,
            (BorrowedExclusive(_), _) | (_, BorrowedExclusive(_)) => MaybePartial,
            (BorrowedShared(n), _) | (_, BorrowedShared(n)) => BorrowedShared(*n),
        }
    }
}

// ---------------------------------------------------------------------------
// ProgramAnalysis dump shape (5BC.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProgramAnalysis {
    pub functions: BTreeMap<String, FunctionAnalysis>,
}

#[derive(Debug, Clone)]
pub struct FunctionAnalysis {
    pub name: String,
    pub points: Vec<PointSnapshot>,
}

#[derive(Debug, Clone)]
pub struct PointSnapshot {
    pub label: String,
    pub state: BTreeMap<Place, PlaceState>,
}

impl ProgramAnalysis {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (name, fa) in &self.functions {
            out.push_str(&format!("fn {name}:\n"));
            for p in &fa.points {
                out.push_str(&format!("  {}: ", p.label));
                if p.state.is_empty() {
                    out.push_str("{}\n");
                } else {
                    out.push('{');
                    let mut first = true;
                    for (pl, st) in &p.state {
                        if !first {
                            out.push_str(", ");
                        }
                        first = false;
                        out.push_str(&pl.canonical());
                        out.push('=');
                        out.push_str(&fmt_state(st));
                    }
                    out.push_str("}\n");
                }
            }
        }
        out
    }
}

fn fmt_state(s: &PlaceState) -> String {
    match s {
        PlaceState::Owned => "Owned".to_string(),
        PlaceState::BorrowedShared(n) => format!("BorrowedShared({n})"),
        PlaceState::BorrowedExclusive(name) => format!("BorrowedExclusive({name})"),
        PlaceState::Moved => "Moved".to_string(),
        PlaceState::MaybePartial => "MaybePartial".to_string(),
    }
}

// ---------------------------------------------------------------------------
// CopyOracle (5BC.2a)
//
// Mirror of sema's struct + enum Copy fixpoint, computed from the AST
// directly so borrowck can run independently of sema. The two computations
// must agree — if they ever drift, borrowck's diagnostics will not match
// sema's view of what's Copy. A regression test pins the alignment on the
// in-tree samples.
// ---------------------------------------------------------------------------

/// `is_copy` for every user-defined type, keyed by the type's bare name
/// as it appears in the AST. Multi-file projects use the resolver-merged
/// qualified name (e.g. `src.math.Point`); single-file mode uses plain
/// names (e.g. `Point`). Built-in primitives are not stored here — see
/// `is_primitive_copy`.
#[derive(Debug, Default, Clone)]
pub struct CopyOracle {
    types: HashMap<String, TypeInfo>,
    /// STRM v3 (2026-08-01): names of struct/enum types that transitively
    /// CONTAIN a view (`str` / `T[]` field or payload). Drives the widened
    /// Rule E-VIEW: a fn returning such a type from view-typed / view-
    /// carrying params ties its result like a bare view return. Raw
    /// pointers are deliberately not counted (container elements behind
    /// heap indirection stay outside this analysis).
    view_carrying: std::collections::HashSet<String>,
    /// Generic-impl ties (2026-08-01): per generic struct/enum, whether each
    /// generic param appears in a STORED position (a field / payload type,
    /// transitively through Generic args, slices, arrays). A param that only
    /// parameterizes fn-pointer signatures or raw pointers is not stored —
    /// `SignalSubscription[Change]` holds no `Change`, so instantiating it
    /// with a view-carrier must not make it a carrier.
    generic_param_stored: HashMap<String, Vec<bool>>,
}

#[derive(Debug, Clone)]
struct TypeInfo {
    is_copy: bool,
    /// Set during construction. Drop structs are always non-Copy
    /// regardless of field types (mirrors sema rule from §3F).
    is_drop: bool,
}

impl CopyOracle {
    /// True iff `name` appears in a STORED position of `ty`: the type
    /// itself, a Generic argument, a slice/array element, or the pointee of
    /// an OWNING raw pointer. Fn-pointer signatures are not storage — a
    /// `fn(T, *u8)` field holds no `T`.
    ///
    /// A raw pointer is storage or not depending on who owns the pointee,
    /// and SPEC §6.6 already forces every author to say which: a
    /// raw-pointer field is either released in the type's `Drop` (the type
    /// owns what it points at) or marked `opaque` ("this pointer is not
    /// owned here"), and E0510 rejects a field that answers neither. So the
    /// answer is always on the declaration; the caller skips `opaque`
    /// fields and everything else reaching here is owned storage.
    ///
    /// This used to return false for every raw pointer, citing §6.6 — which
    /// is the section that draws the distinction rather than erasing it.
    /// The cost was that `Box[T] { _p: *T }` and `Vec[T] { _ptr: *T }`, the
    /// two types that hold a `T` on the heap, both read as storing nothing:
    /// `Box[Sink]` and `Vec[str]` were classified as carrying no view at
    /// all, so a view moved into either one lost its provenance and every
    /// later rule skipped it.
    /// (`bugs/str-field-outliving-its-text-is-not-caught.md`, round three.)
    ///
    /// Nesting is transitive through `stored`, not textual. `Signal[T] {
    /// _subs: Vec[Listener[T]] }` MENTIONS `T`, but `Listener[T]` holds it
    /// only in a `fn(T, *u8)` signature, so no `T` is stored anywhere and
    /// `Signal[Change]` carries no view. Asking the inner type whether it
    /// stores its own parameter is what keeps that true; a purely syntactic
    /// mention would make every signal of a view-carrying event look like a
    /// carrier. An unknown base answers "stored", the sound direction.
    fn type_mentions_param(
        ty: &Type,
        name: &str,
        stored: &HashMap<String, Vec<bool>>,
    ) -> bool {
        match &ty.kind {
            TypeKind::Path(p) => p == name,
            TypeKind::Generic { name: base, args } => args.iter().enumerate().any(|(i, a)| {
                let inner_stores = stored
                    .get(base)
                    .map_or(true, |s| s.get(i).copied().unwrap_or(true));
                inner_stores && Self::type_mentions_param(a, name, stored)
            }),
            TypeKind::Slice(inner) => Self::type_mentions_param(inner, name, stored),
            TypeKind::Array { elem, .. } => Self::type_mentions_param(elem, name, stored),
            TypeKind::RawPtr(inner) => Self::type_mentions_param(inner, name, stored),
            _ => false,
        }
    }

    pub fn build(prog: &Program) -> Self {
        let mut oracle = CopyOracle::default();

        // Pass 1: register every user-defined type. Initial is_copy =
        // true for the fixpoint's lattice; the iteration only ever
        // monotonically flips trues to falses (Copy is structural — a
        // type is non-Copy as soon as any component is non-Copy).
        for item in &prog.items {
            match &item.kind {
                ItemKind::Struct(s) => {
                    oracle.types.insert(
                        s.name.name.clone(),
                        TypeInfo {
                            is_copy: true,
                            is_drop: false,
                        },
                    );
                }
                ItemKind::Enum(e) => {
                    oracle.types.insert(
                        e.name.name.clone(),
                        TypeInfo {
                            is_copy: true,
                            is_drop: false,
                        },
                    );
                }
                _ => {}
            }
        }

        // Generic-impl ties: record which generic params each generic
        // struct/enum actually STORES, so `S[str]` only counts as a
        // carrier when `S` can hold the argument.
        //
        // A fixpoint, because storing is transitive and mutually recursive:
        // whether `Signal[T]` stores a `T` depends on whether the
        // `Listener[T]` inside its `Vec` does. Seeded all-false and grown
        // upward — "stored" only ever turns on — so this converges, and a
        // type is never called a carrier before the evidence arrives.
        for item in &prog.items {
            let (name, arity) = match &item.kind {
                ItemKind::Struct(s) if !s.generic_params.is_empty() => {
                    (&s.name.name, s.generic_params.len())
                }
                ItemKind::Enum(e) if !e.generic_params.is_empty() => {
                    (&e.name.name, e.generic_params.len())
                }
                _ => continue,
            };
            oracle
                .generic_param_stored
                .insert(name.clone(), vec![false; arity]);
        }
        loop {
            let mut changed = false;
            for item in &prog.items {
                // (param name, does some field/payload store it) per param.
                let found: Vec<(String, Vec<bool>)> = match &item.kind {
                    ItemKind::Struct(s) if !s.generic_params.is_empty() => vec![(
                        s.name.name.clone(),
                        s.generic_params
                            .iter()
                            .map(|g| {
                                // `opaque` is the author's declaration that
                                // this pointer's target is owned elsewhere
                                // (SPEC §6.6), so nothing is stored through it.
                                s.fields.iter().any(|f| {
                                    !f.is_opaque
                                        && Self::type_mentions_param(
                                            &f.ty,
                                            &g.name.name,
                                            &oracle.generic_param_stored,
                                        )
                                })
                            })
                            .collect(),
                    )],
                    ItemKind::Enum(e) if !e.generic_params.is_empty() => vec![(
                        e.name.name.clone(),
                        e.generic_params
                            .iter()
                            .map(|g| {
                                e.variants.iter().any(|v| {
                                    v.payload.iter().any(|t| {
                                        Self::type_mentions_param(
                                            t,
                                            &g.name.name,
                                            &oracle.generic_param_stored,
                                        )
                                    })
                                })
                            })
                            .collect(),
                    )],
                    _ => continue,
                };
                for (name, fresh) in found {
                    let cur = oracle.generic_param_stored.entry(name).or_default();
                    for (i, f) in fresh.into_iter().enumerate() {
                        if f && !cur[i] {
                            cur[i] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // Pass 2: detect Drop structs (any `impl` block with a `drop`
        // method). Sets is_drop = true and is_copy = false unconditionally.
        for item in &prog.items {
            if let ItemKind::Impl(b) = &item.kind {
                if b.methods.iter().any(|m| m.name.name == "drop") {
                    if let Some(info) = oracle.types.get_mut(&b.target.name) {
                        info.is_drop = true;
                        info.is_copy = false;
                    }
                }
            }
        }

        // Pass 3: fixpoint over structs and tagged enums. A struct's
        // Copy-ness depends on all field types; an enum's depends on all
        // payload types. Plain enums (no payloads) stay Copy.
        loop {
            let mut changed = false;
            for item in &prog.items {
                match &item.kind {
                    ItemKind::Struct(s) => {
                        let info = oracle.types.get(&s.name.name).cloned();
                        let Some(info) = info else { continue };
                        let all_copy = info.is_copy
                            && s.fields.iter().all(|f| oracle.is_type_copy_internal(&f.ty));
                        if !all_copy && info.is_copy {
                            oracle.types.get_mut(&s.name.name).unwrap().is_copy = false;
                            changed = true;
                        }
                        // A destructor reached through a FIELD runs at this
                        // type's scope exit just as an own `drop` would, and
                        // `has_destructor` is asked "can a loan this value
                        // holds still be observed after its last mention"
                        // — for which the reachable destructor is what
                        // matters, not who declared it.
                        if !info.is_drop && s.fields.iter().any(|f| oracle.type_has_drop(&f.ty)) {
                            oracle.types.get_mut(&s.name.name).unwrap().is_drop = true;
                            changed = true;
                        }
                    }
                    ItemKind::Enum(e) => {
                        let info = oracle.types.get(&e.name.name).cloned();
                        let Some(info) = info else { continue };
                        let all_copy = info.is_copy
                            && e.variants
                            .iter()
                            .all(|v| v.payload.iter().all(|t| oracle.is_type_copy_internal(t)));
                        if !all_copy && info.is_copy {
                            oracle.types.get_mut(&e.name.name).unwrap().is_copy = false;
                            changed = true;
                        }
                        if !info.is_drop
                            && e.variants
                                .iter()
                                .any(|v| v.payload.iter().any(|t| oracle.type_has_drop(t)))
                        {
                            oracle.types.get_mut(&e.name.name).unwrap().is_drop = true;
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }

        // STRM v3 pass 4: fixpoint for view-carrying type names. Monotone
        // upward: a name joins the set when any field / payload contains a
        // view, directly or through an already-joined name.
        loop {
            let mut changed = false;
            for item in &prog.items {
                match &item.kind {
                    ItemKind::Struct(s) => {
                        if oracle.view_carrying.contains(&s.name.name) {
                            continue;
                        }
                        if s.fields.iter().any(|f| oracle.type_contains_view(&f.ty)) {
                            oracle.view_carrying.insert(s.name.name.clone());
                            changed = true;
                        }
                    }
                    ItemKind::Enum(e) => {
                        if oracle.view_carrying.contains(&e.name.name) {
                            continue;
                        }
                        if e.variants
                            .iter()
                            .any(|v| v.payload.iter().any(|t| oracle.type_contains_view(t)))
                        {
                            oracle.view_carrying.insert(e.name.name.clone());
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }

        oracle
    }

    /// STRM v3: true iff `ty` IS a view (`str` / slice) or names a type in
    /// the view-carrying set. AST-level twin of sema's `ty_contains_view`;
    /// the two must agree.
    pub fn type_contains_view(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Slice(_) => true,
            TypeKind::Path(name) => name == "str" || self.view_carrying.contains(name),
            TypeKind::Array { elem, .. } => self.type_contains_view(elem),
            // Contract §5 / generic-impl ties (2026-08-01): an instantiated
            // generic carries a view when its base is a known carrier, or a
            // view-carrying ARGUMENT lands in a stored position of the base
            // (`Option[str]` stores its payload; `SignalSubscription[Change]`
            // never stores a `Change` — its param only parameterizes
            // fn-pointer signatures — so it stays clean). Unknown bases fall
            // back to any-arg, the sound direction.
            TypeKind::Generic { name, args } => {
                if self.view_carrying.contains(name) {
                    return true;
                }
                match self.generic_param_stored.get(name) {
                    Some(stored) => args.iter().enumerate().any(|(i, a)| {
                        stored.get(i).copied().unwrap_or(true) && self.type_contains_view(a)
                    }),
                    None => args.iter().any(|a| self.type_contains_view(a)),
                }
            }
            _ => false,
        }
    }

    /// True iff `ty` is provably Copy. Returns `false` if the type is
    /// unknown (e.g. an undeclared type-name); the caller should treat
    /// `false` as "may be non-Copy" and gate diagnostics accordingly —
    /// for E0370 we additionally require an explicit answer via
    /// `definitely_non_copy` to avoid firing on truly unknown types.
    pub fn is_copy(&self, ty: &Type) -> bool {
        self.is_type_copy_internal(ty)
    }

    /// True iff a value of `ty` runs a destructor when it goes out of
    /// scope — its own `drop`, or one reachable through a field or payload
    /// (the fixpoint in `new` propagates the flag upward).
    ///
    /// This is the DROP-LIVENESS oracle, not the Copy one. NLL ends a
    /// borrower's loans at its last textual mention, which is sound only if
    /// nothing can observe the loan afterwards — and a destructor runs after
    /// the last mention by construction. `thread::Scope::drop` joins the
    /// workers it lent to, so its loans have to survive to scope exit or the
    /// whole guarantee is a comment.
    pub fn type_has_drop(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Path(name) => self.types.get(name).map(|i| i.is_drop).unwrap_or(false),
            // A generic base with a destructor has one for every
            // instantiation (`Vec[i32]`, `Vec[str]`); the type args cannot
            // take it away.
            TypeKind::Generic { name, .. } => {
                self.types.get(name).map(|i| i.is_drop).unwrap_or(false)
            }
            TypeKind::Array { elem, .. } => self.type_has_drop(elem),
            // Views, raw pointers and fn pointers own nothing and drop
            // nothing; a tuple's shape is not resolved here, so stay
            // conservative-in-the-permissive-direction and let the existing
            // scope-exit release handle it.
            _ => false,
        }
    }

    /// True iff `ty` resolves to a user-defined type whose `is_copy`
    /// flag is *known to be false*. Returns `false` for primitives
    /// (which are Copy), Copy aggregates, and unknown / un-resolvable
    /// type names. This is the gate E0370 uses: emit only when we are
    /// *sure* the binding is non-Copy.
    pub fn definitely_non_copy(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Path(name) => {
                if is_primitive_name(name) {
                    return false;
                }
                self.types.get(name).map(|i| !i.is_copy).unwrap_or(false)
            }
            TypeKind::Array { elem, .. } => self.definitely_non_copy(elem),
            // Slice 6BC.5: region annotation is transparent for Copy
            // classification — a borrow-region type is Copy iff T is.
            // (The `borrow A T` source syntax is retired; this arm is now
            // unreachable from source but kept for the type's invariants.)
            // Slice 7GEN.5c: generic instantiation in type position.
            // Borrowck runs *before* monomorphize, so `Pair[i32, bool]`
            // still appears here. A generic base that is a **Drop** type is
            // unconditionally non-Copy regardless of its type args (the
            // destructor makes it non-Copy no matter what `T` is), so we can
            // answer definitively — this is what lets Rule E-VIEW / move
            // tracking fire for `Vec[i32]` (a `Vec` slice view must pin the
            // owner; moving/reallocating it under a live slice is a UAF).
            // For a **non-Drop** generic (`Pair[i32, bool]`) Copy-ness still
            // depends on the type args, which monomorphize resolves later, so
            // stay conservative (return false) to avoid a false non-Copy
            // verdict on what may be a Copy instantiation.
            TypeKind::Generic { name, .. } => {
                self.types.get(name).map(|i| i.is_drop).unwrap_or(false)
            }
            // Slice 10.FFI.1: raw pointers are Copy.
            TypeKind::RawPtr(_) => false,
            // Slice 11.FN_PTR: function pointers are Copy (atomic).
            TypeKind::FnPtr { .. } => false,
            // Phase 11 polish: slice type — fat-pointer view, Copy.
            TypeKind::Slice(_) => false,
            // v0.0.5 Phase 3 Slice 3B: tuple type. Same conservative
            // shape as `Generic` — the synthesized tuple struct's
            // Copy-ness depends on its element types; defer to sema's
            // computed flag after monomorphize lowers this to a Path.
            TypeKind::Tuple(_) => false,
        }
    }

    fn is_type_copy_internal(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Path(name) => {
                if is_primitive_name(name) {
                    return true;
                }
                self.types.get(name).map(|i| i.is_copy).unwrap_or(true)
            }
            TypeKind::Array { elem, .. } => self.is_type_copy_internal(elem),
            TypeKind::RawPtr(_) => true,
            // Slice 11.FN_PTR: function pointers are Copy.
            TypeKind::FnPtr { .. } => true,
            // Slice 7GEN.5c: conservative — assume non-Copy. Real Copy-ness
            // is determined after monomorphize substitutes args into the
            // template's fields.
            TypeKind::Generic { .. } => false,
            // Phase 11 polish: slice type — fat pointer, Copy.
            TypeKind::Slice(_) => true,
            // v0.0.5 Phase 3 Slice 3B: tuple type — same conservative
            // assumption as Generic until lowered to a Path.
            TypeKind::Tuple(_) => false,
        }
    }
}

fn is_primitive_name(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
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
            | "()"
    )
}

// ---------------------------------------------------------------------------
// Function-signature table (5BC.1, extended in 5BC.2a, 5BC.3a)
// ---------------------------------------------------------------------------

/// Source of a function's / method's return-borrow under the design
/// note's elision rules.
///
/// **Rule E1**: function with exactly one non-`Copy` shared-borrow param
/// (no `ref`, no `take`) and a non-`Copy` return type, where every
/// `return EXPR;` has EXPR rooted at that parameter. Records `Param(0)`.
///
/// **Rule E2**: method with a non-`take` non-`Copy` receiver (`this`)
/// and a non-`Copy` return, where every `return EXPR;` has EXPR rooted
/// at `this`. Records `SelfReceiver`.
///
/// **Rule E3** (5BC.4): function with 2+ non-`Copy` shared-borrow params
/// and a non-`Copy` return, where every `return EXPR;` is rooted at
/// *some* parameter (possibly different params on different paths).
/// Records `MultiParam(indices)` listing every parameter the return
/// could borrow from. The call-site treats the returned binding as
/// borrowing from *all* listed params simultaneously — moving any of
/// them while the return-binding is live fires E0372.
///
/// The recorded info is exposed via [`return_borrow_source`] /
/// [`method_return_borrow_source`] for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnBorrowSource {
    /// Return borrows from the parameter at index N (counting from 0;
    /// methods exclude the receiver from this count).
    Param(u32),
    /// Method return borrows from the receiver `this`.
    SelfReceiver,
    /// 5BC.4 / Rule E3: return borrows from one or more parameters; at
    /// the call site, the return-binding is treated as borrowing from
    /// *every* listed parameter (the union, not the choice). Indices
    /// are sorted ascending for canonical equality. Always has 2+
    /// entries — single-param cases collapse to `Param(N)`.
    MultiParam(Vec<u32>),
}

/// Slice 6BC.2: the flavor of a return-borrow — Shared (per Phase-5
/// Rules E1/E2/E3) or Exclusive (per 6BC.2 Rules E1-mut/E2-mut). The
/// caller's `let r = f(...);` binding holds a borrow of the indicated
/// source(s); the flavor decides whether the source's state becomes
/// `BorrowedShared(N)` or `BorrowedExclusive(r)`, and which diagnostic
/// codes fire on conflicting access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowFlavor {
    Shared,
    Exclusive,
}

/// Per-function signature info collected from the AST. Today this is
/// the `take`-flag list and (5BC.3a) the elision-rule return source. Future
/// slices will add types and lifetime info.
#[derive(Debug, Default)]
#[derive(Clone)]
struct FnEntry {
    /// Memory-model hardening (2026-07-06): the method receiver's claim
    /// kind (`this` → Shared, `ref this` → Exclusive, `take this` → Move).
    /// `None` for free fns and receiver-less methods. Lets the intra-call
    /// conflict walk include the receiver — `h.poke(h)` is the same
    /// aliasing bug as `poke2(h, h)` and must fire the same codes.
    receiver_claim: Option<ClaimKind>,
    param_moves: Vec<bool>,
    /// Slice 6BC.1: per-parameter `ref` flag, parallel to `param_moves`.
    /// `param_muts[i]` is true iff parameter i was declared `ref x: T`.
    /// Drives E0380/E0381/E0382 intra-call conflict detection in
    /// `apply_call`.
    param_muts: Vec<bool>,
    return_borrow: Option<ReturnBorrowSource>,
    /// Slice 6BC.2: when `return_borrow` is set, this records whether the
    /// caller's binding holds a shared or exclusive borrow of the
    /// source. `None` when `return_borrow` is also `None`. Defaults to
    /// `Shared` so the Default-derive stays sound for entries that lack
    /// elision info entirely.
    return_borrow_flavor: Option<BorrowFlavor>,
    /// Memory-model contract §5: declared `#[keeps(this)]` — view
    /// arguments survive inside the receiver. `apply_call` makes the
    /// receiver a live borrower of every view argument's owner.
    keeps_this: bool,
    /// Parallel to `param_moves`: true iff the parameter's declared type
    /// is a view or transitively carries one. Selects which arguments a
    /// `keeps(this)` call ties.
    param_view_flags: Vec<bool>,
    /// Memory-model contract §5, COMPUTED half: per-param may-flow-into-
    /// receiver bits derived from the method body (direct `this.f = arg`
    /// stores are already denied by E0515 unless declared, so in practice
    /// these mark TRANSITIVE flows: a wrapper forwarding its param to a
    /// `keeps` callee on `this`). Same call-site tie as the declared form.
    /// Empty for free fns and for methods the flow pass skips (generic
    /// impls, declarations).
    computed_keeps: Vec<bool>,
    /// True iff the return type is a view, carries a view, or is non-Copy
    /// — i.e. a call result that can transport its arguments' taint.
    /// Drives taint-through-calls in the receiver-flow pass.
    ret_view: bool,
    /// Computed free-fn flows: (src param, dst ref-param) pairs — a view
    /// param that may end up stored inside a `ref` param's target (via a
    /// keeps-method call on it or a direct field store). Callers tie the
    /// dst argument's root to the src argument's owners.
    computed_ref_flows: Vec<(usize, usize)>,
    /// Erased-boundary transport (2026-08-04): per-param bits that MAY
    /// flow into the return value, computed by the same body taint walk
    /// that feeds `computed_keeps`. `None` means the flow pass never
    /// analyzes this fn (generic, extern, declaration) — call sites fall
    /// back to the conservative `ret_view` transport. `Some(0)` means
    /// analyzed and returning none of its params — the precision that
    /// keeps copying constructors (`text::from_str`) from tying their
    /// callers when their return taint is consulted.
    computed_ret_flow: Option<u64>,
    /// The receiver half of `computed_ret_flow`: does the receiver's
    /// taint reach the return value (`Box[T].into_raw` returning
    /// `this._p`)? Same `None`/`Some` analyzability contract.
    computed_ret_from_receiver: Option<bool>,
    /// Declared (UNsubstituted) return type — feeds unannotated-let
    /// inference (`let h = mk();` resolves h's methods through mk's
    /// declared return).
    ret_ty: Option<Type>,
    /// Declared (UNsubstituted) parameter types. For methods of generic
    /// impls the tie machinery substitutes the receiver's type arguments
    /// into these at the call site before asking view-ness — `Vec[str]`'s
    /// `take item: T` becomes `str` (tie), `Vec[Text]`'s becomes `Text`
    /// (a real move, no tie).
    param_tys: Vec<Type>,
}

#[derive(Debug, Default)]
struct SigTable {
    fns: HashMap<String, FnEntry>,
    /// Methods keyed by `Type.method` (codegen dot-mangling form).
    methods: HashMap<String, FnEntry>,
    /// Rule E-VIEW aggregate/receiver arms (2026-07-07): declared field types
    /// per struct, so a field-path method receiver (`holder.field.view()`)
    /// can be typed and its `Type.method` entry looked up. Only
    /// `TypeKind::Path` steps are ever followed; generic instantiations stay
    /// conservative (no borrow recorded), matching the pre-mono posture of
    /// the rest of this pass.
    struct_fields: HashMap<String, HashMap<String, Type>>,
    /// Enum names, so `Enum::Variant(args)` calls are recognized as payload
    /// aggregates (views in the payload borrow like any aggregate capture)
    /// rather than associated-fn calls.
    enums: std::collections::HashSet<String>,
    /// Enum payload types: enum name -> variant name -> positional payload
    /// types (unsubstituted). Lets match-arm payload bindings get inferred
    /// types so a payload-bound receiver still resolves its methods.
    enum_payloads: HashMap<String, HashMap<String, Vec<Type>>>,
    /// Generic-enum param names, parallel to `impl_generics` for enums.
    enum_generics: HashMap<String, Vec<String>>,
    /// Generic-impl targets: target name -> ordered generic-param names
    /// (`impl Vec[T]` records `Vec -> [T]`). Call sites zip these with the
    /// receiver's type arguments to substitute before view classification.
    impl_generics: HashMap<String, Vec<String>>,
    /// TEXT→STR fallthrough (2026-08-13): the `#[lang("string")]` struct's
    /// name (`Text`). Sema dispatches a read method missing from the owned
    /// string to the blessed `impl str` set through the borrowing coercion;
    /// `method_entry` mirrors that here so `t.trim()` still classifies as a
    /// view OF `t` — without the mirror, every str-routed read would be an
    /// untracked borrow (the realloc-UAF class the view audit closed).
    lang_string: Option<String>,
}

/// v0.0.24 #9: the borrowck mirror of codegen's `effective_move` — does passing
/// this parameter by value *consume* the argument? Only a `take` parameter
/// consumes: a bare `x: T` is a read-only borrow (the caller keeps and drops
/// it), and `ref` is a write-back borrow — neither moves. This mirrors codegen
/// `effective_move` (`p.move_ && non-Copy aggregate`).
///
/// A `take` parameter whose type NAMES ONE OF THE CALLEE'S OWN GENERIC PARAMS
/// (`take x: T`, `take x: Option[T]`) is a move too — a *conditional* one,
/// settled at the call site by the argument's own type, which is always
/// concrete there. Skipping it was a soundness hole, not conservatism:
///
/// ```text
/// fn nsink(take x: Text)   -> E0372 when `x` is borrowed
/// fn gsink[T](take x: T)   -> silence, and the borrow outlives the value
/// ```
///
/// The permissive direction is what made it dangerous. "Unresolved, so assume
/// it does not move" admits a real move; the analysis has to assume it MIGHT.
/// `thread::spawn_with[I, O](take input: I, ...)` is on the silent side of
/// that line, so the standard library's threading entry point would move a
/// `Text` out from under a live `str` view of it — a use-after-free the
/// checker was built to reject and reported clean (2026-08-28).
///
/// Callers must pair this with the argument-side gate in the call walker:
/// `T` may instantiate to a Copy type, and moving a Copy value invalidates
/// nothing. Whether it consumes is the ARGUMENT's business, and the caller
/// knows the argument's type whether or not the callee is generic.
fn param_is_effective_move(
    p: &crate::ast::Param,
    oracle: &CopyOracle,
    generic_names: &[String],
) -> bool {
    // `Path` and `Generic` (an instantiation like `Vec[i32]`) both reach
    // `definitely_non_copy`, which now answers definitively for Drop generic
    // bases — so passing a `take v: Vec[T]` consumes it, and a move-while-view
    // is caught. Non-Drop generics still answer `false` there, which is where
    // the generic-param arm below takes over.
    p.move_
        && matches!(&p.ty.kind, TypeKind::Path(_) | TypeKind::Generic { .. })
        && (oracle.definitely_non_copy(&p.ty) || type_mentions_name(&p.ty, generic_names))
}

/// Is this `take` parameter's move CONDITIONAL on the argument — i.e. did it
/// qualify only through the generic-param arm above? A parameter whose written
/// type is definitely non-Copy moves whatever the caller hands it; one typed
/// `T` moves only when `T` turned out non-Copy, and the call walker asks the
/// argument. Recomputed at the call site from the recorded parameter type
/// rather than stored, so no signature entry has to carry a parallel vector.
fn param_move_is_conditional(param_ty: &Type, oracle: &CopyOracle) -> bool {
    !oracle.definitely_non_copy(param_ty)
}

/// Does this type name any of the given generic params anywhere in its
/// structure? Drives the `ret_view` widening for generic fns: pre-mono,
/// `Option[Box[T]]` answers false to every view predicate, but an
/// instantiation may be a carrier, so a call must stay able to transport
/// its arguments' taint. Fn-pointer types are deliberately excluded — a
/// signature mentioning `T` does not store one (the stored-ness fixpoint
/// makes the same call).
fn type_mentions_name(t: &Type, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    match &t.kind {
        TypeKind::Path(p) => names.iter().any(|n| n == p),
        TypeKind::Generic { name, args } => {
            names.iter().any(|n| n == name) || args.iter().any(|a| type_mentions_name(a, names))
        }
        TypeKind::Slice(inner) | TypeKind::RawPtr(inner) => type_mentions_name(inner, names),
        TypeKind::Array { elem, .. } => type_mentions_name(elem, names),
        _ => false,
    }
}

/// The unqualified base of a type name: `box::Box` → `Box`. The method
/// map is keyed by the impl target's name as written at the definition
/// (`impl Box[T]` → `Box`), so lookups from qualified-use sites strip.
fn base_type_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

impl SigTable {
    /// Receiver-keyed method lookup with the TEXT→STR fallthrough sema's
    /// dispatch applies (2026-08-13): when the receiver's type is the
    /// `#[lang("string")]` struct and it declares no inherent method of this
    /// name, the call resolved against the blessed `impl str` set — so the
    /// claim/view classification must come from `str.<method>`. An inherent
    /// method always wins, mirroring sema exactly. `type_name` may arrive
    /// module-qualified (`text::Text`); compare through `base_type_name`.
    fn method_entry(&self, type_name: &str, method: &str) -> Option<&FnEntry> {
        if let Some(e) = self.methods.get(&format!("{type_name}.{method}")) {
            return Some(e);
        }
        let ls = self.lang_string.as_deref()?;
        if type_name == ls || base_type_name(type_name) == ls {
            return self.methods.get(&format!("str.{method}"));
        }
        let _ = ls;
        None
    }

    /// Register the COMPILER-PROVIDED `str` methods, which no `impl` block
    /// declares and the collector above therefore never sees. Only
    /// `to_text` matters to this pass: it is the one that returns something
    /// a later link can be called on, and without an entry the table could
    /// not TYPE `q.to_text()` at all — `infer_ty` answered `None` and every
    /// receiver-keyed rule went quiet. That silence is what let
    /// `q.to_text().trim()` bind a view of a temporary with no diagnostic.
    ///
    /// It must be registered, NOT resolved by falling through to the
    /// lang-string struct's own `to_text`. They are different functions:
    /// `Text::to_text` is `this.clone()`, so the flow pass reads it as
    /// returning receiver-rooted data. Borrowing that classification for a
    /// `str` receiver says the returned Text views the str's bytes — false,
    /// this allocates a copy — and a method storing the result into `ref
    /// this` then looked like it kept a view of its argument. That fired
    /// E0514 on four correct iris call sites, including one in a sibling
    /// branch where the named owner was not even in scope.
    fn register_builtin_str_methods(&mut self) {
        let Some(ls) = self.lang_string.clone() else {
            return;
        };
        let entry = FnEntry {
            receiver_claim: Some(ClaimKind::Shared),
            ret_ty: Some(Type {
                kind: TypeKind::Path(ls),
                span: Span::new(0, 0),
            }),
            // Owns its bytes: transports nothing out of the receiver, keeps
            // nothing of its arguments, and is not a view.
            computed_ret_flow: Some(0),
            computed_ret_from_receiver: Some(false),
            ..FnEntry::default()
        };
        self.methods.entry("str.to_text".to_string()).or_insert(entry);
    }

    fn collect(prog: &Program, oracle: &CopyOracle) -> Self {
        let mut t = SigTable::default();
        for item in &prog.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    let (return_borrow, return_borrow_flavor) =
                        detect_fn_elision_with_flavor(f, oracle);
                    // Mirrors the flow-pass skip predicate exactly: an entry
                    // marked analyzable here MUST get its `computed_ret_flow`
                    // filled by `compute_receiver_flows`, or precision lies.
                    let flow_analyzed =
                        f.generic_params.is_empty() && !f.is_extern && !f.is_declaration;
                    let generic_names: Vec<String> = f
                        .generic_params
                        .iter()
                        .map(|g| g.name.name.clone())
                        .collect();
                    t.fns.insert(
                        f.name.name.clone(),
                        FnEntry {
                            receiver_claim: None,
                            param_moves: f
                                .params
                                .iter()
                                .map(|p| param_is_effective_move(p, oracle, &generic_names))
                                .collect(),
                            param_muts: f.params.iter().map(|p| p.mutable).collect(),
                            return_borrow,
                            return_borrow_flavor,
                            keeps_this: crate::attrs::has_keeps(&f.attributes, "this"),
                            param_view_flags: f
                                .params
                                .iter()
                                .map(|p| oracle.type_contains_view(&p.ty))
                                .collect(),
                            computed_keeps: Vec::new(),
                            computed_ref_flows: Vec::new(),
                            computed_ret_flow: if flow_analyzed { Some(0) } else { None },
                            computed_ret_from_receiver: if flow_analyzed {
                                Some(false)
                            } else {
                                None
                            },
                            // A generic fn whose return type names its own
                            // params (`box::new[T] -> Option[Box[T]]`) can
                            // transport its arguments' taint even though the
                            // unsubstituted type answers false to every view
                            // predicate — the instantiation may be a carrier.
                            ret_view: f.return_type.as_ref().is_some_and(|t| {
                                oracle.type_contains_view(t)
                                    || oracle.definitely_non_copy(t)
                                    || type_mentions_name(t, &generic_names)
                            }),
                            param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                            ret_ty: f.return_type.clone(),
                        },
                    );
                }
                ItemKind::Struct(s) => {
                    t.struct_fields.insert(
                        s.name.name.clone(),
                        s.fields
                            .iter()
                            .map(|f| (f.name.name.clone(), f.ty.clone()))
                            .collect(),
                    );
                    // TEXT→STR fallthrough: remember the `#[lang("string")]`
                    // owner's name (sema validated uniqueness/shape).
                    let is_lang_string = s.attributes.iter().any(|a| {
                        a.path.name == "lang"
                            && matches!(a.args.first(),
                                Some(crate::ast::AttrArg::Str(v, _)) if v == "string")
                    });
                    if is_lang_string {
                        t.lang_string = Some(s.name.name.clone());
                    }
                }
                ItemKind::Enum(e) => {
                    t.enums.insert(e.name.name.clone());
                    t.enum_payloads.insert(
                        e.name.name.clone(),
                        e.variants
                            .iter()
                            .map(|v| (v.name.name.clone(), v.payload.clone()))
                            .collect(),
                    );
                    if !e.generic_params.is_empty() {
                        t.enum_generics.insert(
                            e.name.name.clone(),
                            e.generic_params
                                .iter()
                                .map(|g| g.name.name.clone())
                                .collect(),
                        );
                    }
                }
                ItemKind::Impl(b) => {
                    if !b.target_generic_params.is_empty() {
                        t.impl_generics
                            .entry(b.target.name.clone())
                            .or_insert_with(|| {
                                b.target_generic_params
                                    .iter()
                                    .map(|g| g.name.name.clone())
                                    .collect()
                            });
                    }
                    for m in &b.methods {
                        let key = format!("{}.{}", b.target.name, m.name.name);
                        let (return_borrow, return_borrow_flavor) =
                            detect_method_elision_with_flavor(b, m, oracle);
                        // Same predicate as the method flow loop (which
                        // analyzes methods of generic IMPLS, skipping only
                        // method-own generics and declarations).
                        let flow_analyzed = m.generic_params.is_empty()
                            && !m.is_declaration
                            && m.receiver.is_some();
                        let generic_names: Vec<String> = b
                            .target_generic_params
                            .iter()
                            .chain(m.generic_params.iter())
                            .map(|g| g.name.name.clone())
                            .collect();
                        t.methods.insert(
                            key,
                            FnEntry {
                                receiver_claim: m.receiver.as_ref().map(|r| match r {
                                    crate::ast::Receiver::Read => ClaimKind::Shared,
                                    crate::ast::Receiver::Mut => ClaimKind::Exclusive,
                                    crate::ast::Receiver::Move => ClaimKind::Move,
                                }),
                                param_moves: m
                                    .params
                                    .iter()
                                    .map(|p| param_is_effective_move(p, oracle, &generic_names))
                                    .collect(),
                                param_muts: m.params.iter().map(|p| p.mutable).collect(),
                                return_borrow,
                                return_borrow_flavor,
                                keeps_this: crate::attrs::has_keeps(&m.attributes, "this"),
                                param_view_flags: m
                                    .params
                                    .iter()
                                    .map(|p| oracle.type_contains_view(&p.ty))
                                    .collect(),
                                computed_keeps: Vec::new(),
                                computed_ref_flows: Vec::new(),
                                computed_ret_flow: if flow_analyzed { Some(0) } else { None },
                                computed_ret_from_receiver: if flow_analyzed {
                                    Some(false)
                                } else {
                                    None
                                },
                                ret_view: m.return_type.as_ref().is_some_and(|t| {
                                    oracle.type_contains_view(t)
                                        || oracle.definitely_non_copy(t)
                                        || type_mentions_name(t, &generic_names)
                                }),
                                param_tys: m.params.iter().map(|p| p.ty.clone()).collect(),
                                ret_ty: m.return_type.clone(),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        // After the walk: `lang_string` is known by now, and a real
        // `impl str` declaration (there is none for `to_text` — sema rejects
        // redeclaring it) would already hold the key.
        t.register_builtin_str_methods();
        t
    }

    fn fn_param_moves(&self, name: &str) -> Option<&Vec<bool>> {
        self.fns.get(name).map(|e| &e.param_moves)
    }

    /// Slice 6BC.1: per-parameter `ref` flag list. Parallel shape to
    /// `fn_param_muts`. Used by `apply_call` to claim a
    /// `BorrowedExclusive` against each `ref`-marked non-Copy argument
    /// and detect the four intra-call conflict patterns.
    fn fn_param_muts(&self, name: &str) -> Option<&Vec<bool>> {
        self.fns.get(name).map(|e| &e.param_muts)
    }

    fn fn_param_tys(&self, name: &str) -> Option<&Vec<Type>> {
        self.fns.get(name).map(|e| &e.param_tys)
    }

    /// The effective per-param keeps flags for an entry: declared
    /// (`#[keeps(this)]` gated to view-typed positions) unioned with the
    /// computed transitive flows. Empty vec means "ties nothing".
    /// Which parameter positions the RECEIVER keeps a borrow of after the
    /// call returns.
    ///
    /// `#[keeps(this)]` declares it; what it covers is every position that
    /// can BE a borrow:
    ///
    /// - a view-typed parameter (`str`, a slice), which is a borrow written
    ///   as a value and is the case the declaration was introduced for, and
    /// - a **`ref` parameter** (v0.0.28), which is a borrow written as a
    ///   borrow. Storing one means storing its address, and a receiver that
    ///   outlives the borrow is the same dangling pointer either way.
    ///
    /// The second is what makes a scoped thread pool statically safe:
    /// `scope.spawn_mut(ref data, worker)` ties `data` to `scope`, so the
    /// parent cannot touch or drop `data` until the scope — whose destructor
    /// joins the workers — is gone.
    ///
    /// `take` positions are moves, not borrows, and are covered only by the
    /// view gate (a `Vec[str]` that consumes a `str` really does keep the
    /// view its caller owns).
    fn effective_keeps(entry: &FnEntry) -> Vec<bool> {
        let n = entry
            .param_view_flags
            .len()
            .max(entry.computed_keeps.len())
            .max(entry.param_muts.len());
        (0..n)
            .map(|i| {
                (entry.keeps_this
                    && (entry.param_view_flags.get(i).copied().unwrap_or(false)
                        || entry.param_muts.get(i).copied().unwrap_or(false)))
                    || entry.computed_keeps.get(i).copied().unwrap_or(false)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Memory-model contract §5 — computed receiver flows (the summary engine's
// first computed fact family).
//
// For every method of a concrete impl, compute which parameters MAY flow
// into the receiver: a union-over-all-paths taint walk of the body, no
// condition analysis (a store under `if` counts — the same posture as the
// rest of the borrow model). Direct `this.f = param` stores are already
// denied by E0515 unless declared `#[keeps(this)]`, so what this pass adds
// is the TRANSITIVE route the declared form cannot see:
//
//     #[keeps(this)] fn set(ref this, k: str) { this.view = k; }
//     fn set_outer(ref this, k: str) { this.set(k); }   // undeclared!
//
// Without the computed bits, callers of `set_outer` tie nothing and the
// stored view dangles when the argument's owner dies — verified safe-code
// UAF. With them, `set_outer` ties exactly like `set`.
//
// Scope (documented in the contract): concrete impls only — generic impls
// are pre-mono here and their receivers don't resolve at call sites; their
// direct stores keep the E0515 deny. Unresolvable callees contribute no
// taint (their return ties are enforced by the existing E-rules at the
// real call sites).
// ---------------------------------------------------------------------------

/// Contract §5 / generic-impl ties: substitute generic-param names with the
/// receiver's type arguments. Purely structural — no monomorphization, no
/// sema types; just enough to ask `type_contains_view` of an instantiated
/// signature (`take item: T` under `Vec[str]` → `str`).
fn subst_type(ty: &Type, map: &HashMap<String, Type>) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) => {
            if let Some(t) = map.get(name) {
                return t.clone();
            }
            TypeKind::Path(name.clone())
        }
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args.iter().map(|a| subst_type(a, map)).collect(),
        },
        TypeKind::Slice(inner) => TypeKind::Slice(Box::new(subst_type(inner, map))),
        TypeKind::Array {
            elem,
            len,
            len_name,
            len_expr,
        } => TypeKind::Array {
            elem: Box::new(subst_type(elem, map)),
            len: *len,
            len_name: len_name.clone(),
            len_expr: len_expr.clone(),
        },
        other => other.clone(),
    };
    Type {
        kind,
        span: ty.span,
    }
}

/// The taint bit standing for the method receiver in `FlowCtx`. Params
/// occupy bits 0..64 in principle but every read-out masks to the actual
/// param count, so the top bit is free to carry "the receiver's bytes may
/// be here" — what `Box[T].into_raw` returning `this._p` needs to export.
const RECV_TAINT_BIT: u64 = 1u64 << 63;

struct FlowCtx<'a> {
    sigs: &'a SigTable,
    /// binding name -> source-param bitmask (bit i = param i may be here).
    taint: HashMap<String, u64>,
    /// binding name -> declared Path type (for method resolution).
    types: HashMap<String, String>,
    /// accumulated: bits that reached the receiver.
    receiver_bits: u64,
    /// accumulated: bits that reached a `return` (or the body's tail
    /// expression — the caller of `walk_block` folds that in). Feeds
    /// `computed_ret_flow` / `computed_ret_from_receiver`.
    ret_bits: u64,
}

impl<'a> FlowCtx<'a> {
    fn is_receiver_root(root: &str) -> bool {
        root == "self" || root == "this"
    }

    /// Give match-payload bindings a declared type when the pattern spells
    /// the enum's instantiation (`Option[Box[Sink]]::Some(b)` types `b` as
    /// `Box`): zip the enum's generic params with the written type args,
    /// substitute into the variant's payload types, record the base name.
    /// Patterns without written args stay untyped — a conservative miss,
    /// the same posture as unannotated bindings.
    fn type_payload_bindings(&mut self, p: &Pattern) {
        let PatternKind::Variant {
            enum_name,
            type_args,
            variant_name,
            payload,
        } = &p.kind
        else {
            return;
        };
        if type_args.is_empty() || payload.is_empty() {
            return;
        }
        let base = base_type_name(&enum_name.name);
        let Some(generics) = self.sigs.enum_generics.get(base) else {
            return;
        };
        let Some(ptys) = self
            .sigs
            .enum_payloads
            .get(base)
            .and_then(|v| v.get(&variant_name.name))
        else {
            return;
        };
        let map: HashMap<String, Type> = generics
            .iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect();
        for (i, pat) in payload.iter().enumerate() {
            let PatternKind::Binding(b) = &pat.kind else {
                continue;
            };
            let Some(t) = ptys.get(i) else { continue };
            let resolved = subst_type(t, &map);
            let name = match &resolved.kind {
                TypeKind::Path(p) => Some(base_type_name(p).to_string()),
                TypeKind::Generic { name, .. } => Some(base_type_name(name).to_string()),
                _ => None,
            };
            if let Some(n) = name {
                self.types.insert(b.name.clone(), n);
            }
        }
    }

    fn walk_block(&mut self, b: &Block) -> u64 {
        for s in &b.stmts {
            self.walk_stmt(s);
        }
        match &b.tail {
            Some(t) => self.expr_taint(t),
            None => 0,
        }
    }

    fn walk_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, ty, init, .. } => {
                let t = init.as_ref().map(|e| self.expr_taint(e)).unwrap_or(0);
                self.taint.insert(name.name.clone(), t);
                if let Some(decl) = ty {
                    match &decl.kind {
                        TypeKind::Path(p) => {
                            self.types.insert(name.name.clone(), p.clone());
                        }
                        // A generic instantiation resolves methods through
                        // its base (`let b: box::Box[Sink]` → `Box.into_raw`).
                        TypeKind::Generic { name: g, .. } => {
                            self.types
                                .insert(name.name.clone(), base_type_name(g).to_string());
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::LetDestructure { fields, init, .. } => {
                let t = self.expr_taint(init);
                for f in fields {
                    self.taint.insert(f.name.clone(), t);
                }
            }
            StmtKind::Return(Some(e)) => {
                let t = self.expr_taint(e);
                self.ret_bits |= t;
            }
            StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
                self.expr_taint(e);
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::While { cond, body, .. } => {
                self.expr_taint(cond);
                // Twice: loop-carried taint (bottom feeds top) stabilizes in
                // one extra pass for a union-only lattice.
                self.walk_block(body);
                self.walk_block(body);
            }
            StmtKind::Loop(b, _) => {
                self.walk_block(b);
                self.walk_block(b);
            }
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(i) = init {
                        self.walk_stmt(i);
                    }
                    if let Some(c) = cond {
                        self.expr_taint(c);
                    }
                    for u in update {
                        self.expr_taint(u);
                    }
                    self.walk_block(body);
                    self.walk_block(body);
                }
                ForLoop::Range { var, iter, body } => {
                    self.expr_taint(iter);
                    self.taint.insert(var.name.clone(), 0);
                    self.walk_block(body);
                    self.walk_block(body);
                }
            },
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {}
        }
    }

    fn expr_taint(&mut self, e: &Expr) -> u64 {
        match &e.kind {
            ExprKind::Ident(name) => self.taint.get(name).copied().unwrap_or(0),
            ExprKind::Field { .. } | ExprKind::Index { .. } => match place_from_expr(e) {
                Some(p) => {
                    if let ExprKind::Index { index, .. } = &e.kind {
                        self.expr_taint(index);
                    }
                    self.taint.get(&p.root).copied().unwrap_or(0)
                }
                None => {
                    // Non-place chain (call receiver etc.) — taint of the
                    // receiver expression flows through the projection.
                    match &e.kind {
                        ExprKind::Field { receiver, .. } => self.expr_taint(receiver),
                        ExprKind::Index { receiver, index } => {
                            self.expr_taint(index);
                            self.expr_taint(receiver)
                        }
                        _ => 0,
                    }
                }
            },
            ExprKind::Assign { target, value, .. } => {
                let vt = self.expr_taint(value);
                if let Some(place) = place_from_expr(target) {
                    if Self::is_receiver_root(&place.root) {
                        self.receiver_bits |= vt;
                    } else {
                        *self.taint.entry(place.root).or_insert(0) |= vt;
                    }
                }
                0
            }
            ExprKind::Call { callee, args, .. } => {
                let arg_taints: Vec<u64> = args.iter().map(|a| self.expr_taint(a)).collect();
                match &callee.kind {
                    ExprKind::Field {
                        receiver,
                        name: method,
                    } => {
                        let rtaint = self.expr_taint(receiver);
                        let recv_root = place_from_expr(receiver).map(|p| p.root);
                        let recv_ty = match recv_root.as_deref() {
                            Some(r) if Self::is_receiver_root(r) => {
                                self.types.get("self").cloned()
                            }
                            Some(r) => self.types.get(r).cloned(),
                            None => None,
                        };
                        let entry =
                            recv_ty.and_then(|t| self.sigs.method_entry(&t, &method.name));
                        if let Some(entry) = entry {
                            let keeps = SigTable::effective_keeps(entry);
                            let mut kept: u64 = 0;
                            for (i, k) in keeps.iter().enumerate() {
                                if *k {
                                    kept |= arg_taints.get(i).copied().unwrap_or(0);
                                }
                            }
                            if kept != 0 {
                                if let Some(root) = recv_root {
                                    if Self::is_receiver_root(&root) {
                                        self.receiver_bits |= kept;
                                    } else {
                                        *self.taint.entry(root).or_insert(0) |= kept;
                                    }
                                }
                            }
                            // Transport: what of the receiver's and the
                            // arguments' taint rides out in the result?
                            // Analyzed callees answer precisely from their
                            // own body flows (`into_raw` transports its
                            // receiver; `from_str` transports nothing).
                            // Unanalyzed callees (generic methods,
                            // declarations) stay signature-conservative.
                            return match (
                                entry.computed_ret_flow,
                                entry.computed_ret_from_receiver,
                            ) {
                                (Some(bits), Some(recv)) => {
                                    let mut out = if recv { rtaint } else { 0 };
                                    for (i, t) in arg_taints.iter().enumerate() {
                                        if i < 64 && bits & (1u64 << i) != 0 {
                                            out |= t;
                                        }
                                    }
                                    out
                                }
                                _ if entry.ret_view => {
                                    rtaint | arg_taints.iter().fold(0, |a, t| a | t)
                                }
                                _ => 0,
                            };
                        }
                        0
                    }
                    ExprKind::Ident(name) => match self.sigs.fns.get(name) {
                        Some(entry) => match entry.computed_ret_flow {
                            Some(bits) => arg_taints
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| *i < 64 && bits & (1u64 << *i) != 0)
                                .fold(0, |a, (_, t)| a | t),
                            None if entry.ret_view => {
                                arg_taints.iter().fold(0, |a, t| a | t)
                            }
                            None => 0,
                        },
                        None => 0,
                    },
                    _ => {
                        self.expr_taint(callee);
                        0
                    }
                }
            }
            ExprKind::StructLit { fields, .. } => fields
                .iter()
                .fold(0, |a, f| a | self.expr_taint(&f.value)),
            ExprKind::InferredStructLit { fields } => fields
                .iter()
                .fold(0, |a, f| a | self.expr_taint(&f.value)),
            ExprKind::Block(b) => self.walk_block(b),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.expr_taint(cond);
                let t = self.walk_block(then);
                let e2 = else_branch.as_ref().map(|e| self.expr_taint(e)).unwrap_or(0);
                t | e2
            }
            ExprKind::Match { scrutinee, arms } => {
                let st = self.expr_taint(scrutinee);
                let mut out = 0;
                for a in arms {
                    // Payload bindings inherit the scrutinee's taint
                    // (`match opt { Some(v) => this.f = v, .. }`).
                    for name in pattern_binding_names(&a.pattern) {
                        self.taint.insert(name, st);
                    }
                    // And, when the pattern spells the enum's type args
                    // (`Option[Box[Sink]]::Some(b)`), a declared type — so
                    // a method call on the payload resolves its entry and
                    // the taint keeps riding (`b.into_raw()`).
                    self.type_payload_bindings(&a.pattern);
                    out |= self.expr_taint(&a.body);
                }
                out
            }
            ExprKind::Cast { expr, .. } => self.expr_taint(expr),
            ExprKind::Await(inner) | ExprKind::Yield(inner) => self.expr_taint(inner),
            ExprKind::Unary { operand, .. } => {
                self.expr_taint(operand);
                0
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr_taint(lhs);
                self.expr_taint(rhs);
                0
            }
            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let crate::ast::InterpStrPart::Expr(e) = p {
                        self.expr_taint(e);
                    }
                }
                0
            }
            ExprKind::Intrinsic { args, .. } => {
                // #str_from_raw_parts etc. — walk for side effects; the
                // produced view's provenance is raw (out of contract §4).
                for a in args {
                    self.expr_taint(a);
                }
                0
            }
            _ => 0,
        }
    }
}

/// Best-effort binding-name extraction from a match pattern, for taint
/// inheritance. Unknown pattern shapes contribute no names (their bindings
/// just carry no taint — an under-approximation the E0515 deny backstops).
fn pattern_binding_names(p: &Pattern) -> Vec<String> {
    let mut out = Vec::new();
    collect_pattern_names(p, &mut out);
    out
}

fn collect_pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match &p.kind {
        PatternKind::Binding(name) => out.push(name.name.clone()),
        PatternKind::Variant { payload, .. } => {
            for b in payload {
                collect_pattern_names(b, out);
            }
        }
        // Binds nothing, and `lower` desugars it away before this runs.
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
    }
}

/// Contract §3 narrowing support: every fn whose ADDRESS is taken — its
/// name appears as a value (fn-pointer arg, fn-ptr-typed binding, value
/// turbofish) rather than as a call's callee. Indirect calls through the
/// resulting pointers carry only the type-level `ref`/`take` markers, not
/// computed flows, so a storing fn that escapes into a pointer keeps its
/// definition-site deny (E0515). Missing an exotic expression shape here
/// under-scans, which fails SAFE only because sema treats membership as
/// "keep the deny" — so the walker errs on visiting everything it knows.
pub fn fns_with_address_taken(prog: &Program) -> std::collections::HashSet<String> {
    let mut fn_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &prog.items {
        if let ItemKind::Function(f) = &item.kind {
            fn_names.insert(f.name.name.clone());
        }
    }
    let mut taken = std::collections::HashSet::new();
    fn walk_e(
        e: &Expr,
        fns: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        match &e.kind {
            ExprKind::Ident(n) => {
                if fns.contains(n) {
                    out.insert(n.clone());
                }
            }
            ExprKind::FnRef { callee, .. } => {
                if let ExprKind::Ident(n) = &callee.kind {
                    out.insert(n.clone());
                }
            }
            ExprKind::Call { callee, args, .. } => {
                // The callee position is a CALL, not an address-take — but
                // its sub-expressions (a field-call's receiver) still walk.
                if let ExprKind::Field { receiver, .. } = &callee.kind {
                    walk_e(receiver, fns, out);
                } else if !matches!(&callee.kind, ExprKind::Ident(_) | ExprKind::Path { .. }) {
                    walk_e(callee, fns, out);
                }
                for a in args {
                    walk_e(a, fns, out);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                walk_e(target, fns, out);
                walk_e(value, fns, out);
            }
            ExprKind::Field { receiver, .. } => walk_e(receiver, fns, out),
            ExprKind::Index { receiver, index } => {
                walk_e(receiver, fns, out);
                walk_e(index, fns, out);
            }
            ExprKind::Cast { expr, .. } => walk_e(expr, fns, out),
            ExprKind::Unary { operand, .. } => walk_e(operand, fns, out),
            ExprKind::Binary { lhs, rhs, .. } => {
                walk_e(lhs, fns, out);
                walk_e(rhs, fns, out);
            }
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    walk_e(&f.value, fns, out);
                }
            }
            ExprKind::ArrayLit { elements }
            | ExprKind::TupleLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. } => {
                for el in elements {
                    walk_e(el, fns, out);
                }
            }
            ExprKind::ArrayFill { fill, .. } => walk_e(fill, fns, out),
            ExprKind::Block(b) => walk_b(b, fns, out),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                walk_e(cond, fns, out);
                walk_b(then, fns, out);
                if let Some(eb) = else_branch {
                    walk_e(eb, fns, out);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                walk_e(scrutinee, fns, out);
                for a in arms {
                    walk_e(&a.body, fns, out);
                }
            }
            ExprKind::Await(inner) | ExprKind::Yield(inner) => walk_e(inner, fns, out),
            ExprKind::InterpStr { parts } => {
                for pt in parts {
                    if let crate::ast::InterpStrPart::Expr(e2) = pt {
                        walk_e(e2, fns, out);
                    }
                }
            }
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    walk_e(a, fns, out);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    walk_e(&op.value, fns, out);
                }
            }
            _ => {}
        }
    }
    fn walk_b(
        b: &Block,
        fns: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        for s in &b.stmts {
            walk_s(s, fns, out);
        }
        if let Some(t) = &b.tail {
            walk_e(t, fns, out);
        }
    }
    fn walk_s(
        s: &Stmt,
        fns: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        match &s.kind {
            StmtKind::Let { init, .. } => {
                if let Some(e) = init {
                    walk_e(e, fns, out);
                }
            }
            StmtKind::LetDestructure { init, .. } => walk_e(init, fns, out),
            StmtKind::Return(Some(e))
            | StmtKind::Expr(e)
            | StmtKind::Defer(e)
            | StmtKind::Assert(e) => walk_e(e, fns, out),
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::While { cond, body, .. } => {
                walk_e(cond, fns, out);
                walk_b(body, fns, out);
            }
            StmtKind::Loop(b, _) => walk_b(b, fns, out),
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(i) = init {
                        walk_s(i, fns, out);
                    }
                    if let Some(c) = cond {
                        walk_e(c, fns, out);
                    }
                    for u in update {
                        walk_e(u, fns, out);
                    }
                    walk_b(body, fns, out);
                }
                ForLoop::Range { iter, body, .. } => {
                    walk_e(iter, fns, out);
                    walk_b(body, fns, out);
                }
            },
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {}
        }
    }
    for item in &prog.items {
        match &item.kind {
            ItemKind::Function(f) => walk_b(&f.body, &fn_names, &mut taken),
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    walk_b(&m.body, &fn_names, &mut taken);
                }
            }
            _ => {}
        }
    }
    taken
}

/// Run the receiver-flow fixpoint over every concrete impl method and
/// patch each `FnEntry.computed_keeps`. Monotone (bits only grow), so the
/// round cap is a backstop, not a correctness device.
fn compute_receiver_flows(prog: &Program, sigs: &mut SigTable) {
    for _round in 0..8 {
        let mut changed = false;
        for item in &prog.items {
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            // Generic impls are analyzed too (2026-08-01 final pass): the
            // param→receiver STRUCTURE of a body is type-agnostic, and the
            // Generic receiver resolution substitutes at call sites to gate
            // WHICH instantiations tie. The old skip predated that.
            for m in &b.methods {
                if !m.generic_params.is_empty() || m.is_declaration {
                    continue;
                }
                if m.receiver.is_none() {
                    continue;
                }
                let key = format!("{}.{}", b.target.name, m.name.name);
                let mut ctx = FlowCtx {
                    sigs,
                    taint: HashMap::new(),
                    types: HashMap::new(),
                    receiver_bits: 0,
                    ret_bits: 0,
                };
                ctx.types.insert("self".to_string(), b.target.name.clone());
                // Seed the receiver as its own taint source, so a body that
                // returns receiver-rooted data (`return this._p;`) exports
                // the receiver→return transport. Read-outs mask to the
                // param range, so the extra bit never leaks into keeps.
                ctx.taint.insert("this".to_string(), RECV_TAINT_BIT);
                ctx.taint.insert("self".to_string(), RECV_TAINT_BIT);
                for (i, p) in m.params.iter().enumerate() {
                    let Some(entry) = sigs.methods.get(&key) else {
                        continue;
                    };
                    let is_source = entry.param_view_flags.get(i).copied().unwrap_or(false)
                        || (!p.move_
                            && matches!(&p.ty.kind, TypeKind::Path(_) | TypeKind::Generic { .. }));
                    if is_source && i < 64 {
                        ctx.taint.insert(p.name.name.clone(), 1u64 << i);
                    } else {
                        ctx.taint.insert(p.name.name.clone(), 0);
                    }
                    if let TypeKind::Path(tp) = &p.ty.kind {
                        ctx.types.insert(p.name.name.clone(), tp.clone());
                    }
                }
                let tail = ctx.walk_block(&m.body);
                ctx.ret_bits |= tail;
                let bits = ctx.receiver_bits;
                let n_params = m.params.len();
                let new_flags: Vec<bool> =
                    (0..n_params).map(|i| i < 64 && bits & (1u64 << i) != 0).collect();
                let ret_param_bits = {
                    let mask = if n_params >= 64 {
                        !RECV_TAINT_BIT
                    } else {
                        (1u64 << n_params) - 1
                    };
                    ctx.ret_bits & mask
                };
                let ret_from_recv = ctx.ret_bits & RECV_TAINT_BIT != 0;
                if let Some(entry) = sigs.methods.get_mut(&key) {
                    if entry.computed_keeps != new_flags && new_flags.iter().any(|b| *b) {
                        entry.computed_keeps = new_flags;
                        changed = true;
                    }
                    // Union-only (monotone): a later round may add flows,
                    // never retract them, so the fixpoint converges.
                    let merged = entry.computed_ret_flow.unwrap_or(0) | ret_param_bits;
                    let mrecv = entry.computed_ret_from_receiver.unwrap_or(false) || ret_from_recv;
                    if entry.computed_ret_flow != Some(merged)
                        || entry.computed_ret_from_receiver != Some(mrecv)
                    {
                        entry.computed_ret_flow = Some(merged);
                        entry.computed_ret_from_receiver = Some(mrecv);
                        changed = true;
                    }
                }
            }
        }
        for item in &prog.items {
            let ItemKind::Function(f) = &item.kind else {
                continue;
            };
            if !f.generic_params.is_empty() || f.is_extern || f.is_declaration {
                continue;
            }
            let name = f.name.name.clone();
            let mut ctx = FlowCtx {
                sigs,
                taint: HashMap::new(),
                types: HashMap::new(),
                receiver_bits: 0,
                ret_bits: 0,
            };
            for (i, p) in f.params.iter().enumerate() {
                let Some(entry) = sigs.fns.get(&name) else {
                    continue;
                };
                let is_source = entry.param_view_flags.get(i).copied().unwrap_or(false)
                    || (!p.move_
                        && matches!(&p.ty.kind, TypeKind::Path(_) | TypeKind::Generic { .. }));
                if is_source && i < 64 {
                    ctx.taint.insert(p.name.name.clone(), 1u64 << i);
                } else {
                    ctx.taint.insert(p.name.name.clone(), 0);
                }
                if let TypeKind::Path(tp) = &p.ty.kind {
                    ctx.types.insert(p.name.name.clone(), tp.clone());
                }
            }
            let tail = ctx.walk_block(&f.body);
            ctx.ret_bits |= tail;
            let mut flows: Vec<(usize, usize)> = Vec::new();
            for (j, pj) in f.params.iter().enumerate() {
                if !(pj.mutable && !pj.move_) {
                    continue;
                }
                let bits = ctx.taint.get(&pj.name.name).copied().unwrap_or(0);
                for i in 0..f.params.len().min(64) {
                    if i != j && bits & (1u64 << i) != 0 {
                        flows.push((i, j));
                    }
                }
            }
            let ret_param_bits = {
                let n = f.params.len();
                let mask = if n >= 64 {
                    !RECV_TAINT_BIT
                } else {
                    (1u64 << n) - 1
                };
                ctx.ret_bits & mask
            };
            if let Some(entry) = sigs.fns.get_mut(&name) {
                if entry.computed_ref_flows != flows && !flows.is_empty() {
                    entry.computed_ref_flows = flows;
                    changed = true;
                }
                let merged = entry.computed_ret_flow.unwrap_or(0) | ret_param_bits;
                if entry.computed_ret_flow != Some(merged) {
                    entry.computed_ret_flow = Some(merged);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Public test hook (5BC.3a, 5BC.4): given a parsed program and the
/// bare name of a free function, return the elision rule's detected
/// return-borrow source if any. Used by unit tests.
pub fn return_borrow_source(prog: &Program, fn_name: &str) -> Option<ReturnBorrowSource> {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
    sigs.fns.get(fn_name)?.return_borrow.clone()
}

pub fn method_return_borrow_source(
    prog: &Program,
    target: &str,
    method: &str,
) -> Option<ReturnBorrowSource> {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
    let key = format!("{target}.{method}");
    sigs.methods.get(&key)?.return_borrow.clone()
}

/// Slice 6BC.2 test hook: return-borrow source + flavor for a free fn.
pub fn return_borrow_source_with_flavor(
    prog: &Program,
    fn_name: &str,
) -> Option<(ReturnBorrowSource, BorrowFlavor)> {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
    let entry = sigs.fns.get(fn_name)?;
    let src = entry.return_borrow.clone()?;
    let flavor = entry.return_borrow_flavor?;
    Some((src, flavor))
}

/// Slice 6BC.2 test hook: return-borrow source + flavor for a method.
pub fn method_return_borrow_source_with_flavor(
    prog: &Program,
    target: &str,
    method: &str,
) -> Option<(ReturnBorrowSource, BorrowFlavor)> {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
    let key = format!("{target}.{method}");
    let entry = sigs.methods.get(&key)?;
    let src = entry.return_borrow.clone()?;
    let flavor = entry.return_borrow_flavor?;
    Some((src, flavor))
}

/// Slice 6BC.2 / 6BC.4 / 6BC.5: free-function elision with flavor.
/// Explicit region annotations (slice 6BC.5) take precedence over
/// body-flow elision rules. When the signature carries any region
/// annotation, the source set is computed from the regions instead of
/// running E1/E1-mut/E3/E3-mut. When no annotation is present, falls
/// through to the rule ladder: E1-mut → E1 → E3-mut → E3. (The
/// `borrow REGION T` source syntax these annotations came from is
/// retired, so this branch is now unreachable from user source.)
fn detect_fn_elision_with_flavor(
    f: &Function,
    oracle: &CopyOracle,
) -> (Option<ReturnBorrowSource>, Option<BorrowFlavor>) {
    // Memory-model contract §5: `#[keeps(nothing)]` declares the return
    // borrows no argument (the body copies what it needs — `text::intern`).
    // Declared summaries beat guessed ones: skip the whole rule ladder.
    if crate::attrs::has_keeps(&f.attributes, "nothing") {
        return (None, None);
    }
    // 6BC.5: explicit annotations short-circuit elision.
    if let Some(s) = detect_fn_e1_mut(f, oracle) {
        return (Some(s), Some(BorrowFlavor::Exclusive));
    }
    if let Some(s) = detect_fn_e1(f, oracle) {
        return (Some(s), Some(BorrowFlavor::Shared));
    }
    if let Some(s) = detect_fn_e3_mut(f, oracle) {
        return (Some(s), Some(BorrowFlavor::Exclusive));
    }
    if let Some(s) = detect_fn_e3(f, oracle) {
        return (Some(s), Some(BorrowFlavor::Shared));
    }
    if let Some(s) = detect_fn_view(f, oracle) {
        return (Some(s), Some(BorrowFlavor::Shared));
    }
    (None, None)
}

/// Rule E-VIEW-FN (2026-07-06): the free-function analog of Rule
/// E-VIEW. A function returning `str` or a slice with borrow-passed
/// non-Copy parameters returns a VIEW into one of them
/// (`fn head(t: Text) -> str { return t.view(); }`), so the result
/// borrows every such parameter. E1/E3 never see these because views
/// are Copy return types. Same conservatism as the method rule: the
/// tie applies regardless of what the body returns. Flavor is Shared —
/// a view only reads. `take` params never contribute (the value is
/// consumed; a view into it is a different bug, caught by drop order).
fn detect_fn_view(f: &Function, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    // STRM v3 (2026-08-01): widened in both directions. Return side: a fn
    // returning a view-CARRYING aggregate (`fn store(k: str) -> Data` where
    // `Data` has a `str` field) launders the borrow exactly like a bare
    // view return — the str_dangle_repro hole. Param side: a view-typed /
    // view-carrying parameter is itself a borrow the result may extend, so
    // it contributes even though `str` is Copy (the old `definitely_non_copy`
    // filter silently excluded every `str` param).
    let ret = f.return_type.as_ref()?;
    if !oracle.type_contains_view(ret) {
        return None;
    }
    let mut indices: Vec<u32> = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        if p.move_ {
            continue;
        }
        if !(oracle.definitely_non_copy(&p.ty) || oracle.type_contains_view(&p.ty)) {
            continue;
        }
        indices.push(i as u32);
    }
    match indices.len() {
        0 => None,
        1 => Some(ReturnBorrowSource::Param(indices[0])),
        _ => Some(ReturnBorrowSource::MultiParam(indices)),
    }
}
fn detect_method_elision_with_flavor(
    b: &ImplBlock,
    m: &Method,
    oracle: &CopyOracle,
) -> (Option<ReturnBorrowSource>, Option<BorrowFlavor>) {
    // Memory-model contract §5: `#[keeps(nothing)]` — declared no-tie
    // summary, same as the free-fn form.
    if crate::attrs::has_keeps(&m.attributes, "nothing") {
        return (None, None);
    }
    if let Some(s) = detect_method_e2_mut(b, m, oracle) {
        return (Some(s), Some(BorrowFlavor::Exclusive));
    }
    if let Some(s) = detect_method_e2(b, m, oracle) {
        return (Some(s), Some(BorrowFlavor::Shared));
    }
    if let Some(s) = detect_method_view(b, m, oracle) {
        return (Some(s), Some(BorrowFlavor::Shared));
    }
    (None, None)
}

/// Rule E-VIEW (2026-07-06 memory-model hardening): a borrow-receiver
/// method on a non-Copy type returning `str` or a slice returns a VIEW
/// into the receiver's storage — a fat pointer with no lifetime of its
/// own (`Text::view`, `Vec::as_slice`). Binding the result borrows the
/// receiver, so moving/consuming the owner while the view is live is
/// rejected by the existing borrow machinery — closing the audit's #1
/// safe-code use-after-free (`let s = t.view(); consume(t); peek(s)`).
///
/// Conservative on purpose: the tie applies even if the body returns a
/// literal (`fn kind(this) -> str { return "x"; }`) — a rare
/// over-restriction, and the sound direction.
fn detect_method_view(
    b: &ImplBlock,
    m: &Method,
    oracle: &CopyOracle,
) -> Option<ReturnBorrowSource> {
    if !matches!(m.receiver, Some(Receiver::Read) | Some(Receiver::Mut)) {
        return None;
    }
    // STRM v3 (2026-08-01): widened. Receiver side: a view-typed receiver
    // (the blessed `impl str` block's sub-view methods — `v.trim()`) or a
    // view-carrying one qualifies even though it is Copy: the result
    // extends whatever the receiver borrows. Return side: view-carrying
    // aggregates tie like bare views.
    let synth = Type {
        kind: TypeKind::Path(b.target.name.clone()),
        span: Span::new(0, 0),
    };
    if !(oracle.definitely_non_copy(&synth) || oracle.type_contains_view(&synth)) {
        return None;
    }
    let ret = m.return_type.as_ref()?;
    if !oracle.type_contains_view(ret) {
        return None;
    }
    Some(ReturnBorrowSource::SelfReceiver)
}

/// Slice 6BC.2 — Rule E1-mut. Mirror of E1 but for a `ref`-marked param:
/// 1. Exactly one parameter, marked `ref` (and not `take`).
/// 2. Parameter type non-`Copy` (Copy `ref x` is local-mutability, not a borrow).
/// 3. Non-`Copy` return type.
/// 4. Every `return EXPR;` rooted at the parameter (same body-walk as E1).
///
/// When all checks pass, the return is an *exclusive* borrow of the parameter.
fn detect_fn_e1_mut(f: &Function, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    let [p]: &[Param; 1] = (f.params.as_slice()).try_into().ok()?;
    if !p.mutable || p.move_ {
        return None;
    }
    if !oracle.definitely_non_copy(&p.ty) {
        return None;
    }
    let ret = f.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    if !body_returns_only_rooted_at(&f.body, &p.name.name) {
        return None;
    }
    Some(ReturnBorrowSource::Param(0))
}

/// Slice 6BC.2 — Rule E2-mut. Mirror of E2 but for `ref this`:
/// 1. Receiver is `ref this` (i.e. `Receiver::Mut`).
/// 2. Impl-target type non-`Copy`.
/// 3. Non-`Copy` return type.
/// 4. Every `return EXPR;` rooted at `this`.
///
/// The return is an exclusive borrow of `this`.
fn detect_method_e2_mut(
    b: &ImplBlock,
    m: &Method,
    oracle: &CopyOracle,
) -> Option<ReturnBorrowSource> {
    if m.receiver != Some(Receiver::Mut) {
        return None;
    }
    let synth = Type {
        kind: TypeKind::Path(b.target.name.clone()),
        span: Span::new(0, 0),
    };
    if !oracle.definitely_non_copy(&synth) {
        return None;
    }
    let ret = m.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    if !body_returns_only_rooted_at(&m.body, "self") {
        return None;
    }
    Some(ReturnBorrowSource::SelfReceiver)
}

/// Rule E1 detection. The function qualifies iff:
/// 1. Exactly one parameter (zero parameters can't return a borrow of one).
/// 2. That parameter is a shared borrow (no `ref`, no `take`).
/// 3. The parameter type is non-`Copy`.
/// 4. The function has a non-`Copy` return type.
/// 5. The function body has at least one `return` statement, and every
///    `return EXPR;` has EXPR being a path rooted at the parameter's
///    binding (a chain of field / index accesses ending at the param).
fn detect_fn_e1(f: &Function, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    // Step 1: exactly one parameter.
    let [p]: &[Param; 1] = (f.params.as_slice()).try_into().ok()?;
    // Step 2: shared-borrow form (no ref, no take).
    if p.mutable || p.move_ {
        return None;
    }
    // Step 3: param type non-Copy.
    if !oracle.definitely_non_copy(&p.ty) {
        return None;
    }
    // Step 4: non-Copy return.
    let ret = f.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    // Step 5: every return rooted at the param.
    if !body_returns_only_rooted_at(&f.body, &p.name.name) {
        return None;
    }
    Some(ReturnBorrowSource::Param(0))
}

/// Rule E3 detection — the `longest(xs, ys)` case. The function
/// qualifies iff:
/// 1. **2+ parameters**, all shared-borrow form, all non-`Copy`.
/// 2. Non-`Copy` return type.
/// 3. Every `return EXPR;` has EXPR rooted at *some* parameter (not
///    necessarily the same one on each path). Collect the union of
///    referenced params into the result. At least one return must
///    exist (consistent with E1).
///
/// Conservative on purpose: the design note §4.1 picks "elide less
/// rather than more" — only admit Rule E3 when we can prove every
/// return path roots at some parameter. Returns of fresh-constructed
/// values (`return T::new();`) on any path disqualify.
fn detect_fn_e3(f: &Function, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    if f.params.len() < 2 {
        return None;
    }
    for p in &f.params {
        if p.mutable || p.move_ {
            return None;
        }
        if !oracle.definitely_non_copy(&p.ty) {
            return None;
        }
    }
    let ret = f.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    let param_names: Vec<&str> = f.params.iter().map(|p| p.name.name.as_str()).collect();
    let mut roots = std::collections::BTreeSet::new();
    let mut found_return = false;
    if !check_block_returns_e3(&f.body, &param_names, &mut roots, &mut found_return) {
        return None;
    }
    if !found_return || roots.is_empty() {
        return None;
    }
    let indices: Vec<u32> = roots.into_iter().collect();
    if indices.len() < 2 {
        // Every return rooted at the same single param — that's E1's
        // territory, but with 2+ params it's a degenerate case. Treat
        // as MultiParam with one entry for uniformity, since E1 only
        // applies when the function has exactly one param.
        return Some(ReturnBorrowSource::MultiParam(indices));
    }
    Some(ReturnBorrowSource::MultiParam(indices))
}

/// Slice 6BC.4 — Rule E3-mut. Mirror of E3 for `ref`-marked params.
/// Qualifies iff:
/// 1. 2+ params, all `ref`-marked (no `take`), all non-Copy.
/// 2. Non-Copy return type.
/// 3. Every `return EXPR;` rooted at some `ref`-param. At least one
///    return exists. Returns of fresh-constructed values on any path
///    disqualify.
///
/// Result is an exclusive multi-source borrow — the caller's binding
/// is tied to every parameter in `indices`.
fn detect_fn_e3_mut(f: &Function, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    if f.params.len() < 2 {
        return None;
    }
    for p in &f.params {
        if !p.mutable || p.move_ {
            return None;
        }
        if !oracle.definitely_non_copy(&p.ty) {
            return None;
        }
    }
    let ret = f.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    let param_names: Vec<&str> = f.params.iter().map(|p| p.name.name.as_str()).collect();
    let mut roots = std::collections::BTreeSet::new();
    let mut found_return = false;
    if !check_block_returns_e3(&f.body, &param_names, &mut roots, &mut found_return) {
        return None;
    }
    if !found_return || roots.is_empty() {
        return None;
    }
    let indices: Vec<u32> = roots.into_iter().collect();
    Some(ReturnBorrowSource::MultiParam(indices))
}

/// E3 body walk. For each `return EXPR;`, identify which (if any)
/// parameter the expression is rooted at. Returns `true` iff every
/// return is rooted at some parameter in `param_names`. Roots
/// accumulate into `roots` as parameter indices.
fn check_block_returns_e3(
    b: &Block,
    param_names: &[&str],
    roots: &mut std::collections::BTreeSet<u32>,
    found: &mut bool,
) -> bool {
    for s in &b.stmts {
        if !check_stmt_returns_e3(s, param_names, roots, found) {
            return false;
        }
    }
    if let Some(t) = &b.tail {
        if !check_expr_returns_e3(t, param_names, roots, found) {
            return false;
        }
    }
    true
}

fn check_stmt_returns_e3(
    s: &Stmt,
    param_names: &[&str],
    roots: &mut std::collections::BTreeSet<u32>,
    found: &mut bool,
) -> bool {
    match &s.kind {
        StmtKind::Return(Some(e)) => {
            *found = true;
            let Some(root) = expr_root_ident(e) else {
                return false;
            };
            let Some(idx) = param_names.iter().position(|&n| n == root) else {
                return false;
            };
            roots.insert(idx as u32);
            true
        }
        StmtKind::Return(None) => false,
        StmtKind::Expr(e) | StmtKind::Defer(e) => {
            check_expr_returns_e3(e, param_names, roots, found)
        }
        StmtKind::Let { init, .. } => match init {
            Some(e) => check_expr_returns_e3(e, param_names, roots, found),
            None => true,
        },
        StmtKind::LetDestructure { init, .. } => {
            check_expr_returns_e3(init, param_names, roots, found)
        }
        StmtKind::While { cond, body, .. } => {
            check_expr_returns_e3(cond, param_names, roots, found)
                && check_block_returns_e3(body, param_names, roots, found)
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init {
                    if !check_stmt_returns_e3(i, param_names, roots, found) {
                        return false;
                    }
                }
                if let Some(c) = cond {
                    if !check_expr_returns_e3(c, param_names, roots, found) {
                        return false;
                    }
                }
                for u in update {
                    if !check_expr_returns_e3(u, param_names, roots, found) {
                        return false;
                    }
                }
                check_block_returns_e3(body, param_names, roots, found)
            }
            ForLoop::Range { iter, body, .. } => {
                check_expr_returns_e3(iter, param_names, roots, found)
                    && check_block_returns_e3(body, param_names, roots, found)
            }
        },
        StmtKind::Loop(b, _) => check_block_returns_e3(b, param_names, roots, found),
        StmtKind::Break | StmtKind::Continue => true,
        // `assert EXPR;` cannot contain a `return` (it's an expression
        // statement), so it never affects the rooted-returns set. We
        // still walk the expression to keep the analysis recursive in
        // case future expression forms can contain returns.
        StmtKind::Assert(e) => check_expr_returns_e3(e, param_names, roots, found),
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => true,
    }
}

fn check_expr_returns_e3(
    e: &Expr,
    param_names: &[&str],
    roots: &mut std::collections::BTreeSet<u32>,
    found: &mut bool,
) -> bool {
    match &e.kind {
        ExprKind::Block(b) => check_block_returns_e3(b, param_names, roots, found),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            if !check_expr_returns_e3(cond, param_names, roots, found) {
                return false;
            }
            if !check_block_returns_e3(then, param_names, roots, found) {
                return false;
            }
            if let Some(eb) = else_branch {
                if !check_expr_returns_e3(eb, param_names, roots, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Match { scrutinee, arms } => {
            if !check_expr_returns_e3(scrutinee, param_names, roots, found) {
                return false;
            }
            for a in arms {
                if !check_expr_returns_e3(&a.body, param_names, roots, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Call { callee, args, .. } => {
            if !check_expr_returns_e3(callee, param_names, roots, found) {
                return false;
            }
            for a in args {
                if !check_expr_returns_e3(a, param_names, roots, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            check_expr_returns_e3(lhs, param_names, roots, found)
                && check_expr_returns_e3(rhs, param_names, roots, found)
        }
        ExprKind::Unary { operand, .. } => {
            check_expr_returns_e3(operand, param_names, roots, found)
        }
        ExprKind::Range { start, end, .. } => {
            start
                .as_deref()
                .map_or(true, |s| check_expr_returns_e3(s, param_names, roots, found))
                && end
                    .as_deref()
                    .map_or(true, |e| check_expr_returns_e3(e, param_names, roots, found))
        }
        ExprKind::Assign { target, value, .. } => {
            check_expr_returns_e3(target, param_names, roots, found)
                && check_expr_returns_e3(value, param_names, roots, found)
        }
        ExprKind::Cast { expr, .. } => check_expr_returns_e3(expr, param_names, roots, found),
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => fields
            .iter()
            .all(|f| check_expr_returns_e3(&f.value, param_names, roots, found)),
        ExprKind::Field { receiver, .. } => {
            check_expr_returns_e3(receiver, param_names, roots, found)
        }
        ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
            elements
                .iter()
                .all(|e| check_expr_returns_e3(e, param_names, roots, found))
        }
        ExprKind::Index { receiver, index } => {
            check_expr_returns_e3(receiver, param_names, roots, found)
                && check_expr_returns_e3(index, param_names, roots, found)
        }
        _ => true,
    }
}

/// Rule E2 detection. Same shape as E1 but for methods with a `this`
/// receiver. The method qualifies iff:
/// 1. The method has a `Receiver::Read` receiver (i.e. `this`, not
///    `ref this`, not `take this`).
/// 2. The impl target type is non-`Copy`.
/// 3. The method's return type is non-`Copy`.
/// 4. Every `return EXPR;` is a path rooted at `this`.
fn detect_method_e2(b: &ImplBlock, m: &Method, oracle: &CopyOracle) -> Option<ReturnBorrowSource> {
    if m.receiver != Some(Receiver::Read) {
        return None;
    }
    let synth = Type {
        kind: TypeKind::Path(b.target.name.clone()),
        span: Span::new(0, 0),
    };
    if !oracle.definitely_non_copy(&synth) {
        return None;
    }
    let ret = m.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    if !body_returns_only_rooted_at(&m.body, "self") {
        return None;
    }
    Some(ReturnBorrowSource::SelfReceiver)
}

/// True iff `block` has at least one `return EXPR;` and *every* such
/// return's EXPR is a path rooted at `root` (a chain of field / index
/// accesses ending at the identifier `root`). Returns whose value is not
/// rooted at `root` (e.g. `return SomeStruct::new();`) disqualify the
/// function — the design note's conservative Rule E1 / E2 doesn't infer
/// a borrow when the body might construct a fresh owned value on some
/// path.
fn body_returns_only_rooted_at(block: &Block, root: &str) -> bool {
    let mut found = false;
    let ok = check_block_returns(block, root, &mut found);
    ok && found
}

fn check_block_returns(b: &Block, root: &str, found: &mut bool) -> bool {
    for s in &b.stmts {
        if !check_stmt_returns(s, root, found) {
            return false;
        }
    }
    if let Some(t) = &b.tail {
        if !check_expr_returns(t, root, found) {
            return false;
        }
    }
    true
}

fn check_stmt_returns(s: &Stmt, root: &str, found: &mut bool) -> bool {
    match &s.kind {
        StmtKind::Return(Some(e)) => {
            *found = true;
            expr_is_path_rooted_at(e, root)
        }
        StmtKind::Return(None) => false, // return with no value can't return a borrow
        StmtKind::Expr(e) | StmtKind::Defer(e) => check_expr_returns(e, root, found),
        StmtKind::Let { init, .. } => match init {
            Some(e) => check_expr_returns(e, root, found),
            None => true,
        },
        StmtKind::LetDestructure { init, .. } => check_expr_returns(init, root, found),
        StmtKind::While { cond, body, .. } => {
            check_expr_returns(cond, root, found) && check_block_returns(body, root, found)
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init {
                    if !check_stmt_returns(i, root, found) {
                        return false;
                    }
                }
                if let Some(c) = cond {
                    if !check_expr_returns(c, root, found) {
                        return false;
                    }
                }
                for u in update {
                    if !check_expr_returns(u, root, found) {
                        return false;
                    }
                }
                check_block_returns(body, root, found)
            }
            ForLoop::Range { iter, body, .. } => {
                check_expr_returns(iter, root, found) && check_block_returns(body, root, found)
            }
        },
        StmtKind::Loop(b, _) => check_block_returns(b, root, found),
        StmtKind::Break | StmtKind::Continue => true,
        StmtKind::Assert(e) => check_expr_returns(e, root, found),
        // Lowered away pre-borrowck.
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => true,
    }
}

fn check_expr_returns(e: &Expr, root: &str, found: &mut bool) -> bool {
    match &e.kind {
        ExprKind::Block(b) => check_block_returns(b, root, found),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            if !check_expr_returns(cond, root, found) {
                return false;
            }
            if !check_block_returns(then, root, found) {
                return false;
            }
            if let Some(eb) = else_branch {
                if !check_expr_returns(eb, root, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Match { scrutinee, arms } => {
            if !check_expr_returns(scrutinee, root, found) {
                return false;
            }
            for a in arms {
                if !check_expr_returns(&a.body, root, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Call { callee, args, .. } => {
            if !check_expr_returns(callee, root, found) {
                return false;
            }
            for a in args {
                if !check_expr_returns(a, root, found) {
                    return false;
                }
            }
            true
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            check_expr_returns(lhs, root, found) && check_expr_returns(rhs, root, found)
        }
        ExprKind::Unary { operand, .. } => check_expr_returns(operand, root, found),
        ExprKind::Range { start, end, .. } => {
            start
                .as_deref()
                .map_or(true, |s| check_expr_returns(s, root, found))
                && end
                    .as_deref()
                    .map_or(true, |e| check_expr_returns(e, root, found))
        }
        ExprKind::Assign { target, value, .. } => {
            check_expr_returns(target, root, found) && check_expr_returns(value, root, found)
        }
        ExprKind::Cast { expr, .. } => check_expr_returns(expr, root, found),
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => fields
            .iter()
            .all(|f| check_expr_returns(&f.value, root, found)),
        ExprKind::Field { receiver, .. } => check_expr_returns(receiver, root, found),
        ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
            elements.iter().all(|e| check_expr_returns(e, root, found))
        }
        ExprKind::Index { receiver, index } => {
            check_expr_returns(receiver, root, found) && check_expr_returns(index, root, found)
        }
        _ => true,
    }
}

/// True iff `e` is a chain of field / index accesses bottoming out at
/// `Ident(root)`. Examples (with root = "x"): `x`, `x.f`, `x.f.g`,
/// `x[0]`, `x.f[3].g`. Anything else (a call, a literal, a different
/// ident, a struct literal) returns false.
fn expr_is_path_rooted_at(e: &Expr, root: &str) -> bool {
    match &e.kind {
        ExprKind::Ident(n) => n == root,
        ExprKind::Field { receiver, .. } => expr_is_path_rooted_at(receiver, root),
        ExprKind::Index { receiver, .. } => expr_is_path_rooted_at(receiver, root),
        _ => false,
    }
}

/// If `e` is a chain of field/index projections rooted at some plain
/// `Ident`, return the root name. Otherwise `None`. Used to identify
/// "what binding does this expression name?"
fn expr_root_ident(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.as_str()),
        ExprKind::Field { receiver, .. } => expr_root_ident(receiver),
        ExprKind::Index { receiver, .. } => expr_root_ident(receiver),
        _ => None,
    }
}

/// Given a call's args and a parameter index, return the full place
/// expression at that argument position if it's a chain of identifier /
/// field / index projections. Used by E1/E3 classification and (slice
/// 6BC.3) by the intra-call partial-place overlap detection.
fn place_from_arg(args: &[Expr], idx: usize) -> Option<Place> {
    place_from_expr(args.get(idx)?)
}

/// Slice 6BC.3: a per-argument claim against its place. Built by
/// `check_intra_call_conflicts` for each direct-place arg (Mut / Move
/// position holding an Ident-rooted place expression). Shared claims
/// don't materialize as `ArgClaim` — the sibling-read scan probes
/// other args' expression trees rather than requiring a flat claim.
#[derive(Debug, Clone)]
struct ArgClaim {
    kind: ClaimKind,
    place: Place,
    span: crate::lexer::Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimKind {
    Shared,
    Exclusive,
    Move,
}

/// Slice 6BC.3: emit the conflict diagnostic for a pair of direct
/// `ArgClaim`s on overlapping places. Returns None when the pair is
/// admissible (Shared + Shared) or when the same pair has already
/// fired (the j < i ordering skip).
///
/// **Same-place pairs** (overlap = Same) emit the existing 6BC.1 codes:
///   - Mut + Mut    → E0380
///   - Mut + Move   → E0382
///   - Move + Mut   → E0382 (symmetric — emit once on the first pass)
///   - Mut + Shared → not reached (Shared claims don't materialize)
///   - Shared + Shared / any-with-no-conflict → admissible
///
/// **Partial-place pairs** (overlap = Contains / Contained) route to
/// E0374. The diagnostic explains that a borrow of a place includes
/// all its sub-places.
fn build_direct_claim_diag(
    primary: &ArgClaim,
    other: &ArgClaim,
    i: usize,
    j: usize,
    overlap: PlaceOverlap,
) -> Option<RawDiag> {
    // Symmetric pairs fire once per unordered pair. Pick the lower
    // index as the canonical "primary" to avoid duplicate diagnostics.
    // The exception is Mut + Move where the direction matters for the
    // message — E0382 fires regardless of order, but only once.
    use ClaimKind::*;
    let primary_name = &primary.place.root;
    let suggestion_span = primary.span.merge(other.span);

    if matches!(overlap, PlaceOverlap::Contains | PlaceOverlap::Contained) {
        // Partial-place conflict — always E0374. Fire once per unordered pair.
        if j < i {
            return None;
        }
        return Some(RawDiag {
            code: "E0374",
            message: format!("partial-place conflict on `{primary_name}` in the same call"),
            primary: primary.span,
            suggestion: Some((
                suggestion_span,
                String::new(),
                format!(
                    "a borrow of `{}` includes its sub-place `{}` (or vice versa). \
                     Split into two calls if the operations are independent, or \
                     restructure to operate on a single uniform place.",
                    primary.place.canonical(),
                    other.place.canonical()
                ),
            )),
            label: Some((
                other.span,
                format!("overlapping access to `{}` here", other.place.canonical()),
            )),
        });
    }

    // Same-place: dispatch by kinds.
    match (primary.kind, other.kind) {
        (Exclusive, Exclusive) => {
            if j < i {
                return None;
            } // dedup symmetric pair
            Some(RawDiag {
                code: "E0380",
                message: format!(
                    "cannot exclusively borrow `{primary_name}` twice in the same call"
                ),
                primary: other.span,
                suggestion: Some((
                    suggestion_span,
                    String::new(),
                    format!(
                        "at most one exclusive borrow of a place can be live at a time; \
                         split into two calls if the operations are independent, or \
                         restructure to operate on different sub-places \
                         (e.g. `f(mut {primary_name}.left, mut {primary_name}.right)`)."
                    ),
                )),
                label: Some((primary.span, format!("first `mut {primary_name}` here"))),
            })
        }
        (Exclusive, Move) | (Move, Exclusive) => {
            // Fire once per unordered pair. Emit the diagnostic with
            // the Exclusive claim as the primary span — matches the
            // 6BC.1 behavior tests pinned.
            if j < i {
                return None;
            }
            let mut_span = if matches!(primary.kind, Exclusive) {
                primary.span
            } else {
                other.span
            };
            let move_span = if matches!(primary.kind, Exclusive) {
                other.span
            } else {
                primary.span
            };
            Some(RawDiag {
                code: "E0382",
                message: format!(
                    "cannot move `{primary_name}` and exclusively borrow it in the same call"
                ),
                primary: mut_span,
                suggestion: Some((
                    suggestion_span,
                    String::new(),
                    format!(
                        "the exclusive borrow `mut {primary_name}` claims access for the \
                         duration of the call, which conflicts with the `move {primary_name}` \
                         consumption in the same call. Split into two statements."
                    ),
                )),
                label: Some((move_span, format!("`move {primary_name}` here"))),
            })
        }
        // Shared can't appear in direct claims today (see
        // `check_intra_call_conflicts`); listed for exhaustiveness.
        (Move, Move) | (Shared, _) | (_, Shared) => None,
    }
}

/// Walk `expr` looking for any place expression whose place overlaps
/// `primary`. On the first match, records the overlap kind and the
/// matching sub-expression's span into `found`. Used by 6BC.3 to
/// detect cross-arg shared-read conflicts (E0370, E0381, E0374).
fn scan_overlapping_places(
    expr: &Expr,
    primary: &Place,
    found: &mut Option<(PlaceOverlap, crate::lexer::Span)>,
) {
    if found.is_some() {
        return;
    }
    // Is this expression itself a place that overlaps?
    if let Some(p) = place_from_expr(expr) {
        let o = primary.overlap(&p);
        if !matches!(o, PlaceOverlap::Disjoint) {
            *found = Some((o, expr.span));
            return;
        }
        // Even when this expression has its own place, it may still
        // contain sub-expressions (e.g. `arr[i]` where `i` is itself
        // a place). Fall through to walk children.
    }
    // Recurse into children. We only care about places — operators,
    // calls, struct lits, etc. are walked for their sub-expressions.
    match &expr.kind {
        ExprKind::Call { callee, args, .. } => {
            scan_overlapping_places(callee, primary, found);
            for a in args {
                scan_overlapping_places(a, primary, found);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            scan_overlapping_places(lhs, primary, found);
            scan_overlapping_places(rhs, primary, found);
        }
        ExprKind::Unary { operand, .. } => scan_overlapping_places(operand, primary, found),
        ExprKind::Cast { expr: inner, .. } => scan_overlapping_places(inner, primary, found),
        ExprKind::Field { receiver, .. } => scan_overlapping_places(receiver, primary, found),
        ExprKind::Index { receiver, index } => {
            scan_overlapping_places(receiver, primary, found);
            scan_overlapping_places(index, primary, found);
        }
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            for f in fields {
                scan_overlapping_places(&f.value, primary, found);
            }
        }
        ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
            for el in elements {
                scan_overlapping_places(el, primary, found);
            }
        }
        ExprKind::If { cond, .. } => {
            scan_overlapping_places(cond, primary, found);
            // Block bodies are walked through ordinary apply_block;
            // arg-position if-exprs are admitted by the grammar but
            // their body bindings live in the arm scope, so we don't
            // need to recurse into block contents from here.
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                scan_overlapping_places(s, primary, found);
            }
            if let Some(e) = end {
                scan_overlapping_places(e, primary, found);
            }
        }
        ExprKind::Match { scrutinee, .. } => {
            scan_overlapping_places(scrutinee, primary, found);
        }
        _ => {}
    }
}

/// Slice 6BC.3: build a `Place` from an arbitrary expression. Returns
/// None for expressions that aren't a place (literals, calls, struct
/// constructors, etc.). The walker chases Field and Index projections
/// back to the rooting `Ident`.
///
/// Index projections: a constant integer literal index is recorded as
/// `Projection::Index(n)` so the analyzer can distinguish `arr[3]` from
/// `arr[7]`. Non-constant indices coarsen to `Projection::AnyIndex`,
/// matching Phase 5 design note §5.1's conservative rule for indices
/// borrowck can't const-evaluate.
fn place_from_expr(e: &Expr) -> Option<Place> {
    match &e.kind {
        ExprKind::Ident(name) => Some(Place::root(name)),
        ExprKind::Field { receiver, name } => {
            let mut p = place_from_expr(receiver)?;
            p.projections.push(Projection::Field(name.name.clone()));
            Some(p)
        }
        ExprKind::Index { receiver, index } => {
            let mut p = place_from_expr(receiver)?;
            let proj = match &index.kind {
                ExprKind::IntLit(value, _) => Projection::Index(*value),
                _ => Projection::AnyIndex,
            };
            p.projections.push(proj);
            Some(p)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Binding-type tracking (5BC.2a)
//
// Records the declared type of each binding so the Copy fast path can
// gate move events and E0370 emission. Parameters always have explicit
// types; let-bindings only have types when annotated. Unannotated lets
// stay as Unknown and Copy-gated diagnostics are suppressed for them.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum BindingType {
    /// A type was declared at the binding site.
    Known(Type),
    /// No annotation; sema would have inferred a type but borrowck does
    /// not. Diagnostics that require Copy-gating are suppressed for
    /// these bindings until sema integration lands.
    Unknown,
}

// ---------------------------------------------------------------------------
// Analyzer state
// ---------------------------------------------------------------------------

struct Analyzer<'p> {
    sigs: &'p SigTable,
    oracle: &'p CopyOracle,
    binding_types: HashMap<String, BindingType>,
    /// Unannotated-binding inference (contract follow-through 2026-08-01):
    /// types derived from initializer SHAPE — a call's declared return, a
    /// struct literal's name, an ident's type, a match payload's declared
    /// position. Consulted by `binding_type` as the fallback for Unknown,
    /// so receiver resolution (and every tie built on it) works without
    /// annotations. Declared truth only — never guessed.
    inferred_types: HashMap<String, Type>,
    diags: Vec<RawDiag>,
    /// 5BC.3b: per-place set of currently-live borrower bindings.
    /// Place X is `BorrowedShared(N)` iff `live_borrows[X].len() == N`.
    /// Established by Rule-E1 / Rule-E2 calls in `let` initializers;
    /// released when a borrower goes out of scope or is moved out.
    ///
    /// Phase 11 polish (2026-05-13): each borrower also remembers the
    /// span where the borrow was *established* (the `let` site).
    /// Borrow-conflict diagnostics surface this as a "borrowed here"
    /// secondary label so users see both ends of the conflict.
    live_borrows: BTreeMap<Place, std::collections::BTreeMap<String, Span>>,
    /// 5BC.3b/5BC.4: per-binding back-pointer to every place it borrows
    /// from. `binding_borrows_from[r] == [p1, p2]` means `let r = longest(p1, p2);`
    /// (Rule E3) recorded `r` as borrowing from both `p1` and `p2`. For
    /// Rule E1 / E2 the vec has exactly one entry. Used during scope-exit
    /// cleanup — releasing the borrower decrements every source's
    /// `BorrowedShared(N)` count.
    binding_borrows_from: HashMap<String, Vec<Place>>,
}

/// Diagnostic with a `Span` only; the caller converts to a full
/// `Diagnostic` using the appropriate `LineMap`. Multi-file projects
/// route each diagnostic through the file the offending code lives in
/// (matching sema's approach).
#[derive(Debug, Clone)]
struct RawDiag {
    code: &'static str,
    message: String,
    primary: Span,
    suggestion: Option<(Span, String, String)>, // (span, replacement, description)
    /// Phase 11 polish (2026-05-13): optional secondary span. For
    /// borrow-conflict diagnostics this points at the `let` site that
    /// established the conflicting borrow ("borrowed here") so users
    /// see both ends of the conflict in one diagnostic.
    label: Option<(Span, String)>,
}

impl<'p> Analyzer<'p> {
    fn new(sigs: &'p SigTable, oracle: &'p CopyOracle) -> Self {
        Analyzer {
            sigs,
            oracle,
            binding_types: HashMap::new(),
            inferred_types: HashMap::new(),
            diags: Vec::new(),
            live_borrows: BTreeMap::new(),
            binding_borrows_from: HashMap::new(),
        }
    }

    /// Acquire borrows from one or more places. `borrower` becomes a
    /// live borrower of every place in `places`. The flavor decides
    /// each source place's resulting state:
    ///   - **Shared** → `BorrowedShared(N)` where N is the total
    ///     borrower count (multiple bindings may concurrently shared-
    ///     borrow the same place; Phase 5 5BC.3b / 5BC.4).
    ///   - **Exclusive** → `BorrowedExclusive(borrower)` (6BC.2). Only
    ///     one borrower is permitted; the conflict matrix in design
    ///     note §3.0 says all five conflicting operations are rejected
    ///     while the exclusive borrow is live. Rule E1-mut passes a
    ///     single-element vec; multi-mut (E3-mut, 6BC.4) is forbidden
    ///     in 6BC.2.
    fn acquire_borrows(
        &mut self,
        places: Vec<(Place, BorrowFlavor)>,
        borrower: &str,
        borrower_span: Span,
        state: &mut BTreeMap<Place, PlaceState>,
    ) {
        // Dedup defensively — a buggy classifier could repeat the same
        // place; we don't want it to inflate the BorrowedShared count.
        // Flavor rides per-place (an aggregate can capture a shared view of
        // one owner and an exclusive one of another in the same literal).
        let mut seen = std::collections::BTreeSet::new();
        let unique: Vec<(Place, BorrowFlavor)> = places
            .into_iter()
            .filter(|(p, _)| seen.insert(p.clone()))
            .collect();
        self.binding_borrows_from.insert(
            borrower.to_string(),
            unique.iter().map(|(p, _)| p.clone()).collect(),
        );
        for (place, flavor) in unique {
            let set = self.live_borrows.entry(place.clone()).or_default();
            set.insert(borrower.to_string(), borrower_span);
            let new_state = match flavor {
                BorrowFlavor::Shared => PlaceState::BorrowedShared(set.len() as u32),
                BorrowFlavor::Exclusive => PlaceState::BorrowedExclusive(borrower.to_string()),
            };
            state.insert(place, new_state);
        }
    }

    /// Contract §5: per-position keeps flags for a method reached through a
    /// receiver of the given declared type. Path receivers use the entry's
    /// effective flags (declared ∪ computed). Generic receivers substitute
    /// the type arguments into the declared param types first — the
    /// `Vec[str]` route — and gate on the declared `#[keeps(this)]` only
    /// (the flow pass skips generic impls). Returns None when nothing ties.
    fn keeps_flags_for_receiver_ty(&self, kind: &TypeKind, method: &str) -> Option<Vec<bool>> {
        match kind {
            TypeKind::Path(t) => {
                let entry = self.sigs.method_entry(t, method)?;
                let keeps = SigTable::effective_keeps(entry);
                keeps.iter().any(|b| *b).then_some(keeps)
            }
            TypeKind::Generic { name, args } => {
                let entry = self.sigs.methods.get(&format!("{name}.{method}"))?;
                let params = self.sigs.impl_generics.get(name)?;
                let map: HashMap<String, Type> = params
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                // Declared keeps ties every view-typed position; computed
                // bits tie their own positions. Both are gated by the
                // SUBSTITUTED type — GenHolder[str].set ties, GenHolder[i32]
                // does not, with or without the attribute.
                let keeps: Vec<bool> = entry
                    .param_tys
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        // A `ref` parameter is a borrow whatever it is a
                        // borrow OF, so the view gate does not apply to it
                        // (see `effective_keeps`).
                        if entry.keeps_this && entry.param_muts.get(i).copied().unwrap_or(false) {
                            return true;
                        }
                        (entry.keeps_this
                            || entry.computed_keeps.get(i).copied().unwrap_or(false))
                            && self.oracle.type_contains_view(&subst_type(t, &map))
                    })
                    .collect();
                keeps.iter().any(|b| *b).then_some(keeps)
            }
            _ => None,
        }
    }

    /// Memory-model contract §5: like `acquire_borrows`, but UNIONS into
    /// the borrower's existing back-pointer list instead of replacing it.
    /// A `#[keeps(this)]` receiver accumulates a borrow per call
    /// (`names.push(a); names.push(b);` pins both owners); replacing the
    /// list would leak the earlier edge at release time and leave a stale
    /// `live_borrows` entry to false-fire E0372/E0514 later.
    fn extend_borrows(
        &mut self,
        places: Vec<(Place, BorrowFlavor)>,
        borrower: &str,
        borrower_span: Span,
        state: &mut BTreeMap<Place, PlaceState>,
    ) {
        let mut seen = std::collections::BTreeSet::new();
        let unique: Vec<(Place, BorrowFlavor)> = places
            .into_iter()
            .filter(|(p, _)| seen.insert(p.clone()))
            .collect();
        let back = self
            .binding_borrows_from
            .entry(borrower.to_string())
            .or_default();
        for (p, _) in &unique {
            if !back.contains(p) {
                back.push(p.clone());
            }
        }
        for (place, flavor) in unique {
            let set = self.live_borrows.entry(place.clone()).or_default();
            set.insert(borrower.to_string(), borrower_span);
            let new_state = match flavor {
                BorrowFlavor::Shared => PlaceState::BorrowedShared(set.len() as u32),
                BorrowFlavor::Exclusive => PlaceState::BorrowedExclusive(borrower.to_string()),
            };
            state.insert(place, new_state);
        }
    }

    /// Release a single borrow held by `borrower` on `place`. If this
    /// was the last borrow, `place` returns to `Owned`; otherwise the
    /// state decrements to `BorrowedShared(n-1)`.
    fn release_borrow(
        &mut self,
        place: &Place,
        borrower: &str,
        state: &mut BTreeMap<Place, PlaceState>,
    ) {
        let n_after = if let Some(set) = self.live_borrows.get_mut(place) {
            set.remove(borrower);
            set.len() as u32
        } else {
            return;
        };
        if n_after == 0 {
            self.live_borrows.remove(place);
            if state.contains_key(place) {
                state.insert(place.clone(), PlaceState::Owned);
            }
        } else if state.contains_key(place) {
            state.insert(place.clone(), PlaceState::BorrowedShared(n_after));
        }
    }

    /// Release every borrow `borrower` is currently holding. Called
    /// when a borrowing binding goes out of scope or is moved.
    fn drop_borrower(&mut self, borrower: &str, state: &mut BTreeMap<Place, PlaceState>) {
        if let Some(places) = self.binding_borrows_from.remove(borrower) {
            for place in places {
                self.release_borrow(&place, borrower, state);
            }
        }
    }

    /// Classify a `let`-initializer expression for borrow-acquisition.
    /// Returns the set of places the result-binding borrows from plus the
    /// flavor (shared vs exclusive). Empty vec means "no rule applied";
    /// the flavor in that case is meaningless and defaults to Shared.
    ///
    /// Rules (each maps to the elision detected at SigTable-collect time):
    ///   * **5BC.3b / Rule E1**: shared single-param → one-element vec.
    ///   * **5BC.3b / Rule E2**: shared self-method → receiver place.
    ///   * **5BC.4 / Rule E3**: shared multi-param → one entry per param.
    ///   * **6BC.2 / Rule E1-mut**: exclusive single-`ref`-param → one entry.
    ///   * **6BC.2 / Rule E2-mut**: exclusive `ref this` method → receiver place.
    fn classify_borrow_source(&self, e: &Expr) -> Vec<(Place, BorrowFlavor)> {
        match &e.kind {
            ExprKind::Call { callee, args, .. } => self.classify_call_borrow(callee, args),
            // Rule E-VIEW aggregate arm (2026-07-07): a view produced inside
            // an aggregate literal escapes into the aggregate binding, so
            // `let w: Slot = Slot { s: t.view() };` must record `w` as a
            // borrower of `t` exactly like the direct `let s: str = t.view();`
            // form. Before this arm the owner stayed `Owned` and could be
            // moved/dropped while `w.s` still pointed into it (safe-code
            // use-after-free). Recurses so nested aggregates compose.
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => fields
                .iter()
                .flat_map(|f| self.classify_borrow_source(&f.value))
                .collect(),
            ExprKind::ArrayLit { elements }
            | ExprKind::TupleLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. } => elements
                .iter()
                .flat_map(|el| self.classify_borrow_source(el))
                .collect(),
            ExprKind::ArrayFill { fill, .. } => self.classify_borrow_source(fill),
            // Rule E-VIEW through control-flow *expressions* (2026-07-22): an
            // `if` / block / `match` used in value position produces its value
            // from a branch tail, and that tail may be a view. Union the
            // borrow sources of every value-producing arm so
            // `let s = if c { t.view() } else { "" };` records `s` as a
            // borrower of `t` exactly like the direct `let s = t.view();`
            // form. Conservative on multi-arm: pin *every* possible owner.
            // Before this arm these landed in `_ => Vec::new()`, leaving the
            // owner `Owned` and movable out from under the live view (UAF).
            ExprKind::If {
                then, else_branch, ..
            } => {
                let mut v = then
                    .tail
                    .as_ref()
                    .map(|t| self.classify_borrow_source(t))
                    .unwrap_or_default();
                if let Some(eb) = else_branch {
                    v.extend(self.classify_borrow_source(eb));
                }
                v
            }
            ExprKind::Block(b) => b
                .tail
                .as_ref()
                .map(|t| self.classify_borrow_source(t))
                .unwrap_or_default(),
            ExprKind::Match { arms, .. } => arms
                .iter()
                .flat_map(|a| self.classify_borrow_source(&a.body))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The call arm of `classify_borrow_source`: free fns with a detected
    /// return-borrow elision, view-returning methods (`SelfReceiver`), and
    /// non-generic enum-variant constructors (`Enum::Variant(payload)`, which
    /// parse as a `Call` with a `Path` callee and capture payload views like
    /// any aggregate).
    fn classify_call_borrow(&self, callee: &Expr, args: &[Expr]) -> Vec<(Place, BorrowFlavor)> {
        match &callee.kind {
            ExprKind::Ident(fn_name) => {
                let Some(entry) = self.sigs.fns.get(fn_name) else {
                    return Vec::new();
                };
                let Some(rb) = entry.return_borrow.as_ref() else {
                    return Vec::new();
                };
                let flavor = entry.return_borrow_flavor.unwrap_or(BorrowFlavor::Shared);
                // A non-place argument in a borrowed position may itself be a
                // view-producing expression (`first(t.view())`): the result
                // then borrows whatever the argument borrows, so recurse
                // instead of dropping the source on the floor.
                let arg_sources = |idx: usize| -> Vec<(Place, BorrowFlavor)> {
                    match place_from_arg(args, idx) {
                        Some(p) => vec![(p, flavor)],
                        None => args
                            .get(idx)
                            .map(|a| self.classify_borrow_source(a))
                            .unwrap_or_default(),
                    }
                };
                match rb {
                    ReturnBorrowSource::Param(idx) => arg_sources(*idx as usize),
                    ReturnBorrowSource::MultiParam(indices) => indices
                        .iter()
                        .flat_map(|&idx| arg_sources(idx as usize))
                        .collect(),
                    // `SelfReceiver` doesn't apply to free-function calls.
                    ReturnBorrowSource::SelfReceiver => Vec::new(),
                }
            }
            ExprKind::Field {
                receiver,
                name: method_name,
            } => {
                // Any place expression works as a receiver (`t.view()`,
                // `holder.field.view()`); the borrow lands on the receiver
                // place, so moving any prefix of it conflicts via the
                // partial-place overlap rules. Non-place receivers (chained
                // calls) stay untracked — typing them needs return-type info
                // this table doesn't carry.
                let Some(place) = place_from_expr(receiver) else {
                    return Vec::new();
                };
                let Some(type_name) = self.place_type_name(receiver) else {
                    return Vec::new();
                };
                let Some(entry) = self.sigs.method_entry(&type_name, &method_name.name) else {
                    return Vec::new();
                };
                let flavor = entry.return_borrow_flavor.unwrap_or(BorrowFlavor::Shared);
                match entry.return_borrow.as_ref() {
                    Some(ReturnBorrowSource::SelfReceiver) => vec![(place, flavor)],
                    _ => Vec::new(),
                }
            }
            // `Enum::Variant(payload)` — payload views escape into the value.
            ExprKind::Path { segments } => {
                let is_enum_ctor =
                    segments.len() == 2 && self.sigs.enums.contains(&segments[0].name);
                if !is_enum_ctor {
                    return Vec::new();
                }
                args.iter()
                    .flat_map(|a| self.classify_borrow_source(a))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Resolve the declared type NAME of a place expression: a bare binding
    /// via `binding_types`, a field path by walking the SigTable's struct
    /// field types. Returns `None` (conservative: no borrow recorded) for
    /// anything it cannot follow — unannotated bindings, generic
    /// instantiations, index projections.
    fn place_type_name(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(name) => Self::type_name_of(&self.binding_type(name)?.kind),
            ExprKind::Field { receiver, name } => {
                let recv_ty = self.place_type_name(receiver)?;
                let fty = self.sigs.struct_fields.get(&recv_ty)?.get(&name.name)?;
                Self::type_name_of(&fty.kind)
            }
            _ => None,
        }
    }

    /// Is `e` a METHOD reference — `recv.method` where `method` names no field
    /// of `recv`'s struct — rather than a field read?
    ///
    /// This is how a bound method reference is written, and it is not a read of
    /// the receiver at all: the pair it becomes is a synthesized bridge fn plus
    /// `#addr_of(recv)`. Treating it as a read made
    /// `run_job(this, then: this.on_created)` a partial-place conflict with its
    /// own `ref` argument (E0374) — the two "conflicting" places being the same
    /// pointer by construction. Only visible when the receiver is non-Copy,
    /// because a Copy root carries no claim, which is why it looked like it
    /// depended on an unrelated `drop` impl.
    ///
    /// Conservative: an unresolvable receiver type answers `false`, so an
    /// unknown shape is still treated as a read.
    fn is_method_ref(&self, e: &Expr) -> bool {
        let ExprKind::Field { receiver, name } = &e.kind else {
            return false;
        };
        let Some(recv_ty) = self.place_type_name(receiver) else {
            return false;
        };
        let Some(fields) = self.sigs.struct_fields.get(&recv_ty) else {
            return false;
        };
        !fields.contains_key(&name.name)
    }

    /// The lookup key a type contributes to `SigTable::methods`
    /// (`"{name}.{method}"`). A bare `Path` uses its own name; a **generic
    /// instantiation** (`Vec[i32]`, `Pair[A, B]`) uses its base `name` —
    /// the same string the generic `impl Vec[T]` block registers its methods
    /// under (both are pre-mono AST names). Handling `Generic` here is what
    /// lets Rule E-VIEW fire for `v.as_slice()` on a `Vec[T]` receiver; before
    /// this it returned `None` and no borrow was recorded (a UAF: the `Vec`
    /// could be moved/reallocated out from under a live slice view).
    fn type_name_of(kind: &TypeKind) -> Option<String> {
        match kind {
            TypeKind::Path(p) => Some(p.clone()),
            TypeKind::Generic { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Looks up the binding's recorded type. `None` if the binding is
    /// `Unknown` or wasn't tracked (e.g. introduced inside a sub-expression
    /// before its `let` was recorded — should not happen in well-formed
    /// programs).
    fn binding_type(&self, name: &str) -> Option<&Type> {
        match self.binding_types.get(name) {
            Some(BindingType::Known(t)) => Some(t),
            Some(BindingType::Unknown) | None => self.inferred_types.get(name),
        }
    }

    /// Structural type inference for initializer expressions — see
    /// [`infer_expr_type_with`]; this binds it to the walker's own
    /// two-tier binding map.
    fn infer_expr_type(&self, e: &Expr) -> Option<Type> {
        infer_expr_type_with(e, self.sigs, &|n| self.binding_type(n).cloned())
    }

    /// Unannotated-binding inference, match-payload arm: bind each
    /// `PatternKind::Binding` in a variant pattern to its declared payload
    /// type. Generic enums substitute the pattern's explicit type args
    /// when present, else the scrutinee's inferred `Generic` args.
    fn register_pattern_types(&mut self, pat: &Pattern, scrutinee: &Expr) {
        let PatternKind::Variant {
            enum_name,
            type_args,
            variant_name,
            payload,
        } = &pat.kind
        else {
            return;
        };
        let Some(variants) = self.sigs.enum_payloads.get(&enum_name.name) else {
            return;
        };
        let Some(ptys) = variants.get(&variant_name.name) else {
            return;
        };
        let map: HashMap<String, Type> = match self.sigs.enum_generics.get(&enum_name.name) {
            None => HashMap::new(),
            Some(gp) => {
                let args: Vec<Type> = if !type_args.is_empty() {
                    type_args.clone()
                } else {
                    match self.infer_expr_type(scrutinee).map(|t| t.kind) {
                        Some(TypeKind::Generic { name, args }) if name == enum_name.name => args,
                        _ => return,
                    }
                };
                gp.iter().cloned().zip(args).collect()
            }
        };
        let hits: Vec<(String, Type)> = payload
            .iter()
            .zip(ptys.iter())
            .filter_map(|(bp, t)| match &bp.kind {
                PatternKind::Binding(id) => Some((id.name.clone(), subst_type(t, &map))),
                _ => None,
            })
            .collect();
        for (n, t) in hits {
            self.inferred_types.insert(n, t);
        }
    }

    /// True iff we know the binding's type AND that type is provably
    /// non-Copy. The gate for E0370 and for Owned→Moved transitions.
    fn binding_is_non_copy(&self, name: &str) -> bool {
        match self.binding_type(name) {
            Some(t) => self.oracle.definitely_non_copy(t),
            None => false,
        }
    }
}

/// Structural type inference for expressions. Only declared facts flow
/// through: fn/method return types (generic receivers substituted),
/// struct-literal names, idents, casts, literals, value-producing
/// control-flow tails. `None` means "no declared truth reachable" — the
/// expression stays untyped and every type-gated diagnostic keeps skipping
/// it.
///
/// `lookup` resolves a binding name to its recorded type. The flow walker
/// and the view rules keep their bindings in different shapes (a two-tier
/// declared/inferred map vs. a lexical scope stack) and differ in nothing
/// else, so the inference itself lives here once.
fn infer_expr_type_with(
    e: &Expr,
    sigs: &SigTable,
    lookup: &dyn Fn(&str) -> Option<Type>,
) -> Option<Type> {
    let path = |n: &str| Type {
        kind: TypeKind::Path(n.to_string()),
        span: e.span,
    };
    match &e.kind {
        ExprKind::Ident(n) => lookup(n),
        ExprKind::StrLit(_) => Some(path("str")),
        ExprKind::Cast { ty, .. } => Some(ty.clone()),
        ExprKind::StructLit { name, .. } => Some(path(&name.name)),
        ExprKind::GenericStructLit {
            name, type_args, ..
        } => Some(Type {
            kind: TypeKind::Generic {
                name: name.name.clone(),
                args: type_args.clone(),
            },
            span: e.span,
        }),
        ExprKind::GenericEnumCall {
            enum_name,
            type_args,
            ..
        } => Some(Type {
            kind: TypeKind::Generic {
                name: enum_name.name.clone(),
                args: type_args.clone(),
            },
            span: e.span,
        }),
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Ident(f) => sigs.fns.get(f).and_then(|e2| e2.ret_ty.clone()),
            // `Type::assoc(...)`. `BorrowCk::infer_ty` already answers this
            // for a TOP-LEVEL call; the recursive walk needs it too, or a
            // chain rooted in a constructor (`Buf::new().view()…`) goes
            // untyped from the second link on and every receiver-keyed rule
            // silently declines to fire.
            ExprKind::Path { segments } => {
                let [ty, assoc] = segments.as_slice() else {
                    return None;
                };
                sigs.methods
                    .get(&format!("{}.{}", ty.name, assoc.name))
                    .and_then(|e2| e2.ret_ty.clone())
            }
            ExprKind::Field {
                receiver,
                name: method,
            } => {
                let recv_ty = infer_expr_type_with(receiver, sigs, lookup)?;
                match &recv_ty.kind {
                    TypeKind::Path(t) => sigs
                        .method_entry(t, &method.name)
                        .and_then(|e2| e2.ret_ty.clone()),
                    TypeKind::Generic { name, args } => {
                        let entry = sigs.methods.get(&format!("{name}.{}", method.name))?;
                        let params = sigs.impl_generics.get(name)?;
                        let map: HashMap<String, Type> =
                            params.iter().cloned().zip(args.iter().cloned()).collect();
                        entry.ret_ty.as_ref().map(|t| subst_type(t, &map))
                    }
                    _ => None,
                }
            }
            _ => None,
        },
        ExprKind::Field { receiver, name } => {
            let recv_ty = infer_expr_type_with(receiver, sigs, lookup)?;
            let TypeKind::Path(t) = &recv_ty.kind else {
                return None;
            };
            sigs.struct_fields
                .get(t)
                .and_then(|fs| fs.get(&name.name).cloned())
        }
        ExprKind::Block(b) => b
            .tail
            .as_ref()
            .and_then(|t| infer_expr_type_with(t, sigs, lookup)),
        ExprKind::If { then, .. } => then
            .tail
            .as_ref()
            .and_then(|t| infer_expr_type_with(t, sigs, lookup)),
        ExprKind::Match { arms, .. } => arms
            .first()
            .and_then(|a| infer_expr_type_with(&a.body, sigs, lookup)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Memory-model contract §3.1 — view escapes, judged at the definition.
//
// Ported from sema (issue-07): sema used to emit E0513/E0515/E0516 and
// hand-encode where it believed this pass would tie instead. Two passes
// co-owning one rule family, synchronized by belief, is a silent-unsoundness
// seam — if the flow pass's coverage shrinks, the skip conditions stay and
// nothing denies. Here the two are one analysis: the flow pass ties, or the
// rule denies.
// ---------------------------------------------------------------------------

/// What the view rules need to know about one binding in scope.
#[derive(Debug, Clone)]
struct ViewLocal {
    /// Declared or structurally inferred type; `None` when nothing
    /// declared reaches it (every type-gated rule then skips the binding).
    ty: Option<Type>,
    /// True iff this binding OWNS the storage it names — a `let`/`var`
    /// local, a `take` parameter, or a `take this` receiver. False for a
    /// bare or `ref` parameter and a `this` / `ref this` receiver, which
    /// name storage the caller owns and outlives the frame with. This is
    /// the gate the whole family turns on: only an owner's storage dies at
    /// return.
    owns_value: bool,
    /// For a view-shaped binding, the owners its bytes come from
    /// (`let s = t.view();` records `t`). Empty means literal / static /
    /// untraceable provenance. Consulted so an ALIAS of a view is judged
    /// against the real owner, not against itself.
    borrow_roots: BTreeSet<String>,
}

/// The enclosing definition, in the terms the E0515 question asks about:
/// did the flow pass export this store, so that call sites tie?
enum ViewSite {
    /// A free function. `exported` is false when its ADDRESS is taken —
    /// indirect calls through a fn pointer carry no computed flows, so
    /// nothing at the call site can apply them.
    Free { name: String, exported: bool },
    /// A method, keyed `Type.method` for the signature table.
    Method { key: String },
}

/// The definition-site view rules. One instance per function/method body.
struct ViewRules<'a> {
    sigs: &'a SigTable,
    oracle: &'a CopyOracle,
    /// Module `static`s by name — the sinks with no owner to tie to.
    statics: &'a HashMap<String, Type>,
    /// `(Type, method)` pairs whose returned value carries the address of
    /// its receiver — the program-wide fixpoint the capture rules read.
    capturing: &'a BTreeSet<(String, String)>,
    /// Per-body capture taint: binding name -> the locals whose address the
    /// value it holds carries. Flat, not scoped: a capture that reached a
    /// binding stays reachable through it for the rest of the body.
    capture_taint: BTreeMap<String, Vec<String>>,
    /// Lexical scope stack; scope 0 holds the receiver and parameters.
    scopes: Vec<HashMap<String, ViewLocal>>,
    /// Parameter (and receiver) names, for phrasing owner descriptions and
    /// for recognizing a store whose source is a caller-owned view.
    param_names: HashSet<String>,
    /// Parameter positions, so a store can ask the flow pass whether THIS
    /// source reached THAT sink.
    param_index: HashMap<String, usize>,
    /// Write targets that outlive the frame: `ref` parameters and a
    /// `ref this` receiver, which alias the caller's storage.
    ref_targets: HashSet<String>,
    return_ty: Option<Type>,
    site: ViewSite,
    /// Contract §5: the definition carries some `#[keeps(...)]` — it has
    /// declared its flows, which is the whole ask at an opaque boundary.
    declares_flows: bool,
    diags: Vec<RawDiag>,
}

impl<'a> ViewRules<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        sigs: &'a SigTable,
        oracle: &'a CopyOracle,
        statics: &'a HashMap<String, Type>,
        capturing: &'a BTreeSet<(String, String)>,
        site: ViewSite,
        receiver: Option<Receiver>,
        receiver_ty: Option<Type>,
        params: &[Param],
        return_ty: Option<Type>,
        attributes: &[Attribute],
    ) -> Self {
        let mut base: HashMap<String, ViewLocal> = HashMap::new();
        let mut param_names = HashSet::new();
        let mut param_index = HashMap::new();
        let mut ref_targets = HashSet::new();
        if let Some(r) = receiver {
            param_names.insert("self".to_string());
            if r == Receiver::Mut {
                // `ref this` aliases the caller's receiver — a write target
                // that outlives the call.
                ref_targets.insert("self".to_string());
            }
            base.insert(
                "self".to_string(),
                ViewLocal {
                    ty: receiver_ty,
                    owns_value: r == Receiver::Move,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        for (i, p) in params.iter().enumerate() {
            param_names.insert(p.name.name.clone());
            param_index.insert(p.name.name.clone(), i);
            // `ref x: T` borrows the caller's storage and writes back.
            if p.mutable && !p.move_ {
                ref_targets.insert(p.name.name.clone());
            }
            base.insert(
                p.name.name.clone(),
                ViewLocal {
                    ty: Some(p.ty.clone()),
                    owns_value: p.move_,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        ViewRules {
            sigs,
            oracle,
            statics,
            capturing,
            capture_taint: BTreeMap::new(),
            scopes: vec![base],
            param_names,
            param_index,
            ref_targets,
            return_ty,
            site,
            declares_flows: crate::attrs::has_keeps(attributes, "this")
                || crate::attrs::has_keeps(attributes, "nothing"),
            diags: Vec::new(),
        }
    }

    /// `this` and `self` name the same binding; every lookup goes through
    /// the receiver's canonical name.
    fn canonical(name: &str) -> &str {
        if name == "this" {
            "self"
        } else {
            name
        }
    }

    fn lookup(&self, name: &str) -> Option<&ViewLocal> {
        let key = Self::canonical(name);
        self.scopes.iter().rev().find_map(|s| s.get(key))
    }

    fn bind(&mut self, name: &str, local: ViewLocal) {
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .insert(Self::canonical(name).to_string(), local);
    }

    /// Re-point a live binding's view provenance. A plain `=` replaces it:
    /// the binding now reads the new value's bytes and nothing else. Branch
    /// merges union instead — see `walk_expr`'s `If` / `Match` arms.
    fn set_roots(&mut self, name: &str, roots: BTreeSet<String>) {
        let key = Self::canonical(name);
        for scope in self.scopes.iter_mut().rev() {
            if let Some(l) = scope.get_mut(key) {
                l.borrow_roots = roots;
                return;
            }
        }
    }

    /// Union another path's view provenance into the current state. The
    /// contract is flow-insensitive about conditions (§2): a flow that
    /// happens on any path counts as happening, so a dangling branch cannot
    /// hide behind a sound one.
    fn union_roots_from(&mut self, other: &[HashMap<String, ViewLocal>]) {
        for (depth, scope) in other.iter().enumerate() {
            let Some(mine) = self.scopes.get_mut(depth) else {
                break;
            };
            for (name, l) in scope {
                if let Some(m) = mine.get_mut(name) {
                    m.borrow_roots.extend(l.borrow_roots.iter().cloned());
                }
            }
        }
    }

    /// Types resolve through the scope stack first, then module `static`s —
    /// a `static` is a place like any other, and store targets name them.
    ///
    /// An associated call (`Buf::new()`) is resolved here rather than in the
    /// shared inference: it is how an unnamed owner is most often produced,
    /// and the rest of the pass has no reason to start typing paths.
    fn infer_ty(&self, e: &Expr) -> Option<Type> {
        if let Some(t) = self.assoc_call_ret_ty(e) {
            return Some(t);
        }
        infer_expr_type_with(e, self.sigs, &|n| {
            self.lookup(n)
                .and_then(|l| l.ty.clone())
                .or_else(|| self.statics.get(Self::canonical(n)).cloned())
        })
    }

    /// The declared return type of `Type::assoc(...)`.
    fn assoc_call_ret_ty(&self, e: &Expr) -> Option<Type> {
        let ExprKind::Call { callee, .. } = &e.kind else {
            return None;
        };
        let ExprKind::Path { segments } = &callee.kind else {
            return None;
        };
        let [ty, assoc] = segments.as_slice() else {
            return None;
        };
        self.sigs
            .methods
            .get(&format!("{}.{}", ty.name, assoc.name))?
            .ret_ty
            .clone()
    }

    /// Does a value of this type carry a borrow? The oracle's answer, plus
    /// tuples: the oracle classifies the named types the rest of the pass
    /// reasons about, and a tuple has no name until monomorphize synthesizes
    /// its struct — but `(str, i32)` transports a view today, and a returned
    /// one launders it out of the frame. Kept here rather than widened in
    /// the oracle so no existing rule starts tying on a shape it never did.
    fn carries_view(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Tuple(elems) => elems.iter().any(|t| self.carries_view(t)),
            _ => self.oracle.type_contains_view(ty),
        }
    }

    /// A view proper: `str` or a slice. (View-CARRYING aggregates are a
    /// separate question — `carries_view`.)
    fn is_view_ty(ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::Slice(_) => true,
            TypeKind::Path(n) => n == "str",
            _ => false,
        }
    }

    /// True iff `receiver.method(...)` produces a view that borrows the
    /// receiver. The answer is the flow pass's own `detect_method_view`
    /// verdict — a borrow receiver plus a view-carrying return, recorded as
    /// `SelfReceiver` — filtered to returns that actually carry a view, so
    /// Rule E2's non-Copy-aggregate returns don't count. Sema used to
    /// re-derive this from its own tables; one answer, one place.
    fn method_produces_view(&self, receiver: &Expr, method: &str) -> bool {
        let Some(ty) = self.infer_ty(receiver) else {
            return false;
        };
        let base = match &ty.kind {
            TypeKind::Path(n) => n.clone(),
            TypeKind::Generic { name, .. } => name.clone(),
            _ => return false,
        };
        let Some(entry) = self.sigs.method_entry(&base, method) else {
            return false;
        };
        if entry.return_borrow != Some(ReturnBorrowSource::SelfReceiver) {
            return false;
        }
        entry
            .ret_ty
            .as_ref()
            .is_some_and(|r| self.carries_view(r))
    }

    /// The set of root bindings a view expression borrows from, traced by
    /// SHAPE. Covers place chains, a view-producing method call, a free-fn
    /// call whose signature returns a borrow of its parameters, aggregate
    /// literals, and control-flow expressions (the union of every
    /// value-producing arm, so a dangling arm cannot hide behind a sound
    /// one). A bare `Ident` is a root itself; alias expansion happens in
    /// `borrow_roots_of`.
    fn view_source_roots(&self, e: &Expr) -> BTreeSet<String> {
        let mut roots = BTreeSet::new();
        match &e.kind {
            ExprKind::Ident(n) => {
                roots.insert(n.clone());
            }
            ExprKind::Field { receiver, .. } | ExprKind::Index { receiver, .. } => {
                roots.extend(self.view_source_roots(receiver));
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => {
                roots.extend(self.view_source_roots(operand));
            }
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    roots.extend(self.view_source_roots(&f.value));
                }
            }
            ExprKind::ArrayLit { elements }
            | ExprKind::TupleLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. } => {
                for el in elements {
                    roots.extend(self.view_source_roots(el));
                }
            }
            ExprKind::ArrayFill { fill, .. } => {
                roots.extend(self.view_source_roots(fill));
            }
            ExprKind::Call { callee, args, .. } => match &callee.kind {
                ExprKind::Field { receiver, name } => {
                    if self.method_produces_view(receiver, &name.name) {
                        roots.extend(self.view_source_roots(receiver));
                    }
                }
                ExprKind::Ident(fn_name) => {
                    match self.sigs.fns.get(fn_name).and_then(|f| f.return_borrow.as_ref()) {
                        Some(ReturnBorrowSource::Param(i)) => {
                            if let Some(a) = args.get(*i as usize) {
                                roots.extend(self.view_source_roots(a));
                            }
                        }
                        Some(ReturnBorrowSource::MultiParam(is)) => {
                            for &i in is {
                                if let Some(a) = args.get(i as usize) {
                                    roots.extend(self.view_source_roots(a));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            ExprKind::If {
                then, else_branch, ..
            } => {
                if let Some(t) = &then.tail {
                    roots.extend(self.view_source_roots(t));
                }
                if let Some(eb) = else_branch {
                    roots.extend(self.view_source_roots(eb));
                }
            }
            ExprKind::Block(b) => {
                if let Some(t) = &b.tail {
                    roots.extend(self.view_source_roots(t));
                }
            }
            ExprKind::Match { arms, .. } => {
                for a in arms {
                    roots.extend(self.view_source_roots(&a.body));
                }
            }
            _ => {}
        }
        roots
    }

    /// `view_source_roots` with aliases expanded: a view binding that
    /// itself borrows other owners (`let s = t.view(); return s;`)
    /// contributes those owners, not its own name.
    fn borrow_roots_of(&self, e: &Expr) -> BTreeSet<String> {
        let mut roots = BTreeSet::new();
        for root in self.view_source_roots(e) {
            match self.lookup(&root) {
                Some(l) if !l.borrow_roots.is_empty() => {
                    roots.extend(l.borrow_roots.iter().cloned());
                }
                _ => {
                    roots.insert(root);
                }
            }
        }
        roots
    }

    /// True iff a view rooted at `root` reads storage that this frame frees
    /// on the way out — the `owns_value` gate, plus non-Copy (a Copy root
    /// owns no heap to free).
    fn root_dies_at_return(&self, root: &str) -> bool {
        match self.lookup(root) {
            Some(l) => {
                l.owns_value
                    && l.ty
                        .as_ref()
                        .is_some_and(|t| !self.oracle.is_copy(t))
            }
            None => false,
        }
    }

    /// Human description of an owner root — a `take this` receiver, a
    /// `take` parameter, or a plain local — so the fix is obvious from the
    /// message.
    fn owner_desc(&self, root: &str) -> String {
        if root == "self" || root == "this" {
            "the `take this` receiver".to_string()
        } else if self.param_names.contains(root) {
            format!("`take` parameter `{root}`")
        } else {
            format!("local `{root}`")
        }
    }

    fn err(&mut self, code: &'static str, message: String, primary: Span) {
        self.diags.push(RawDiag {
            code,
            message,
            primary,
            suggestion: None,
            label: None,
        });
    }

    // -- the rules ---------------------------------------------------------

    /// Contract §3.1: a view (or view-carrying value) returned from the
    /// frame may not be rooted in storage the frame frees. The gate is the
    /// root's `owns_value` flag, not "is it a parameter": a plain local, a
    /// `take` parameter, and a `take this` receiver ALL own their storage
    /// and drop it at return, so a returned view of any of them dangles. A
    /// bare / `ref` parameter and a `this` / `ref this` receiver borrow
    /// caller-owned storage, so a view of them is caller-tied and sound.
    fn check_return(&mut self, e: &Expr) {
        let Some(ret) = self.return_ty.clone() else {
            return;
        };
        let ret_is_view = Self::is_view_ty(&ret);
        let ret_carries_view = !ret_is_view && self.carries_view(&ret);
        if !(ret_is_view || ret_carries_view) {
            return;
        }
        if ret_is_view {
            self.check_view_of_temp(e);
        }
        // A root the return MOVES out does not die here — its storage
        // transfers to the caller. `return out;` where `out: Vec[str]` is
        // the returned value itself, not a view of something the frame
        // frees. Only roots the returned value BORROWS from can dangle.
        //
        // Never when the return type is a view: `-> str` with `return s;`
        // for an owning `s` is a COERCION, not a move. The owner stays
        // behind and is dropped, and the view handed back is exactly the
        // dangle this rule exists for.
        let moved_out = if ret_is_view {
            BTreeSet::new()
        } else {
            Self::moved_out_roots(e)
        };
        for root in self.borrow_roots_of(e) {
            if moved_out.contains(&root) {
                continue;
            }
            if !self.root_dies_at_return(&root) {
                continue;
            }
            let msg = if ret_is_view {
                format!(
                    "cannot return a borrow of {}: it owns heap that is freed when the function returns, so the returned view would dangle. Return an owned value (`Text` / `Vec[T]`) instead, or borrow from a non-`take` parameter",
                    self.owner_desc(&root)
                )
            } else {
                format!(
                    "the returned value holds a view of {}: it owns heap that is freed when the function returns, so the stored view would dangle. Store an owned `Text` / `Vec[T]` in the field, or borrow the view from a non-`take` parameter",
                    self.owner_desc(&root)
                )
            };
            self.err("E0513", msg, e.span);
            return;
        }
    }

    /// The bindings a return expression hands over WHOLE, so that the
    /// callee's storage becomes the caller's rather than dying at the
    /// return. A bare binding moves; so does one named directly as an
    /// aggregate's field or element (`return Surface { reg: reg }` gives
    /// `reg` away), and so does each arm of a control-flow expression that
    /// yields one, mirroring `view_source_roots`.
    ///
    /// A projection is deliberately absent: `return out.field;` copies or
    /// borrows a part and leaves `out` behind to be dropped, which is the
    /// escape the rule is looking for. So is any computed leaf — `Sink {
    /// key: t.view() }` borrows from `t` and hands nothing over, which is
    /// why it still fires.
    ///
    /// A `Copy` leaf needs no special case: `root_dies_at_return` already
    /// requires a non-Copy owner, so copying a `str` into the returned
    /// aggregate never reached this question.
    fn moved_out_roots(e: &Expr) -> BTreeSet<String> {
        let mut roots = BTreeSet::new();
        match &e.kind {
            ExprKind::Ident(n) => {
                roots.insert(n.clone());
            }
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    if let ExprKind::Ident(n) = &f.value.kind {
                        roots.insert(n.clone());
                    }
                }
            }
            ExprKind::ArrayLit { elements }
            | ExprKind::TupleLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. } => {
                for el in elements {
                    if let ExprKind::Ident(n) = &el.kind {
                        roots.insert(n.clone());
                    }
                }
            }
            ExprKind::Block(b) => {
                if let Some(t) = &b.tail {
                    roots.extend(Self::moved_out_roots(t));
                }
            }
            ExprKind::If {
                then, else_branch, ..
            } => {
                if let Some(t) = &then.tail {
                    roots.extend(Self::moved_out_roots(t));
                }
                if let Some(eb) = else_branch {
                    roots.extend(Self::moved_out_roots(eb));
                }
            }
            ExprKind::Match { arms, .. } => {
                for a in arms {
                    roots.extend(Self::moved_out_roots(&a.body));
                }
            }
            _ => {}
        }
        roots
    }

    /// Contract §3.1, the aggregate half: a view LEAF built inside a
    /// returned aggregate literal — `return Holder { view: local.view() };`
    /// — dangles exactly like a bare view return, and says so in the leaf's
    /// own terms. Runs whatever the return type is, so the escape is still
    /// named when the returned type itself cannot be classified.
    ///
    /// Only unambiguous view leaves are flagged, so moving an owned value
    /// into an owned field (`Holder { text: local }`) is never mistaken for
    /// a borrow.
    fn check_returned_aggregate(&mut self, e: &Expr) {
        if !matches!(
            &e.kind,
            ExprKind::StructLit { .. }
                | ExprKind::InferredStructLit { .. }
                | ExprKind::GenericStructLit { .. }
                | ExprKind::ArrayLit { .. }
                | ExprKind::TupleLit { .. }
                | ExprKind::ArrayFill { .. }
        ) {
            return;
        }
        let expected = self.return_ty.clone();
        self.flag_view_leaves(e, expected.as_ref());
    }

    /// The declared type of one field of a named struct.
    fn field_ty(&self, struct_name: &str, field: &str) -> Option<Type> {
        self.sigs
            .struct_fields
            .get(struct_name)
            .and_then(|fs| fs.get(field))
            .cloned()
    }

    /// The element type an aggregate literal's elements are stored at.
    fn element_ty(expected: Option<&Type>) -> Option<Type> {
        match expected.map(|t| &t.kind) {
            Some(TypeKind::Slice(inner)) => Some((**inner).clone()),
            Some(TypeKind::Array { elem, .. }) => Some((**elem).clone()),
            _ => None,
        }
    }

    fn flag_view_leaves(&mut self, e: &Expr, expected: Option<&Type>) {
        match &e.kind {
            ExprKind::StructLit { name, fields } | ExprKind::GenericStructLit { name, fields, .. } => {
                for f in fields {
                    let fty = self.field_ty(&name.name, &f.name.name);
                    self.flag_view_leaves(&f.value, fty.as_ref());
                }
            }
            ExprKind::InferredStructLit { fields } => {
                let sname = match expected.map(|t| &t.kind) {
                    Some(TypeKind::Path(n)) => Some(n.clone()),
                    Some(TypeKind::Generic { name, .. }) => Some(name.clone()),
                    _ => None,
                };
                for f in fields {
                    let fty = sname.as_ref().and_then(|s| self.field_ty(s, &f.name.name));
                    self.flag_view_leaves(&f.value, fty.as_ref());
                }
            }
            ExprKind::ArrayLit { elements } => {
                let ety = Self::element_ty(expected);
                for el in elements {
                    self.flag_view_leaves(el, ety.as_ref());
                }
            }
            ExprKind::TupleLit { elements } => {
                let etys = match expected.map(|t| &t.kind) {
                    Some(TypeKind::Tuple(ts)) => ts.clone(),
                    _ => Vec::new(),
                };
                for (i, el) in elements.iter().enumerate() {
                    self.flag_view_leaves(el, etys.get(i));
                }
            }
            ExprKind::ArrayFill { fill, .. } => {
                let ety = Self::element_ty(expected);
                self.flag_view_leaves(fill, ety.as_ref());
            }
            _ => {
                for root in self.view_leaf_roots(e, expected) {
                    if !self.root_dies_at_return(&root) {
                        continue;
                    }
                    let what = if root == "self" || root == "this" {
                        "the `take this` receiver".to_string()
                    } else if self.param_names.contains(&root) {
                        format!("`take` parameter `{root}`")
                    } else {
                        format!("local `{root}`")
                    };
                    self.err(
                        "E0513",
                        format!(
                            "view of {what} escapes inside the returned value: it is freed when the function returns, so the stored view would dangle. Store an owned `Text` / `Vec[T]`, or borrow the view from a non-`take` parameter"
                        ),
                        e.span,
                    );
                }
            }
        }
    }

    /// View-source roots of an aggregate LEAF — but only for a leaf that is
    /// genuinely a view: a view-producing call, or an owner READ AS a view
    /// where the field expects one (the `Text`→`str` coercion, which has no
    /// accessor to key on). A leaf that moves an owned value into an owned
    /// field is not a view and must never be flagged; that would reject a
    /// valid ownership transfer.
    fn view_leaf_roots(&self, e: &Expr, expected: Option<&Type>) -> BTreeSet<String> {
        match &e.kind {
            ExprKind::Call { callee, .. }
                if matches!(&callee.kind, ExprKind::Field { .. } | ExprKind::Ident(_)) =>
            {
                self.view_source_roots(e)
            }
            _ if self.is_coercion_to_view(e, expected) => self.view_source_roots(e),
            _ => BTreeSet::new(),
        }
    }

    /// True iff `e` is an owner being read where a view is expected — the
    /// coercion an owning string performs at a `str` position. The owner
    /// must be non-Copy (it has bytes to free) and not already a view.
    fn is_coercion_to_view(&self, e: &Expr, expected: Option<&Type>) -> bool {
        let Some(want) = expected else {
            return false;
        };
        if !Self::is_view_ty(want) {
            return false;
        }
        match self.infer_ty(e) {
            Some(t) => !Self::is_view_ty(&t) && !self.oracle.is_copy(&t),
            None => false,
        }
    }

    /// The declared type of a WRITE target: a place chain resolved step by
    /// step. Two steps the rest of the analysis never has to type appear
    /// only here — `*p` where `p: *T` stores a `T`, and `a[i]` where `a` is
    /// an array or slice stores its element. Everything else defers to the
    /// shared inference.
    fn place_ty(&self, e: &Expr) -> Option<Type> {
        match &e.kind {
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => match self.place_ty(operand).map(|t| t.kind) {
                Some(TypeKind::RawPtr(inner)) => Some(*inner),
                _ => None,
            },
            ExprKind::Index { receiver, .. } => {
                Self::element_ty(self.place_ty(receiver).as_ref())
            }
            ExprKind::Field { receiver, name } => {
                let base = match self.place_ty(receiver)?.kind {
                    TypeKind::Path(n) => n,
                    TypeKind::Generic { name, .. } => name,
                    _ => return None,
                };
                self.field_ty(&base, &name.name)
            }
            _ => self.infer_ty(e),
        }
    }

    /// Contract §5, the mandatory choice at the raw seam: a store of
    /// view-typed data through a raw-pointer deref is invisible to every
    /// flow analysis, so the function must DECLARE its flows. This is the
    /// doctrine E0510 applies to raw-pointer fields (drop-or-`opaque`), one
    /// accountability question later: `opaque` answers who frees, `keeps`
    /// answers who outlives.
    ///
    /// Byte stores (`*p = b`) and pointer stores never fire; only a view or
    /// a carrier VALUE does.
    ///
    /// The store is judged by whether the WRITE PATH crosses the seam, not
    /// by the target's outermost shape. `(*p).key = v` is the same opaque
    /// store as `*p = v` — the analysis knows nothing about what `p` points
    /// at, so a projection of the pointee is exactly as invisible as the
    /// whole pointee. The first cut matched only a bare deref target, which
    /// left the field form open; that is
    /// `bugs/str-field-outliving-its-text-is-not-caught.md`, where a `str`
    /// parameter stored into `(*sink).key` compiled clean and read freed
    /// bytes at runtime.
    fn check_raw_store(&mut self, target: &Expr, value: &Expr) {
        if self.declares_flows {
            return;
        }
        if !self.writes_through_raw_deref(target) {
            return;
        }
        let Some(ty) = self.place_ty(target) else {
            return;
        };
        if !(Self::is_view_ty(&ty) || self.carries_view(&ty)) {
            return;
        }
        self.err(
            "E0516",
            "storing a view through a raw pointer without a declared flow: no analysis can see where these bytes end up, so the function must declare it — `#[keeps(this)]` if the view survives in the receiver, `#[keeps(nothing)]` if the bytes are copied and nothing borrowed escapes"
                .to_string(),
            value.span,
        );
    }

    /// Does writing to this place go through a raw-pointer deref? Walks the
    /// projection chain to the base: `*p`, `(*p).f`, `(*p)[i]`, `(*p).a.b`
    /// all answer yes, and every place rooted in a plain binding answers no.
    ///
    /// The deref step must be TYPED as a raw pointer. That keeps the rule
    /// where the contract puts it (§4: what `*T` reaches is untracked) and
    /// matches what the rest of the pass can act on — a deref whose operand
    /// has no known type gives `place_ty` nothing either, so the store
    /// would skip on the type gate regardless.
    fn writes_through_raw_deref(&self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => matches!(
                self.place_ty(operand).map(|t| t.kind),
                Some(TypeKind::RawPtr(_))
            ),
            ExprKind::Field { receiver, .. } | ExprKind::Index { receiver, .. } => {
                self.writes_through_raw_deref(receiver)
            }
            _ => false,
        }
    }

    /// Contract §3.1, the temporary: `let s: str = mk().view();` binds a
    /// view of a value nothing named, so the statement drops the owner and
    /// the binding dangles before it is ever read. Only a NON-place
    /// receiver is a temporary — a named place is somebody's binding, and
    /// the `owns_value` gate judges that one. Passing such a view straight
    /// to a call is sound (the temporary outlives the call) and never
    /// routes here.
    fn check_view_of_temp(&mut self, value: &Expr) {
        let Some((tyname, m)) = self.temp_view_origin(value) else {
            return;
        };
        self.err(
            "E0513",
            format!(
                "cannot bind the view returned by `.{m}()` on a temporary `{tyname}`: the temporary is dropped at the end of this statement, so the view would dangle. Bind the owner to a named local first (`let owner = ...; owner.{m}()`)"
            ),
            value.span,
        );
    }

    /// Contract §3.1, the coercion: `let s: str = t.clone();` and
    /// `let s: str = "x ${i}";` bind a view of an owner nothing named. The
    /// coercion spills the `Text` to an anonymous slot and keeps its
    /// `{ptr,len}` prefix; no binding owns the slot. Codegen keeps the shape
    /// SOUND by never freeing it — one leaked allocation per evaluation — so
    /// the defect reads as memory growth rather than as a crash, and the
    /// honest answer is that the binding is wrong, not that the free is
    /// missing (bugs/rvalue-text-coercion-binding-leak.md).
    ///
    /// Sibling of `check_view_of_temp`: that one catches an EXPLICIT
    /// view-producing method on a temporary (`mk().view()`), this one the
    /// IMPLICIT owner→view coercion of a temporary. Both fire only on a
    /// non-place initializer — `let s: str = t;` coerces from a named owner,
    /// which outlives the statement, and `owns_value` judges any later escape.
    ///
    /// Argument and receiver positions never route here: a temporary in a
    /// consumed context outlives the call it is an argument to, which is what
    /// makes `f("x = ${i}")` sound (and, since 2026-08-13, non-leaking).
    fn check_view_of_rvalue_owner(&mut self, value: &Expr) {
        let core = Self::trivial_tail(value);
        // A named place is somebody's binding; it outlives this statement.
        if Self::place_root(core).is_some() {
            return;
        }
        // An interpolation builds an owned string and names nothing. It has no
        // declared type for `infer_ty` to return, so it is answered here.
        if matches!(core.kind, ExprKind::InterpStr { .. }) {
            self.err(
                "E0513",
                "cannot bind a view of a string interpolation: it builds an owned string in an anonymous slot that nothing frees and nothing keeps, so the view is backed by storage with no lifetime. Make the binding owned (`let s: Text = \"…\";`) and take the view from it"
                    .to_string(),
                value.span,
            );
            return;
        }
        let Some(ty) = self.infer_ty(core) else {
            return;
        };
        // Already a view — `mk().view()` is `check_view_of_temp`'s.
        if Self::is_view_ty(&ty) {
            return;
        }
        // An owner with no heap of its own cannot be the source of a coercion
        // to a view. Sema has already accepted the program, so a non-Copy,
        // non-view initializer under a view-typed target IS the owner→view
        // coercion; naming the type keeps the message specific.
        if self.oracle.is_copy(&ty) {
            return;
        }
        let tyname = match &ty.kind {
            TypeKind::Path(n) => n.clone(),
            TypeKind::Generic { name, .. } => name.clone(),
            _ => return,
        };
        self.err(
            "E0513",
            format!(
                "cannot bind a view of a temporary `{tyname}`: the coercion spills the owner to an anonymous slot that nothing frees and nothing keeps, so the view is backed by storage with no lifetime. Bind the owner first (`let owner: {tyname} = ...;` then take the view), or keep the binding owned"
            ),
            value.span,
        );
    }

    /// The expression a trivial `{ … }` wrapper stands for — the block form
    /// every raw-pointer read and most coercion sites are written in. A block
    /// with statements is not trivial: its tail is evaluated after them and is
    /// not the same expression.
    fn trivial_tail(e: &Expr) -> &Expr {
        match &e.kind {
            ExprKind::Block(b) if b.stmts.is_empty() => match &b.tail {
                Some(t) => Self::trivial_tail(t),
                None => e,
            },
            _ => e,
        }
    }

    /// Contract §3.1 at a CAPTURE: `let slot: Slot = Slot { s: mk().view() };`
    /// stores a view of a temporary into a binding that outlives the
    /// statement — the same dangle as `let s: str = mk().view()`, one
    /// aggregate deep.
    ///
    /// The mirror of `flag_view_leaves`, which asks whether a leaf's owner
    /// dies at RETURN; this asks whether the leaf has a named owner at all.
    /// Same aggregate walk, so a field's declared type reaches its leaf and an
    /// owned value moved into an owned field is never mistaken for a view.
    ///
    /// Binding, assignment and destructure positions only. The same aggregate
    /// built as a call ARGUMENT is sound — its temporaries outlive the call.
    fn check_captured_view_of_temp(&mut self, e: &Expr, expected: Option<&Type>) {
        match &e.kind {
            ExprKind::StructLit { name, fields } | ExprKind::GenericStructLit { name, fields, .. } => {
                for f in fields {
                    let fty = self.field_ty(&name.name, &f.name.name);
                    self.check_captured_view_of_temp(&f.value, fty.as_ref());
                }
            }
            ExprKind::InferredStructLit { fields } => {
                let sname = match expected.map(|t| &t.kind) {
                    Some(TypeKind::Path(n)) => Some(n.clone()),
                    Some(TypeKind::Generic { name, .. }) => Some(name.clone()),
                    _ => None,
                };
                for f in fields {
                    let fty = sname.as_ref().and_then(|s| self.field_ty(s, &f.name.name));
                    self.check_captured_view_of_temp(&f.value, fty.as_ref());
                }
            }
            ExprKind::ArrayLit { elements } => {
                let ety = Self::element_ty(expected);
                for el in elements {
                    self.check_captured_view_of_temp(el, ety.as_ref());
                }
            }
            ExprKind::TupleLit { elements } => {
                let etys = match expected.map(|t| &t.kind) {
                    Some(TypeKind::Tuple(ts)) => ts.clone(),
                    _ => Vec::new(),
                };
                for (i, el) in elements.iter().enumerate() {
                    self.check_captured_view_of_temp(el, etys.get(i));
                }
            }
            ExprKind::ArrayFill { fill, .. } => {
                let ety = Self::element_ty(expected);
                self.check_captured_view_of_temp(fill, ety.as_ref());
            }
            // A leaf. `check_view_of_temp` gates itself on the call actually
            // producing a view of its receiver, so asking it about every leaf
            // is free; the coercion arm needs the field's type to know a view
            // was wanted at all.
            _ => {
                self.check_view_of_temp(e);
                if self.is_coercion_to_view(e, expected) {
                    self.check_view_of_rvalue_owner(e);
                }
            }
        }
    }

    /// The owner a view-producing call chain ultimately borrows from, when
    /// that owner is a TEMPORARY — `(owner type name, the method that took
    /// the view)`. `None` when the chain is rooted in a named place (some
    /// other rule judges that one), or when any link fails to propagate the
    /// borrow.
    ///
    /// The recursion is the fix for the laundering hole: `mk().view()` was
    /// caught, but `mk().view().trim()` was not. The old code read the
    /// receiver's type, saw `str` — Copy, "nothing to dangle" — and
    /// returned. That is true of the fat POINTER and false of the bytes it
    /// points into: a `str→str` method hands the same borrow along, so the
    /// dangle just moved one link out. `mk().view().trim()` compiled clean
    /// and read freed memory. So a Copy *view* receiver no longer ends the
    /// walk; it continues it, down to the first non-Copy owner.
    ///
    /// Every link must still pass `method_produces_view`, which is what
    /// keeps a method that returns an unrelated view (an interned or
    /// `'static` one — no `SelfReceiver` borrow) from being blamed on a
    /// receiver it never pointed into.
    fn temp_view_origin(&self, e: &Expr) -> Option<(String, String)> {
        let ExprKind::Call { callee, .. } = &e.kind else {
            return None;
        };
        let ExprKind::Field { receiver, name } = &callee.kind else {
            return None;
        };
        // A named place is somebody's binding — the `owns_value` gate and
        // the root rules judge that one, not this.
        if Self::place_root(receiver).is_some() {
            return None;
        }
        let rty = self.infer_ty(receiver)?;
        // The link must actually hand the receiver's borrow along.
        if !self.method_produces_view(receiver, &name.name) {
            return None;
        }
        if self.oracle.is_copy(&rty) {
            // A view receiver carries no storage of its own; whatever it
            // points into is one link further down.
            if Self::is_view_ty(&rty) {
                return self.temp_view_origin(receiver);
            }
            return None;
        }
        let tyname = match &rty.kind {
            TypeKind::Path(n) => n.clone(),
            TypeKind::Generic { name, .. } => name.clone(),
            _ => return None,
        };
        Some((tyname, name.name.clone()))
    }

    /// Does a `match` OWN the value it destructures, so that a payload
    /// binding owns its part of it? A call result or a constructor is a
    /// temporary the match owns outright; a whole binding owns what that
    /// binding owns; a projection or a deref names storage owned elsewhere,
    /// so the payload is a borrow the owner still drops.
    fn scrutinee_is_owned(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Ident(n) => self.lookup(n).is_some_and(|l| l.owns_value),
            ExprKind::Field { .. } | ExprKind::Index { .. } => false,
            ExprKind::Unary {
                op: UnaryOp::Deref, ..
            } => false,
            _ => true,
        }
    }

    /// The declared type of each payload binding in a variant pattern, with
    /// the enum's type arguments substituted — from the pattern's own
    /// turbofish where it has one, else from the scrutinee's type. Without
    /// these a payload binding is untyped and every type-gated rule skips
    /// it, which is how a view of a matched-out owner escaped.
    fn payload_types(&self, pat: &Pattern, scrutinee: &Expr) -> HashMap<String, Type> {
        let mut out = HashMap::new();
        let PatternKind::Variant {
            enum_name,
            type_args,
            variant_name,
            payload,
        } = &pat.kind
        else {
            return out;
        };
        let Some(ptys) = self
            .sigs
            .enum_payloads
            .get(&enum_name.name)
            .and_then(|vs| vs.get(&variant_name.name))
        else {
            return out;
        };
        let map: HashMap<String, Type> = match self.sigs.enum_generics.get(&enum_name.name) {
            None => HashMap::new(),
            Some(gp) => {
                let args: Vec<Type> = if !type_args.is_empty() {
                    type_args.clone()
                } else {
                    match self.infer_ty(scrutinee).map(|t| t.kind) {
                        Some(TypeKind::Generic { name, args }) if name == enum_name.name => args,
                        _ => return out,
                    }
                };
                gp.iter().cloned().zip(args).collect()
            }
        };
        for (bp, t) in payload.iter().zip(ptys.iter()) {
            if let PatternKind::Binding(id) = &bp.kind {
                out.insert(id.name.clone(), subst_type(t, &map));
            }
        }
        out
    }

    /// The base binding name of a place expression. `None` for anything
    /// that is not a place.
    fn place_root(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field { receiver, .. } | ExprKind::Index { receiver, .. } => {
                Self::place_root(receiver)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => Self::place_root(operand),
            _ => None,
        }
    }

    /// Contract §3.1 / §5: a view stored into a place that OUTLIVES the
    /// frame — a module `static`, or a `ref` / `ref this` target aliasing
    /// the caller's storage. Two ways for that to dangle, and they read the
    /// same store:
    ///
    /// - the view's owner is storage THIS frame frees (a local, a `take`
    ///   parameter, a `take this` receiver) — E0513, unconditionally;
    /// - the view's owner is the CALLER's, guaranteed only for the duration
    ///   of the call — E0515, unless the flow pass exported this store, in
    ///   which case every call site ties the sink to the argument's owner
    ///   and the lifetime is the caller's to get right.
    ///
    /// That second clause is the whole point of the port. Sema used to
    /// answer it by hand — "a concrete method's stores are exported, so
    /// skip"; "a concrete free fn whose address is untaken is exported, so
    /// skip" — which is the NEGATION of this pass's coverage, maintained by
    /// belief. Here the store asks the flow pass directly, so a store the
    /// analysis stopped covering is denied instead of silently allowed.
    fn check_store_escape(&mut self, target: &Expr, value: &Expr) {
        let Some(ty) = self.place_ty(target) else {
            return;
        };
        if !(Self::is_view_ty(&ty) || self.carries_view(&ty)) {
            return;
        }
        let Some(troot_raw) = Self::place_root(target) else {
            return;
        };
        let troot = Self::canonical(&troot_raw).to_string();
        let target_is_static = self.statics.contains_key(&troot);
        let target_is_ref = self.ref_targets.contains(&troot);
        if !(target_is_static || target_is_ref) {
            return;
        }
        let target_is_receiver = troot == "self";
        let dest = if target_is_static {
            format!("static `{troot}`")
        } else {
            format!("`ref` target `{troot_raw}`")
        };
        for vroot in self.borrow_roots_of(value) {
            let key = Self::canonical(&vroot).to_string();
            let Some(local) = self.lookup(&key).cloned() else {
                continue;
            };
            let Some(vty) = local.ty.clone() else {
                continue;
            };
            if local.owns_value && !self.oracle.is_copy(&vty) {
                let owner = self.owner_desc(&vroot);
                self.err(
                    "E0513",
                    format!(
                        "cannot store a view of {owner} into {dest}: the view's owner is freed when the function returns, but {dest} outlives it, so the stored view would dangle"
                    ),
                    value.span,
                );
                return;
            }
            // The root is a borrowed view the CALLER owns: a view-typed or
            // view-carrying parameter, or a view PROJECTED from a borrowed
            // non-Copy one (`this.f = k.view()` with `k: Text` — `Text` is
            // an owner, not a carrier, and a bare parameter is not
            // `owns_value`, so neither earlier gate sees it).
            let root_is_param_view = self.param_names.contains(&key)
                && (self.carries_view(&vty)
                    || (!local.owns_value && !self.oracle.is_copy(&vty)));
            if !root_is_param_view {
                continue;
            }
            if !target_is_static && self.store_is_tied(&key, target_is_receiver, &troot) {
                continue;
            }
            let hint = if target_is_receiver {
                " Own the bytes (a `Text` field), intern them (`text::intern`), or declare the method `#[keeps(this)]` so every caller ties the receiver to the argument's owner."
            } else {
                " Own the bytes (`Text`), or intern them (`text::intern`) for a process-lifetime view."
            };
            self.err(
                "E0515",
                format!(
                    "cannot store the view parameter `{vroot}` into {dest}: the caller only guarantees `{vroot}`'s bytes for this call, but {dest} outlives it, so the stored view would dangle.{hint}"
                ),
                value.span,
            );
            return;
        }
    }

    /// Did the flow pass export this parameter→sink store, so that call
    /// sites tie? A `static` never reaches here: it has no owner to tie to.
    fn store_is_tied(&self, src: &str, target_is_receiver: bool, target_root: &str) -> bool {
        if target_is_receiver {
            // A view of the receiver stored back into the receiver outlives
            // nothing.
            if src == "self" {
                return true;
            }
            let ViewSite::Method { key } = &self.site else {
                return false;
            };
            let (Some(entry), Some(i)) = (self.sigs.methods.get(key), self.param_index.get(src))
            else {
                return false;
            };
            return SigTable::effective_keeps(entry)
                .get(*i)
                .copied()
                .unwrap_or(false);
        }
        // A `ref` parameter target. Only a free fn publishes (src → dst)
        // flows, and only a directly-called one can have them applied.
        let ViewSite::Free { name, exported } = &self.site else {
            return false;
        };
        if !exported {
            return false;
        }
        let (Some(entry), Some(src_i), Some(dst_i)) = (
            self.sigs.fns.get(name),
            self.param_index.get(src),
            self.param_index.get(target_root),
        ) else {
            return false;
        };
        entry.computed_ref_flows.contains(&(*src_i, *dst_i))
    }

    // -- capture escapes (E0365) -------------------------------------------
    //
    // The view family's sibling, ported from sema with issue-07 step 5. A
    // view carries BYTES the owner frees; a capture carries the owner's
    // ADDRESS, bound into a handler that some later event-loop turn calls.
    // The escape is the same question — does the value outlive the storage
    // it points at — and it turns on the same `owns_value` gate, which is
    // why a child held in a field of `this` is legal where a local is not.
    //
    // Sema asked it at three separately-patched POSITIONS (return,
    // assignment, call argument), so a fourth way out of the frame needed a
    // fourth patch. Here there is one source classifier and one ownership
    // gate; the sinks are places the walk already visits.

    /// The root binding of a place expression (`a.b[i].c` → `a`), or `None`
    /// for a non-place. Deliberately not `place_root`: a deref step is NOT
    /// followed, because `(*p).handler` binds a method to the pointee, not
    /// to `p`'s own frame slot, and the pointee's lifetime is not this
    /// frame's question.
    fn capture_place_root(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field { receiver, .. } | ExprKind::Index { receiver, .. } => {
                Self::capture_place_root(receiver)
            }
            _ => None,
        }
    }

    /// The struct type named by a place expression, when it is one. Both
    /// capture questions are about a struct's methods, so a non-struct
    /// receiver (an enum, a primitive, an untypeable place) answers no.
    fn struct_name_of(&self, e: &Expr) -> Option<String> {
        let base = match self.place_ty(e)?.kind {
            TypeKind::Path(n) => n,
            TypeKind::Generic { name, .. } => name,
            _ => return None,
        };
        self.sigs.struct_fields.contains_key(&base).then_some(base)
    }

    /// Is `<place>.name` a bound METHOD reference rather than a field read?
    /// A real fn-pointer FIELD is an ordinary read and binds nothing.
    fn is_bound_method_ref(&self, base: &str, name: &str) -> bool {
        let Some(fields) = self.sigs.struct_fields.get(base) else {
            return false;
        };
        !fields.contains_key(name) && self.sigs.methods.contains_key(&format!("{base}.{name}"))
    }

    /// Does this binding's storage die when the frame returns? The same
    /// `owns_value` flag the view rules turn on: a plain local, a `take`
    /// parameter and a `take this` do; a bare / `ref` parameter and a
    /// `this` / `ref this` receiver name the caller's storage and do not. A
    /// `static` or an unknown name outlives us.
    ///
    /// The Copy filter runs the other way from `root_dies_at_return`, and
    /// that difference is the whole reason the two questions need two
    /// gates. A view of a Copy root is harmless — a Copy root owns no heap
    /// to free — so the view rules drop it. A capture of one is the
    /// hazard: what a handler bound to a by-value Copy PARAMETER points at
    /// is this frame's copy, not the caller's storage, and that copy is
    /// gone at return. So a by-value parameter widens the gate when its
    /// type is Copy, which is what sema's `owns_value` meant by
    /// `param.move_ || is_copy(ty)`. The receiver does not widen: `this`
    /// names the caller's object however it is passed.
    fn capture_root_dies(&self, root: &str) -> bool {
        let key = Self::canonical(root);
        let Some(l) = self.lookup(key) else {
            return false;
        };
        l.owns_value
            || (key != "self"
                && self.param_names.contains(key)
                && l.ty.as_ref().is_some_and(|t| self.oracle.is_copy(t)))
    }

    /// The locals whose address `e` carries — because it hands a bound
    /// method reference to a call, because it calls a receiver-capturing
    /// method on one, or because it merely NAMES a binding already tainted
    /// by one of those. That last clause is what makes the analysis
    /// transitive through ordinary data flow: once `b` holds `&c`, every
    /// expression mentioning `b` carries `&c` too.
    ///
    /// `flow_only` narrows the answer to captures the expression's OWN
    /// result carries: an already-tainted binding, or `local.m(...)` where
    /// `m` hands out its receiver — in both cases the value in hand IS the
    /// carrier. It excludes an argument-position bound reference like
    /// `f(local.handler)`, because whether `f`'s RESULT carries the address
    /// is a property of `f` that this analysis cannot see; counting those
    /// when propagating taint made `var n: i32 = take_handler(c.clicked);`
    /// an error, and an `i32` has nowhere to put an address. The sinks scan
    /// for them (`flow_only == false`), since there the value leaving the
    /// frame is the expression itself.
    fn capture_sources_inner(&self, e: &Expr, flow_only: bool) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for_each_expr(e, &mut |x: &Expr| {
            // A binding already known to hold captures contributes all of
            // them — one builder can absorb several children, and each is
            // its own dangling receiver needing its own fix.
            if let ExprKind::Ident(n) = &x.kind {
                if let Some(srcs) = self.capture_taint.get(n) {
                    for sc in srcs {
                        if !out.contains(sc) {
                            out.push(sc.clone());
                        }
                    }
                }
            }
            let ExprKind::Call { callee, args, .. } = &x.kind else {
                return;
            };
            // Direct: `f(local.handler)`. Only meaningful at a sink.
            if !flow_only {
                for a in args {
                    let ExprKind::Field { receiver, name } = &a.kind else {
                        continue;
                    };
                    let (Some(root), Some(base)) = (
                        Self::capture_place_root(receiver),
                        self.struct_name_of(receiver),
                    ) else {
                        continue;
                    };
                    if self.is_bound_method_ref(&base, &name.name)
                        && self.capture_root_dies(&root)
                        && !out.contains(&root)
                    {
                        out.push(root);
                    }
                }
            }
            // Transitive: `local.m(...)` where `m` binds its own receiver.
            let ExprKind::Field { receiver, name } = &callee.kind else {
                return;
            };
            let (Some(root), Some(base)) = (
                Self::capture_place_root(receiver),
                self.struct_name_of(receiver),
            ) else {
                return;
            };
            if self.capturing.contains(&(base, name.name.clone()))
                && self.capture_root_dies(&root)
                && !out.contains(&root)
            {
                out.push(root);
            }
        });
        out
    }

    fn capture_sources(&self, e: &Expr) -> Vec<String> {
        self.capture_sources_inner(e, false)
    }

    fn capture_sources_flow(&self, e: &Expr) -> Vec<String> {
        self.capture_sources_inner(e, true)
    }

    /// Propagate capture taint across one statement, before the statement
    /// is checked. Only the shapes that move a value INTO a longer-lived
    /// binding matter; anything else cannot make a capture escape further
    /// than it already has.
    fn update_capture_taint(&mut self, s: &Stmt) {
        match &s.kind {
            // `var b = <expr carrying &c>` — `b` now holds it.
            StmtKind::Let {
                name, init: Some(e), ..
            } => {
                let srcs = self.capture_sources_flow(e);
                if !srcs.is_empty() {
                    self.taint(name.name.clone(), srcs);
                }
            }
            StmtKind::Expr(e) => {
                // `b.add(c.build())` — the RECEIVER absorbs the argument.
                // This is the builder shape, and the one a component tree
                // actually writes.
                if let ExprKind::Call { callee, args, .. } = &e.kind {
                    if let ExprKind::Field { receiver, .. } = &callee.kind {
                        if let Some(dest) = Self::capture_place_root(receiver) {
                            let acc: Vec<String> = args
                                .iter()
                                .flat_map(|a| self.capture_sources_flow(a))
                                .collect();
                            self.taint(dest, acc);
                        }
                    }
                }
                // `b = <expr carrying &c>`
                if let ExprKind::Assign { target, value, .. } = &e.kind {
                    let srcs = self.capture_sources_flow(value);
                    if let Some(dest) = Self::capture_place_root(target) {
                        self.taint(dest, srcs);
                    }
                }
            }
            _ => {}
        }
    }

    fn taint(&mut self, dest: String, srcs: Vec<String>) {
        if srcs.is_empty() {
            return;
        }
        let slot = self.capture_taint.entry(dest).or_default();
        for sc in srcs {
            if !slot.contains(&sc) {
                slot.push(sc);
            }
        }
    }

    /// Every identifier named anywhere in `e`. Used only to phrase the
    /// E0365 message.
    fn expr_names(e: &Expr) -> HashSet<String> {
        let mut out = HashSet::new();
        for_each_expr(e, &mut |x: &Expr| {
            if let ExprKind::Ident(n) = &x.kind {
                out.insert(n.clone());
            }
        });
        out
    }

    /// Name the binding that carried the capture out, when the escape is
    /// indirect — otherwise the span points at a statement that never
    /// mentions the offending local and the message reads like a
    /// non-sequitur.
    fn carried_out_by(&self, root: &str, named: &HashSet<String>) -> String {
        if named.contains(root) {
            return String::new();
        }
        match self
            .capture_taint
            .iter()
            .find(|(h, v)| v.iter().any(|r| r == root) && named.contains(*h))
        {
            Some((holder, _)) => format!(" (carried out by `{holder}`)"),
            None => String::new(),
        }
    }

    /// bugs/facet-component-local-child-dangling-receiver.md — a returned
    /// value that captured the address of a local. Two shapes, both of
    /// which put `&local` into the returned value: direct — `return
    /// button().on_click(local.handler)` — and transitive — `return
    /// local.build()`, where `build` binds a reference to its own receiver.
    /// The second is the documented component-composition shape, and the
    /// one that actually shipped broken.
    /// bugs/e0365-catches-the-return-but-not-the-assignment.md — the same
    /// rule, applied where the value leaves the frame by ASSIGNMENT. The
    /// first cut was written about the returned EXPRESSION, so every other
    /// way out was open: a `static`, a field reached through a `ref`
    /// parameter, anything the frame does not own. The escape is identical,
    /// and it is the one a component tree invites, because composing a child
    /// as a local and hanging its node somewhere is the obvious way to write
    /// a widget.
    ///
    /// Same destination set as `check_store_escape` — a `static`, or a
    /// `ref` / `ref this` target aliasing the caller's storage — and the
    /// same ownership gate on the captured root, so storing a child held in
    /// a FIELD stays legal while a local does not.
    fn check_capture_store(&mut self, target: &Expr, value: &Expr) {
        let Some(troot_raw) = Self::place_root(target) else {
            return;
        };
        let troot = Self::canonical(&troot_raw).to_string();
        let target_is_static = self.statics.contains_key(&troot);
        if !(target_is_static || self.ref_targets.contains(&troot)) {
            return;
        }
        let roots = self.capture_sources(value);
        let Some(root) = roots.first().cloned() else {
            return;
        };
        let dest = if target_is_static {
            format!("static `{troot}`")
        } else {
            format!("`ref` target `{troot_raw}`")
        };
        let via = self.carried_out_by(&root, &Self::expr_names(value));
        self.err(
            "E0365",
            format!(
                "storing a value that captures the address of `{root}`{via} into {dest}: `{root}` is a local, so a handler bound to it would point at a stack slot that is freed when this function returns, while {dest} outlives it. Give it storage that outlives the stored value — a field of `this`, a `static`, or a `Box`"
            ),
            value.span,
        );
    }

    /// bugs/e0365-catches-the-return-but-not-the-assignment.md, third round
    /// — the escape that is neither a `return` nor an assignment: handing
    /// the capturing value to a CALL that keeps it.
    ///
    /// ```text
    /// find("list").add_child(clickable(b).on_click(w.on_click));
    /// ```
    ///
    /// `add_child` puts the node in a retained tree, which outlives the
    /// frame exactly as a `static` does. Nothing in either function is wrong
    /// on its own — the callee stores a PARAMETER, which is how every
    /// registry works — so the violation exists only across the pair, and
    /// the callee is often reached through a fn-pointer static no analysis
    /// can trace.
    ///
    /// So this does not try to prove the callee keeps it. It refuses to hand
    /// a frame-local's address across a call boundary at all, which is
    /// over-approximate and, measured across every vendor package and
    /// example, costs nothing: real code binds a handler to `this` or to a
    /// field, and the ownership gate passes both.
    ///
    /// What does NOT fire is the binding site itself. `take_handler(c.clicked)`
    /// has a bare bound reference as its argument, and whether the RESULT
    /// carries the address is a property of the callee — the classifier
    /// finds a capture only once the value in hand already carries one.
    /// That is what keeps ordinary composition legal.
    fn check_capture_arg(&mut self, arg: &Expr) {
        let Some(root) = self
            .capture_sources(arg)
            .into_iter()
            .find(|r| self.capture_root_dies(r))
        else {
            return;
        };
        self.err(
            "E0365",
            format!(
                "passing a value that captures the address of `{root}` to a call: `{root}` is a local, so if the callee keeps the value — a registry, a retained view tree, a static — the handler bound to `{root}` would point at a stack slot freed when this function returns. Give `{root}` storage that outlives the call — a field of `this`, a `static`, or a `Box`"
            ),
            arg.span,
        );
    }

    fn check_capture_return(&mut self, e: &Expr) {
        let roots = self.capture_sources(e);
        if roots.is_empty() {
            return;
        }
        let named = Self::expr_names(e);
        for root in roots {
            let via = self.carried_out_by(&root, &named);
            self.err(
                "E0365",
                format!(
                    "returning a value that captures the address of `{root}`{via}: it is a local, so a handler bound to it would point at a stack slot that is freed when this function returns. Give it storage that outlives the returned value — a field of `this`, a `static`, or a `Box`"
                ),
                e.span,
            );
        }
    }

    // -- the walk ----------------------------------------------------------

    /// Record what a `let` / `var` introduces: an owning binding, plus the
    /// owners its bytes come from when it is view-shaped (so a later
    /// `return alias;` is judged against the real owner).
    fn bind_let(&mut self, name: &str, ty: &Option<Type>, init: Option<&Expr>) {
        let resolved = ty.clone().or_else(|| init.and_then(|e| self.infer_ty(e)));
        let is_view = resolved.as_ref().is_some_and(Self::is_view_ty);
        let carries_view = resolved
            .as_ref()
            .is_some_and(|t| Self::is_view_ty(t) || self.carries_view(t));
        if is_view {
            if let Some(e) = init {
                self.check_view_of_temp(e);
                self.check_view_of_rvalue_owner(e);
            }
        } else if carries_view {
            // A carrier binding outlives the statement exactly as a view
            // binding does, so a view captured in one of its fields needs the
            // same owner. Disjoint from the `is_view` arm above, so a leaf is
            // never reported twice.
            if let Some(e) = init {
                self.check_captured_view_of_temp(e, resolved.as_ref());
            }
        }
        let borrow_roots = match (carries_view, init) {
            (true, Some(e)) => self.borrow_roots_of(e),
            _ => BTreeSet::new(),
        };
        self.bind(
            name,
            ViewLocal {
                ty: resolved,
                owns_value: true,
                borrow_roots,
            },
        );
    }

    fn walk_block(&mut self, b: &Block) {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.walk_stmt(s);
        }
        if let Some(t) = &b.tail {
            self.walk_expr(t);
        }
        self.scopes.pop();
    }

    fn walk_stmt(&mut self, s: &Stmt) {
        // Before the statement is checked: a capture that reaches a binding
        // here is carried by every later expression naming it.
        self.update_capture_taint(s);
        match &s.kind {
            StmtKind::Let {
                name, ty, init, ..
            } => {
                if let Some(e) = init {
                    self.walk_expr(e);
                }
                self.bind_let(&name.name, ty, init.as_ref());
            }
            StmtKind::LetDestructure {
                type_name,
                fields,
                init,
                ..
            } => {
                self.walk_expr(init);
                // The decomposed value's view-typed fields outlive the
                // statement as their own bindings, so a view captured from a
                // temporary dangles here exactly as it does under a whole-value
                // binding.
                let decomposed = Type {
                    kind: TypeKind::Path(type_name.name.clone()),
                    span: init.span,
                };
                if self.carries_view(&decomposed) {
                    self.check_captured_view_of_temp(init, Some(&decomposed));
                }
                // Each field binding re-owns its field of the decomposed
                // value; its type comes from the named struct.
                for f in fields {
                    let fty = self
                        .sigs
                        .struct_fields
                        .get(&type_name.name)
                        .and_then(|fs| fs.get(&f.name))
                        .cloned();
                    self.bind(
                        &f.name,
                        ViewLocal {
                            ty: fty,
                            owns_value: true,
                            borrow_roots: BTreeSet::new(),
                        },
                    );
                }
            }
            StmtKind::Return(Some(e)) => {
                self.walk_expr(e);
                self.check_returned_aggregate(e);
                self.check_return(e);
                self.check_capture_return(e);
            }
            StmtKind::Return(None) => {}
            StmtKind::While { cond, body, .. } => {
                self.walk_expr(cond);
                self.walk_block(body);
            }
            StmtKind::For(f, _) => {
                self.walk_for(f);
            }
            StmtKind::Loop(b, _) => self.walk_block(b),
            StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => self.walk_expr(e),
            StmtKind::Break | StmtKind::Continue => {}
            // Lowered away before this pass runs.
            StmtKind::IfLet { .. } | StmtKind::WhileLet { .. } | StmtKind::GuardLet { .. } => {}
        }
    }

    fn walk_for(&mut self, f: &ForLoop) {
        self.scopes.push(HashMap::new());
        match f {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init {
                    self.walk_stmt(i);
                }
                if let Some(c) = cond {
                    self.walk_expr(c);
                }
                for u in update {
                    self.walk_expr(u);
                }
                self.walk_block(body);
            }
            ForLoop::Range { var, iter, body } => {
                self.walk_expr(iter);
                // The loop variable names an element read out of the
                // iterable; it owns no storage the frame frees.
                self.bind(
                    &var.name,
                    ViewLocal {
                        ty: None,
                        owns_value: false,
                        borrow_roots: BTreeSet::new(),
                    },
                );
                self.walk_block(body);
            }
        }
        self.scopes.pop();
    }

    fn walk_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Block(b) => self.walk_block(b),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.walk_expr(cond);
                let before = self.scopes.clone();
                self.walk_block(then);
                let after_then = std::mem::replace(&mut self.scopes, before);
                if let Some(eb) = else_branch {
                    self.walk_expr(eb);
                }
                self.union_roots_from(&after_then);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr(scrutinee);
                let owned = self.scrutinee_is_owned(scrutinee);
                let before = self.scopes.clone();
                let mut per_arm = Vec::with_capacity(arms.len());
                for a in arms {
                    self.scopes = before.clone();
                    self.scopes.push(HashMap::new());
                    let payload_tys = self.payload_types(&a.pattern, scrutinee);
                    for n in pattern_binding_names(&a.pattern) {
                        let ty = payload_tys.get(&n).cloned();
                        self.bind(
                            &n,
                            ViewLocal {
                                ty,
                                owns_value: owned,
                                borrow_roots: BTreeSet::new(),
                            },
                        );
                    }
                    self.walk_expr(&a.body);
                    self.scopes.pop();
                    per_arm.push(std::mem::take(&mut self.scopes));
                }
                self.scopes = before;
                for arm_state in &per_arm {
                    self.union_roots_from(arm_state);
                }
            }
            ExprKind::Assign { op, target, value } => {
                self.walk_expr(value);
                self.walk_expr(target);
                if *op != AssignOp::Assign {
                    return;
                }
                self.check_store_escape(target, value);
                self.check_raw_store(target, value);
                self.check_capture_store(target, value);
                let target_ty = self.place_ty(target);
                if target_ty.as_ref().is_some_and(Self::is_view_ty) {
                    self.check_view_of_temp(value);
                    self.check_view_of_rvalue_owner(value);
                } else if target_ty.as_ref().is_some_and(|t| self.carries_view(t)) {
                    self.check_captured_view_of_temp(value, target_ty.as_ref());
                }
                let ExprKind::Ident(n) = &target.kind else {
                    return;
                };
                let Some(ty) = self.lookup(n).and_then(|l| l.ty.clone()) else {
                    return;
                };
                let roots = if Self::is_view_ty(&ty) || self.carries_view(&ty) {
                    self.borrow_roots_of(value)
                } else {
                    BTreeSet::new()
                };
                self.set_roots(n, roots);
            }
            // The third escape sink. Every argument leaves the frame if the
            // callee keeps it, so each is asked before it is descended into
            // — except an `Enum::Variant(payload)`, which has no callee to
            // keep anything. That is a payload aggregate: the value it
            // builds escapes at its own sink, or it dies with the frame.
            ExprKind::Call { callee, args, .. } => {
                self.walk_expr(callee);
                let is_enum_ctor = match &callee.kind {
                    ExprKind::Path { segments } => {
                        segments.len() == 2 && self.sigs.enums.contains(&segments[0].name)
                    }
                    _ => false,
                };
                for a in args {
                    if !is_enum_ctor {
                        self.check_capture_arg(a);
                    }
                    self.walk_expr(a);
                }
            }
            _ => walk_expr_children(e, &mut |c| self.walk_expr(c)),
        }
    }
}

/// Every immediate sub-expression of `e`, for walkers that only need to
/// reach nested blocks. Scope-introducing shapes (block / if / match) are
/// handled by the caller before this is reached.
fn walk_expr_children(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match &e.kind {
        ExprKind::Call { callee, args, .. } => {
            f(callee);
            for a in args {
                f(a);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Unary { operand, .. } => f(operand),
        ExprKind::Field { receiver, .. } => f(receiver),
        ExprKind::Index { receiver, index } => {
            f(receiver);
            f(index);
        }
        ExprKind::Assign { target, value, .. } => {
            f(target);
            f(value);
        }
        ExprKind::Cast { expr, .. } => f(expr),
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            for fl in fields {
                f(&fl.value);
            }
        }
        ExprKind::ArrayLit { elements }
        | ExprKind::TupleLit { elements }
        | ExprKind::GenericEnumCall { args: elements, .. } => {
            for el in elements {
                f(el);
            }
        }
        ExprKind::ArrayFill { fill, .. } => f(fill),
        _ => {}
    }
}

/// Every expression in `e`, itself first, then its sub-expressions —
/// including the ones inside nested blocks, so a capture bound deep in an
/// `if` arm is seen. `walk_expr_children` is the shallow, scope-aware
/// version the stateful walk uses; this one is for the syntactic questions
/// that have no state to keep.
fn for_each_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Block(b) => for_each_expr_in_block(b, f),
        ExprKind::Await(i) | ExprKind::Yield(i) | ExprKind::Cast { expr: i, .. } => {
            for_each_expr(i, f)
        }
        ExprKind::Unary { operand, .. } => for_each_expr(operand, f),
        ExprKind::Field { receiver, .. } => for_each_expr(receiver, f),
        ExprKind::Binary { lhs, rhs, .. } => {
            for_each_expr(lhs, f);
            for_each_expr(rhs, f);
        }
        ExprKind::Assign { target, value, .. } => {
            for_each_expr(target, f);
            for_each_expr(value, f);
        }
        ExprKind::Index { receiver, index } => {
            for_each_expr(receiver, f);
            for_each_expr(index, f);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(x) = start {
                for_each_expr(x, f);
            }
            if let Some(x) = end {
                for_each_expr(x, f);
            }
        }
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            for_each_expr(cond, f);
            for_each_expr_in_block(then, f);
            if let Some(x) = else_branch {
                for_each_expr(x, f);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            for_each_expr(scrutinee, f);
            for a in arms {
                for_each_expr(&a.body, f);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            for_each_expr(callee, f);
            for a in args {
                for_each_expr(a, f);
            }
        }
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            for fl in fields {
                for_each_expr(&fl.value, f);
            }
        }
        ExprKind::ArrayLit { elements }
        | ExprKind::TupleLit { elements }
        | ExprKind::GenericEnumCall { args: elements, .. } => {
            for x in elements {
                for_each_expr(x, f);
            }
        }
        ExprKind::ArrayFill { fill, .. } => for_each_expr(fill, f),
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                for_each_expr(a, f);
            }
        }
        ExprKind::InterpStr { parts } => {
            for pt in parts {
                if let crate::ast::InterpStrPart::Expr(x) = pt {
                    for_each_expr(x, f);
                }
            }
        }
        _ => {}
    }
}

/// `for_each_expr` over every expression a block contains. The `let`-family
/// statements this pass never sees (`if let` and friends are lowered before
/// it runs) are absent by the same reasoning as `ViewRules::walk_stmt`.
fn for_each_expr_in_block(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { init: Some(e), .. }
            | StmtKind::Expr(e)
            | StmtKind::Return(Some(e))
            | StmtKind::Defer(e)
            | StmtKind::Assert(e) => for_each_expr(e, f),
            StmtKind::LetDestructure { init, .. } => for_each_expr(init, f),
            StmtKind::While { cond, body, .. } => {
                for_each_expr(cond, f);
                for_each_expr_in_block(body, f);
            }
            StmtKind::Loop(body, _) => for_each_expr_in_block(body, f),
            StmtKind::For(fl, _) => match fl {
                ForLoop::Range { iter, body, .. } => {
                    for_each_expr(iter, f);
                    for_each_expr_in_block(body, f);
                }
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(StmtKind::Let { init: Some(e), .. }) = init.as_ref().map(|i| &i.kind)
                    {
                        for_each_expr(e, f);
                    }
                    if let Some(c) = cond {
                        for_each_expr(c, f);
                    }
                    for u in update {
                        for_each_expr(u, f);
                    }
                    for_each_expr_in_block(body, f);
                }
            },
            _ => {}
        }
    }
    if let Some(t) = &b.tail {
        for_each_expr(t, f);
    }
}

/// The receiver-capturing method set: every `(Type, method)` whose returned
/// value can carry the address of its receiver, so that `local.m(...)`
/// hands `&local` to whatever comes back.
///
/// Syntactic on purpose — it runs before any body is walked, so a call site
/// is judged the same no matter which order the impls were declared in.
/// `this.name` is a bound reference when `name` is a method of the impl
/// target and NOT a field (a real fn-pointer field is an ordinary read) and
/// it appears somewhere other than callee position (`this.name()` is just a
/// call).
///
/// Iterated to a fixpoint, because capturing is TRANSITIVE through the
/// receiver: `build` may bind nothing itself and just return
/// `this.picker(...)`, where `picker` does the binding. That is the real
/// shape in a component tree, and a single pass reported only the methods
/// that bind directly. The set only grows and is bounded by the number of
/// methods, so it converges in at most that many rounds.
fn receiver_capturing_methods(prog: &Program, sigs: &SigTable) -> BTreeSet<(String, String)> {
    let mut set: BTreeSet<(String, String)> = BTreeSet::new();
    loop {
        let before = set.len();
        for item in &prog.items {
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            let target = &b.target.name;
            let Some(fields) = sigs.struct_fields.get(target) else {
                continue;
            };
            for m in &b.methods {
                // A method returning nothing cannot carry the address out.
                if m.return_type.is_none() {
                    continue;
                }
                let key = (target.clone(), m.name.name.clone());
                if set.contains(&key) {
                    continue;
                }
                let mut binds = false;
                for_each_expr_in_block(&m.body, &mut |e: &Expr| {
                    if binds {
                        return;
                    }
                    let ExprKind::Call { callee, args, .. } = &e.kind else {
                        return;
                    };
                    // A bound reference handed to a call: `f(this.handler)`.
                    for a in args {
                        let ExprKind::Field { receiver, name } = &a.kind else {
                            continue;
                        };
                        if !expr_is_receiver(receiver) {
                            continue;
                        }
                        if !fields.contains_key(&name.name)
                            && sigs
                                .methods
                                .contains_key(&format!("{target}.{}", name.name))
                        {
                            binds = true;
                            return;
                        }
                    }
                    // A call to an already-known capturing method on `this`
                    // hands `this`'s address to whatever it returns.
                    let ExprKind::Field { receiver, name } = &callee.kind else {
                        return;
                    };
                    if expr_is_receiver(receiver)
                        && set.contains(&(target.clone(), name.name.clone()))
                    {
                        binds = true;
                    }
                });
                if binds {
                    set.insert(key);
                }
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// Is this expression the receiver binding itself?
fn expr_is_receiver(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Ident(n) if n == "this" || n == "self")
}

/// Run the definition-site view rules over every function and method body.
fn collect_view_diagnostics(
    prog: &Program,
    sigs: &SigTable,
    oracle: &CopyOracle,
    diags: &mut Vec<(Option<String>, RawDiag)>,
) {
    let mut statics: HashMap<String, Type> = HashMap::new();
    for item in &prog.items {
        if let ItemKind::Static(s) = &item.kind {
            statics.insert(s.name.name.clone(), s.ty.clone());
        }
    }
    let addr_taken = fns_with_address_taken(prog);
    let capturing = receiver_capturing_methods(prog, sigs);
    for item in &prog.items {
        match &item.kind {
            ItemKind::Function(f) => {
                if f.is_extern || f.is_declaration {
                    continue;
                }
                let mut r = ViewRules::new(
                    sigs,
                    oracle,
                    &statics,
                    &capturing,
                    ViewSite::Free {
                        name: f.name.name.clone(),
                        exported: !addr_taken.contains(&f.name.name),
                    },
                    None,
                    None,
                    &f.params,
                    f.return_type.clone(),
                    &f.attributes,
                );
                r.walk_block(&f.body);
                diags.extend(r.diags.into_iter().map(|d| (item.origin_file.clone(), d)));
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if m.is_declaration {
                        continue;
                    }
                    let recv_ty = m.receiver.map(|_| Type {
                        kind: TypeKind::Path(b.target.name.clone()),
                        span: b.target.span,
                    });
                    let mut r = ViewRules::new(
                        sigs,
                        oracle,
                        &statics,
                        &capturing,
                        ViewSite::Method {
                            key: format!("{}.{}", b.target.name, m.name.name),
                        },
                        m.receiver,
                        recv_ty,
                        &m.params,
                        m.return_type.clone(),
                        &m.attributes,
                    );
                    r.walk_block(&m.body);
                    diags.extend(r.diags.into_iter().map(|d| (item.origin_file.clone(), d)));
                }
            }
            ItemKind::Struct(_)
            | ItemKind::Enum(_)
            | ItemKind::Interface(_)
            | ItemKind::TypeAlias(_)
            | ItemKind::Const(_)
            | ItemKind::Static(_)
            | ItemKind::ModuleAsm(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Snapshot-only entry. Returns the per-function state trace; produces
/// no diagnostics. Used by unit tests via `dump()`.
pub fn analyze(prog: &Program) -> ProgramAnalysis {
    let (analysis, _diags) = analyze_with_diags(prog);
    analysis
}

/// Pipeline entry. Runs the same analysis but renders any conflicts as
/// proper `Diagnostic`s against the given file context. Multi-file
/// projects pass the entry file's path / source; per-file routing is a
/// follow-up (sema-style threading via `current_file`).
pub fn check(prog: &Program, file: &Path, src: &str) -> Vec<Diagnostic> {
    check_multi(prog, file, src, &std::collections::BTreeMap::new())
}

/// Multi-file entry: as `check`, but with the loader's per-file source map
/// (keyed by each item's `origin_file`, the same map sema's
/// `check_multi_with_mono` takes). Each raw diagnostic is resolved against
/// the file the offending item actually lives in, so a borrow error inside
/// an imported module names THAT file — not the entry file with a line
/// number from somewhere else. Items without an origin (or with one the
/// map doesn't know) fall back to the entry file, which is also the whole
/// story in single-file mode.
pub fn check_multi(
    prog: &Program,
    entry_file: &Path,
    entry_src: &str,
    files: &std::collections::BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    let (_analysis, raws) = analyze_with_diags(prog);
    if raws.is_empty() {
        return Vec::new();
    }
    let entry_lm = LineMap::new(entry_src);
    // One LineMap per distinct origin file, built on first use.
    let mut lms: std::collections::BTreeMap<&str, LineMap> = std::collections::BTreeMap::new();
    raws.into_iter()
        .map(|(origin, r)| {
            match origin
                .as_deref()
                .and_then(|o| files.get_key_value(o))
            {
                Some((key, (path, fsrc))) => {
                    let lm = lms
                        .entry(key.as_str())
                        .or_insert_with(|| LineMap::new(fsrc));
                    raw_to_diagnostic(r, path, fsrc, lm)
                }
                None => raw_to_diagnostic(r, entry_file, entry_src, &entry_lm),
            }
        })
        .collect()
}

fn raw_to_diagnostic(r: RawDiag, file: &Path, src: &str, lm: &LineMap) -> Diagnostic {
    let suggestions = match r.suggestion {
        Some((span, replacement, description)) => vec![Suggestion {
            applicability: Applicability::MaybeIncorrect,
            description,
            replacement,
            span: lm.span(file, span, src),
        }],
        None => Vec::new(),
    };
    let labels = match r.label {
        Some((span, message)) => vec![crate::diagnostics::Label {
            span: lm.span(file, span, src),
            message,
        }],
        None => Vec::new(),
    };
    Diagnostic {
        severity: Severity::Error,
        code: DiagCode(r.code),
        message: r.message,
        primary: lm.span(file, r.primary, src),
        labels,
        notes: Vec::new(),
        suggestions,
    }
}

/// Slice 6BC.4 — walk every fn / method and emit **E0384** when the
/// signature suggests the user wants to borrow from inputs but the
/// elision-rule body analysis couldn't prove which input. The trigger:
///   - 2+ non-Copy params, non-Copy return
///   - No elision rule matched (FnEntry.return_borrow is None)
///   - The body has *at least one* return rooted at a parameter
///
/// Fresh-value-on-every-path functions stay silent (the return is
/// owned, not borrowed). The diagnostic historically taught the
/// `borrow REGION T` annotation surface, but that source syntax is now
/// retired, so the suggested annotation is no longer writable.
fn collect_e0384_diagnostics(
    prog: &Program,
    sigs: &SigTable,
    oracle: &CopyOracle,
    diags: &mut Vec<(Option<String>, RawDiag)>,
) {
    for item in &prog.items {
        match &item.kind {
            ItemKind::Function(f) => {
                if let Some(d) = e0384_for_fn(f, sigs, oracle) {
                    diags.push((item.origin_file.clone(), d));
                }
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if let Some(d) = e0384_for_method(b, m, sigs, oracle) {
                        diags.push((item.origin_file.clone(), d));
                    }
                }
            }
            ItemKind::Struct(_)
            | ItemKind::Enum(_)
            | ItemKind::Interface(_)
            | ItemKind::TypeAlias(_)
            | ItemKind::Const(_)
            | ItemKind::Static(_)
            | ItemKind::ModuleAsm(_) => {}
        }
    }
}

fn e0384_for_fn(f: &Function, sigs: &SigTable, oracle: &CopyOracle) -> Option<RawDiag> {
    if f.params.len() < 2 {
        return None;
    }
    // Every param must be non-Copy borrow-like (no `take`).
    for p in &f.params {
        if p.move_ {
            return None;
        }
        if !oracle.definitely_non_copy(&p.ty) {
            return None;
        }
    }
    let ret = f.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    // Skip if elision matched.
    let entry = sigs.fns.get(&f.name.name)?;
    if entry.return_borrow.is_some() {
        return None;
    }
    // The trigger: at least one return rooted at a parameter.
    let param_names: Vec<&str> = f.params.iter().map(|p| p.name.name.as_str()).collect();
    if !any_return_rooted_at_param(&f.body, &param_names) {
        return None;
    }
    Some(build_e0384(&f.name.name, &f.params, ret, f.name.span))
}

fn e0384_for_method(
    b: &ImplBlock,
    m: &Method,
    sigs: &SigTable,
    oracle: &CopyOracle,
) -> Option<RawDiag> {
    if m.params.len() < 2 {
        return None;
    }
    for p in &m.params {
        if p.move_ {
            return None;
        }
        if !oracle.definitely_non_copy(&p.ty) {
            return None;
        }
    }
    let ret = m.return_type.as_ref()?;
    if !oracle.definitely_non_copy(ret) {
        return None;
    }
    let key = format!("{}.{}", b.target.name, m.name.name);
    let entry = sigs.methods.get(&key)?;
    if entry.return_borrow.is_some() {
        return None;
    }
    let param_names: Vec<&str> = m.params.iter().map(|p| p.name.name.as_str()).collect();
    if !any_return_rooted_at_param(&m.body, &param_names) {
        return None;
    }
    Some(build_e0384(&key, &m.params, ret, m.name.span))
}

fn build_e0384(name: &str, _params: &[Param], _ret: &Type, span: Span) -> RawDiag {
    RawDiag {
        code: "E0384",
        message: format!(
            "cannot infer which parameter the return of `{name}` borrows from"
        ),
        primary: span,
        suggestion: Some((
            span,
            String::new(),
            // The old `borrow REGION T` annotation is retired and unwritable, so
            // the remedy is structural: a view return is only tracked when it
            // derives from exactly ONE non-Copy parameter.
            "different return paths borrow from different parameters, so the \
             borrow checker cannot pin the result to a single source. Restructure \
             so every return path borrows from the SAME parameter (a view return \
             is tied to exactly one non-Copy parameter), or return an owned value \
             instead of a view."
                .to_string(),
        )),
        label: None,
    }
}

/// Slice 6BC.4 helper: true iff at least one `return EXPR;` in `block`
/// has EXPR rooted at one of the named parameters. The mirror question
/// to `body_returns_only_rooted_at` — that one asks "every return
/// rooted?", this one asks "any return rooted?". Used by E0384
/// detection to distinguish the "wants annotation" case from the
/// "always-fresh-return" case.
fn any_return_rooted_at_param(block: &Block, param_names: &[&str]) -> bool {
    for s in &block.stmts {
        if any_return_rooted_in_stmt(s, param_names) {
            return true;
        }
    }
    if let Some(t) = &block.tail {
        if any_return_rooted_in_expr(t, param_names) {
            return true;
        }
    }
    false
}

fn any_return_rooted_in_stmt(s: &Stmt, param_names: &[&str]) -> bool {
    match &s.kind {
        StmtKind::Return(Some(e)) => {
            expr_root_ident(e).is_some_and(|root| param_names.contains(&root))
        }
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => false,
        StmtKind::Let { init, .. } => init
            .as_ref()
            .is_some_and(|e| any_return_rooted_in_expr(e, param_names)),
        StmtKind::LetDestructure { init, .. } => any_return_rooted_in_expr(init, param_names),
        StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
            any_return_rooted_in_expr(e, param_names)
        }
        StmtKind::While { cond, body, .. } => {
            any_return_rooted_in_expr(cond, param_names)
                || any_return_rooted_at_param(body, param_names)
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                init.as_deref()
                    .is_some_and(|i| any_return_rooted_in_stmt(i, param_names))
                    || cond
                        .as_ref()
                        .is_some_and(|c| any_return_rooted_in_expr(c, param_names))
                    || update
                        .iter()
                        .any(|u| any_return_rooted_in_expr(u, param_names))
                    || any_return_rooted_at_param(body, param_names)
            }
            ForLoop::Range { iter, body, .. } => {
                any_return_rooted_in_expr(iter, param_names)
                    || any_return_rooted_at_param(body, param_names)
            }
        },
        StmtKind::Loop(body, _) => any_return_rooted_at_param(body, param_names),
        // Lowered before borrowck — should not be present here.
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => false,
    }
}

fn any_return_rooted_in_expr(e: &Expr, param_names: &[&str]) -> bool {
    match &e.kind {
        ExprKind::Block(b) => any_return_rooted_at_param(b, param_names),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            any_return_rooted_in_expr(cond, param_names)
                || any_return_rooted_at_param(then, param_names)
                || else_branch
                    .as_deref()
                    .is_some_and(|eb| any_return_rooted_in_expr(eb, param_names))
        }
        ExprKind::Match { scrutinee, arms } => {
            any_return_rooted_in_expr(scrutinee, param_names)
                || arms
                    .iter()
                    .any(|a| any_return_rooted_in_expr(&a.body, param_names))
        }
        _ => false,
    }
}

/// The erased-`*u8` seam (bugs/str-field-outliving-its-text-is-not-caught,
/// round three): `Sink{key} → box::new → into_raw() -> *u8` moves a view
/// out through a return type that no longer names it, so every type-gated
/// rule skips and the caller never learns the tie. The body flow knows,
/// though: when a fn returns a raw pointer and its computed return flow
/// carries a parameter that is a view or could own viewed bytes, promote
/// that flow to a `return_borrow` — the caller then ties the result to the
/// argument's owner through the ordinary E-rule machinery, and the scope
/// checks (E0514/E0372) fire with no new diagnostic.
///
/// Scope, deliberately narrow:
/// - raw-pointer returns only — the erasure seam. Other Copy returns
///   (`as u64` laundering) stay out of contract §4.
/// - non-`take` params only, and only view-typed / view-carrying /
///   definitely-non-Copy ones — the same filter as Rule E-VIEW-FN. A moved
///   param's bytes belong to the callee (the box owns them); a scalar has
///   no owner to tie.
/// - `#[keeps(nothing)]` opts out, same as the detection ladder.
/// - the ladder's own verdict wins: an existing `return_borrow` is never
///   overwritten.
///
/// Self-referential carriers stay sound with no special case here: their
/// views root at the receiver's own fields, not at any parameter, so no
/// param bit ever reaches the return and nothing is promoted.
fn promote_erased_return_flows(prog: &Program, oracle: &CopyOracle, sigs: &mut SigTable) {
    for item in &prog.items {
        let ItemKind::Function(f) = &item.kind else {
            continue;
        };
        if crate::attrs::has_keeps(&f.attributes, "nothing") {
            continue;
        }
        let Some(rt) = f.return_type.as_ref() else {
            continue;
        };
        if !matches!(rt.kind, TypeKind::RawPtr(_)) {
            continue;
        }
        let name = &f.name.name;
        let bits = match sigs.fns.get(name) {
            Some(e) if e.return_borrow.is_none() => match e.computed_ret_flow {
                Some(b) if b != 0 => b,
                _ => continue,
            },
            _ => continue,
        };
        let mut indices: Vec<u32> = Vec::new();
        for (i, p) in f.params.iter().enumerate() {
            if i >= 64 || bits & (1u64 << i) == 0 {
                continue;
            }
            if p.move_ {
                continue;
            }
            if !(oracle.type_contains_view(&p.ty) || oracle.definitely_non_copy(&p.ty)) {
                continue;
            }
            indices.push(i as u32);
        }
        let rb = match indices.len() {
            0 => continue,
            1 => ReturnBorrowSource::Param(indices[0]),
            _ => ReturnBorrowSource::MultiParam(indices),
        };
        if let Some(e) = sigs.fns.get_mut(name) {
            e.return_borrow = Some(rb);
            e.return_borrow_flavor = Some(BorrowFlavor::Shared);
        }
    }
}

fn analyze_with_diags(prog: &Program) -> (ProgramAnalysis, Vec<(Option<String>, RawDiag)>) {
    let oracle = CopyOracle::build(prog);
    let mut sigs = SigTable::collect(prog, &oracle);
    // Contract §5: patch computed receiver flows (transitive keeps) into
    // the method entries before any body is analyzed.
    compute_receiver_flows(prog, &mut sigs);
    // Erased-boundary closing (2026-08-04): a raw-pointer return whose
    // computed body flow carries a view-capable parameter is a return
    // borrow, exactly as if the type had named the view.
    promote_erased_return_flows(prog, &oracle, &mut sigs);
    let sigs = sigs;
    let mut analysis = ProgramAnalysis {
        functions: BTreeMap::new(),
    };
    // Each raw diagnostic is paired with its item's `origin_file` so the
    // multi-file entry (`check_multi`) can render it against the file the
    // code actually lives in.
    let mut all_diags: Vec<(Option<String>, RawDiag)> = Vec::new();
    // Slice 6BC.4 — signature-level E0384 emission. Walks every fn /
    // method whose signature matches the "wants elision but can't be
    // proven" pattern: 2+ non-Copy params, non-Copy return, no
    // elision rule matched, and the body has at least one return
    // rooted at a parameter (indicating the user wants to borrow
    // from inputs but the body-flow analysis can't prove which).
    // Fresh-value-on-every-path functions stay silent — the return
    // is owned, no annotation needed.
    collect_e0384_diagnostics(prog, &sigs, &oracle, &mut all_diags);
    // Memory-model contract §3.1 (issue-07) — the definition-site view
    // rules: a view escaping the frame it is rooted in. Syntax-directed
    // like the E0384 pass above, and reading the same signature table the
    // flow pass publishes, so there is one answer about what ties.
    collect_view_diagnostics(prog, &sigs, &oracle, &mut all_diags);
    for item in &prog.items {
        match &item.kind {
            ItemKind::Function(f) => {
                let mut a = Analyzer::new(&sigs, &oracle);
                let fa = a.analyze_function(&f.name.name, &f.params, &f.body);
                analysis.functions.insert(f.name.name.clone(), fa);
                let origin = &item.origin_file;
                all_diags.extend(a.diags.into_iter().map(|d| (origin.clone(), d)));
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    let mut a = Analyzer::new(&sigs, &oracle);
                    let key = format!("{}.{}", b.target.name, m.name.name);
                    let fa = a.analyze_method(&key, &b.target.name, m.receiver, &m.params, &m.body);
                    analysis.functions.insert(key, fa);
                    let origin = &item.origin_file;
                    all_diags.extend(a.diags.into_iter().map(|d| (origin.clone(), d)));
                }
            }
            ItemKind::Struct(_)
            | ItemKind::Enum(_)
            | ItemKind::Interface(_)
            | ItemKind::TypeAlias(_)
            | ItemKind::Const(_)
            | ItemKind::Static(_)
            | ItemKind::ModuleAsm(_) => {}
        }
    }
    dedupe_conflicts_at_one_span(&mut all_diags);
    (analysis, all_diags)
}

/// ONE CONFLICT, ONE DIAGNOSTIC.
///
/// A place expression is analysed as a read wherever it appears, including as
/// an assignment TARGET and as a `ref` ARGUMENT — positions where it is not a
/// read at all. While an exclusive loan is live that produced two errors on
/// one span, the read-flavoured one first and wrongly:
///
/// ```text
/// d.n = 5;    E0374 cannot READ `d.n` while it overlaps ... exclusive borrow
///             E0381 cannot write to `d` while it is borrowed by `s`
/// touch(d);   E0381 cannot borrow `d` exclusively while it is borrowed by `s`
///             E0383 cannot READ `d` while it is exclusively borrowed by `s`
/// ```
///
/// Both members of each pair describe the same conflict, so the claim-flavoured
/// code (which names what the code was actually trying to do) wins and the
/// read-flavoured one is dropped. Nothing is suppressed that stands alone: a
/// genuine read of an exclusively-borrowed place is the only diagnostic at its
/// span and survives untouched.
fn dedupe_conflicts_at_one_span(diags: &mut Vec<(Option<String>, RawDiag)>) {
    const CLAIM: [&str; 4] = ["E0370", "E0380", "E0381", "E0382"];
    const READ: [&str; 2] = ["E0374", "E0383"];
    let claimed: std::collections::BTreeSet<(u32, u32)> = diags
        .iter()
        .filter(|(_, d)| CLAIM.contains(&d.code))
        .map(|(_, d)| (d.primary.start, d.primary.end))
        .collect();
    diags.retain(|(_, d)| {
        !READ.contains(&d.code) || !claimed.contains(&(d.primary.start, d.primary.end))
    });
}

// ---------------------------------------------------------------------------
// Walker
// ---------------------------------------------------------------------------

impl Analyzer<'_> {
    fn analyze_function(&mut self, name: &str, params: &[Param], body: &Block) -> FunctionAnalysis {
        let mut state: BTreeMap<Place, PlaceState> = BTreeMap::new();
        for p in params {
            self.binding_types
                .insert(p.name.name.clone(), BindingType::Known(p.ty.clone()));
            state.insert(Place::root(&p.name.name), PlaceState::Owned);
        }
        self.walk_body(name, body, state)
    }

    fn analyze_method(
        &mut self,
        name: &str,
        target_type: &str,
        receiver: Option<Receiver>,
        params: &[Param],
        body: &Block,
    ) -> FunctionAnalysis {
        let mut state: BTreeMap<Place, PlaceState> = BTreeMap::new();
        if receiver.is_some() {
            // `this`'s type is the impl block's target. Build a synthetic
            // `Type` so the oracle can answer.
            let synth = Type {
                kind: TypeKind::Path(target_type.to_string()),
                span: Span::new(0, 0),
            };
            self.binding_types
                .insert("self".to_string(), BindingType::Known(synth));
            state.insert(Place::root("self"), PlaceState::Owned);
        }
        for p in params {
            self.binding_types
                .insert(p.name.name.clone(), BindingType::Known(p.ty.clone()));
            state.insert(Place::root(&p.name.name), PlaceState::Owned);
        }
        self.walk_body(name, body, state)
    }

    fn walk_body(
        &mut self,
        name: &str,
        body: &Block,
        initial: BTreeMap<Place, PlaceState>,
    ) -> FunctionAnalysis {
        let mut state = initial;
        let mut points = Vec::with_capacity(body.stmts.len() + 2);
        points.push(PointSnapshot {
            label: "entry".into(),
            state: state.clone(),
        });

        let nll = self.nll_release_schedule(body);
        for (i, stmt) in body.stmts.iter().enumerate() {
            self.apply_stmt(stmt, &mut state);
            // NLL: end borrows whose last mention was this statement.
            for name in &nll[i] {
                self.drop_borrower(name, &mut state);
            }
            points.push(PointSnapshot {
                label: format!("after stmt {i}"),
                state: state.clone(),
            });
        }
        if let Some(tail) = &body.tail {
            self.apply_expr(tail, &mut state);
        }
        points.push(PointSnapshot {
            label: "exit".into(),
            state,
        });

        FunctionAnalysis {
            name: name.into(),
            points,
        }
    }

    fn apply_stmt(&mut self, stmt: &Stmt, state: &mut BTreeMap<Place, PlaceState>) {
        match &stmt.kind {
            StmtKind::Let { name, ty, init, .. } => {
                let mut borrow_sources: Vec<(Place, BorrowFlavor)> = Vec::new();
                if let Some(e) = init {
                    // 5BC.3b/5BC.4/6BC.2: classify *before* walking. The
                    // walk's call-handler does the regular state
                    // transitions (move-arg → Moved, etc.); the
                    // borrow-acquire happens after so it sees the
                    // post-walk state.
                    borrow_sources = self.classify_borrow_source(e);
                    // Rule E-VIEW, bare-coercion arm (2026-07-06): binding a
                    // view-typed name (`str` / slice) straight to a non-Copy
                    // place — `let s: str = t;` — coerces owner → view with
                    // no call involved. The binding borrows the place, same
                    // as `let s: str = t.view();`.
                    if borrow_sources.is_empty() {
                        if let Some(decl) = ty {
                            let is_view_ty = matches!(&decl.kind, TypeKind::Slice(_))
                                || matches!(&decl.kind, TypeKind::Path(p) if p == "str");
                            if is_view_ty {
                                if let Some(place) = place_from_expr(e) {
                                    if self.binding_is_non_copy(&place.root) {
                                        borrow_sources = vec![(place, BorrowFlavor::Shared)];
                                    }
                                }
                            }
                        }
                    }
                    // Move-site symmetry (2026-07-07): `let y: T = x;` with a
                    // non-Copy `x` consumes it exactly like a take-call arg —
                    // the move must be checked against live view borrows
                    // (E0372) and transition the place. Only the view-
                    // coercion case above is a borrow, not a move. Before
                    // this, `let t2: Buf = t;` moved `t` out from under a
                    // live `let w: str = t.view();` with no diagnostic.
                    match &e.kind {
                        ExprKind::Ident(src)
                            if borrow_sources.is_empty() && self.binding_is_non_copy(src) =>
                        {
                            let src = src.clone();
                            self.apply_move_of_binding(&src, e.span, state);
                        }
                        _ => self.apply_expr(e, state),
                    }
                }
                let bt = match ty {
                    Some(t) => BindingType::Known(t.clone()),
                    None => BindingType::Unknown,
                };
                if ty.is_none() {
                    if let Some(inferred) = init.as_ref().and_then(|e| self.infer_expr_type(e)) {
                        self.inferred_types.insert(name.name.clone(), inferred);
                    }
                }
                self.binding_types.insert(name.name.clone(), bt);
                state.insert(Place::root(&name.name), PlaceState::Owned);
                // 5BC.3b/5BC.4/6BC.2: acquire borrows if the initializer
                // was a borrow-returning call. The new binding becomes a
                // borrower of every source place. Source state becomes
                // BorrowedShared(N) for shared borrows (Phase 5) or
                // BorrowedExclusive(name) for exclusive ones (6BC.2).
                if !borrow_sources.is_empty() {
                    self.acquire_borrows(borrow_sources, &name.name, name.span, state);
                }
            }
            StmtKind::LetDestructure { fields, init, .. } => {
                // Destructuring consumes `init` wholly and re-owns each field as
                // a new binding. Walk the init for its sub-expression
                // transitions, then mark every field binding Owned.
                //
                // Rule E-VIEW through destructure (2026-07-22): when the `init`
                // aggregate embeds a view (`let Slot { s } = Slot { s: t.view()
                // };`), the moved-out field is a *borrow*, not an owned
                // resource — the same view the non-destructure form
                // (`let w = Slot { s: t.view() }`) already pins. Classify the
                // init's borrow sources and record each field binding as a
                // borrower, so moving/dropping the owner while any field is
                // live fires E0372. Conservative: union every source onto every
                // field (destructured fields share one scope and release
                // together, so over-pinning is harmless). Owning-field
                // destructures classify to no sources and stay plain `Owned`.
                let borrow_sources = self.classify_borrow_source(init);
                let init_ty = self.infer_expr_type(init);
                if let Some(Type {
                    kind: TypeKind::Path(sname),
                    ..
                }) = &init_ty
                {
                    if let Some(fs) = self.sigs.struct_fields.get(sname) {
                        let hits: Vec<(String, Type)> = fields
                            .iter()
                            .filter_map(|f| fs.get(&f.name).map(|t| (f.name.clone(), t.clone())))
                            .collect();
                        for (n, t) in hits {
                            self.inferred_types.insert(n, t);
                        }
                    }
                }
                self.apply_expr(init, state);
                for f in fields {
                    self.binding_types
                        .insert(f.name.clone(), BindingType::Unknown);
                    state.insert(Place::root(&f.name), PlaceState::Owned);
                    if !borrow_sources.is_empty() {
                        self.acquire_borrows(borrow_sources.clone(), &f.name, f.span, state);
                    }
                }
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) | StmtKind::Defer(e) => {
                self.apply_expr(e, state);
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Assert(e) => {
                // The condition expression is evaluated like any other
                // place-producing read. Behavior on the trap path is
                // codegen's concern; here we just walk the AST.
                self.apply_expr(e, state);
            }
            StmtKind::While { cond, body, .. } => {
                self.apply_expr(cond, state);
                self.walk_loop_body(body, state);
            }
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(i) = init {
                        self.apply_stmt(i, state);
                    }
                    if let Some(c) = cond {
                        self.apply_expr(c, state);
                    }
                    for u in update {
                        self.apply_expr(u, state);
                    }
                    self.walk_loop_body(body, state);
                }
                ForLoop::Range { var, iter, body } => {
                    self.apply_expr(iter, state);
                    // Range loop var is the range's element type. For
                    // numeric ranges (the only kind C+ has today) it's
                    // some integer — always Copy. Record as i32 so the
                    // oracle answers "Copy" without us guessing the width.
                    let synth = Type {
                        kind: TypeKind::Path("i32".to_string()),
                        span: var.span,
                    };
                    self.binding_types
                        .insert(var.name.clone(), BindingType::Known(synth));
                    let mut body_state = state.clone();
                    body_state.insert(Place::root(&var.name), PlaceState::Owned);
                    let pre_loop = state.clone();
                    self.walk_block_in_scope(body, &mut body_state, &pre_loop);
                    *state = merge_branches(&pre_loop, &[&pre_loop, &body_state], &[false, false]);
                }
            },
            StmtKind::Loop(b, _) => self.walk_loop_body(b, state),
            // Lowered away by `crate::lower`.
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {}
        }
    }

    /// NLL borrow ends (2026-08-13): per-statement release schedule for one
    /// block. `schedule[i]` holds the `let`-declared binding names whose LAST
    /// mention in this block is statement `i` — after statement `i` completes,
    /// `drop_borrower` runs for each, ending their borrows at last use instead
    /// of scope exit (`let v: str = t; io::println(v); t.append("!")` is
    /// admitted: `v` is dead at the append).
    ///
    /// Soundness shape: the release point is the top-level statement OF THE
    /// DECLARING BLOCK containing the last mention, so a use inside a nested
    /// loop pins the borrow past the whole loop (the back edge re-reads it),
    /// and a use inside one `if` branch pins it past the join. "Mention" is
    /// any `Ident` in the statement's subtree — an over-approximation (a
    /// shadowing arm binding counts), which only ever HOLDS a borrow longer,
    /// never releases early. Two positions pin a name to scope exit outright:
    /// the block's tail expression, and any `defer` (defers execute at scope
    /// exit regardless of where they appear). A name never mentioned after
    /// its `let` releases immediately — the borrow was never observable.
    /// Scope-exit cleanup still runs for everything scheduled here;
    /// `drop_borrower` is idempotent.
    ///
    /// DROP LIVENESS (2026-08-22). A binding whose type runs a destructor is
    /// never released early, because the destructor is a use that happens
    /// after every textual mention. `thread::Scope` is the case that made
    /// this a soundness hole rather than a precision one: its `drop` joins
    /// the workers, so between `s.lend(c, w)` and the end of the scope the
    /// worker is still writing `c` — but `s` is not MENTIONED again, so the
    /// loan was released at the lend and the parent could read, write and
    /// re-lend `c` freely. The three refusals `thread.cplus` documents were
    /// reachable only by naming the scope again afterwards.
    ///
    /// This is the same rule Rust's NLL applies to a type with a `Drop` impl,
    /// and it can only ever HOLD a loan longer than before.
    fn nll_release_schedule(&self, b: &Block) -> Vec<Vec<String>> {
        use std::collections::BTreeSet;
        let n = b.stmts.len();
        let mut schedule: Vec<Vec<String>> = vec![Vec::new(); n];
        if n == 0 {
            return schedule;
        }
        let mention_set = |s: &Stmt| -> BTreeSet<String> {
            let mut set = BTreeSet::new();
            let one = Block {
                stmts: vec![s.clone()],
                tail: None,
                span: s.span,
            };
            crate::ast::visit_exprs_in_block(&one, &mut |e| {
                if let ExprKind::Ident(name) = &e.kind {
                    set.insert(name.clone());
                }
            });
            set
        };
        let mentions: Vec<BTreeSet<String>> = b.stmts.iter().map(mention_set).collect();
        let mut pinned: BTreeSet<String> = BTreeSet::new();
        if let Some(t) = &b.tail {
            crate::ast::visit_exprs(t, &mut |e| {
                if let ExprKind::Ident(name) = &e.kind {
                    pinned.insert(name.clone());
                }
            });
        }
        for (i, s) in b.stmts.iter().enumerate() {
            if matches!(s.kind, StmtKind::Defer(_)) {
                pinned.extend(mentions[i].iter().cloned());
            }
        }
        for (i, s) in b.stmts.iter().enumerate() {
            let names: Vec<String> = match &s.kind {
                StmtKind::Let { name, ty, init, .. } => {
                    // A destructor runs after the last mention, so a binding
                    // that has one is never released early. The type comes
                    // from the STATEMENT, not from `binding_type`: this
                    // schedule is computed before the block is walked, so
                    // nothing declared in it has been recorded yet.
                    let declared = ty.clone().or_else(|| {
                        init.as_ref().and_then(|e| self.infer_expr_type(e))
                    });
                    if declared.is_some_and(|t| self.oracle.type_has_drop(&t)) {
                        continue;
                    }
                    vec![name.name.clone()]
                }
                StmtKind::LetDestructure { fields, .. } => {
                    fields.iter().map(|f| f.name.clone()).collect()
                }
                _ => continue,
            };
            for nm in names {
                if pinned.contains(&nm) {
                    continue;
                }
                let last = (i + 1..n)
                    .filter(|&j| mentions[j].contains(&nm))
                    .next_back()
                    .unwrap_or(i);
                schedule[last].push(nm);
            }
        }
        schedule
    }

    /// Walk a block whose state must be scope-restricted to bindings that
    /// existed at `outer`. Bindings introduced inside the block are
    /// discarded from `state` on exit so they don't leak to subsequent
    /// statements.
    fn walk_block_in_scope(
        &mut self,
        b: &Block,
        state: &mut BTreeMap<Place, PlaceState>,
        outer: &BTreeMap<Place, PlaceState>,
    ) {
        let nll = self.nll_release_schedule(b);
        for (i, s) in b.stmts.iter().enumerate() {
            self.apply_stmt(s, state);
            // NLL: this statement was the last mention of these borrowers —
            // their borrows end here, not at scope exit.
            for name in &nll[i] {
                self.drop_borrower(name, state);
            }
        }
        if let Some(t) = &b.tail {
            self.apply_expr(t, state);
        }
        // 5BC.3b: release any borrows held by bindings that are about to
        // be dropped (block-local bindings not present in `outer`).
        // This decrements the source-place's `BorrowedShared(N)` count or
        // restores it to `Owned` when the last borrower dies.
        let dying: std::collections::BTreeSet<String> = state
            .keys()
            .filter(|k| !outer.contains_key(*k))
            .map(|k| k.root.clone())
            .collect();
        for borrower in &dying {
            self.drop_borrower(borrower, state);
        }
        // E0514 (memory-model contract §3.3): a block-local owner may not
        // die while a borrower declared outside the block still holds a
        // view of it. Dying borrowers released their claims above, so any
        // borrower still registered against a dying place outlives its
        // owner — the view dangles the moment the block ends. One report
        // per owner root; the edges are then dropped so a reported escape
        // doesn't cascade into unrelated E0372/E0383 noise.
        let mut escaped_owners: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut escapes: Vec<(String, String, Span)> = Vec::new();
        for (place, borrowers) in self.live_borrows.iter() {
            if !dying.contains(&place.root) || !escaped_owners.insert(place.root.clone()) {
                continue;
            }
            if let Some((borrower, span)) = borrowers.iter().next() {
                escapes.push((place.root.clone(), borrower.clone(), *span));
            }
        }
        for (owner, borrower, borrow_span) in escapes {
            self.diags.push(RawDiag {
                code: "E0514",
                message: format!(
                    "`{owner}` does not live long enough: `{borrower}` still borrows it when `{owner}` goes out of scope"
                ),
                primary: borrow_span,
                suggestion: Some((
                    borrow_span,
                    String::new(),
                    format!(
                        "`{borrower}` outlives this block but views memory owned by `{owner}`, \
                         which is dropped at the block's end. Declare `{borrower}` inside the \
                         block, extend `{owner}`'s scope past the last use of `{borrower}`, or \
                         store an owned value instead (`Text`, or `text::intern` for a \
                         process-lifetime view)."
                    ),
                )),
                label: None,
            });
        }
        self.live_borrows.retain(|p, _| !dying.contains(&p.root));
        // Drop branch-local bindings (keys not in `outer`).
        state.retain(|k, _| outer.contains_key(k));
    }

    /// Loop body: state changes inside the body merge back with pre-loop
    /// state via `merge_branches`, modeling "body might not run." Any
    /// move inside the body becomes `MaybePartial` post-loop.
    fn walk_loop_body(&mut self, b: &Block, state: &mut BTreeMap<Place, PlaceState>) {
        let pre_loop = state.clone();
        let mut body_state = state.clone();
        self.walk_block_in_scope(b, &mut body_state, &pre_loop);
        *state = merge_branches(&pre_loop, &[&pre_loop, &body_state], &[false, false]);
    }

    fn apply_expr(&mut self, e: &Expr, state: &mut BTreeMap<Place, PlaceState>) {
        match &e.kind {
            // v0.0.22 DSL.2: never reached — builder blocks desugar to
            // ordinary AST before the borrow checker runs.
            ExprKind::BuilderBlock { .. } => {}
            // A value-turbofish takes a function's address; no place is read
            // or moved.
            ExprKind::FnRef { .. } => {}
            ExprKind::IntLit(_, _)
            | ExprKind::FloatLit(_, _)
            | ExprKind::BoolLit(_)
            | ExprKind::StrLit(_)
            | ExprKind::CStrLit(_)
            | ExprKind::IncludeBytes { .. }
            | ExprKind::IncludeStr { .. }
            | ExprKind::EnvVar { .. }
            | ExprKind::Path { .. } => {}
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    self.apply_expr(a, state);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    self.apply_expr(&op.value, state);
                }
            }

            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let crate::ast::InterpStrPart::Expr(e) = p {
                        self.apply_expr(e, state);
                    }
                }
            }

            ExprKind::Ident(name) => {
                self.record_read(name, e.span, state);
            }

            // Slice 6BC.3: a Field/Index chain rooted at an Ident is a
            // *place*. Compute the full Place and do a place-aware
            // read check that respects projections — recursing into
            // the receiver would record a read at the root level
            // (e.g. `p.right` would mis-record as a read of `p`).
            // Non-place chains (e.g. `foo().field` where the receiver
            // is a call) fall through to the per-kind cases below.
            ExprKind::Field { .. } | ExprKind::Index { .. } if place_from_expr(e).is_some() => {
                let place = place_from_expr(e).unwrap();
                self.record_place_read(&place, e.span, state);
                // Sub-expressions of an Index (the index expr) need
                // their own walk because index isn't part of the place.
                if let ExprKind::Index { index, .. } = &e.kind {
                    self.apply_expr(index, state);
                }
            }

            ExprKind::Block(b) => {
                let outer = state.clone();
                self.walk_block_in_scope(b, state, &outer);
            }
            // v0.0.3 Phase 5 Slice 5E.1: `await EXPR` evaluates EXPR
            // (the Future) and then suspends. From a borrow-checker
            // standpoint the inner expr's side effects flow through;
            // the suspend itself doesn't change Place state. (5E.4
            // adds the cross-await borrow-lifetime check on top.)
            ExprKind::Await(inner) => {
                self.apply_expr(inner, state);
            }
            // v0.0.4 Phase 4 Slice 4A: yield's value flows through; the
            // suspend itself doesn't change Place state.
            ExprKind::Yield(inner) => {
                self.apply_expr(inner, state);
            }
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.apply_expr(cond, state);
                let pre = state.clone();
                let mut then_state = pre.clone();
                self.walk_block_in_scope(then, &mut then_state, &pre);
                let then_diverges = crate::lower::block_diverges(then);

                let (else_state, else_diverges) = match else_branch {
                    Some(eb) => {
                        let mut s = pre.clone();
                        self.apply_expr(eb, &mut s);
                        // Branch-restrict on the else expression too —
                        // expr blocks (Block / If) introduce their own
                        // scopes via the inner walk_block_in_scope call;
                        // for non-block exprs the state is already in the
                        // pre keyset.
                        s.retain(|k, _| pre.contains_key(k));
                        (s, crate::lower::expr_diverges(eb))
                    }
                    None => (pre.clone(), false),
                };

                *state = merge_branches(
                    &pre,
                    &[&then_state, &else_state],
                    &[then_diverges, else_diverges],
                );
            }
            ExprKind::Call { callee, args, .. } => self.apply_call(callee, args, state),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.apply_expr(lhs, state);
                self.apply_expr(rhs, state);
            }
            ExprKind::Unary { operand, .. } => self.apply_expr(operand, state),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.apply_expr(s, state);
                }
                if let Some(en) = end {
                    self.apply_expr(en, state);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                // Re-initialization heal: assigning a whole binding that is
                // DEFINITELY moved makes it Owned again, so a branch that
                // does `consume(v); v = mk();` presents a live `v` at the
                // join (no E0371). Only the definite state heals —
                // MaybePartial stays an error because codegen has no drop
                // flags: `v = mk()` after a *conditional* move can't decide
                // statically whether to drop the old value.
                let heal = match &target.kind {
                    ExprKind::Ident(name) => matches!(
                        state.get(&Place::root(name)),
                        Some(PlaceState::Moved)
                    ),
                    _ => false,
                };
                if !heal {
                    self.apply_expr(target, state);
                }
                // v0.0.28: a WRITE into a place someone else is borrowing is
                // the same conflict as a mutating method call on it (E0381,
                // `check_method_receiver_claim`) — the call form was checked
                // and the plain assignment was not, so `d.n = 5;` slipped
                // past a live borrow of `d` while `d.set(5);` did not.
                //
                // What made the gap matter: a scoped thread borrows a
                // parent local for the length of the scope, and the parent
                // writing a field of it during that window is a data race
                // no destructor ordering can fix.
                self.check_write_against_borrows(target, state);
                // Rule E-VIEW on REASSIGNMENT (mirror of the `StmtKind::Let`
                // arm): classify the value's borrow sources *before* the walk,
                // so a view-returning RHS (`s = t.view()`) records the target
                // as a borrower of the owner. Without this, `s = t.view();`
                // left the owner `Owned` with zero borrows, so safe code could
                // move/drop it out from under the still-live view (`let t2 =
                // t;`) — a use-after-free the `let s: str = t.view();` form
                // already rejects (E0372).
                let borrow_sources = self.classify_borrow_source(value);
                self.apply_expr(value, state);
                if heal {
                    if let ExprKind::Ident(name) = &target.kind {
                        state.insert(Place::root(name), PlaceState::Owned);
                    }
                }
                // Only simple-local targets participate in borrow tracking
                // (borrows are keyed by binding name). Reassigning the local
                // ends whatever it previously borrowed, then it borrows the new
                // sources — the same acquire the `let` form runs.
                if let ExprKind::Ident(name) = &target.kind {
                    self.drop_borrower(name, state);
                    let mut sources = borrow_sources;
                    if sources.is_empty() {
                        // Bare-coercion arm (mirror of the Let arm): `s = t;`
                        // where `s` is view-typed (`str` / slice) and `t` is a
                        // non-Copy owner coerces owner → view with no call, so
                        // `s` borrows `t` just like `s = t.view();`.
                        let target_is_view = self
                            .binding_type(name)
                            .map(|t| {
                                matches!(&t.kind, TypeKind::Slice(_))
                                    || matches!(&t.kind, TypeKind::Path(p) if p == "str")
                            })
                            .unwrap_or(false);
                        if target_is_view {
                            if let Some(place) = place_from_expr(value) {
                                if self.binding_is_non_copy(&place.root) {
                                    sources = vec![(place, BorrowFlavor::Shared)];
                                }
                            }
                        }
                    }
                    if !sources.is_empty() {
                        self.acquire_borrows(sources, name, target.span, state);
                    }
                } else if !borrow_sources.is_empty() {
                    // Rule E-VIEW into a *projection* target (2026-07-22):
                    // `w.s = t.view()` / `arr[0] = t.view()` stores a view into
                    // a field/index of a local aggregate. The aggregate root
                    // becomes a borrower of the owner, so moving/dropping the
                    // owner while the aggregate is live fires E0372 — the same
                    // pin the construction form `let w = Slot { s: t.view() }`
                    // records. Keyed under the root binding so the block's
                    // scope-exit release (which drops by place root) frees it
                    // when the aggregate leaves scope. Deliberately not
                    // `drop_borrower`'d first: a field reassignment keeps the
                    // prior owner conservatively pinned (a safe
                    // over-approximation) rather than under-pinning a sibling
                    // view field — the field-granular alternative would need
                    // per-projection borrower tracking.
                    if let Some(root_place) = place_from_expr(target) {
                        if !root_place.projections.is_empty() {
                            self.acquire_borrows(
                                borrow_sources,
                                &root_place.root,
                                target.span,
                                state,
                            );
                        }
                    }
                }
            }
            ExprKind::Cast { expr, .. } | ExprKind::CastChecked { expr, .. } => {
                self.apply_expr(expr, state)
            }
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    self.apply_aggregate_element(&f.value, state);
                }
            }
            ExprKind::Field { receiver, .. } => self.apply_expr(receiver, state),
            ExprKind::ArrayFill { fill, .. } => {
                self.apply_expr(fill, state);
            }
            ExprKind::GenericEnumCall {
                enum_name,
                args: elements,
                ..
            } => {
                // Enum-variant constructors capture payloads by value (moves);
                // this node also encodes generic-struct ASSOCIATED-fn calls,
                // whose args follow the callee's param modes — those stay
                // plain reads (conservative).
                let is_enum_ctor = self.sigs.enums.contains(&enum_name.name);
                for el in elements {
                    if is_enum_ctor {
                        self.apply_aggregate_element(el, state);
                    } else {
                        self.apply_expr(el, state);
                    }
                }
            }
            ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
                for el in elements {
                    self.apply_aggregate_element(el, state);
                }
            }
            ExprKind::Index { receiver, index } => {
                self.apply_expr(receiver, state);
                self.apply_expr(index, state);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.apply_expr(scrutinee, state);
                if arms.is_empty() {
                    return;
                }
                let pre = state.clone();
                let mut arm_states = Vec::with_capacity(arms.len());
                let mut arm_diverges = Vec::with_capacity(arms.len());
                for a in arms {
                    let mut s = pre.clone();
                    // Pattern bindings are scope-local to the arm (we
                    // don't register them in `state` at all — they aren't
                    // visible from outside the arm). For tracking inside
                    // the arm, the existing `apply_expr` walk on the arm
                    // body is enough.
                    //
                    // Unannotated-binding inference: payload bindings get
                    // their DECLARED positional types (generic enums
                    // substituted via the pattern's type args, else via
                    // the scrutinee's), so a payload-bound receiver still
                    // resolves its methods. Shadowing is E0363, so a
                    // per-arm overwrite is exact for this arm's body.
                    self.register_pattern_types(&a.pattern, scrutinee);
                    self.apply_expr(&a.body, &mut s);
                    s.retain(|k, _| pre.contains_key(k));
                    arm_diverges.push(crate::lower::expr_diverges(&a.body));
                    arm_states.push(s);
                }
                let refs: Vec<&BTreeMap<Place, PlaceState>> = arm_states.iter().collect();
                *state = merge_branches(&pre, &refs, &arm_diverges);
            }
        }
    }

    /// Record a read of `name`. If state is `MaybePartial` and the
    /// binding is non-Copy, emit E0371. `Moved` reads are intentionally
    /// not caught here — sema's E0335 handles those.
    /// Slice 6BC.3: place-aware variant of `record_read`. Used when a
    /// Field/Index chain is the read target — operates at the full
    /// place granularity so a read of `p.right` doesn't conflict with
    /// a borrow of `p.left`. Calls `record_read` for the root-only
    /// case when projections are empty (preserving Phase 5's
    /// MaybePartial check at the root level).
    fn record_place_read(
        &mut self,
        place: &Place,
        span: Span,
        state: &BTreeMap<Place, PlaceState>,
    ) {
        if place.projections.is_empty() {
            self.record_read(&place.root, span, state);
            return;
        }
        // Scan state for exclusive borrows that overlap this place.
        for (other, st) in state.iter() {
            if other.root != place.root {
                continue;
            }
            let PlaceState::BorrowedExclusive(borrower) = st else {
                continue;
            };
            let overlap = place.overlap(other);
            if matches!(overlap, PlaceOverlap::Disjoint) {
                continue;
            }
            // Self-conflict suppression: if the read is the borrower
            // itself (rare for projected places but possible), skip.
            if borrower == &place.root {
                continue;
            }
            let (code, msg) = if matches!(overlap, PlaceOverlap::Same) {
                (
                    "E0383",
                    format!(
                        "cannot read `{}` while it is exclusively borrowed by `{borrower}`",
                        place.canonical()
                    ),
                )
            } else {
                ("E0374", format!(
                    "cannot read `{}` while it overlaps the exclusive borrow `{}` held by `{borrower}`",
                    place.canonical(),
                    other.canonical()
                ))
            };
            let borrow_span = self
                .live_borrows
                .get(other)
                .and_then(|m| m.get(borrower))
                .copied();
            self.diags.push(RawDiag {
                code,
                message: msg,
                primary: span,
                suggestion: Some((
                    span,
                    place.root.clone(),
                    format!(
                        "while `{borrower}` is alive, no overlapping access to `{}` is admitted.",
                        place.canonical()
                    ),
                )),
                label: borrow_span.map(|s| (s, format!("`{borrower}` borrows here"))),
            });
            return;
        }
    }

    fn record_read(&mut self, name: &str, span: Span, state: &BTreeMap<Place, PlaceState>) {
        // MaybePartial check operates at the root level — Phase 5
        // branch-merging produces MaybePartial only on whole bindings.
        if let Some(PlaceState::MaybePartial) = state.get(&Place::root(name)) {
            if self.binding_is_non_copy(name) {
                self.diags.push(RawDiag {
                    code: "E0371",
                    message: format!("use of possibly-moved binding `{name}`"),
                    primary: span,
                    suggestion: Some((
                        span,
                        name.to_string(),
                        format!(
                            "`{name}` is moved on some branches but not others; \
                             ensure every branch either moves or preserves the binding, \
                             or clone it before the branch: `let {name}_owned = {name}.clone();`"
                        ),
                    )),
                    label: None,
                });
                return;
            }
        }
        // Slice 6BC.2 / 6BC.3 — E0383: any read of a place currently
        // held in exclusive borrow (at any projection level) is
        // rejected. Scan `state` for places rooted at `name`; the
        // read of `name` aliases every sub-place of `name`. Skip the
        // borrower itself (a binding may read its own borrow).
        let target = Place::root(name);
        for (place, st) in state.iter() {
            if place.root != name {
                continue;
            }
            let PlaceState::BorrowedExclusive(borrower) = st else {
                continue;
            };
            if borrower == name {
                continue;
            }
            let overlap = target.overlap(place);
            if matches!(overlap, PlaceOverlap::Disjoint) {
                continue;
            }
            // Same place vs. partial-overlap chooses code.
            let (code, msg) = if matches!(overlap, PlaceOverlap::Same) {
                (
                    "E0383",
                    format!(
                        "cannot read `{name}` while it is exclusively borrowed by `{borrower}`"
                    ),
                )
            } else {
                ("E0374", format!(
                    "cannot read `{name}` while one of its sub-places (`{}`) is exclusively borrowed by `{borrower}`",
                    place.canonical()
                ))
            };
            let borrow_span = self
                .live_borrows
                .get(place)
                .and_then(|m| m.get(borrower))
                .copied();
            self.diags.push(RawDiag {
                code,
                message: msg,
                primary: span,
                suggestion: Some((
                    span,
                    name.to_string(),
                    format!(
                        "while `{borrower}` is alive, no overlapping access to `{name}` is admitted. \
                         Either drop `{borrower}` before reading `{name}`, or restructure so \
                         the read happens before the exclusive borrow is established."
                    ),
                )),
                label: borrow_span.map(|s| (s, format!("`{borrower}` borrows `{name}` here"))),
            });
            return; // one diagnostic per access
        }
    }

    /// Slice 6BC.2: the move-arg variant of `record_read`. Used when an
    /// argument names a binding at a `take`-position. Fires E0371 for
    /// the MaybePartial-on-move case (Phase 5 behavior preserved), but
    /// suppresses E0383 — moving is more specific than reading, and the
    /// E0372 path emits the precise diagnostic for that case. Without
    /// this split, a move-arg of an exclusively-borrowed binding would
    /// fire both E0383 and E0372 for one conflict (cascading per
    /// design note §6.3, deferred polish).
    /// One consume site moving the whole binding `name`: fires the
    /// maybe-moved check (E0371), the move-while-borrowed check (E0372),
    /// and transitions the place to `Moved` with borrow cleanup. Shared by
    /// every move site — call move-args, bare `let`-init moves, and
    /// aggregate-literal captures — so a view borrow blocks the owner's
    /// move identically no matter which syntactic form consumes it.
    fn apply_move_of_binding(
        &mut self,
        name: &str,
        span: Span,
        state: &mut BTreeMap<Place, PlaceState>,
    ) {
        // 5BC.2b / 6BC.2 — moving a MaybePartial binding fires E0371
        // uniformly. E0383 is suppressed for the move case so cascading
        // errors don't produce both E0383 and E0372 for one conflict;
        // E0372 below is the precise diagnostic.
        self.record_move_arg_use(name, span, state);
        // 5BC.3b / 6BC.2 — E0372: moving a binding while it is borrowed
        // by a still-live binding. Message branches on flavor (shared vs
        // exclusive).
        self.check_move_against_borrow(name, span, state);
        if self.binding_is_non_copy(name) {
            state.insert(Place::root(name), PlaceState::Moved);
            // Moving x also invalidates any borrowers of x. Clean up
            // live_borrows entries for x; the borrowers themselves stay
            // in state (they still exist syntactically but reading them
            // post-move is undefined). E0372 already fired for this
            // case, so suppress cascading errors.
            self.live_borrows.remove(&Place::root(name));
            // Also: if the source binding `name` itself was a borrower
            // of something else, its move now releases that borrow.
            self.drop_borrower(name, state);
        }
    }

    /// One element of an aggregate literal (struct field, array element,
    /// tuple element, enum payload). A bare non-Copy binding is captured by
    /// value — a move, routed through `apply_move_of_binding` so live view
    /// borrows block it (E0372). Everything else walks normally.
    fn apply_aggregate_element(&mut self, e: &Expr, state: &mut BTreeMap<Place, PlaceState>) {
        match &e.kind {
            ExprKind::Ident(name) if self.binding_is_non_copy(name) => {
                let name = name.clone();
                self.apply_move_of_binding(&name, e.span, state);
            }
            _ => self.apply_expr(e, state),
        }
    }

    fn record_move_arg_use(&mut self, name: &str, span: Span, state: &BTreeMap<Place, PlaceState>) {
        let Some(st) = state.get(&Place::root(name)) else {
            return;
        };
        if matches!(st, PlaceState::MaybePartial) && self.binding_is_non_copy(name) {
            self.diags.push(RawDiag {
                code: "E0371",
                message: format!("use of possibly-moved binding `{name}`"),
                primary: span,
                suggestion: Some((
                    span,
                    name.to_string(),
                    format!(
                        "`{name}` is moved on some branches but not others; \
                         ensure every branch either moves or preserves the binding, \
                         or clone it before the branch: `let {name}_owned = {name}.clone();`"
                    ),
                )),
                label: None,
            });
        }
    }

    fn apply_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        state: &mut BTreeMap<Place, PlaceState>,
    ) {
        // Slice 6BC.opt / Phase-6 exit: method-call receiver claim
        // check. When `recv.method(args)` is a `ref this` / `take this`
        // method, the receiver itself is a `ref`-position claim against
        // its place — this is the iterator-invalidation pattern's
        // structural rejection. Without this check, calling
        // `vec.push(x)` while a shared borrow of `vec` is alive would
        // pass; the cross-statement form of E0381 fires here. Routes
        // through `record_method_receiver_claim` before the regular
        // call walking so the diagnostic lands at the call site.
        if let ExprKind::Field {
            receiver,
            name: method,
        } = &callee.kind
        {
            self.check_method_receiver_claim(receiver, &method.name, state);
        }
        self.apply_expr(callee, state);

        // Non-generic enum-variant constructor (`Wrap::V(payload)`) — the
        // payloads are by-value captures (moves), same as any aggregate
        // literal element. Route them through the aggregate walker so a
        // live view borrow blocks moving the owner into the payload.
        if let ExprKind::Path { segments } = &callee.kind {
            if segments.len() == 2 && self.sigs.enums.contains(&segments[0].name) {
                for a in args {
                    self.apply_aggregate_element(a, state);
                }
                return;
            }
        }

        let mut move_flags: Option<Vec<bool>> = match &callee.kind {
            ExprKind::Ident(name) => self.sigs.fn_param_moves(name).cloned(),
            _ => None,
        };
        // Slice 6BC.1: per-parameter `ref` flags for the callee, parallel
        // to `move_flags`. None for non-Ident callees and unknown fns —
        // matches the conservative gate Phase 5 already applies.
        let mut mut_flags: Option<Vec<bool>> = match &callee.kind {
            ExprKind::Ident(name) => self.sigs.fn_param_muts(name).cloned(),
            _ => None,
        };
        // Parallel to `move_flags`: the callee's WRITTEN parameter types, which
        // is how the move loop below tells an unconditional `take Text` from a
        // `take T` whose move depends on what the caller passed.
        let mut param_tys: Option<Vec<Type>> = match &callee.kind {
            ExprKind::Ident(name) => self.sigs.fn_param_tys(name).cloned(),
            _ => None,
        };
        // Handle-projection Tier 2: an indirect call through a fn-pointer
        // local. The callee ident isn't a named fn, but its recorded binding
        // type carries the per-param `take`/`ref` markers — source the same
        // flags from there so intra-call conflicts (E0380/E0381/E0382/E0374)
        // and move transitions fire identically for `f(ref x, x)` whether `f`
        // is a named fn or a fn-pointer.
        if move_flags.is_none() && mut_flags.is_none() {
            if let ExprKind::Ident(name) = &callee.kind {
                if let Some(Type {
                    kind:
                        TypeKind::FnPtr {
                            param_takes,
                            param_refs,
                            ..
                        },
                    ..
                }) = self.binding_type(name)
                {
                    move_flags = Some(param_takes.clone());
                    mut_flags = Some(param_refs.clone());
                }
            }
        }

        // Memory-model hardening (2026-07-06): method calls used to run
        // the intra-call walk with NO flags — a method's `ref`/`take`
        // args and its receiver escaped E0380/E0381/E0382/E0374 entirely
        // (`h.poke(h)` compiled while `poke2(h, h)` errored). Resolve the
        // method's FnEntry through the receiver's binding type (the same
        // lookup the cross-statement receiver check uses) and record the
        // receiver as a claim of its declared kind.
        let mut receiver_claim: Option<(ClaimKind, &Expr)> = None;
        let mut keeps_this_tie: Option<(String, Vec<bool>)> = None;
        if let ExprKind::Field {
            receiver,
            name: method,
        } = &callee.kind
        {
            if let ExprKind::Ident(recv_name) = &receiver.kind {
                // NOTE (2026-08-28): matching only `Path` here means a method
                // call on a GENERIC-typed receiver (`Vec[T]`, `Box[T]`,
                // `Option[T]`, `Mutex[T]`) carries no flags at all — no move
                // flags, no `ref` flags, no receiver claim — so E0372 and the
                // whole intra-call family (E0380/E0381/E0382/E0374) skip it
                // while firing on the concrete-receiver spelling of the same
                // call. Extending the match to `Generic` closes it in three
                // lines and every suite stays green, but it then rejects real
                // code through a SEPARATE precision bug: a view whose last use
                // is an `if` condition is still treated as live inside the
                // branch body, so `if !seen(p) { out.append(pg); }` reports
                // E0372 against a borrow that NLL should already have ended.
                // Fix that first. See
                // bugs/generic-receiver-method-calls-carry-no-borrow-flags.md.
                let entry = self
                    .binding_type(recv_name)
                    .and_then(|bt| match &bt.kind {
                        TypeKind::Path(type_name) => self
                            .sigs
                            .methods
                            .get(&format!("{type_name}.{}", method.name)),
                        _ => None,
                    })
                    .cloned();
                if let Some(entry) = entry {
                    if move_flags.is_none() {
                        move_flags = Some(entry.param_moves.clone());
                        param_tys = Some(entry.param_tys.clone());
                    }
                    if mut_flags.is_none() {
                        mut_flags = Some(entry.param_muts.clone());
                    }
                    receiver_claim = entry.receiver_claim.map(|k| (k, &**receiver));
                }
                // §5 tie — resolved from the binding's declared TYPE KIND,
                // covering Path AND Generic receivers (`Vec[str]`).
                if let Some(kind) = self.binding_type(recv_name).map(|t| t.kind.clone()) {
                    if let Some(keeps) = self.keeps_flags_for_receiver_ty(&kind, &method.name) {
                        keeps_this_tie = Some((recv_name.clone(), keeps));
                    }
                }
            } else if let Some(place) = place_from_expr(receiver) {
                // Field-path receiver (`holder.field.set(k)`): resolve the
                // receiver's type through the declared-field table so the
                // keeps tie still lands — the borrower is the ROOT binding
                // (over-approximate and sound: pinning the whole aggregate
                // pins the field). Claims/flags stay Ident-only; this
                // branch only feeds the §5 tie. The final field may be
                // Path- or Generic-typed (`panel.names: Vec[str]`).
                let mut ty: Option<TypeKind> = self.binding_type(&place.root).map(|t| t.kind.clone());
                for proj in &place.projections {
                    let (Some(TypeKind::Path(t)), Projection::Field(f)) = (ty.as_ref(), proj)
                    else {
                        ty = None;
                        break;
                    };
                    ty = self
                        .sigs
                        .struct_fields
                        .get(t)
                        .and_then(|fs| fs.get(f).map(|ft| ft.kind.clone()));
                }
                if let Some(kind) = ty {
                    if let Some(keeps) = self.keeps_flags_for_receiver_ty(&kind, &method.name) {
                        keeps_this_tie = Some((place.root.clone(), keeps));
                    }
                }
            }
        }

        // Slice 6BC.3 — intra-call conflict detection, place-aware.
        // Builds a per-arg `Claim` (place + kind) and walks pairs. The
        // overlap matrix decides which code fires:
        //
        //   Pair / overlap | Same → existing code     | Contains/Contained → E0374
        //   ---------------+--------------------------+--------------------------
        //   Mut + Mut      | E0380 (mut-mut)           | E0374 (parent + sub-place)
        //   Mut + Move     | E0382 (mut-move)          | E0374
        //   Mut + Shared   | E0381 (mut-shared)        | E0374
        //   Move + Shared  | E0370 (move-shared)       | E0374
        //
        // Disjoint sub-places (`buf.left` vs `buf.right`) admit; that's
        // the design-note §5.2 win — partial-place tracking via
        // `Place::projections`. Copy bindings produce no claim (per
        // §2.9 `ref`-on-Copy is local-mutability, not a borrow).
        self.check_intra_call_conflicts(args, &move_flags, &mut_flags, receiver_claim);

        // v0.0.28, the cross-STATEMENT half of the same rule: a `ref`
        // argument is an exclusive claim, and a place that something else is
        // already borrowing cannot be claimed exclusively.
        //
        // `check_intra_call_conflicts` compares the arguments of ONE call to
        // each other; `check_method_receiver_claim` compares a mutating
        // receiver to live borrows. The gap between them was a `ref` ARGUMENT
        // handed out twice — `s.spawn(d, w1); s.spawn(d, w2);` gives two
        // workers exclusive access to the same place, which is the race a
        // scope exists to prevent.
        if let Some(muts) = mut_flags.as_ref() {
            for (i, arg) in args.iter().enumerate() {
                if !muts.get(i).copied().unwrap_or(false) {
                    continue;
                }
                self.check_ref_arg_against_borrows(arg, state);
            }
        }

        // State transitions. Each move-arg of a *non-Copy* binding
        // transitions Owned → Moved. Copy-typed bindings (or bindings of
        // unknown type) stay Owned — for unknown we conservatively assume
        // Copy so we don't over-track. 5BC.4 / sema integration will
        // tighten this once binding types are fully resolved.
        for (i, arg) in args.iter().enumerate() {
            let arg_is_move = move_flags
                .as_ref()
                .and_then(|v| v.get(i).copied())
                .unwrap_or(false);
            if arg_is_move {
                if let ExprKind::Ident(name) = &arg.kind {
                    // A `take` parameter written as one of the callee's own
                    // generic params consumes only what is actually non-Copy,
                    // and the CALLER is where that is known — `gsink::[Text](t)`
                    // moves, `gsink::[i32](n)` copies. A parameter with a
                    // definitely-non-Copy written type moves unconditionally,
                    // which is the pre-existing path and stays exact: asking
                    // the argument there would silently drop the check for a
                    // binding whose type borrowck could not infer.
                    let conditional = param_tys
                        .as_ref()
                        .and_then(|v| v.get(i))
                        .map(|t| param_move_is_conditional(t, &self.oracle))
                        .unwrap_or(false);
                    if !conditional || self.binding_is_non_copy(name) {
                        self.apply_move_of_binding(name, arg.span, state);
                        continue;
                    }
                }
            }
            self.apply_expr(arg, state);
        }

        // Memory-model contract §5: a `#[keeps(this)]` method stores its
        // view arguments in the receiver. The receiver becomes a live
        // borrower of each view argument's owner, so the owner can neither
        // move (E0372) nor die at scope exit (E0514) while the receiver
        // lives. A view-binding argument contributes the owners it already
        // borrows; an owner passed by coercion (`push(t)`) contributes its
        // own place. Literal arguments have no root and tie nothing
        // ('static bytes).
        if let Some((recv_name, view_flags)) = keeps_this_tie {
            let mut sources: Vec<(Place, BorrowFlavor)> = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                if !view_flags.get(i).copied().unwrap_or(false) {
                    continue;
                }
                // WHAT KIND OF LOAN a kept position establishes is the
                // parameter's mode, not the argument's type (2026-08-22).
                //
                // A `ref` parameter is EXCLUSIVE access, and it is passed BY
                // POINTER for every type — that is what makes a callee's
                // writes visible to the caller at all. Both halves were
                // missing here, and each cost a guarantee:
                //
                //   * the flavour was always `Shared`, so a READ of a place
                //     lent exclusively to a worker was admitted (E0383 never
                //     saw an exclusive state to fire on);
                //   * the Copy gate below dropped the tie ENTIRELY for a Copy
                //     argument, so `struct Cell { n: i64 }` recorded no loan
                //     at all and `thread::Scope` lost all three of the
                //     guarantees `thread.cplus` documents — while the same
                //     program with a `Text` in the struct was checked
                //     correctly. The Copy ones are the values a scope is FOR.
                //
                // §2.9's "`ref` on a Copy type is local mutability, not a
                // borrow" holds for a call that RETURNS; it stops holding the
                // moment the receiver KEEPS the pointer, which is exactly
                // what `#[keeps(this)]` declares.
                //
                // A non-`ref` kept position still ties Shared: `names.push(t)`
                // stores a VIEW of `t`, and reading `t` afterwards is sound.
                let ref_param = mut_flags
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or(false);
                let flavor = if ref_param {
                    BorrowFlavor::Exclusive
                } else {
                    BorrowFlavor::Shared
                };
                // A view-producing call argument (`h.set(t.view())`)
                // classifies to its owner places directly. A bare place
                // argument is either the owner itself (coercion:
                // `h.set(t)`) or a view binding whose recorded owners we
                // inherit (`h.set(s)`).
                let mut arg_sources = self.classify_borrow_source(arg);
                if arg_sources.is_empty() {
                    if let Some(place) = place_from_expr(arg) {
                        if let Some(owners) = self.binding_borrows_from.get(&place.root) {
                            for o in owners.clone() {
                                arg_sources.push((o, flavor));
                            }
                        }
                        if ref_param || self.binding_is_non_copy(&place.root) {
                            arg_sources.push((place, flavor));
                        }
                    }
                }
                sources.extend(arg_sources.into_iter().filter(|(p, _)| p.root != recv_name));
            }
            if !sources.is_empty() {
                self.extend_borrows(sources, &recv_name, callee.span, state);
            }
        }

        // Contract §5, free-fn half: computed (src → ref-param dst) flows.
        // The dst argument's root becomes a borrower of the src argument's
        // owners — `store_in(ref h, t.view())` ties h ← t.
        if let ExprKind::Ident(fn_name) = &callee.kind {
            let flows = self
                .sigs
                .fns
                .get(fn_name)
                .map(|e| e.computed_ref_flows.clone())
                .unwrap_or_default();
            for (src, dst) in flows {
                let Some(dst_place) = args.get(dst).and_then(place_from_expr) else {
                    continue;
                };
                let Some(src_arg) = args.get(src) else {
                    continue;
                };
                let mut srcs = self.classify_borrow_source(src_arg);
                if srcs.is_empty() {
                    if let Some(place) = place_from_expr(src_arg) {
                        if let Some(owners) = self.binding_borrows_from.get(&place.root) {
                            for o in owners.clone() {
                                srcs.push((o, BorrowFlavor::Shared));
                            }
                        }
                        if self.binding_is_non_copy(&place.root) {
                            srcs.push((place, BorrowFlavor::Shared));
                        }
                    }
                }
                let srcs: Vec<_> = srcs
                    .into_iter()
                    .filter(|(p, _)| p.root != dst_place.root)
                    .collect();
                if !srcs.is_empty() {
                    let borrower = dst_place.root.clone();
                    self.extend_borrows(srcs, &borrower, callee.span, state);
                }
            }
        }
    }

    /// Slice 6BC.3 — intra-call conflict detection. Builds an
    /// `ArgClaim` per argument and walks pairs of claims, emitting the
    /// appropriate diagnostic for each conflict. Replaces the per-error
    /// scanning loops from 6BC.1, with two correctness gains:
    ///   - Partial-place overlap is admitted/rejected on `Place`
    ///     comparison: `ref buf.left` + `ref buf.right` no longer
    ///     conflict; `ref buf` + `ref buf.left` now reject as E0374.
    ///   - Each pair fires at most one diagnostic. Same-place codes
    ///     win over E0374 when the projections match exactly; partial
    ///     overlap routes through E0374 regardless of which kinds are
    ///     in conflict.
    fn check_intra_call_conflicts(
        &mut self,
        args: &[Expr],
        move_flags: &Option<Vec<bool>>,
        mut_flags: &Option<Vec<bool>>,
        receiver: Option<(ClaimKind, &Expr)>,
    ) {
        // Build per-arg claims. Non-place exprs produce no claim, and so
        // does a MOVE claim on a Copy binding — a Copy argument is copied,
        // not moved, so it constrains nothing.
        //
        // An EXCLUSIVE claim is different and used to be gated the same way
        // (2026-08-22). `ref` passes a POINTER to the caller's storage for
        // every type — that is what makes a callee's writes visible to the
        // caller at all — so `f(ref a, ref a)` hands the callee two aliasing
        // exclusive pointers whether or not `a` is Copy, and E0380/E0381/
        // E0374 all went silent on the Copy half of the family. The
        // sibling of this gate, on the `#[keeps(this)]` tie, is what cost
        // `thread::Scope` its guarantees.
        let claims: Vec<Option<ArgClaim>> = args
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                let is_move = move_flags
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or(false);
                let is_mut = mut_flags
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or(false);
                // For the sibling-read case (Shared claims), we use the
                // arg's expression tree rather than its place, so even
                // non-place exprs like `peek(buf)` count as "reads of buf".
                // Direct (Mut/Move) claims need a real place expression.
                let kind = if is_move {
                    ClaimKind::Move
                } else if is_mut {
                    ClaimKind::Exclusive
                } else {
                    ClaimKind::Shared
                };
                match kind {
                    ClaimKind::Move | ClaimKind::Exclusive => {
                        let place = place_from_expr(arg)?;
                        // A Copy place cannot be moved out of, so a `take`
                        // argument of one constrains nothing. A `ref` one
                        // aliases the caller's storage either way.
                        if matches!(kind, ClaimKind::Move)
                            && !self.binding_is_non_copy(&place.root)
                        {
                            return None;
                        }
                        Some(ArgClaim {
                            kind,
                            place,
                            span: arg.span,
                        })
                    }
                    ClaimKind::Shared => {
                        // Shared claims need a place if we want to fire
                        // structural codes against them. The shared-read
                        // path below uses `expr_reads_overlapping_place`
                        // which doesn't require a claim — so we leave
                        // Shared at None here and let the per-pair check
                        // probe the arg expression tree directly.
                        let _ = arg;
                        None
                    }
                }
            })
            .collect();

        // Receiver claim (2026-07-06): receivers are by-pointer for the
        // whole call, so a receiver of a non-Copy place participates in
        // the same conflicts as a direct arg claim. Copy receivers carry
        // no aliasing constraint (codegen no longer marks Copy exclusive
        // pointers `noalias`).
        let recv_claim: Option<ArgClaim> = receiver.and_then(|(kind, rexpr)| {
            let place = place_from_expr(rexpr)?;
            if !self.binding_is_non_copy(&place.root) {
                return None;
            }
            Some(ArgClaim {
                kind,
                place,
                span: rexpr.span,
            })
        });
        if let Some(rc) = &recv_claim {
            for (j, arg) in args.iter().enumerate() {
                if let Some(other) = &claims[j] {
                    let overlap = rc.place.overlap(&other.place);
                    if matches!(overlap, PlaceOverlap::Disjoint) {
                        continue;
                    }
                    match (rc.kind, other.kind) {
                        (ClaimKind::Shared, ClaimKind::Shared) => {}
                        (ClaimKind::Shared, ClaimKind::Exclusive)
                        | (ClaimKind::Shared, ClaimKind::Move) => {
                            let name = &rc.place.root;
                            let verb = if matches!(other.kind, ClaimKind::Move) {
                                "move"
                            } else {
                                "exclusively borrow"
                            };
                            let code = if matches!(other.kind, ClaimKind::Move) {
                                "E0370"
                            } else {
                                "E0381"
                            };
                            self.diags.push(RawDiag {
                                code,
                                message: format!(
                                    "cannot {verb} `{name}` in an argument while it is the call's receiver"
                                ),
                                primary: other.span,
                                suggestion: Some((
                                    rc.span.merge(other.span),
                                    String::new(),
                                    format!(
                                        "the receiver reads `{name}` for the duration of the call;                                          split into two statements."
                                    ),
                                )),
                                label: Some((rc.span, format!("`{name}` is the receiver here"))),
                            });
                        }
                        _ => {
                            if let Some(diag) = build_direct_claim_diag(rc, other, 0, 1, overlap) {
                                self.diags.push(diag);
                            }
                        }
                    }
                } else if matches!(rc.kind, ClaimKind::Exclusive | ClaimKind::Move) {
                    // Sibling with no direct claim: fire ONLY when the arg
                    // is the receiver's whole place passed bare (a by-ptr
                    // borrow live for the call, `h.poke(h)`). An arg that
                    // merely READS the place while evaluating
                    // (`this.grow(this._cap * 2)`, `this.helper(this.len())`)
                    // finishes before the call begins and its value is a
                    // copy — temporally safe, and ubiquitous in method
                    // bodies, so the deep expression scan is deliberately
                    // NOT applied to receivers.
                    if let Some(ap) = place_from_expr(arg) {
                        if matches!(rc.place.overlap(&ap), PlaceOverlap::Same)
                            && self.binding_is_non_copy(&ap.root)
                        {
                            let name = &rc.place.root;
                            self.diags.push(RawDiag {
                                code: "E0381",
                                message: format!(
                                    "cannot pass `{name}` by borrow while it is the call's receiver"
                                ),
                                primary: arg.span,
                                suggestion: Some((
                                    rc.span.merge(arg.span),
                                    String::new(),
                                    format!(
                                        "the receiver holds `{name}` for the duration of the call;                                          split into two statements."
                                    ),
                                )),
                                label: Some((rc.span, format!("`{name}` is the receiver here"))),
                            });
                        }
                    }
                }
            }
        }

        // Pairwise walk. For each "primary" claim (Mut or Move), scan
        // every sibling for a conflict.
        for i in 0..args.len() {
            let Some(primary) = &claims[i] else { continue };
            for j in 0..args.len() {
                if i == j {
                    continue;
                }
                // Direct claim on the sibling?
                if let Some(other) = &claims[j] {
                    let overlap = primary.place.overlap(&other.place);
                    if matches!(overlap, PlaceOverlap::Disjoint) {
                        continue;
                    }
                    // Direct-claim conflict. Determine the code.
                    if let Some(diag) = build_direct_claim_diag(primary, other, i, j, overlap) {
                        self.diags.push(diag);
                    }
                } else {
                    // Sibling carries no direct claim but might contain
                    // a shared read of an overlapping place inside its
                    // expression tree (e.g. `peek(buf)` reads `buf`).
                    // Only meaningful when the primary is itself a
                    // claim (Mut/Move) — Shared+Shared is admissible.
                    if let Some(diag) = self.find_overlapping_shared_read(primary, &args[j]) {
                        self.diags.push(diag);
                    }
                }
            }
        }
    }

    /// Scan an arg expression tree for a read of any place that
    /// overlaps `primary`'s place. Returns a diagnostic if one is
    /// found. Used to detect E0370 (move + shared read) and E0381
    /// (mut + shared read) — the latter possibly via partial-place
    /// overlap, in which case E0374 fires instead.
    fn find_overlapping_shared_read(&self, primary: &ArgClaim, other: &Expr) -> Option<RawDiag> {
        // A method reference is not a read of its receiver.
        if self.is_method_ref(other) {
            return None;
        }
        // Walk other's expression tree, collecting all place
        // expressions that overlap primary.place.
        let mut found = None;
        scan_overlapping_places(other, &primary.place, &mut found);
        let (overlap, other_place_span) = found?;
        let name = &primary.place.root;
        let primary_span = primary.span;
        let suggestion_span = primary_span.merge(other_place_span);
        // Partial-place conflicts always route to E0374.
        if matches!(overlap, PlaceOverlap::Contains | PlaceOverlap::Contained) {
            return Some(RawDiag {
                code: "E0374",
                message: format!("partial-place conflict on `{name}` in the same call"),
                primary: primary_span,
                suggestion: Some((
                    suggestion_span,
                    String::new(),
                    format!(
                        "the borrow of `{name}` (or one of its sub-places) overlaps a sibling \
                         argument that reads an overlapping place; a borrow of a place includes \
                         all of its sub-places. Split into two statements."
                    ),
                )),
                label: Some((other_place_span, format!("sibling read of `{name}` here"))),
            });
        }
        // Same-place: E0370 for move, E0381 for exclusive.
        match primary.kind {
            ClaimKind::Move => Some(RawDiag {
                code: "E0370",
                message: format!("cannot move `{name}` and shared-borrow it in the same call"),
                primary: primary_span,
                suggestion: Some((
                    suggestion_span,
                    String::new(),
                    format!(
                        "split into two statements so `{name}` is read before being moved: \
                         `let tmp = ...; consume(move {name}, tmp);`"
                    ),
                )),
                label: Some((other_place_span, format!("shared read of `{name}` here"))),
            }),
            ClaimKind::Exclusive => Some(RawDiag {
                code: "E0381",
                message: format!(
                    "cannot exclusively borrow `{name}` and shared-borrow it in the same call"
                ),
                primary: primary_span,
                suggestion: Some((
                    suggestion_span,
                    String::new(),
                    format!(
                        "the exclusive borrow `mut {name}` claims access for the duration of \
                         the call; the sibling argument reads `{name}` concurrently. Split into \
                         two statements: `let tmp = ...; f(mut {name}, tmp);`"
                    ),
                )),
                label: Some((other_place_span, format!("shared read of `{name}` here"))),
            }),
            ClaimKind::Shared => None, // shared+shared is admissible
        }
    }

    /// Slice 6BC.opt / Phase-6 exit: for a method call `recv.m(args)`,
    /// the receiver claims access. Reject if `recv`'s place is already
    /// borrowed by a live borrower — this is the cross-statement form
    /// of E0381 / E0383 for method-call receivers. Without this,
    /// iterator-invalidation (`let cur = vec.iter(); vec.push(...);`)
    /// would pass.
    ///
    /// Conservative: skips when receiver isn't a plain `Ident` or the
    /// method isn't resolvable. Treats all method calls on borrowed
    /// receivers as potentially conflicting — for shared-receiver
    /// methods this is over-strict but sound; tightening to "only
    /// `ref this` methods" requires plumbing receiver kind into the
    /// SigTable, deferred to a polish slice.
    /// v0.0.28: E0381 for a `ref` argument naming an already-borrowed place.
    ///
    /// Deliberately fires even when the live borrower is this call's own
    /// receiver: lending the same data to a scope twice is precisely the
    /// aliasing a scoped thread pool must refuse, and "the same borrower" is
    /// no defence when the borrower hands each lend to a different thread.
    fn check_ref_arg_against_borrows(&mut self, arg: &Expr, state: &BTreeMap<Place, PlaceState>) {
        let Some(place) = place_from_expr(arg) else {
            return;
        };
        let root = Place::root(&place.root);
        // Either flavour of loan conflicts with a fresh exclusive claim. The
        // exclusive arm is not decoration: a `ref` position of a
        // `#[keeps(this)]` call now records `BorrowedExclusive`, which is the
        // state a second `s.lend(c, w2)` has to see.
        if !matches!(
            state.get(&root),
            Some(PlaceState::BorrowedShared(_)) | Some(PlaceState::BorrowedExclusive(_))
        ) {
            return;
        }
        // NO COPY GATE. §2.9's "`ref` on a Copy type is local mutability, not
        // a borrow" describes a call that returns; a live loan on the place
        // means someone is still holding the pointer, and `ref` is
        // by-pointer for every type. Gating this on non-Copy is what let two
        // workers take `struct Cell { n: i64 }` exclusively at once.
        let Some((borrower, borrow_span)) = self
            .live_borrows
            .get(&root)
            .and_then(|s| s.iter().next().map(|(n, sp)| (n.clone(), *sp)))
        else {
            return;
        };
        let name = &place.root;
        self.diags.push(RawDiag {
            code: "E0381",
            message: format!(
                "cannot borrow `{name}` exclusively while it is borrowed by `{borrower}`"
            ),
            primary: arg.span,
            suggestion: Some((
                arg.span,
                name.clone(),
                format!(
                    "a `ref` argument is exclusive access, and `{borrower}` is still reading \
                     `{name}`. Give each exclusive user its own value, or finish with \
                     `{borrower}` first."
                ),
            )),
            label: Some((borrow_span, format!("`{borrower}` borrows `{name}` here"))),
        });
    }

    /// v0.0.28: E0381 for an assignment into a borrowed place.
    ///
    /// `heal` reassignment of a MOVED binding is not a conflict (nothing
    /// borrows a moved-out place), and neither is writing through a raw
    /// pointer — the raw seam is the developer's, by design. Everything else
    /// that names a live-borrowed root and writes to it is refused, with the
    /// borrower named.
    fn check_write_against_borrows(&mut self, target: &Expr, state: &BTreeMap<Place, PlaceState>) {
        let Some(place) = place_from_expr(target) else {
            return;
        };
        // `*p = v` / `p[i] = v` through a raw pointer: not a tracked place.
        if matches!(target.kind, ExprKind::Unary { op: UnaryOp::Deref, .. }) {
            return;
        }
        let root = Place::root(&place.root);
        // A write conflicts with a loan of either flavour. (An exclusive loan
        // reaches the dedicated E0383 path only for READS; the write path is
        // here, so both states have to be named.)
        if !matches!(
            state.get(&root),
            Some(PlaceState::BorrowedShared(_)) | Some(PlaceState::BorrowedExclusive(_))
        ) {
            return;
        }
        let Some((borrower, borrow_span)) = self
            .live_borrows
            .get(&root)
            .and_then(|s| s.iter().next().map(|(n, sp)| (n.clone(), *sp)))
        else {
            return;
        };
        let name = &place.root;
        self.diags.push(RawDiag {
            code: "E0381",
            message: format!("cannot write to `{name}` while it is borrowed by `{borrower}`"),
            primary: target.span,
            suggestion: Some((
                target.span,
                name.clone(),
                format!(
                    "`{borrower}` reads memory owned by `{name}`, so a write here could be \
                     observed half-done. Finish with `{borrower}` (let it go out of scope) \
                     before writing, or write before the borrow is established."
                ),
            )),
            label: Some((borrow_span, format!("`{borrower}` borrows `{name}` here"))),
        });
    }

    fn check_method_receiver_claim(
        &mut self,
        receiver: &Expr,
        method_name: &str,
        state: &BTreeMap<Place, PlaceState>,
    ) {
        let ExprKind::Ident(recv_name) = &receiver.kind else {
            return;
        };
        let Some(bt) = self.binding_type(recv_name) else {
            return;
        };
        // Path or generic instantiation (`Vec[i32]`) — a method call on a
        // shared-borrowed generic receiver (`v.append(..)` while a slice view
        // of `v` is live) is the same iterator-invalidation conflict as on a
        // non-generic one. Before this it returned early on `Generic` and the
        // mutation slipped past (a UAF: append can realloc the buffer the live
        // slice points into).
        let Some(type_name) = Self::type_name_of(&bt.kind) else {
            return;
        };
        let Some(entry) = self.sigs.method_entry(&type_name, method_name) else {
            return;
        };
        // A `this` (read-only) method does NOT conflict with a *shared* borrow —
        // both are shared reads. Only a `ref this` / `take this` method needs
        // exclusive/consuming access, which is what actually invalidates a live
        // slice view (`v.append(..)` reallocates the buffer). Gating on the
        // receiver's claim keeps the real iterator-invalidation catch while not
        // rejecting a benign read (`v.count()`) taken alongside a view.
        let receiver_mutates = matches!(
            entry.receiver_claim,
            Some(ClaimKind::Exclusive) | Some(ClaimKind::Move)
        );
        let place = Place::root(recv_name);
        let Some(st) = state.get(&place) else { return };
        match st {
            PlaceState::BorrowedShared(_) if receiver_mutates => {
                let (borrower, borrow_span) = self
                    .live_borrows
                    .get(&place)
                    .and_then(|s| s.iter().next().map(|(n, sp)| (n.clone(), *sp)))
                    .map(|(n, s)| (n, Some(s)))
                    .unwrap_or_else(|| ("(unknown)".to_string(), None));
                self.diags.push(RawDiag {
                    code: "E0381",
                    message: format!(
                        "cannot call `{recv_name}.{method_name}(...)` while `{recv_name}` is shared-borrowed by `{borrower}`"
                    ),
                    primary: receiver.span,
                    suggestion: Some((
                        receiver.span,
                        recv_name.clone(),
                        format!(
                            "method calls on `{recv_name}` may require exclusive access; \
                             while `{borrower}` is alive, no overlapping access is admitted. \
                             Drop `{borrower}` before calling the method, or restructure \
                             so the call happens before the borrow is established."
                        ),
                    )),
                    label: borrow_span.map(|s| (s, format!("`{borrower}` borrows `{recv_name}` here"))),
                });
            }
            PlaceState::BorrowedExclusive(borrower) if borrower != recv_name => {
                let borrow_span = self
                    .live_borrows
                    .get(&place)
                    .and_then(|m| m.get(borrower))
                    .copied();
                self.diags.push(RawDiag {
                    code: "E0383",
                    message: format!(
                        "cannot call `{recv_name}.{method_name}(...)` while `{recv_name}` is exclusively borrowed by `{borrower}`"
                    ),
                    primary: receiver.span,
                    suggestion: Some((
                        receiver.span,
                        recv_name.clone(),
                        format!(
                            "while `{borrower}` is alive, no overlapping access to `{recv_name}` is admitted."
                        ),
                    )),
                    label: borrow_span.map(|s| (s, format!("`{borrower}` exclusively borrows `{recv_name}` here"))),
                });
            }
            _ => {}
        }
    }

    /// 5BC.3b / 6BC.2 / 6BC.3: emit E0372 if moving `name` would
    /// invalidate any live borrow at an overlapping place. Scans
    /// `live_borrows` for entries rooted at `name`. The diagnostic
    /// message branches on the flavor: shared (Phase 5) vs
    /// exclusive (6BC.2). Partial-place borrows (e.g. moving `buf`
    /// while `buf.left` is borrowed) route through the same code with
    /// a refined message naming the sub-place.
    fn check_move_against_borrow(
        &mut self,
        name: &str,
        span: Span,
        state: &BTreeMap<Place, PlaceState>,
    ) {
        let target = Place::root(name);
        // Scan live_borrows for entries rooted at `name`. Pick the
        // first overlapping entry deterministically (BTreeMap iterates
        // in sorted order). The same-place case is the most common
        // pattern; partial-overlap is the 6BC.3 extension.
        let mut hit: Option<(Place, String, Span, PlaceOverlap)> = None;
        for (place, borrowers) in self.live_borrows.iter() {
            if place.root != name {
                continue;
            }
            if borrowers.is_empty() {
                continue;
            }
            let overlap = target.overlap(place);
            if matches!(overlap, PlaceOverlap::Disjoint) {
                continue;
            }
            let (borrower, borrower_span) = borrowers
                .iter()
                .next()
                .map(|(n, s)| (n.clone(), *s))
                .unwrap();
            hit = Some((place.clone(), borrower, borrower_span, overlap));
            break;
        }
        let Some((place, borrower, borrower_span, overlap)) = hit else {
            return;
        };
        let is_exclusive = matches!(state.get(&place), Some(PlaceState::BorrowedExclusive(_)));
        let flavor_label = if is_exclusive {
            "exclusively"
        } else {
            "shared"
        };
        let (msg, hint) = if matches!(overlap, PlaceOverlap::Same) {
            (
                format!("cannot move `{name}` while it is {flavor_label} borrowed by `{borrower}`"),
                if is_exclusive {
                    format!(
                        "the exclusive borrow `{borrower}` is the only borrower allowed \
                         while it is alive; moving `{name}` would invalidate it. \
                         Drop `{borrower}` before moving `{name}`."
                    )
                } else {
                    format!(
                        "the value returned to `{borrower}` borrows from `{name}`; \
                         while `{borrower}` is alive, `{name}` cannot be moved. \
                         Either drop `{borrower}` before moving `{name}`, or \
                         clone `{borrower}` if you need both bindings to outlive \
                         the move."
                    )
                },
            )
        } else {
            // Partial-place: name overlaps but isn't identical to the
            // borrowed place. The aliasing-XOR-mutability rule still
            // rejects the move; the message names the sub-place.
            (
                format!(
                    "cannot move `{name}` while sub-place `{}` is {flavor_label} borrowed by `{borrower}`",
                    place.canonical()
                ),
                format!(
                    "moving `{name}` invalidates all of its sub-places, including `{}`. \
                     Drop `{borrower}` before moving `{name}`.",
                    place.canonical()
                ),
            )
        };
        self.diags.push(RawDiag {
            code: "E0372",
            message: msg,
            primary: span,
            suggestion: Some((span, String::new(), hint)),
            label: Some((borrower_span, format!("`{borrower}` borrows `{name}` here"))),
        });
    }
}

/// Merge per-arm post-states into one post-join state. Bindings present
/// in `pre` (and only those — branch-locals are scope-restricted earlier
/// via `walk_block_in_scope`) get a state computed by pairwise
/// `PlaceState::merge` across every non-diverging arm. Diverging arms
/// are excluded — their post-state is unreachable from the join point.
/// If every arm diverges, the join itself is unreachable; we return `pre`
/// as a sane default (caller code below the join is dead).
fn merge_branches(
    pre: &BTreeMap<Place, PlaceState>,
    arms: &[&BTreeMap<Place, PlaceState>],
    diverges: &[bool],
) -> BTreeMap<Place, PlaceState> {
    // Filter to arms that flow through to the join.
    let live: Vec<&BTreeMap<Place, PlaceState>> = arms
        .iter()
        .zip(diverges.iter())
        .filter_map(|(s, d)| if *d { None } else { Some(*s) })
        .collect();
    if live.is_empty() {
        return pre.clone();
    }
    let mut out = BTreeMap::new();
    for k in pre.keys() {
        let mut acc: Option<PlaceState> = None;
        for arm in &live {
            let arm_state = arm.get(k).cloned().unwrap_or(PlaceState::Owned);
            acc = Some(match acc {
                None => arm_state,
                Some(prev) => prev.merge(&arm_state),
            });
        }
        out.insert(k.clone(), acc.expect("live arms non-empty"));
    }
    out
}

/// True iff `e` (or any of its sub-expressions) reads the binding `name`
/// via a plain `Ident` reference. Originally used by E0370 detection
/// (now replaced by `scan_overlapping_places` in 6BC.3), retained for
/// possible future use cases that need binding-name-only lookups.
#[allow(dead_code)]
fn expr_reads_ident(e: &Expr, name: &str) -> bool {
    match &e.kind {
        // v0.0.22 DSL.2: never reached — builder blocks desugar to
        // ordinary AST before the borrow checker runs.
        ExprKind::BuilderBlock { .. } => false,
        ExprKind::FnRef { .. } => false,
        ExprKind::Ident(n) => n == name,
        ExprKind::IntLit(_, _)
        | ExprKind::FloatLit(_, _)
        | ExprKind::BoolLit(_)
        | ExprKind::StrLit(_)
        | ExprKind::CStrLit(_)
        | ExprKind::IncludeBytes { .. }
        | ExprKind::IncludeStr { .. }
        | ExprKind::EnvVar { .. } => false,
        ExprKind::Intrinsic { args, .. } => args.iter().any(|a| expr_reads_ident(a, name)),
        ExprKind::Asm { operands, .. } => {
            operands.iter().any(|op| expr_reads_ident(&op.value, name))
        }
        ExprKind::InterpStr { parts } => parts.iter().any(|p| match p {
            crate::ast::InterpStrPart::Expr(e) => expr_reads_ident(e, name),
            _ => false,
        }),
        ExprKind::Path { .. } => false,
        ExprKind::Block(b) => {
            b.stmts.iter().any(|s| stmt_reads_ident(s, name))
                || b.tail.as_deref().is_some_and(|t| expr_reads_ident(t, name))
        }
        ExprKind::Await(inner) => expr_reads_ident(inner, name),
        ExprKind::Yield(inner) => expr_reads_ident(inner, name),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            expr_reads_ident(cond, name)
                || then.stmts.iter().any(|s| stmt_reads_ident(s, name))
                || then
                    .tail
                    .as_deref()
                    .is_some_and(|t| expr_reads_ident(t, name))
                || else_branch
                    .as_deref()
                    .is_some_and(|e| expr_reads_ident(e, name))
        }
        ExprKind::Call { callee, args, .. } => {
            expr_reads_ident(callee, name) || args.iter().any(|a| expr_reads_ident(a, name))
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            expr_reads_ident(lhs, name) || expr_reads_ident(rhs, name)
        }
        ExprKind::Unary { operand, .. } => expr_reads_ident(operand, name),
        ExprKind::Range { start, end, .. } => {
            start.as_deref().is_some_and(|s| expr_reads_ident(s, name))
                || end.as_deref().is_some_and(|e| expr_reads_ident(e, name))
        }
        ExprKind::Assign { target, value, .. } => {
            expr_reads_ident(target, name) || expr_reads_ident(value, name)
        }
        ExprKind::Cast { expr, .. } | ExprKind::CastChecked { expr, .. } => {
            expr_reads_ident(expr, name)
        }
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            fields.iter().any(|f| expr_reads_ident(&f.value, name))
        }
        ExprKind::Field { receiver, .. } => expr_reads_ident(receiver, name),
        ExprKind::ArrayFill { fill, .. } => expr_reads_ident(fill, name),
        ExprKind::ArrayLit { elements }
        | ExprKind::GenericEnumCall { args: elements, .. }
        | ExprKind::TupleLit { elements } => elements.iter().any(|e| expr_reads_ident(e, name)),
        ExprKind::Index { receiver, index } => {
            expr_reads_ident(receiver, name) || expr_reads_ident(index, name)
        }
        ExprKind::Match { scrutinee, arms } => {
            expr_reads_ident(scrutinee, name)
                || arms.iter().any(|a| expr_reads_ident(&a.body, name))
        }
    }
}

#[allow(dead_code)]
fn stmt_reads_ident(s: &Stmt, name: &str) -> bool {
    match &s.kind {
        StmtKind::Let { init, .. } => init.as_ref().is_some_and(|e| expr_reads_ident(e, name)),
        StmtKind::LetDestructure { init, .. } => expr_reads_ident(init, name),
        StmtKind::Return(Some(e)) | StmtKind::Expr(e) | StmtKind::Defer(e) => {
            expr_reads_ident(e, name)
        }
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => false,
        StmtKind::Assert(e) => expr_reads_ident(e, name),
        StmtKind::While { cond, body, .. } => {
            expr_reads_ident(cond, name)
                || body.stmts.iter().any(|s| stmt_reads_ident(s, name))
                || body
                    .tail
                    .as_deref()
                    .is_some_and(|t| expr_reads_ident(t, name))
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                init.as_deref().is_some_and(|i| stmt_reads_ident(i, name))
                    || cond.as_ref().is_some_and(|c| expr_reads_ident(c, name))
                    || update.iter().any(|u| expr_reads_ident(u, name))
                    || body.stmts.iter().any(|s| stmt_reads_ident(s, name))
                    || body
                        .tail
                        .as_deref()
                        .is_some_and(|t| expr_reads_ident(t, name))
            }
            ForLoop::Range { iter, body, .. } => {
                expr_reads_ident(iter, name)
                    || body.stmts.iter().any(|s| stmt_reads_ident(s, name))
                    || body
                        .tail
                        .as_deref()
                        .is_some_and(|t| expr_reads_ident(t, name))
            }
        },
        StmtKind::Loop(b, _) => {
            b.stmts.iter().any(|s| stmt_reads_ident(s, name))
                || b.tail.as_deref().is_some_and(|t| expr_reads_ident(t, name))
        }
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn analyze_src(src: &str) -> String {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        analyze(&prog).dump()
    }

    fn check_src(src: &str) -> Vec<String> {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, &PathBuf::from("t.cplus"), src);
        diags.into_iter().map(|d| d.code.0.to_string()).collect()
    }

    // --- 5BC.1 tests preserved ---

    // ---- v0.0.28: the three guarantees a thread scope rests on ----
    //
    // `stdlib/thread`'s `Scope::lend` is `#[keeps(this)] fn lend[T](ref this,
    // ref data: T, f: fn(ref T))`. Everything that makes it safe is checked
    // here, on a mock with the same shape — the scope keeps the borrow, so
    // the data cannot die, cannot be written, and cannot be lent twice while
    // a worker may still be holding it.

    const SCOPE_MOCK: &str = "struct Data { n: i32 }\n         impl Data { fn drop(ref this) { return; } }\n         struct Scope { c: i32 }\n         impl Scope {\n             #[keeps(this)]\n             fn lend(ref this, ref d: Data) { this.c = this.c + 1; }\n             fn drop(ref this) { return; }\n         }\n";

    #[test]
    fn scope_outliving_its_lent_data_is_rejected() {
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 {{ var d: Data = Data {{ n: 1 }}; s.lend(d); }}\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "data must outlive the scope: {codes:?}"
        );
    }

    #[test]
    fn writing_lent_data_is_rejected() {
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 var d: Data = Data {{ n: 1 }};\n                 s.lend(d);\n                 d.n = 5;\n                 s.c = 2;\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "a write into borrowed data must be refused: {codes:?}"
        );
    }

    #[test]
    fn lending_the_same_data_twice_is_rejected() {
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 var d: Data = Data {{ n: 1 }};\n                 s.lend(d);\n                 s.lend(d);\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "two exclusive lends of one place must be refused: {codes:?}"
        );
    }

    #[test]
    fn lending_two_different_places_is_fine() {
        // The rule is about the PLACE, not about the scope: two workers on
        // two locals is the whole point of the API.
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 var a: Data = Data {{ n: 1 }};\n                 var b: Data = Data {{ n: 2 }};\n                 s.lend(a);\n                 s.lend(b);\n                 return 0;\n             }}"
        ));
        assert!(codes.is_empty(), "two locals, two workers: {codes:?}");
    }

    // ---- 2026-08-22: the two holes the guarantees above were leaking
    // through. Both were filed against `thread::Scope`; neither is about
    // threads.
    //
    // `SCOPE_MOCK`'s `Data` is non-Copy (it declares `fn drop`) and every
    // test above names the scope again after the offending line. Removing
    // either of those two accidents removed the diagnostic:
    //
    //   * a Copy argument recorded NO LOAN AT ALL, because the §5 tie was
    //     gated on `binding_is_non_copy` — so the same program with
    //     `struct Cell { n: i64 }` lost all three guarantees, and a plain
    //     counter is exactly what a scope is for;
    //   * a scope not mentioned again had its loan released at the lend by
    //     NLL last-use, so read, write and re-lend were all admitted after
    //     it.
    //
    // `COP` is `SCOPE_MOCK`'s `Data` with the destructor removed and nothing
    // else changed: every pair below is the same program, twice.

    const COP: &str = "struct Cop { n: i32 }\n         struct CScope { c: i32 }\n         impl CScope {\n             #[keeps(this)]\n             fn lend(ref this, ref d: Cop) { this.c = this.c + 1; }\n             fn drop(ref this) { return; }\n         }\n         fn touch(ref d: Cop) { d.n = d.n + 1; return; }\n";

    #[test]
    fn lending_a_copy_place_twice_is_rejected() {
        let codes = check_src(&format!(
            "{COP}\
             fn main() -> i32 {{\n                 var s: CScope = CScope {{ c: 0 }};\n                 var d: Cop = Cop {{ n: 1 }};\n                 s.lend(d);\n                 s.lend(d);\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "a Copy place is still a place: two exclusive lends must be refused; got {codes:?}"
        );
    }

    #[test]
    fn writing_a_lent_copy_place_is_rejected() {
        let codes = check_src(&format!(
            "{COP}\
             fn main() -> i32 {{\n                 var s: CScope = CScope {{ c: 0 }};\n                 var d: Cop = Cop {{ n: 1 }};\n                 s.lend(d);\n                 d.n = 5;\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "`ref` is by-pointer for every type, Copy included; got {codes:?}"
        );
    }

    #[test]
    fn a_copy_place_dying_before_its_scope_is_rejected() {
        let codes = check_src(&format!(
            "{COP}\
             fn main() -> i32 {{\n                 var s: CScope = CScope {{ c: 0 }};\n                 {{ var d: Cop = Cop {{ n: 1 }}; s.lend(d); }}\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "the worker holds the address of a dead stack slot; got {codes:?}"
        );
    }

    #[test]
    fn reading_a_lent_place_is_rejected() {
        // The row the borrow checker had no state for: the §5 tie recorded
        // `BorrowedShared` for a `ref` parameter, and a shared loan admits
        // reads. A `ref` position is EXCLUSIVE, so E0383 has something to
        // fire on — and a read racing a worker's write is what TSan reports.
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 var d: Data = Data {{ n: 1 }};\n                 s.lend(d);\n                 let v: i32 = d.n;\n                 return v;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0383" || c == "E0374"),
            "a lent place cannot be read while a worker may be writing it; got {codes:?}"
        );
    }

    #[test]
    fn a_plain_ref_call_on_a_lent_place_is_rejected() {
        // The one that costs memory safety rather than tidiness: a plain
        // `ref` argument is exactly as much of an exclusive borrow as `lend`
        // is, so this program had two live exclusive borrows of one place on
        // two threads — the state memory-model.md §4 says cannot exist.
        // It is also the natural way to write a two-way split, which is why
        // "put a Text in the struct" was no defence.
        let codes = check_src(&format!(
            "{COP}\
             fn main() -> i32 {{\n                 var s: CScope = CScope {{ c: 0 }};\n                 var d: Cop = Cop {{ n: 1 }};\n                 s.lend(d);\n                 touch(d);\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "the parent may not take the lent place exclusively too; got {codes:?}"
        );
    }

    #[test]
    fn a_non_ref_keeps_position_still_admits_reads() {
        // The boundary. `#[keeps(this)]` on a BY-VALUE position stores a
        // VIEW of the argument (`names.push(t)`), which is a shared loan —
        // reading the owner afterwards is sound and must stay admitted.
        // Every other `#[keeps(this)]` in vendor/ is this shape; only
        // `Scope::lend` has a `ref` parameter, which is why the flavour is
        // read off the parameter mode rather than applied to the attribute.
        let src = "struct Holder { s: str }\n\
             impl Holder {\n\
                 #[keeps(this)]\n\
                 fn set(ref this, k: str) { this.s = k; }\n\
                 fn drop(ref this) { return; }\n\
             }\n\
             struct Owner { n: i32 }\n\
             impl Owner { fn drop(ref this) { return; } }\n\
             fn view(o: Owner) -> str { return \"x\"; }\n\
             fn main() -> i32 {\n\
                 var o: Owner = Owner { n: 1 };\n\
                 var h: Holder = Holder { s: \"\" };\n\
                 h.set(view(o));\n\
                 let v: i32 = o.n;\n\
                 return v;\n\
             }";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0383"),
            "a stored view is a SHARED loan; reads of the owner stay legal; got {codes:?}"
        );
    }

    #[test]
    fn place_canonical_root_only() {
        assert_eq!(Place::root("buf").canonical(), "buf");
    }

    #[test]
    fn place_canonical_with_projections() {
        let p = Place {
            root: "buf".into(),
            projections: vec![
                Projection::Field("payload".into()),
                Projection::Index(3),
                Projection::AnyIndex,
            ],
        };
        assert_eq!(p.canonical(), "buf.payload[3][*]");
    }

    #[test]
    fn merge_owned_owned_is_owned() {
        assert_eq!(
            PlaceState::Owned.merge(&PlaceState::Owned),
            PlaceState::Owned
        );
    }

    #[test]
    fn merge_owned_moved_is_maybe_partial() {
        assert_eq!(
            PlaceState::Owned.merge(&PlaceState::Moved),
            PlaceState::MaybePartial
        );
        assert_eq!(
            PlaceState::Moved.merge(&PlaceState::Owned),
            PlaceState::MaybePartial
        );
    }

    #[test]
    fn merge_borrowed_shared_takes_max() {
        assert_eq!(
            PlaceState::BorrowedShared(2).merge(&PlaceState::BorrowedShared(5)),
            PlaceState::BorrowedShared(5)
        );
    }

    #[test]
    fn empty_function_has_entry_and_exit_only() {
        let dump = analyze_src("fn f() { return; }");
        assert_eq!(dump, "fn f:\n  entry: {}\n  after stmt 0: {}\n  exit: {}\n");
    }

    #[test]
    fn parameters_appear_in_entry_state() {
        let dump = analyze_src("fn f(a: i32, b: i32) { return; }");
        assert!(dump.contains("entry: {a=Owned, b=Owned}"), "got:\n{dump}");
    }

    #[test]
    fn let_binding_appears_after_its_statement() {
        let src = "fn f() {\n  let x: i32 = 1;\n  return;\n}";
        let dump = analyze_src(src);
        assert!(dump.contains("entry: {}"));
        assert!(dump.contains("after stmt 0: {x=Owned}"));
        assert!(dump.contains("exit: {x=Owned}"));
    }

    #[test]
    fn method_appears_as_type_dot_method_in_analysis() {
        let src = "\
struct P { x: i32 }
impl P { fn read(this) -> i32 { return this.x; } }
fn main() -> i32 { return 0; }";
        let dump = analyze_src(src);
        assert!(dump.contains("fn P.read:"), "got:\n{dump}");
        assert!(dump.contains("entry: {self=Owned}"), "got:\n{dump}");
    }

    #[test]
    fn for_range_loop_var_scoped_to_body() {
        // 5BC.2b: the for-range loop var is scoped to the body. Both
        // `i` and `_x` should appear inside the loop's walk but be
        // dropped from state at the loop's join (the snapshot taken
        // *after* the for statement). The test pins the scoping rule.
        let src = "\
fn f() {
  for i in 0..3 {
    let _x: i32 = i;
  }
  return;
}";
        let dump = analyze_src(src);
        // entry: empty (no params)
        // after stmt 0: the for statement closes; `i` and `_x` are scoped to its
        //   body, so they're not in state here.
        assert!(dump.contains("after stmt 0: {}"), "got:\n{dump}");
        assert!(dump.contains("exit: {}"), "got:\n{dump}");
        // Sanity: no panic; analyzer walked the body without leaking
        // the loop-local bindings.
    }

    #[test]
    fn dump_is_deterministic_across_runs() {
        let src = "fn f(a: i32, b: i32) { let c: i32 = a; return; }";
        let d1 = analyze_src(src);
        let d2 = analyze_src(src);
        assert_eq!(d1, d2);
    }

    // --- 5BC.2a CopyOracle tests ---

    #[test]
    fn copy_oracle_marks_drop_struct_non_copy() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let oracle = CopyOracle::build(&prog);
        let t = Type {
            kind: TypeKind::Path("B".into()),
            span: Span::new(0, 0),
        };
        assert!(oracle.definitely_non_copy(&t), "B should be non-Copy");
        assert!(!oracle.is_copy(&t));
    }

    #[test]
    fn copy_oracle_marks_plain_struct_copy() {
        let src = "struct P { x: i32, y: i32 }";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let oracle = CopyOracle::build(&prog);
        let t = Type {
            kind: TypeKind::Path("P".into()),
            span: Span::new(0, 0),
        };
        assert!(oracle.is_copy(&t));
        assert!(
            !oracle.definitely_non_copy(&t),
            "Copy struct should not be definitely_non_copy"
        );
    }

    #[test]
    fn copy_oracle_handles_unknown_type_as_not_definitely_non_copy() {
        let src = "fn f() { return; }";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let oracle = CopyOracle::build(&prog);
        let t = Type {
            kind: TypeKind::Path("Mystery".into()),
            span: Span::new(0, 0),
        };
        assert!(
            !oracle.definitely_non_copy(&t),
            "Unknown types should not fire definitely_non_copy"
        );
    }

    #[test]
    fn copy_oracle_propagates_non_copy_through_struct_field() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
struct Outer { b: B, n: i32 }";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let oracle = CopyOracle::build(&prog);
        let outer = Type {
            kind: TypeKind::Path("Outer".into()),
            span: Span::new(0, 0),
        };
        assert!(
            oracle.definitely_non_copy(&outer),
            "Outer should be non-Copy because it contains B"
        );
    }

    #[test]
    fn copy_oracle_primitives_are_copy() {
        let prog = parse(tokenize("fn f() { return; }").unwrap()).unwrap();
        let oracle = CopyOracle::build(&prog);
        for name in ["i32", "u64", "f64", "bool", "usize"] {
            let t = Type {
                kind: TypeKind::Path(name.into()),
                span: Span::new(0, 0),
            };
            assert!(oracle.is_copy(&t), "{name} should be Copy");
            assert!(
                !oracle.definitely_non_copy(&t),
                "{name} should not be definitely_non_copy"
            );
        }
    }

    // --- 5BC.2a Copy-gating tests (Owned→Moved only on non-Copy) ---

    #[test]
    fn move_of_copy_binding_does_not_transition_state() {
        // i32 is Copy — the move marker bit-copies, source stays Owned.
        // (Sema may eventually lint this as E0336 but for now silently
        // accepts; borrowck must not over-track.)
        let src = "\
fn sink(take x: i32) { return; }
fn caller() {
  let y: i32 = 7;
  sink(y);
  return;
}";
        let dump = analyze_src(src);
        assert!(dump.contains("exit: {y=Owned}"), "got:\n{dump}");
    }

    #[test]
    fn move_of_non_copy_binding_transitions_to_moved() {
        // B is non-Copy because it has a `drop`. The move actually consumes.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  sink(y);
  return;
}";
        let dump = analyze_src(src);
        // y should be Moved after the sink call.
        let exit_line = dump
            .lines()
            .find(|l| l.starts_with("fn caller:") || l.contains("exit:"))
            .unwrap_or("");
        assert!(dump.contains("y=Moved"), "y should be Moved; got:\n{dump}");
        let _ = exit_line; // for clarity if assert fails
    }

    // --- 5BC.2a E0370 emission tests ---

    #[test]
    fn e0370_fires_on_move_and_read_of_same_non_copy_binding() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let y: B = B { x: 1 };
  drain(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0370"),
            "expected E0370 in {codes:?}"
        );
    }

    #[test]
    fn e0370_does_not_fire_on_copy_binding() {
        let src = "\
fn drain(take x: i32, n: i32) { return; }
fn peek(x: i32) -> i32 { return x; }
fn caller() {
  let y: i32 = 1;
  drain(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0370"),
            "E0370 should not fire on Copy bindings; got {codes:?}"
        );
    }

    // v0.0.15: a bare (implicit-move) non-Copy *concrete* struct/enum call
    // argument is now classified as a MOVE (matching codegen's `effective_move`
    // and sema's E0335), not a shared read. Passing such an arg while its place
    // is exclusively borrowed therefore fires E0372 (move-while-borrowed), not
    // E0383 (read-while-borrowed). Pre-fix, `param_moves` used the raw `move_`
    // keyword, so the bare arg was treated as a read and mis-reported E0383.
    #[test]
    fn take_noncopy_struct_arg_while_borrowed_is_move_e0372() {
        // v0.0.24 #9: only a `take` param consumes. Moving `v` into `take(v)`
        // while `cur` still exclusively borrows it is a move-while-borrowed
        // (E0372). (A bare arg is a read — that case is E0383, covered by
        // `e0383_fires_on_read_of_exclusively_borrowed_place`.)
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn take(take b: B) -> i32 { return 0; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  let n: i32 = take(v);
  let m: i32 = cur.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "take non-Copy struct arg while borrowed should move (E0372), got {codes:?}"
        );
    }

    // 2026-08-28: the GENERIC half of the same rule, which was missing.
    // `param_is_effective_move` asked whether the WRITTEN parameter type is
    // definitely non-Copy, and a bare `T` never is before monomorphization —
    // so `fn gsink[T](take x: T)` recorded no move at all and a live view
    // outlived the value it borrowed. That is not a corner: the signature is
    // `thread::spawn_with[I, O](take input: I, ...)`, so the standard
    // library's threading entry point moved a `Text` into a worker out from
    // under a `str` view of it, and the checker reported clean. Confirmed as a
    // real heap-use-after-free under `--asan` (freed by `Text.drop` inside the
    // instantiated sink) before the fix went in.
    //
    // The comment on the old code called treating `T` as a non-move
    // "conservative". It is the opposite: for a move-while-borrowed check,
    // assuming no move is the PERMISSIVE direction, and it admits a real one.
    #[test]
    fn generic_take_param_while_borrowed_is_move_e0372() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn gsink[T](take v: T) -> i32 { return 0; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  let n: i32 = gsink::[B](v);
  let m: i32 = cur.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "a `take` param typed by the callee's own generic param still moves; got {codes:?}"
        );
    }

    // The other half, and the reason the call site asks the ARGUMENT rather
    // than trusting the parameter: `T` may instantiate Copy, and moving a Copy
    // value invalidates nothing. Marking every generic `take` a move outright
    // would have turned this into a false E0335/E0372 — a compile error on
    // correct code, which is worse than the hole it closes.
    #[test]
    fn generic_take_param_of_a_copy_argument_is_not_a_move() {
        let src = "\
fn gsink[T](take v: T) -> i32 { return 0; }
fn caller() -> i32 {
  let y: i32 = 1;
  let a: i32 = gsink::[i32](y);
  let b: i32 = gsink::[i32](y);
  return y;
}";
        let codes = check_src(src);
        assert!(
            !codes
                .iter()
                .any(|c| c == "E0372" || c == "E0335" || c == "E0370"),
            "a Copy instantiation consumes nothing; got {codes:?}"
        );
    }

    #[test]
    fn take_noncopy_enum_arg_while_borrowed_is_move_e0372() {
        let src = "\
struct Leaf { tag: i32 }
impl Leaf { fn drop(ref this) { return; } }
enum E { A(Leaf), B }
fn cursor(ref e: E) -> E { return e; }
fn take(take e: E) -> i32 { return 0; }
fn peek(e: E) -> i32 { return 0; }
fn caller() {
  let v: E = E::B;
  let cur: E = cursor(v);
  let n: i32 = take(v);
  let m: i32 = peek(cur);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "take non-Copy enum arg while borrowed should move (E0372), got {codes:?}"
        );
    }

    #[test]
    fn e0370_does_not_fire_when_other_arg_does_not_read_binding() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn caller() {
  let y: B = B { x: 1 };
  let z: i32 = 42;
  drain(y, z);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0370"),
            "E0370 should not fire when sibling arg doesn't read the moved binding; got {codes:?}"
        );
    }

    #[test]
    fn e0370_does_not_fire_on_unknown_binding_type() {
        // The promised 5BC.2/5BC.4 tightening landed 2026-08-01 as
        // structural inference: `let y = B { x: 1 };` resolves to B, so
        // E0370 now correctly fires on the move-and-borrow call below.
        // The Unknown-stays-silent guard moves to a GENUINELY
        // unresolvable initializer (a call through a fn-pointer local —
        // no declared return type reachable).
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let y = B { x: 1 };
  drain(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0370"),
            "inferred binding type must enable E0370; got {codes:?}"
        );
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller(make: fn() -> B) {
  let y = make();
  drain(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0370"),
            "E0370 must stay silent on a genuinely unresolvable binding; got {codes:?}"
        );
    }

    #[test]
    fn clean_function_produces_no_diagnostics() {
        let src = "\
fn add(a: i32, b: i32) -> i32 { return a + b; }
fn main() -> i32 { return add(2, 3); }";
        let codes = check_src(src);
        assert!(codes.is_empty(), "expected no diagnostics; got {codes:?}");
    }

    // --- 5BC.2b branch-merge state tests ---

    #[test]
    fn asymmetric_if_move_produces_maybe_partial() {
        // The then-branch moves `y`; else-branch doesn't. After the
        // if, `y`'s state is MaybePartial (Owned ∩ Moved). Pins the
        // merge-rule behavior.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn caller(c: bool) {
  let y: B = B { x: 1 };
  if c {
    sink(y);
  }
  return;
}";
        let dump = analyze_src(src);
        // After the if statement, `y` is MaybePartial.
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(
            line.contains("y=MaybePartial"),
            "expected y=MaybePartial in: {line}"
        );
    }

    #[test]
    fn symmetric_if_move_in_both_branches_is_moved() {
        // Both branches move `y`. After the if, `y` is definitively
        // Moved — not MaybePartial.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn caller(c: bool) {
  let y: B = B { x: 1 };
  if c {
    sink(y);
  } else {
    sink(y);
  }
  return;
}";
        let dump = analyze_src(src);
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(line.contains("y=Moved"), "expected y=Moved in: {line}");
    }

    #[test]
    fn diverging_branch_excluded_from_merge() {
        // Then-branch moves `y` then returns. Else-branch doesn't run
        // (no else here, but the if-without-else case: only the
        // "no-then-taken" path flows through). Since the then-branch
        // diverges (return), its post-state is excluded from the join;
        // the join inherits the pre-if state where `y` is Owned.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn caller(c: bool) -> i32 {
  let y: B = B { x: 1 };
  if c {
    sink(y);
    return 1;
  }
  return 0;
}";
        let dump = analyze_src(src);
        // After the if (the no-then path), `y` is still Owned. The
        // diverging then-branch's Moved state is filtered out by
        // merge_branches.
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(line.contains("y=Owned"), "expected y=Owned in: {line}");
    }

    #[test]
    fn branch_local_let_does_not_leak() {
        // A `let` introduced inside an if branch should not appear in
        // post-if state. Pins scope-restriction in walk_block_in_scope.
        let src = "\
fn caller(c: bool) {
  if c {
    let inner: i32 = 1;
  } else {
    let other: i32 = 2;
  }
  return;
}";
        let dump = analyze_src(src);
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 0:"))
            .unwrap_or_else(|| panic!("no after stmt 0 in:\n{dump}"));
        assert!(
            !line.contains("inner"),
            "branch-local `inner` should not leak: {line}"
        );
        assert!(
            !line.contains("other"),
            "branch-local `other` should not leak: {line}"
        );
    }

    #[test]
    fn loop_body_move_produces_maybe_partial() {
        // A move inside a while body: the body might not run (0
        // iterations), so post-loop `y`'s state is MaybePartial
        // (pre-state Owned merged with body-end Moved).
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn caller(c: bool) {
  let y: B = B { x: 1 };
  while c {
    sink(y);
  }
  return;
}";
        let dump = analyze_src(src);
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(
            line.contains("y=MaybePartial"),
            "expected y=MaybePartial in: {line}"
        );
    }

    // --- 5BC.2b E0371 emission tests ---

    #[test]
    fn e0371_does_not_fire_on_copy_binding_after_asymmetric_branch() {
        // i32 is Copy — the "move" is a bit-copy that leaves the source
        // Owned. State after the if is Owned, not MaybePartial. No
        // E0371.
        let src = "\
fn sink(take x: i32) { return; }
fn caller(c: bool) {
  let y: i32 = 1;
  if c { sink(y); }
  let z: i32 = y;
  let _ignore: i32 = z;
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0371"),
            "E0371 should not fire on Copy bindings; got {codes:?}"
        );
    }

    #[test]
    fn diverging_match_arms_excluded() {
        // All-arms-diverge: every arm of the match returns, so post-
        // match state is pre-match state (no Moved leakage).
        // Match-arm bodies are expressions, so `return` must live in a
        // block.
        let src = "\
enum Color { Red, Green, Blue }
fn caller(c: Color) -> i32 {
  match c {
    Color::Red => { return 1; },
    Color::Green => { return 2; },
    Color::Blue => { return 3; },
  }
  return 0;
}";
        // Smoke test: program analyzes without panicking; merge_branches
        // handles the all-diverge case via the pre-state fallback.
        let codes = check_src(src);
        assert!(!codes.iter().any(|c| c == "E0371"), "got {codes:?}");
    }

    // --- 5BC.3a Rule E1 / E2 elision detection ---

    fn parse_prog(src: &str) -> Program {
        let toks = tokenize(src).expect("lex");
        parse(toks).expect("parse")
    }

    #[test]
    fn e1_fires_on_single_param_passthrough() {
        // Single non-Copy shared-borrow param + non-Copy return + body is
        // `return b;`. Rule E1 matches.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source(&prog, "passthrough"),
            Some(ReturnBorrowSource::Param(0))
        );
    }

    #[test]
    fn e1_fires_on_return_of_field_access_rooted_at_param() {
        // `return b.inner;` — chain of field accesses rooted at the
        // parameter still qualifies under E1.
        let src = "\
struct Inner { x: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Outer { inner: Inner }
impl Outer { fn drop(ref this) { return; } }
fn pull(o: Outer) -> Inner { return o.inner; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source(&prog, "pull"),
            Some(ReturnBorrowSource::Param(0))
        );
    }

    #[test]
    fn e1_does_not_fire_when_return_constructs_fresh_value() {
        // The body constructs a new B and returns it. E1 doesn't apply
        // — the return is owned, not a borrow.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn make(b: B) -> B { return B { x: 0 }; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "make"), None);
    }

    #[test]
    fn e1_does_not_fire_on_copy_param() {
        // i32 is Copy. E1 only applies to non-Copy types.
        let src = "fn id(x: i32) -> i32 { return x; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "id"), None);
    }

    #[test]
    fn e1_does_not_fire_with_move_marker() {
        // `take b: B` — the function takes ownership, the return is a
        // transferred owned value, not a borrow.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn consume(take b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "consume"), None);
    }

    #[test]
    fn e_view_fn_ties_str_return_to_non_copy_borrow_params() {
        // Rule E-VIEW-FN: a free fn returning `str` with non-Copy borrow
        // params is a view of those params — regardless of what the body
        // returns (same conservatism as the method rule). Copy params
        // never contribute; all-Copy signatures don't tie.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn head(b: B) -> str { return \"h\"; }
fn fmt(b: B, n: i32) -> str { return \"f\"; }
fn pick(a: B, b: B) -> str { return \"p\"; }
fn lit(n: i32) -> str { return \"l\"; }
fn eat(take b: B) -> str { return \"e\"; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "head"),
            Some((ReturnBorrowSource::Param(0), BorrowFlavor::Shared))
        );
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "fmt"),
            Some((ReturnBorrowSource::Param(0), BorrowFlavor::Shared))
        );
        assert_eq!(
            return_borrow_source(&prog, "pick"),
            Some(ReturnBorrowSource::MultiParam(vec![0, 1]))
        );
        assert_eq!(return_borrow_source(&prog, "lit"), None);
        assert_eq!(return_borrow_source(&prog, "eat"), None);
    }

    #[test]
    fn e1_mut_fires_on_mut_marker_with_exclusive_flavor() {
        // Slice 6BC.2: `ref b: B` qualifies for Rule E1-mut. The return
        // is classified as an *exclusive* borrow of the parameter.
        // Compare to Rule E1 (shared form) which requires no marker.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn through(ref b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "through"),
            Some((ReturnBorrowSource::Param(0), BorrowFlavor::Exclusive))
        );
    }

    #[test]
    fn e1_does_not_fire_with_multiple_params() {
        // Multi-param functions are not E1's domain — they go to Rule
        // E3 (5BC.4). This `pick` returns only `a`, so E3 records
        // `MultiParam([0])` — only param 0 is in the return's source
        // set. (E1 would have required exactly one param.)
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn pick(a: B, b: B) -> B { return a; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source(&prog, "pick"),
            Some(ReturnBorrowSource::MultiParam(vec![0])),
        );
    }

    #[test]
    fn e1_does_not_fire_with_no_return_type() {
        // Void return — no value flows.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn noop(b: B) { return; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "noop"), None);
    }

    #[test]
    fn e1_does_not_fire_when_some_path_doesnt_return_rooted() {
        // Body has a return that's rooted at the param AND another
        // return that constructs a fresh value. E1 requires every
        // return to be rooted at the param.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn maybe(b: B, c: bool) -> B {
  if c {
    return b;
  }
  return B { x: 0 };
}";
        let prog = parse_prog(src);
        // Function has two params (b and c) so E1 is already disqualified
        // by the multi-param rule. To exercise the "non-rooted return"
        // path, we need a single-param example below.
        assert_eq!(return_borrow_source(&prog, "maybe"), None);
    }

    #[test]
    fn e1_does_not_fire_when_one_branch_doesnt_return_rooted_single_param() {
        // Single param but one branch returns a fresh value. E1 rejects.
        // (Requires a way to vary control flow without a bool param.
        // Use a match on a same-file enum.)
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn weird(b: B) -> B {
  // A nested block expression that diverges-on-one-arm via match-of-a-field
  match b.x {
    _ => { return B { x: 0 }; },
  }
}";
        let prog = parse_prog(src);
        // The match arm doesn't return `b` — it constructs a fresh B.
        // E1 should reject because the only return-path doesn't root at
        // `b`.
        //
        // (Literal `match` arms landed with reports/bug-25, and `lower`
        // desugars them to an if/else chain before borrowck runs — so this
        // shape reaches borrowck as ordinary control flow either way.)
        // Assert only that the result is not Some(Param(0)).
        assert_eq!(return_borrow_source(&prog, "weird"), None);
    }

    #[test]
    fn e2_fires_on_self_passthrough_method() {
        // Rule E2: method with `this` receiver + non-Copy target + non-Copy
        // return + every return rooted at `this`.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn pass(this) -> B { return this; }
}";
        let prog = parse_prog(src);
        assert_eq!(
            method_return_borrow_source(&prog, "B", "pass"),
            Some(ReturnBorrowSource::SelfReceiver)
        );
    }

    #[test]
    fn e2_fires_on_self_field_access() {
        // `return this.field;` — Rule E2 admits field chains rooted at
        // the receiver.
        let src = "\
struct Inner { x: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Outer { inner: Inner }
impl Outer {
  fn drop(ref this) { return; }
  fn payload(this) -> Inner { return this.inner; }
}";
        let prog = parse_prog(src);
        assert_eq!(
            method_return_borrow_source(&prog, "Outer", "payload"),
            Some(ReturnBorrowSource::SelfReceiver)
        );
    }

    #[test]
    fn e2_mut_fires_on_mut_self_with_exclusive_flavor() {
        // Slice 6BC.2: `ref this` qualifies for Rule E2-mut. The return
        // is an exclusive borrow of `this`. Rule E2 (shared `this`)
        // continues to apply separately when the receiver is `this`.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn pass(ref this) -> B { return this; }
}";
        let prog = parse_prog(src);
        assert_eq!(
            method_return_borrow_source_with_flavor(&prog, "B", "pass"),
            Some((ReturnBorrowSource::SelfReceiver, BorrowFlavor::Exclusive))
        );
    }

    #[test]
    fn e2_does_not_fire_on_move_self() {
        // `take this` is ownership transfer; the receiver is owned by
        // the method, so the return is an owned transfer, not a borrow.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn pass(take this) -> B { return this; }
}";
        let prog = parse_prog(src);
        assert_eq!(method_return_borrow_source(&prog, "B", "pass"), None);
    }

    #[test]
    fn e2_does_not_fire_on_copy_target() {
        // Copy struct (no Drop, no non-Copy fields) — E2 only applies
        // to non-Copy targets.
        let src = "\
struct P { x: i32, y: i32 }
impl P {
  fn dup(this) -> P { return this; }
}";
        let prog = parse_prog(src);
        assert_eq!(method_return_borrow_source(&prog, "P", "dup"), None);
    }

    #[test]
    fn detection_does_not_emit_diagnostics_in_isolation() {
        // 5BC.3a is analysis-only: detecting an E1 / E2 candidate
        // signature must not cause borrowck to emit any diagnostic
        // through the pipeline `check()` entry. (Call-site borrow
        // tracking + E0372 / E0373 come in 5BC.3b.)
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn pass(this) -> B { return this; }
}
fn passthrough(b: B) -> B { return b; }
fn main() -> i32 { return 0; }";
        let codes = check_src(src);
        assert!(codes.is_empty(), "5BC.3a should not emit; got {codes:?}");
    }

    // --- 5BC.4 Rule E3 multi-parameter elision ---

    #[test]
    fn e3_fires_on_longest_pattern() {
        // The design note's Phase-5 exit criterion. Function has two
        // non-Copy shared-borrow params; branches return either the
        // first or the second. Rule E3 records MultiParam([0, 1]).
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x {
    return a;
  }
  return b;
}";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source(&prog, "longest"),
            Some(ReturnBorrowSource::MultiParam(vec![0, 1])),
        );
    }

    #[test]
    fn e3_call_records_borrows_from_every_source() {
        // `let r = longest(a, b);` records `r` as borrowing from both
        // `a` and `b`. State after the let: both BorrowedShared(1).
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  let r: B = longest(a, b);
  let n: i32 = r.x;
  return;
}";
        let dump = analyze_src(src);
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 2:"))
            .unwrap_or_else(|| panic!("no after stmt 2 in:\n{dump}"));
        assert!(
            line.contains("a=BorrowedShared(1)"),
            "expected a=BorrowedShared(1); got: {line}"
        );
        assert!(
            line.contains("b=BorrowedShared(1)"),
            "expected b=BorrowedShared(1); got: {line}"
        );
    }

    #[test]
    fn e3_fires_e0372_on_move_of_any_source() {
        // Moving either `a` or `b` while `r` borrows from both fires
        // E0372. This test moves `a`; symmetric case for `b` follows
        // the same path through `check_move_against_borrow`.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn drain(take b: B) { return; }
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  let r: B = longest(a, b);
  drain(a);
  let n: i32 = r.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "expected E0372 on move of `a` while `r` borrows it; got {codes:?}"
        );
    }

    #[test]
    fn e3_fires_e0372_on_move_of_other_source() {
        // The mirror case — moving `b` instead of `a`. Same diagnostic.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn drain(take b: B) { return; }
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  let r: B = longest(a, b);
  drain(b);
  let n: i32 = r.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "expected E0372 on move of `b` while `r` borrows it; got {codes:?}"
        );
    }

    #[test]
    fn e3_does_not_fire_when_some_path_returns_fresh_value() {
        // One return path constructs a fresh value (`return B { x: 0 };`).
        // E3 requires every return rooted at some parameter, so it
        // disqualifies.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn maybe(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return B { x: 0 };
}";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "maybe"), None);
    }

    #[test]
    fn e3_does_not_fire_with_copy_param() {
        // Rule E3 requires every param non-Copy. A Copy param disqualifies.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn weird(a: B, n: i32) -> B { return a; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "weird"), None);
    }

    #[test]
    fn e3_does_not_fire_with_move_param() {
        // Rule E3 requires shared-borrow form on every param.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn weird(take a: B, b: B) -> B { return a; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source(&prog, "weird"), None);
    }

    #[test]
    fn e3_borrow_released_after_borrower_scope_exits() {
        // The longest borrower lives inside a block; after the block
        // closes, both `a` and `b` return to Owned. The subsequent move
        // of `a` is permitted.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn drain(take b: B) { return; }
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  {
    let r: B = longest(a, b);
  }
  drain(a);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0372"),
            "E0372 should not fire after r's scope closes; got {codes:?}"
        );
    }

    // --- 5BC.3b E0372 + call-site borrow tracking ---

    #[test]
    fn e1_call_records_borrow_in_state() {
        // `let r = passthrough(x);` records `r` as borrowing from `x`.
        // After the let-stmt, `x` is `BorrowedShared(1)`.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn caller() {
  let x: B = B { x: 1 };
  let y: B = passthrough(x);
  let n: i32 = y.x;
  return;
}";
        let dump = analyze_src(src);
        // After stmt 1 (the `let y` line): x is BorrowedShared(1). The later
        // read of `y` keeps the borrow live there (NLL releases at last use).
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(
            line.contains("x=BorrowedShared(1)"),
            "expected x=BorrowedShared(1); got: {line}"
        );
    }

    #[test]
    fn e2_method_call_records_borrow_in_state() {
        // `let r = b.pass();` where pass is E2-classified.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn pass(this) -> B { return this; }
}
fn caller() {
  let b: B = B { x: 1 };
  let r: B = b.pass();
  let n: i32 = r.x;
  return;
}";
        let dump = analyze_src(src);
        let line = dump
            .lines()
            .find(|l| l.contains("after stmt 1:"))
            .unwrap_or_else(|| panic!("no after stmt 1 in:\n{dump}"));
        assert!(
            line.contains("b=BorrowedShared(1)"),
            "expected b=BorrowedShared(1); got: {line}"
        );
    }

    #[test]
    fn e0372_fires_on_move_while_e1_borrow_live() {
        // The classic case: `let r = passthrough(x); drain(take x);`
        // where drain takes `take b: B`. The move-arg path detects `x`
        // is borrowed and fires E0372.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let x: B = B { x: 1 };
  let r: B = passthrough(x);
  drain(x);
  let n: i32 = r.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "expected E0372; got {codes:?}"
        );
    }

    #[test]
    fn e0372_does_not_fire_after_borrower_scope_exits() {
        // `let r = passthrough(x)` is inside a block; the block closes
        // before `drain(take x)` runs. After scope exit r is gone and
        // its borrow is released, so the move is fine.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let x: B = B { x: 1 };
  {
    let r: B = passthrough(x);
  }
  drain(x);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0372"),
            "E0372 should not fire after borrower's scope exits; got {codes:?}"
        );
    }

    #[test]
    fn scope_exit_under_live_borrow_fires_e0514() {
        // Memory-model contract §3.3: assigning a view outward and letting
        // the owner die at the block's end dangles the outer binding —
        // the scope-exit twin of E0372's move-while-borrowed.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
fn caller() {
  var s: str = \"\";
  {
    let t: B = B { x: 1 };
    s = t.view();
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 on owner scope exit under live borrow; got {codes:?}"
        );
    }

    #[test]
    fn scope_exit_with_borrower_dying_together_no_e0514() {
        // Owner and borrower die in the same block: released together,
        // nothing survives to dangle.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
fn caller() {
  {
    let t: B = B { x: 1 };
    let s: str = t.view();
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "E0514 must not fire when owner and borrower die together; got {codes:?}"
        );
    }

    #[test]
    fn scope_exit_outer_owner_inner_borrower_no_e0514() {
        // The sound direction: the owner outlives the block-local view.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
fn caller() {
  let t: B = B { x: 1 };
  {
    let s: str = t.view();
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "E0514 must not fire when the owner outlives the borrower; got {codes:?}"
        );
    }

    #[test]
    fn loop_body_owner_view_assigned_outward_fires_e0514() {
        // The per-iteration owner dies at each body exit while the outer
        // binding keeps the view.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
fn caller(n: i32) {
  var s: str = \"\";
  var i: i32 = 0;
  while i < n {
    let t: B = B { x: 1 };
    s = t.view();
    i = i + 1;
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 for loop-body owner viewed by outer binding; got {codes:?}"
        );
    }

    #[test]
    fn transitive_wrapper_of_keeps_method_ties_e0514() {
        // Contract §5, computed half: `set_outer` never declares
        // #[keeps(this)] and has no direct store, but its body forwards
        // the param to a keeps method on `this`. The flow pass computes
        // the transitive param→receiver flow and the caller ties exactly
        // as for the declared form.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Holder { view: str }
impl Holder {
  #[keeps(this)]
  fn set(ref this, k: str) { this.view = k; return; }
  fn set_outer(ref this, k: str) { this.set(k); return; }
}
fn caller() {
  var h: Holder = Holder { view: \"\" };
  {
    let t: B = B { x: 1 };
    h.set_outer(t.view());
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 through the undeclared wrapper; got {codes:?}"
        );
    }

    #[test]
    fn field_path_receiver_keeps_tie_e0514() {
        // `p.h.set(...)` — the receiver is a field path. The tie resolves
        // the receiver type through the declared-field table and pins the
        // ROOT binding (over-approximate, sound).
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Holder { view: str }
impl Holder {
  #[keeps(this)]
  fn set(ref this, k: str) { this.view = k; return; }
}
struct Panel { h: Holder }
fn caller() {
  var p: Panel = Panel { h: Holder { view: \"\" } };
  {
    let t: B = B { x: 1 };
    p.h.set(t.view());
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 through the field-path receiver; got {codes:?}"
        );
    }

    #[test]
    fn generic_keeps_ties_only_view_instantiations() {
        // Contract §5 / generic ties: `#[keeps(this)]` on a generic-impl
        // method ties per INSTANTIATION — the receiver's type args are
        // substituted into the declared param types before view
        // classification. `Store[str]` ties (E0514 when the owner dies
        // first); `Store[i32]` must not.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Store[T] { opaque p: *u8 }
impl Store[T] {
  #[keeps(this)]
  fn put(ref this, take item: T) { return; }
}
fn view_case() {
  var s: Store[str] = Store[str] { p: 0 as *u8 };
  {
    let t: B = B { x: 1 };
    s.put(t.view());
  }
  return;
}
fn copy_case() {
  var s: Store[i32] = Store[i32] { p: 0 as *u8 };
  {
    let t: B = B { x: 1 };
    s.put(t.x);
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "Store[str] must tie and fire E0514; got {codes:?}"
        );
        assert_eq!(
            codes.iter().filter(|c| *c == "E0514").count(),
            1,
            "Store[i32] must NOT tie; got {codes:?}"
        );
    }

    #[test]
    fn generic_with_unstored_param_is_not_a_carrier() {
        // A generic whose param only appears in fn-pointer positions holds
        // no value of that type — instantiating it with a view-carrier must
        // not create a carrier (the SignalSubscription[Change] false-tie
        // regression).
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Payload { s: str }
struct Handle[T] { cb: fn(T, *u8), ctx: *u8 }
impl B {
  fn subscribe(ref this, cb: fn(Payload, *u8)) -> Handle[Payload] {
    return Handle[Payload] { cb: cb, ctx: 0 as *u8 };
  }
}
fn noop(p: Payload, c: *u8) { return; }
fn caller() {
  var b: B = B { x: 1 };
  let h: Handle[Payload] = b.subscribe(noop);
  let n: i32 = b.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0381" || c == "E0383"),
            "an unstored-param generic must not tie its producer; got {codes:?}"
        );
    }

    #[test]
    fn wrapper_not_reaching_keeps_ties_nothing() {
        // Negative: a method that only READS its view param (no store, no
        // keeps callee) must compute no receiver flow — callers stay free.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Holder { view: str, n: usize }
impl Holder {
  fn measure(ref this, k: str) { this.n = #str_len(k); return; }
}
fn caller() {
  var h: Holder = Holder { view: \"\", n: 0 as usize };
  {
    let t: B = B { x: 1 };
    h.measure(t.view());
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "a read-only method must not tie; got {codes:?}"
        );
    }

    #[test]
    fn scope_exit_carrier_through_call_fires_e0514() {
        // The aggregate route: the view rides inside a returned carrier
        // (`make`), the tie comes from Rule E-VIEW-FN, and the scope
        // check must still see the dying owner under the outer carrier.
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Data { key: str }
fn make(k: str) -> Data { return Data { key: k }; }
fn caller() {
  var d: Data = Data { key: \"\" };
  {
    let t: B = B { x: 1 };
    d = make(t.view());
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 for carrier assigned outward over a dying owner; got {codes:?}"
        );
    }

    // --- Erased-`*u8` transport (2026-08-04): the round-three seam of
    // bugs/str-field-outliving-its-text-is-not-caught. A view rides a
    // carrier into a generic box, `into_raw` erases the carrier to `*u8`,
    // and the enclosing fn's computed return flow must still export the
    // tie so the caller's scope check fires. ---

    // The shared cast of these tests: a viewable owner, a str carrier, a
    // box-alike whose `into_raw` transports its receiver into a raw return.
    const ERASED_PRELUDE: &str = "\
struct Owner { x: i32 }
impl Owner {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
struct Sink { key: str }
struct Bx[T] { _p: *T }
impl Bx[T] {
  fn into_raw(take this) -> *u8 { return this._p as *u8; }
}
fn bnew[T](take v: T) -> Bx[T] { return Bx[T] { _p: 0 as *T }; }
";

    #[test]
    fn erased_raw_return_carries_view_param_fires_e0514() {
        // node_with returns `*u8`, a type that names no view — the tie
        // must come from the promoted body flow, not the signature.
        let src = format!(
            "{ERASED_PRELUDE}\
fn node_with(key: str) -> *u8 {{
  let d: Sink = Sink {{ key: key }};
  let b: Bx[Sink] = bnew::[Sink](d);
  return b.into_raw();
}}
fn caller() {{
  var held: *u8 = 0 as *u8;
  {{
    let a: Owner = Owner {{ x: 1 }};
    held = node_with(a.view());
  }}
  return;
}}"
        );
        let codes = check_src(&src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514: erased *u8 return still carries the view param; got {codes:?}"
        );
    }

    #[test]
    fn erased_raw_return_through_match_payload_fires_e0514() {
        // The full repro shape: the carrier crosses an Option-alike and a
        // match payload before the erasure — payload typing plus the
        // conservative generic transport keep the taint riding.
        let src = format!(
            "{ERASED_PRELUDE}\
enum Opt[T] {{ Some(T), None }}
fn bnew2[T](take v: T) -> Opt[Bx[T]] {{ return Opt[Bx[T]]::None; }}
fn node_with(key: str) -> *u8 {{
  let d: Sink = Sink {{ key: key }};
  return match bnew2::[Sink](d) {{
    Opt[Bx[Sink]]::Some(b) => b.into_raw(),
    Opt[Bx[Sink]]::None => 0 as *u8,
  }};
}}
fn caller() {{
  var held: *u8 = 0 as *u8;
  {{
    let a: Owner = Owner {{ x: 1 }};
    held = node_with(a.view());
  }}
  return;
}}"
        );
        let codes = check_src(&src);
        assert!(
            codes.iter().any(|c| c == "E0514"),
            "expected E0514 through Option/match/into_raw erasure; got {codes:?}"
        );
    }

    #[test]
    fn keeps_nothing_suppresses_erased_return_promotion() {
        // §5 declaration wins: the author signs for the erasure.
        let src = format!(
            "{ERASED_PRELUDE}\
#[keeps(nothing)]
fn node_with(key: str) -> *u8 {{
  let d: Sink = Sink {{ key: key }};
  let b: Bx[Sink] = bnew::[Sink](d);
  return b.into_raw();
}}
fn caller() {{
  var held: *u8 = 0 as *u8;
  {{
    let a: Owner = Owner {{ x: 1 }};
    held = node_with(a.view());
  }}
  return;
}}"
        );
        let codes = check_src(&src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "keeps(nothing) must suppress the promoted tie; got {codes:?}"
        );
    }

    #[test]
    fn copying_callee_does_not_promote_erased_return() {
        // Precision: `wrap` forwards its view param to a CONCRETE callee
        // whose computed return flow is empty (it returns a fresh
        // pointer). The precise flow beats the conservative transport, so
        // wrap exports nothing — the `text::from_str` shape.
        let src = "\
struct Owner { x: i32 }
impl Owner {
  fn drop(ref this) { return; }
  fn view(this) -> str { return \"\"; }
}
fn scrub(k: str) -> *u8 { return 0 as *u8; }
fn wrap(k: str) -> *u8 { return scrub(k); }
fn caller() {
  var held: *u8 = 0 as *u8;
  {
    let a: Owner = Owner { x: 1 };
    held = wrap(a.view());
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "a copying callee must not tie its caller; got {codes:?}"
        );
    }

    #[test]
    fn scalar_param_reaching_raw_return_is_not_promoted() {
        // The promotion mask: a usize has no owner to tie, even when its
        // bits genuinely reach the raw return.
        let src = "\
fn make(n: usize) -> *u8 { return n as *u8; }
fn caller() {
  var held: *u8 = 0 as *u8;
  {
    let x: usize = 5 as usize;
    held = make(x);
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "scalar params must not promote; got {codes:?}"
        );
    }

    #[test]
    fn e0372_does_not_fire_on_copy_param() {
        // Rule E1 doesn't classify Copy-param functions, so no borrow
        // is registered. Moving `x` is fine.
        let src = "\
fn passthrough(b: i32) -> i32 { return b; }
fn drain(take b: i32) { return; }
fn caller() {
  let x: i32 = 1;
  let r: i32 = passthrough(x);
  drain(x);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0372"),
            "E0372 should not fire on Copy params; got {codes:?}"
        );
    }

    #[test]
    fn moving_borrower_releases_borrow() {
        // `let r = passthrough(x); drain_b(take r);` — moving r out
        // releases its borrow on x; subsequent `drain_b(take x)` is OK.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain_b(take b: B) { return; }
fn caller() {
  let x: B = B { x: 1 };
  let r: B = passthrough(x);
  drain_b(r);
  drain_b(x);
  return;
}";
        let codes = check_src(src);
        // Moving r releases its borrow on x; moving x after that is
        // permitted by borrowck. (Note: codegen would still have a
        // double-drop issue with this exact program because runtime
        // semantics for non-Copy non-move param-passing is currently
        // broken — that's the real bug §2.9 implies will be fixed once
        // borrowck takes over from sema for non-Copy param passing.
        // For borrowck's static analysis, this program is clean.)
        assert!(
            !codes.iter().any(|c| c == "E0372"),
            "moving the borrower should release the borrow; got {codes:?}"
        );
    }

    #[test]
    fn diagnostic_carries_machine_applicable_suggestion() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let y: B = B { x: 1 };
  drain(y, peek(y));
  return;
}";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, &PathBuf::from("t.cplus"), src);
        let e0370 = diags
            .iter()
            .find(|d| d.code.0 == "E0370")
            .expect("should have E0370");
        assert!(
            !e0370.suggestions.is_empty(),
            "E0370 should carry a suggestion"
        );
    }

    // ---- 6BC.1 — intra-call exclusive-borrow conflicts ----

    #[test]
    fn e0380_fires_on_two_mut_borrows_of_same_non_copy_binding() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  modify_both(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0380"),
            "expected E0380 in {codes:?}"
        );
        // Exactly one E0380 per conflicting pair — not duplicated.
        let count = codes.iter().filter(|c| *c == "E0380").count();
        assert_eq!(
            count, 1,
            "expected exactly one E0380, got {count}: {codes:?}"
        );
    }

    #[test]
    fn e0380_fires_through_fn_pointer_ref_slots() {
        // Handle-projection Tier 2: intra-call conflict detection sources the
        // per-param `take`/`ref` flags from a fn-pointer local's binding type,
        // so `f(y, y)` through a `fn(ref B, ref B)` pointer rejects exactly
        // like the named-fn call.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn caller() {
  let f: fn(ref B, ref B) = modify_both;
  var y: B = B { x: 1 };
  f(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0380"),
            "expected E0380 through fn-pointer ref slots in {codes:?}"
        );
    }

    #[test]
    fn e0381_fires_through_fn_pointer_ref_plus_shared() {
        // Mut + Shared of the same place through a fn-pointer call → E0381,
        // same as the named-fn form.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn mix(ref a: B, b: B) { return; }
fn caller() {
  let f: fn(ref B, B) = mix;
  var y: B = B { x: 1 };
  f(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "expected E0381 through fn-pointer ref+shared slots in {codes:?}"
        );
    }

    #[test]
    fn e0380_fires_on_two_mut_copy_args() {
        // This asserted the opposite until 2026-08-22, on the strength of
        // memory-model.md §4's "for a Copy type, every mode passes by value".
        // It does not. `ref a: i32` lowers to
        //
        //     define ... @takes_ref_scalar(ptr nonnull align 4 %0)
        //
        // — a pointer, which is the only way "writes reach the caller" can be
        // true. So `modify_both(y, y)` hands the callee two aliasing
        // exclusive pointers to one stack slot, exactly as it would for a
        // non-Copy struct, and E0380 is the right answer for both. The doc
        // paragraph has been corrected.
        let src = "\
fn modify_both(ref a: i32, ref b: i32) { return; }
fn caller() {
  let y: i32 = 1;
  modify_both(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0380"),
            "`ref` is by-pointer for every type; got {codes:?}"
        );
    }

    #[test]
    fn take_of_a_copy_arg_still_carries_no_claim() {
        // The other half of the correction, and the reason the Copy gate is
        // kept for MOVE claims: `take` on a Copy type consumes nothing —
        // the callee gets a copy — so naming one place in a `take` slot and
        // a read slot is not a conflict. Only `ref` aliases.
        let src = "\
fn consume_and_read(take a: i32, b: i32) { return; }
fn caller() {
  let y: i32 = 1;
  consume_and_read(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.is_empty(),
            "a Copy `take` is a copy, not a move; got {codes:?}"
        );
    }

    #[test]
    fn e0380_does_not_fire_on_different_bindings() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  let z: B = B { x: 2 };
  modify_both(y, z);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0380"),
            "E0380 should not fire on distinct bindings; got {codes:?}"
        );
    }

    #[test]
    fn e0381_fires_on_mut_arg_with_sibling_read() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn write_thing(ref a: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let y: B = B { x: 1 };
  write_thing(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "expected E0381 in {codes:?}"
        );
    }

    #[test]
    fn e0381_fires_on_copy_binding() {
        // Same correction as `e0380_fires_on_two_mut_copy_args`: the `ref`
        // slot is a pointer to `y`, and the sibling argument reads `y`
        // through it while the callee may be writing.
        let src = "\
fn write_thing(ref a: i32, n: i32) { return; }
fn peek(x: i32) -> i32 { return x; }
fn caller() {
  let y: i32 = 1;
  write_thing(y, peek(y));
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "an exclusive Copy slot conflicts with a sibling read; got {codes:?}"
        );
    }

    #[test]
    fn e0382_fires_on_mut_arg_with_sibling_move() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn write_and_take(ref a: B, take b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  write_and_take(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0382"),
            "expected E0382 in {codes:?}"
        );
    }

    #[test]
    fn e0382_does_not_fire_when_other_arg_does_not_name_binding() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn modify(ref a: B, take b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  let z: B = B { x: 2 };
  modify(y, z);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0382"),
            "E0382 should not fire on distinct bindings; got {codes:?}"
        );
    }

    #[test]
    fn e0382_suppresses_e0370_for_same_pair() {
        // A `ref`+`take` conflict should fire E0382 only, NOT E0370.
        // E0370 is the move-and-shared-read class; the `ref`-position
        // sibling is a more specific (and structurally different) case.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn write_and_take(ref a: B, take b: B) { return; }
fn caller() {
  let y: B = B { x: 1 };
  write_and_take(y, y);
  return;
}";
        let codes = check_src(src);
        let e0370_count = codes.iter().filter(|c| *c == "E0370").count();
        let e0382_count = codes.iter().filter(|c| *c == "E0382").count();
        assert_eq!(
            e0370_count, 0,
            "E0370 should be suppressed when E0382 fires; got {codes:?}"
        );
        assert_eq!(e0382_count, 1, "expected exactly one E0382; got {codes:?}");
    }

    #[test]
    fn e0380_e0381_e0382_carry_suggestions() {
        // Each new error must carry a help suggestion so the diagnostic
        // pipeline can offer a Quick Fix in the LSP.
        for (label, src) in &[
            (
                "E0380",
                "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn f(ref a: B, ref b: B) { return; }
fn c() { let y: B = B { x: 1 }; f(y, y); return; }",
            ),
            (
                "E0381",
                "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn f(ref a: B, n: i32) { return; }
fn p(b: B) -> i32 { return b.x; }
fn c() { let y: B = B { x: 1 }; f(y, p(y)); return; }",
            ),
            (
                "E0382",
                "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn f(ref a: B, take b: B) { return; }
fn c() { let y: B = B { x: 1 }; f(y, y); return; }",
            ),
        ] {
            let toks = tokenize(src).expect("lex");
            let prog = parse(toks).expect("parse");
            let diags = check(&prog, &PathBuf::from("t.cplus"), src);
            let d = diags
                .iter()
                .find(|d| d.code.0 == *label)
                .unwrap_or_else(|| {
                    panic!(
                        "expected {label}; got {:?}",
                        diags.iter().map(|d| d.code.0).collect::<Vec<_>>()
                    )
                });
            assert!(
                !d.suggestions.is_empty(),
                "{label} should carry a suggestion"
            );
        }
    }

    #[test]
    fn borrowed_exclusive_state_in_merge() {
        // Same borrower on both branches merges to BorrowedExclusive.
        let a = PlaceState::BorrowedExclusive("h".to_string());
        let b = PlaceState::BorrowedExclusive("h".to_string());
        assert_eq!(a.merge(&b), PlaceState::BorrowedExclusive("h".to_string()));
    }

    #[test]
    fn borrowed_exclusive_different_borrowers_merge_to_maybe_partial() {
        let a = PlaceState::BorrowedExclusive("h1".to_string());
        let b = PlaceState::BorrowedExclusive("h2".to_string());
        assert_eq!(a.merge(&b), PlaceState::MaybePartial);
    }

    #[test]
    fn borrowed_exclusive_vs_owned_merges_to_maybe_partial() {
        let a = PlaceState::BorrowedExclusive("h".to_string());
        let b = PlaceState::Owned;
        assert_eq!(a.merge(&b), PlaceState::MaybePartial);
        // Symmetric.
        assert_eq!(b.merge(&a), PlaceState::MaybePartial);
    }

    #[test]
    fn borrowed_exclusive_vs_shared_merges_to_maybe_partial() {
        let a = PlaceState::BorrowedExclusive("h".to_string());
        let b = PlaceState::BorrowedShared(2);
        assert_eq!(a.merge(&b), PlaceState::MaybePartial);
        assert_eq!(b.merge(&a), PlaceState::MaybePartial);
    }

    #[test]
    fn fmt_state_includes_borrowed_exclusive() {
        assert_eq!(
            fmt_state(&PlaceState::BorrowedExclusive("h".to_string())),
            "BorrowedExclusive(h)"
        );
    }

    // ---- 6BC.2 — cross-statement exclusive-borrow tracking ----

    #[test]
    fn e1_mut_call_records_exclusive_borrow_in_state() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  let m: i32 = cur.x;
  return;
}";
        let dump = analyze_src(src);
        // After `let cur = cursor(v);`, v should be BorrowedExclusive(cur).
        assert!(
            dump.contains("v=BorrowedExclusive(cur)"),
            "expected exclusive-borrow state on v; got:\n{dump}"
        );
    }

    #[test]
    fn e2_mut_method_call_records_exclusive_borrow_in_state() {
        let src = "\
struct B { x: i32 }
impl B {
  fn drop(ref this) { return; }
  fn cursor(ref this) -> B { return this; }
}
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = v.cursor();
  let m: i32 = cur.x;
  return;
}";
        let dump = analyze_src(src);
        assert!(
            dump.contains("v=BorrowedExclusive(cur)"),
            "expected exclusive-borrow state on v; got:\n{dump}"
        );
    }

    #[test]
    fn e0383_fires_on_read_of_exclusively_borrowed_place() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  let n: i32 = peek(v);
  let m: i32 = cur.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0383"),
            "expected E0383 in {codes:?}"
        );
    }

    #[test]
    fn e0383_does_not_fire_after_exclusive_borrower_scope_exits() {
        // The exclusive borrow is released when `cur` goes out of scope
        // (end of the `if` body); reading `v` after the `if` is fine.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let v: B = B { x: 1 };
  if true {
    let cur: B = cursor(v);
    return;
  }
  let n: i32 = peek(v);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0383"),
            "E0383 should not fire after the borrower's scope exits; got {codes:?}"
        );
    }

    // ---- 2026-08-13 — NLL borrow ends: a borrow ends after the statement
    // (of its declaring block) containing its last mention, not at scope
    // exit. Relaxations are positive tests; the pins (loop / defer / block
    // tail) keep the borrow live where a release would be unsound.
    //
    // 2026-08-22 — DROP LIVENESS. The relaxation applies only to a borrower
    // with NO destructor. A destructor is a use that happens after every
    // textual mention, so releasing such a borrower at its last mention is
    // unsound, and the three tests below used to assert exactly that: each
    // borrows a `struct B` that declares `fn drop`, then moves or writes the
    // owner while `B::drop` is still owed a run. `thread::Scope` is the case
    // that made it matter — its `drop` JOINS the workers it lent to, so
    // between the lend and the scope's end a worker is writing the lent
    // place, and the parent could read, write and re-lend it freely because
    // the scope was not mentioned again.
    //
    // `nll_view_borrower_without_a_destructor_still_relaxes` is the case the
    // relaxation was built for and it is untouched: a `str` view of a `Text`
    // owns nothing and drops nothing. ----

    #[test]
    fn nll_move_under_a_borrower_with_a_destructor_is_rejected() {
        // `r` borrows `x` and declares `fn drop`. Its destructor runs at the
        // end of `caller`, AFTER the move — so `B::drop(r)` would run against
        // storage `drain` has taken. The last textual mention of `r` is not
        // its last use.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let x: B = B { x: 1 };
  let r: B = passthrough(x);
  let n: i32 = r.x;
  drain(x);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "`r`'s destructor still has to run — the move must be refused; got {codes:?}"
        );
    }

    #[test]
    fn nll_view_borrower_without_a_destructor_still_relaxes() {
        // The case the relaxation exists for, and the one that must keep
        // working: a `str` view owns nothing and drops nothing, so once it
        // is dead the owner is free again. If drop-liveness ever widened to
        // "any non-Copy borrower", this test is what would catch it.
        let src = "\
struct Owner { n: i32 }
impl Owner { fn drop(ref this) { return; } }
fn view(o: Owner) -> str { return \"x\"; }
fn caller() {
  var o: Owner = Owner { n: 1 };
  let v: str = view(o);
  let w: str = v;
  o.n = 5;
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0381" || c == "E0383" || c == "E0372"),
            "a dead view borrower must release its owner; got {codes:?}"
        );
    }

    #[test]
    fn nll_write_under_a_scope_that_has_not_joined_is_rejected() {
        // The bug report's shape, and the sharpest instance of the rule:
        // the scope is never mentioned after the lend, so under last-use
        // release its loan on `d` was already gone and this write compiled.
        // `Scope::drop` is what joins the worker — until it has run, the
        // worker is still writing `d`.
        let codes = check_src(&format!(
            "{SCOPE_MOCK}\
             fn main() -> i32 {{\n                 var s: Scope = Scope {{ c: 0 }};\n                 var d: Data = Data {{ n: 1 }};\n                 s.lend(d);\n                 d.n = 5;\n                 return 0;\n             }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "the loan ends where the scope does, not at its last mention; got {codes:?}"
        );
    }

    #[test]
    fn a_never_used_borrower_with_a_destructor_still_holds_its_claim() {
        // "Never mentioned again" is not "never used": `cur` has a
        // destructor, so its claim on `v` is observable at exactly one point
        // no source line names.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  drain(v);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372" || c == "E0383"),
            "a borrower with a destructor is never dead early; got {codes:?}"
        );
    }

    #[test]
    fn nll_use_inside_loop_pins_borrow_past_the_loop() {
        // `r` is read inside the loop body; the loop's back edge re-reads
        // it, so its borrow is pinned past the whole loop statement — the
        // write to `x` inside the loop must still be rejected.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn caller() {
  var x: B = B { x: 1 };
  let r: B = passthrough(x);
  var i: i32 = 0;
  while i < 3 {
    let n: i32 = r.x;
    x.x = 5;
    i = i + 1;
  }
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0381"),
            "a loop-body use pins the borrow across the back edge; got {codes:?}"
        );
    }

    #[test]
    fn nll_defer_mention_pins_borrow_to_scope_exit() {
        // `defer` runs at scope exit, so a deferred read of the borrower
        // keeps its borrow live for the whole scope regardless of position.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let x: B = B { x: 1 };
  let r: B = passthrough(x);
  defer peek(r);
  drain(x);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "a defer mention pins the borrow to scope exit; got {codes:?}"
        );
    }

    #[test]
    fn nll_block_tail_mention_pins_borrow() {
        // The borrower is read in the block's TAIL expression, which
        // evaluates after every statement — the mid-block move must still
        // be rejected.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn passthrough(b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let x: B = B { x: 1 };
  let m: i32 = {
    let r: B = passthrough(x);
    drain(x);
    r.x
  };
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "a tail mention pins the borrow past every statement; got {codes:?}"
        );
    }

    #[test]
    fn e0383_does_not_fire_on_borrower_itself() {
        // The binding being read may legitimately BE the borrower —
        // record_read skips the self-conflict case. (Reading `cur` is
        // fine: it owns the borrow that points at `v`.)
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  let n: i32 = peek(cur);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0383"),
            "E0383 should not fire when reading the borrower itself; got {codes:?}"
        );
    }

    #[test]
    fn e0372_message_refined_when_borrow_is_exclusive() {
        // Move-while-exclusively-borrowed → E0372 with the refined
        // "exclusively borrowed" wording. Phase 5's shared-borrow text
        // is the wrong story here.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn drain(take b: B) { return; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  drain(v);
  let m: i32 = cur.x;
  return;
}";
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, &PathBuf::from("t.cplus"), src);
        let e0372 = diags
            .iter()
            .find(|d| d.code.0 == "E0372")
            .expect("expected E0372");
        assert!(
            e0372.message.contains("exclusively borrowed"),
            "E0372 message should say 'exclusively borrowed'; got: {}",
            e0372.message
        );
        // E0383 must NOT also fire for the same conflict — the move-arg
        // path suppresses it to avoid cascading errors.
        let e0383_count = diags.iter().filter(|d| d.code.0 == "E0383").count();
        assert_eq!(
            e0383_count,
            0,
            "E0383 should be suppressed for move-while-exclusive; got {} diagnostics",
            diags.len()
        );
    }

    #[test]
    fn exclusive_borrow_does_not_fire_on_copy_param() {
        // `ref x: i32` is local-mutability for Copy, not a borrow. The
        // E1-mut detector must require non-Copy.
        let src = "\
fn handle(ref x: i32) -> i32 { return x; }
fn caller() {
  let v: i32 = 1;
  let r: i32 = handle(v);
  let m: i32 = v;
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0383"),
            "E0383 should not fire on Copy params; got {codes:?}"
        );
    }

    #[test]
    fn e1_mut_classification_with_flavor() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn through(ref b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "through"),
            Some((ReturnBorrowSource::Param(0), BorrowFlavor::Exclusive))
        );
    }

    #[test]
    fn e1_shared_classification_with_flavor() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn through(b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "through"),
            Some((ReturnBorrowSource::Param(0), BorrowFlavor::Shared))
        );
    }

    #[test]
    fn e1_mut_does_not_fire_on_move_marker() {
        // `take x: B` is ownership transfer, not an exclusive borrow.
        // No elision rule applies.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source_with_flavor(&prog, "drain"), None);
    }

    // ---- 6BC.3 — partial-place activation ----

    #[test]
    fn place_overlap_same() {
        let a = Place::root("buf");
        let b = Place::root("buf");
        assert_eq!(a.overlap(&b), PlaceOverlap::Same);
    }

    #[test]
    fn place_overlap_disjoint_roots() {
        let a = Place::root("buf");
        let b = Place::root("ctx");
        assert_eq!(a.overlap(&b), PlaceOverlap::Disjoint);
    }

    #[test]
    fn place_overlap_disjoint_sub_places() {
        let mut a = Place::root("buf");
        a.projections.push(Projection::Field("left".to_string()));
        let mut b = Place::root("buf");
        b.projections.push(Projection::Field("right".to_string()));
        assert_eq!(a.overlap(&b), PlaceOverlap::Disjoint);
    }

    #[test]
    fn place_overlap_parent_contains_child() {
        let parent = Place::root("buf");
        let mut child = Place::root("buf");
        child
            .projections
            .push(Projection::Field("left".to_string()));
        assert_eq!(parent.overlap(&child), PlaceOverlap::Contains);
        assert_eq!(child.overlap(&parent), PlaceOverlap::Contained);
    }

    #[test]
    fn place_overlap_index_const_distinct() {
        let mut a = Place::root("arr");
        a.projections.push(Projection::Index(3));
        let mut b = Place::root("arr");
        b.projections.push(Projection::Index(7));
        assert_eq!(a.overlap(&b), PlaceOverlap::Disjoint);
    }

    #[test]
    fn place_overlap_index_any_overlaps_const() {
        // `arr[*]` (non-constant index) is conservatively treated as
        // a distinct projection from `arr[3]`. Per design note §5.1
        // we coarsen non-constant to AnyIndex; same-root different
        // projection list means Disjoint until index-aliasing is
        // proven (future work).
        let mut a = Place::root("arr");
        a.projections.push(Projection::AnyIndex);
        let mut b = Place::root("arr");
        b.projections.push(Projection::Index(3));
        // The current rule treats these as Disjoint because the
        // projection lists differ. A precision improvement (treat
        // AnyIndex as overlapping with every Index) is recorded in
        // design note §9; not Phase-6 territory.
        assert_eq!(a.overlap(&b), PlaceOverlap::Disjoint);
    }

    #[test]
    fn partial_place_admit_disjoint_subfields_in_one_call() {
        // The headline 6BC.3 win: `ref buf.left` + `ref buf.right`
        // claim disjoint sub-places and admit.
        let src = "\
struct Inner { v: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn modify_both(ref a: Inner, ref b: Inner) { return; }
fn caller() {
  let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
  modify_both(p.left, p.right);
  return;
}";
        let codes = check_src(src);
        let conflict_codes: Vec<&String> = codes
            .iter()
            .filter(|c| {
                ["E0370", "E0374", "E0380", "E0381", "E0382", "E0383"].contains(&c.as_str())
            })
            .collect();
        assert!(
            conflict_codes.is_empty(),
            "disjoint sub-places should admit; got: {codes:?}"
        );
    }

    #[test]
    fn e0374_partial_overlap_parent_with_subfield_in_one_call() {
        // `ref buf` + a sibling reading `buf.left` overlap (parent
        // contains sub-place). Fires E0374 not E0381.
        let src = "\
struct Inner { v: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn write_pair(ref a: Pair, b: Inner) { return; }
fn caller() {
  let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
  write_pair(p, p.left);
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0374"),
            "expected E0374 in {codes:?}"
        );
    }

    #[test]
    fn e0374_cross_statement_read_of_parent_while_subfield_borrowed() {
        // Cross-statement partial-place: `let cur = cursor(p.left);`
        // makes `p.left` exclusively borrowed. Reading the parent
        // `p` past that point fires E0374.
        let src = "\
struct Inner { v: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn cursor(ref i: Inner) -> Inner { return i; }
fn peek_pair(p: Pair) -> i32 { return 0; }
fn caller() {
  let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
  let cur: Inner = cursor(p.left);
  let n: i32 = peek_pair(p);
  let m: i32 = cur.v;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0374"),
            "expected E0374 cross-statement in {codes:?}"
        );
    }

    #[test]
    fn e0374_does_not_fire_on_disjoint_subfield_cross_statement() {
        // Cross-statement, disjoint sub-places: borrowing `p.left`
        // doesn't block reading `p.right`.
        let src = "\
struct Inner { v: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn cursor(ref i: Inner) -> Inner { return i; }
fn peek(i: Inner) -> i32 { return i.v; }
fn caller() {
  let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
  let cur: Inner = cursor(p.left);
  let n: i32 = peek(p.right);
  return;
}";
        let codes = check_src(src);
        let conflict_codes: Vec<&String> = codes
            .iter()
            .filter(|c| ["E0374", "E0383"].contains(&c.as_str()))
            .collect();
        assert!(
            conflict_codes.is_empty(),
            "disjoint sub-places should admit cross-statement; got: {codes:?}"
        );
    }

    #[test]
    fn place_from_expr_walks_field_chain() {
        // `p.left.v` parses as Field(Field(Ident "p", "left"), "v").
        // The walker should produce a Place with two projections.
        let toks =
            tokenize("fn f() { let p: Inner = Inner { v: 1 }; let n: i32 = p.left.v; return; }")
                .expect("lex");
        let prog = parse(toks).expect("parse");
        // Drill into the second let-init to grab the expression.
        let ItemKind::Function(ref f) = prog.items[0].kind else {
            panic!()
        };
        let StmtKind::Let { init: Some(e), .. } = &f.body.stmts[1].kind else {
            panic!()
        };
        let place = place_from_expr(e).expect("place built");
        assert_eq!(place.root, "p");
        assert_eq!(place.projections.len(), 2);
        assert_eq!(place.canonical(), "p.left.v");
    }

    // ---- 6BC.4 — Rule E3-mut + E0384 ----

    #[test]
    fn e3_mut_fires_on_multi_mut_param_with_param_rooted_returns() {
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest_mut(ref a: B, ref b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}";
        let prog = parse_prog(src);
        assert_eq!(
            return_borrow_source_with_flavor(&prog, "longest_mut"),
            Some((
                ReturnBorrowSource::MultiParam(vec![0, 1]),
                BorrowFlavor::Exclusive
            ))
        );
    }

    #[test]
    fn e3_mut_does_not_fire_with_mixed_shared_and_mut_params() {
        // E3-mut requires *every* param to be `ref`. A function
        // mixing `a: B` (shared) and `ref b: B` (exclusive) doesn't
        // qualify for either E3 or E3-mut.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn mixed(a: B, ref b: B) -> B { return b; }";
        let prog = parse_prog(src);
        assert_eq!(return_borrow_source_with_flavor(&prog, "mixed"), None);
    }

    // ---- 6BC.5 — explicit region annotations (the `borrow REGION T` source syntax is retired) ----

    #[test]
    fn e3_mut_multi_source_borrow_at_call_site() {
        // Calling an E3-mut function records the result as borrowing
        // from every parameter in the result's MultiParam set. Moving
        // any parameter while the result is alive fires E0372.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest_mut(ref a: B, ref b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn drain(take b: B) { return; }
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  let r: B = longest_mut(a, b);
  drain(a);
  let n: i32 = r.x;
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "expected E0372 in {codes:?}"
        );
    }

    #[test]
    fn e0383_releases_when_exclusive_borrower_is_moved() {
        // Moving the exclusive borrower releases the borrow on its source.
        let src = "\
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn drain(take c: B) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  let v: B = B { x: 1 };
  let cur: B = cursor(v);
  drain(cur);
  let n: i32 = peek(v);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0383"),
            "E0383 should not fire after moving the exclusive borrower; got {codes:?}"
        );
    }

    // ---- 2026-07-07 — view escape via aggregate capture (CRITICAL UAF) ----
    //
    // A view (`str`) produced inside an aggregate literal escapes into the
    // aggregate binding; the owner must stay move-blocked while the aggregate
    // lives, exactly as with a direct `let s: str = t.view();`. Before the
    // fix `classify_borrow_source` only recognized direct call initializers,
    // so every aggregate shape below was a safe-code use-after-free.

    const VIEW_PRELUDE: &str = "\
struct Buf { opaque p: *u8 }
impl Buf {
  fn new() -> Buf { return Buf { p: { 0 as *u8 } }; }
  fn view(this) -> str { return \"x\"; }
  fn drop(ref this) { return; }
}
struct Slot { s: str }
struct Nest { inner: Slot }
struct Holder { b: Buf }
impl Holder { fn drop(ref this) { return; } }
enum Wrap { V(str), N }
fn consume(take t: Buf) -> i32 { return 0; }
fn consume_h(take h: Holder) -> i32 { return 0; }
";

    #[test]
    fn view_captured_into_aggregate_blocks_owner_move() {
        // Every aggregate shape that can hold a view must record the borrow.
        let shapes: &[(&str, &str, &str)] = &[
            ("struct_lit", "let w: Slot = Slot { s: t.view() };", "let u: Slot = w;"),
            ("nested_struct_lit", "let w: Nest = Nest { inner: Slot { s: t.view() } };", "let u: Nest = w;"),
            ("array_lit", "let w: [str; 1] = [t.view()];", "let u: [str; 1] = w;"),
            ("tuple_lit", "let w: (str, i32) = (t.view(), 1);", "let u: (str, i32) = w;"),
            ("enum_payload", "let w: Wrap = Wrap::V(t.view());", "let u: Wrap = w;"),
        ];
        for (sname, capture, later_use) in shapes {
            // The trailing use of `w` keeps the aggregate's borrow live at the
            // move site (NLL releases at last use).
            let src = format!(
                "{VIEW_PRELUDE}fn f_{sname}() {{ let t: Buf = Buf::new(); {capture} let _c: i32 = consume(t); {later_use} return; }}"
            );
            let codes = check_src(&src);
            assert!(
                codes.iter().any(|c| c == "E0372"),
                "[view-escape {sname}] expected E0372 (move while view captured in aggregate), got {codes:?}"
            );
        }
    }

    #[test]
    fn view_method_on_field_path_receiver_blocks_owner_move() {
        // `h.b.view()` — the receiver is a field path, not a bare ident.
        // The borrow lands on the sub-place `h.b`, so moving `h` conflicts
        // via the partial-place overlap rules.
        let src = format!(
            "{VIEW_PRELUDE}fn f() {{ let h: Holder = Holder {{ b: Buf::new() }}; \
             let w: str = h.b.view(); let _c: i32 = consume_h(h); let u: str = w; return; }}"
        );
        let codes = check_src(&src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "[view-escape field-path] expected E0372, got {codes:?}"
        );
    }

    #[test]
    fn owner_move_sites_blocked_while_view_live() {
        // Move-site symmetry: with a live view of `t`, EVERY consume site
        // must reject the owner's move — not just call move-args. The bare
        // `let` re-bind and aggregate captures were unchecked before the fix.
        let sites: &[(&str, &str)] = &[
            ("take_call", "let _c: i32 = consume(t);"),
            ("let_rebind", "let t2: Buf = t;"),
            ("struct_field", "let h2: Holder = Holder { b: t };"),
            ("array_elem", "let a2: [Buf; 1] = [t];"),
            ("tuple_elem", "let p2: (Buf, i32) = (t, 1);"),
        ];
        for (sname, mv) in sites {
            // `let u: str = w;` after the move keeps the view live at it.
            let src = format!(
                "{VIEW_PRELUDE}fn f_{sname}() {{ let t: Buf = Buf::new(); \
                 let w: str = t.view(); {mv} let u: str = w; return; }}"
            );
            let codes = check_src(&src);
            assert!(
                codes.iter().any(|c| c == "E0372"),
                "[move-site {sname}] expected E0372 (move while view live), got {codes:?}"
            );
        }
    }

    #[test]
    fn aggregate_without_view_capture_stays_clean() {
        // Negative controls: aggregates that own their contents outright
        // must not invent borrows, and moving the owner after the view
        // borrower has itself been consumed is legal.
        let clean: &[(&str, &str)] = &[
            // aggregate of owned values — no view, no borrow
            (
                "owned_capture",
                "let t: Buf = Buf::new(); let h: Holder = Holder { b: t }; return;",
            ),
            // a view of `t` must not block moving an unrelated owner `u`
            (
                "unrelated_owner",
                "let t: Buf = Buf::new(); let u: Buf = Buf::new(); \
                 let w: str = t.view(); let _c: i32 = consume(u); return;",
            ),
        ];
        for (cname, body) in clean {
            let src = format!("{VIEW_PRELUDE}fn f_{cname}() {{ {body} }}");
            let codes = check_src(&src);
            assert!(
                !codes.iter().any(|c| c == "E0372"),
                "[clean {cname}] unexpected E0372: {codes:?}"
            );
        }
    }

    // -- Memory-model contract §3.1: view escapes at the definition ------
    // (issue-07 — ported from sema, which used to emit these while
    // hand-encoding where it believed this pass would tie instead.)

    /// A `#[lang("string")]` owner, so the `Text`→`str` coercion routes are
    /// reachable without the stdlib. `opaque` exempts the pointer from
    /// raw-pointer drop accounting; the `drop` makes it non-Copy, which is
    /// what makes a view of a LOCAL one dangle.
    const LANG_STR_PRELUDE: &str = "\
#[lang(\"string\")]
struct LStr { opaque ptr: *u8, len: usize, cap: usize }
impl LStr { fn drop(ref this) { return; } }
fn mk() -> LStr { return LStr { ptr: { 0 as *u8 }, len: { 0 as usize }, cap: { 0 as usize } }; }
";

    #[test]
    fn returned_view_of_local_owner_denied_e0513() {
        // The root gate is `owns_value`, not "is it a parameter": a local, a
        // `take` parameter and a `take this` receiver all free their storage
        // on the way out, so a view of any of them dangles at the caller.
        let cases: &[(&str, &str)] = &[
            ("local", "fn bad() -> str { let b: Buf = Buf::new(); return b.view(); }"),
            (
                "alias",
                "fn bad() -> str { let b: Buf = Buf::new(); let s: str = b.view(); return s; }",
            ),
            ("take_param", "fn steal(take b: Buf) -> str { return b.view(); }"),
            (
                "take_this",
                "impl Buf { fn into_view(take this) -> str { return this.view(); } }",
            ),
            (
                "through_free_fn",
                "fn head(b: Buf) -> str { return b.view(); }\n\
                 fn bad() -> str { let b: Buf = Buf::new(); return head(b); }",
            ),
            (
                "carrying_aggregate",
                "fn keep() -> Slot { let b: Buf = Buf::new(); return Slot { s: b.view() }; }",
            ),
            (
                "branch_alias",
                "fn bad(flag: bool) -> str { let b: Buf = Buf::new(); \
                 var v: str; if flag { v = b.view(); } else { v = \"lit\"; } return v; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    #[test]
    fn returning_a_view_carrying_owner_is_a_move_not_an_escape() {
        // `Vec[str]` and `Box[T]` carry their element's views now, so the
        // return rule sees them for the first time. Handing the container
        // itself back is a MOVE — its storage becomes the caller's — and
        // must not read as a view of something this frame frees.
        let clean: &[(&str, &str)] = &[
            (
                "bare_move",
                "fn split(s: str) -> Vec[str] { var out: Vec[str] = mk_vec(); return out; }",
            ),
            (
                "moved_into_returned_aggregate",
                "struct Surface { rows: Vec[str] }\n\
                 fn open() -> Surface { var rows: Vec[str] = mk_vec(); return Surface { rows: rows }; }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!(
                "fn mk_vec() -> Vec[str] {{ return #zero::[Vec[str]](); }}\n{tail}"
            ));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] a move is not an escape, got {codes:?}"
            );
        }
    }

    #[test]
    fn returned_view_coerced_from_a_local_owner_denied_e0513() {
        // The coercion route has no accessor to key on: a lang-string local
        // returned where `str` is expected is the same dangle.
        let codes = check_src(&format!(
            "{LANG_STR_PRELUDE}fn bad() -> str {{ let s: LStr = mk(); return s; }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0513"),
            "expected E0513 on a returned local-string view, got {codes:?}"
        );
    }

    #[test]
    fn returned_view_of_caller_owned_storage_stays_clean() {
        // Controls. A bare / `ref` parameter and a `this` receiver name
        // storage the caller owns and outlives the call with, so a view of
        // them is caller-tied; a literal is static. None may be denied.
        let clean: &[(&str, &str)] = &[
            ("bare_param", "fn head(b: Buf) -> str { return b.view(); }"),
            ("this_receiver", "impl Buf { fn v2(this) -> str { return this.view(); } }"),
            ("literal", "fn ok() -> str { let s: str = \"lit\"; return s; }"),
            ("view_param", "fn first(s: str) -> str { return s; }"),
            (
                "param_rooted_aggregate",
                "fn wrap(b: Buf) -> Slot { return Slot { s: b.view() }; }",
            ),
            (
                "moved_owned_field",
                "fn wrap2(take b: Buf) -> Holder { return Holder { b: b }; }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn returned_view_of_param_lang_string_stays_clean() {
        let codes = check_src(&format!(
            "{LANG_STR_PRELUDE}fn bare(p: LStr) -> str {{ return p; }}"
        ));
        assert!(
            !codes.iter().any(|c| c == "E0513"),
            "a param-rooted coerced view must not be denied, got {codes:?}"
        );
    }

    #[test]
    fn view_leaf_inside_returned_aggregate_denied_e0513() {
        // The leaf half of §3.1: the dangle is named where it is built,
        // whatever the returned type turns out to be.
        let cases: &[(&str, &str)] = &[
            (
                "struct_field",
                "fn keep() -> Slot { let b: Buf = Buf::new(); return Slot { s: b.view() }; }",
            ),
            (
                "nested_struct",
                "fn keep2() -> Nest { let b: Buf = Buf::new(); \
                 return Nest { inner: Slot { s: b.view() } }; }",
            ),
            (
                "array_element",
                "fn keep3() -> [str; 1] { let b: Buf = Buf::new(); return [b.view()]; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    #[test]
    fn coerced_owner_leaf_in_returned_aggregate_denied_e0513() {
        // The coercion route has no accessor to key on: a lang-string local
        // stored where the field declares `str` is read AS a view.
        let codes = check_src(&format!(
            "{LANG_STR_PRELUDE}struct Holder {{ view: str }}\n\
             fn keep() -> Holder {{ let s: LStr = mk(); return Holder {{ view: s }}; }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0513"),
            "expected E0513 on a coerced view escaping in an aggregate, got {codes:?}"
        );
    }

    #[test]
    fn owned_leaf_moved_into_returned_aggregate_stays_clean() {
        // Ownership transfer is not a borrow. `Holder { b: b }` moves the
        // value in; nothing is left pointing at freed storage.
        let clean: &[(&str, &str)] = &[
            (
                "moved_owner",
                "fn wrap(take b: Buf) -> Holder { return Holder { b: b }; }",
            ),
            (
                "param_rooted_leaf",
                "fn wrap2(b: Buf) -> Slot { return Slot { s: b.view() }; }",
            ),
            (
                "literal_leaf",
                "fn wrap3() -> Slot { return Slot { s: \"lit\" }; }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn raw_view_store_without_a_declaration_denied_e0516() {
        // Contract §5: nothing can see through a raw-pointer store, so the
        // function has to say what it does with the bytes.
        let codes = check_src("fn stash(slot: *str, v: str) { *slot = v; return; }");
        assert!(
            codes.iter().any(|c| c == "E0516"),
            "expected E0516 on an undeclared raw view store, got {codes:?}"
        );
        let codes = check_src(
            "struct Data { key: str }\n\
             fn stash2(slot: *Data, v: Data) { *slot = v; return; }",
        );
        assert!(
            codes.iter().any(|c| c == "E0516"),
            "a carrier through a raw pointer is the same store, got {codes:?}"
        );
    }

    #[test]
    fn raw_store_through_a_projection_is_the_same_seam_e0516() {
        // bugs/str-field-outliving-its-text-is-not-caught.md: the first cut
        // matched a bare `*p =` target only, so writing the same view one
        // field deeper walked straight through. Every one of these is a
        // store the analysis cannot see the far side of.
        let cases: &[(&str, &str)] = &[
            (
                "field_of_pointee",
                "struct Sink { key: str }\n\
                 fn keep(into: *Sink, k: str) { { (*into).key = k }; return; }",
            ),
            (
                "nested_field",
                "struct Inner { key: str }\n\
                 struct Outer { inner: Inner }\n\
                 fn keep2(into: *Outer, k: str) { { (*into).inner.key = k }; return; }",
            ),
            (
                "element_of_pointee",
                "fn keep3(into: *[str; 4], k: str) { { (*into)[0] = k }; return; }",
            ),
            (
                "carrier_field",
                "struct Sink { key: str }\n\
                 struct Holder { s: Sink }\n\
                 fn keep4(into: *Holder, v: Sink) { { (*into).s = v }; return; }",
            ),
        ];
        for (name, src) in cases {
            let codes = check_src(src);
            assert!(
                codes.iter().any(|c| c == "E0516"),
                "[{name}] a projected raw store must be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn projected_raw_store_respects_the_same_gates() {
        // The widened shape must not widen the RULE: the declaration still
        // answers it, a non-view field is still not a view store, and a
        // field of a plain local is not a raw store at all.
        let clean: &[(&str, &str)] = &[
            (
                "declared_keeps_nothing",
                "struct Sink { key: str }\n\
                 #[keeps(nothing)]\n\
                 fn keep(into: *Sink, k: str) { { (*into).key = k }; return; }",
            ),
            (
                "non_view_field",
                "struct Counter { n: i32 }\n\
                 fn bump(into: *Counter, n: i32) { { (*into).n = n }; return; }",
            ),
            (
                "field_of_a_local",
                "struct Sink { key: str }\n\
                 fn keep2(k: str) { var s: Sink = Sink { key: \"\" }; s.key = k; return; }",
            ),
        ];
        for (name, src) in clean {
            let codes = check_src(src);
            assert!(
                !codes.iter().any(|c| c == "E0516"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn declared_or_non_view_raw_store_stays_clean() {
        let clean: &[(&str, &str)] = &[
            (
                "keeps_nothing",
                "#[keeps(nothing)]\nfn stash(slot: *str, v: str) { *slot = v; return; }",
            ),
            (
                "keeps_this",
                "struct S { n: i32 }\n\
                 impl S { #[keeps(this)] fn put(this, slot: *str, v: str) { *slot = v; return; } }",
            ),
            ("bytes", "fn poke(p: *u8, b: u8) { *p = b; return; }"),
            (
                "pointer_store",
                "fn relink(pp: **u8, p: *u8) { *pp = p; return; }",
            ),
        ];
        for (name, src) in clean {
            let codes = check_src(src);
            assert!(
                !codes.iter().any(|c| c == "E0516"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_of_a_local_stored_into_an_outliving_place_denied_e0513() {
        // A `static` and a `ref` target both outlive the frame, and the
        // view's owner does not.
        let cases: &[(&str, &str)] = &[
            (
                "static_view",
                "static S: str = \"\";\n\
                 fn bad() { let b: Buf = Buf::new(); S = b.view(); return; }",
            ),
            (
                "static_field",
                "static W: Slot = Slot { s: \"\" };\n\
                 fn bad() { let b: Buf = Buf::new(); W.s = b.view(); return; }",
            ),
            (
                "ref_out_param",
                "fn stash(ref w: Slot) { let b: Buf = Buf::new(); w.s = b.view(); return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_param_stored_where_nothing_ties_denied_e0515() {
        // The three places contract §3 says the deny survives: a `static`
        // (no owner to tie), a fn whose ADDRESS is taken (indirect calls
        // carry no flows), and a store the flow pass does not analyze (a
        // method with its own generic params).
        let cases: &[(&str, &str)] = &[
            (
                "static_sink",
                "static KEY: str = \"\";\nfn stash(k: str) { KEY = k; return; }",
            ),
            (
                "address_taken",
                "struct Holder { view: str }\n\
                 fn put(ref h: Holder, k: str) { h.view = k; return; }\n\
                 fn consume(f: fn(ref Holder, str)) { return; }\n\
                 fn main() -> i32 { consume(put); return 0; }",
            ),
            (
                "generic_method_receiver_store",
                "struct Holder { view: str }\n\
                 impl Holder { fn set[U](ref this, k: str, u: U) { this.view = k; return; } }",
            ),
        ];
        for (name, src) in cases {
            let codes = check_src(src);
            assert!(
                codes.iter().any(|c| c == "E0515"),
                "[{name}] expected E0515, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_param_store_the_flow_pass_exports_stays_clean() {
        // Where the flow pass publishes the store, call sites tie and the
        // definition needs no deny — declared `#[keeps(this)]`, a computed
        // concrete receiver store, a computed generic-impl receiver store,
        // and a computed free-fn ref-param flow.
        let clean: &[(&str, &str)] = &[
            (
                "keeps_this",
                "struct Holder { view: str }\n\
                 impl Holder { #[keeps(this)] fn set(ref this, k: str) { this.view = k; return; } }",
            ),
            (
                "computed_concrete_setter",
                "struct Holder { view: str }\n\
                 impl Holder { fn set(ref this, k: str) { this.view = k; return; } }",
            ),
            (
                "computed_generic_setter",
                "struct GenHolder[T] { opaque p: *u8, view: str }\n\
                 impl GenHolder[T] { fn gset(ref this, k: str) { this.view = k; return; } }",
            ),
            (
                "computed_free_fn_ref_flow",
                "struct Holder { view: str }\n\
                 fn put(ref h: Holder, k: str) { h.view = k; return; }",
            ),
        ];
        for (name, src) in clean {
            let codes = check_src(src);
            assert!(
                !codes.iter().any(|c| c == "E0515"),
                "[{name}] an exported store must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_of_an_unnamed_temporary_denied_e0513() {
        // Nothing names the receiver, so the statement drops it and the
        // view is dead before it is read. All three binding positions.
        let cases: &[(&str, &str)] = &[
            ("let", "fn f() { let s: str = Buf::new().view(); return; }"),
            ("return", "fn f() -> str { return Buf::new().view(); }"),
            (
                "assign",
                "fn f() { var s: str = \"\"; s = Buf::new().view(); return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    /// `VIEW_PRELUDE` plus a `str→str` method, so a view can be handed one
    /// link further along the way `str::trim` does in the stdlib.
    const VIEW_CHAIN_PRELUDE: &str = "\
impl str { fn narrow(this) -> str { return this; } }
";

    #[test]
    fn view_of_a_temporary_laundered_through_a_str_method_denied_e0513() {
        // The hole this closes: the rule read the receiver's type, saw the
        // `str` a previous link returned, and stopped — Copy, "nothing to
        // dangle". True of the fat pointer, false of the bytes under it.
        // `Buf::new().view().narrow()` compiled clean and read freed memory.
        let cases: &[(&str, &str)] = &[
            (
                "let_one_link",
                "fn f() { let s: str = Buf::new().view().narrow(); return; }",
            ),
            (
                "let_two_links",
                "fn f() { let s: str = Buf::new().view().narrow().narrow(); return; }",
            ),
            (
                "return",
                "fn f() -> str { return Buf::new().view().narrow(); }",
            ),
            (
                "assign",
                "fn f() { var s: str = \"\"; s = Buf::new().view().narrow(); return; }",
            ),
            (
                "field_store",
                "fn f() { var sl: Slot = Slot { s: \"\" }; sl.s = Buf::new().view().narrow(); return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{VIEW_CHAIN_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_chain_rooted_in_a_named_owner_stays_clean() {
        // The recursion must stop at a named place: `b` outlives the
        // statement, so every link off it is somebody's binding to judge,
        // not a temporary. A literal and a parameter root the same way.
        let clean: &[(&str, &str)] = &[
            (
                "named_owner",
                "fn f() { let b: Buf = Buf::new(); let s: str = b.view().narrow(); return; }",
            ),
            (
                "named_owner_deep",
                "fn f() { let b: Buf = Buf::new(); let s: str = b.view().narrow().narrow(); return; }",
            ),
            (
                "literal_root",
                "fn f() { let s: str = \"lit\".narrow(); return; }",
            ),
            (
                "param_root",
                "fn f(p: str) { let s: str = p.narrow().narrow(); return; }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{VIEW_PRELUDE}{VIEW_CHAIN_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn a_method_that_copies_its_view_argument_keeps_nothing() {
        // Regression, found by running iris: `to_text()` on a `str` ALLOCATES
        // a copy, so a method storing the result into `ref this` keeps no
        // view of its argument and its callers tie nothing.
        //
        // Resolving `str.to_text` by falling through to the lang-string
        // struct's own `to_text` broke exactly this — `LStr::to_text` is
        // `this.clone()`, which the flow pass reads as returning
        // receiver-rooted data, so the copy looked like a borrow. It fired
        // E0514 on four correct iris call sites, one of them in a sibling
        // `else if` where the named owner was not in scope at all.
        let src = format!(
            "{LANG_STR_PRELUDE}\
             impl LStr {{ fn to_text(this) -> LStr {{ return mk(); }} }}\n\
             struct Row {{ body: LStr }}\n\
             impl Row {{ fn show(ref this, body: str) {{ this.body = body.to_text(); return; }} }}\n\
             struct Chat {{ row: Row }}\n\
             impl Chat {{\n\
               fn aim(ref this, k: i32) {{\n\
                 if k == 1 {{\n\
                   let owner: LStr = mk();\n\
                   let v: str = owner.view();\n\
                   this.row.show(v);\n\
                 }} else {{\n\
                   this.row.show(\"lit\");\n\
                 }}\n\
                 return;\n\
               }}\n\
             }}\n\
             impl LStr {{ fn view(this) -> str {{ return \"x\"; }} }}"
        );
        let codes = check_src(&src);
        assert!(
            !codes.iter().any(|c| c == "E0514"),
            "a copying method must not tie its caller's receiver, got {codes:?}"
        );
    }

    #[test]
    fn str_receiver_resolves_lang_string_methods_for_the_temporary_rule() {
        // `q.to_text()` on a `str` promotes the receiver, so the method lives
        // on the lang-string struct. The sig table used to map only
        // lang-string→`str`; without the reverse the call could not be TYPED
        // and every receiver-keyed rule went quiet — which is how
        // `q.to_text().trim()` bound a view of a Text temporary in silence.
        let src = format!(
            "{LANG_STR_PRELUDE}\
             impl LStr {{ fn view(this) -> str {{ return \"x\"; }} }}\n\
             impl str {{ fn to_lstr(this) -> LStr {{ return mk(); }} }}\n\
             fn f(q: str) {{ let s: str = q.to_lstr().view(); return; }}"
        );
        let codes = check_src(&src);
        assert!(
            codes.iter().any(|c| c == "E0513"),
            "expected E0513 for a view of a promoted-receiver temporary, got {codes:?}"
        );
    }

    /// `LANG_STR_PRELUDE` plus the pieces a capture needs: a view accessor on
    /// the owner, a `str`-field aggregate, an owned-field aggregate (the
    /// control an over-eager rule would break), and a `str` sink.
    const COERCE_PRELUDE: &str = "\
impl LStr { fn view(this) -> str { return \"x\"; } }
struct Slot { s: str, n: i32 }
struct Own { o: LStr }
impl Own { fn drop(ref this) { return; } }
fn peek(x: str) -> i32 { return 0; }
";

    #[test]
    fn view_bound_from_an_rvalue_owner_denied_e0513() {
        // bugs/rvalue-text-coercion-binding-leak.md: the owner→view coercion
        // of a TEMPORARY spills the owner to an anonymous slot and keeps its
        // `{ptr,len}` prefix. Codegen never frees that slot — which is what
        // kept the shape sound, and is exactly the leak. The binding is the
        // wrong part, so it is the part that is rejected.
        let cases: &[(&str, &str)] = &[
            ("let_coercion", "fn f() { let s: str = mk(); return; }"),
            (
                "assign_coercion",
                "fn f() { var s: str = \"\"; s = mk(); return; }",
            ),
            (
                "let_interpolation",
                "fn f() { let n: i32 = 1; let s: str = \"x ${n}\"; return; }",
            ),
            (
                "assign_interpolation",
                "fn f() { let n: i32 = 1; var s: str = \"\"; s = \"x ${n}\"; return; }",
            ),
            // Captures: the binding outlives the statement one aggregate deep.
            (
                "struct_field_coercion",
                "fn f() { let w: Slot = Slot { s: mk(), n: 1 }; return; }",
            ),
            (
                "struct_field_view_of_temp",
                "fn f() { let w: Slot = Slot { s: mk().view(), n: 1 }; return; }",
            ),
            (
                "tuple_element",
                "fn f() { let w: (str, i32) = (mk(), 1); return; }",
            ),
            ("array_element", "fn f() { let w: [str; 1] = [mk()]; return; }"),
            (
                "assign_into_carrier",
                "fn f() { var w: Slot = Slot { s: \"\", n: 0 }; w = Slot { s: mk(), n: 1 }; return; }",
            ),
            (
                "destructure",
                "fn f() { let Slot { s, n } = Slot { s: mk().view(), n: 1 }; return; }",
            ),
            // The block spelling every coercion site is written in.
            (
                "braced",
                "fn f() { let s: str = { mk() }; return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{LANG_STR_PRELUDE}{COERCE_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    #[test]
    fn coercion_from_a_named_owner_or_at_an_argument_stays_clean() {
        // The controls that decide whether the rule is usable. A named owner
        // outlives the statement (whether it outlives the VIEW is
        // `owns_value`'s question); a temporary at an argument position
        // outlives the call it is an argument to; and moving an owned value
        // into an owned field is an ownership transfer, not a view.
        let clean: &[(&str, &str)] = &[
            (
                "named_owner_coercion",
                "fn f() { let o: LStr = mk(); let s: str = o; return; }",
            ),
            (
                "named_owner_capture",
                "fn f() { let o: LStr = mk(); let w: Slot = Slot { s: o.view(), n: 1 }; return; }",
            ),
            (
                "named_owner_field_capture",
                "fn f() { let h: Own = Own { o: mk() }; let w: Slot = Slot { s: h.o.view(), n: 1 }; return; }",
            ),
            ("literal", "fn f() { let s: str = \"lit\"; return; }"),
            (
                "literal_capture",
                "fn f() { let w: Slot = Slot { s: \"lit\", n: 1 }; return; }",
            ),
            (
                "argument_coercion",
                "fn f() -> i32 { return peek(mk()); }",
            ),
            (
                "argument_interpolation",
                "fn f() -> i32 { let n: i32 = 1; return peek(\"x ${n}\"); }",
            ),
            // The one an over-eager rule breaks: an owned value moved into an
            // owned field is not a view and must never be flagged.
            (
                "owned_move_into_owned_field",
                "fn f() { let w: Own = Own { o: mk() }; return; }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{LANG_STR_PRELUDE}{COERCE_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn view_of_a_named_owner_is_not_the_temporary_rule() {
        // A named receiver is somebody's binding; whether it dangles is the
        // `owns_value` question, not this one. As a direct call argument the
        // temporary is still alive, so that stays legal too.
        let clean: &[(&str, &str)] = &[
            (
                "named_owner",
                "fn peek(x: str) -> i32 { return 0; }\n\
                 fn f() -> i32 { let b: Buf = Buf::new(); let s: str = b.view(); return peek(s); }",
            ),
            (
                "temp_as_argument",
                "fn peek(x: str) -> i32 { return 0; }\n\
                 fn f() -> i32 { return peek(Buf::new().view()); }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0513"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    // ── STRM v3 (2026-08-01): view-carrying aggregates inherit borrows ──
    // (the str_dangle_repro family: a `str` field must not launder a view of
    // dying storage past the frame). Moved here from sema with the rules
    // themselves — issue-07.

    #[test]
    fn view_carrying_struct_literal_to_static_e0513() {
        // Owner is a Drop struct with a view accessor; storing a struct
        // literal that holds its view into a static must reject.
        let codes = check_src(
            "struct Buf { p: *u8 }\n\
             impl Buf { fn drop(ref this) { return; } fn view(this) -> str { return \"x\"; } }\n\
             struct Data { key: str, n: i32 }\n\
             static SLOT: Data = #zero::[Data]();\n\
             fn build() {\n\
                 let owner: Buf = Buf { p: 0 as *u8 };\n\
                 SLOT = Data { key: owner.view(), n: 1 };\n\
                 return;\n\
             }\n\
             fn main() -> i32 { build(); return 0; }",
        );
        assert!(codes.iter().any(|c| c == "E0513"), "static store of view-carrying literal; got {codes:?}");
    }

    #[test]
    fn view_carrying_call_result_to_static_e0513() {
        // The original repro shape: the view is laundered through a fn
        // param into a returned struct, then stored into a static.
        let codes = check_src(
            "struct Buf { p: *u8 }\n\
             impl Buf { fn drop(ref this) { return; } fn view(this) -> str { return \"x\"; } }\n\
             struct Data { key: str, n: i32 }\n\
             static SLOT: Data = #zero::[Data]();\n\
             fn store(key: str, n: i32) -> Data { return Data { key: key, n: n }; }\n\
             fn build() {\n\
                 let owner: Buf = Buf { p: 0 as *u8 };\n\
                 SLOT = store(owner.view(), 2);\n\
                 return;\n\
             }\n\
             fn main() -> i32 { build(); return 0; }",
        );
        assert!(codes.iter().any(|c| c == "E0513"), "repro shape must reject; got {codes:?}");
    }

    #[test]
    fn view_carrying_whole_struct_to_ref_target_e0513() {
        let codes = check_src(
            "struct Buf { p: *u8 }\n\
             impl Buf { fn drop(ref this) { return; } fn view(this) -> str { return \"x\"; } }\n\
             struct Data { key: str, n: i32 }\n\
             fn put(ref out: Data) {\n\
                 let owner: Buf = Buf { p: 0 as *u8 };\n\
                 out = Data { key: owner.view(), n: 1 };\n\
                 return;\n\
             }\n\
             fn main() -> i32 { var d: Data = Data { key: \"x\", n: 0 }; put(d); return 0; }",
        );
        assert!(codes.iter().any(|c| c == "E0513"), "ref-target whole-struct store; got {codes:?}");
    }

    #[test]
    fn view_carrying_alias_return_e0513() {
        let codes = check_src(
            "struct Buf { p: *u8 }\n\
             impl Buf { fn drop(ref this) { return; } fn view(this) -> str { return \"x\"; } }\n\
             struct Data { key: str, n: i32 }\n\
             fn store(key: str, n: i32) -> Data { return Data { key: key, n: n }; }\n\
             fn make() -> Data {\n\
                 let owner: Buf = Buf { p: 0 as *u8 };\n\
                 let d: Data = store(owner.view(), 1);\n\
                 return d;\n\
             }\n\
             fn main() -> i32 { let d: Data = make(); return d.n; }",
        );
        assert!(codes.iter().any(|c| c == "E0513"), "alias return of rooted carrier; got {codes:?}");
    }

    #[test]
    fn builtin_str_method_chain_return_e0513() {
        // A sub-view method from the blessed `impl str` block extends the
        // receiver's borrow: returning `v.second()` where `v` views a dying
        // local dangles exactly like returning `v`.
        let codes = check_src(
            "impl str {\n\
                 fn second(this) -> str {\n\
                     let p: *u8 = { #str_ptr(this) + (1 as usize) };\n\
                     return { #str_from_raw_parts(p, #str_len(this) -% (1 as usize)) };\n\
                 }\n\
             }\n\
             struct Buf { p: *u8 }\n\
             impl Buf { fn drop(ref this) { return; } fn view(this) -> str { return \"xy\"; } }\n\
             fn peek() -> str {\n\
                 let owner: Buf = Buf { p: 0 as *u8 };\n\
                 let v: str = owner.view();\n\
                 return v.second();\n\
             }\n\
             fn main() -> i32 { return #str_len(peek()) as i32; }",
        );
        assert!(codes.iter().any(|c| c == "E0513"), "builtin sub-view chain return; got {codes:?}");
    }

    #[test]
    fn view_of_a_matched_out_owner_denied_e0513() {
        // A payload bound out of an OWNED scrutinee owns its part of it.
        // The transition assert found this shape missing: without payload
        // types and payload ownership the binding was untyped, so every
        // type-gated rule skipped it.
        let codes = check_src(&format!(
            "{VIEW_PRELUDE}enum Opt {{ S(Buf), N }}\n\
             fn take_it() -> Opt {{ return Opt::S(Buf::new()); }}\n\
             fn bad() -> str {{ match take_it() {{ Opt::S(b) => {{ return b.view(); }} \
             Opt::N => {{ return \"zz\"; }} }} }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0513"),
            "expected E0513 on a view of a matched-out owner, got {codes:?}"
        );
    }

    #[test]
    fn view_of_a_payload_borrowed_from_a_field_stays_clean() {
        // Matching a FIELD names storage the receiver still owns, so the
        // payload is a borrow of something that outlives the call.
        let codes = check_src(&format!(
            "{VIEW_PRELUDE}enum Opt {{ S(Buf), N }}\n\
             struct Box2 {{ o: Opt }}\n\
             impl Box2 {{ fn get(this) -> str {{ match this.o {{ Opt::S(b) => {{ return b.view(); }} \
             Opt::N => {{ return \"zz\"; }} }} }} }}"
        ));
        assert!(
            !codes.iter().any(|c| c == "E0513"),
            "a payload borrowed from a field must not be denied, got {codes:?}"
        );
    }

    #[test]
    fn view_escaping_through_a_tuple_or_an_index_denied_e0513() {
        // Two place/type shapes the port did not reach at first, both found
        // by the transition assert. A tuple has no name until monomorphize
        // synthesizes its struct, but `(str, i32)` transports a view out of
        // the frame today; and an element of an array `static` is a write
        // target like any other.
        let cases: &[(&str, &str)] = &[
            (
                "tuple_leaf",
                "fn bad() -> (str, i32) { let b: Buf = Buf::new(); return (b.view(), 1); }",
            ),
            (
                "tuple_alias",
                "fn bad2() -> (str, i32) { let b: Buf = Buf::new(); let s: str = b.view(); \
                 return (s, 1); }",
            ),
            (
                "static_element",
                "static A: [str; 2] = [\"\", \"\"];\n\
                 fn bad3() { let b: Buf = Buf::new(); A[0] = b.view(); return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{VIEW_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0513"),
                "[{name}] expected E0513, got {codes:?}"
            );
        }
    }

    // --- capture escapes (E0365), ported from sema with issue-07 step 5 ---

    const CAPTURE_PRELUDE: &str = "struct Child { clicks: i32 }\n\
         impl Child {\n\
           fn clicked(ref this) { this.clicks = this.clicks + 1; return; }\n\
           fn build(ref this) -> i32 { return take_handler(this.clicked); }\n\
         }\n\
         fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }\n\
         fn sink(v: i32) { return; }\n\
         static SLOT: i32 = 0;\n";

    // A capturing method reachable from a read receiver, so a by-value
    // parameter can be the receiver at all, plus a non-Copy twin (`drop`
    // makes it non-Copy) for the control.
    const COPY_CAPTURE_PRELUDE: &str = "struct Ro { n: i32 }\n\
         impl Ro {\n\
           fn tap(this) { return; }\n\
           fn build(this) -> i32 { return take_handler(this.tap); }\n\
         }\n\
         struct Heavy { n: i32 }\n\
         impl Heavy {\n\
           fn drop(ref this) { return; }\n\
           fn htap(this) { return; }\n\
           fn hbuild(this) -> i32 { return take_handler(this.htap); }\n\
         }\n\
         fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }\n";

    #[test]
    fn returned_capture_of_a_local_denied_e0365() {
        // The address of a frame-local reaches the caller bound into a
        // handler that some later event-loop turn calls. Direct (the
        // handler is bound here), transitive (the callee binds it), and
        // through an intermediate that absorbed it.
        let cases: &[(&str, &str)] = &[
            (
                "direct",
                "fn make() -> i32 { var c: Child = Child { clicks: 0 }; \
                 return take_handler(c.clicked); }",
            ),
            (
                "transitive",
                "fn make() -> i32 { var c: Child = Child { clicks: 0 }; return c.build(); }",
            ),
            (
                "through_a_builder_local",
                "struct Bag { n: i32 }\n\
                 impl Bag {\n\
                   fn new() -> Bag { return Bag { n: 0 }; }\n\
                   fn add(ref this, v: i32) { this.n = this.n + v; return; }\n\
                 }\n\
                 fn wrap(b: Bag) -> i32 { return b.n; }\n\
                 fn make() -> i32 { var bag: Bag = Bag::new(); \
                 var c: Child = Child { clicks: 0 }; bag.add(c.build()); return wrap(bag); }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{CAPTURE_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0365"),
                "[{name}] expected E0365, got {codes:?}"
            );
        }
    }

    #[test]
    fn returned_capture_through_a_transitively_capturing_method_denied_e0365() {
        // The capture set is a fixpoint: `outer` binds nothing itself, it
        // returns `this.inner()`, which does the binding. Without the
        // fixpoint only directly-binding methods are known, and a component
        // that composes its handler one level down is missed.
        let codes = check_src(
            "struct Child { clicks: i32 }\n\
             impl Child {\n\
               fn clicked(ref this) { this.clicks = this.clicks + 1; return; }\n\
               fn inner(ref this) -> i32 { return take_handler(this.clicked); }\n\
               fn outer(ref this) -> i32 { return this.inner(); }\n\
             }\n\
             fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }\n\
             fn make() -> i32 { var c: Child = Child { clicks: 0 }; return c.outer(); }",
        );
        assert!(
            codes.iter().any(|c| c == "E0365"),
            "expected E0365 through the transitive capture, got {codes:?}"
        );
    }

    #[test]
    fn stored_capture_of_a_local_denied_e0365() {
        // The escape that is not a `return`: a `static`, or a `ref` target
        // aliasing the caller's storage. Both outlive the frame, so both
        // read the stack slot after it is gone.
        let cases: &[(&str, &str)] = &[
            (
                "static_direct",
                "fn stash() { var c: Child = Child { clicks: 0 }; \
                 SLOT = take_handler(c.clicked); return; }",
            ),
            (
                "static_transitive",
                "fn stash() { var c: Child = Child { clicks: 0 }; SLOT = c.build(); return; }",
            ),
            (
                "ref_param",
                "fn stash(ref out: i32) { var c: Child = Child { clicks: 0 }; \
                 out = take_handler(c.clicked); return; }",
            ),
        ];
        for (name, tail) in cases {
            let codes = check_src(&format!("{CAPTURE_PRELUDE}{tail}"));
            assert!(
                codes.iter().any(|c| c == "E0365"),
                "[{name}] expected E0365, got {codes:?}"
            );
        }
    }

    #[test]
    fn stored_capture_that_outlives_nothing_stays_clean() {
        // A destination that dies with the frame outlives nothing, and a
        // handler bound to `this` points at caller-owned storage. Over-firing
        // on either would reject the ordinary way to build a node.
        let clean: &[(&str, &str)] = &[
            (
                "local_destination",
                "fn build_it() { var c: Child = Child { clicks: 0 }; \
                 var n: i32 = 0; n = take_handler(c.clicked); return; }",
            ),
            (
                "bound_to_this_into_a_static",
                "impl Child { fn stash(ref this) { SLOT = take_handler(this.clicked); return; } }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{CAPTURE_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0365"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }

    #[test]
    fn a_capture_of_a_by_value_copy_parameter_is_denied_e0365() {
        // Found by the transition assert, exercised by no test and no
        // vendor program. A by-value parameter of a Copy type is the
        // frame's OWN copy, so a handler bound to it points at this stack
        // slot; the view family filters exactly this case out because a
        // Copy root owns no heap. A non-Copy by-value parameter is the
        // caller's storage and stays legal — that is the control.
        let cases: &[(&str, &str, bool)] = &[
            (
                "copy_by_value",
                "fn steal(c: Ro) -> i32 { return c.build(); }",
                true,
            ),
            (
                "copy_take",
                "fn steal2(take c: Ro) -> i32 { return c.build(); }",
                true,
            ),
            (
                "copy_ref",
                "fn steal3(ref c: Ro) -> i32 { return c.build(); }",
                true,
            ),
            (
                "non_copy_by_value",
                "fn borrowed(h: Heavy) -> i32 { return h.hbuild(); }",
                false,
            ),
            (
                "this_receiver_is_not_widened",
                "impl Ro { fn keepit(this) -> i32 { return this.build(); } }",
                false,
            ),
        ];
        for (name, tail, denied) in cases {
            let codes = check_src(&format!("{COPY_CAPTURE_PRELUDE}{tail}"));
            assert_eq!(
                codes.iter().any(|c| c == "E0365"),
                *denied,
                "[{name}] expected denied={denied}, got {codes:?}"
            );
        }
    }

    #[test]
    fn a_capture_handed_to_a_call_is_denied_e0365() {
        // The callee may be a registry or a retained tree; nothing here can
        // prove it is not, and the pair is where the violation lives.
        let codes = check_src(&format!(
            "{CAPTURE_PRELUDE}fn hand_over() {{ var c: Child = Child {{ clicks: 0 }}; \
             sink(take_handler(c.clicked)); return; }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0365"),
            "expected E0365 on the call argument, got {codes:?}"
        );
    }

    #[test]
    fn an_enum_payload_is_judged_at_its_own_sink_not_at_the_constructor() {
        // `Enum::Variant(payload)` parses as a call, but there is no callee
        // to keep anything — it builds a value. Asking the argument sink
        // there would deny a payload that never leaves the frame. The
        // escape is still denied when the value it built escapes.
        let prelude = format!("{CAPTURE_PRELUDE}enum Holder2 {{ Some2(i32), None2 }}\n");
        let codes = check_src(&format!(
            "{prelude}fn make() -> i32 {{ var c: Child = Child {{ clicks: 0 }}; \
             var h: Holder2 = Holder2::Some2(c.build()); return 0; }}"
        ));
        assert!(
            !codes.iter().any(|c| c == "E0365"),
            "a payload that dies with the frame must not be denied, got {codes:?}"
        );
        let codes = check_src(&format!(
            "{prelude}fn make() -> Holder2 {{ var c: Child = Child {{ clicks: 0 }}; \
             return Holder2::Some2(c.build()); }}"
        ));
        assert!(
            codes.iter().any(|c| c == "E0365"),
            "a returned payload still escapes, got {codes:?}"
        );
    }

    #[test]
    fn a_capture_of_caller_owned_storage_handed_to_a_call_stays_clean() {
        // `this` is the caller's, so it outlives the call — the gate is the
        // whole reason this sink costs nothing in real code.
        let codes = check_src(&format!(
            "{CAPTURE_PRELUDE}impl Child {{ fn hand(ref this) {{ \
             sink(take_handler(this.clicked)); return; }} }}"
        ));
        assert!(
            !codes.iter().any(|c| c == "E0365"),
            "a handler bound to `this` must not be denied, got {codes:?}"
        );
    }

    #[test]
    fn returned_capture_of_storage_that_outlives_the_frame_stays_clean() {
        // The line the rule must not cross. Real code binds handlers to
        // `this` or to a field, and the BINDING site itself is not an
        // escape — whether the callee's RESULT carries the address is a
        // property of the callee, which this analysis does not read.
        // Rejecting these would reject every handler in every component.
        let clean: &[(&str, &str)] = &[
            (
                "bound_to_this",
                "impl Child { fn mk(ref this) -> i32 { return take_handler(this.clicked); } }",
            ),
            (
                "child_in_a_field",
                "struct Parent { child: Child }\n\
                 impl Parent { fn build(ref this) -> i32 { return this.child.build(); } }",
            ),
            (
                "static_receiver",
                "static GLOBAL_CHILD: Child = Child { clicks: 0 };\n\
                 fn make() -> i32 { return GLOBAL_CHILD.build(); }",
            ),
            (
                "binding_site_result_is_trusted",
                "fn make() -> i32 { var c: Child = Child { clicks: 0 }; \
                 var n: i32 = take_handler(c.clicked); return n; }",
            ),
            (
                // The false-positive guard that matters most after widening:
                // a local child that binds NO handler is pure composition,
                // and it has the same call shape as the real hazard.
                "child_binds_no_handler",
                "struct Quiet { n: i32 }\n\
                 impl Quiet {\n\
                   fn new() -> Quiet { return Quiet { n: 1 }; }\n\
                   fn build(ref this) -> i32 { return this.n; }\n\
                 }\n\
                 struct Bag { n: i32 }\n\
                 impl Bag {\n\
                   fn new() -> Bag { return Bag { n: 0 }; }\n\
                   fn add(ref this, v: i32) { this.n = this.n + v; return; }\n\
                 }\n\
                 fn wrap(b: Bag) -> i32 { return b.n; }\n\
                 fn make() -> i32 { var bag: Bag = Bag::new(); var q: Quiet = Quiet::new(); \
                 bag.add(q.build()); return wrap(bag); }",
            ),
        ];
        for (name, tail) in clean {
            let codes = check_src(&format!("{CAPTURE_PRELUDE}{tail}"));
            assert!(
                !codes.iter().any(|c| c == "E0365"),
                "[{name}] must not be denied, got {codes:?}"
            );
        }
    }
}
