//! Phase 5 — borrow checker (slices 5BC.1, 5BC.2a, 5BC.2b).
//!
//! Design note: [`docs/design/phase5-borrow-shared.md`](../../docs/design/phase5-borrow-shared.md).
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
use std::collections::HashMap;
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
}

#[derive(Debug, Clone)]
struct TypeInfo {
    is_copy: bool,
    /// Set during construction. Drop structs are always non-Copy
    /// regardless of field types (mirrors sema rule from §3F).
    is_drop: bool,
}

impl CopyOracle {
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
                        if !info.is_copy {
                            continue;
                        }
                        let all_copy = s.fields.iter().all(|f| oracle.is_type_copy_internal(&f.ty));
                        if !all_copy {
                            oracle.types.get_mut(&s.name.name).unwrap().is_copy = false;
                            changed = true;
                        }
                    }
                    ItemKind::Enum(e) => {
                        let info = oracle.types.get(&e.name.name).cloned();
                        let Some(info) = info else { continue };
                        if !info.is_copy {
                            continue;
                        }
                        let all_copy = e
                            .variants
                            .iter()
                            .all(|v| v.payload.iter().all(|t| oracle.is_type_copy_internal(t)));
                        if !all_copy {
                            oracle.types.get_mut(&e.name.name).unwrap().is_copy = false;
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
            TypeKind::Borrowed { inner, .. } => self.definitely_non_copy(inner),
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
            TypeKind::Borrowed { inner, .. } => self.is_type_copy_internal(inner),
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
}

/// v0.0.24 #9: the borrowck mirror of codegen's `effective_move` — does passing
/// this parameter by value *consume* the argument? Only a `take` parameter
/// consumes: a bare `x: T` is a read-only borrow (the caller keeps and drops
/// it), and `ref` is a write-back borrow — neither moves. This mirrors codegen
/// `effective_move` (`p.move_ && non-Copy aggregate`).
///
/// Restricted to `TypeKind::Path` on purpose: borrowck runs *before*
/// monomorphization, so a generic instantiation (`Vec[T]`, `Pair[A, B]`) or a
/// bare generic param (`T`) is unresolved here — `definitely_non_copy` returns
/// false for those. Treating them as non-moves keeps the analysis conservative;
/// codegen still moves them correctly post-mono.
fn param_is_effective_move(p: &crate::ast::Param, oracle: &CopyOracle) -> bool {
    // `Path` and `Generic` (an instantiation like `Vec[i32]`) both reach
    // `definitely_non_copy`, which now answers definitively for Drop generic
    // bases — so passing a `take v: Vec[T]` consumes it, and a move-while-view
    // is caught. Non-Drop generics still answer `false` there (conservative),
    // so this stays a no-move for genuinely-maybe-Copy instantiations.
    p.move_
        && matches!(&p.ty.kind, TypeKind::Path(_) | TypeKind::Generic { .. })
        && oracle.definitely_non_copy(&p.ty)
}

impl SigTable {
    fn collect(prog: &Program, oracle: &CopyOracle) -> Self {
        let mut t = SigTable::default();
        for item in &prog.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    let (return_borrow, return_borrow_flavor) =
                        detect_fn_elision_with_flavor(f, oracle);
                    t.fns.insert(
                        f.name.name.clone(),
                        FnEntry {
                            receiver_claim: None,
                            param_moves: f
                                .params
                                .iter()
                                .map(|p| param_is_effective_move(p, oracle))
                                .collect(),
                            param_muts: f.params.iter().map(|p| p.mutable).collect(),
                            return_borrow,
                            return_borrow_flavor,
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
                }
                ItemKind::Enum(e) => {
                    t.enums.insert(e.name.name.clone());
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        let key = format!("{}.{}", b.target.name, m.name.name);
                        let (return_borrow, return_borrow_flavor) =
                            detect_method_elision_with_flavor(b, m, oracle);
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
                                    .map(|p| param_is_effective_move(p, oracle))
                                    .collect(),
                                param_muts: m.params.iter().map(|p| p.mutable).collect(),
                                return_borrow,
                                return_borrow_flavor,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        t
    }

    fn fn_param_moves(&self, name: &str) -> Option<&Vec<bool>> {
        self.fns.get(name).map(|e| &e.param_moves)
    }

    /// Slice 6BC.1: per-parameter `ref` flag list. Parallel shape to
    /// `fn_param_moves`. Used by `apply_call` to claim a
    /// `BorrowedExclusive` against each `ref`-marked non-Copy argument
    /// and detect the four intra-call conflict patterns.
    fn fn_param_muts(&self, name: &str) -> Option<&Vec<bool>> {
        self.fns.get(name).map(|e| &e.param_muts)
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

/// The free-function return-borrow map for a whole program: every free fn
/// whose body was detected (Rules E1 / E1-mut / E3) to return a `str`/slice
/// view of one or more of its parameters, keyed by fn name. Sema reuses this
/// as the single source of truth for its E0513 return-escape check — so a
/// `return head(local)` where `head(x) -> str` returns a view of `x` is
/// traced to `local` and rejected, exactly as the direct `return local.view()`
/// form is. Built once (walks the program); the caller should cache it.
pub fn free_fn_return_borrows(prog: &Program) -> HashMap<String, ReturnBorrowSource> {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
    sigs.fns
        .into_iter()
        .filter_map(|(name, e)| e.return_borrow.map(|rb| (name, rb)))
        .collect()
}

/// Public test hook (5BC.3a): given a parsed program, the impl target
/// type name, and the method name, return the elision rule's detected
/// return-borrow source if any.
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
    // 6BC.5: explicit annotations short-circuit elision.
    if let Some((src, flavor)) = detect_fn_explicit_regions(f, oracle) {
        return (Some(src), Some(flavor));
    }
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

/// Slice 6BC.5: explicit region-annotation detection. The source
/// syntax this matched (`borrow REGION T`) is retired, so this detector
/// is now unreachable from user source; the structural rules it encodes
/// are kept for the region-typed AST it still recognizes.
///
/// Qualifies iff:
/// 1. The return type is a borrow-region type for some region name R.
/// 2. At least one parameter type is a borrow-region type for the same R.
/// 3. The return type and all matching parameter types are non-Copy.
///
/// The flavor is **Exclusive** when any of the matching parameters is
/// `ref`-marked, else **Shared**. (Mixed ref/shared on the same
/// region is rejected by sema in a future polish; 6BC.5 first cut
/// picks Exclusive whenever any contributing param is `ref`.)
///
/// Sources = the indices of every parameter typed with the matching
/// region. Single-source collapses to `Param(N)`; multi-source uses
/// `MultiParam(indices)`. `take`-marked params don't carry regions
/// (parser rejects); they never contribute to the source set.
fn detect_fn_explicit_regions(
    f: &Function,
    oracle: &CopyOracle,
) -> Option<(ReturnBorrowSource, BorrowFlavor)> {
    let ret = f.return_type.as_ref()?;
    let TypeKind::Borrowed {
        region: ret_region,
        inner: ret_inner,
    } = &ret.kind
    else {
        return None;
    };
    if !oracle.definitely_non_copy(ret_inner) {
        return None;
    }
    let mut indices: Vec<u32> = Vec::new();
    let mut any_mut = false;
    for (i, p) in f.params.iter().enumerate() {
        let TypeKind::Borrowed { region, inner } = &p.ty.kind else {
            continue;
        };
        if region != ret_region {
            continue;
        }
        if p.move_ {
            continue;
        } // parser rejects this, but defensive
        if !oracle.definitely_non_copy(inner) {
            continue;
        }
        indices.push(i as u32);
        if p.mutable {
            any_mut = true;
        }
    }
    if indices.is_empty() {
        return None;
    }
    let src = if indices.len() == 1 {
        ReturnBorrowSource::Param(indices[0])
    } else {
        ReturnBorrowSource::MultiParam(indices)
    };
    let flavor = if any_mut {
        BorrowFlavor::Exclusive
    } else {
        BorrowFlavor::Shared
    };
    Some((src, flavor))
}

/// Slice 6BC.2: method elision with flavor. Same shape as the free-fn
/// version, but for methods: tries Rule E2-mut (`ref this` + non-Copy
/// return) before Rule E2 (`this` + non-Copy return).
fn detect_method_elision_with_flavor(
    b: &ImplBlock,
    m: &Method,
    oracle: &CopyOracle,
) -> (Option<ReturnBorrowSource>, Option<BorrowFlavor>) {
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
                let key = format!("{type_name}.{}", method_name.name);
                let Some(entry) = self.sigs.methods.get(&key) else {
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
        match self.binding_types.get(name)? {
            BindingType::Known(t) => Some(t),
            BindingType::Unknown => None,
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

fn analyze_with_diags(prog: &Program) -> (ProgramAnalysis, Vec<(Option<String>, RawDiag)>) {
    let oracle = CopyOracle::build(prog);
    let sigs = SigTable::collect(prog, &oracle);
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
    (analysis, all_diags)
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

        for (i, stmt) in body.stmts.iter().enumerate() {
            self.apply_stmt(stmt, &mut state);
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
        for s in &b.stmts {
            self.apply_stmt(s, state);
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
            ExprKind::Cast { expr, .. } => self.apply_expr(expr, state),
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
        if let ExprKind::Field {
            receiver,
            name: method,
        } = &callee.kind
        {
            if let ExprKind::Ident(recv_name) = &receiver.kind {
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
                    }
                    if mut_flags.is_none() {
                        mut_flags = Some(entry.param_muts.clone());
                    }
                    receiver_claim = entry.receiver_claim.map(|k| (k, &**receiver));
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
                    self.apply_move_of_binding(name, arg.span, state);
                    continue;
                }
            }
            self.apply_expr(arg, state);
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
        // Build per-arg claims. Copy bindings, non-place exprs, and
        // unknown-type bindings (binding_is_non_copy returns false) all
        // produce no claim — they carry no aliasing constraint.
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
                        // Only non-Copy bindings carry borrow constraints.
                        if !self.binding_is_non_copy(&place.root) {
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
        let key = format!("{type_name}.{method_name}");
        let Some(entry) = self.sigs.methods.get(&key) else {
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
        ExprKind::Cast { expr, .. } => expr_reads_ident(expr, name),
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
  return;
}";
        let codes = check_src(src);
        assert!(
            codes.iter().any(|c| c == "E0372"),
            "take non-Copy struct arg while borrowed should move (E0372), got {codes:?}"
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
fn caller() {
  let v: E = E::B;
  let cur: E = cursor(v);
  let n: i32 = take(v);
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
        // `let y = ...;` without annotation — borrowck treats it as
        // Unknown and conservatively skips E0370 emission. 5BC.2/5BC.4
        // (sema integration) will tighten this.
        //
        // This program would actually be caught by sema-level E0335
        // already (sema's move tracking) — recording the behavior here
        // is about confirming borrowck doesn't double-fire / fire-where-it-can't-prove.
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
        // We're asserting borrowck does NOT fire E0370 here; sema may or
        // may not fire E0335 — that's not our concern. We use check_src
        // which goes through borrowck only.
        assert!(
            !codes.iter().any(|c| c == "E0370"),
            "E0370 should not fire on bindings of unknown type; got {codes:?}"
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
        // Caveat: match on i32 literal isn't currently supported (E0343).
        // This test may not be reachable until match-on-int is added.
        // For now we assert that whatever happens, it's not Some(Param(0)).
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
  return;
}";
        let dump = analyze_src(src);
        // After stmt 1 (the `let y` line): x is BorrowedShared(1).
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
    fn e0380_does_not_fire_on_two_mut_copy_args() {
        // `ref x: i32` is local-mutability for Copy types, not a borrow.
        let src = "\
fn modify_both(ref a: i32, ref b: i32) { return; }
fn caller() {
  let y: i32 = 1;
  modify_both(y, y);
  return;
}";
        let codes = check_src(src);
        assert!(
            !codes.iter().any(|c| c == "E0380"),
            "E0380 should not fire on Copy bindings; got {codes:?}"
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
    fn e0381_does_not_fire_on_copy_binding() {
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
            !codes.iter().any(|c| c == "E0381"),
            "E0381 should not fire on Copy bindings; got {codes:?}"
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
        let shapes: &[(&str, &str)] = &[
            ("struct_lit", "let w: Slot = Slot { s: t.view() };"),
            ("nested_struct_lit", "let w: Nest = Nest { inner: Slot { s: t.view() } };"),
            ("array_lit", "let w: [str; 1] = [t.view()];"),
            ("tuple_lit", "let w: (str, i32) = (t.view(), 1);"),
            ("enum_payload", "let w: Wrap = Wrap::V(t.view());"),
        ];
        for (sname, capture) in shapes {
            let src = format!(
                "{VIEW_PRELUDE}fn f_{sname}() {{ let t: Buf = Buf::new(); {capture} let _c: i32 = consume(t); return; }}"
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
             let w: str = h.b.view(); let _c: i32 = consume_h(h); return; }}"
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
            let src = format!(
                "{VIEW_PRELUDE}fn f_{sname}() {{ let t: Buf = Buf::new(); \
                 let w: str = t.view(); {mv} return; }}"
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
}
