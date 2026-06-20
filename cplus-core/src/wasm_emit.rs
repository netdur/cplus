//! wasm_emit — the wasm32 backend: C+ source → WebAssembly text (WAT).
//!
//! Browser-playground backend (`plans/plan.wasm-playground.md`). It emits WAT —
//! which the browser assembles and runs in its own sandbox — for the scalar
//! core of the language:
//!
//!   - **Phase 0** (i32 core): integer arithmetic, control flow, `#println`.
//!   - **Phase 1** (this file): all scalar widths and floats —
//!     `i32`/`u32`/`i64`/`u64`/`bool` + `f32`/`f64` — with per-type/per-sign
//!     instruction selection, numeric casts, value-position `if`, and
//!     short-circuit `&&`/`||`. Functions, calls, `let`/`var`, assignment,
//!     `if`/`while`/`loop` + `break`/`continue`/`return`, and `#println(i32)`.
//!
//! Type information comes from sema's `check_multi_with_value_types`: a
//! `span → rendered-type` table the caller passes in, so the emitter resolves
//! literal types (the one thing not derivable from the AST alone) without
//! re-running inference. Everything outside the scalar subset returns a clean
//! [`Diagnostic`] (code `E1900`), never a panic — structs, `Text`/strings, the
//! heap, sub-width ints, pointers/FFI are deferred to Phases 2/3.
//!
//! Control flow is emitted straight from the structured AST (C+ has no `goto`),
//! so wasm's `block`/`loop`/`br` cover it with no relooper.

use crate::ast::{
    AssignOp, BinOp, Block, Expr, ExprKind, Function, ItemKind, Program, Stmt, StmtKind, Type,
    TypeKind, UnaryOp,
};
use crate::diagnostics::{DiagCode, Diagnostic, LineMap, Severity};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The single diagnostic code the backend emits for any out-of-subset construct.
const UNSUPPORTED: DiagCode = DiagCode("E1900");

/// The host function the playground's JS shim supplies: prints one `i32` as
/// decimal followed by a newline. `#println(i32)` lowers to a call to it.
const PRINTLN_IMPORT: &str = r#"  (import "env" "println_i32" (func $println_i32 (param i32)))"#;

/// A wasm value type. C+ scalars collapse onto these four: `bool` and the
/// 32-bit-and-narrower ints onto `I32`, the 64-bit ints onto `I64`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WasmTy {
    I32,
    I64,
    F32,
    F64,
}

impl WasmTy {
    fn wat(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32",
            WasmTy::I64 => "i64",
            WasmTy::F32 => "f32",
            WasmTy::F64 => "f64",
        }
    }
    fn is_float(self) -> bool {
        matches!(self, WasmTy::F32 | WasmTy::F64)
    }
}

/// A scalar value: its wasm type plus signedness (signedness selects `_s`/`_u`
/// instruction variants for ints; it's irrelevant for floats).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Num {
    ty: WasmTy,
    signed: bool,
}

impl Num {
    const BOOL: Num = Num { ty: WasmTy::I32, signed: false };
}

/// What an emitted expression leaves on the wasm value stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Val {
    Void,
    Num(Num),
}

/// Per-function signature for resolving calls (param types + result arity).
#[derive(Clone)]
struct FnSig {
    params: Vec<Num>,
    result: Val,
}

/// Map a C+ scalar type *name* (as `Ty::render`/`name` produces it) to a [`Num`].
/// Sub-width ints, `isize`/`usize`, and everything aggregate return `None` (out
/// of the Phase-1 subset).
fn num_of_name(s: &str) -> Option<Num> {
    Some(match s {
        "i32" => Num { ty: WasmTy::I32, signed: true },
        "u32" | "bool" => Num { ty: WasmTy::I32, signed: false },
        "i64" => Num { ty: WasmTy::I64, signed: true },
        "u64" => Num { ty: WasmTy::I64, signed: false },
        "f32" => Num { ty: WasmTy::F32, signed: true },
        "f64" => Num { ty: WasmTy::F64, signed: true },
        _ => return None,
    })
}

/// Compile a checked [`Program`] to WAT. `value_types` is sema's
/// `MonoInfo::value_types` (`check_multi_with_value_types`) — `(_, span, type)`
/// triples used to resolve literal types. `file`/`src` anchor diagnostics.
pub fn generate_wat(
    program: &Program,
    file: &Path,
    src: &str,
    value_types: &[(Option<String>, Span, String)],
) -> Result<String, Diagnostic> {
    // span → scalar type, for literal-type resolution.
    let mut types: HashMap<(u32, u32), Num> = HashMap::new();
    for (_, span, rendered) in value_types {
        if let Some(n) = num_of_name(rendered) {
            types.entry((span.start, span.end)).or_insert(n);
        }
    }

    let mut em = Emitter::new(file, src, types);

    // Pass 1: signatures (so calls resolve regardless of source order).
    for item in &program.items {
        if let ItemKind::Function(f) = &item.kind {
            if f.is_extern {
                continue;
            }
            em.reject_unsupported_fn(f)?;
            let params = f
                .params
                .iter()
                .map(|p| em.ty_to_num(&p.ty, p.span))
                .collect::<Result<Vec<_>, _>>()?;
            let result = em.fn_result(f)?;
            em.funcs.insert(f.name.name.clone(), FnSig { params, result });
        }
    }

    if !em.funcs.contains_key("main") {
        return Err(em.err(
            "the wasm playground needs a `fn main` to run".to_string(),
            program.items.first().map(|i| i.span).unwrap_or(Span::new(0, 0)),
        ));
    }

    let mut out = String::new();
    out.push_str("(module\n");
    out.push_str(PRINTLN_IMPORT);
    out.push('\n');
    out.push_str("  (memory (export \"memory\") 1)\n");
    for item in &program.items {
        if let ItemKind::Function(f) = &item.kind {
            if f.is_extern {
                continue;
            }
            em.emit_function(f, &mut out)?;
        }
    }
    out.push_str(")\n");
    Ok(out)
}

struct Emitter {
    file: PathBuf,
    lm: LineMap,
    src: String,
    types: HashMap<(u32, u32), Num>,
    funcs: HashMap<String, FnSig>,
    /// Current function's local + param scalar types (for `local.get`/decls).
    locals: HashMap<String, Num>,
    /// Stack of `(break_label, continue_label)` for enclosing loops.
    loops: Vec<(String, String)>,
    label_ctr: usize,
}

impl Emitter {
    fn new(file: &Path, src: &str, types: HashMap<(u32, u32), Num>) -> Self {
        Emitter {
            file: file.to_path_buf(),
            lm: LineMap::new(src),
            src: src.to_string(),
            types,
            funcs: HashMap::new(),
            locals: HashMap::new(),
            loops: Vec::new(),
            label_ctr: 0,
        }
    }

    fn err(&self, message: String, span: Span) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: UNSUPPORTED,
            message,
            primary: self.lm.span(&self.file, span, &self.src),
            labels: Vec::new(),
            notes: vec![
                "the web playground runs the scalar core of C+; structs, text, the \
                 heap, and FFI are not available client-side yet"
                    .to_string(),
            ],
            suggestions: Vec::new(),
        }
    }

    /// The scalar type recorded for an expression's span, if any.
    fn span_num(&self, span: Span) -> Option<Num> {
        self.types.get(&(span.start, span.end)).copied()
    }

    /// Map a syntactic type to a scalar [`Num`], erroring if out of subset.
    fn ty_to_num(&self, ty: &Type, span: Span) -> Result<Num, Diagnostic> {
        match &ty.kind {
            TypeKind::Path(p) => num_of_name(p).ok_or_else(|| {
                self.err(
                    format!("type `{p}` is not a scalar the wasm playground supports yet"),
                    span,
                )
            }),
            _ => Err(self.err(
                "non-scalar type isn't supported in the wasm playground yet".to_string(),
                span,
            )),
        }
    }

    fn reject_unsupported_fn(&self, f: &Function) -> Result<(), Diagnostic> {
        if !f.generic_params.is_empty() {
            return Err(self.err(
                format!("generic function `{}` is not supported in the wasm playground", f.name.name),
                f.name.span,
            ));
        }
        if f.is_async || f.is_gen {
            return Err(self.err(
                format!("`async`/`gen` function `{}` is not supported in the wasm playground", f.name.name),
                f.name.span,
            ));
        }
        Ok(())
    }

    fn fn_result(&self, f: &Function) -> Result<Val, Diagnostic> {
        match &f.return_type {
            None => Ok(Val::Void),
            Some(t) => Ok(Val::Num(self.ty_to_num(t, t.span)?)),
        }
    }

    fn emit_function(&mut self, f: &Function, out: &mut String) -> Result<(), Diagnostic> {
        self.loops.clear();
        self.locals.clear();
        let result = self.fn_result(f)?;

        out.push_str(&format!("  (func ${}", f.name.name));
        out.push_str(&format!(" (export \"{}\")", f.name.name));
        for p in &f.params {
            let n = self.ty_to_num(&p.ty, p.span)?;
            self.locals.insert(p.name.name.clone(), n);
            out.push_str(&format!(" (param ${} {})", p.name.name, n.ty.wat()));
        }
        if let Val::Num(n) = result {
            out.push_str(&format!(" (result {})", n.ty.wat()));
        }
        out.push('\n');

        // Gather every `let`/`var` (locals are function-scoped in wasm), record
        // each one's scalar type, and declare it up front.
        let mut decls: Vec<(String, Num)> = Vec::new();
        let mut seen: HashSet<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
        self.collect_locals(&f.body, &mut decls, &mut seen)?;
        for (name, n) in &decls {
            self.locals.insert(name.clone(), *n);
            out.push_str(&format!("    (local ${} {})\n", name, n.ty.wat()));
        }

        let want_tail = matches!(result, Val::Num(_)) && f.body.tail.is_some();
        self.emit_block(&f.body, out, want_tail)?;

        if matches!(result, Val::Num(_)) && f.body.tail.is_none() && !ends_in_return(&f.body) {
            return Err(self.err(
                format!(
                    "`{}` returns a value but its body can fall off the end; the wasm playground \
                     needs an explicit `return` or a tail expression",
                    f.name.name
                ),
                f.name.span,
            ));
        }
        out.push_str("  )\n");
        Ok(())
    }

    fn collect_locals(
        &self,
        block: &Block,
        out: &mut Vec<(String, Num)>,
        seen: &mut HashSet<String>,
    ) -> Result<(), Diagnostic> {
        for s in &block.stmts {
            match &s.kind {
                StmtKind::Let { name, ty, init, .. } => {
                    // Determine the local's scalar type: declared type wins;
                    // otherwise the initializer's recorded type.
                    let n = if let Some(t) = ty {
                        self.ty_to_num(t, s.span)?
                    } else if let Some(init) = init {
                        self.span_num(init.span).ok_or_else(|| {
                            self.err(
                                format!("can't determine the type of `{}` for the wasm playground", name.name),
                                s.span,
                            )
                        })?
                    } else {
                        return Err(self.err(
                            format!("`{}` needs a type or initializer in the wasm playground", name.name),
                            s.span,
                        ));
                    };
                    if !seen.insert(name.name.clone()) {
                        return Err(self.err(
                            format!("`{}` is re-bound; shadowing isn't supported in the wasm playground yet", name.name),
                            s.span,
                        ));
                    }
                    out.push((name.name.clone(), n));
                }
                StmtKind::While { body, .. } => self.collect_locals(body, out, seen)?,
                StmtKind::Loop(body, _) => self.collect_locals(body, out, seen)?,
                StmtKind::Expr(e) => self.collect_locals_in_expr(e, out, seen)?,
                _ => {}
            }
        }
        if let Some(tail) = &block.tail {
            self.collect_locals_in_expr(tail, out, seen)?;
        }
        Ok(())
    }

    fn collect_locals_in_expr(
        &self,
        e: &Expr,
        out: &mut Vec<(String, Num)>,
        seen: &mut HashSet<String>,
    ) -> Result<(), Diagnostic> {
        if let ExprKind::If { then, else_branch, .. } = &e.kind {
            self.collect_locals(then, out, seen)?;
            if let Some(eb) = else_branch {
                if let ExprKind::Block(b) = &eb.kind {
                    self.collect_locals(b, out, seen)?;
                } else if matches!(eb.kind, ExprKind::If { .. }) {
                    self.collect_locals_in_expr(eb, out, seen)?;
                }
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, block: &Block, out: &mut String, keep_tail: bool) -> Result<(), Diagnostic> {
        for s in &block.stmts {
            self.emit_stmt(s, out)?;
        }
        if let Some(tail) = &block.tail {
            let v = self.emit_expr(tail, out)?;
            if !keep_tail && v != Val::Void {
                out.push_str("    drop\n");
            }
        }
        Ok(())
    }

    fn emit_stmt(&mut self, s: &Stmt, out: &mut String) -> Result<(), Diagnostic> {
        match &s.kind {
            StmtKind::Let { name, init, .. } => {
                if let Some(e) = init {
                    self.emit_expr(e, out)?;
                    out.push_str(&format!("    local.set ${}\n", name.name));
                }
                Ok(())
            }
            StmtKind::Return(opt) => {
                if let Some(e) = opt {
                    self.emit_expr(e, out)?;
                }
                out.push_str("    return\n");
                Ok(())
            }
            StmtKind::While { cond, body, .. } => self.emit_while(cond, body, out),
            StmtKind::Loop(body, _) => self.emit_loop(body, out),
            StmtKind::Break => {
                let brk = self.loops.last().ok_or_else(|| self.err("`break` outside a loop".to_string(), s.span))?.0.clone();
                out.push_str(&format!("    br {}\n", brk));
                Ok(())
            }
            StmtKind::Continue => {
                let cont = self.loops.last().ok_or_else(|| self.err("`continue` outside a loop".to_string(), s.span))?.1.clone();
                out.push_str(&format!("    br {}\n", cont));
                Ok(())
            }
            StmtKind::Expr(e) => {
                if let ExprKind::If { cond, then, else_branch } = &e.kind {
                    // Statement-position `if` only when it has no value (Unit).
                    if self.span_num(e.span).is_none() {
                        return self.emit_if_stmt(cond, then, else_branch.as_deref(), out);
                    }
                }
                let v = self.emit_expr(e, out)?;
                if v != Val::Void {
                    out.push_str("    drop\n");
                }
                Ok(())
            }
            _ => Err(self.err("this statement isn't supported in the wasm playground yet".to_string(), s.span)),
        }
    }

    fn emit_while(&mut self, cond: &Expr, body: &Block, out: &mut String) -> Result<(), Diagnostic> {
        let n = self.label_ctr;
        self.label_ctr += 1;
        let (brk, cont) = (format!("$brk{n}"), format!("$cont{n}"));
        out.push_str(&format!("    block {brk}\n    loop {cont}\n"));
        self.emit_expr(cond, out)?;
        out.push_str(&format!("    i32.eqz\n    br_if {brk}\n"));
        self.loops.push((brk.clone(), cont.clone()));
        self.emit_block(body, out, false)?;
        self.loops.pop();
        out.push_str(&format!("    br {cont}\n    end\n    end\n"));
        Ok(())
    }

    fn emit_loop(&mut self, body: &Block, out: &mut String) -> Result<(), Diagnostic> {
        let n = self.label_ctr;
        self.label_ctr += 1;
        let (brk, cont) = (format!("$brk{n}"), format!("$cont{n}"));
        out.push_str(&format!("    block {brk}\n    loop {cont}\n"));
        self.loops.push((brk.clone(), cont.clone()));
        self.emit_block(body, out, false)?;
        self.loops.pop();
        out.push_str(&format!("    br {cont}\n    end\n    end\n"));
        Ok(())
    }

    fn emit_if_stmt(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_branch: Option<&Expr>,
        out: &mut String,
    ) -> Result<(), Diagnostic> {
        self.emit_expr(cond, out)?;
        out.push_str("    if\n");
        self.emit_block(then, out, false)?;
        if let Some(eb) = else_branch {
            out.push_str("    else\n");
            match &eb.kind {
                ExprKind::Block(b) => self.emit_block(b, out, false)?,
                ExprKind::If { cond, then, else_branch } => {
                    self.emit_if_stmt(cond, then, else_branch.as_deref(), out)?
                }
                _ => return Err(self.err("unexpected `else` form in the wasm playground".to_string(), eb.span)),
            }
        }
        out.push_str("    end\n");
        Ok(())
    }

    fn emit_expr(&mut self, e: &Expr, out: &mut String) -> Result<Val, Diagnostic> {
        match &e.kind {
            ExprKind::IntLit(v, _) => {
                let n = self.span_num(e.span).unwrap_or(Num { ty: WasmTy::I32, signed: true });
                match n.ty {
                    WasmTy::I32 => out.push_str(&format!("    i32.const {}\n", *v as i32)),
                    WasmTy::I64 => out.push_str(&format!("    i64.const {}\n", *v as i64)),
                    // An integer literal typed as a float (e.g. `let x: f64 = 2;`).
                    WasmTy::F32 => out.push_str(&format!("    f32.const {}\n", fmt_float(*v as f64))),
                    WasmTy::F64 => out.push_str(&format!("    f64.const {}\n", fmt_float(*v as f64))),
                }
                Ok(Val::Num(n))
            }
            ExprKind::FloatLit(f, _) => {
                let n = self.span_num(e.span).unwrap_or(Num { ty: WasmTy::F64, signed: true });
                let t = if n.ty.is_float() { n.ty } else { WasmTy::F64 };
                out.push_str(&format!("    {}.const {}\n", t.wat(), fmt_float(*f)));
                Ok(Val::Num(Num { ty: t, signed: true }))
            }
            ExprKind::BoolLit(b) => {
                out.push_str(&format!("    i32.const {}\n", if *b { 1 } else { 0 }));
                Ok(Val::Num(Num::BOOL))
            }
            ExprKind::Ident(name) => {
                let n = *self.locals.get(name).ok_or_else(|| {
                    self.err(format!("unknown variable `{name}` in the wasm playground"), e.span)
                })?;
                out.push_str(&format!("    local.get ${}\n", name));
                Ok(Val::Num(n))
            }
            ExprKind::Binary { op, lhs, rhs } => self.emit_binary(*op, lhs, rhs, e.span, out),
            ExprKind::Unary { op, operand } => self.emit_unary(*op, operand, e.span, out),
            ExprKind::Assign { op, target, value } => self.emit_assign(*op, target, value, e.span, out),
            ExprKind::Cast { expr, ty } => {
                let from = self.emit_num(expr, out)?;
                let to = self.ty_to_num(ty, e.span)?;
                for instr in cast_instrs(from, to).ok_or_else(|| {
                    self.err("this cast isn't supported in the wasm playground yet".to_string(), e.span)
                })? {
                    out.push_str(&format!("    {instr}\n"));
                }
                Ok(Val::Num(to))
            }
            ExprKind::Call { callee, args, .. } => {
                let fname = match &callee.kind {
                    ExprKind::Ident(n) => n.clone(),
                    _ => return Err(self.err("only direct calls to named functions are supported in the wasm playground".to_string(), callee.span)),
                };
                let sig = self.funcs.get(&fname).cloned().ok_or_else(|| {
                    self.err(format!("call to unknown / unsupported function `{fname}`"), e.span)
                })?;
                for a in args {
                    self.emit_num(a, out)?;
                }
                out.push_str(&format!("    call ${fname}\n"));
                Ok(sig.result)
            }
            ExprKind::Intrinsic { name, args, .. } if name == "println" => {
                if args.len() != 1 {
                    return Err(self.err("the wasm playground supports `#println(i32)` with one argument".to_string(), e.span));
                }
                let n = self.emit_num(&args[0], out)?;
                if n.ty != WasmTy::I32 {
                    return Err(self.err("`#println` takes an i32 in the wasm playground".to_string(), args[0].span));
                }
                out.push_str("    call $println_i32\n");
                Ok(Val::Void)
            }
            // Value-position `if` → `(if (result T) (then …) (else …))`.
            ExprKind::If { cond, then, else_branch } => {
                let rty = self.span_num(e.span).ok_or_else(|| {
                    self.err("this `if` value type isn't supported in the wasm playground yet".to_string(), e.span)
                })?;
                let else_b = else_branch.as_deref().ok_or_else(|| {
                    self.err("a value-position `if` needs an `else` in the wasm playground".to_string(), e.span)
                })?;
                self.emit_expr(cond, out)?;
                out.push_str(&format!("    if (result {})\n", rty.ty.wat()));
                self.emit_block(then, out, true)?;
                out.push_str("    else\n");
                match &else_b.kind {
                    ExprKind::Block(b) => self.emit_block(b, out, true)?,
                    _ => {
                        self.emit_expr(else_b, out)?;
                    }
                }
                out.push_str("    end\n");
                Ok(Val::Num(rty))
            }
            ExprKind::Block(b) => {
                // A value block: emit stmts, keep the tail value.
                let keep = self.span_num(e.span).is_some();
                self.emit_block(b, out, keep)?;
                Ok(self.span_num(e.span).map(Val::Num).unwrap_or(Val::Void))
            }
            _ => Err(self.err("this expression isn't supported in the wasm playground yet".to_string(), e.span)),
        }
    }

    fn emit_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span, out: &mut String) -> Result<Val, Diagnostic> {
        // Short-circuit logical ops need control flow, not a binary op.
        if matches!(op, BinOp::And | BinOp::Or) {
            self.emit_num(lhs, out)?;
            out.push_str("    if (result i32)\n");
            match op {
                BinOp::And => {
                    self.emit_num(rhs, out)?;
                    out.push_str("    else\n    i32.const 0\n    end\n");
                }
                BinOp::Or => {
                    out.push_str("    i32.const 1\n    else\n");
                    self.emit_num(rhs, out)?;
                    out.push_str("    end\n");
                }
                _ => unreachable!(),
            }
            return Ok(Val::Num(Num::BOOL));
        }
        let ln = self.emit_num(lhs, out)?;
        self.emit_num(rhs, out)?;
        let (instr, result) = bin_op(op, ln).ok_or_else(|| {
            self.err(
                format!("operator not supported on `{}` in the wasm playground", ln.ty.wat()),
                span,
            )
        })?;
        out.push_str(&format!("    {instr}\n"));
        Ok(Val::Num(result))
    }

    fn emit_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span, out: &mut String) -> Result<Val, Diagnostic> {
        match op {
            UnaryOp::Neg => {
                let n = self.span_num(operand.span).or_else(|| self.peek_num(operand));
                match n.map(|n| n.ty) {
                    Some(WasmTy::F32) | Some(WasmTy::F64) => {
                        let nn = self.emit_num(operand, out)?;
                        out.push_str(&format!("    {}.neg\n", nn.ty.wat()));
                        Ok(Val::Num(nn))
                    }
                    _ => {
                        // ints: 0 - x
                        let t = n.map(|n| n.ty).unwrap_or(WasmTy::I32);
                        out.push_str(&format!("    {}.const 0\n", t.wat()));
                        let nn = self.emit_num(operand, out)?;
                        out.push_str(&format!("    {}.sub\n", nn.ty.wat()));
                        Ok(Val::Num(nn))
                    }
                }
            }
            UnaryOp::Not => {
                let nn = self.emit_num(operand, out)?;
                out.push_str(&format!("    {}.eqz\n", nn.ty.wat()));
                Ok(Val::Num(Num::BOOL))
            }
            UnaryOp::BitNot => {
                let nn = self.emit_num(operand, out)?;
                if nn.ty.is_float() {
                    return Err(self.err("`~` is not defined on floats".to_string(), span));
                }
                let allones = if nn.ty == WasmTy::I64 { "i64.const -1" } else { "i32.const -1" };
                out.push_str(&format!("    {allones}\n    {}.xor\n", nn.ty.wat()));
                Ok(Val::Num(nn))
            }
            _ => Err(self.err("references / dereferences aren't supported in the wasm playground".to_string(), span)),
        }
    }

    fn emit_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, span: Span, out: &mut String) -> Result<Val, Diagnostic> {
        let name = match &target.kind {
            ExprKind::Ident(n) => n.clone(),
            _ => return Err(self.err("assignment target must be a simple variable in the wasm playground".to_string(), target.span)),
        };
        let n = *self.locals.get(&name).ok_or_else(|| {
            self.err(format!("unknown variable `{name}` in the wasm playground"), target.span)
        })?;
        match op {
            AssignOp::Assign => {
                self.emit_num(value, out)?;
            }
            _ => {
                out.push_str(&format!("    local.get ${}\n", name));
                self.emit_num(value, out)?;
                let bop = compound_binop(op);
                let (instr, _) = bin_op(bop, n).ok_or_else(|| {
                    self.err("this compound assignment isn't supported in the wasm playground".to_string(), span)
                })?;
                out.push_str(&format!("    {instr}\n"));
            }
        }
        out.push_str(&format!("    local.set ${}\n", name));
        Ok(Val::Void)
    }

    /// Emit an expression and require it produced a scalar (not unit/void).
    fn emit_num(&mut self, e: &Expr, out: &mut String) -> Result<Num, Diagnostic> {
        match self.emit_expr(e, out)? {
            Val::Num(n) => Ok(n),
            Val::Void => Err(self.err("expected a value here in the wasm playground".to_string(), e.span)),
        }
    }

    /// Best-effort scalar type of an expression without emitting it (used to
    /// pick int-vs-float for unary neg). Falls back to span info / locals.
    fn peek_num(&self, e: &Expr) -> Option<Num> {
        match &e.kind {
            ExprKind::Ident(n) => self.locals.get(n).copied(),
            _ => self.span_num(e.span),
        }
    }
}

/// `(instruction, result type)` for a binary operator on operands of type `n`.
/// `None` when the operator isn't defined on that type (e.g. `%`/bitwise on
/// floats). Comparisons yield `bool` (i32); arithmetic yields `n`.
fn bin_op(op: BinOp, n: Num) -> Option<(String, Num)> {
    let t = n.ty.wat();
    let float = n.ty.is_float();
    let sgn = if n.signed { "s" } else { "u" };
    let arith = |base: &str| (format!("{t}.{base}"), n);
    let cmp = |instr: String| (instr, Num::BOOL);
    Some(match op {
        BinOp::Add | BinOp::AddWrap => arith("add"),
        BinOp::Sub | BinOp::SubWrap => arith("sub"),
        BinOp::Mul | BinOp::MulWrap => arith("mul"),
        BinOp::Div => {
            if float { arith("div") } else { (format!("{t}.div_{sgn}"), n) }
        }
        BinOp::Mod => {
            if float { return None } else { (format!("{t}.rem_{sgn}"), n) }
        }
        BinOp::Eq => cmp(format!("{t}.eq")),
        BinOp::Ne => cmp(format!("{t}.ne")),
        BinOp::Lt => cmp(if float { format!("{t}.lt") } else { format!("{t}.lt_{sgn}") }),
        BinOp::Le => cmp(if float { format!("{t}.le") } else { format!("{t}.le_{sgn}") }),
        BinOp::Gt => cmp(if float { format!("{t}.gt") } else { format!("{t}.gt_{sgn}") }),
        BinOp::Ge => cmp(if float { format!("{t}.ge") } else { format!("{t}.ge_{sgn}") }),
        BinOp::BitAnd if !float => arith("and"),
        BinOp::BitOr if !float => arith("or"),
        BinOp::BitXor if !float => arith("xor"),
        BinOp::Shl if !float => arith("shl"),
        BinOp::Shr if !float => (format!("{t}.shr_{sgn}"), n),
        _ => return None,
    })
}

/// The binary operator underlying a compound assignment (`+=` → `+`).
fn compound_binop(op: AssignOp) -> BinOp {
    match op {
        AssignOp::AddAssign => BinOp::Add,
        AssignOp::SubAssign => BinOp::Sub,
        AssignOp::MulAssign => BinOp::Mul,
        AssignOp::DivAssign => BinOp::Div,
        AssignOp::ModAssign => BinOp::Mod,
        AssignOp::BitAndAssign => BinOp::BitAnd,
        AssignOp::BitOrAssign => BinOp::BitOr,
        AssignOp::BitXorAssign => BinOp::BitXor,
        AssignOp::ShlAssign => BinOp::Shl,
        AssignOp::ShrAssign => BinOp::Shr,
        AssignOp::Assign => BinOp::Add, // unreachable (handled before call)
    }
}

/// The wasm conversion instructions for a numeric cast `from → to`. A
/// same-wasm-type cast (incl. signedness-only, e.g. `i32`↔`u32`) is a no-op.
/// Float→int uses the non-trapping `trunc_sat` family (deterministic; in-range
/// values match the native truncation).
fn cast_instrs(from: Num, to: Num) -> Option<Vec<String>> {
    use WasmTy::*;
    if from.ty == to.ty {
        return Some(vec![]);
    }
    let s = |signed: bool| if signed { "s" } else { "u" };
    Some(match (from.ty, to.ty) {
        (I32, I64) => vec![format!("i64.extend_i32_{}", s(from.signed))],
        (I64, I32) => vec!["i32.wrap_i64".into()],
        (I32, F32) => vec![format!("f32.convert_i32_{}", s(from.signed))],
        (I32, F64) => vec![format!("f64.convert_i32_{}", s(from.signed))],
        (I64, F32) => vec![format!("f32.convert_i64_{}", s(from.signed))],
        (I64, F64) => vec![format!("f64.convert_i64_{}", s(from.signed))],
        (F32, I32) => vec![format!("i32.trunc_sat_f32_{}", s(to.signed))],
        (F64, I32) => vec![format!("i32.trunc_sat_f64_{}", s(to.signed))],
        (F32, I64) => vec![format!("i64.trunc_sat_f32_{}", s(to.signed))],
        (F64, I64) => vec![format!("i64.trunc_sat_f64_{}", s(to.signed))],
        (F32, F64) => vec!["f64.promote_f32".into()],
        (F64, F32) => vec!["f32.demote_f64".into()],
        _ => return None,
    })
}

/// Format a float for WAT: shortest round-tripping decimal, always with a `.`
/// or exponent so it parses as a float; `inf`/`-inf`/`nan` for non-finite.
fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

fn ends_in_return(block: &Block) -> bool {
    matches!(block.stmts.last().map(|s| &s.kind), Some(StmtKind::Return(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{attrs, borrowck, lexer, lower, parser, sema};
    use std::collections::BTreeMap;

    /// Front end (recording value types) → WAT, mirroring the playground path.
    fn compile(src: &str) -> Result<String, String> {
        let path = PathBuf::from("playground.cplus");
        let toks = lexer::tokenize(src).map_err(|e| format!("lex: {e:?}"))?;
        let mut prog = parser::parse(toks).map_err(|e| format!("parse: {e:?}"))?;
        let mut diags = attrs::check(&prog, path.clone(), src);
        diags.extend(lower::lower(&mut prog, &path, src));
        let (sema_diags, mono) =
            sema::check_multi_with_value_types(&prog, path.clone(), src, BTreeMap::new());
        diags.extend(sema_diags);
        diags.extend(borrowck::check(&prog, &path, src));
        if let Some(d) = diags.iter().find(|d| d.severity == Severity::Error) {
            return Err(format!("frontend: {} {}", d.code, d.message));
        }
        generate_wat(&prog, &path, src, &mono.value_types).map_err(|d| format!("{} {}", d.code, d.message))
    }

    #[test]
    fn i32_slice_still_emits() {
        let wat = compile("fn main() -> i32 {\n    var i: i32 = 0;\n    while i < 3 {\n        #println(i);\n        i = i +% 1;\n    }\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("call $println_i32"));
        assert!(wat.contains("i32.lt_s"));
    }

    // sema pins `main` to `-> i32`, so non-i32 results are exercised through a
    // helper function (still emitted by the backend) called from a trivial main.
    #[test]
    fn i64_uses_64bit_instrs() {
        let wat = compile("fn big() -> i64 {\n    var x: i64 = 10;\n    x = x *% 2;\n    return x;\n}\nfn main() -> i32 {\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("(local $x i64)"));
        assert!(wat.contains("i64.mul"));
        assert!(wat.contains(r#"(func $big (export "big") (result i64)"#));
    }

    #[test]
    fn f64_arithmetic_and_compare() {
        let wat = compile("fn calc() -> f64 {\n    var a: f64 = 1.5;\n    var b: f64 = 2.0;\n    if a < b {\n        a = a + b;\n    }\n    return a;\n}\nfn main() -> i32 {\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("f64.const"));
        assert!(wat.contains("f64.add"));
        assert!(wat.contains("f64.lt"));
    }

    #[test]
    fn unsigned_uses_u_variants() {
        let wat = compile("fn d(x: u32, y: u32) -> u32 {\n    return x / y;\n}\nfn main() -> i32 {\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("i32.div_u"), "got: {wat}");
    }

    #[test]
    fn numeric_cast_int_to_float() {
        let wat = compile("fn conv(n: i32) -> f64 {\n    return n as f64;\n}\nfn main() -> i32 {\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("f64.convert_i32_s"), "got: {wat}");
    }

    #[test]
    fn value_position_if() {
        let wat = compile("fn main() -> i32 {\n    let c: bool = true;\n    let x: i32 = if c { 1 } else { 2 };\n    return x;\n}\n").expect("emit");
        assert!(wat.contains("if (result i32)"), "got: {wat}");
    }

    #[test]
    fn short_circuit_and() {
        let wat = compile("fn main() -> i32 {\n    let a: bool = true;\n    let b: bool = false;\n    if a && b {\n        return 1;\n    }\n    return 0;\n}\n").expect("emit");
        assert!(wat.contains("if (result i32)"), "got: {wat}");
    }

    #[test]
    fn float_in_main_is_now_supported() {
        // What Phase 0 rejected, Phase 1 runs.
        assert!(compile("fn main() -> i32 {\n    let x: f64 = 1.5;\n    return 0;\n}\n").is_ok());
    }

    #[test]
    fn struct_is_rejected_cleanly() {
        let err = compile("struct P {\n    x: i32,\n}\nfn main() -> i32 {\n    let p: P = { x: 1 };\n    return p.x;\n}\n").unwrap_err();
        assert!(err.contains("E1900"), "expected E1900, got: {err}");
    }

    #[test]
    fn missing_main_is_rejected() {
        let err = compile("fn helper() -> i32 {\n    return 1;\n}\n").unwrap_err();
        assert!(err.contains("E1900") && err.contains("main"), "got: {err}");
    }
}
