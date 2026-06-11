//! Slice 7GEN.5a: monomorphization pass for generic functions.
//!
//! Runs after sema. Consumes the `MonoInfo` sema produced (the set of
//! unique generic-fn instantiations + per-call-site type-arg mapping)
//! and emits a new `Program` where:
//!
//!   - Each unique `(generic_fn_name, [concrete_types])` pair becomes
//!     a synthesized concrete `Function` item with the type-param
//!     references in its signature and body rewritten to the concrete
//!     types. The synthesized fn's name is the mangled form (e.g.
//!     `identity__i32`).
//!   - Every `Call { callee: Ident(generic_fn_name), ... }` is
//!     rewritten so the callee names the mangled fn instead.
//!   - The original generic-fn templates are *removed* — they're no
//!     longer reachable through any call site, and codegen would have
//!     skipped them anyway. Removing them keeps the post-mono program
//!     uniform (every fn it contains is concrete).
//!
//! Scope kept narrow for the first cut:
//!   - Generic *functions* only — generic types (`struct Pair[A, B]`,
//!     `enum Option[T]`) parse but don't yet have an instantiation
//!     surface (no constructor sugar that names them with type args).
//!   - Inference rule: each generic param must appear as the top-level
//!     type of at least one parameter. Nested-only / return-only
//!     params fire E0500 at the call site (handled in sema).
//!   - No bound checking yet (E0502 deferred).
//!   - No turbofish `::[T]` syntax (every call must be type-inferable).
//!   - Generic methods inside `impl` blocks are deferred.
//!
//! The mangled name format follows the design note: `name__T1__T2`,
//! with each `Ti` rendered by `mangle_ty` (primitives use their
//! literal name, structs/enums use their `name`, arrays render as
//! `arrN_<elem>` so the structure is preserved without bracket
//! characters LLVM rejects in identifiers).

use crate::ast::*;
use crate::lexer::Span;
use crate::sema::{EnumId, MonoInfo, StructId, Ty};

/// Slice 7GEN.5c carry-forward (2026-05-13): generic-instantiation
/// lookup re-keyed by *mangled argument names* (vs sema's Vec<Ty> form).
/// Required because `subst_type_ast` operates on AST after recursion has
/// produced `Path("Box__i32")` for inner generics, and converting that
/// string back to the `Ty::Struct(id)` that sema used as a key would
/// require sema's id table — which doesn't survive the handoff. Indexing
/// by rendered name sidesteps the id round-trip.
struct StructLookup {
    by_names: std::collections::HashMap<(String, Vec<String>), String>,
}

/// Public entry point. Consumes the input `Program` and returns a new
/// `Program` with generic templates expanded into monomorphized
/// instances and call sites rewritten. Sema's type tables are needed
/// to render struct/enum names inside the mangled-name builder; the
/// caller passes a closure to look them up.
pub fn monomorphize(
    program: Program,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
) -> Program {
    // Phase 11 polish (2026-05-13): substitute type aliases first.
    // Every `TypeKind::Path(name)` where `name` is an alias gets
    // replaced by the alias target (recursively). Cycle detection
    // happened at sema, so we walk straight through here.
    let program = if mono.type_aliases.is_empty() {
        program
    } else {
        rewrite_aliases_in_program(program, &mono.type_aliases)
    };
    // Slice 7GEN.5c: synthesize concrete struct items for each generic
    // struct instantiation. These get appended to the output Program.
    // The lookup map below also drives the `TypeKind::Generic` →
    // `TypeKind::Path(mangled)` rewrite at use sites.
    let struct_lookup: std::collections::HashMap<(String, Vec<Ty>), String> = mono
        .struct_instantiations
        .iter()
        .map(|(key, info)| (key.clone(), info.mangled_name.clone()))
        .collect();
    // Slice 7GEN.5d: enum lookup. Combined with struct_lookup for the
    // type-position Generic→Path rewrite (so `Option[i32]` in a type
    // position resolves to the enum's mangled name).
    let enum_lookup: std::collections::HashMap<(String, Vec<Ty>), String> = mono
        .enum_instantiations
        .iter()
        .map(|(key, info)| (key.clone(), info.mangled_name.clone()))
        .collect();
    // Merge both into struct_lookup since subst_type_ast uses a single
    // map; sema already de-conflicts struct/enum names (E0301).
    let mut by_ty: std::collections::HashMap<(String, Vec<Ty>), String> = struct_lookup;
    for (k, v) in &enum_lookup {
        by_ty.insert(k.clone(), v.clone());
    }
    let by_names: std::collections::HashMap<(String, Vec<String>), String> = by_ty
        .iter()
        .map(|(k, v)| {
            let names: Vec<String> = k.1.iter().map(|t| mangle_ty(t, type_name_of)).collect();
            ((k.0.clone(), names), v.clone())
        })
        .collect();
    let struct_lookup = StructLookup { by_names };
    // v0.0.4 Phase 1B: propagate fn-instantiation set to a fixed point.
    //
    // Sema records each generic-fn call site's type-args once, using the
    // surrounding fn's type-parameter names where they appear. So
    // `make_buf[T]() -> Vec[T] { return vec::new::[T](); }` produces
    // `(make_buf, [i32])` from `main`'s `make_buf::[i32]()` call site and
    // `(vec::new, [Ty::Param("T")])` from inside `make_buf`'s body.
    //
    // Without propagation the latter never resolves to a concrete
    // instantiation — `vec_new__i32` is never synthesized — and codegen
    // panics looking up `sigs["vec::new"]` (the un-mangled name).
    //
    // Fix: walk each instantiation's body, substitute the outer subst
    // through recorded inner call args, and add the resolved
    // `(callee, concrete_args)` to the instantiation set. Iterate until
    // no new pair is produced.
    let propagated_instantiations = propagate_fn_instantiations(
        &program,
        &mono.instantiations,
        &mono.call_monos,
        &mono.struct_instantiations,
    );
    // Build the substitution context for each instantiation up front
    // so call-site rewriting and template-expansion share one source.
    let mut instances: Vec<MonoInstance> = Vec::new();
    for (name, args) in &propagated_instantiations {
        instances.push(MonoInstance {
            generic_name: name.clone(),
            concrete_args: args.clone(),
            mangled: mangle_name(name, args, type_name_of),
        });
    }
    // Walk the program's items: pass through everything except generic
    // fns; for those, swap each instantiation for a concrete-typed
    // clone. Also rewrite all `Call` sites whose callee is `Ident(name)`
    // matching a generic fn name to use the mangled name; we look up
    // the right instantiation by call_span.
    let generic_names: std::collections::HashSet<String> = propagated_instantiations
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let mut out_items: Vec<Item> = Vec::with_capacity(program.items.len() + instances.len());
    let inst_lookup: std::collections::HashMap<(String, Vec<Ty>), String> = instances
        .iter()
        .map(|i| {
            (
                (i.generic_name.clone(), i.concrete_args.clone()),
                i.mangled.clone(),
            )
        })
        .collect();
    for item in program.items {
        // Carry the current item's source file into the rewriter so the
        // `call_monos` lookup in `rewrite_expr` can supply the matching file
        // component — `Span` is file-less, and two inferred generic calls at
        // the same byte offset in different files would otherwise resolve to
        // the same (wrong) instantiation. Set once here so every rewrite path
        // below (synthesize_fn / synthesize_generic_typed_impls /
        // rewrite_item_calls) sees the right file. Synthesized generic-body
        // calls carry their template's file and find no `call_monos` entry
        // (sema doesn't record generic bodies), so this never mis-hits.
        *mono.call_mono_file.borrow_mut() = item.origin_file.clone();
        match &item.kind {
            ItemKind::Function(f) if !f.generic_params.is_empty() => {
                // Generic template — emit one synthesized concrete fn
                // per instantiation that targets this name; drop the
                // template itself.
                let template = f.clone();
                for inst in instances
                    .iter()
                    .filter(|i| i.generic_name == template.name.name)
                {
                    let subst = build_subst(&template.generic_params, &inst.concrete_args);
                    let synthesized = synthesize_fn(
                        &template,
                        inst,
                        &subst,
                        &generic_names,
                        &inst_lookup,
                        mono,
                        type_name_of,
                        &struct_lookup,
                    );
                    out_items.push(Item {
                        kind: ItemKind::Function(synthesized),
                        span: item.span,
                        origin_file: item.origin_file.clone(),
                    });
                }
                // Drop the generic template (no `out_items.push(item)`).
            }
            // Slice 7GEN.5c: generic struct template — drop, the
            // synthesized instantiations are emitted below.
            ItemKind::Struct(s) if !s.generic_params.is_empty() => {}
            // Slice 7GEN.5e step 3: generic-typed impl block — drop,
            // and emit one synthesized concrete ImplBlock per matching
            // struct_instantiation below.
            ItemKind::Impl(b) if !b.target_generic_params.is_empty() => {
                synthesize_generic_typed_impls(
                    &b,
                    item.span,
                    item.origin_file.clone(),
                    mono,
                    type_name_of,
                    &struct_lookup,
                    &generic_names,
                    &inst_lookup,
                    &mut out_items,
                );
            }
            _ => {
                let rewritten = rewrite_item_calls(
                    item,
                    &generic_names,
                    &inst_lookup,
                    mono,
                    type_name_of,
                    &struct_lookup,
                );
                out_items.push(rewritten);
            }
        }
    }
    // Slice 7GEN.5c: append synthesized concrete struct items for each
    // generic-struct instantiation. Field types are already concrete
    // `Ty` values from sema; render them back to AST `Type` nodes so
    // codegen's `collect_types` ingests them via the normal Phase-2
    // path.
    // Slice 7GEN.5d: append synthesized concrete enum items for each
    // generic-enum instantiation. Variant payload types are already
    // concrete `Ty` values from sema; render them back to AST `Type`
    // nodes.
    for ((_, _args), info) in &mono.enum_instantiations {
        let variants: Vec<EnumVariant> = info
            .variants
            .iter()
            .map(|v| EnumVariant {
                name: Ident {
                    name: v.name.clone(),
                    span: Span::new(0, 0),
                },
                payload: v
                    .payload
                    .iter()
                    .map(|t| ty_to_type_ast(t, type_name_of))
                    .collect(),
                span: Span::new(0, 0),
                attributes: Vec::new(),
            })
            .collect();
        let decl = EnumDecl {
            name: Ident {
                name: info.mangled_name.clone(),
                span: Span::new(0, 0),
            },
            variants,
            is_pub: true,
            attributes: Vec::new(),
            generic_params: Vec::new(),
        };
        out_items.push(Item {
            kind: ItemKind::Enum(decl),
            span: Span::new(0, 0),
            origin_file: info.template_origin_file.clone(),
        });
    }
    for ((_, _args), info) in &mono.struct_instantiations {
        let fields: Vec<StructField> = info
            .fields
            .iter()
            .map(|(name, ty, is_pub)| StructField {
                name: Ident {
                    name: name.clone(),
                    span: Span::new(0, 0),
                },
                ty: ty_to_type_ast(ty, type_name_of),
                span: Span::new(0, 0),
                is_pub: *is_pub,
                attributes: Vec::new(),
                // Accountability (E0510) is checked on the pre-mono source struct
                // in sema; this post-mono copy never re-runs the check, so the
                // flag is irrelevant here.
                is_opaque: false,
            })
            .collect();
        let decl = StructDecl {
            name: Ident {
                name: info.mangled_name.clone(),
                span: Span::new(0, 0),
            },
            fields,
            is_pub: true,
            attributes: Vec::new(),
            generic_params: Vec::new(),
        };
        out_items.push(Item {
            kind: ItemKind::Struct(decl),
            span: Span::new(0, 0),
            origin_file: info.template_origin_file.clone(),
        });
    }
    Program {
        items: out_items,
        imports: program.imports,
    }
}

/// v0.0.6 Slice 1B helper: render a lane scalar `Ty` to its source name
/// (`i32` → `"i32"`, etc.). Used only for SIMD type-name reconstruction;
/// the full `ty_to_type_ast` would over-recurse for our needs.
fn ty_to_source_name_for_simd(ty: &Ty) -> String {
    match ty {
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::U64 => "u64".into(),
        Ty::F16 => "f16".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        other => panic!(
            "non-numeric SIMD lane type reached AST reconstruction: {:?}",
            other
        ),
    }
}

/// Slice 7GEN.5c: render a `Ty` back to an AST `Type` node for
/// post-monomorphize emission. Primitives → `Path("i32")`, structs/enums
/// → `Path(<source_name>)`, arrays preserved. Param shouldn't appear
/// here (substitution has already happened).
fn ty_to_type_ast(ty: &Ty, type_name_of: &dyn Fn(&Ty) -> String) -> Type {
    let kind = match ty {
        Ty::I8 => TypeKind::Path("i8".into()),
        Ty::I16 => TypeKind::Path("i16".into()),
        Ty::I32 => TypeKind::Path("i32".into()),
        Ty::I64 => TypeKind::Path("i64".into()),
        Ty::U8 => TypeKind::Path("u8".into()),
        Ty::U16 => TypeKind::Path("u16".into()),
        Ty::U32 => TypeKind::Path("u32".into()),
        Ty::U64 => TypeKind::Path("u64".into()),
        Ty::Isize => TypeKind::Path("isize".into()),
        Ty::Usize => TypeKind::Path("usize".into()),
        Ty::F16 => TypeKind::Path("f16".into()),
        Ty::F32 => TypeKind::Path("f32".into()),
        Ty::F64 => TypeKind::Path("f64".into()),
        Ty::Bool => TypeKind::Path("bool".into()),
        Ty::Unit => TypeKind::Path("()".into()),
        Ty::Str => TypeKind::Path("str".into()),
        Ty::String => TypeKind::Path("string".into()),
        Ty::Slice(inner) => TypeKind::Slice(Box::new(ty_to_type_ast(inner, type_name_of))),
        Ty::RawPtr(inner) => TypeKind::RawPtr(Box::new(ty_to_type_ast(inner, type_name_of))),
        Ty::FnPtr {
            params,
            return_type,
        } => TypeKind::FnPtr {
            params: params
                .iter()
                .map(|p| ty_to_type_ast(p, type_name_of))
                .collect(),
            return_type: if matches!(**return_type, Ty::Unit) {
                None
            } else {
                Some(Box::new(ty_to_type_ast(return_type, type_name_of)))
            },
        },
        Ty::Struct(_) | Ty::Enum(_) => TypeKind::Path(type_name_of(ty)),
        Ty::Array(elem, n) => TypeKind::Array {
            elem: Box::new(ty_to_type_ast(elem, type_name_of)),
            len: *n,
            len_name: None,
        },
        // v0.0.6 Slice 1B: SIMD vector source name is `<elem>x<lanes>`,
        // e.g. `f32x4`. Mirrors the resolver's accepted form.
        Ty::Simd { elem, lanes } => {
            TypeKind::Path(format!("{}x{}", ty_to_source_name_for_simd(elem), lanes))
        }
        // Masks render back to the `mask<width>x<lanes>` source form
        // (e.g. `mask32x4`). The resolver canonicalises both ways.
        Ty::Mask { elem, lanes } => {
            let width: u32 = match elem.as_ref() {
                Ty::I8 => 8,
                Ty::I16 => 16,
                Ty::I32 => 32,
                Ty::I64 => 64,
                _ => 0,
            };
            TypeKind::Path(format!("mask{width}x{lanes}"))
        }
        Ty::Param(name) => TypeKind::Path(name.clone()),
        Ty::Error => TypeKind::Path("<error>".into()),
    };
    Type {
        kind,
        span: Span::new(0, 0),
    }
}

struct MonoInstance {
    generic_name: String,
    concrete_args: Vec<Ty>,
    mangled: String,
}

/// Slice 7GEN.5e step 3: emit one concrete `ImplBlock` per matching
/// struct instantiation. Substitutes the impl-level `target_generic_params`
/// (e.g. `T`) and `Self` references throughout the methods' signatures
/// and bodies. The synthesized impl block points at the mangled struct
/// name (`Vec__i32`) instead of the generic source name (`Vec`).
fn synthesize_generic_typed_impls(
    b: &ImplBlock,
    item_span: Span,
    origin_file: Option<String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    out_items: &mut Vec<Item>,
) {
    let target_name = b.target.name.clone();
    let impl_param_names: Vec<String> = b
        .target_generic_params
        .iter()
        .map(|g| g.name.name.clone())
        .collect();
    // For each instantiation of this generic struct OR enum, build a
    // fresh concrete impl block. Enum instantiations carry the same
    // shape (mangled_name + concrete args) so both paths route through
    // the same per-instantiation rewriting below.
    let struct_pairs: Vec<(String, Vec<Ty>, String)> = mono
        .struct_instantiations
        .iter()
        .map(|((n, a), info)| (n.clone(), a.clone(), info.mangled_name.clone()))
        .collect();
    let enum_pairs: Vec<(String, Vec<Ty>, String)> = mono
        .enum_instantiations
        .iter()
        .map(|((n, a), info)| (n.clone(), a.clone(), info.mangled_name.clone()))
        .collect();
    let all_pairs = struct_pairs.into_iter().chain(enum_pairs.into_iter());
    for (sname, args, mangled_from_info) in all_pairs {
        if sname != target_name {
            continue;
        }
        if args.len() != impl_param_names.len() {
            continue;
        }
        let _ = &mangled_from_info; // kept for parity with prior shape
                                    // Re-resolve mangled name from the appropriate instantiation
                                    // map (we know `sname == target_name` and arities match).
        let info_mangled: String = mono
            .struct_instantiations
            .get(&(sname.clone(), args.clone()))
            .map(|i| i.mangled_name.clone())
            .or_else(|| {
                mono.enum_instantiations
                    .get(&(sname.clone(), args.clone()))
                    .map(|i| i.mangled_name.clone())
            })
            .expect("instantiation present (just iterated)");
        // Subst: impl-level T → concrete Ty; "Self" → Path(mangled)
        // handled separately by inserting Self into subst with the
        // concrete struct's Ty rendered by type_name_of.
        let mut subst: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
        for (gp, arg) in impl_param_names.iter().zip(args.iter()) {
            subst.insert(gp.clone(), arg.clone());
        }
        // For `Self` in method signatures/bodies, we need a Ty value
        // that subst_type_ast renders to the mangled name. We don't
        // have a StructId here; use a synthetic Ty::Param entry that
        // type_name_of-of would not handle, then handle "Self" as a
        // special Path name in subst_type_ast via a name-level rewrite.
        // Simpler: pre-rewrite Self → Path(mangled) by walking AST.
        let mangled_name = info_mangled.clone();
        let mut new_methods: Vec<Method> = Vec::with_capacity(b.methods.len());
        for m in &b.methods {
            let mut m2 = m.clone();
            // Substitute impl-level T in param types + return type.
            for p in &mut m2.params {
                p.ty = rewrite_self_in_type(
                    &subst_type_ast(&p.ty, &subst, type_name_of, struct_lookup),
                    &mangled_name,
                );
            }
            if let Some(rt) = &mut m2.return_type {
                *rt = rewrite_self_in_type(
                    &subst_type_ast(rt, &subst, type_name_of, struct_lookup),
                    &mangled_name,
                );
            }
            // Rewrite body: subst T → concrete, Self → mangled.
            m2.body = rewrite_block_with_self(
                &m2.body,
                &subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
                &mangled_name,
            );
            new_methods.push(m2);
        }
        let new_impl = ImplBlock {
            target: Ident {
                name: mangled_name.clone(),
                span: b.target.span,
            },
            target_generic_params: Vec::new(),
            methods: new_methods,
            interface_name: b.interface_name.clone(),
            is_unsafe: b.is_unsafe,
        };
        out_items.push(Item {
            kind: ItemKind::Impl(new_impl),
            span: item_span,
            origin_file: origin_file.clone(),
        });
    }
}

/// Slice 7GEN.5e step 3: rewrite `Path("Self")` references to
/// `Path(mangled_name)` inside an AST `Type`. Run after subst_type_ast
/// (so impl-level Generic-args are already lowered).
fn rewrite_self_in_type(ty: &Type, mangled_name: &str) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) if name == "Self" => TypeKind::Path(mangled_name.to_string()),
        TypeKind::Path(name) => TypeKind::Path(name.clone()),
        TypeKind::Array { elem, len, .. } => TypeKind::Array {
            elem: Box::new(rewrite_self_in_type(elem, mangled_name)),
            len: *len,
            len_name: None,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(rewrite_self_in_type(inner, mangled_name)),
        },
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| rewrite_self_in_type(a, mangled_name))
                .collect(),
        },
        TypeKind::RawPtr(inner) => {
            TypeKind::RawPtr(Box::new(rewrite_self_in_type(inner, mangled_name)))
        }
        TypeKind::FnPtr {
            params,
            return_type,
        } => TypeKind::FnPtr {
            params: params
                .iter()
                .map(|p| rewrite_self_in_type(p, mangled_name))
                .collect(),
            return_type: return_type
                .as_ref()
                .map(|rt| Box::new(rewrite_self_in_type(rt, mangled_name))),
        },
        TypeKind::Slice(inner) => {
            TypeKind::Slice(Box::new(rewrite_self_in_type(inner, mangled_name)))
        }
        TypeKind::Tuple(elems) => TypeKind::Tuple(
            elems
                .iter()
                .map(|t| rewrite_self_in_type(t, mangled_name))
                .collect(),
        ),
    };
    Type {
        kind,
        span: ty.span,
    }
}

/// Slice 7GEN.5e step 3: walk a method body, recursively applying
/// the existing rewrite_block logic AND replacing any `Path("Self")`
/// type references with `Path(mangled)`. This is used only when
/// monomorphizing methods inside generic-typed impl blocks.
fn rewrite_block_with_self(
    block: &Block,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
    mangled_name: &str,
) -> Block {
    // First run the generic rewrite that handles subst + generic-fn
    // call-site rewriting, then do a second pass that replaces Self.
    let pass1 = rewrite_block(
        block,
        subst,
        generic_names,
        inst_lookup,
        mono,
        type_name_of,
        struct_lookup,
    );
    rewrite_block_self(&pass1, mangled_name)
}

fn rewrite_block_self(block: &Block, mangled_name: &str) -> Block {
    Block {
        stmts: block
            .stmts
            .iter()
            .map(|s| rewrite_stmt_self(s, mangled_name))
            .collect(),
        tail: block
            .tail
            .as_ref()
            .map(|e| Box::new(rewrite_expr_self(e, mangled_name))),
        span: block.span,
    }
}

fn rewrite_stmt_self(stmt: &Stmt, mangled_name: &str) -> Stmt {
    let kind = match &stmt.kind {
        StmtKind::Let {
            mutable,
            name,
            ty,
            init,
        } => StmtKind::Let {
            mutable: *mutable,
            name: name.clone(),
            ty: ty.as_ref().map(|t| rewrite_self_in_type(t, mangled_name)),
            init: init.as_ref().map(|e| rewrite_expr_self(e, mangled_name)),
        },
        StmtKind::Expr(e) => StmtKind::Expr(rewrite_expr_self(e, mangled_name)),
        StmtKind::Return(e) => {
            StmtKind::Return(e.as_ref().map(|e| rewrite_expr_self(e, mangled_name)))
        }
        StmtKind::While { cond, body, attributes } => StmtKind::While {
            cond: rewrite_expr_self(cond, mangled_name),
            body: rewrite_block_self(body, mangled_name),
            attributes: attributes.clone(),
        },
        StmtKind::For(forloop, attributes) => StmtKind::For(
            rewrite_for_self(forloop, mangled_name),
            attributes.clone(),
        ),
        other => other.clone(),
    };
    Stmt {
        kind,
        span: stmt.span,
    }
}

fn rewrite_for_self(f: &ForLoop, mangled_name: &str) -> ForLoop {
    match f {
        ForLoop::Range { var, iter, body } => ForLoop::Range {
            var: var.clone(),
            iter: rewrite_expr_self(iter, mangled_name),
            body: rewrite_block_self(body, mangled_name),
        },
        ForLoop::CStyle {
            init,
            cond,
            update,
            body,
        } => ForLoop::CStyle {
            init: init
                .as_ref()
                .map(|s| Box::new(rewrite_stmt_self(s, mangled_name))),
            cond: cond.as_ref().map(|e| rewrite_expr_self(e, mangled_name)),
            update: update
                .iter()
                .map(|e| rewrite_expr_self(e, mangled_name))
                .collect(),
            body: rewrite_block_self(body, mangled_name),
        },
    }
}

fn rewrite_expr_self(expr: &Expr, mangled_name: &str) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Path { segments } if segments.len() == 1 && segments[0].name == "Self" => {
            ExprKind::Path {
                segments: vec![Ident {
                    name: mangled_name.to_string(),
                    span: segments[0].span,
                }],
            }
        }
        // Most expressions don't carry types, so the only thing we
        // really need to chase is nested blocks/cast/etc.
        ExprKind::Block(b) => ExprKind::Block(rewrite_block_self(b, mangled_name)),
        ExprKind::Unsafe(b) => ExprKind::Unsafe(rewrite_block_self(b, mangled_name)),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => ExprKind::If {
            cond: Box::new(rewrite_expr_self(cond, mangled_name)),
            then: rewrite_block_self(then, mangled_name),
            else_branch: else_branch
                .as_ref()
                .map(|e| Box::new(rewrite_expr_self(e, mangled_name))),
        },
        ExprKind::Cast { expr: inner, ty } => ExprKind::Cast {
            expr: Box::new(rewrite_expr_self(inner, mangled_name)),
            ty: rewrite_self_in_type(ty, mangled_name),
        },
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => ExprKind::Call {
            callee: Box::new(rewrite_expr_self(callee, mangled_name)),
            args: args
                .iter()
                .map(|a| rewrite_expr_self(a, mangled_name))
                .collect(),
            type_args: type_args
                .iter()
                .map(|t| rewrite_self_in_type(t, mangled_name))
                .collect(),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op: *op,
            lhs: Box::new(rewrite_expr_self(lhs, mangled_name)),
            rhs: Box::new(rewrite_expr_self(rhs, mangled_name)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(rewrite_expr_self(operand, mangled_name)),
        },
        ExprKind::Field { receiver, name } => ExprKind::Field {
            receiver: Box::new(rewrite_expr_self(receiver, mangled_name)),
            name: name.clone(),
        },
        ExprKind::Index { receiver, index } => ExprKind::Index {
            receiver: Box::new(rewrite_expr_self(receiver, mangled_name)),
            index: Box::new(rewrite_expr_self(index, mangled_name)),
        },
        ExprKind::Assign { op, target, value } => ExprKind::Assign {
            op: *op,
            target: Box::new(rewrite_expr_self(target, mangled_name)),
            value: Box::new(rewrite_expr_self(value, mangled_name)),
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: Box::new(rewrite_expr_self(scrutinee, mangled_name)),
            arms: arms
                .iter()
                .map(|a| MatchArm {
                    pattern: a.pattern.clone(),
                    body: rewrite_expr_self(&a.body, mangled_name),
                    span: a.span,
                })
                .collect(),
        },
        ExprKind::StructLit { name, fields } => {
            let new_name = if name.name == "Self" {
                Ident {
                    name: mangled_name.to_string(),
                    span: name.span,
                }
            } else {
                name.clone()
            };
            ExprKind::StructLit {
                name: new_name,
                fields: fields
                    .iter()
                    .map(|f| StructLitField {
                        name: f.name.clone(),
                        value: rewrite_expr_self(&f.value, mangled_name),
                        span: f.span,
                    })
                    .collect(),
            }
        }
        ExprKind::ArrayLit { elements } => ExprKind::ArrayLit {
            elements: elements
                .iter()
                .map(|e| rewrite_expr_self(e, mangled_name))
                .collect(),
        },
        ExprKind::ArrayFill { fill, count, .. } => ExprKind::ArrayFill {
            fill: Box::new(rewrite_expr_self(fill, mangled_name)),
            count: *count,
            count_name: None,
        },
        ExprKind::Intrinsic { name, type_args, args, ret_ty } => ExprKind::Intrinsic {
            name: name.clone(),
            type_args: type_args.clone(),
            args: args.iter().map(|a| rewrite_expr_self(a, mangled_name)).collect(),
            ret_ty: ret_ty.clone(),
        },
        other => other.clone(),
    };
    Expr {
        kind,
        span: expr.span,
    }
}

/// Build the param-name → concrete-type substitution for a single
/// instantiation. `generic_params` order matches `concrete_args` order.
/// v0.0.4 Phase 1B: Resolve a turbofish `Type` (AST) to a `Ty` (sema-level)
/// using the surrounding instantiation's substitution map. Returns
/// `None` for type-arg shapes monomorphize can't fully resolve without
/// sema's type-id table (Struct / Enum references — those would need
/// the StructLookup-by-name lookup, which is not threaded here).
///
/// This handles primitives + Param substitution + pointer/slice/array
/// recursion. The propagation pass uses it to discover transitive
/// generic-fn instantiations without forcing sema to type-check generic
/// fn bodies (which has its own set of complications around qualified
/// names and intrinsic checks).
fn type_ast_to_ty_with_subst(
    t: &Type,
    subst: &std::collections::HashMap<String, Ty>,
) -> Option<Ty> {
    match &t.kind {
        TypeKind::Path(name) => {
            if let Some(concrete) = subst.get(name) {
                return Some(concrete.clone());
            }
            match name.as_str() {
                "i8" => Some(Ty::I8),
                "i16" => Some(Ty::I16),
                "i32" => Some(Ty::I32),
                "i64" => Some(Ty::I64),
                "u8" => Some(Ty::U8),
                "u16" => Some(Ty::U16),
                "u32" => Some(Ty::U32),
                "u64" => Some(Ty::U64),
                "isize" => Some(Ty::Isize),
                "usize" => Some(Ty::Usize),
                "f16" => Some(Ty::F16),
                "f32" => Some(Ty::F32),
                "f64" => Some(Ty::F64),
                "bool" => Some(Ty::Bool),
                "()" => Some(Ty::Unit),
                "str" => Some(Ty::Str),
                "string" => Some(Ty::String),
                // Struct / enum names: monomorphize doesn't carry sema's
                // id table. Skip — these instantiations get discovered
                // via the struct_instantiations path instead.
                _ => None,
            }
        }
        TypeKind::RawPtr(inner) => {
            type_ast_to_ty_with_subst(inner, subst).map(|t| Ty::RawPtr(Box::new(t)))
        }
        TypeKind::Slice(inner) => {
            type_ast_to_ty_with_subst(inner, subst).map(|t| Ty::Slice(Box::new(t)))
        }
        TypeKind::Array { elem, len, .. } => {
            type_ast_to_ty_with_subst(elem, subst).map(|t| Ty::Array(Box::new(t), *len))
        }
        TypeKind::FnPtr {
            params,
            return_type,
        } => {
            let params: Option<Vec<Ty>> = params
                .iter()
                .map(|p| type_ast_to_ty_with_subst(p, subst))
                .collect();
            let ret = match return_type {
                Some(rt) => type_ast_to_ty_with_subst(rt, subst)?,
                None => Ty::Unit,
            };
            Some(Ty::FnPtr {
                params: params?,
                return_type: Box::new(ret),
            })
        }
        _ => None,
    }
}

/// v0.0.4 Phase 1B: Does `ty` contain any `Ty::Param(...)` reference?
/// Used to filter out generic-context fn_instantiation entries that
/// sema records when type-checking a generic fn body — those are not
/// real concrete monomorphs and only become real after substitution
/// through an outer caller's subst.
fn ty_contains_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Array(elem, _) | Ty::Slice(elem) | Ty::RawPtr(elem) => ty_contains_param(elem),
        Ty::FnPtr {
            params,
            return_type,
        } => params.iter().any(ty_contains_param) || ty_contains_param(return_type),
        _ => false,
    }
}

/// v0.0.4 Phase 1B: Substitute `Ty::Param` references through `subst`
/// without triggering re-instantiation. Mirrors sema's `subst_ty_deep`
/// for the Param/Array/Slice/RawPtr/FnPtr branches but stops at
/// `Ty::Struct(id)` / `Ty::Enum(id)` — those carry sema-assigned ids
/// that monomorphize doesn't re-resolve here.
fn subst_ty_plain(ty: &Ty, subst: &std::collections::HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Param(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Array(elem, len) => Ty::Array(Box::new(subst_ty_plain(elem, subst)), *len),
        Ty::Slice(elem) => Ty::Slice(Box::new(subst_ty_plain(elem, subst))),
        Ty::RawPtr(inner) => Ty::RawPtr(Box::new(subst_ty_plain(inner, subst))),
        Ty::FnPtr {
            params,
            return_type,
        } => Ty::FnPtr {
            params: params.iter().map(|p| subst_ty_plain(p, subst)).collect(),
            return_type: Box::new(subst_ty_plain(return_type, subst)),
        },
        other => other.clone(),
    }
}

/// v0.0.4 Phase 1B: walk a body and call `f` with
/// `(callee_name, type_args, span)` for every `Call` whose callee is a
/// plain `Ident`. Used by the fn-instantiation propagation pass.
fn visit_ident_calls(expr: &Expr, f: &mut impl FnMut(&str, &[Type], crate::lexer::Span)) {
    match &expr.kind {
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            if let ExprKind::Ident(name) = &callee.kind {
                f(name, type_args, expr.span);
            }
            visit_ident_calls(callee, f);
            for a in args {
                visit_ident_calls(a, f);
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => visit_ident_calls_in_block(b, f),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            visit_ident_calls(cond, f);
            visit_ident_calls_in_block(then, f);
            if let Some(e) = else_branch {
                visit_ident_calls(e, f);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            visit_ident_calls(lhs, f);
            visit_ident_calls(rhs, f);
        }
        ExprKind::Unary { operand, .. } => visit_ident_calls(operand, f),
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                visit_ident_calls(s, f);
            }
            if let Some(e) = end {
                visit_ident_calls(e, f);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            visit_ident_calls(target, f);
            visit_ident_calls(value, f);
        }
        ExprKind::Field { receiver, .. } => visit_ident_calls(receiver, f),
        ExprKind::Index { receiver, index } => {
            visit_ident_calls(receiver, f);
            visit_ident_calls(index, f);
        }
        ExprKind::Cast { expr, .. } => visit_ident_calls(expr, f),
        ExprKind::StructLit { fields, .. } => {
            for sf in fields {
                visit_ident_calls(&sf.value, f);
            }
        }
        ExprKind::GenericStructLit { fields, .. } => {
            for sf in fields {
                visit_ident_calls(&sf.value, f);
            }
        }
        ExprKind::ArrayLit { elements } => {
            for e in elements {
                visit_ident_calls(e, f);
            }
        }
        ExprKind::ArrayFill { fill, .. } => visit_ident_calls(fill, f),
        ExprKind::GenericEnumCall { args, .. } => {
            for a in args {
                visit_ident_calls(a, f);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            visit_ident_calls(scrutinee, f);
            for arm in arms {
                visit_ident_calls(&arm.body, f);
            }
        }
        ExprKind::Await(inner) => visit_ident_calls(inner, f),
        ExprKind::InterpStr { parts } => {
            for p in parts {
                if let InterpStrPart::Expr(e) = p {
                    visit_ident_calls(e, f);
                }
            }
        }
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                visit_ident_calls(a, f);
            }
        }
        _ => {}
    }
}

fn visit_ident_calls_in_block(
    block: &Block,
    f: &mut impl FnMut(&str, &[Type], crate::lexer::Span),
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { init: Some(e), .. } => visit_ident_calls(e, f),
            StmtKind::Let { init: None, .. } => {}
            StmtKind::Expr(e) => visit_ident_calls(e, f),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    visit_ident_calls(e, f);
                }
            }
            StmtKind::While { cond, body, .. } => {
                visit_ident_calls(cond, f);
                visit_ident_calls_in_block(body, f);
            }
            StmtKind::For(forloop, _) => match forloop {
                ForLoop::Range { iter, body, .. } => {
                    visit_ident_calls(iter, f);
                    visit_ident_calls_in_block(body, f);
                }
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(s) = init.as_deref() {
                        let wrap = Block {
                            stmts: vec![s.clone()],
                            tail: None,
                            span: stmt.span,
                        };
                        visit_ident_calls_in_block(&wrap, f);
                    }
                    if let Some(c) = cond {
                        visit_ident_calls(c, f);
                    }
                    for u in update {
                        visit_ident_calls(u, f);
                    }
                    visit_ident_calls_in_block(body, f);
                }
            },
            StmtKind::Defer(e) | StmtKind::Assert(e) => visit_ident_calls(e, f),
            StmtKind::Loop(body, _) => visit_ident_calls_in_block(body, f),
            // Lowering pass converts IfLet / WhileLet / GuardLet to
            // match/loop+match before monomorphize, but cover them
            // defensively in case a sample reaches us pre-lowering.
            StmtKind::IfLet {
                scrutinee,
                body,
                else_body,
                ..
            } => {
                visit_ident_calls(scrutinee, f);
                visit_ident_calls_in_block(body, f);
                if let Some(b) = else_body {
                    visit_ident_calls_in_block(b, f);
                }
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                visit_ident_calls(scrutinee, f);
                visit_ident_calls_in_block(body, f);
            }
            StmtKind::GuardLet { scrutinee, .. } => visit_ident_calls(scrutinee, f),
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    if let Some(t) = &block.tail {
        visit_ident_calls(t, f);
    }
}

/// v0.0.4 Phase 1B: fixed-point propagation of fn instantiations through
/// transitive generic calls. See the explanatory comment at the call site
/// in `monomorphize()`.
fn propagate_fn_instantiations(
    program: &Program,
    initial: &std::collections::BTreeSet<(String, Vec<Ty>)>,
    _call_monos: &std::collections::HashMap<(Option<String>, crate::lexer::Span), Vec<Ty>>,
    struct_instantiations: &std::collections::BTreeMap<
        (String, Vec<Ty>),
        crate::sema::StructInstantiationInfo,
    >,
) -> std::collections::BTreeSet<(String, Vec<Ty>)> {
    // Build template lookup: name -> &Function. Only generic templates.
    let templates: std::collections::HashMap<String, &Function> = program
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Function(f) if !f.generic_params.is_empty() => Some((f.name.name.clone(), f)),
            _ => None,
        })
        .collect();
    // Drop Param-bearing entries from the seed set. Sema records every
    // turbofish call (including those inside a generic body, where the
    // type-args are `Ty::Param`) into `fn_instantiations`. Those entries
    // don't name a real concrete monomorph — they're context-dependent
    // and only become real after substitution through an outer caller's
    // subst, which is the propagation step below.
    let is_concrete = |args: &Vec<Ty>| !args.iter().any(ty_contains_param);
    let mut out: std::collections::BTreeSet<(String, Vec<Ty>)> = initial
        .iter()
        .filter(|(_, args)| is_concrete(args))
        .cloned()
        .collect();
    let mut worklist: std::collections::VecDeque<(String, Vec<Ty>)> = out.iter().cloned().collect();
    while let Some((caller, caller_args)) = worklist.pop_front() {
        let Some(template) = templates.get(&caller) else {
            continue;
        };
        if template.generic_params.len() != caller_args.len() {
            continue;
        }
        let subst = build_subst(&template.generic_params, &caller_args);
        let mut discoveries: Vec<(String, Vec<Ty>)> = Vec::new();
        visit_ident_calls_in_block(&template.body, &mut |callee_name, type_args, _span| {
            // Only generic-fn templates need propagation. Plain
            // non-generic calls don't need monomorphization.
            if !templates.contains_key(callee_name) {
                return;
            }
            // Read the turbofish type-args directly from the AST. Sema
            // doesn't type-check generic-fn bodies in v0.0.4, so
            // `call_monos` is empty for these spans. The AST is the
            // ground truth here.
            if type_args.is_empty() {
                return;
            }
            let Some(resolved) = type_args
                .iter()
                .map(|t| type_ast_to_ty_with_subst(t, &subst))
                .collect::<Option<Vec<Ty>>>()
            else {
                return;
            };
            if !is_concrete(&resolved) {
                return;
            }
            discoveries.push((callee_name.to_string(), resolved));
        });
        for d in discoveries {
            if out.insert(d.clone()) {
                worklist.push_back(d);
            }
        }
    }
    // v0.0.5 Phase 2A: extend propagation through generic-impl-method
    // bodies. Phase 1B only walks generic-FREE-fn bodies, so a generic
    // method body like `HashMap[K, V]::get` calling `result::io_err::[V]`
    // never gets the propagated `(io_err, [i32])` entry when the user
    // instantiates `HashMap[i32, i32]`. Worked around in v0.0.4 by
    // inlining the constructor; this slice closes the gap.
    //
    // For each `struct_instantiation` (struct_name, concrete_args), find
    // the matching generic impl block, build the subst from
    // `target_generic_params → concrete_args`, walk each method body's
    // turbofish call sites, substitute, and feed concrete pairs to the
    // worklist for transitive discovery.
    let mut method_worklist: std::collections::VecDeque<(String, Vec<Ty>)> =
        std::collections::VecDeque::new();
    for ((sname, sargs), _info) in struct_instantiations {
        for item in &program.items {
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            if b.target_generic_params.is_empty() {
                continue;
            }
            if &b.target.name != sname {
                continue;
            }
            if b.target_generic_params.len() != sargs.len() {
                continue;
            }
            let subst: std::collections::HashMap<String, Ty> = b
                .target_generic_params
                .iter()
                .zip(sargs.iter())
                .map(|(gp, t)| (gp.name.name.clone(), t.clone()))
                .collect();
            for m in &b.methods {
                visit_ident_calls_in_block(&m.body, &mut |callee_name, type_args, _span| {
                    if !templates.contains_key(callee_name) {
                        return;
                    }
                    if type_args.is_empty() {
                        return;
                    }
                    let Some(resolved) = type_args
                        .iter()
                        .map(|t| type_ast_to_ty_with_subst(t, &subst))
                        .collect::<Option<Vec<Ty>>>()
                    else {
                        return;
                    };
                    if !is_concrete(&resolved) {
                        return;
                    }
                    let pair = (callee_name.to_string(), resolved);
                    if out.insert(pair.clone()) {
                        method_worklist.push_back(pair);
                    }
                });
            }
        }
    }
    // Drain the method-discovered set transitively — a freshly-discovered
    // generic-fn instantiation might call yet another generic fn in its
    // body. Same fixed-point shape as the main worklist above.
    while let Some((caller, caller_args)) = method_worklist.pop_front() {
        let Some(template) = templates.get(&caller) else {
            continue;
        };
        if template.generic_params.len() != caller_args.len() {
            continue;
        }
        let subst = build_subst(&template.generic_params, &caller_args);
        let mut discoveries: Vec<(String, Vec<Ty>)> = Vec::new();
        visit_ident_calls_in_block(&template.body, &mut |callee_name, type_args, _span| {
            if !templates.contains_key(callee_name) {
                return;
            }
            if type_args.is_empty() {
                return;
            }
            let Some(resolved) = type_args
                .iter()
                .map(|t| type_ast_to_ty_with_subst(t, &subst))
                .collect::<Option<Vec<Ty>>>()
            else {
                return;
            };
            if !is_concrete(&resolved) {
                return;
            }
            discoveries.push((callee_name.to_string(), resolved));
        });
        for d in discoveries {
            if out.insert(d.clone()) {
                method_worklist.push_back(d);
            }
        }
    }
    out
}

fn build_subst(
    generic_params: &[GenericParam],
    concrete_args: &[Ty],
) -> std::collections::HashMap<String, Ty> {
    generic_params
        .iter()
        .zip(concrete_args.iter())
        .map(|(g, t)| (g.name.name.clone(), t.clone()))
        .collect()
}

/// Synthesize a concrete-typed `Function` from a generic template by
/// substituting type-param references in the signature and body.
fn synthesize_fn(
    template: &Function,
    inst: &MonoInstance,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Function {
    Function {
        name: Ident {
            name: inst.mangled.clone(),
            span: template.name.span,
        },
        params: template
            .params
            .iter()
            .map(|p| Param {
                name: p.name.clone(),
                ty: subst_type_ast(&p.ty, subst, type_name_of, struct_lookup),
                mutable: p.mutable,
                move_: p.move_,
                restrict: p.restrict,
                borrow_: p.borrow_,
                span: p.span,
            })
            .collect(),
        return_type: template
            .return_type
            .as_ref()
            .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup)),
        body: rewrite_block(
            &template.body,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        ),
        is_pub: template.is_pub,
        is_extern: template.is_extern,
        is_variadic: template.is_variadic,
        attributes: template.attributes.clone(),
        generic_params: Vec::new(), // monomorphized — no longer generic
        is_async: template.is_async,
        is_gen: template.is_gen,
        is_unsafe: template.is_unsafe,
    }
}

/// Substitute type-param names inside an AST `Type` node. Recurses
/// into array element types and borrow-region wrappers.
fn subst_type_ast(
    ty: &Type,
    subst: &std::collections::HashMap<String, Ty>,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) => {
            if let Some(concrete) = subst.get(name) {
                // v0.0.3 Phase 5 Slice 5D: round-trip through
                // `ty_to_type_ast` so non-Path Tys (RawPtr, FnPtr,
                // Slice, Array) keep their AST structure. Substituting
                // `T` → `*u64` must produce `TypeKind::RawPtr(...)`,
                // not `TypeKind::Path("raw-pointer")` — the latter
                // codegens to `Ty::Error` since `raw-pointer` isn't a
                // valid type name. The original Path → Path mapping
                // works for primitives + structs/enums; the new path
                // covers the pointer/aggregate cases too.
                return ty_to_type_ast(concrete, type_name_of);
            }
            TypeKind::Path(name.clone())
        }
        TypeKind::Array { elem, len, .. } => TypeKind::Array {
            elem: Box::new(subst_type_ast(elem, subst, type_name_of, struct_lookup)),
            len: *len,
            len_name: None,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(subst_type_ast(inner, subst, type_name_of, struct_lookup)),
        },
        // Slice 7GEN.5c: rewrite `Pair[i32, bool]` to `Path("Pair__i32__bool")`.
        // First substitute fn-generic params in each arg (so `Pair[T]` inside
        // a `fn id[T]` becomes `Pair[i32]` for the i32-instantiation), then
        // look up the result in the struct-instantiation map sema produced.
        //
        // 7GEN.5c carry-forward (2026-05-13): nested generics like
        // `Pair[Box[T], i32]` need a *mangled-name-keyed* lookup because
        // monomorphize doesn't carry sema's id table to round-trip an
        // AST `Path("Box__i32")` back to `Ty::Struct(id)`. The names map
        // is built once at the top of `monomorphize()`.
        TypeKind::Generic { name, args } => {
            let resolved_args: Vec<Type> = args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            // Compute each arg's mangled name directly from the AST form
            // (post-recursion). This bypasses the Ty round-trip the prior
            // version did.
            let arg_names: Vec<String> = resolved_args.iter().map(mangle_type_ast_arg).collect();
            if let Some(mangled) = struct_lookup.by_names.get(&(name.clone(), arg_names)) {
                TypeKind::Path(mangled.clone())
            } else {
                // No matching instantiation — leave as-is and let
                // downstream surface the error (sema would have already).
                TypeKind::Generic {
                    name: name.clone(),
                    args: resolved_args,
                }
            }
        }
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(subst_type_ast(
            inner,
            subst,
            type_name_of,
            struct_lookup,
        ))),
        TypeKind::FnPtr {
            params,
            return_type,
        } => TypeKind::FnPtr {
            params: params
                .iter()
                .map(|p| subst_type_ast(p, subst, type_name_of, struct_lookup))
                .collect(),
            return_type: return_type
                .as_ref()
                .map(|rt| Box::new(subst_type_ast(rt, subst, type_name_of, struct_lookup))),
        },
        TypeKind::Slice(inner) => TypeKind::Slice(Box::new(subst_type_ast(
            inner,
            subst,
            type_name_of,
            struct_lookup,
        ))),
        // v0.0.5 Phase 3 Slice 3B: tuple type. Recurse first, then
        // look up the synthesized tuple struct under the synthetic
        // template name `"__Tuple"` (same key sema stored under).
        // Falls through unchanged if the lookup misses — sema would
        // have synthesized it on first encounter, so a miss here
        // means an out-of-band tuple type that won't codegen.
        TypeKind::Tuple(elems) => {
            let resolved: Vec<Type> = elems
                .iter()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup))
                .collect();
            let arg_names: Vec<String> = resolved.iter().map(mangle_type_ast_arg).collect();
            if let Some(mangled) = struct_lookup
                .by_names
                .get(&("__Tuple".to_string(), arg_names))
            {
                TypeKind::Path(mangled.clone())
            } else {
                TypeKind::Tuple(resolved)
            }
        }
    };
    Type {
        kind,
        span: ty.span,
    }
}

/// Rewrite a top-level item to update generic-fn call sites to their
/// mangled targets. Non-function items pass through; non-generic
/// function bodies have their calls rewritten too (a non-generic fn
/// can call a generic fn).
fn rewrite_item_calls(
    item: Item,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Item {
    let empty_subst = std::collections::HashMap::new();
    let kind = match item.kind {
        ItemKind::Function(mut f) => {
            // Slice 7GEN.5c: also rewrite Generic types in the signature.
            for p in &mut f.params {
                p.ty = subst_type_ast(&p.ty, &empty_subst, type_name_of, struct_lookup);
            }
            if let Some(rt) = &mut f.return_type {
                *rt = subst_type_ast(rt, &empty_subst, type_name_of, struct_lookup);
            }
            f.body = rewrite_block(
                &f.body,
                &empty_subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            );
            ItemKind::Function(f)
        }
        ItemKind::Impl(mut b) => {
            // Slice 7GEN.5e: for each generic method template inside
            // this impl, synthesize one concrete copy per matching
            // entry in mono.method_instantiations. Non-generic methods
            // just get their bodies rewritten in place.
            let target_name = b.target.name.clone();
            let mut new_methods: Vec<Method> = Vec::with_capacity(b.methods.len());
            for m in b.methods {
                if m.generic_params.is_empty() {
                    let mut m2 = m;
                    for p in &mut m2.params {
                        p.ty = subst_type_ast(&p.ty, &empty_subst, type_name_of, struct_lookup);
                    }
                    if let Some(rt) = &mut m2.return_type {
                        *rt = subst_type_ast(rt, &empty_subst, type_name_of, struct_lookup);
                    }
                    m2.body = rewrite_block(
                        &m2.body,
                        &empty_subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    );
                    new_methods.push(m2);
                } else {
                    // Synthesize one concrete method per instantiation.
                    for (sname, mname, args) in &mono.method_instantiations {
                        if sname != &target_name || mname != &m.name.name {
                            continue;
                        }
                        let subst = build_subst(&m.generic_params, args);
                        let mangled = mangle_name(&m.name.name, args, type_name_of);
                        let mut clone = m.clone();
                        clone.name = Ident {
                            name: mangled,
                            span: m.name.span,
                        };
                        clone.generic_params = Vec::new();
                        for p in &mut clone.params {
                            p.ty = subst_type_ast(&p.ty, &subst, type_name_of, struct_lookup);
                        }
                        if let Some(rt) = &mut clone.return_type {
                            *rt = subst_type_ast(rt, &subst, type_name_of, struct_lookup);
                        }
                        clone.body = rewrite_block(
                            &m.body,
                            &subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        );
                        new_methods.push(clone);
                    }
                    // Drop the template (no push).
                }
            }
            b.methods = new_methods;
            ItemKind::Impl(b)
        }
        ItemKind::Struct(mut s) => {
            // Non-generic struct with a Generic-typed field (e.g.
            // `struct Box { inner: Pair[i32, bool] }`). Rewrite each
            // field's type.
            for f in &mut s.fields {
                f.ty = subst_type_ast(&f.ty, &empty_subst, type_name_of, struct_lookup);
            }
            ItemKind::Struct(s)
        }
        // G-026 fix: non-generic enum with a Generic-typed variant
        // payload (e.g. `enum Value { Array(vec::Vec[Value]) }`).
        // Without this pass, the `TypeKind::Generic { name: "Vec",
        // args: [Value] }` AST node leaks past monomorphize and
        // codegen panics on "monomorphize did not rewrite this site".
        // Mirror the struct path — walk each variant's payload list
        // and rewrite generics to their mangled names.
        ItemKind::Enum(mut e) => {
            for v in &mut e.variants {
                for t in &mut v.payload {
                    *t = subst_type_ast(t, &empty_subst, type_name_of, struct_lookup);
                }
            }
            ItemKind::Enum(e)
        }
        other => other,
    };
    Item {
        kind,
        span: item.span,
        origin_file: item.origin_file,
    }
}

fn rewrite_block(
    block: &Block,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Block {
    Block {
        stmts: block
            .stmts
            .iter()
            .map(|s| {
                rewrite_stmt(
                    s,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                )
            })
            .collect(),
        tail: block.tail.as_ref().map(|e| {
            Box::new(rewrite_expr(
                e,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ))
        }),
        span: block.span,
    }
}

fn rewrite_stmt(
    stmt: &Stmt,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Stmt {
    let kind = match &stmt.kind {
        StmtKind::Let {
            mutable,
            name,
            ty,
            init,
        } => StmtKind::Let {
            mutable: *mutable,
            name: name.clone(),
            ty: ty
                .as_ref()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup)),
            init: init.as_ref().map(|e| {
                rewrite_expr(
                    e,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                )
            }),
        },
        StmtKind::Return(opt) => StmtKind::Return(opt.as_ref().map(|e| {
            rewrite_expr(
                e,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )
        })),
        StmtKind::While { cond, body, attributes } => StmtKind::While {
            cond: rewrite_expr(
                cond,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            body: rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            attributes: attributes.clone(),
        },
        StmtKind::For(forloop, attributes) => StmtKind::For(
            rewrite_for(
                forloop,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            attributes.clone(),
        ),
        StmtKind::Expr(e) => StmtKind::Expr(rewrite_expr(
            e,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        )),
        StmtKind::Defer(e) => StmtKind::Defer(rewrite_expr(
            e,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        )),
        StmtKind::Assert(e) => StmtKind::Assert(rewrite_expr(
            e,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        )),
        // `if let` / `guard let` / `while let` are lowered to plain match
        // before sema, but the lower pass runs *before* monomorphize
        // currently — we still see these. Rewrite their components.
        StmtKind::IfLet {
            pattern,
            scrutinee,
            body,
            else_body,
        } => StmtKind::IfLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(
                scrutinee,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            body: rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            else_body: else_body.as_ref().map(|b| {
                rewrite_block(
                    b,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                )
            }),
        },
        StmtKind::Break | StmtKind::Continue => stmt.kind.clone(),
        StmtKind::GuardLet {
            pattern,
            scrutinee,
            else_body,
            complement,
        } => StmtKind::GuardLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(
                scrutinee,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            else_body: rewrite_block(
                else_body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            complement: complement.clone(),
        },
        StmtKind::Loop(body, attributes) => StmtKind::Loop(
            rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            attributes.clone(),
        ),
        StmtKind::WhileLet {
            pattern,
            scrutinee,
            body,
        } => StmtKind::WhileLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(
                scrutinee,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            body: rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
        },
    };
    Stmt {
        kind,
        span: stmt.span,
    }
}

fn rewrite_for(
    f: &ForLoop,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> ForLoop {
    match f {
        ForLoop::Range { var, iter, body } => ForLoop::Range {
            var: var.clone(),
            iter: rewrite_expr(
                iter,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            body: rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
        },
        ForLoop::CStyle {
            init,
            cond,
            update,
            body,
        } => ForLoop::CStyle {
            init: init.as_ref().map(|s| {
                Box::new(rewrite_stmt(
                    s,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                ))
            }),
            cond: cond.as_ref().map(|e| {
                rewrite_expr(
                    e,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                )
            }),
            update: update
                .iter()
                .map(|e| {
                    rewrite_expr(
                        e,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    )
                })
                .collect(),
            body: rewrite_block(
                body,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
        },
    }
}

fn rewrite_expr(
    expr: &Expr,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            // Inspect the callee for a generic-fn / generic-method
            // dispatch and rewrite to the mangled name where applicable.
            // Slice 7GEN.5e: extended from generic-fn (Ident callee) only
            // to also include generic methods (Field callee) and
            // generic associated functions (Path callee).
            // Keyed by `(origin_file, span)` — the rewriter loop set
            // `call_mono_file` to this item's file so a same-offset call site
            // in another file can't shadow this one's recorded type-args.
            let call_mono_key = (mono.call_mono_file.borrow().clone(), expr.span);
            let args_for_call_opt = mono.call_monos.get(&call_mono_key);
            // v0.0.4 Phase 1B: resolve the call site's effective concrete
            // type-args. Two sources:
            //   - `call_monos`: sema's record (may contain `Ty::Param(T)`
            //     when this call is inside a non-generic enclosing fn
            //     that references a type-param of its impl block; or in
            //     mixed cases).
            //   - AST `type_args` (turbofish): direct user-supplied
            //     types. The propagation pass uses these to discover
            //     transitive generic instantiations inside generic-fn
            //     bodies (sema doesn't type-check those, so `call_monos`
            //     is empty for spans inside them).
            // In both cases the active `subst` resolves remaining Params
            // to the outer instantiation's concrete types.
            let resolved_args_for_call: Option<Vec<Ty>> = if let Some(args) = args_for_call_opt {
                Some(args.iter().map(|t| subst_ty_plain(t, subst)).collect())
            } else if !type_args.is_empty() {
                type_args
                    .iter()
                    .map(|t| type_ast_to_ty_with_subst(t, subst))
                    .collect()
            } else {
                None
            };
            let new_callee: Expr = match (&callee.kind, resolved_args_for_call.as_ref()) {
                // v0.0.19: a turbofish generic-fn call mangles its callee
                // directly from the (collision-free) AST type-args, never
                // consulting the file-less-span `call_monos`. See
                // `mangle_call_from_ast`.
                (ExprKind::Ident(cname), _)
                    if generic_names.contains(cname) && !type_args.is_empty() =>
                {
                    Expr {
                        kind: ExprKind::Ident(mangle_call_from_ast(
                            cname,
                            type_args,
                            subst,
                            type_name_of,
                            struct_lookup,
                        )),
                        span: callee.span,
                    }
                }
                (ExprKind::Ident(cname), Some(args_for_call)) if generic_names.contains(cname) => {
                    if let Some(mangled) = inst_lookup.get(&(cname.clone(), args_for_call.clone()))
                    {
                        Expr {
                            kind: ExprKind::Ident(mangled.clone()),
                            span: callee.span,
                        }
                    } else {
                        rewrite_expr(
                            callee,
                            subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        )
                    }
                }
                (ExprKind::Field { receiver, name }, Some(args_for_call)) => {
                    let is_generic = mono
                        .method_instantiations
                        .iter()
                        .any(|(_, mname, margs)| mname == &name.name && margs == args_for_call);
                    if is_generic {
                        let mangled = mangle_name(&name.name, args_for_call, type_name_of);
                        let new_recv = rewrite_expr(
                            receiver,
                            subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        );
                        Expr {
                            kind: ExprKind::Field {
                                receiver: Box::new(new_recv),
                                name: Ident {
                                    name: mangled,
                                    span: name.span,
                                },
                            },
                            span: callee.span,
                        }
                    } else {
                        rewrite_expr(
                            callee,
                            subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        )
                    }
                }
                (ExprKind::Path { segments }, Some(args_for_call)) if segments.len() == 2 => {
                    let method_seg_name = segments[1].name.clone();
                    let is_generic = mono.method_instantiations.iter().any(|(_, mname, margs)| {
                        mname == &method_seg_name && margs == args_for_call
                    });
                    if is_generic {
                        let mangled = mangle_name(&method_seg_name, args_for_call, type_name_of);
                        let mut new_segs = segments.clone();
                        new_segs[1] = Ident {
                            name: mangled,
                            span: segments[1].span,
                        };
                        Expr {
                            kind: ExprKind::Path { segments: new_segs },
                            span: callee.span,
                        }
                    } else {
                        (**callee).clone()
                    }
                }
                _ => rewrite_expr(
                    callee,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                ),
            };
            let new_args: Vec<Expr> = args
                .iter()
                .map(|a| {
                    rewrite_expr(
                        a,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    )
                })
                .collect();
            // Substitute type-parameter references in turbofish type_args
            // through the active subst map. Without this, `size_of::[T]()`
            // (or any future intrinsic taking a type arg) inside a generic
            // body would keep the literal `T` and panic at codegen when
            // the LLVM type-renderer hits `Ty::Param("T")`.
            let new_type_args: Vec<Type> = type_args
                .iter()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup))
                .collect();
            ExprKind::Call {
                callee: Box::new(new_callee),
                args: new_args,
                type_args: new_type_args,
            }
        }
        ExprKind::Block(b) => ExprKind::Block(rewrite_block(
            b,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        )),
        ExprKind::Unsafe(b) => ExprKind::Unsafe(rewrite_block(
            b,
            subst,
            generic_names,
            inst_lookup,
            mono,
            type_name_of,
            struct_lookup,
        )),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => ExprKind::If {
            cond: Box::new(rewrite_expr(
                cond,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            then: rewrite_block(
                then,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            ),
            else_branch: else_branch.as_ref().map(|e| {
                Box::new(rewrite_expr(
                    e,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                ))
            }),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op: *op,
            lhs: Box::new(rewrite_expr(
                lhs,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            rhs: Box::new(rewrite_expr(
                rhs,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(rewrite_expr(
                operand,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
        },
        ExprKind::Range {
            start,
            end,
            inclusive,
        } => ExprKind::Range {
            start: start.as_ref().map(|e| {
                Box::new(rewrite_expr(
                    e,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                ))
            }),
            end: end.as_ref().map(|e| {
                Box::new(rewrite_expr(
                    e,
                    subst,
                    generic_names,
                    inst_lookup,
                    mono,
                    type_name_of,
                    struct_lookup,
                ))
            }),
            inclusive: *inclusive,
        },
        ExprKind::Assign { op, target, value } => ExprKind::Assign {
            op: *op,
            target: Box::new(rewrite_expr(
                target,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            value: Box::new(rewrite_expr(
                value,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
        },
        ExprKind::Cast { expr: inner, ty } => ExprKind::Cast {
            expr: Box::new(rewrite_expr(
                inner,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            ty: subst_type_ast(ty, subst, type_name_of, struct_lookup),
        },
        ExprKind::StructLit { name, fields } => ExprKind::StructLit {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|f| StructLitField {
                    name: f.name.clone(),
                    value: rewrite_expr(
                        &f.value,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    ),
                    span: f.span,
                })
                .collect(),
        },
        // Slice 7GEN.5c: rewrite `Pair[i32, bool] { ... }` to a plain
        // StructLit with the mangled name. Same approach as the type-side
        // Generic → Path rewrite: substitute fn-generic params first,
        // then look up the mangled name in struct_lookup.
        ExprKind::GenericStructLit {
            name,
            type_args,
            fields,
        } => {
            let resolved_args: Vec<Type> = type_args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            let arg_names: Vec<String> = resolved_args.iter().map(mangle_type_ast_arg).collect();
            let mangled_name = struct_lookup
                .by_names
                .get(&(name.name.clone(), arg_names))
                .cloned()
                .unwrap_or_else(|| name.name.clone());
            ExprKind::StructLit {
                name: Ident {
                    name: mangled_name,
                    span: name.span,
                },
                fields: fields
                    .iter()
                    .map(|f| StructLitField {
                        name: f.name.clone(),
                        value: rewrite_expr(
                            &f.value,
                            subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        ),
                        span: f.span,
                    })
                    .collect(),
            }
        }
        ExprKind::Field { receiver, name } => ExprKind::Field {
            receiver: Box::new(rewrite_expr(
                receiver,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            name: name.clone(),
        },
        // Slice 7GEN.5d: rewrite `Option[i32]::Some(7)` to a regular
        // `Call { callee: Path([mangled_enum, variant]), args }`.
        // Codegen never sees GenericEnumCall.
        //
        // v0.0.4 Phase 1C: also handles the `Type[args]::name(...)` shape
        // when `name` is a free generic fn in the same module as the
        // struct (not an impl-block method). Sema's
        // `check_generic_enum_call` recorded the dispatch decision in
        // `mono.assoc_free_fn_dispatches`; mirror it here by rewriting
        // to `Call { callee: Ident(qualified_fn_name), type_args, args }`.
        ExprKind::GenericEnumCall {
            enum_name,
            type_args,
            variant,
            args,
        } => {
            let resolved_args: Vec<Type> = type_args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            // v0.0.4 Phase 1C: free-fn fallback.
            if let Some(qualified_fn_name) = mono.assoc_free_fn_dispatches.get(&expr.span) {
                let new_args: Vec<Expr> = args
                    .iter()
                    .map(|a| {
                        rewrite_expr(
                            a,
                            subst,
                            generic_names,
                            inst_lookup,
                            mono,
                            type_name_of,
                            struct_lookup,
                        )
                    })
                    .collect();
                // Try to mangle to the monomorphized fn name. The
                // outer rewrite_expr doesn't re-process the Call we
                // construct here, so this mangling needs to land
                // inline.
                let arg_tys: Option<Vec<Ty>> = resolved_args
                    .iter()
                    .map(|t| type_ast_to_ty_with_subst(t, subst))
                    .collect();
                let final_name = if let Some(tys) = arg_tys {
                    inst_lookup
                        .get(&(qualified_fn_name.clone(), tys))
                        .cloned()
                        .unwrap_or_else(|| qualified_fn_name.clone())
                } else {
                    qualified_fn_name.clone()
                };
                let callee_expr = Expr {
                    kind: ExprKind::Ident(final_name),
                    span: variant.span,
                };
                ExprKind::Call {
                    callee: Box::new(callee_expr),
                    args: new_args,
                    type_args: Vec::new(),
                }
            } else {
                let arg_names: Vec<String> =
                    resolved_args.iter().map(mangle_type_ast_arg).collect();
                let mangled_enum = struct_lookup
                    .by_names
                    .get(&(enum_name.name.clone(), arg_names))
                    .cloned()
                    .unwrap_or_else(|| enum_name.name.clone());
                let segments = vec![
                    Ident {
                        name: mangled_enum,
                        span: enum_name.span,
                    },
                    variant.clone(),
                ];
                // For payload-less variants written without parens (args is
                // empty), rewrite to a bare Path expression (e.g.
                // `Maybe::None`). Otherwise rewrite to a Call.
                if args.is_empty() {
                    ExprKind::Path { segments }
                } else {
                    let path_expr = Expr {
                        kind: ExprKind::Path { segments },
                        span: enum_name.span,
                    };
                    let new_args: Vec<Expr> = args
                        .iter()
                        .map(|a| {
                            rewrite_expr(
                                a,
                                subst,
                                generic_names,
                                inst_lookup,
                                mono,
                                type_name_of,
                                struct_lookup,
                            )
                        })
                        .collect();
                    ExprKind::Call {
                        callee: Box::new(path_expr),
                        args: new_args,
                        type_args: Vec::new(),
                    }
                }
            }
        }
        ExprKind::ArrayLit { elements } => ExprKind::ArrayLit {
            elements: elements
                .iter()
                .map(|e| {
                    rewrite_expr(
                        e,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    )
                })
                .collect(),
        },
        ExprKind::Index { receiver, index } => ExprKind::Index {
            receiver: Box::new(rewrite_expr(
                receiver,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            index: Box::new(rewrite_expr(
                index,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: Box::new(rewrite_expr(
                scrutinee,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            arms: arms
                .iter()
                .map(|a| MatchArm {
                    pattern: a.pattern.clone(),
                    body: rewrite_expr(
                        &a.body,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    ),
                    span: a.span,
                })
                .collect(),
        },
        ExprKind::ArrayFill { fill, count, .. } => ExprKind::ArrayFill {
            fill: Box::new(rewrite_expr(
                fill,
                subst,
                generic_names,
                inst_lookup,
                mono,
                type_name_of,
                struct_lookup,
            )),
            count: *count,
            count_name: None,
        },
        // v0.0.11 Phase 4: subst type-parameter references in `#name`
        // intrinsics' turbofish type_args (e.g. `#size_of::[T]()` inside
        // a generic body) through the active subst map. Without this,
        // codegen sees `Ty::Param("T")` and panics.
        ExprKind::Intrinsic { name, type_args, args, ret_ty } => ExprKind::Intrinsic {
            name: name.clone(),
            type_args: type_args
                .iter()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup))
                .collect(),
            args: args
                .iter()
                .map(|a| {
                    rewrite_expr(
                        a,
                        subst,
                        generic_names,
                        inst_lookup,
                        mono,
                        type_name_of,
                        struct_lookup,
                    )
                })
                .collect(),
            ret_ty: ret_ty
                .as_ref()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup)),
        },
        other => other.clone(),
    };
    Expr {
        kind,
        span: expr.span,
    }
}

/// Build the mangled name for a `(generic_fn_name, [concrete_types])`
/// pair. Uses double-underscore as separator (per design note §6).
/// The `type_name_of` closure maps a `Ty` to its source-level name,
/// which is sema's source of truth for struct / enum names.
fn mangle_name(name: &str, args: &[Ty], type_name_of: &dyn Fn(&Ty) -> String) -> String {
    let mut s = name.to_string();
    for arg in args {
        s.push_str("__");
        s.push_str(&mangle_ty(arg, type_name_of));
    }
    s
}

// v0.0.19: mangle a generic call's callee straight from its explicit turbofish
// type-arg AST, producing the SAME symbol `mangle_name` builds from the resolved
// `Vec<Ty>`. `subst_type_ast` resolves type-params through the active subst and
// qualifies struct/enum names, so the per-arg `mangle_type_ast_arg` string
// equals `type_name_of(resolved_ty)`.
//
// Why this exists: the resolved type-args otherwise come from `call_monos`,
// keyed by a file-less `ByteSpan` — so two turbofish calls at the same byte
// offset in *different files* collide and one gets the other's type-args (a
// `Vec[A]` value miscompiled into a `Vec[B]` slot). The turbofish AST is
// per-call and collision-free, so for turbofish calls we mangle from it directly
// and never consult `call_monos`.
fn mangle_call_from_ast(
    name: &str,
    type_args: &[Type],
    subst: &std::collections::HashMap<String, Ty>,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &StructLookup,
) -> String {
    let mut s = name.to_string();
    for t in type_args {
        let resolved = subst_type_ast(t, subst, type_name_of, struct_lookup);
        s.push_str("__");
        s.push_str(&mangle_type_ast_arg(&resolved));
    }
    s
}

/// Phase 11 polish (2026-05-13): walk the program and substitute every
/// `TypeKind::Path(name)` where `name` is a type alias with the alias's
/// target. Recursive through nested types (Array, RawPtr, FnPtr, Generic
/// args). Drops `TypeAlias` items themselves — they're sema-only.
fn rewrite_aliases_in_program(
    mut program: Program,
    aliases: &std::collections::BTreeMap<String, Type>,
) -> Program {
    program
        .items
        .retain(|it| !matches!(&it.kind, ItemKind::TypeAlias(_)));
    for item in &mut program.items {
        match &mut item.kind {
            ItemKind::Function(f) => {
                for p in &mut f.params {
                    rewrite_alias_type(&mut p.ty, aliases);
                }
                if let Some(rt) = &mut f.return_type {
                    rewrite_alias_type(rt, aliases);
                }
                rewrite_alias_block(&mut f.body, aliases);
            }
            ItemKind::Struct(s) => {
                for f in &mut s.fields {
                    rewrite_alias_type(&mut f.ty, aliases);
                }
            }
            ItemKind::Enum(e) => {
                for v in &mut e.variants {
                    for t in &mut v.payload {
                        rewrite_alias_type(t, aliases);
                    }
                }
            }
            ItemKind::Impl(b) => {
                for m in &mut b.methods {
                    for p in &mut m.params {
                        rewrite_alias_type(&mut p.ty, aliases);
                    }
                    if let Some(rt) = &mut m.return_type {
                        rewrite_alias_type(rt, aliases);
                    }
                    rewrite_alias_block(&mut m.body, aliases);
                }
            }
            ItemKind::Interface(i) => {
                for m in &mut i.methods {
                    for p in &mut m.params {
                        rewrite_alias_type(&mut p.ty, aliases);
                    }
                    if let Some(rt) = &mut m.return_type {
                        rewrite_alias_type(rt, aliases);
                    }
                }
            }
            ItemKind::TypeAlias(_) => unreachable!("filtered above"),
            // v0.0.9 Phase 4: rewrite the declared type so type aliases
            // referenced from a const/static signature resolve. The
            // initializer is a literal — no nested types to rewrite —
            // but a future struct-literal extension would walk it here.
            ItemKind::Const(c) => {
                rewrite_alias_type(&mut c.ty, aliases);
            }
            ItemKind::Static(s) => {
                rewrite_alias_type(&mut s.ty, aliases);
            }
            // v0.0.15: module-scope `#asm("...")` carries no types — no alias
            // rewriting needed; the item passes through monomorphization inert.
            ItemKind::ModuleAsm(_) => {}
        }
    }
    program
}

fn rewrite_alias_type(t: &mut Type, aliases: &std::collections::BTreeMap<String, Type>) {
    match &mut t.kind {
        TypeKind::Path(name) => {
            if let Some(target) = aliases.get(name) {
                let mut new_t = target.clone();
                rewrite_alias_type(&mut new_t, aliases);
                *t = new_t;
            }
        }
        TypeKind::Array { elem, .. } => rewrite_alias_type(elem, aliases),
        TypeKind::Borrowed { inner, .. } => rewrite_alias_type(inner, aliases),
        TypeKind::RawPtr(inner) => rewrite_alias_type(inner, aliases),
        TypeKind::FnPtr {
            params,
            return_type,
        } => {
            for p in params {
                rewrite_alias_type(p, aliases);
            }
            if let Some(rt) = return_type {
                rewrite_alias_type(rt, aliases);
            }
        }
        TypeKind::Generic { args, .. } => {
            for a in args {
                rewrite_alias_type(a, aliases);
            }
        }
        TypeKind::Slice(inner) => rewrite_alias_type(inner, aliases),
        TypeKind::Tuple(elems) => {
            for t in elems {
                rewrite_alias_type(t, aliases);
            }
        }
    }
}

fn rewrite_alias_block(b: &mut Block, aliases: &std::collections::BTreeMap<String, Type>) {
    for s in &mut b.stmts {
        rewrite_alias_stmt(s, aliases);
    }
    if let Some(e) = &mut b.tail {
        rewrite_alias_expr(e, aliases);
    }
}

fn rewrite_alias_stmt(s: &mut Stmt, aliases: &std::collections::BTreeMap<String, Type>) {
    match &mut s.kind {
        StmtKind::Let { ty, init, .. } => {
            if let Some(t) = ty {
                rewrite_alias_type(t, aliases);
            }
            if let Some(e) = init {
                rewrite_alias_expr(e, aliases);
            }
        }
        StmtKind::Return(opt) => {
            if let Some(e) = opt {
                rewrite_alias_expr(e, aliases);
            }
        }
        StmtKind::While { cond, body, .. } => {
            rewrite_alias_expr(cond, aliases);
            rewrite_alias_block(body, aliases);
        }
        StmtKind::Loop(b, _) => rewrite_alias_block(b, aliases),
        StmtKind::For(fl, _) => match fl {
            ForLoop::Range { iter, body, .. } => {
                rewrite_alias_expr(iter, aliases);
                rewrite_alias_block(body, aliases);
            }
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init {
                    rewrite_alias_stmt(i, aliases);
                }
                if let Some(c) = cond {
                    rewrite_alias_expr(c, aliases);
                }
                for u in update {
                    rewrite_alias_expr(u, aliases);
                }
                rewrite_alias_block(body, aliases);
            }
        },
        StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
            rewrite_alias_expr(e, aliases)
        }
        StmtKind::IfLet {
            scrutinee,
            body,
            else_body,
            ..
        } => {
            rewrite_alias_expr(scrutinee, aliases);
            rewrite_alias_block(body, aliases);
            if let Some(b) = else_body {
                rewrite_alias_block(b, aliases);
            }
        }
        StmtKind::GuardLet {
            scrutinee,
            else_body,
            ..
        } => {
            rewrite_alias_expr(scrutinee, aliases);
            rewrite_alias_block(else_body, aliases);
        }
        StmtKind::WhileLet {
            scrutinee, body, ..
        } => {
            rewrite_alias_expr(scrutinee, aliases);
            rewrite_alias_block(body, aliases);
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn rewrite_alias_expr(e: &mut Expr, aliases: &std::collections::BTreeMap<String, Type>) {
    match &mut e.kind {
        ExprKind::Cast { expr, ty } => {
            rewrite_alias_expr(expr, aliases);
            rewrite_alias_type(ty, aliases);
        }
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            rewrite_alias_expr(callee, aliases);
            for a in args {
                rewrite_alias_expr(a, aliases);
            }
            for t in type_args {
                rewrite_alias_type(t, aliases);
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => rewrite_alias_block(b, aliases),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            rewrite_alias_expr(cond, aliases);
            rewrite_alias_block(then, aliases);
            if let Some(eb) = else_branch {
                rewrite_alias_expr(eb, aliases);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_alias_expr(lhs, aliases);
            rewrite_alias_expr(rhs, aliases);
        }
        ExprKind::Unary { operand, .. } => rewrite_alias_expr(operand, aliases),
        ExprKind::Range { start, end, .. } => {
            if let Some(e2) = start {
                rewrite_alias_expr(e2, aliases);
            }
            if let Some(e2) = end {
                rewrite_alias_expr(e2, aliases);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            rewrite_alias_expr(target, aliases);
            rewrite_alias_expr(value, aliases);
        }
        ExprKind::Field { receiver, .. } => rewrite_alias_expr(receiver, aliases),
        ExprKind::StructLit { name, fields } => {
            rewrite_alias_ident(name, aliases);
            for f in fields {
                rewrite_alias_expr(&mut f.value, aliases);
            }
        }
        ExprKind::GenericStructLit {
            fields, type_args, ..
        } => {
            for f in fields {
                rewrite_alias_expr(&mut f.value, aliases);
            }
            for t in type_args {
                rewrite_alias_type(t, aliases);
            }
        }
        ExprKind::GenericEnumCall {
            type_args, args, ..
        } => {
            for t in type_args {
                rewrite_alias_type(t, aliases);
            }
            for a in args {
                rewrite_alias_expr(a, aliases);
            }
        }
        ExprKind::ArrayLit { elements } => {
            for el in elements {
                rewrite_alias_expr(el, aliases);
            }
        }
        ExprKind::ArrayFill { fill, .. } => rewrite_alias_expr(fill, aliases),
        ExprKind::Index { receiver, index } => {
            rewrite_alias_expr(receiver, aliases);
            rewrite_alias_expr(index, aliases);
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_alias_expr(scrutinee, aliases);
            for a in arms {
                rewrite_alias_expr(&mut a.body, aliases);
            }
        }
        ExprKind::Intrinsic { type_args, args, ret_ty, .. } => {
            for t in type_args {
                rewrite_alias_type(t, aliases);
            }
            for a in args {
                rewrite_alias_expr(a, aliases);
            }
            if let Some(rt) = ret_ty {
                rewrite_alias_type(rt, aliases);
            }
        }
        _ => {}
    }
}

fn rewrite_alias_ident(ident: &mut Ident, aliases: &std::collections::BTreeMap<String, Type>) {
    let mut current = ident.name.clone();
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(current.clone()) {
        let Some(target) = aliases.get(&current) else {
            return;
        };
        let TypeKind::Path(next) = &target.kind else {
            return;
        };
        ident.name = next.clone();
        current = next.clone();
    }
}

/// Slice 7GEN.5c carry-forward (2026-05-13): mirror of `mangle_ty` for
/// AST `Type` nodes. Used to extract a `StructLookup::by_names` key for
/// each post-recursion arg. The shapes must match `mangle_ty` exactly so
/// nested generics like `Pair[Box[T], i32]` look up correctly: the inner
/// `Box[T]` resolves to `Path("Box__i32")` and we render its name as the
/// literal `"Box__i32"` here. Falls back to a best-effort spelling for
/// AST shapes that don't appear inside generic args today.
fn mangle_type_ast_arg(t: &Type) -> String {
    match &t.kind {
        // v0.0.12 G-026: `()` source-spelled unit type. Sema's `mangle_ty`
        // renders Ty::Unit as "unit"; the AST-side mangler has to match
        // that name so the struct-lookup map hits when the same type is
        // reached via the AST instead of via Ty.
        TypeKind::Path(name) if name == "()" => "unit".to_string(),
        TypeKind::Path(name) => name.clone(),
        TypeKind::Array { elem, len, .. } => format!("arr{}_{}", len, mangle_type_ast_arg(elem)),
        TypeKind::Borrowed { inner, .. } => mangle_type_ast_arg(inner),
        TypeKind::RawPtr(inner) => format!("ptr_{}", mangle_type_ast_arg(inner)),
        TypeKind::FnPtr {
            params,
            return_type,
        } => {
            let mut s = String::from("fn");
            for p in params {
                s.push('_');
                s.push_str(&mangle_type_ast_arg(p));
            }
            if let Some(rt) = return_type {
                s.push_str("_ret_");
                s.push_str(&mangle_type_ast_arg(rt));
            }
            s
        }
        TypeKind::Generic { name, args } => {
            // After subst_type_ast recursion this should be unreachable
            // (Generic→Path rewrite consumes Generic nodes). If it shows
            // up, render best-effort so an unresolved key falls through
            // to the unchanged Generic branch in subst_type_ast.
            let mut s = name.clone();
            for a in args {
                s.push_str("__");
                s.push_str(&mangle_type_ast_arg(a));
            }
            s
        }
        TypeKind::Slice(inner) => format!("slice_{}", mangle_type_ast_arg(inner)),
        TypeKind::Tuple(elems) => {
            let mut s = format!("tuple{}", elems.len());
            for e in elems {
                s.push('_');
                s.push_str(&mangle_type_ast_arg(e));
            }
            s
        }
    }
}

/// Render a `Ty` as a name-safe string for mangling. Primitives use
/// their literal name (`i32`, `bool`); aggregates use their source
/// name. Arrays render as `arrN_<elem>` so the structure round-trips
/// without bracket characters LLVM identifiers reject.
fn mangle_ty(ty: &Ty, type_name_of: &dyn Fn(&Ty) -> String) -> String {
    match ty {
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(),
        Ty::Usize => "usize".into(),
        Ty::F16 => "f16".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(),
        Ty::Unit => "unit".into(),
        Ty::Str => "str".into(),
        Ty::String => "string".into(),
        Ty::Slice(inner) => format!("slice_{}", mangle_ty(inner, type_name_of)),
        Ty::RawPtr(inner) => format!("ptr_{}", mangle_ty(inner, type_name_of)),
        Ty::FnPtr {
            params,
            return_type,
        } => {
            let mut s = String::from("fn");
            for p in params {
                s.push('_');
                s.push_str(&mangle_ty(p, type_name_of));
            }
            if !matches!(**return_type, Ty::Unit) {
                s.push_str("_ret_");
                s.push_str(&mangle_ty(return_type, type_name_of));
            }
            s
        }
        Ty::Struct(_) | Ty::Enum(_) => type_name_of(ty),
        Ty::Array(elem, n) => format!("arr{}_{}", n, mangle_ty(elem, type_name_of)),
        Ty::Simd { elem, lanes } => format!("{}x{}", mangle_ty(elem, type_name_of), lanes),
        Ty::Mask { elem, lanes } => format!("mask{}x{}", mangle_ty(elem, type_name_of), lanes),
        Ty::Param(name) => format!("Param_{name}"),
        Ty::Error => "ERR".into(),
    }
}

// Suppress unused-import warning on StructId / EnumId — kept for
// forward-compatibility when mangling needs them directly.
#[allow(dead_code)]
fn _unused_id_imports(_s: StructId, _e: EnumId) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{tokenize, Span as ByteSpan};
    use crate::parser::parse;
    use crate::sema::{check_multi_with_mono, Ty};
    use std::path::PathBuf;

    fn name_of(ty: &Ty) -> String {
        match ty {
            Ty::I32 => "i32".into(),
            Ty::I64 => "i64".into(),
            Ty::Bool => "bool".into(),
            other => other.name().to_string(),
        }
    }

    fn run(src: &str) -> Program {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let (diags, mono) = check_multi_with_mono(
            &prog,
            PathBuf::from("test.cplus"),
            src,
            std::collections::BTreeMap::new(),
        );
        for d in &diags {
            if matches!(d.severity, crate::diagnostics::Severity::Error) {
                panic!("sema errors: {:#?}", diags);
            }
        }
        monomorphize(prog, &mono, &name_of)
    }

    #[test]
    fn identity_call_synthesizes_concrete_fn_and_rewrites_callee() {
        let p = run("fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = identity(7); return a; }");
        // Generic template removed; synthesized concrete fn present.
        let names: Vec<&str> = p
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::Function(f) => Some(f.name.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !names.contains(&"identity"),
            "generic template should be removed: {names:?}"
        );
        assert!(
            names.contains(&"identity__i32"),
            "expected monomorphized identity__i32: {names:?}"
        );
        // The call site in main was rewritten.
        let main = p
            .items
            .iter()
            .find_map(|i| match &i.kind {
                ItemKind::Function(f) if f.name.name == "main" => Some(f),
                _ => None,
            })
            .expect("main");
        let body_src = format!("{:?}", main.body);
        assert!(
            body_src.contains("identity__i32"),
            "main body should reference identity__i32: {body_src}"
        );
        assert!(
            !body_src.contains("Ident(\"identity\")"),
            "main body should not reference bare identity: {body_src}"
        );
    }

    #[test]
    fn distinct_instantiations_emit_distinct_fns() {
        let p = run("fn id[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = id(7); let b: bool = id(true); return a; }");
        let names: Vec<&str> = p
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::Function(f) => Some(f.name.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"id__i32"), "missing id__i32: {names:?}");
        assert!(names.contains(&"id__bool"), "missing id__bool: {names:?}");
    }

    #[test]
    fn mangle_separates_args_with_double_underscore() {
        let result = mangle_name("pick", &[Ty::I32, Ty::Bool], &name_of);
        assert_eq!(result, "pick__i32__bool");
    }

    #[test]
    fn non_generic_program_passes_through_unchanged() {
        let src = "fn add(a: i32, b: i32) -> i32 { return a + b; } fn main() -> i32 { return add(2, 3); }";
        let before_count = {
            let toks = tokenize(src).expect("lex");
            let prog = parse(toks).expect("parse");
            prog.items.len()
        };
        let p = run(src);
        assert_eq!(p.items.len(), before_count);
    }

    #[allow(dead_code)]
    fn _byte_span_used(_s: ByteSpan) {}
}
