//! Code knowledge graph — a resolved, queryable index over the front-end's
//! output (see `plan.graph.md`).
//!
//! The thesis: `cpc` already computes resolved names, spans, and structure on
//! every build and throws them away. This module *retains* that information as a
//! stable, addressable graph so an agent (and the LSP) can navigate C+ by
//! symbol rather than by `grep`. It is pure data over the resolved program; it
//! never touches codegen.
//!
//! Phase 1 (this slice) builds the **index skeleton**: one node per program
//! entity (module, function, method, struct/enum and their members, const,
//! static, type alias, interface), each with a stable symbol id and a resolved
//! `file:line:col`, plus the structural edges that come straight from the AST —
//! `defines` (module → item), `has_method` / `has_field` (type → member), and
//! `has_variant` (enum → variant). Call edges, reference edges, and types-at
//! positions are later phases that need sema's retained tables; they are not in
//! this slice.
//!
//! Symbol ids use the source name, never a monomorphized `Point__i32`
//! (consistent with the no-mangling rule), so a query answer pastes straight
//! back into source.

use crate::ast::{
    Block, BuilderEntry, BuilderModifierKind, Expr, ExprKind, ForLoop, Function, ImplBlock,
    InterpStrPart, ItemKind, Method, Param, Pattern, PatternKind, Receiver, Stmt, StmtKind, Type,
    TypeKind,
};
use crate::diagnostics::LineMap;
use crate::lexer::Span;
use crate::resolver::LoadedProject;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A resolved source location, in the same `file:line:col` shape diagnostics
/// emit, so a consumer can act on it without parsing prose.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// The kind of program entity a node represents.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Module,
    Function,
    ExternFn,
    Method,
    Struct,
    Enum,
    Variant,
    Field,
    Const,
    Static,
    TypeAlias,
    Interface,
}

/// A typed, directed edge between two nodes (identified by symbol id).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// module → item it declares.
    Defines,
    /// type → method in one of its `impl` blocks.
    HasMethod,
    /// struct → field.
    HasField,
    /// enum → variant.
    HasVariant,
    /// fn/method → the fn/method it calls (Phase 3). Resolved structurally:
    /// free/associated calls by name, method calls by the receiver's type
    /// where that type is locally known (`self`, or a typed local/param).
    /// Call sites whose receiver type can't be determined locally are not
    /// edges; they are counted in `unresolved_calls` instead.
    Calls,
}

/// A program entity with stable identity.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Node {
    /// Stable symbol id — a qualified path, e.g. `src.math::Point::translate`.
    pub id: String,
    pub kind: NodeKind,
    /// The source-level name (the id's last segment).
    pub name: String,
    /// Definition site, resolved to `file:line:col`. `None` only for synthetic
    /// items the resolver left without an origin file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// A rendered signature / type, where one applies (functions, methods,
    /// fields, consts, statics, aliases).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub is_pub: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// The whole-project index: nodes plus the structural edges between them.
/// What a reference's use site is doing with the symbol.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// A call site for the symbol (Phase 4a).
    Call,
    /// A use of a named type — in a signature, field, let annotation, cast, or
    /// struct literal (Phase 4b). Value references (const/static/fn-as-value)
    /// land in a later slice.
    Type,
}

/// A resolved use site of a symbol, with its precise `file:line:col`. This is
/// the line-level answer to "where is X used", distinct from `callers` (which
/// returns the enclosing functions).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reference {
    /// The referenced symbol's id.
    pub symbol: String,
    pub kind: RefKind,
    pub location: Location,
    /// The enclosing item the reference sits inside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_context: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Per-function count of call sites whose target could not be resolved
    /// structurally (keyed by the caller's symbol id). This is the honesty
    /// signal for the call queries: an agent trusts the `Calls` edges and
    /// falls back to `grep` only for the unresolved residue. Empty for
    /// functions with no unresolved calls.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unresolved_calls: BTreeMap<String, u32>,
    /// Resolved use sites with precise locations (Phase 4a: call sites).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Reference>,
    /// Spans whose type is known locally — parameters, fields, typed locals,
    /// and their identifier uses — backing `type-at`. Internal (not part of the
    /// `cpc graph` JSON surface); a sparse map, not every expression.
    #[serde(skip)]
    pub type_spots: Vec<TypeSpot>,
    /// v0.0.14 graph value-depth: per-binding value-flow within each function,
    /// backing the `value-refs` query. Internal (queried, not part of the
    /// `cpc graph` JSON surface).
    #[serde(skip)]
    pub value_flows: Vec<ValueFlow>,
}

/// A source span with a locally-known type, for `type-at`. `what` names the
/// kind of place (parameter, field, local, …) for a human/agent reading the
/// answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSpot {
    pub fid: String,
    pub span: Span,
    pub location: Location,
    pub ty: String,
    pub what: String,
}

/// v0.0.14 graph value-depth: how a binding's value flows at a use site —
/// the classification that makes value-references more than "find name uses".
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    /// Read in place (field/method/index receiver, operand, condition).
    Read,
    /// Moved/passed into a function or method call argument.
    Call,
    /// Moved into a struct or enum constructor (a "re-wrap").
    Construct,
    /// Returned from the enclosing function.
    Return,
    /// Used as a `match` scrutinee.
    Match,
    /// Moved into another binding or assigned to a place (`let y = x;`, `p = x;`).
    Assign,
}

/// v0.0.14 graph value-depth: one use site of a local binding, with how its
/// value flows there.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValueUse {
    pub location: Location,
    #[serde(skip)]
    pub span: Span,
    pub flow: FlowKind,
}

/// v0.0.14 graph value-depth: the value-flow of one local binding (a parameter
/// or `let`) within its function — its definition site plus every classified
/// use. Backs the `value-refs` query ("where does this value go").
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValueFlow {
    #[serde(skip)]
    pub fid: String,
    pub binding: String,
    /// The kind of definition: `parameter` or `local`.
    pub def_kind: String,
    pub def_location: Location,
    #[serde(skip)]
    pub def_span: Span,
    pub uses: Vec<ValueUse>,
    /// v0.0.15 inter-procedural flow: if this binding's value escapes via the
    /// enclosing free function's `return`, the call-site destinations the
    /// returned value lands in (`let y = f(...)` in a caller → `y`). Best-effort
    /// and name-matched: free-function calls whose result is bound by a `let`.
    /// Empty for bindings that don't escape or whose callers can't be resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub returns_into: Vec<ReturnDest>,
}

/// v0.0.15 inter-procedural flow: one downstream destination a returned value
/// reaches in a caller — the binding it is `let`-bound to at the call site.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReturnDest {
    /// The caller-side binding that receives the call result.
    pub binding: String,
    pub location: Location,
    #[serde(skip)]
    pub span: Span,
}

impl CodeGraph {
    /// Build the index from a resolved project. Pure over the AST + per-file
    /// source map; runs after the resolver, independent of sema and codegen.
    pub fn build(proj: &LoadedProject) -> CodeGraph {
        let mut g = CodeGraph::default();
        // Function/method bodies to resolve call edges over, collected during
        // the node pass and walked once the node index exists (§ call edges).
        let mut callables: Vec<Callable> = Vec::new();
        // Type references from signatures / fields, as (short_name, location,
        // in_context); resolved to ids once the type index exists.
        let mut sig_type_refs: Vec<(String, Location, String)> = Vec::new();

        // One `LineMap` per file id, built once and reused for every span in
        // that file. Spans are file-relative, so each item resolves against the
        // source of its own `origin_file`.
        let mut linemaps: BTreeMap<String, LineMap> = BTreeMap::new();
        for (fid, (_, src)) in &proj.files {
            linemaps.insert(fid.clone(), LineMap::new(src));
        }

        // Module nodes: one per file the resolver loaded.
        for (fid, (path, _)) in &proj.files {
            g.nodes.push(Node {
                id: fid.clone(),
                kind: NodeKind::Module,
                name: fid.clone(),
                location: Some(Location {
                    file: path.display().to_string(),
                    line: 1,
                    col: 1,
                }),
                signature: None,
                is_pub: true,
            });
        }

        let resolve = |fid: &str, span: crate::lexer::Span| -> Option<Location> {
            let (path, src) = proj.files.get(fid)?;
            let lm = linemaps.get(fid)?;
            let pos = lm.position(span.start, src);
            Some(Location {
                file: path.display().to_string(),
                line: pos.line,
                col: pos.col,
            })
        };

        for item in &proj.program.items {
            // In project mode every real item carries its origin file; fall
            // back to the entry file id for any synthetic item that doesn't.
            let fid = item
                .origin_file
                .clone()
                .unwrap_or_else(|| proj.entry_file_id.clone());

            match &item.kind {
                ItemKind::Function(f) => {
                    let name = short_name(&f.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    let kind = if f.is_extern {
                        NodeKind::ExternFn
                    } else {
                        NodeKind::Function
                    };
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind,
                        name,
                        location: resolve(&fid, f.name.span),
                        signature: Some(fn_signature(f)),
                        is_pub: !f.name.name.starts_with('_'),
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id.clone(),
                        kind: EdgeKind::Defines,
                    });
                    for p in &f.params {
                        push_type_refs(&p.ty, &fid, &id, &resolve, &mut sig_type_refs);
                        if let Some(location) = resolve(&fid, p.name.span) {
                            g.type_spots.push(TypeSpot {
                                fid: fid.clone(),
                                span: p.name.span,
                                location,
                                ty: type_to_string(&p.ty),
                                what: "parameter".to_string(),
                            });
                        }
                    }
                    if let Some(rt) = &f.return_type {
                        push_type_refs(rt, &fid, &id, &resolve, &mut sig_type_refs);
                    }
                    if !f.is_extern {
                        callables.push(Callable {
                            from_id: id,
                            fid: fid.clone(),
                            self_type: None,
                            params: &f.params,
                            body: &f.body,
                        });
                    }
                }
                ItemKind::Struct(s) => {
                    let name = short_name(&s.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::Struct,
                        name,
                        location: resolve(&fid, s.name.span),
                        signature: None,
                        is_pub: !s.name.name.starts_with('_'),
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id.clone(),
                        kind: EdgeKind::Defines,
                    });
                    for field in &s.fields {
                        let fid_id = format!("{id}::{}", field.name.name);
                        g.nodes.push(Node {
                            id: fid_id.clone(),
                            kind: NodeKind::Field,
                            name: field.name.name.clone(),
                            location: resolve(&fid, field.name.span),
                            signature: Some(type_to_string(&field.ty)),
                            is_pub: !field.name.name.starts_with('_'),
                        });
                        g.edges.push(Edge {
                            from: id.clone(),
                            to: fid_id,
                            kind: EdgeKind::HasField,
                        });
                        push_type_refs(&field.ty, &fid, &id, &resolve, &mut sig_type_refs);
                        if let Some(location) = resolve(&fid, field.name.span) {
                            g.type_spots.push(TypeSpot {
                                fid: fid.clone(),
                                span: field.name.span,
                                location,
                                ty: type_to_string(&field.ty),
                                what: "field".to_string(),
                            });
                        }
                    }
                }
                ItemKind::Enum(e) => {
                    let name = short_name(&e.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::Enum,
                        name,
                        location: resolve(&fid, e.name.span),
                        signature: None,
                        is_pub: !e.name.name.starts_with('_'),
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id.clone(),
                        kind: EdgeKind::Defines,
                    });
                    for v in &e.variants {
                        let v_id = format!("{id}::{}", v.name.name);
                        let sig = if v.payload.is_empty() {
                            None
                        } else {
                            let parts: Vec<String> =
                                v.payload.iter().map(type_to_string).collect();
                            Some(format!("({})", parts.join(", ")))
                        };
                        g.nodes.push(Node {
                            id: v_id.clone(),
                            kind: NodeKind::Variant,
                            name: v.name.name.clone(),
                            location: resolve(&fid, v.name.span),
                            signature: sig,
                            is_pub: !e.name.name.starts_with('_'),
                        });
                        g.edges.push(Edge {
                            from: id.clone(),
                            to: v_id,
                            kind: EdgeKind::HasVariant,
                        });
                        for pty in &v.payload {
                            push_type_refs(pty, &fid, &id, &resolve, &mut sig_type_refs);
                        }
                    }
                }
                ItemKind::Impl(b) => {
                    add_impl_methods(&mut g, &fid, b, &resolve, &mut callables, &mut sig_type_refs);
                }
                ItemKind::Interface(it) => {
                    let name = short_name(&it.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::Interface,
                        name,
                        location: resolve(&fid, it.name.span),
                        signature: None,
                        is_pub: !it.name.name.starts_with('_'),
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
                }
                ItemKind::TypeAlias(a) => {
                    let name = short_name(&a.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::TypeAlias,
                        name,
                        location: resolve(&fid, a.name.span),
                        signature: Some(type_to_string(&a.target)),
                        is_pub: !a.name.name.starts_with('_'),
                    });
                    push_type_refs(&a.target, &fid, &id, &resolve, &mut sig_type_refs);
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
                }
                ItemKind::Const(c) => {
                    let name = short_name(&c.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::Const,
                        name,
                        location: resolve(&fid, c.name.span),
                        signature: Some(type_to_string(&c.ty)),
                        is_pub: !c.name.name.starts_with('_'),
                    });
                    push_type_refs(&c.ty, &fid, &id, &resolve, &mut sig_type_refs);
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
                }
                ItemKind::Static(s) => {
                    let name = short_name(&s.name.name).to_string();
                    let id = format!("{fid}::{name}");
                    g.nodes.push(Node {
                        id: id.clone(),
                        kind: NodeKind::Static,
                        name,
                        location: resolve(&fid, s.name.span),
                        signature: Some(type_to_string(&s.ty)),
                        is_pub: !s.name.name.starts_with('_'),
                    });
                    push_type_refs(&s.ty, &fid, &id, &resolve, &mut sig_type_refs);
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
                }
                // v0.0.15: module-scope `#asm("...")` defines no named symbol
                // the graph can reference (raw assembly text) — no node, no edge.
                ItemKind::ModuleAsm(_) => {}
            }
        }

        resolve_call_edges(&mut g, &callables, &sig_type_refs, &resolve);

        // v0.0.14 value-depth: the type-at and value-refs analyses both need a
        // LOWERED program (sema rejects un-lowered if-let/guard-let/while-let).
        // Lower a clone once and reuse it for both. Lowering preserves byte
        // spans for unchanged expressions, so spots/flows still align with the
        // original source.
        if let Some((entry_path, entry_src)) = proj.files.get(&proj.entry_file_id) {
            let mut lowered = proj.program.clone();
            let _ = crate::lower::lower(&mut lowered, entry_path, entry_src);
            let (_diags, mono) = crate::sema::check_multi_with_value_types(
                &lowered,
                entry_path.clone(),
                entry_src,
                proj.files.clone(),
            );
            g.add_inferred_type_spots(&mono.value_types, proj, &linemaps);
            g.collect_value_flows(&lowered, proj, &linemaps);
        }
        g
    }

    /// v0.0.14 graph value-depth: build per-binding value-flow for every
    /// function/method. For each parameter and `let`, record its definition and
    /// every classified use (read / call / construct / return / match / assign)
    /// within the body. Intra-procedural and shadow-naive (uses are grouped by
    /// name within one function), which covers the common "where does this value
    /// go" question backing the `value-refs` query.
    fn collect_value_flows(
        &mut self,
        program: &crate::ast::Program,
        proj: &LoadedProject,
        linemaps: &BTreeMap<String, LineMap>,
    ) {
        // v0.0.15 inter-procedural flow: free-function name -> indices into
        // `value_flows` of the bindings that escape via that function's
        // `return`. Filled as each free function's flows are built, then
        // consumed by `link_return_flows` to connect callers.
        let mut returning_fns: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for item in &program.items {
            let fid = item
                .origin_file
                .clone()
                .unwrap_or_else(|| proj.entry_file_id.clone());
            match &item.kind {
                ItemKind::Function(f) if !f.is_extern => {
                    let start = self.value_flows.len();
                    self.flows_for_body(&f.params, &f.body, &fid, proj, linemaps);
                    // A binding escapes this function if it flows to a `return`.
                    let returned: Vec<usize> = (start..self.value_flows.len())
                        .filter(|&i| {
                            self.value_flows[i]
                                .uses
                                .iter()
                                .any(|u| u.flow == FlowKind::Return)
                        })
                        .collect();
                    if !returned.is_empty() {
                        returning_fns
                            .entry(f.name.name.clone())
                            .or_default()
                            .extend(returned);
                    }
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        self.flows_for_body(&m.params, &m.body, &fid, proj, linemaps);
                    }
                }
                _ => {}
            }
        }
        self.link_return_flows(program, proj, linemaps, &returning_fns);
    }

    /// v0.0.15 inter-procedural flow: for every `let y = f(...)` whose callee
    /// `f` is a free function some of whose bindings escape via `return`, record
    /// the caller-side destination `y` on each of those returned bindings'
    /// flows. Best-effort and name-matched (exact resolved-name match on a
    /// free-function `Ident` callee); methods and indirectly-bound results are
    /// out of scope. This answers "where does the returned value go" one hop
    /// into the caller.
    fn link_return_flows(
        &mut self,
        program: &crate::ast::Program,
        proj: &LoadedProject,
        linemaps: &BTreeMap<String, LineMap>,
        returning_fns: &BTreeMap<String, Vec<usize>>,
    ) {
        if returning_fns.is_empty() {
            return;
        }
        for item in &program.items {
            let fid = item
                .origin_file
                .clone()
                .unwrap_or_else(|| proj.entry_file_id.clone());
            let bodies: Vec<&Block> = match &item.kind {
                ItemKind::Function(f) if !f.is_extern => vec![&f.body],
                ItemKind::Impl(b) => b.methods.iter().map(|m| &m.body).collect(),
                _ => Vec::new(),
            };
            if bodies.is_empty() {
                continue;
            }
            let resolve = |span: Span| -> Option<Location> {
                let (path, src) = proj.files.get(&fid)?;
                let lm = linemaps.get(&fid)?;
                let pos = lm.position(span.start, src);
                Some(Location {
                    file: path.display().to_string(),
                    line: pos.line,
                    col: pos.col,
                })
            };
            for body in bodies {
                let mut dests: Vec<(String, String, Span)> = Vec::new();
                collect_let_call_dests(body, &mut dests);
                for (callee, binding, span) in dests {
                    let Some(indices) = returning_fns.get(&callee) else {
                        continue;
                    };
                    let Some(location) = resolve(span) else {
                        continue;
                    };
                    for &idx in indices {
                        self.value_flows[idx].returns_into.push(ReturnDest {
                            binding: binding.clone(),
                            location: location.clone(),
                            span,
                        });
                    }
                }
            }
        }
    }

    fn flows_for_body(
        &mut self,
        params: &[Param],
        body: &Block,
        fid: &str,
        proj: &LoadedProject,
        linemaps: &BTreeMap<String, LineMap>,
    ) {
        let resolve = |span: Span| -> Option<Location> {
            let (path, src) = proj.files.get(fid)?;
            let lm = linemaps.get(fid)?;
            let pos = lm.position(span.start, src);
            Some(Location {
                file: path.display().to_string(),
                line: pos.line,
                col: pos.col,
            })
        };

        // v0.0.15 precise scoping: a single scope-tracking walk resolves every
        // use to the innermost in-scope definition, so shadowed names and
        // match-arm bindings attribute correctly (replacing the old
        // collect-defs + flat-uses + match-by-name approach).
        let mut sf = ScopedFlows::new();
        sf.push_scope(); // parameter scope
        for p in params {
            sf.define(&p.name.name, p.name.span, "parameter");
        }
        sf.walk_block(body, FlowKind::Read);
        sf.pop_scope();

        for def in sf.defs {
            let Some(def_location) = resolve(def.span) else {
                continue;
            };
            let mut flow_uses: Vec<ValueUse> = Vec::new();
            for (span, flow) in &def.uses {
                if let Some(location) = resolve(*span) {
                    flow_uses.push(ValueUse {
                        location,
                        span: *span,
                        flow: *flow,
                    });
                }
            }
            self.value_flows.push(ValueFlow {
                fid: fid.to_string(),
                binding: def.name,
                def_kind: def.kind.to_string(),
                def_location,
                def_span: def.span,
                uses: flow_uses,
                returns_into: Vec::new(),
            });
        }
    }

    /// v0.0.14 graph value-depth: add a `TypeSpot` for every inferred
    /// expression sema recorded — call results, arithmetic, field/index reads,
    /// `match`/`if` values — the cases the AST-only passes can't see. These
    /// coexist with the annotated spots; `type_at` returns the narrowest, so an
    /// annotated parameter/field still wins at its own span.
    fn add_inferred_type_spots(
        &mut self,
        value_types: &[(Option<String>, Span, String)],
        proj: &LoadedProject,
        linemaps: &BTreeMap<String, LineMap>,
    ) {
        for (fid_opt, span, ty) in value_types {
            let fid = fid_opt
                .clone()
                .unwrap_or_else(|| proj.entry_file_id.clone());
            let Some((path, src)) = proj.files.get(&fid) else {
                continue;
            };
            let Some(lm) = linemaps.get(&fid) else {
                continue;
            };
            let pos = lm.position(span.start, src);
            self.type_spots.push(TypeSpot {
                fid: fid.clone(),
                span: *span,
                location: Location {
                    file: path.display().to_string(),
                    line: pos.line,
                    col: pos.col,
                },
                ty: ty.clone(),
                what: "expression".to_string(),
            });
        }
    }

    /// Serialize the whole graph as pretty JSON (`cpc graph`).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Serialize a query result (a list of node references) as pretty JSON.
    /// Kept here so the only `serde_json` dependency stays in core; the CLI
    /// (which carries `serde_json` as a dev-dep only) calls this.
    pub fn nodes_to_json(nodes: &[&Node]) -> String {
        serde_json::to_string_pretty(nodes).unwrap_or_else(|_| "[]".to_string())
    }

    // ---- Phase-1 queries (pure node/edge lookups, no new analysis) ----

    /// Definition site(s) of a symbol. Matches a fully-qualified id exactly, or
    /// any node whose name (last segment) equals the query — so both
    /// `src.math::area` and a bare `area` resolve.
    pub fn def(&self, symbol: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| n.id == symbol || n.name == symbol)
            .collect()
    }

    /// Fields + methods of a struct/enum (by id or bare name). Returns the
    /// member nodes reachable via `has_field` / `has_method` / `has_variant`.
    pub fn members(&self, ty: &str) -> Vec<&Node> {
        let owners: Vec<&str> = self
            .nodes
            .iter()
            .filter(|n| {
                matches!(n.kind, NodeKind::Struct | NodeKind::Enum) && (n.id == ty || n.name == ty)
            })
            .map(|n| n.id.as_str())
            .collect();
        let member_ids: Vec<&str> = self
            .edges
            .iter()
            .filter(|e| {
                owners.contains(&e.from.as_str())
                    && matches!(
                        e.kind,
                        EdgeKind::HasField | EdgeKind::HasMethod | EdgeKind::HasVariant
                    )
            })
            .map(|e| e.to.as_str())
            .collect();
        self.nodes
            .iter()
            .filter(|n| member_ids.contains(&n.id.as_str()))
            .collect()
    }

    /// Outline of one file (by file id) or the whole project. Returns the
    /// non-module nodes, optionally restricted to those defined in `file`.
    pub fn symbols(&self, file: Option<&str>) -> Vec<&Node> {
        let in_file: Option<Vec<&str>> = file.map(|f| {
            self.edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Defines && e.from == f)
                .map(|e| e.to.as_str())
                .collect()
        });
        self.nodes
            .iter()
            .filter(|n| n.kind != NodeKind::Module)
            .filter(|n| match &in_file {
                None => true,
                Some(ids) => ids.contains(&n.id.as_str()),
            })
            .collect()
    }

    // ---- call queries (Phase 3) ----

    /// Symbol ids of the function/method nodes matching a query (by id or bare
    /// name). The anchor for `callers` / `callees` / `call-hierarchy`.
    fn callable_ids(&self, name: &str) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    NodeKind::Function | NodeKind::ExternFn | NodeKind::Method
                ) && (n.id == name || n.name == name)
            })
            .map(|n| n.id.clone())
            .collect()
    }

    /// Functions/methods that the named function calls (one hop, resolved).
    pub fn callees(&self, name: &str) -> Vec<&Node> {
        let ids = self.callable_ids(name);
        let targets: BTreeSet<&str> = self
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && ids.iter().any(|i| i == &e.from))
            .map(|e| e.to.as_str())
            .collect();
        self.nodes
            .iter()
            .filter(|n| targets.contains(n.id.as_str()))
            .collect()
    }

    /// Functions/methods that call the named function (one hop, resolved).
    pub fn callers(&self, name: &str) -> Vec<&Node> {
        let ids = self.callable_ids(name);
        let srcs: BTreeSet<&str> = self
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && ids.iter().any(|i| i == &e.to))
            .map(|e| e.from.as_str())
            .collect();
        self.nodes
            .iter()
            .filter(|n| srcs.contains(n.id.as_str()))
            .collect()
    }

    /// Transitive callees of the named function, to `depth` hops (deduped).
    pub fn call_hierarchy(&self, name: &str, depth: u32) -> Vec<&Node> {
        let mut seen: BTreeSet<String> = self.callable_ids(name).into_iter().collect();
        let mut frontier: Vec<String> = seen.iter().cloned().collect();
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut d = 0;
        while d < depth && !frontier.is_empty() {
            let mut next = Vec::new();
            for f in &frontier {
                for e in &self.edges {
                    if e.kind == EdgeKind::Calls && &e.from == f && seen.insert(e.to.clone()) {
                        reached.insert(e.to.clone());
                        next.push(e.to.clone());
                    }
                }
            }
            frontier = next;
            d += 1;
        }
        self.nodes
            .iter()
            .filter(|n| reached.contains(&n.id))
            .collect()
    }

    /// Unresolved call sites inside the named function (sum over matches).
    fn unresolved_for(&self, name: &str) -> u32 {
        self.callable_ids(name)
            .iter()
            .filter_map(|id| self.unresolved_calls.get(id))
            .sum()
    }

    /// Total unresolved call sites in the whole program — the caveat for
    /// `callers`, since any unresolved site could be a hidden caller.
    pub fn unresolved_total(&self) -> u32 {
        self.unresolved_calls.values().sum()
    }

    /// `cpc query callees` — JSON, or `None` if the function isn't found.
    pub fn callees_json(&self, name: &str) -> Option<String> {
        if self.callable_ids(name).is_empty() {
            return None;
        }
        Some(call_result_json(
            "callees",
            name,
            self.callees(name),
            self.unresolved_for(name),
        ))
    }

    /// `cpc query callers` — JSON, or `None` if the function isn't found. The
    /// `unresolved` count here is program-wide (any unresolved call site could
    /// be a caller this answer missed).
    pub fn callers_json(&self, name: &str) -> Option<String> {
        if self.callable_ids(name).is_empty() {
            return None;
        }
        Some(call_result_json(
            "callers",
            name,
            self.callers(name),
            self.unresolved_total(),
        ))
    }

    /// `cpc query call-hierarchy` — JSON, or `None` if the function isn't found.
    pub fn call_hierarchy_json(&self, name: &str, depth: u32) -> Option<String> {
        if self.callable_ids(name).is_empty() {
            return None;
        }
        Some(call_result_json(
            "call-hierarchy",
            name,
            self.call_hierarchy(name, depth),
            self.unresolved_for(name),
        ))
    }

    // ---- reference queries (Phase 4a: call-site references) ----

    /// Ids of any nodes matching a query (by id or bare name), across kinds.
    fn node_ids_matching(&self, name: &str) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| n.id == name || n.name == name)
            .map(|n| n.id.clone())
            .collect()
    }

    /// Resolved use sites of a symbol, with precise locations.
    pub fn refs(&self, name: &str) -> Vec<&Reference> {
        let ids = self.node_ids_matching(name);
        self.references
            .iter()
            .filter(|r| ids.iter().any(|i| i == &r.symbol))
            .collect()
    }

    /// `cpc query refs` — JSON, or `None` if the symbol isn't a known node.
    /// Carries `scope: "call-sites"`, since this build resolves call-site
    /// references only (type and value references land in a later slice).
    pub fn refs_json(&self, name: &str) -> Option<String> {
        if self.node_ids_matching(name).is_empty() {
            return None;
        }
        let references: Vec<Reference> = self.refs(name).into_iter().cloned().collect();
        Some(refs_result_json(name, references))
    }

    // ---- composite query ----

    /// `cpc query context <fn>` — the one-shot edit pack: the function's node
    /// (signature + location), its callers and callees, and the count of
    /// unresolved calls inside it. One call gives an agent the neighborhood it
    /// needs to change `fn` safely, instead of several. `None` if the name is
    /// not a function or method.
    pub fn context_json(&self, name: &str) -> Option<String> {
        let target_id = self.callable_ids(name).into_iter().next()?;
        let target = self.nodes.iter().find(|n| n.id == target_id)?.clone();
        // The named-type uses inside this function — the types it touches.
        let type_refs: Vec<Reference> = self
            .references
            .iter()
            .filter(|r| r.kind == RefKind::Type && r.in_context.as_deref() == Some(&target_id))
            .cloned()
            .collect();
        let res = ContextResult {
            kind: "context".to_string(),
            target,
            callers: self.callers(name).into_iter().cloned().collect(),
            callees: self.callees(name).into_iter().cloned().collect(),
            type_refs,
            unresolved: self.unresolved_for(name),
        };
        Some(serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string()))
    }

    // ---- type-at ----

    /// The locally-known type at a byte offset within a file id: the narrowest
    /// typed spot whose span contains the offset. `None` for an inferred
    /// expression (no spot) — type-at resolves parameters, fields, typed
    /// locals, `self`, and their identifier uses, not arbitrary expressions.
    pub fn type_at(&self, fid: &str, byte: u32) -> Option<&TypeSpot> {
        self.type_spots
            .iter()
            .filter(|s| s.fid == fid && byte >= s.span.start && byte < s.span.end)
            .min_by_key(|s| s.span.end.saturating_sub(s.span.start))
    }

    /// `cpc query type-at` — JSON for the type at a position, or `None` if no
    /// locally-typed node covers it.
    pub fn type_at_json(&self, fid: &str, byte: u32) -> Option<String> {
        let spot = self.type_at(fid, byte)?;
        let res = TypeAtResult {
            kind: "type-at".to_string(),
            ty: spot.ty.clone(),
            of: spot.what.clone(),
            location: spot.location.clone(),
        };
        Some(serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string()))
    }

    /// v0.0.14 value-refs: the value-flow of the binding at a byte offset —
    /// its definition plus every classified use. Prefers a binding whose
    /// *definition* covers the cursor (you clicked the `let`/param), else one
    /// whose *use* covers it (you clicked a use site).
    pub fn value_refs(&self, fid: &str, byte: u32) -> Option<&ValueFlow> {
        let in_span = |s: &Span| byte >= s.start && byte < s.end;
        self.value_flows
            .iter()
            .filter(|vf| vf.fid == fid)
            .find(|vf| in_span(&vf.def_span))
            .or_else(|| {
                self.value_flows
                    .iter()
                    .filter(|vf| vf.fid == fid)
                    .find(|vf| vf.uses.iter().any(|u| in_span(&u.span)))
            })
    }

    /// `cpc query value-refs` — JSON for the binding's value-flow at a position.
    pub fn value_refs_json(&self, fid: &str, byte: u32) -> Option<String> {
        let vf = self.value_refs(fid, byte)?;
        Some(serde_json::to_string_pretty(vf).unwrap_or_else(|_| "{}".to_string()))
    }
}

/// Byte offset of a 1-based `(line, col)` position in `src`, counted in chars
/// (so multi-byte UTF-8 is handled). A column past the line's end clamps to the
/// line end. `None` if `line`/`col` is 0 or the line doesn't exist.
pub fn byte_offset(src: &str, line: u32, col: u32) -> Option<u32> {
    if line == 0 || col == 0 {
        return None;
    }
    let mut byte = 0usize;
    let mut cur = 1u32;
    for l in src.split_inclusive('\n') {
        if cur == line {
            let mut b = byte;
            for (i, ch) in l.chars().enumerate() {
                if ch == '\n' || (i as u32) >= col - 1 {
                    break;
                }
                b += ch.len_utf8();
            }
            return Some(b as u32);
        }
        byte += l.len();
        cur += 1;
    }
    None
}

/// JSON shape for a call query: the result nodes plus the explicit
/// `unresolved` count, so a consumer knows exactly how much to trust the
/// answer and where to fall back to `grep`.
#[derive(Serialize)]
struct CallQueryResult {
    kind: String,
    target: String,
    nodes: Vec<Node>,
    unresolved: u32,
}

fn call_result_json(kind: &str, target: &str, nodes: Vec<&Node>, unresolved: u32) -> String {
    let res = CallQueryResult {
        kind: kind.to_string(),
        target: target.to_string(),
        nodes: nodes.into_iter().cloned().collect(),
        unresolved,
    };
    serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string())
}

/// JSON shape for `refs`: the resolved use sites plus an explicit `scope` so a
/// consumer knows the coverage (call sites today) and where to still use `grep`.
#[derive(Serialize)]
struct RefsQueryResult {
    kind: String,
    target: String,
    scope: String,
    count: usize,
    references: Vec<Reference>,
}

fn refs_result_json(target: &str, references: Vec<Reference>) -> String {
    let res = RefsQueryResult {
        kind: "refs".to_string(),
        target: target.to_string(),
        // Coverage: call sites and named-type uses. Value references
        // (const/static/fn-as-value) are not yet resolved — grep for those.
        scope: "calls,types".to_string(),
        count: references.len(),
        references,
    };
    serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string())
}

/// JSON shape for the composite `context` query: the target function's node
/// alongside its caller and callee neighborhoods in one payload.
#[derive(Serialize)]
struct ContextResult {
    kind: String,
    target: Node,
    callers: Vec<Node>,
    callees: Vec<Node>,
    /// Named-type uses inside the target (the types it touches), with locations.
    type_refs: Vec<Reference>,
    unresolved: u32,
}

/// JSON shape for `type-at`: the resolved type, what kind of place it is, and
/// where.
#[derive(Serialize)]
struct TypeAtResult {
    kind: String,
    #[serde(rename = "type")]
    ty: String,
    of: String,
    location: Location,
}

fn add_impl_methods<'a>(
    g: &mut CodeGraph,
    fid: &str,
    b: &'a ImplBlock,
    resolve: &impl Fn(&str, Span) -> Option<Location>,
    callables: &mut Vec<Callable<'a>>,
    sig_type_refs: &mut Vec<(String, Location, String)>,
) {
    let target = short_name(&b.target.name).to_string();
    let type_id = format!("{fid}::{target}");
    for m in &b.methods {
        let mname = short_name(&m.name.name);
        let id = format!("{type_id}::{mname}");
        g.nodes.push(Node {
            id: id.clone(),
            kind: NodeKind::Method,
            name: mname.to_string(),
            location: resolve(fid, m.name.span),
            signature: Some(method_signature(m)),
            is_pub: !m.name.name.starts_with('_'),
        });
        g.edges.push(Edge {
            from: type_id.clone(),
            to: id.clone(),
            kind: EdgeKind::HasMethod,
        });
        for p in &m.params {
            push_type_refs(&p.ty, fid, &id, resolve, sig_type_refs);
            if let Some(location) = resolve(fid, p.name.span) {
                g.type_spots.push(TypeSpot {
                    fid: fid.to_string(),
                    span: p.name.span,
                    location,
                    ty: type_to_string(&p.ty),
                    what: "parameter".to_string(),
                });
            }
        }
        if let Some(rt) = &m.return_type {
            push_type_refs(rt, fid, &id, resolve, sig_type_refs);
        }
        callables.push(Callable {
            from_id: id,
            fid: fid.to_string(),
            self_type: Some(target.clone()),
            params: &m.params,
            body: &m.body,
        });
    }
}

// ---- Call-edge resolution (Phase 3) ----

/// A function or method body to resolve call edges over, collected during the
/// node pass and walked once the node index exists.
struct Callable<'a> {
    from_id: String,
    /// The file id the body lives in, for resolving reference spans.
    fid: String,
    /// For methods: the short name of the impl target (`self`'s type).
    self_type: Option<String>,
    params: &'a [Param],
    body: &'a Block,
}

/// The base (named) type of a type, short-named, or `None` for shapes that
/// can't be a resolvable method receiver (pointers, arrays, slices, fn-ptrs,
/// tuples, regions).
fn base_type_name(t: &Type) -> Option<String> {
    match &t.kind {
        TypeKind::Path(s) => Some(short_name(s).to_string()),
        TypeKind::Generic { name, .. } => Some(short_name(name).to_string()),
        _ => None,
    }
}

/// Every named-type occurrence inside a type, as `(short_name, span)` — the
/// base of a `Path`/`Generic` plus the bases of any nested element / argument
/// / pointee / tuple-member / fn-ptr-param types. Primitives surface here too
/// (`i32`); they resolve to no node and are dropped at resolution.
fn collect_type_names(t: &Type, out: &mut Vec<(String, Span)>) {
    match &t.kind {
        TypeKind::Path(s) => out.push((short_name(s).to_string(), t.span)),
        TypeKind::Generic { name, args } => {
            out.push((short_name(name).to_string(), t.span));
            for a in args {
                collect_type_names(a, out);
            }
        }
        TypeKind::Array { elem, .. } => collect_type_names(elem, out),
        TypeKind::RawPtr(inner) | TypeKind::Slice(inner) => collect_type_names(inner, out),
        TypeKind::Borrowed { inner, .. } => collect_type_names(inner, out),
        TypeKind::Tuple(ts) => {
            for ty in ts {
                collect_type_names(ty, out);
            }
        }
        TypeKind::FnPtr {
            params,
            return_type,
            ..
        } => {
            for p in params {
                collect_type_names(p, out);
            }
            if let Some(r) = return_type {
                collect_type_names(r, out);
            }
        }
    }
}

/// Resolve a type's named occurrences against `fid` and append a `Type`
/// reference (with location and enclosing context) for each one — left for the
/// post-pass to map the name to a node id. Collected here as
/// `(short_name, location, in_context)`.
fn push_type_refs(
    ty: &Type,
    fid: &str,
    ctx: &str,
    resolve: &impl Fn(&str, Span) -> Option<Location>,
    out: &mut Vec<(String, Location, String)>,
) {
    let mut names = Vec::new();
    collect_type_names(ty, &mut names);
    for (short, span) in names {
        if let Some(loc) = resolve(fid, span) {
            out.push((short, loc, ctx.to_string()));
        }
    }
}

/// Pick the single target id from a candidate list, or `None` if there are
/// zero or more than one (ambiguous → honestly unresolved, never a wrong edge).
fn unique(ids: Option<&Vec<String>>) -> Option<String> {
    match ids {
        Some(v) if v.len() == 1 => Some(v[0].clone()),
        _ => None,
    }
}

/// Resolve call edges for every collected callable and record per-caller
/// unresolved counts. Builds two name indexes from the node set first so the
/// edge vector can be mutated without borrowing the nodes.
fn resolve_call_edges(
    g: &mut CodeGraph,
    callables: &[Callable],
    sig_type_refs: &[(String, Location, String)],
    resolve: &impl Fn(&str, Span) -> Option<Location>,
) {
    let mut fn_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // v0.0.13: a free call's callee, after the resolver, is the *qualified*
    // dotted name (`src.main.helper`), while node ids use `::`
    // (`src.main::helper`) and `fn_by_name` is keyed by the short name. This
    // map keys each fn by its qualified dotted form (id with `::`→`.`) so a
    // qualified callee resolves *uniquely* — even when two modules share a
    // short name. Without it, ordinary direct calls fall into `unresolved`,
    // which is the bug that made `callers`/`refs` under-report (see plan.md F).
    let mut fn_by_qualified: BTreeMap<String, String> = BTreeMap::new();
    let mut method_idx: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut type_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in &g.nodes {
        match n.kind {
            NodeKind::Function | NodeKind::ExternFn => {
                fn_by_name.entry(n.name.clone()).or_default().push(n.id.clone());
                fn_by_qualified.insert(n.id.replace("::", "."), n.id.clone());
            }
            NodeKind::Method => {
                // id = "fid::Type::method"; pull the Type segment.
                let mut it = n.id.rsplit("::");
                let _method = it.next();
                if let Some(ty) = it.next() {
                    method_idx
                        .entry((ty.to_string(), n.name.clone()))
                        .or_default()
                        .push(n.id.clone());
                }
            }
            NodeKind::Struct | NodeKind::Enum | NodeKind::TypeAlias | NodeKind::Interface => {
                type_by_name.entry(n.name.clone()).or_default().push(n.id.clone());
            }
            _ => {}
        }
    }

    // Signature / field type references (collected during the node pass).
    for (short, location, ctx) in sig_type_refs {
        if let Some(id) = unique(type_by_name.get(short)) {
            g.references.push(Reference {
                symbol: id,
                kind: RefKind::Type,
                location: location.clone(),
                in_context: Some(ctx.clone()),
            });
        }
    }

    for c in callables {
        let mut r = Resolver {
            env: BTreeMap::new(),
            self_type: &c.self_type,
            fn_by_name: &fn_by_name,
            fn_by_qualified: &fn_by_qualified,
            method_idx: &method_idx,
            from_id: &c.from_id,
            edges: Vec::new(),
            refs: Vec::new(),
            type_refs: Vec::new(),
            spots: Vec::new(),
            unresolved: 0,
        };
        for p in c.params {
            if let Some(bt) = base_type_name(&p.ty) {
                r.env.insert(p.name.name.clone(), bt);
            }
        }
        r.walk_block(c.body);
        let Resolver {
            edges,
            refs,
            type_refs,
            spots,
            unresolved,
            ..
        } = r;
        g.edges.extend(edges);
        for (target, span) in refs {
            if let Some(location) = resolve(&c.fid, span) {
                g.references.push(Reference {
                    symbol: target,
                    kind: RefKind::Call,
                    location,
                    in_context: Some(c.from_id.clone()),
                });
            }
        }
        for (short, span) in type_refs {
            if let (Some(id), Some(location)) =
                (unique(type_by_name.get(&short)), resolve(&c.fid, span))
            {
                g.references.push(Reference {
                    symbol: id,
                    kind: RefKind::Type,
                    location,
                    in_context: Some(c.from_id.clone()),
                });
            }
        }
        for (span, ty, what) in spots {
            if let Some(location) = resolve(&c.fid, span) {
                g.type_spots.push(TypeSpot {
                    fid: c.fid.clone(),
                    span,
                    location,
                    ty,
                    what,
                });
            }
        }
        if unresolved > 0 {
            g.unresolved_calls.insert(c.from_id.clone(), unresolved);
        }
    }
}

/// Walks one body in declaration order, seeding `env` from params and adding
/// each annotated `let` as it is reached, so a call's receiver resolves against
/// the locals visible at that point.
struct Resolver<'a> {
    env: BTreeMap<String, String>,
    self_type: &'a Option<String>,
    fn_by_name: &'a BTreeMap<String, Vec<String>>,
    fn_by_qualified: &'a BTreeMap<String, String>,
    method_idx: &'a BTreeMap<(String, String), Vec<String>>,
    from_id: &'a str,
    edges: Vec<Edge>,
    /// (target id, call-site span) for each resolved call, turned into a
    /// `Reference` with a precise location by the caller.
    refs: Vec<(String, Span)>,
    /// (short type name, use-site span) for each type use in the body —
    /// resolved against the type index and located by the caller.
    type_refs: Vec<(String, Span)>,
    /// (span, type, what) for typed locals, `self`, and identifier uses of a
    /// param/local — backing `type-at`.
    spots: Vec<(Span, String, String)>,
    unresolved: u32,
}

impl<'a> Resolver<'a> {
    fn walk_block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.walk_stmt(s);
        }
        if let Some(t) = &b.tail {
            self.walk_expr(t);
        }
    }

    fn walk_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, ty, init, .. } => {
                if let Some(t) = ty {
                    if let Some(bt) = base_type_name(t) {
                        self.env.insert(name.name.clone(), bt);
                    }
                    self.record_type(t);
                    self.spots
                        .push((name.span, type_to_string(t), "local".to_string()));
                }
                if let Some(e) = init {
                    self.walk_expr(e);
                }
            }
            StmtKind::Return(Some(e))
            | StmtKind::Expr(e)
            | StmtKind::Defer(e)
            | StmtKind::Assert(e) => self.walk_expr(e),
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::While { cond, body, .. } => {
                self.walk_expr(cond);
                self.walk_block(body);
            }
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(s) = init {
                        self.walk_stmt(s);
                    }
                    if let Some(c) = cond {
                        self.walk_expr(c);
                    }
                    for u in update {
                        self.walk_expr(u);
                    }
                    self.walk_block(body);
                }
                ForLoop::Range { iter, body, .. } => {
                    self.walk_expr(iter);
                    self.walk_block(body);
                }
            },
            StmtKind::Loop(b, _) => self.walk_block(b),
            StmtKind::IfLet {
                scrutinee,
                body,
                else_body,
                ..
            } => {
                self.walk_expr(scrutinee);
                self.walk_block(body);
                if let Some(eb) = else_body {
                    self.walk_block(eb);
                }
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                self.walk_expr(scrutinee);
                self.walk_block(body);
            }
            StmtKind::GuardLet {
                scrutinee,
                else_body,
                ..
            } => {
                self.walk_expr(scrutinee);
                self.walk_block(else_body);
            }
        }
    }

    /// v0.0.22 DSL.4: walk a builder body's entries (forward-defensive;
    /// post-resolver the block is already desugared — see `walk_expr`).
    fn walk_builder_entries(&mut self, entries: &[BuilderEntry]) {
        for entry in entries {
            match entry {
                BuilderEntry::Let(s) => self.walk_stmt(s),
                BuilderEntry::Item { expr, modifiers } => {
                    self.walk_expr(expr);
                    for m in modifiers {
                        match &m.kind {
                            BuilderModifierKind::Assign(v) => self.walk_expr(v),
                            BuilderModifierKind::Call(args) => {
                                for a in args {
                                    self.walk_expr(a);
                                }
                            }
                        }
                    }
                }
                BuilderEntry::If { cond, then, else_ } => {
                    self.walk_expr(cond);
                    self.walk_builder_entries(then);
                    if let Some(eb) = else_ {
                        self.walk_builder_entries(eb);
                    }
                }
                BuilderEntry::For { iter, body, .. } => {
                    self.walk_expr(iter);
                    self.walk_builder_entries(body);
                }
            }
        }
    }

    fn walk_expr(&mut self, e: &Expr) {
        match &e.kind {
            // v0.0.22 DSL.3: the graph builds on the resolved program, by
            // which point the resolver has contextual-rewritten bare item
            // names to real `ctx::name` paths and desugared the block to
            // ordinary calls — so those references are already indexed via
            // the normal Call/Path arms and resolve to the real package
            // symbols. This arm is the forward-defensive fallback for any
            // path that walks a pre-desugar AST; it walks entries directly.
            ExprKind::BuilderBlock { body, .. } => {
                self.walk_builder_entries(&body.entries);
            }
            ExprKind::Call { callee, args, .. } => {
                self.resolve_call(callee, e.span);
                self.walk_expr(callee);
                for a in args {
                    self.walk_expr(a);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            ExprKind::Unary { operand, .. } => self.walk_expr(operand),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.walk_expr(s);
                }
                if let Some(en) = end {
                    self.walk_expr(en);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                self.walk_expr(target);
                self.walk_expr(value);
            }
            ExprKind::Cast { expr, ty } => {
                self.record_type(ty);
                self.walk_expr(expr);
            }
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.walk_expr(cond);
                self.walk_block(then);
                if let Some(eb) = else_branch {
                    self.walk_expr(eb);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr(scrutinee);
                for a in arms {
                    self.walk_expr(&a.body);
                }
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.walk_block(b),
            ExprKind::Await(x) | ExprKind::Yield(x) => self.walk_expr(x),
            ExprKind::Field { receiver, .. } => self.walk_expr(receiver),
            ExprKind::Index { receiver, index } => {
                self.walk_expr(receiver);
                self.walk_expr(index);
            }
            ExprKind::StructLit { name, fields } => {
                self.type_refs
                    .push((short_name(&name.name).to_string(), name.span));
                for f in fields {
                    self.walk_expr(&f.value);
                }
            }
            // v0.0.24 de-Rust: the type-inferred literal carries no type name
            // (it is resolved from the expected type at sema time), so there is
            // no type_ref to record here — just walk the field values.
            ExprKind::InferredStructLit { fields } => {
                for f in fields {
                    self.walk_expr(&f.value);
                }
            }
            ExprKind::GenericStructLit {
                name,
                type_args,
                fields,
            } => {
                self.type_refs
                    .push((short_name(&name.name).to_string(), name.span));
                for t in type_args {
                    self.record_type(t);
                }
                for f in fields {
                    self.walk_expr(&f.value);
                }
            }
            ExprKind::GenericEnumCall {
                enum_name,
                type_args,
                args,
                ..
            } => {
                self.type_refs
                    .push((short_name(&enum_name.name).to_string(), enum_name.span));
                for t in type_args {
                    self.record_type(t);
                }
                for a in args {
                    self.walk_expr(a);
                }
            }
            ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
                for el in elements {
                    self.walk_expr(el);
                }
            }
            ExprKind::ArrayFill { fill, .. } => self.walk_expr(fill),
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    self.walk_expr(a);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    self.walk_expr(&op.value);
                }
            }
            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let InterpStrPart::Expr(e) = p {
                        self.walk_expr(e);
                    }
                }
            }
            ExprKind::Ident(n) => {
                if n == "self" {
                    if let Some(st) = self.self_type {
                        self.spots.push((e.span, st.clone(), "self".to_string()));
                    }
                } else if let Some(t) = self.env.get(n) {
                    self.spots
                        .push((e.span, t.clone(), "local".to_string()));
                }
            }
            ExprKind::IntLit(..)
            | ExprKind::FloatLit(..)
            | ExprKind::BoolLit(..)
            | ExprKind::StrLit(..)
            | ExprKind::CStrLit(..)
            | ExprKind::Path { .. }
            | ExprKind::IncludeBytes { .. }
            | ExprKind::IncludeStr { .. }
            | ExprKind::EnvVar { .. } => {}
        }
    }

    /// Classify a call's callee and add a `Calls` edge (and a `Reference` at
    /// the call site) if its target resolves uniquely; otherwise bump the
    /// unresolved count.
    fn resolve_call(&mut self, callee: &Expr, span: Span) {
        let target = match &callee.kind {
            ExprKind::Ident(name) => self.resolve_fn_name(name),
            ExprKind::Path { segments } => self.resolve_path(segments),
            ExprKind::Field { receiver, name } => match self.receiver_type(receiver) {
                Some(ty) => unique(self.method_idx.get(&(ty, short_name(&name.name).to_string()))),
                None => None,
            },
            // Calling a non-name expression (a fn-pointer value, an index, …):
            // genuinely indirect, can't be resolved to a symbol.
            _ => None,
        };
        self.link(target, span);
    }

    /// Resolve a (possibly qualified) free-function callee name to a node id.
    /// The resolver rewrites a call's callee to its qualified dotted form
    /// (`src.main.helper`); a bare name (`main`, or a lib-entry export) stays
    /// short. Three tiers, most-precise first:
    ///   1. exact short match (`fn_by_name["main"]`, unique) — bare callees;
    ///   2. qualified-id match (`fn_by_qualified["src.main.helper"]`) — the
    ///      common case, and the only one that disambiguates a short-name
    ///      collision across modules;
    ///   3. short-name fallback (`fn_by_name[short_name(...)]`, unique) — a
    ///      safety net for any qualified form whose `::`→`.` reconstruction
    ///      didn't line up, accepted only when the short name is unambiguous.
    /// A miss at all three is a genuine non-resolution (e.g. a fn-pointer
    /// indirection) and is counted as `unresolved`.
    fn resolve_fn_name(&self, name: &str) -> Option<String> {
        unique(self.fn_by_name.get(name))
            .or_else(|| self.fn_by_qualified.get(name).cloned())
            .or_else(|| unique(self.fn_by_name.get(short_name(name))))
    }

    /// `Type::assoc()` (associated fn / method) first, then `module::free_fn()`.
    fn resolve_path(&self, segments: &[crate::ast::Ident]) -> Option<String> {
        if segments.is_empty() {
            return None;
        }
        let last = short_name(&segments[segments.len() - 1].name).to_string();
        if segments.len() >= 2 {
            let prev = short_name(&segments[segments.len() - 2].name).to_string();
            if let Some(id) = unique(self.method_idx.get(&(prev, last.clone()))) {
                return Some(id);
            }
        }
        // Try the qualified dotted join of all segments before the short
        // fallback, so a `module::free_fn` path resolves uniquely too.
        let joined: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        self.fn_by_qualified
            .get(&joined.join("."))
            .cloned()
            .or_else(|| unique(self.fn_by_name.get(&last)))
    }

    /// Record every named-type occurrence in a type for later resolution.
    fn record_type(&mut self, ty: &Type) {
        let mut names = Vec::new();
        collect_type_names(ty, &mut names);
        self.type_refs.extend(names);
    }

    fn receiver_type(&self, recv: &Expr) -> Option<String> {
        match &recv.kind {
            ExprKind::Ident(n) if n == "self" => self.self_type.clone(),
            ExprKind::Ident(n) => self.env.get(n).cloned(),
            _ => None,
        }
    }

    fn link(&mut self, target: Option<String>, span: Span) {
        match target {
            Some(id) => {
                self.edges.push(Edge {
                    from: self.from_id.to_string(),
                    to: id.clone(),
                    kind: EdgeKind::Calls,
                });
                self.refs.push((id, span));
            }
            None => self.unresolved += 1,
        }
    }
}

/// Render a function signature: `fn name(p: T, ...) -> R`.
fn fn_signature(f: &Function) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, type_to_string(&p.ty)))
        .collect();
    let ret = f
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", type_to_string(t)))
        .unwrap_or_default();
    let kw = if f.is_extern { "extern fn" } else { "fn" };
    format!("{kw} {}({}){ret}", f.name.name, params.join(", "))
}

/// Render a method signature, including the receiver form.
fn method_signature(m: &Method) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = m.receiver {
        parts.push(
            match r {
                Receiver::Read => "this",
                Receiver::Mut => "ref this",
                Receiver::Move => "take this",
            }
            .to_string(),
        );
    }
    for p in &m.params {
        parts.push(format!("{}: {}", p.name.name, type_to_string(&p.ty)));
    }
    let ret = m
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", type_to_string(t)))
        .unwrap_or_default();
    format!("fn {}({}){ret}", m.name.name, parts.join(", "))
}

/// The last `.`-separated segment of a (possibly resolver-qualified) name.
/// The resolver rewrites cross-file type names to a dotted file-path-qualified
/// form (`...vendor.stdlib.src.option.Option`); for display we want the source
/// spelling the user writes (`Option`). Names without a dot pass through.
fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

// ---- v0.0.15 graph value-depth: scope-aware value-flow walk ----

/// One accumulating binding definition: its name, definition span, kind
/// (`parameter` / `local` / `match binding` / `loop variable`), and the
/// classified uses resolved to it.
struct DefAccum {
    name: String,
    span: Span,
    kind: &'static str,
    uses: Vec<(Span, FlowKind)>,
}

/// v0.0.15 precise scoping: a single scope-tracking walk over a function body.
/// Every binding use resolves to the *innermost* in-scope definition, so a
/// shadowed name (`let x = a; { let x = b; use(x) }`) attributes each use to the
/// right definition, and `match`-arm bindings / `for`-range loop variables are
/// first-class definitions. Replaces the previous order-blind, name-grouped
/// matching that conflated all same-named bindings in a function.
struct ScopedFlows {
    /// Every definition seen, in source order. Records are never removed —
    /// popping a scope ends a name's *visibility*, not its accumulated flow.
    defs: Vec<DefAccum>,
    /// Visibility stack: name -> index into `defs`. Inner scopes shadow outer.
    scopes: Vec<BTreeMap<String, usize>>,
}

impl ScopedFlows {
    fn new() -> Self {
        ScopedFlows {
            defs: Vec::new(),
            scopes: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, span: Span, kind: &'static str) {
        let idx = self.defs.len();
        self.defs.push(DefAccum {
            name: name.to_string(),
            span,
            kind,
            uses: Vec::new(),
        });
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name.to_string(), idx);
        }
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn record_use(&mut self, name: &str, span: Span, flow: FlowKind) {
        if let Some(idx) = self.resolve(name) {
            self.defs[idx].uses.push((span, flow));
        }
    }

    fn walk_block(&mut self, b: &Block, tail_ctx: FlowKind) {
        self.push_scope();
        for s in &b.stmts {
            self.walk_stmt(s);
        }
        if let Some(t) = &b.tail {
            self.walk_expr(t, tail_ctx);
        }
        self.pop_scope();
    }

    fn walk_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            // The initializer is evaluated *before* the new binding enters
            // scope, so walk it first (this is what makes `let x = x;` resolve
            // the RHS to the outer `x`), then define the name.
            StmtKind::Let { name, init, .. } => {
                if let Some(e) = init {
                    self.walk_expr(e, FlowKind::Assign);
                }
                self.define(&name.name, name.span, "local");
            }
            StmtKind::Return(Some(e)) => self.walk_expr(e, FlowKind::Return),
            StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
                self.walk_expr(e, FlowKind::Read)
            }
            StmtKind::While { cond, body, .. } => {
                self.walk_expr(cond, FlowKind::Read);
                self.walk_block(body, FlowKind::Read);
            }
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    // The C-style header binding (`for (let i = 0; ...)`) is
                    // visible to cond/update/body but not after the loop, so it
                    // lives in its own scope.
                    self.push_scope();
                    if let Some(init_stmt) = init {
                        self.walk_stmt(init_stmt);
                    }
                    if let Some(c) = cond {
                        self.walk_expr(c, FlowKind::Read);
                    }
                    for u in update {
                        self.walk_expr(u, FlowKind::Read);
                    }
                    self.walk_block(body, FlowKind::Read);
                    self.pop_scope();
                }
                ForLoop::Range { var, iter, body } => {
                    self.walk_expr(iter, FlowKind::Read);
                    self.push_scope();
                    self.define(&var.name, var.span, "loop variable");
                    self.walk_block(body, FlowKind::Read);
                    self.pop_scope();
                }
            },
            StmtKind::Loop(b, _) => self.walk_block(b, FlowKind::Read),
            _ => {}
        }
    }

    fn walk_expr(&mut self, e: &Expr, ctx: FlowKind) {
        match &e.kind {
            ExprKind::Ident(n) => self.record_use(n, e.span, ctx),
            ExprKind::Call { callee, args, .. } => {
                // Enum/assoc constructor (`E::V(x)`) vs a real call: a Path
                // callee is a constructor, so its args are a "re-wrap"; an
                // Ident/Field callee is a function/method call.
                let arg_ctx = if matches!(callee.kind, ExprKind::Path { .. }) {
                    FlowKind::Construct
                } else {
                    FlowKind::Call
                };
                // A method receiver (`x.m(...)`) reads x; a bare fn name is not
                // a local value, so don't descend into an Ident/Path callee.
                if let ExprKind::Field { receiver, .. } = &callee.kind {
                    self.walk_expr(receiver, FlowKind::Read);
                }
                for a in args {
                    self.walk_expr(a, arg_ctx);
                }
            }
            ExprKind::StructLit { fields, .. } | ExprKind::InferredStructLit { fields } | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    self.walk_expr(&f.value, FlowKind::Construct);
                }
            }
            ExprKind::GenericEnumCall { args, .. } => {
                for a in args {
                    self.walk_expr(a, FlowKind::Construct);
                }
            }
            ExprKind::ArrayLit { elements } => {
                for el in elements {
                    self.walk_expr(el, FlowKind::Construct);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr(scrutinee, FlowKind::Match);
                for a in arms {
                    // Each arm is its own scope: the pattern's bindings (whole-
                    // scrutinee `Binding` and one-level `Variant` payloads) are
                    // definitions visible only inside that arm's body.
                    self.push_scope();
                    self.define_pattern(&a.pattern);
                    self.walk_expr(&a.body, FlowKind::Read);
                    self.pop_scope();
                }
            }
            ExprKind::Field { receiver, .. } => self.walk_expr(receiver, FlowKind::Read),
            ExprKind::Index { receiver, index } => {
                self.walk_expr(receiver, FlowKind::Read);
                self.walk_expr(index, FlowKind::Read);
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.walk_expr(lhs, FlowKind::Read);
                self.walk_expr(rhs, FlowKind::Read);
            }
            ExprKind::Unary { operand, .. } => self.walk_expr(operand, FlowKind::Read),
            ExprKind::Cast { expr, .. } => self.walk_expr(expr, ctx),
            ExprKind::Assign { target, value, .. } => {
                // The assignee place (target) is a write, not a value read of
                // the binding; record only the RHS value flow.
                let _ = target;
                self.walk_expr(value, FlowKind::Assign);
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.walk_block(b, ctx),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.walk_expr(cond, FlowKind::Read);
                self.walk_block(then, ctx);
                if let Some(eb) = else_branch {
                    self.walk_expr(eb, ctx);
                }
            }
            ExprKind::Await(inner) | ExprKind::Yield(inner) => {
                self.walk_expr(inner, FlowKind::Read)
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.walk_expr(s, FlowKind::Read);
                }
                if let Some(en) = end {
                    self.walk_expr(en, FlowKind::Read);
                }
            }
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    self.walk_expr(a, FlowKind::Call);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    self.walk_expr(&op.value, FlowKind::Call);
                }
            }
            _ => {}
        }
    }

    /// Define every binding a `match` arm pattern introduces. Phase-3I patterns
    /// are one nesting level, so payloads are `Wildcard` or `Binding`.
    fn define_pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Binding(name) => self.define(&name.name, name.span, "match binding"),
            PatternKind::Variant { payload, .. } => {
                for pp in payload {
                    if let PatternKind::Binding(name) = &pp.kind {
                        self.define(&name.name, name.span, "match binding");
                    }
                }
            }
            PatternKind::Wildcard => {}
        }
    }
}

/// v0.0.15 inter-procedural flow: collect every `let BINDING = f(...)` in `b`
/// (including nested blocks / `if` / `match` arms / loops), as
/// `(callee_fn_name, binding_name, binding_name_span)`. Only direct
/// free-function `Ident` callees are reported (optionally through a trailing
/// `as` cast); method calls and block-wrapped results are skipped.
fn collect_let_call_dests(b: &Block, out: &mut Vec<(String, String, Span)>) {
    for s in &b.stmts {
        collect_let_call_dests_stmt(s, out);
    }
    if let Some(t) = &b.tail {
        collect_let_call_dests_expr(t, out);
    }
}

fn collect_let_call_dests_stmt(s: &Stmt, out: &mut Vec<(String, String, Span)>) {
    match &s.kind {
        StmtKind::Let {
            name,
            init: Some(e),
            ..
        } => {
            if let Some(callee) = callee_fn_name(e) {
                out.push((callee, name.name.clone(), name.span));
            }
            collect_let_call_dests_expr(e, out);
        }
        StmtKind::Return(Some(e))
        | StmtKind::Expr(e)
        | StmtKind::Defer(e)
        | StmtKind::Assert(e) => collect_let_call_dests_expr(e, out),
        StmtKind::While { body, .. } => collect_let_call_dests(body, out),
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle { body, .. } | ForLoop::Range { body, .. } => {
                collect_let_call_dests(body, out)
            }
        },
        StmtKind::Loop(b, _) => collect_let_call_dests(b, out),
        _ => {}
    }
}

fn collect_let_call_dests_expr(e: &Expr, out: &mut Vec<(String, String, Span)>) {
    match &e.kind {
        ExprKind::Block(b) | ExprKind::Unsafe(b) => collect_let_call_dests(b, out),
        ExprKind::If {
            then, else_branch, ..
        } => {
            collect_let_call_dests(then, out);
            if let Some(eb) = else_branch {
                collect_let_call_dests_expr(eb, out);
            }
        }
        ExprKind::Match { arms, .. } => {
            for a in arms {
                collect_let_call_dests_expr(&a.body, out);
            }
        }
        _ => {}
    }
}

/// The called free-function name if `e` is a direct `f(...)` call (an `Ident`
/// callee), seen through a trailing `as` cast. `None` for method calls,
/// constructors (`Path` callees), and non-call expressions.
fn callee_fn_name(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            _ => None,
        },
        ExprKind::Cast { expr, .. } => callee_fn_name(expr),
        _ => None,
    }
}

/// Render an AST type back to its source spelling. Uses source names, never a
/// monomorphized form.
pub fn type_to_string(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path(s) => short_name(s).to_string(),
        TypeKind::Array { elem, len, .. } => format!("[{}; {len}]", type_to_string(elem)),
        TypeKind::Borrowed { region, inner } => {
            format!("borrow {region} {}", type_to_string(inner))
        }
        TypeKind::Generic { name, args } => {
            let parts: Vec<String> = args.iter().map(type_to_string).collect();
            format!("{}[{}]", short_name(name), parts.join(", "))
        }
        TypeKind::RawPtr(inner) => format!("*{}", type_to_string(inner)),
        TypeKind::FnPtr {
            params,
            param_takes,
            return_type,
        } => {
            let parts: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let mark = if param_takes.get(i).copied().unwrap_or(false) {
                        "take "
                    } else {
                        ""
                    };
                    format!("{mark}{}", type_to_string(p))
                })
                .collect();
            let ret = return_type
                .as_ref()
                .map(|t| format!(" -> {}", type_to_string(t)))
                .unwrap_or_default();
            format!("fn({}){ret}", parts.join(", "))
        }
        TypeKind::Slice(inner) => format!("{}[]", type_to_string(inner)),
        TypeKind::Tuple(ts) => {
            let parts: Vec<String> = ts.iter().map(type_to_string).collect();
            format!("({})", parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a single-file `LoadedProject` from source, under file id `src`.
    fn project(src: &str) -> LoadedProject {
        let program = parse_ok(src);
        let mut files = BTreeMap::new();
        files.insert(
            "src".to_string(),
            (PathBuf::from("src/main.cplus"), src.to_string()),
        );
        LoadedProject {
            program,
            entry_file_id: "src".to_string(),
            files,
        }
    }

    /// Parse source and stamp every item's `origin_file` to `src`, mimicking
    /// what the resolver bakes in for a real project load.
    fn parse_ok(src: &str) -> crate::ast::Program {
        let toks = crate::lexer::tokenize(src).expect("lex");
        let mut program = crate::parser::parse(toks).expect("parse");
        for item in &mut program.items {
            item.origin_file = Some("src".to_string());
        }
        program
    }

    fn node<'a>(g: &'a CodeGraph, id: &str) -> &'a Node {
        g.nodes.iter().find(|n| n.id == id).expect("node present")
    }

    #[test]
    fn function_node_has_signature_and_defines_edge() {
        let g = CodeGraph::build(&project("fn add(a: i32, b: i32) -> i32 { return a +% b; }"));
        let n = node(&g, "src::add");
        assert_eq!(n.kind, NodeKind::Function);
        assert_eq!(n.name, "add");
        assert_eq!(n.signature.as_deref(), Some("fn add(a: i32, b: i32) -> i32"));
        assert!(n.location.is_some(), "function resolves to a location");
        assert!(g
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::Defines && e.from == "src" && e.to == "src::add"));
    }

    #[test]
    fn struct_fields_become_nodes_with_has_field_edges() {
        // v0.0.24 #10: field visibility is name-based — `x` is public, `_y` is
        // module-private.
        let g = CodeGraph::build(&project(
            "struct Point { x: i32, _y: i32 }",
        ));
        let s = node(&g, "src::Point");
        assert_eq!(s.kind, NodeKind::Struct);
        let fx = node(&g, "src::Point::x");
        assert_eq!(fx.kind, NodeKind::Field);
        assert_eq!(fx.signature.as_deref(), Some("i32"));
        assert!(fx.is_pub);
        assert!(!node(&g, "src::Point::_y").is_pub);
        let members = g.members("Point");
        assert_eq!(members.len(), 2, "Point has two fields");
    }

    #[test]
    fn impl_methods_attach_to_their_type() {
        let src = "struct Counter { v: i32 }\n\
                   impl Counter {\n\
                     fn read(this) -> i32 { return this.v; }\n\
                     fn inc(ref this) { this.v = this.v +% 1; }\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let read = node(&g, "src::Counter::read");
        assert_eq!(read.kind, NodeKind::Method);
        assert_eq!(read.signature.as_deref(), Some("fn read(this) -> i32"));
        let inc = node(&g, "src::Counter::inc");
        assert_eq!(inc.signature.as_deref(), Some("fn inc(ref this)"));
        assert!(g
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::HasMethod
                && e.from == "src::Counter"
                && e.to == "src::Counter::read"));
        // members() reaches both methods.
        let names: Vec<&str> = g.members("Counter").iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"read") && names.contains(&"inc"));
    }

    #[test]
    fn enum_variants_become_nodes() {
        let g = CodeGraph::build(&project("enum Shape { Circle(i32), Square(i32, i32), Empty }"));
        assert_eq!(node(&g, "src::Shape").kind, NodeKind::Enum);
        let sq = node(&g, "src::Shape::Square");
        assert_eq!(sq.kind, NodeKind::Variant);
        assert_eq!(sq.signature.as_deref(), Some("(i32, i32)"));
        assert_eq!(node(&g, "src::Shape::Empty").signature, None);
    }

    #[test]
    fn def_matches_qualified_and_bare_names() {
        let g = CodeGraph::build(&project("fn area(r: i32) -> i32 { return r *% r; }"));
        assert_eq!(g.def("src::area").len(), 1);
        assert_eq!(g.def("area").len(), 1);
        assert_eq!(g.def("nonexistent").len(), 0);
    }

    #[test]
    fn symbols_filters_by_file_and_skips_modules() {
        let g = CodeGraph::build(&project("fn a() {}\nfn b() {}\nstruct S { x: i32 }"));
        // Whole-project: a, b, S, and S::x — but no module node.
        assert!(g.symbols(None).iter().all(|n| n.kind != NodeKind::Module));
        assert!(g.symbols(Some("src")).iter().any(|n| n.name == "a"));
        assert_eq!(g.symbols(Some("nope")).len(), 0);
    }

    #[test]
    fn raw_pointer_and_generic_types_render() {
        let g = CodeGraph::build(&project("struct Buf { ptr: *u8, items: Vec[i32] }"));
        assert_eq!(node(&g, "src::Buf::ptr").signature.as_deref(), Some("*u8"));
        assert_eq!(
            node(&g, "src::Buf::items").signature.as_deref(),
            Some("Vec[i32]")
        );
    }

    #[test]
    fn qualified_type_names_render_to_their_source_segment() {
        use crate::lexer::Span;
        let z = Span::new(0, 0);
        let inner = Type {
            kind: TypeKind::Path("Uuid".into()),
            span: z,
        };
        let generic = Type {
            kind: TypeKind::Generic {
                name: "a.b.vendor.stdlib.src.option.Option".into(),
                args: vec![inner],
            },
            span: z,
        };
        assert_eq!(type_to_string(&generic), "Option[Uuid]");
        let path = Type {
            kind: TypeKind::Path("x.y.Foo".into()),
            span: z,
        };
        assert_eq!(type_to_string(&path), "Foo");
    }

    #[test]
    fn json_roundtrips_to_a_nodes_edges_object() {
        let g = CodeGraph::build(&project("fn f() {}"));
        let json = g.to_json();
        assert!(json.contains("\"nodes\""));
        assert!(json.contains("\"edges\""));
        assert!(json.contains("src::f"));
    }

    // ---- call edges (Phase 3) ----

    fn has_call(g: &CodeGraph, from: &str, to: &str) -> bool {
        g.edges
            .iter()
            .any(|e| e.kind == EdgeKind::Calls && e.from == from && e.to == to)
    }

    #[test]
    fn free_function_call_edge() {
        let g = CodeGraph::build(&project(
            "fn helper() -> i32 { return 1; }\n\
             fn main() -> i32 { return helper(); }",
        ));
        assert!(has_call(&g, "src::main", "src::helper"));
        let callees: Vec<&str> = g.callees("main").iter().map(|n| n.name.as_str()).collect();
        assert!(callees.contains(&"helper"));
        let callers: Vec<&str> = g.callers("helper").iter().map(|n| n.name.as_str()).collect();
        assert!(callers.contains(&"main"));
    }

    #[test]
    fn self_method_call_resolves_to_impl_target() {
        let src = "struct Counter { v: i32 }\n\
                   impl Counter {\n\
                     fn inc(ref this) { this.bump(); }\n\
                     fn bump(ref this) { this.v = this.v +% 1; }\n\
                   }";
        let g = CodeGraph::build(&project(src));
        assert!(has_call(&g, "src::Counter::inc", "src::Counter::bump"));
    }

    #[test]
    fn typed_local_method_call_resolves() {
        let src = "struct Point { x: i32 }\n\
                   impl Point { fn mag(this) -> i32 { return this.x; } }\n\
                   fn run() -> i32 { let p: Point = Point { x: 3 }; return p.mag(); }";
        let g = CodeGraph::build(&project(src));
        assert!(has_call(&g, "src::run", "src::Point::mag"));
    }

    #[test]
    fn associated_call_resolves() {
        let src = "struct Point { x: i32 }\n\
                   impl Point { fn origin() -> Point { return Point { x: 0 }; } }\n\
                   fn run() -> i32 { let p: Point = Point::origin(); return p.x; }";
        let g = CodeGraph::build(&project(src));
        assert!(has_call(&g, "src::run", "src::Point::origin"));
    }

    #[test]
    fn unresolved_receiver_is_counted_never_a_wrong_edge() {
        // The receiver of `.go()` is a call result with no locally-known type,
        // so it must be counted unresolved, not mis-linked.
        let src = "struct W { x: i32 }\n\
                   impl W {\n\
                     fn make() -> W { return W { x: 0 }; }\n\
                     fn go(this) -> i32 { return this.x; }\n\
                   }\n\
                   fn run() -> i32 { return W::make().go(); }";
        let g = CodeGraph::build(&project(src));
        assert!(has_call(&g, "src::run", "src::W::make"));
        assert!(!has_call(&g, "src::run", "src::W::go"));
        assert!(g.unresolved_calls.get("src::run").copied().unwrap_or(0) >= 1);
        assert!(g.unresolved_total() >= 1);
    }

    #[test]
    fn call_hierarchy_is_transitive_and_depth_bounded() {
        let src = "fn c() -> i32 { return 0; }\n\
                   fn b() -> i32 { return c(); }\n\
                   fn a() -> i32 { return b(); }";
        let g = CodeGraph::build(&project(src));
        let deep: Vec<&str> = g
            .call_hierarchy("a", 3)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(deep.contains(&"b") && deep.contains(&"c"));
        let shallow: Vec<&str> = g
            .call_hierarchy("a", 1)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(shallow.contains(&"b") && !shallow.contains(&"c"));
    }

    #[test]
    fn callees_json_carries_unresolved_and_handles_missing() {
        // `mystery()` has no node → an unresolved call site.
        let src = "fn helper() {}\n\
                   fn main() -> i32 { helper(); return mystery(); }";
        let g = CodeGraph::build(&project(src));
        let j = g.callees_json("main").expect("main is found");
        assert!(j.contains("\"unresolved\""));
        assert!(j.contains("helper"));
        assert!(g.callees_json("nonexistent").is_none());
    }

    // ---- reference edges (Phase 4a: call-site references) ----

    #[test]
    fn call_sites_become_references_with_locations() {
        let src = "fn helper() -> i32 { return 1; }\n\
                   fn a() -> i32 { return helper(); }\n\
                   fn b() -> i32 { return helper(); }";
        let g = CodeGraph::build(&project(src));
        let refs = g.refs("helper");
        assert_eq!(refs.len(), 2, "two call sites reference helper");
        assert!(refs.iter().all(|r| r.symbol == "src::helper"));
        assert!(refs.iter().all(|r| r.kind == RefKind::Call));
        // Distinct use-site locations, each carrying its enclosing context.
        let lines: BTreeSet<u32> = refs.iter().map(|r| r.location.line).collect();
        assert_eq!(lines.len(), 2, "two distinct call-site lines");
        let ctxs: BTreeSet<&str> = refs
            .iter()
            .filter_map(|r| r.in_context.as_deref())
            .collect();
        assert!(ctxs.contains("src::a") && ctxs.contains("src::b"));
    }

    #[test]
    fn refs_json_carries_scope_and_handles_missing() {
        let src = "fn helper() {}\nfn main() -> i32 { helper(); return 0; }";
        let g = CodeGraph::build(&project(src));
        let j = g.refs_json("helper").expect("helper is a known symbol");
        assert!(j.contains("\"scope\""), "refs carries a coverage scope");
        assert!(j.contains("calls"));
        assert!(j.contains("\"references\""));
        assert!(g.refs_json("nonexistent").is_none());
    }

    #[test]
    fn type_uses_become_references() {
        let src = "struct Widget { x: i32 }\n\
                   struct Holder { w: Widget }\n\
                   fn make(w: Widget) -> Widget { let local: Widget = w; return local; }";
        let g = CodeGraph::build(&project(src));
        let refs = g.refs("Widget");
        // Widget is referenced in: Holder's field, make's param, make's return,
        // and the `let local: Widget` annotation — four type uses.
        assert!(refs.iter().all(|r| r.kind == RefKind::Type));
        assert!(
            refs.len() >= 4,
            "expected >=4 Widget type refs, got {}",
            refs.len()
        );
        assert!(refs.iter().all(|r| r.in_context.is_some()));
        // A primitive type produces no reference (no node to point at).
        assert!(g.refs("i32").is_empty());
    }

    #[test]
    fn struct_literal_construction_is_a_type_reference() {
        let src = "struct P { x: i32 }\n\
                   fn mk() -> P { return P { x: 1 }; }";
        let g = CodeGraph::build(&project(src));
        // `P { x: 1 }` constructs P, plus the `-> P` return type = 2 refs.
        let refs = g.refs("P");
        assert!(refs.len() >= 2, "P referenced by ctor + return: {}", refs.len());
        assert!(refs.iter().any(|r| r.in_context.as_deref() == Some("src::mk")));
    }

    #[test]
    fn refs_of_uncalled_symbol_is_empty_but_found() {
        let src = "fn lonely() {}\nfn main() -> i32 { return 0; }";
        let g = CodeGraph::build(&project(src));
        // `lonely` exists (so json is Some) but has no call sites.
        assert!(g.refs("lonely").is_empty());
        assert!(g.refs_json("lonely").is_some());
    }

    #[test]
    fn context_packs_target_callers_and_callees() {
        let src = "fn leaf() -> i32 { return 1; }\n\
                   fn mid() -> i32 { return leaf(); }\n\
                   fn top() -> i32 { return mid(); }";
        let g = CodeGraph::build(&project(src));
        let j = g.context_json("mid").expect("mid is a function");
        assert!(j.contains("\"kind\": \"context\""));
        // mid is called by top and calls leaf.
        assert!(j.contains("\"callers\""));
        assert!(j.contains("\"callees\""));
        assert!(j.contains("top"), "top is a caller of mid: {j}");
        assert!(j.contains("leaf"), "leaf is a callee of mid: {j}");
        assert!(j.contains("\"type_refs\""), "context carries the types touched");
        // Not a function → None.
        assert!(g.context_json("nonexistent").is_none());
    }

    #[test]
    fn context_includes_referenced_types() {
        let src = "struct Cfg { n: i32 }\n\
                   fn run(c: Cfg) -> i32 { return c.n; }";
        let g = CodeGraph::build(&project(src));
        let j = g.context_json("run").expect("run is a function");
        assert!(j.contains("\"type_refs\""));
        assert!(j.contains("Cfg"), "context surfaces the Cfg type run touches: {j}");
    }

    // ---- type-at ----

    #[test]
    fn byte_offset_is_char_accurate() {
        let src = "ab\ncde\n";
        assert_eq!(byte_offset(src, 1, 1), Some(0)); // 'a'
        assert_eq!(byte_offset(src, 1, 2), Some(1)); // 'b'
        assert_eq!(byte_offset(src, 2, 1), Some(3)); // 'c' (after "ab\n")
        assert_eq!(byte_offset(src, 2, 3), Some(5)); // 'e'
        assert_eq!(byte_offset(src, 0, 1), None);
    }

    #[test]
    fn type_at_resolves_params_locals_and_self() {
        let src = "struct Point { x: i32 }\n\
                   impl Point { fn mag(this) -> i32 { return this.x; } }\n\
                   fn run(p: Point) -> i32 { let q: Point = p; return q.x; }";
        let g = CodeGraph::build(&project(src));

        // `self` inside mag → Point.
        let self_spot = g
            .type_spots
            .iter()
            .find(|s| s.what == "self")
            .expect("a self spot exists");
        assert_eq!(self_spot.ty, "Point");

        // The `p` parameter of run.
        let param = g
            .type_spots
            .iter()
            .find(|s| s.what == "parameter" && s.ty == "Point")
            .expect("p parameter spot");
        // type_at at the param's own start byte resolves to it.
        let at = g.type_at(&param.fid, param.span.start).expect("spot at param");
        assert_eq!(at.ty, "Point");
        assert_eq!(at.what, "parameter");

        // The `q` local (typed let).
        assert!(g
            .type_spots
            .iter()
            .any(|s| s.what == "local" && s.ty == "Point"));

        // A byte in dead space (e.g. offset 0, the `struct` keyword) has no spot.
        assert!(g.type_at("src", 0).is_none());
    }

    #[test]
    fn type_at_resolves_inferred_expressions() {
        // v0.0.14 value-depth: a `mk()` call result and a `p.x` field read are
        // inferred (not annotated at the use site), so only sema-retention sees
        // them. They show up as `expression` spots.
        let src = "struct Point { x: i32 }\n\
                   fn mk() -> Point { return Point { x: 7 }; }\n\
                   fn run() -> i32 { let p: Point = mk(); return p.x; }";
        let g = CodeGraph::build(&project(src));
        assert!(
            g.type_spots
                .iter()
                .any(|s| s.what == "expression" && s.ty == "Point"),
            "expected an inferred Point expression spot (the mk() call)"
        );
        assert!(
            g.type_spots
                .iter()
                .any(|s| s.what == "expression" && s.ty == "i32"),
            "expected an inferred i32 expression spot (the p.x read)"
        );
        // type_at at the inferred Point spot's own start byte resolves to it.
        let mk_call = g
            .type_spots
            .iter()
            .find(|s| s.what == "expression" && s.ty == "Point")
            .unwrap();
        let at = g.type_at(&mk_call.fid, mk_call.span.start).unwrap();
        assert_eq!(at.ty, "Point");
    }

    #[test]
    fn value_refs_classifies_binding_flow() {
        let src = "struct Box { v: i32 }\n\
                   enum Opt { Some(Box), None }\n\
                   fn consume(b: Box) -> i32 { return b.v; }\n\
                   fn run(p: Box) -> i32 {\n\
                       let q: Box = p;\n\
                       let o: Opt = Opt::Some(q);\n\
                       let code: i32 = match o { Opt::Some(b) => consume(b), Opt::None => 0 };\n\
                       return code;\n\
                   }";
        let g = CodeGraph::build(&project(src));

        // `p` (param) flows into `q` via the `let` initializer → Assign.
        let p = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "p" && vf.def_kind == "parameter")
            .expect("p binding");
        assert!(
            p.uses.iter().any(|u| u.flow == FlowKind::Assign),
            "p should flow as Assign, got {:?}",
            p.uses
        );

        // `q` is re-wrapped into `Opt::Some(q)` → Construct.
        let q = g.value_flows.iter().find(|vf| vf.binding == "q").expect("q");
        assert!(
            q.uses.iter().any(|u| u.flow == FlowKind::Construct),
            "q should flow as Construct, got {:?}",
            q.uses
        );

        // `o` is the scrutinee of a `match` → Match.
        let o = g.value_flows.iter().find(|vf| vf.binding == "o").expect("o");
        assert!(
            o.uses.iter().any(|u| u.flow == FlowKind::Match),
            "o should flow as Match, got {:?}",
            o.uses
        );

        // `consume`'s parameter `b` is read (`b.v`).
        let b = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "b" && vf.def_kind == "parameter")
            .expect("b param");
        assert!(
            b.uses.iter().any(|u| u.flow == FlowKind::Read),
            "b param should be read (b.v), got {:?}",
            b.uses
        );

        // v0.0.15: the match-arm payload binding `b` in `run` is now its own
        // definition, flowing into `consume(b)` → Call.
        let arm_b = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "b" && vf.def_kind == "match binding")
            .expect("b match binding");
        assert!(
            arm_b.uses.iter().any(|u| u.flow == FlowKind::Call),
            "match-arm b should flow as Call into consume(b), got {:?}",
            arm_b.uses
        );

        // value_refs at p's definition returns p's flow.
        let vf = g.value_refs(&p.fid, p.def_span.start).expect("value-refs at p");
        assert_eq!(vf.binding, "p");
    }

    // ---- v0.0.15: value-refs precise scoping ----

    #[test]
    fn value_refs_shadowed_name_resolves_to_innermost_def() {
        // Two `let x` bindings shadow each other; each use must attribute to the
        // right definition, not all `x` defs (the old shadow-naive behavior).
        let src = "fn f(p: i32) -> i32 {\n\
                       let x: i32 = p;\n\
                       let outer: i32 = x;\n\
                       let x: i32 = outer;\n\
                       let inner: i32 = x;\n\
                       return inner;\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let xs: Vec<&ValueFlow> = g
            .value_flows
            .iter()
            .filter(|vf| vf.binding == "x")
            .collect();
        assert_eq!(xs.len(), 2, "expected two distinct `x` definitions");
        // Each `x` definition has exactly one use (the `let` that reads it),
        // never both — proof the uses aren't conflated across shadows.
        for x in &xs {
            assert_eq!(
                x.uses.len(),
                1,
                "each shadowed `x` should own exactly its in-scope use, got {:?}",
                x.uses
            );
        }
        // The first `x` is read by `let outer = x`; resolving value-refs at the
        // `outer` initializer's `x` use lands on the first `x`, not the second.
        let first_x = xs.iter().min_by_key(|vf| vf.def_span.start).unwrap();
        let use_span = first_x.uses[0].span;
        let vf = g.value_refs(&first_x.fid, use_span.start).expect("value-refs");
        assert_eq!(vf.def_span, first_x.def_span, "use resolved to wrong def");
    }

    #[test]
    fn value_refs_match_binding_is_a_definition() {
        // A match-arm payload binding is a first-class definition with its own
        // classified uses.
        let src = "struct Box { v: i32 }\n\
                   enum Opt { Some(Box), None }\n\
                   fn use_it(b: Box) -> i32 { return b.v; }\n\
                   fn run(o: Opt) -> i32 {\n\
                       let code: i32 = match o { Opt::Some(inner) => use_it(inner), Opt::None => 0 };\n\
                       return code;\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let inner = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "inner" && vf.def_kind == "match binding")
            .expect("inner match binding");
        assert!(
            inner.uses.iter().any(|u| u.flow == FlowKind::Call),
            "match binding `inner` should flow as Call, got {:?}",
            inner.uses
        );
    }

    #[test]
    fn value_refs_for_range_loop_var_is_a_definition() {
        // A `for x in ...` loop variable is a definition scoped to the loop.
        let src = "fn sum(n: i32) -> i32 {\n\
                       var total: i32 = 0;\n\
                       for v in 0..n {\n\
                           total = total +% v;\n\
                       }\n\
                       return total;\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let v = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "v" && vf.def_kind == "loop variable")
            .expect("loop variable v");
        // `v` is read as an operand of `total +% v`.
        assert!(
            v.uses.iter().any(|u| u.flow == FlowKind::Read),
            "loop var v should be read in `total +% v`, got {:?}",
            v.uses
        );
    }

    #[test]
    fn value_refs_interprocedural_return_dest() {
        // A binding returned from `make` flows, in the caller, into the `let`
        // that captures `make(...)`'s result.
        let src = "fn make(seed: i32) -> i32 {\n\
                       let r: i32 = seed;\n\
                       return r;\n\
                   }\n\
                   fn caller() -> i32 {\n\
                       let got: i32 = make(7);\n\
                       return got;\n\
                   }";
        let g = CodeGraph::build(&project(src));
        // `r` in `make` escapes via return; its value lands in `got` in caller.
        let r = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "r")
            .expect("r binding");
        assert!(
            r.returns_into.iter().any(|d| d.binding == "got"),
            "r should return into caller binding `got`, got {:?}",
            r.returns_into
        );
    }

    #[test]
    fn value_refs_no_return_dest_when_result_discarded() {
        // No `let` captures the call result, so there is no inter-procedural
        // destination to record.
        let src = "fn make(seed: i32) -> i32 {\n\
                       let r: i32 = seed;\n\
                       return r;\n\
                   }\n\
                   fn caller() -> i32 {\n\
                       make(7);\n\
                       return 0;\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let r = g
            .value_flows
            .iter()
            .find(|vf| vf.binding == "r")
            .expect("r binding");
        assert!(
            r.returns_into.is_empty(),
            "discarded result should record no destination, got {:?}",
            r.returns_into
        );
    }
}
