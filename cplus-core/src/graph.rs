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

use crate::ast::{Function, ImplBlock, ItemKind, Method, Receiver, Type, TypeKind};
use crate::diagnostics::LineMap;
use crate::resolver::LoadedProject;
use serde::Serialize;
use std::collections::BTreeMap;

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
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl CodeGraph {
    /// Build the index from a resolved project. Pure over the AST + per-file
    /// source map; runs after the resolver, independent of sema and codegen.
    pub fn build(proj: &LoadedProject) -> CodeGraph {
        let mut g = CodeGraph::default();

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
                        is_pub: f.is_pub,
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
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
                        is_pub: s.is_pub,
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
                            is_pub: field.is_pub,
                        });
                        g.edges.push(Edge {
                            from: id.clone(),
                            to: fid_id,
                            kind: EdgeKind::HasField,
                        });
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
                        is_pub: e.is_pub,
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
                            is_pub: e.is_pub,
                        });
                        g.edges.push(Edge {
                            from: id.clone(),
                            to: v_id,
                            kind: EdgeKind::HasVariant,
                        });
                    }
                }
                ItemKind::Impl(b) => {
                    add_impl_methods(&mut g, &fid, b, &resolve);
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
                        is_pub: it.is_pub,
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
                        is_pub: a.is_pub,
                    });
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
                        is_pub: c.is_pub,
                    });
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
                        is_pub: s.is_pub,
                    });
                    g.edges.push(Edge {
                        from: fid.clone(),
                        to: id,
                        kind: EdgeKind::Defines,
                    });
                }
            }
        }

        g
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
}

fn add_impl_methods(
    g: &mut CodeGraph,
    fid: &str,
    b: &ImplBlock,
    resolve: &impl Fn(&str, crate::lexer::Span) -> Option<Location>,
) {
    let target = short_name(&b.target.name);
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
            is_pub: m.is_pub,
        });
        g.edges.push(Edge {
            from: type_id.clone(),
            to: id,
            kind: EdgeKind::HasMethod,
        });
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
                Receiver::Read => "self",
                Receiver::Mut => "mut self",
                Receiver::Move => "move self",
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

/// Render an AST type back to its source spelling. Uses source names, never a
/// monomorphized form.
pub fn type_to_string(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path(s) => short_name(s).to_string(),
        TypeKind::Array { elem, len } => format!("[{}; {len}]", type_to_string(elem)),
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
            return_type,
        } => {
            let parts: Vec<String> = params.iter().map(type_to_string).collect();
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
        let g = CodeGraph::build(&project(
            "struct Point { pub x: i32, y: i32 }",
        ));
        let s = node(&g, "src::Point");
        assert_eq!(s.kind, NodeKind::Struct);
        let fx = node(&g, "src::Point::x");
        assert_eq!(fx.kind, NodeKind::Field);
        assert_eq!(fx.signature.as_deref(), Some("i32"));
        assert!(fx.is_pub);
        assert!(!node(&g, "src::Point::y").is_pub);
        let members = g.members("Point");
        assert_eq!(members.len(), 2, "Point has two fields");
    }

    #[test]
    fn impl_methods_attach_to_their_type() {
        let src = "struct Counter { v: i32 }\n\
                   impl Counter {\n\
                     fn read(self) -> i32 { return self.v; }\n\
                     fn inc(mut self) { self.v = self.v +% 1; }\n\
                   }";
        let g = CodeGraph::build(&project(src));
        let read = node(&g, "src::Counter::read");
        assert_eq!(read.kind, NodeKind::Method);
        assert_eq!(read.signature.as_deref(), Some("fn read(self) -> i32"));
        let inc = node(&g, "src::Counter::inc");
        assert_eq!(inc.signature.as_deref(), Some("fn inc(mut self)"));
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
}
