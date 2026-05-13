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
use crate::sema::{MonoInfo, StructId, EnumId, Ty};
use crate::lexer::Span;

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
    let mut struct_lookup: std::collections::HashMap<(String, Vec<Ty>), String> = struct_lookup;
    for (k, v) in &enum_lookup {
        struct_lookup.insert(k.clone(), v.clone());
    }
    // Build the substitution context for each instantiation up front
    // so call-site rewriting and template-expansion share one source.
    let mut instances: Vec<MonoInstance> = Vec::new();
    for (name, args) in &mono.instantiations {
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
    let generic_names: std::collections::HashSet<String> = mono
        .instantiations
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let mut out_items: Vec<Item> = Vec::with_capacity(program.items.len() + instances.len());
    let inst_lookup: std::collections::HashMap<(String, Vec<Ty>), String> = instances
        .iter()
        .map(|i| ((i.generic_name.clone(), i.concrete_args.clone()), i.mangled.clone()))
        .collect();
    for item in program.items {
        match &item.kind {
            ItemKind::Function(f) if !f.generic_params.is_empty() => {
                // Generic template — emit one synthesized concrete fn
                // per instantiation that targets this name; drop the
                // template itself.
                let template = f.clone();
                for inst in instances.iter().filter(|i| i.generic_name == template.name.name) {
                    let subst = build_subst(&template.generic_params, &inst.concrete_args);
                    let synthesized = synthesize_fn(&template, inst, &subst, &generic_names, &inst_lookup, mono, type_name_of, &struct_lookup);
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
                let rewritten = rewrite_item_calls(item, &generic_names, &inst_lookup, mono, type_name_of, &struct_lookup);
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
                name: Ident { name: v.name.clone(), span: Span::new(0, 0) },
                payload: v.payload.iter().map(|t| ty_to_type_ast(t, type_name_of)).collect(),
                span: Span::new(0, 0),
                attributes: Vec::new(),
            })
            .collect();
        let decl = EnumDecl {
            name: Ident { name: info.mangled_name.clone(), span: Span::new(0, 0) },
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
                name: Ident { name: name.clone(), span: Span::new(0, 0) },
                ty: ty_to_type_ast(ty, type_name_of),
                span: Span::new(0, 0),
                is_pub: *is_pub,
                attributes: Vec::new(),
            })
            .collect();
        let decl = StructDecl {
            name: Ident { name: info.mangled_name.clone(), span: Span::new(0, 0) },
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
    Program { items: out_items, imports: program.imports }
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
        Ty::F32 => TypeKind::Path("f32".into()),
        Ty::F64 => TypeKind::Path("f64".into()),
        Ty::Bool => TypeKind::Path("bool".into()),
        Ty::Unit => TypeKind::Path("()".into()),
        Ty::Str => TypeKind::Path("str".into()),
        Ty::RawPtr(inner) => TypeKind::RawPtr(Box::new(ty_to_type_ast(inner, type_name_of))),
        Ty::FnPtr { params, return_type } => TypeKind::FnPtr {
            params: params.iter().map(|p| ty_to_type_ast(p, type_name_of)).collect(),
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
        },
        Ty::Param(name) => TypeKind::Path(name.clone()),
        Ty::Error => TypeKind::Path("<error>".into()),
    };
    Type { kind, span: Span::new(0, 0) }
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    out_items: &mut Vec<Item>,
) {
    let target_name = b.target.name.clone();
    let impl_param_names: Vec<String> = b.target_generic_params.iter()
        .map(|g| g.name.name.clone()).collect();
    // For each instantiation of this generic struct, build a fresh
    // concrete impl block.
    for ((sname, args), info) in &mono.struct_instantiations {
        if sname != &target_name { continue; }
        if args.len() != impl_param_names.len() { continue; }
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
        let mangled_name = info.mangled_name.clone();
        let mut new_methods: Vec<Method> = Vec::with_capacity(b.methods.len());
        for m in &b.methods {
            let mut m2 = m.clone();
            // Substitute impl-level T in param types + return type.
            for p in &mut m2.params {
                p.ty = rewrite_self_in_type(&subst_type_ast(&p.ty, &subst, type_name_of, struct_lookup), &mangled_name);
            }
            if let Some(rt) = &mut m2.return_type {
                *rt = rewrite_self_in_type(&subst_type_ast(rt, &subst, type_name_of, struct_lookup), &mangled_name);
            }
            // Rewrite body: subst T → concrete, Self → mangled.
            m2.body = rewrite_block_with_self(&m2.body, &subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup, &mangled_name);
            new_methods.push(m2);
        }
        let new_impl = ImplBlock {
            target: Ident { name: mangled_name.clone(), span: b.target.span },
            target_generic_params: Vec::new(),
            methods: new_methods,
            interface_name: b.interface_name.clone(),
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
        TypeKind::Array { elem, len } => TypeKind::Array {
            elem: Box::new(rewrite_self_in_type(elem, mangled_name)),
            len: *len,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(rewrite_self_in_type(inner, mangled_name)),
        },
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args.iter().map(|a| rewrite_self_in_type(a, mangled_name)).collect(),
        },
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(rewrite_self_in_type(inner, mangled_name))),
        TypeKind::FnPtr { params, return_type } => TypeKind::FnPtr {
            params: params.iter().map(|p| rewrite_self_in_type(p, mangled_name)).collect(),
            return_type: return_type.as_ref().map(|rt| Box::new(rewrite_self_in_type(rt, mangled_name))),
        },
    };
    Type { kind, span: ty.span }
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mangled_name: &str,
) -> Block {
    // First run the generic rewrite that handles subst + generic-fn
    // call-site rewriting, then do a second pass that replaces Self.
    let pass1 = rewrite_block(block, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup);
    rewrite_block_self(&pass1, mangled_name)
}

fn rewrite_block_self(block: &Block, mangled_name: &str) -> Block {
    Block {
        stmts: block.stmts.iter().map(|s| rewrite_stmt_self(s, mangled_name)).collect(),
        tail: block.tail.as_ref().map(|e| Box::new(rewrite_expr_self(e, mangled_name))),
        span: block.span,
    }
}

fn rewrite_stmt_self(stmt: &Stmt, mangled_name: &str) -> Stmt {
    let kind = match &stmt.kind {
        StmtKind::Let { mutable, name, ty, init } => StmtKind::Let {
            mutable: *mutable,
            name: name.clone(),
            ty: ty.as_ref().map(|t| rewrite_self_in_type(t, mangled_name)),
            init: init.as_ref().map(|e| rewrite_expr_self(e, mangled_name)),
        },
        StmtKind::Expr(e) => StmtKind::Expr(rewrite_expr_self(e, mangled_name)),
        StmtKind::Return(e) => StmtKind::Return(e.as_ref().map(|e| rewrite_expr_self(e, mangled_name))),
        StmtKind::While { cond, body } => StmtKind::While {
            cond: rewrite_expr_self(cond, mangled_name),
            body: rewrite_block_self(body, mangled_name),
        },
        StmtKind::For(forloop) => StmtKind::For(rewrite_for_self(forloop, mangled_name)),
        other => other.clone(),
    };
    Stmt { kind, span: stmt.span }
}

fn rewrite_for_self(f: &ForLoop, mangled_name: &str) -> ForLoop {
    match f {
        ForLoop::Range { var, iter, body } => ForLoop::Range {
            var: var.clone(),
            iter: rewrite_expr_self(iter, mangled_name),
            body: rewrite_block_self(body, mangled_name),
        },
        ForLoop::CStyle { init, cond, update, body } => ForLoop::CStyle {
            init: init.as_ref().map(|s| Box::new(rewrite_stmt_self(s, mangled_name))),
            cond: cond.as_ref().map(|e| rewrite_expr_self(e, mangled_name)),
            update: update.iter().map(|e| rewrite_expr_self(e, mangled_name)).collect(),
            body: rewrite_block_self(body, mangled_name),
        },
    }
}

fn rewrite_expr_self(expr: &Expr, mangled_name: &str) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Path { segments } if segments.len() == 1 && segments[0].name == "Self" => {
            ExprKind::Path { segments: vec![Ident { name: mangled_name.to_string(), span: segments[0].span }] }
        }
        // Most expressions don't carry types, so the only thing we
        // really need to chase is nested blocks/cast/etc.
        ExprKind::Block(b) => ExprKind::Block(rewrite_block_self(b, mangled_name)),
        ExprKind::Unsafe(b) => ExprKind::Unsafe(rewrite_block_self(b, mangled_name)),
        ExprKind::If { cond, then, else_branch } => ExprKind::If {
            cond: Box::new(rewrite_expr_self(cond, mangled_name)),
            then: rewrite_block_self(then, mangled_name),
            else_branch: else_branch.as_ref().map(|e| Box::new(rewrite_expr_self(e, mangled_name))),
        },
        ExprKind::Cast { expr: inner, ty } => ExprKind::Cast {
            expr: Box::new(rewrite_expr_self(inner, mangled_name)),
            ty: rewrite_self_in_type(ty, mangled_name),
        },
        ExprKind::Call { callee, args, type_args } => ExprKind::Call {
            callee: Box::new(rewrite_expr_self(callee, mangled_name)),
            args: args.iter().map(|a| rewrite_expr_self(a, mangled_name)).collect(),
            type_args: type_args.iter().map(|t| rewrite_self_in_type(t, mangled_name)).collect(),
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
            arms: arms.iter().map(|a| MatchArm {
                pattern: a.pattern.clone(),
                body: rewrite_expr_self(&a.body, mangled_name),
                span: a.span,
            }).collect(),
        },
        ExprKind::StructLit { name, fields } => {
            let new_name = if name.name == "Self" {
                Ident { name: mangled_name.to_string(), span: name.span }
            } else { name.clone() };
            ExprKind::StructLit {
                name: new_name,
                fields: fields.iter().map(|f| StructLitField {
                    name: f.name.clone(),
                    value: rewrite_expr_self(&f.value, mangled_name),
                    span: f.span,
                }).collect(),
            }
        }
        ExprKind::ArrayLit { elements } => ExprKind::ArrayLit {
            elements: elements.iter().map(|e| rewrite_expr_self(e, mangled_name)).collect(),
        },
        other => other.clone(),
    };
    Expr { kind, span: expr.span }
}

/// Build the param-name → concrete-type substitution for a single
/// instantiation. `generic_params` order matches `concrete_args` order.
fn build_subst(generic_params: &[GenericParam], concrete_args: &[Ty]) -> std::collections::HashMap<String, Ty> {
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> Function {
    Function {
        name: Ident { name: inst.mangled.clone(), span: template.name.span },
        params: template.params.iter().map(|p| Param {
            name: p.name.clone(),
            ty: subst_type_ast(&p.ty, subst, type_name_of, struct_lookup),
            mutable: p.mutable,
            move_: p.move_,
            span: p.span,
        }).collect(),
        return_type: template.return_type.as_ref().map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup)),
        body: rewrite_block(&template.body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
        is_pub: template.is_pub,
        is_extern: template.is_extern,
        is_variadic: template.is_variadic,
        attributes: template.attributes.clone(),
        generic_params: Vec::new(),   // monomorphized — no longer generic
    }
}

/// Substitute type-param names inside an AST `Type` node. Recurses
/// into array element types and borrow-region wrappers.
fn subst_type_ast(
    ty: &Type,
    subst: &std::collections::HashMap<String, Ty>,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) => {
            if let Some(concrete) = subst.get(name) {
                TypeKind::Path(type_name_of(concrete))
            } else {
                TypeKind::Path(name.clone())
            }
        }
        TypeKind::Array { elem, len } => TypeKind::Array {
            elem: Box::new(subst_type_ast(elem, subst, type_name_of, struct_lookup)),
            len: *len,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(subst_type_ast(inner, subst, type_name_of, struct_lookup)),
        },
        // Slice 7GEN.5c: rewrite `Pair[i32, bool]` to `Path("Pair__i32__bool")`.
        // First substitute fn-generic params in each arg (so `Pair[T]` inside
        // a `fn id[T]` becomes `Pair[i32]` for the i32-instantiation), then
        // look up the result in the struct-instantiation map sema produced.
        TypeKind::Generic { name, args } => {
            let resolved_args: Vec<Type> = args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            // Convert each resolved AST arg back to a `Ty` for the lookup key.
            // The args here are concrete after substitution; render them via
            // type_name_of-style introspection. We need a `Ty` key.
            let arg_tys: Vec<Ty> = resolved_args
                .iter()
                .map(|a| type_ast_to_ty(a, type_name_of))
                .collect();
            if let Some(mangled) = struct_lookup.get(&(name.clone(), arg_tys)) {
                TypeKind::Path(mangled.clone())
            } else {
                // No matching instantiation — leave as-is and let
                // downstream surface the error (sema would have already).
                TypeKind::Generic { name: name.clone(), args: resolved_args }
            }
        }
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(subst_type_ast(inner, subst, type_name_of, struct_lookup))),
        TypeKind::FnPtr { params, return_type } => TypeKind::FnPtr {
            params: params.iter()
                .map(|p| subst_type_ast(p, subst, type_name_of, struct_lookup))
                .collect(),
            return_type: return_type.as_ref()
                .map(|rt| Box::new(subst_type_ast(rt, subst, type_name_of, struct_lookup))),
        },
    };
    Type { kind, span: ty.span }
}

/// Slice 7GEN.5c: convert a concrete-typed AST `Type` to a `Ty` for
/// instantiation-map lookups. This is the inverse of `ty_to_type_ast`
/// for the primitive + struct/enum cases that monomorphize encounters.
/// Struct/enum name resolution is best-effort — names that don't
/// match any in the program render as a `Param` placeholder which
/// won't key-match anything and falls through.
fn type_ast_to_ty(ty: &Type, _type_name_of: &dyn Fn(&Ty) -> String) -> Ty {
    match &ty.kind {
        TypeKind::Path(name) => match name.as_str() {
            "i8" => Ty::I8, "i16" => Ty::I16, "i32" => Ty::I32, "i64" => Ty::I64,
            "u8" => Ty::U8, "u16" => Ty::U16, "u32" => Ty::U32, "u64" => Ty::U64,
            "isize" => Ty::Isize, "usize" => Ty::Usize,
            "f32" => Ty::F32, "f64" => Ty::F64,
            "bool" => Ty::Bool, "()" => Ty::Unit,
            // For struct/enum names: monomorphize doesn't carry the
            // sema id table. Use `Ty::Param(name)` as a stable
            // synthetic key. The struct_lookup map's keys originally
            // came from sema's resolved `Ty::Struct(id)`, so a string
            // key like this won't match — but no in-tree case today
            // exercises generic-args-that-are-themselves-aggregates.
            // Document the limitation; revisit when needed.
            _ => Ty::Param(name.clone()),
        },
        TypeKind::Array { elem, len } => Ty::Array(Box::new(type_ast_to_ty(elem, _type_name_of)), *len),
        TypeKind::Borrowed { inner, .. } => type_ast_to_ty(inner, _type_name_of),
        TypeKind::Generic { .. } => Ty::Param("<generic>".into()),
        TypeKind::RawPtr(inner) => Ty::RawPtr(Box::new(type_ast_to_ty(inner, _type_name_of))),
        TypeKind::FnPtr { params, return_type } => Ty::FnPtr {
            params: params.iter().map(|p| type_ast_to_ty(p, _type_name_of)).collect(),
            return_type: Box::new(match return_type {
                Some(rt) => type_ast_to_ty(rt, _type_name_of),
                None => Ty::Unit,
            }),
        },
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
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
            f.body = rewrite_block(&f.body, &empty_subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup);
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
                    m2.body = rewrite_block(&m2.body, &empty_subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup);
                    new_methods.push(m2);
                } else {
                    // Synthesize one concrete method per instantiation.
                    for (sname, mname, args) in &mono.method_instantiations {
                        if sname != &target_name || mname != &m.name.name { continue; }
                        let subst = build_subst(&m.generic_params, args);
                        let mangled = mangle_name(&m.name.name, args, type_name_of);
                        let mut clone = m.clone();
                        clone.name = Ident { name: mangled, span: m.name.span };
                        clone.generic_params = Vec::new();
                        for p in &mut clone.params {
                            p.ty = subst_type_ast(&p.ty, &subst, type_name_of, struct_lookup);
                        }
                        if let Some(rt) = &mut clone.return_type {
                            *rt = subst_type_ast(rt, &subst, type_name_of, struct_lookup);
                        }
                        clone.body = rewrite_block(&m.body, &subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup);
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
        other => other,
    };
    Item { kind, span: item.span, origin_file: item.origin_file }
}

fn rewrite_block(
    block: &Block,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> Block {
    Block {
        stmts: block.stmts.iter().map(|s| rewrite_stmt(s, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)).collect(),
        tail: block.tail.as_ref().map(|e| Box::new(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> Stmt {
    let kind = match &stmt.kind {
        StmtKind::Let { mutable, name, ty, init } => StmtKind::Let {
            mutable: *mutable,
            name: name.clone(),
            ty: ty.as_ref().map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup)),
            init: init.as_ref().map(|e| rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        StmtKind::Return(opt) => StmtKind::Return(opt.as_ref().map(|e| rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
        StmtKind::While { cond, body } => StmtKind::While {
            cond: rewrite_expr(cond, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            body: rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
        },
        StmtKind::For(forloop) => StmtKind::For(rewrite_for(forloop, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        StmtKind::Expr(e) => StmtKind::Expr(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        StmtKind::Defer(e) => StmtKind::Defer(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        StmtKind::Assert(e) => StmtKind::Assert(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        // `if let` / `guard let` / `while let` are lowered to plain match
        // before sema, but the lower pass runs *before* monomorphize
        // currently — we still see these. Rewrite their components.
        StmtKind::IfLet { pattern, scrutinee, body, else_body } => StmtKind::IfLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(scrutinee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            body: rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            else_body: else_body.as_ref().map(|b| rewrite_block(b, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        StmtKind::Break | StmtKind::Continue => stmt.kind.clone(),
        StmtKind::GuardLet { pattern, scrutinee, else_body, complement } => StmtKind::GuardLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(scrutinee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            else_body: rewrite_block(else_body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            complement: complement.clone(),
        },
        StmtKind::Loop(body) => StmtKind::Loop(rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        StmtKind::WhileLet { pattern, scrutinee, body } => StmtKind::WhileLet {
            pattern: pattern.clone(),
            scrutinee: rewrite_expr(scrutinee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            body: rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
        },
    };
    Stmt { kind, span: stmt.span }
}

fn rewrite_for(
    f: &ForLoop,
    subst: &std::collections::HashMap<String, Ty>,
    generic_names: &std::collections::HashSet<String>,
    inst_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
    mono: &MonoInfo,
    type_name_of: &dyn Fn(&Ty) -> String,
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> ForLoop {
    match f {
        ForLoop::Range { var, iter, body } => ForLoop::Range {
            var: var.clone(),
            iter: rewrite_expr(iter, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            body: rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
        },
        ForLoop::CStyle { init, cond, update, body } => ForLoop::CStyle {
            init: init.as_ref().map(|s| Box::new(rewrite_stmt(s, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
            cond: cond.as_ref().map(|e| rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            update: update.iter().map(|e| rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)).collect(),
            body: rewrite_block(body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
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
    struct_lookup: &std::collections::HashMap<(String, Vec<Ty>), String>,
) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Call { callee, args, type_args } => {
            // Inspect the callee for a generic-fn / generic-method
            // dispatch and rewrite to the mangled name where applicable.
            // Slice 7GEN.5e: extended from generic-fn (Ident callee) only
            // to also include generic methods (Field callee) and
            // generic associated functions (Path callee).
            let args_for_call_opt = mono.call_monos.get(&expr.span);
            let new_callee: Expr = match (&callee.kind, args_for_call_opt) {
                (ExprKind::Ident(cname), Some(args_for_call)) if generic_names.contains(cname) => {
                    if let Some(mangled) = inst_lookup.get(&(cname.clone(), args_for_call.clone())) {
                        Expr { kind: ExprKind::Ident(mangled.clone()), span: callee.span }
                    } else { rewrite_expr(callee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup) }
                }
                (ExprKind::Field { receiver, name }, Some(args_for_call)) => {
                    let is_generic = mono.method_instantiations.iter()
                        .any(|(_, mname, margs)| mname == &name.name && margs == args_for_call);
                    if is_generic {
                        let mangled = mangle_name(&name.name, args_for_call, type_name_of);
                        let new_recv = rewrite_expr(receiver, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup);
                        Expr {
                            kind: ExprKind::Field {
                                receiver: Box::new(new_recv),
                                name: Ident { name: mangled, span: name.span },
                            },
                            span: callee.span,
                        }
                    } else {
                        rewrite_expr(callee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)
                    }
                }
                (ExprKind::Path { segments }, Some(args_for_call)) if segments.len() == 2 => {
                    let method_seg_name = segments[1].name.clone();
                    let is_generic = mono.method_instantiations.iter()
                        .any(|(_, mname, margs)| mname == &method_seg_name && margs == args_for_call);
                    if is_generic {
                        let mangled = mangle_name(&method_seg_name, args_for_call, type_name_of);
                        let mut new_segs = segments.clone();
                        new_segs[1] = Ident { name: mangled, span: segments[1].span };
                        Expr { kind: ExprKind::Path { segments: new_segs }, span: callee.span }
                    } else { (**callee).clone() }
                }
                _ => rewrite_expr(callee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            };
            let new_args: Vec<Expr> = args.iter().map(|a| rewrite_expr(a, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)).collect();
            // Substitute type-parameter references in turbofish type_args
            // through the active subst map. Without this, `size_of::[T]()`
            // (or any future intrinsic taking a type arg) inside a generic
            // body would keep the literal `T` and panic at codegen when
            // the LLVM type-renderer hits `Ty::Param("T")`.
            let new_type_args: Vec<Type> = type_args.iter()
                .map(|t| subst_type_ast(t, subst, type_name_of, struct_lookup))
                .collect();
            ExprKind::Call { callee: Box::new(new_callee), args: new_args, type_args: new_type_args }
        }
        ExprKind::Block(b) => ExprKind::Block(rewrite_block(b, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        ExprKind::Unsafe(b) => ExprKind::Unsafe(rewrite_block(b, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        ExprKind::If { cond, then, else_branch } => ExprKind::If {
            cond: Box::new(rewrite_expr(cond, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            then: rewrite_block(then, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
            else_branch: else_branch.as_ref().map(|e| Box::new(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op: *op,
            lhs: Box::new(rewrite_expr(lhs, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            rhs: Box::new(rewrite_expr(rhs, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(rewrite_expr(operand, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        ExprKind::Range { start, end, inclusive } => ExprKind::Range {
            start: start.as_ref().map(|e| Box::new(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
            end: end.as_ref().map(|e| Box::new(rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup))),
            inclusive: *inclusive,
        },
        ExprKind::Assign { op, target, value } => ExprKind::Assign {
            op: *op,
            target: Box::new(rewrite_expr(target, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            value: Box::new(rewrite_expr(value, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        ExprKind::Cast { expr: inner, ty } => ExprKind::Cast {
            expr: Box::new(rewrite_expr(inner, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            ty: subst_type_ast(ty, subst, type_name_of, struct_lookup),
        },
        ExprKind::StructLit { name, fields } => ExprKind::StructLit {
            name: name.clone(),
            fields: fields.iter().map(|f| StructLitField {
                name: f.name.clone(),
                value: rewrite_expr(&f.value, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
                span: f.span,
            }).collect(),
        },
        // Slice 7GEN.5c: rewrite `Pair[i32, bool] { ... }` to a plain
        // StructLit with the mangled name. Same approach as the type-side
        // Generic → Path rewrite: substitute fn-generic params first,
        // then look up the mangled name in struct_lookup.
        ExprKind::GenericStructLit { name, type_args, fields } => {
            let resolved_args: Vec<Type> = type_args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            let arg_tys: Vec<Ty> = resolved_args
                .iter()
                .map(|a| type_ast_to_ty(a, type_name_of))
                .collect();
            let mangled_name = struct_lookup
                .get(&(name.name.clone(), arg_tys))
                .cloned()
                .unwrap_or_else(|| name.name.clone());
            ExprKind::StructLit {
                name: Ident { name: mangled_name, span: name.span },
                fields: fields.iter().map(|f| StructLitField {
                    name: f.name.clone(),
                    value: rewrite_expr(&f.value, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
                    span: f.span,
                }).collect(),
            }
        }
        ExprKind::Field { receiver, name } => ExprKind::Field {
            receiver: Box::new(rewrite_expr(receiver, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            name: name.clone(),
        },
        // Slice 7GEN.5d: rewrite `Option[i32]::Some(7)` to a regular
        // `Call { callee: Path([mangled_enum, variant]), args }`.
        // Codegen never sees GenericEnumCall.
        ExprKind::GenericEnumCall { enum_name, type_args, variant, args } => {
            let resolved_args: Vec<Type> = type_args
                .iter()
                .map(|a| subst_type_ast(a, subst, type_name_of, struct_lookup))
                .collect();
            let arg_tys: Vec<Ty> = resolved_args
                .iter()
                .map(|a| type_ast_to_ty(a, type_name_of))
                .collect();
            let mangled_enum = struct_lookup
                .get(&(enum_name.name.clone(), arg_tys))
                .cloned()
                .unwrap_or_else(|| enum_name.name.clone());
            let segments = vec![
                Ident { name: mangled_enum, span: enum_name.span },
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
                let new_args: Vec<Expr> = args.iter().map(|a| rewrite_expr(a, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)).collect();
                ExprKind::Call {
                    callee: Box::new(path_expr),
                    args: new_args,
                    type_args: Vec::new(),
                }
            }
        }
        ExprKind::ArrayLit { elements } => ExprKind::ArrayLit {
            elements: elements.iter().map(|e| rewrite_expr(e, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)).collect(),
        },
        ExprKind::Index { receiver, index } => ExprKind::Index {
            receiver: Box::new(rewrite_expr(receiver, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            index: Box::new(rewrite_expr(index, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: Box::new(rewrite_expr(scrutinee, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup)),
            arms: arms.iter().map(|a| MatchArm {
                pattern: a.pattern.clone(),
                body: rewrite_expr(&a.body, subst, generic_names, inst_lookup, mono, type_name_of, struct_lookup),
                span: a.span,
            }).collect(),
        },
        other => other.clone(),
    };
    Expr { kind, span: expr.span }
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

/// Render a `Ty` as a name-safe string for mangling. Primitives use
/// their literal name (`i32`, `bool`); aggregates use their source
/// name. Arrays render as `arrN_<elem>` so the structure round-trips
/// without bracket characters LLVM identifiers reject.
fn mangle_ty(ty: &Ty, type_name_of: &dyn Fn(&Ty) -> String) -> String {
    match ty {
        Ty::I8 => "i8".into(), Ty::I16 => "i16".into(), Ty::I32 => "i32".into(), Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(), Ty::U16 => "u16".into(), Ty::U32 => "u32".into(), Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(), Ty::Usize => "usize".into(),
        Ty::F32 => "f32".into(), Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(), Ty::Unit => "unit".into(),
        Ty::Str => "str".into(),
        Ty::RawPtr(inner) => format!("ptr_{}", mangle_ty(inner, type_name_of)),
        Ty::FnPtr { params, return_type } => {
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
        let p = run(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = identity(7); return a; }",
        );
        // Generic template removed; synthesized concrete fn present.
        let names: Vec<&str> = p.items.iter().filter_map(|i| match &i.kind {
            ItemKind::Function(f) => Some(f.name.name.as_str()),
            _ => None,
        }).collect();
        assert!(!names.contains(&"identity"), "generic template should be removed: {names:?}");
        assert!(names.contains(&"identity__i32"), "expected monomorphized identity__i32: {names:?}");
        // The call site in main was rewritten.
        let main = p.items.iter().find_map(|i| match &i.kind {
            ItemKind::Function(f) if f.name.name == "main" => Some(f),
            _ => None,
        }).expect("main");
        let body_src = format!("{:?}", main.body);
        assert!(body_src.contains("identity__i32"), "main body should reference identity__i32: {body_src}");
        assert!(!body_src.contains("Ident(\"identity\")"), "main body should not reference bare identity: {body_src}");
    }

    #[test]
    fn distinct_instantiations_emit_distinct_fns() {
        let p = run(
            "fn id[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = id(7); let b: bool = id(true); return a; }",
        );
        let names: Vec<&str> = p.items.iter().filter_map(|i| match &i.kind {
            ItemKind::Function(f) => Some(f.name.name.as_str()),
            _ => None,
        }).collect();
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
