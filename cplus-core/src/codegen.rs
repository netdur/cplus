//! Codegen: emit LLVM IR text from a sema-validated AST.
//!
//! Strategy (per plan §4.1): allocate every local with `alloca`, read/write
//! through `load`/`store`, let LLVM's `mem2reg` pass do the SSA conversion.
//! Avoids hand-rolled SSA construction.
//!
//! Phase 1 first cut: no overflow or div-by-zero checks. Sample programs don't
//! exercise those paths; they land as a refinement (`llvm.sadd.with.overflow.i32`
//! et al.) before Phase 2 begins.

use crate::ast::*;
use crate::sema::{EnumId, StructId, Ty};
use std::collections::HashMap;
use std::fmt::Write;

/// Build mode controls overflow checking on plain `+ - *`. Division-by-zero
/// trapping is emitted regardless of mode (per plan §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// Debug: insert `llvm.{sadd,ssub,smul}.with.overflow.i32` + `llvm.trap`
    /// around `+ - *`. Matches Rust's debug-mode arithmetic.
    Debug,
    /// Release: emit plain `add` / `sub` / `mul`. Wrapping is defined per §2.3.
    Release,
}

/// Generate LLVM IR for a sema-validated program. Caller must run sema first;
/// codegen will panic on unresolvable references that sema would have caught.
pub fn generate(program: &Program, mode: BuildMode) -> String {
    let types = collect_types(program);
    let sigs = collect_sigs(program, &types);
    let mut out = String::new();
    write_preamble(&mut out);
    write_struct_decls(&mut out, &types, program);
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) => gen_function(&mut out, f, &sigs, &types, mode),
            ItemKind::Impl(b) => {
                let Some(&id) = types.struct_by_name.get(&b.target.name) else { continue; };
                for m in &b.methods {
                    gen_method(&mut out, id, m, &sigs, &types, mode);
                }
            }
            ItemKind::Enum(_) | ItemKind::Struct(_) => {
                // Enum types are erased to i32; struct types are declared
                // upfront in `write_struct_decls`. Nothing to emit per-item.
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Ty>,
    return_type: Ty,
}

fn collect_sigs(p: &Program, types: &TypeTable) -> HashMap<String, FnSig> {
    let mut sigs = HashMap::new();
    // builtin: println(i32) -> ()
    sigs.insert(
        "println".to_string(),
        FnSig { params: vec![Ty::I32], return_type: Ty::Unit },
    );
    for item in &p.items {
        let ItemKind::Function(f) = &item.kind else { continue; };
        let params = f.params.iter().map(|p| ty_from(&p.ty, types)).collect();
        let ret = match &f.return_type {
            Some(t) => ty_from(t, types),
            None => Ty::Unit,
        };
        sigs.insert(f.name.name.clone(), FnSig { params, return_type: ret });
    }
    sigs
}

/// Codegen-side type registry. Mirrors sema's enum/struct numbering by walking
/// `program.items` in the same declaration order.
#[derive(Debug, Clone, Default)]
struct TypeTable {
    enum_by_name: HashMap<String, EnumId>,
    enum_defs: Vec<EnumInfo>,
    struct_by_name: HashMap<String, StructId>,
    struct_defs: Vec<StructInfo>,
}

#[derive(Debug, Clone)]
struct EnumInfo {
    variants: HashMap<String, u32>,
}

#[derive(Debug, Clone)]
struct StructInfo {
    name: String,
    /// Fields in declaration order. The pair is (field name, field type).
    fields: Vec<(String, Ty)>,
    /// Methods declared in `impl` blocks for this struct.
    methods: HashMap<String, MethodInfo>,
}

#[derive(Debug, Clone)]
struct MethodInfo {
    receiver: Option<Receiver>,
    /// Parameter types excluding the receiver.
    params: Vec<Ty>,
    return_type: Ty,
}

impl StructInfo {
    fn field_index(&self, name: &str) -> u32 {
        self.fields.iter().position(|(n, _)| n == name).expect("sema validated") as u32
    }
    fn field_type(&self, name: &str) -> Ty {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone()).expect("sema validated")
    }
}

fn mangle(struct_name: &str, method_name: &str) -> String {
    format!("{}.{}", struct_name, method_name)
}

fn collect_types(p: &Program) -> TypeTable {
    let mut t = TypeTable::default();
    // First pass: register names so struct field type resolution can refer
    // to other types declared anywhere in the program (forward refs).
    for item in &p.items {
        match &item.kind {
            ItemKind::Enum(e) => {
                if t.enum_by_name.contains_key(&e.name.name) || t.struct_by_name.contains_key(&e.name.name) {
                    continue;
                }
                let id = EnumId(t.enum_defs.len() as u32);
                let mut variants = HashMap::new();
                for (idx, v) in e.variants.iter().enumerate() {
                    variants.entry(v.name.clone()).or_insert(idx as u32);
                }
                t.enum_defs.push(EnumInfo { variants });
                t.enum_by_name.insert(e.name.name.clone(), id);
            }
            ItemKind::Struct(s) => {
                if t.enum_by_name.contains_key(&s.name.name) || t.struct_by_name.contains_key(&s.name.name) {
                    continue;
                }
                let id = StructId(t.struct_defs.len() as u32);
                t.struct_defs.push(StructInfo {
                    name: s.name.name.clone(),
                    fields: Vec::new(),
                    methods: HashMap::new(),
                });
                t.struct_by_name.insert(s.name.name.clone(), id);
            }
            ItemKind::Function(_) | ItemKind::Impl(_) => {}
        }
    }
    // Second pass: resolve struct field types.
    for item in &p.items {
        let ItemKind::Struct(s) = &item.kind else { continue; };
        let Some(&id) = t.struct_by_name.get(&s.name.name) else { continue; };
        let mut fields: Vec<(String, Ty)> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for f in &s.fields {
            if seen.contains_key(&f.name.name) { continue; }
            seen.insert(f.name.name.clone(), ());
            let ty = ty_from(&f.ty, &t);
            fields.push((f.name.name.clone(), ty));
        }
        t.struct_defs[id.0 as usize].fields = fields;
    }
    // Third pass: collect methods from impl blocks.
    for item in &p.items {
        let ItemKind::Impl(b) = &item.kind else { continue; };
        let Some(&id) = t.struct_by_name.get(&b.target.name) else { continue; };
        for m in &b.methods {
            if t.struct_defs[id.0 as usize].methods.contains_key(&m.name.name) {
                continue;
            }
            let params: Vec<Ty> = m.params.iter().map(|p| ty_from(&p.ty, &t)).collect();
            let return_type = match &m.return_type {
                Some(ty) => ty_from(ty, &t),
                None => Ty::Unit,
            };
            t.struct_defs[id.0 as usize].methods.insert(
                m.name.name.clone(),
                MethodInfo { receiver: m.receiver, params, return_type },
            );
        }
    }
    t
}

fn write_struct_decls(out: &mut String, types: &TypeTable, _p: &Program) {
    if types.struct_defs.is_empty() { return; }
    for s in &types.struct_defs {
        let inner: Vec<String> = s.fields.iter().map(|(_, t)| llvm_ty(t, types)).collect();
        writeln!(out, "%{} = type {{ {} }}", s.name, inner.join(", ")).unwrap();
    }
    out.push('\n');
}

fn ty_from(t: &Type, types: &TypeTable) -> Ty {
    let name = match &t.kind {
        TypeKind::Path(n) => n,
        TypeKind::Array { elem, len } => {
            let elem_ty = ty_from(elem, types);
            return Ty::Array(Box::new(elem_ty), *len);
        }
    };
    match name.as_str() {
        "i8" => Ty::I8, "i16" => Ty::I16, "i32" => Ty::I32, "i64" => Ty::I64,
        "u8" => Ty::U8, "u16" => Ty::U16, "u32" => Ty::U32, "u64" => Ty::U64,
        "isize" => Ty::Isize, "usize" => Ty::Usize,
        "f32" => Ty::F32, "f64" => Ty::F64,
        "bool" => Ty::Bool,
        _ => {
            if let Some(&id) = types.enum_by_name.get(name) { return Ty::Enum(id); }
            if let Some(&id) = types.struct_by_name.get(name) { return Ty::Struct(id); }
            Ty::Error
        }
    }
}

fn llvm_ty(ty: &Ty, types: &TypeTable) -> String {
    match ty {
        Ty::I8 | Ty::U8 => "i8".to_string(),
        Ty::I16 | Ty::U16 => "i16".to_string(),
        Ty::I32 | Ty::U32 | Ty::Enum(_) => "i32".to_string(),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => "i64".to_string(),
        Ty::F32 => "float".to_string(),
        Ty::F64 => "double".to_string(),
        Ty::Bool => "i1".to_string(),
        Ty::Unit => "void".to_string(),
        Ty::Struct(id) => format!("%{}", types.struct_defs[id.0 as usize].name),
        Ty::Array(elem, n) => format!("[{n} x {}]", llvm_ty(elem, types)),
        Ty::Error => panic!("codegen reached Ty::Error — sema should have rejected the program"),
    }
}

fn ty_bit_width(ty: &Ty) -> u32 {
    match ty {
        Ty::I8 | Ty::U8 => 8,
        Ty::I16 | Ty::U16 => 16,
        Ty::I32 | Ty::U32 | Ty::F32 | Ty::Enum(_) => 32,
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => 64,
        Ty::Bool => 1,
        _ => 0,
    }
}

fn write_preamble(out: &mut String) {
    out.push_str("; C+ Phase 1 codegen output\n");
    out.push_str("\n");
    // Format string used by `println(i32)`. Module-private constant.
    out.push_str("@.fmt_int_nl = private unnamed_addr constant [4 x i8] c\"%d\\0A\\00\", align 1\n");
    out.push_str("\n");
    out.push_str("declare i32 @printf(ptr noundef, ...)\n");
    // Trap intrinsic — used for both overflow (debug) and divide-by-zero (always).
    out.push_str("declare void @llvm.trap()\n");
    // Checked-arithmetic intrinsics used in debug mode for signed integers
    // of every supported width. Always declared; LLVM drops unused ones.
    for op in ["sadd", "ssub", "smul"] {
        for bits in [8, 16, 32, 64] {
            out.push_str(&format!(
                "declare {{i{bits}, i1}} @llvm.{op}.with.overflow.i{bits}(i{bits}, i{bits})\n"
            ));
        }
    }
    out.push_str("\n");
}

fn gen_function(
    out: &mut String,
    f: &Function,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    mode: BuildMode,
) {
    // Builtin name: codegen never emits a definition for it; clang links printf.
    if f.name.name == "println" {
        return;
    }

    let sig = sigs.get(&f.name.name).expect("sig was collected");
    let return_ty = sig.return_type.clone();

    // Function header
    write!(out, "define {} @{}(", llvm_ty(&return_ty, types), f.name.name).unwrap();
    for (i, (param, pty)) in f.params.iter().zip(sig.params.iter()).enumerate() {
        if i > 0 { out.push_str(", "); }
        write!(out, "{} %{}", llvm_ty(pty, types), i).unwrap();
        let _ = param;
    }
    out.push_str(") {\n");
    out.push_str("entry:\n");

    // Build the function body
    let mut state = FnState::new(return_ty.clone(), sigs, types, mode);

    // Allocate slots and store params
    for (i, (param, pty)) in f.params.iter().zip(sig.params.iter()).enumerate() {
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types), i, slot
        ));
        state.bind(&param.name.name, slot, pty.clone());
    }

    // Emit body
    state.gen_body_block(&f.body);

    // Ensure final terminator
    if !state.terminated {
        match &return_ty {
            Ty::Unit => state.emit_terminator("ret void"),
            // Sema guarantees a value; this is unreachable, but emit
            // `unreachable` so the IR validates if we slip through.
            _ => state.emit_terminator("unreachable"),
        }
    }

    // Glue: allocas first (in entry), then body
    for line in &state.allocas {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// Emit a method as a regular LLVM function with a mangled name `@Type.method`.
/// Receivers compile to LLVM parameters:
/// - `self` (value): a struct-typed parameter, stored in an alloca
/// - `self` / `mut self`: a `ptr` parameter, bound directly (no alloca)
fn gen_method(
    out: &mut String,
    struct_id: StructId,
    m: &Method,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    mode: BuildMode,
) {
    let struct_name = types.struct_defs[struct_id.0 as usize].name.clone();
    let sig = types.struct_defs[struct_id.0 as usize]
        .methods.get(&m.name.name).expect("sig was collected").clone();
    let mangled = mangle(&struct_name, &m.name.name);

    let return_ty = sig.return_type.clone();
    let struct_ty = Ty::Struct(struct_id);

    // Function header. Both `self` and `mut self` lower to a `ptr` parameter
    // (the struct's address). The receiver kind only affects sema-level
    // mutability checks, not the LLVM signature.
    write!(out, "define {} @{}(", llvm_ty(&return_ty, types), mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    if sig.receiver.is_some() {
        write!(out, "ptr %{llvm_idx}").unwrap();
        llvm_idx += 1;
        first = false;
    }
    for (param, pty) in m.params.iter().zip(sig.params.iter()) {
        if !first { out.push_str(", "); }
        write!(out, "{} %{}", llvm_ty(pty, types), llvm_idx).unwrap();
        llvm_idx += 1;
        first = false;
        let _ = param;
    }
    out.push_str(") {\n");
    out.push_str("entry:\n");

    let mut state = FnState::new(return_ty.clone(), sigs, types, mode);

    // Bind the receiver: `self` is the pointer parameter directly.
    let mut next_idx: u32 = 0;
    if sig.receiver.is_some() {
        state.bind("self", "%0".to_string(), struct_ty.clone());
        next_idx = 1;
    }

    // Bind non-receiver params.
    for (i, (param, pty)) in m.params.iter().zip(sig.params.iter()).enumerate() {
        let idx = next_idx + i as u32;
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types), idx, slot
        ));
        state.bind(&param.name.name, slot, pty.clone());
    }

    state.gen_body_block(&m.body);

    if !state.terminated {
        match &return_ty {
            Ty::Unit => state.emit_terminator("ret void"),
            _ => state.emit_terminator("unreachable"),
        }
    }

    for line in &state.allocas {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

struct FnState<'a> {
    body: String,
    allocas: Vec<String>,
    scopes: Vec<HashMap<String, (String, Ty)>>,
    return_ty: Ty,
    sigs: &'a HashMap<String, FnSig>,
    types: &'a TypeTable,
    mode: BuildMode,
    tmp_counter: u32,
    block_counter: u32,
    terminated: bool,
}

impl<'a> FnState<'a> {
    fn new(return_ty: Ty, sigs: &'a HashMap<String, FnSig>, types: &'a TypeTable, mode: BuildMode) -> Self {
        Self {
            body: String::new(),
            allocas: Vec::new(),
            scopes: vec![HashMap::new()],
            return_ty,
            sigs,
            types,
            mode,
            tmp_counter: 0,
            block_counter: 0,
            terminated: false,
        }
    }

    fn lty(&self, ty: &Ty) -> String { llvm_ty(ty, self.types) }

    // ---- counters ----

    fn next_tmp(&mut self) -> String {
        self.tmp_counter += 1;
        format!("%t{}", self.tmp_counter)
    }

    fn next_block_label(&mut self) -> String {
        self.block_counter += 1;
        format!("bb{}", self.block_counter)
    }

    // ---- block / instruction emission ----

    fn emit(&mut self, s: &str) {
        if self.terminated { return; }
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
    }

    fn emit_terminator(&mut self, s: &str) {
        if self.terminated { return; }
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
        self.terminated = true;
    }

    fn open_block(&mut self, label: &str) {
        // Ensure the previous block has a terminator. Connect by `br` if not.
        if !self.terminated {
            self.body.push_str(&format!("  br label %{label}\n"));
        }
        self.body.push('\n');
        self.body.push_str(&format!("{label}:\n"));
        self.terminated = false;
    }

    fn alloca_named(&mut self, name_hint: &str, ty: Ty) -> String {
        let slot = format!("%{}.addr", sanitize(name_hint));
        self.allocas.push(format!("{slot} = alloca {}", self.lty(&ty)));
        slot
    }

    fn alloca_anon(&mut self, ty: Ty) -> String {
        self.tmp_counter += 1;
        let slot = format!("%a{}", self.tmp_counter);
        self.allocas.push(format!("{slot} = alloca {}", self.lty(&ty)));
        slot
    }

    // ---- locals / scopes ----

    fn push_scope(&mut self) { self.scopes.push(HashMap::new()); }
    fn pop_scope(&mut self) { self.scopes.pop(); }

    fn bind(&mut self, name: &str, slot: String, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), (slot, ty));
    }

    fn lookup(&self, name: &str) -> Option<&(String, Ty)> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) { return Some(entry); }
        }
        None
    }

    // ---- function body ----

    fn gen_body_block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        if !self.terminated {
            match &b.tail {
                Some(t) => {
                    let val = self.gen_expr(t);
                    match self.return_ty {
                        Ty::Unit => self.emit_terminator("ret void"),
                        _ => {
                            let (v, _) = val.expect("non-Unit fn requires tail value");
                            self.emit_terminator(&format!("ret {} {}", self.lty(&self.return_ty), v));
                        }
                    }
                }
                None => {
                    if self.return_ty == Ty::Unit {
                        self.emit_terminator("ret void");
                    }
                    // Otherwise sema required an explicit `return`; the last
                    // stmt's terminator already closed the block.
                }
            }
        }
        self.pop_scope();
    }

    // ---- statements ----

    fn gen_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, ty, init, .. } => {
                let (val, val_ty) = self.gen_expr(init).expect("let init produces a value");
                // Prefer the annotation if present; otherwise use the value's
                // actual type. Sema has already verified they agree.
                let var_ty = ty.as_ref().map(|t| ty_from(t, self.types)).unwrap_or(val_ty);
                let slot = self.alloca_named(&name.name, var_ty.clone());
                self.emit(&format!("store {} {}, ptr {}", self.lty(&var_ty), val, slot));
                self.bind(&name.name, slot, var_ty);
            }
            StmtKind::Return(value) => {
                let ret_ty = self.return_ty.clone();
                match (value, &ret_ty) {
                    (Some(e), _) => {
                        let (v, _) = self.gen_expr(e).expect("non-Unit return value");
                        self.emit_terminator(&format!("ret {} {}", self.lty(&ret_ty), v));
                    }
                    (None, &Ty::Unit) => self.emit_terminator("ret void"),
                    (None, _) => unreachable!("sema should reject return-without-value for non-Unit"),
                }
            }
            StmtKind::While { cond, body } => self.gen_while(cond, body),
            StmtKind::For(fl) => self.gen_for(fl),
            StmtKind::Expr(e) => {
                let _ = self.gen_expr(e);
            }
        }
    }

    fn gen_while(&mut self, cond: &Expr, body: &Block) {
        let head = self.next_block_label();
        let loop_body = self.next_block_label();
        let exit = self.next_block_label();

        self.emit_terminator(&format!("br label %{head}"));
        self.open_block(&head);
        let (cond_v, _) = self.gen_expr(cond).expect("while cond produces bool");
        self.emit_terminator(&format!("br i1 {cond_v}, label %{loop_body}, label %{exit}"));

        self.open_block(&loop_body);
        self.push_scope();
        for s in &body.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &body.tail {
                // value discarded
                let _ = self.gen_expr(tail);
            }
            self.emit_terminator(&format!("br label %{head}"));
        }
        self.pop_scope();

        self.open_block(&exit);
    }

    fn gen_for(&mut self, fl: &ForLoop) {
        match fl {
            ForLoop::Range { var, iter, body } => {
                let (start_e, end_e, inclusive) = match &iter.kind {
                    ExprKind::Range { start: Some(s), end: Some(e), inclusive } => (s.as_ref(), e.as_ref(), *inclusive),
                    _ => unreachable!("sema only allows closed Range as for-iter"),
                };
                self.push_scope();
                let i_slot = self.alloca_named(&var.name, Ty::I32);
                self.bind(&var.name, i_slot.clone(), Ty::I32);
                let end_slot = self.alloca_anon(Ty::I32);

                let (start_v, _) = self.gen_expr(start_e).expect("range start");
                self.emit(&format!("store i32 {start_v}, ptr {i_slot}"));
                let (end_v, _) = self.gen_expr(end_e).expect("range end");
                self.emit(&format!("store i32 {end_v}, ptr {end_slot}"));

                let head = self.next_block_label();
                let body_lbl = self.next_block_label();
                let exit = self.next_block_label();

                self.emit_terminator(&format!("br label %{head}"));
                self.open_block(&head);
                let i_v = self.next_tmp();
                self.emit(&format!("{i_v} = load i32, ptr {i_slot}"));
                let e_v = self.next_tmp();
                self.emit(&format!("{e_v} = load i32, ptr {end_slot}"));
                let cond_v = self.next_tmp();
                let cmp = if inclusive { "sle" } else { "slt" };
                self.emit(&format!("{cond_v} = icmp {cmp} i32 {i_v}, {e_v}"));
                self.emit_terminator(&format!("br i1 {cond_v}, label %{body_lbl}, label %{exit}"));

                self.open_block(&body_lbl);
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated { break; }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail { let _ = self.gen_expr(tail); }
                    // i = i + 1
                    let cur_i = self.next_tmp();
                    self.emit(&format!("{cur_i} = load i32, ptr {i_slot}"));
                    let next_i = self.next_tmp();
                    self.emit(&format!("{next_i} = add i32 {cur_i}, 1"));
                    self.emit(&format!("store i32 {next_i}, ptr {i_slot}"));
                    self.emit_terminator(&format!("br label %{head}"));
                }
                self.pop_scope();
                self.pop_scope();

                self.open_block(&exit);
            }
            ForLoop::CStyle { init, cond, update, body } => {
                self.push_scope();
                if let Some(init) = init { self.gen_stmt(init); }

                let head = self.next_block_label();
                let body_lbl = self.next_block_label();
                let exit = self.next_block_label();

                self.emit_terminator(&format!("br label %{head}"));
                self.open_block(&head);
                let cond_v = match cond {
                    Some(c) => self.gen_expr(c).expect("for-cond produces bool").0,
                    None => "true".to_string(),
                };
                self.emit_terminator(&format!("br i1 {cond_v}, label %{body_lbl}, label %{exit}"));

                self.open_block(&body_lbl);
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated { break; }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail { let _ = self.gen_expr(tail); }
                    for u in update { let _ = self.gen_expr(u); }
                    self.emit_terminator(&format!("br label %{head}"));
                }
                self.pop_scope();
                self.pop_scope();

                self.open_block(&exit);
            }
        }
    }

    // ---- expressions ----

    /// Generate IR for an expression. Returns Some((value, type)) for value-
    /// producing expressions, None for diverging or Unit-typed expressions
    /// where the caller can't use a value.
    fn gen_expr(&mut self, e: &Expr) -> Option<(String, Ty)> {
        match &e.kind {
            ExprKind::IntLit(v, suf) => {
                use crate::lexer::NumSuffix;
                // Honor the literal's numeric suffix so downstream consumers
                // (array literals, binary arithmetic, anything that builds a
                // typed SSA temporary) emit the right LLVM width. Without
                // this, `[10u8, 20u8]` becomes `[N x i32]` and `1u64 + 2u64`
                // computes in i32 — both produce invalid IR when their
                // results meet a typed destination.
                let ty = match suf {
                    NumSuffix::I8 => Ty::I8,
                    NumSuffix::I16 => Ty::I16,
                    NumSuffix::I32 => Ty::I32,
                    NumSuffix::I64 => Ty::I64,
                    NumSuffix::U8 => Ty::U8,
                    NumSuffix::U16 => Ty::U16,
                    NumSuffix::U32 => Ty::U32,
                    NumSuffix::U64 => Ty::U64,
                    NumSuffix::Isize => Ty::Isize,
                    NumSuffix::Usize => Ty::Usize,
                    // Unsuffixed integer literal: default to i32. Sema-driven
                    // declared types still flow correctly because `let x: u64
                    // = 42` emits `store i64 42` (LLVM accepts width-agnostic
                    // numeric literals in the textual operand position).
                    NumSuffix::None | NumSuffix::F32 | NumSuffix::F64 => Ty::I32,
                };
                Some((v.to_string(), ty))
            }
            ExprKind::BoolLit(b) => Some((if *b { "true" } else { "false" }.to_string(), Ty::Bool)),
            ExprKind::FloatLit(v, suf) => {
                use crate::lexer::NumSuffix;
                let ty = match suf {
                    NumSuffix::F32 => Ty::F32,
                    _ => Ty::F64,
                };
                // LLVM IR float literals: scientific notation works for both
                // `float` and `double`. Use a hex-float for round-trippable
                // determinism — but for Phase-2 simplicity emit decimal. The
                // optimizer canonicalizes anyway.
                Some((format!("{v:?}"), ty))
            }

            ExprKind::Ident(name) => {
                let (slot, ty) = self.lookup(name).expect("sema validated").clone();
                let v = self.next_tmp();
                self.emit(&format!("{v} = load {}, ptr {slot}", self.lty(&ty)));
                Some((v, ty))
            }

            ExprKind::Block(b) => self.gen_block_expr(b),

            ExprKind::If { cond, then, else_branch } => {
                self.gen_if(cond, then, else_branch.as_deref())
            }

            ExprKind::Call { callee, args } => self.gen_call(callee, args),

            ExprKind::Binary { op, lhs, rhs } => Some(self.gen_binary(*op, lhs, rhs)),

            ExprKind::Unary { op, operand } => Some(self.gen_unary(*op, operand)),

            ExprKind::Assign { target, value, .. } => {
                self.gen_assign(target, value);
                None
            }

            ExprKind::Cast { expr, ty } => Some(self.gen_cast(expr, ty)),
            ExprKind::Path { segments } => Some(self.gen_path(segments)),
            ExprKind::StructLit { name, fields } => Some(self.gen_struct_lit(name, fields)),
            ExprKind::Field { receiver, name } => Some(self.gen_field(receiver, name)),
            ExprKind::ArrayLit { elements } => Some(self.gen_array_lit(elements)),
            ExprKind::Index { receiver, index } => Some(self.gen_index(receiver, index)),
            ExprKind::Range { .. } => {
                unreachable!("sema rejects ranges outside `for ... in`")
            }
        }
    }

    fn gen_array_lit(&mut self, elements: &[Expr]) -> (String, Ty) {
        // Determine element type from the first element. Sema enforces uniformity.
        let (first_val, elem_ty) = self.gen_expr(&elements[0]).expect("array lit element");
        let len = elements.len() as u32;
        let array_ty = Ty::Array(Box::new(elem_ty.clone()), len);
        let llvm_arr = self.lty(&array_ty);
        let llvm_elem = self.lty(&elem_ty);
        let slot = self.alloca_anon(array_ty.clone());
        // Store first element.
        let p0 = self.next_tmp();
        self.emit(&format!("{p0} = getelementptr {llvm_arr}, ptr {slot}, i32 0, i32 0"));
        self.emit(&format!("store {llvm_elem} {first_val}, ptr {p0}"));
        // Store the rest.
        for (i, e) in elements.iter().enumerate().skip(1) {
            let (v, _) = self.gen_expr(e).expect("array lit element");
            let p = self.next_tmp();
            self.emit(&format!("{p} = getelementptr {llvm_arr}, ptr {slot}, i32 0, i32 {i}"));
            self.emit(&format!("store {llvm_elem} {v}, ptr {p}"));
        }
        let v = self.next_tmp();
        self.emit(&format!("{v} = load {llvm_arr}, ptr {slot}"));
        (v, array_ty)
    }

    fn gen_index(&mut self, receiver: &Expr, index: &Expr) -> (String, Ty) {
        let (recv_ptr, recv_ty) = self.gen_place(receiver);
        let Ty::Array(elem, n) = recv_ty.clone() else { unreachable!("sema validated"); };
        let (idx_val, _) = self.gen_expr(index).expect("index has value");
        let llvm_arr = self.lty(&recv_ty);
        let llvm_elem = self.lty(&elem);
        // Bounds check: `icmp uge i64 idx, N` → branch to trap.
        let bound = self.next_tmp();
        self.emit(&format!("{bound} = icmp uge i64 {idx_val}, {n}"));
        let trap_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {bound}, label %{trap_lbl}, label %{ok_lbl}"));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&ok_lbl);
        // GEP and load.
        let ptr = self.next_tmp();
        self.emit(&format!("{ptr} = getelementptr {llvm_arr}, ptr {recv_ptr}, i64 0, i64 {idx_val}"));
        let v = self.next_tmp();
        self.emit(&format!("{v} = load {llvm_elem}, ptr {ptr}"));
        (v, (*elem).clone())
    }

    /// Build a struct literal: alloca a slot for the new value, store each
    /// field via GEP, load the whole struct as the SSA value. mem2reg
    /// promotes this to PHI/aggregate construction at -O2.
    fn gen_struct_lit(&mut self, name: &Ident, fields: &[StructLitField]) -> (String, Ty) {
        let id = *self.types.struct_by_name.get(&name.name).expect("sema validated");
        let info = self.types.struct_defs[id.0 as usize].clone();
        let struct_ty = Ty::Struct(id);
        let llvm_struct = self.lty(&struct_ty);

        let slot = self.alloca_anon(struct_ty.clone());
        for f in fields {
            let (val, _val_ty) = self.gen_expr(&f.value).expect("field init has value");
            let idx = info.field_index(&f.name.name);
            let field_ty = info.field_type(&f.name.name);
            let ptr = self.next_tmp();
            self.emit(&format!(
                "{ptr} = getelementptr {llvm_struct}, ptr {slot}, i32 0, i32 {idx}"
            ));
            self.emit(&format!("store {} {val}, ptr {ptr}", self.lty(&field_ty)));
        }
        let v = self.next_tmp();
        self.emit(&format!("{v} = load {llvm_struct}, ptr {slot}"));
        (v, struct_ty)
    }

    /// Read a field. The receiver may be a place (`p.x`), in which case we
    /// keep the address chain as long as possible (one GEP off the local's
    /// alloca), or a value (`make().x`), in which case we stash the value
    /// in a temporary alloca first.
    fn gen_field(&mut self, receiver: &Expr, name: &Ident) -> (String, Ty) {
        let (slot, struct_ty) = self.gen_place(receiver);
        let Ty::Struct(id) = struct_ty else { unreachable!("sema validated"); };
        let info = self.types.struct_defs[id.0 as usize].clone();
        let llvm_struct = self.lty(&struct_ty);
        let idx = info.field_index(&name.name);
        let field_ty = info.field_type(&name.name);
        let ptr = self.next_tmp();
        self.emit(&format!(
            "{ptr} = getelementptr {llvm_struct}, ptr {slot}, i32 0, i32 {idx}"
        ));
        let v = self.next_tmp();
        self.emit(&format!("{v} = load {}, ptr {ptr}", self.lty(&field_ty)));
        (v, field_ty)
    }

    /// Compute a (slot-pointer, type) for a place expression. For an Ident
    /// the slot is the local's alloca. For a Field chain we GEP through.
    /// For arbitrary value-producing expressions, materialize into a temp
    /// alloca so we can address it.
    fn gen_place(&mut self, e: &Expr) -> (String, Ty) {
        match &e.kind {
            ExprKind::Ident(name) => {
                let (slot, ty) = self.lookup(name).expect("sema validated").clone();
                (slot, ty)
            }
            ExprKind::Field { receiver, name } => {
                let (recv_slot, recv_ty) = self.gen_place(receiver);
                let Ty::Struct(id) = recv_ty.clone() else { unreachable!("sema validated"); };
                let info = self.types.struct_defs[id.0 as usize].clone();
                let llvm_struct = self.lty(&recv_ty);
                let idx = info.field_index(&name.name);
                let field_ty = info.field_type(&name.name);
                let ptr = self.next_tmp();
                self.emit(&format!(
                    "{ptr} = getelementptr {llvm_struct}, ptr {recv_slot}, i32 0, i32 {idx}"
                ));
                (ptr, field_ty)
            }
            ExprKind::Index { receiver, index } => {
                let (recv_slot, recv_ty) = self.gen_place(receiver);
                let Ty::Array(elem, n) = recv_ty.clone() else { unreachable!("sema validated"); };
                let (idx_val, _) = self.gen_expr(index).expect("index has value");
                let llvm_arr = self.lty(&recv_ty);
                // Bounds check.
                let bound = self.next_tmp();
                self.emit(&format!("{bound} = icmp uge i64 {idx_val}, {n}"));
                let trap_lbl = self.next_block_label();
                let ok_lbl = self.next_block_label();
                self.emit_terminator(&format!("br i1 {bound}, label %{trap_lbl}, label %{ok_lbl}"));
                self.open_block(&trap_lbl);
                self.emit("call void @llvm.trap()");
                self.emit_terminator("unreachable");
                self.open_block(&ok_lbl);
                let ptr = self.next_tmp();
                self.emit(&format!("{ptr} = getelementptr {llvm_arr}, ptr {recv_slot}, i64 0, i64 {idx_val}"));
                (ptr, (*elem).clone())
            }
            _ => {
                // Value expression: stash in a temp alloca and address that.
                let (val, ty) = self.gen_expr(e).expect("place fallback expects a value");
                let slot = self.alloca_anon(ty.clone());
                self.emit(&format!("store {} {val}, ptr {slot}", self.lty(&ty)));
                (slot, ty)
            }
        }
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> (String, Ty) {
        // Short-circuit evaluation for && and ||.
        match op {
            BinOp::And => return self.gen_short_circuit(lhs, rhs, true),
            BinOp::Or  => return self.gen_short_circuit(lhs, rhs, false),
            _ => {}
        }
        let (l, lt) = self.gen_expr(lhs).expect("binary lhs has value");
        let (r, _rt) = self.gen_expr(rhs).expect("binary rhs has value");
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if lt.is_float() {
                    let v = self.next_tmp();
                    let fop = match op { BinOp::Add => "fadd", BinOp::Sub => "fsub", BinOp::Mul => "fmul", _ => unreachable!() };
                    self.emit(&format!("{v} = {fop} {} {l}, {r}", self.lty(&lt)));
                    return (v, lt);
                }
                // Integer: signed gets debug overflow checks, unsigned wraps.
                if lt.is_signed_int() && self.mode == BuildMode::Debug {
                    return (self.arith_with_overflow_check(op, &lt, &l, &r), lt);
                }
                let v = self.next_tmp();
                let iop = match op { BinOp::Add => "add", BinOp::Sub => "sub", BinOp::Mul => "mul", _ => unreachable!() };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::Div => {
                if lt.is_float() {
                    let v = self.next_tmp();
                    self.emit(&format!("{v} = fdiv {} {l}, {r}", self.lty(&lt)));
                    return (v, lt);
                }
                (self.divide_with_zero_check(op, &lt, &l, &r), lt)
            }
            BinOp::Mod => {
                // Sema rejects float `%`; only integer reaches here.
                (self.divide_with_zero_check(op, &lt, &l, &r), lt)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let v = self.next_tmp();
                let cmp = cmp_op_for_type(op, &lt);
                let inst = if lt.is_float() { "fcmp" } else { "icmp" };
                self.emit(&format!("{v} = {inst} {cmp} {} {l}, {r}", self.lty(&lt)));
                (v, Ty::Bool)
            }
            BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => {
                // Wrapping operators emit plain integer `add/sub/mul`
                // regardless of build mode: documents intent and gives
                // predictable wrap behavior in debug too. Sema has already
                // restricted these to integer operands.
                let v = self.next_tmp();
                let iop = match op {
                    BinOp::AddWrap => "add",
                    BinOp::SubWrap => "sub",
                    BinOp::MulWrap => "mul",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
            _ => unreachable!("sema rejects bitwise/shift"),
        }
    }

    /// Emit a debug-mode checked signed `+ - *` using the
    /// `llvm.{sadd,ssub,smul}.with.overflow.iN` intrinsic, where N is chosen
    /// from the operand type. On overflow, trap and `unreachable`; otherwise
    /// extract the result.
    fn arith_with_overflow_check(&mut self, op: BinOp, ty: &Ty, l: &str, r: &str) -> String {
        let intrinsic = match op {
            BinOp::Add => "sadd",
            BinOp::Sub => "ssub",
            BinOp::Mul => "smul",
            _ => unreachable!(),
        };
        let llvm_t = self.lty(&ty);
        let bits = ty_bit_width(&ty);
        let pair = self.next_tmp();
        self.emit(&format!(
            "{pair} = call {{{llvm_t}, i1}} @llvm.{intrinsic}.with.overflow.i{bits}({llvm_t} {l}, {llvm_t} {r})"
        ));
        let overflow_bit = self.next_tmp();
        self.emit(&format!("{overflow_bit} = extractvalue {{{llvm_t}, i1}} {pair}, 1"));
        let trap_lbl = self.next_block_label();
        let cont_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {overflow_bit}, label %{trap_lbl}, label %{cont_lbl}"
        ));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&cont_lbl);
        let result = self.next_tmp();
        self.emit(&format!("{result} = extractvalue {{{llvm_t}, i1}} {pair}, 0"));
        result
    }

    /// Emit a divide-by-zero check before `sdiv` / `udiv` / `srem` / `urem`.
    /// Trap and `unreachable` on zero (always — both modes per §2.3).
    fn divide_with_zero_check(&mut self, op: BinOp, ty: &Ty, l: &str, r: &str) -> String {
        let llvm_op = match (op, ty.is_signed_int()) {
            (BinOp::Div, true) => "sdiv",
            (BinOp::Div, false) => "udiv",
            (BinOp::Mod, true) => "srem",
            (BinOp::Mod, false) => "urem",
            _ => unreachable!(),
        };
        let llvm_t = self.lty(&ty);
        let zero_check = self.next_tmp();
        self.emit(&format!("{zero_check} = icmp eq {llvm_t} {r}, 0"));
        let trap_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {zero_check}, label %{trap_lbl}, label %{ok_lbl}"
        ));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&ok_lbl);
        let result = self.next_tmp();
        self.emit(&format!("{result} = {llvm_op} {llvm_t} {l}, {r}"));
        result
    }

    fn gen_short_circuit(&mut self, lhs: &Expr, rhs: &Expr, is_and: bool) -> (String, Ty) {
        // `a && b`:   if a then b else false
        // `a || b`:   if a then true else b
        let result_slot = self.alloca_anon(Ty::Bool);
        let (lv, _) = self.gen_expr(lhs).expect("lhs of && / ||");
        let then_lbl = self.next_block_label();
        let else_lbl = self.next_block_label();
        let merge_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {lv}, label %{then_lbl}, label %{else_lbl}"));

        self.open_block(&then_lbl);
        let (v_then, v_else) = if is_and {
            let (rv, _) = self.gen_expr(rhs).expect("rhs of &&");
            self.emit(&format!("store i1 {rv}, ptr {result_slot}"));
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            self.open_block(&else_lbl);
            self.emit(&format!("store i1 false, ptr {result_slot}"));
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            ("rhs".to_string(), "false".to_string())
        } else {
            self.emit(&format!("store i1 true, ptr {result_slot}"));
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            self.open_block(&else_lbl);
            let (rv, _) = self.gen_expr(rhs).expect("rhs of ||");
            self.emit(&format!("store i1 {rv}, ptr {result_slot}"));
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            ("true".to_string(), "rhs".to_string())
        };
        let _ = (v_then, v_else);

        self.open_block(&merge_lbl);
        let v = self.next_tmp();
        self.emit(&format!("{v} = load i1, ptr {result_slot}"));
        (v, Ty::Bool)
    }

    fn gen_unary(&mut self, op: UnaryOp, operand: &Expr) -> (String, Ty) {
        let (v, ty) = self.gen_expr(operand).expect("unary operand has value");
        let r = self.next_tmp();
        match op {
            UnaryOp::Neg => {
                if ty.is_float() {
                    self.emit(&format!("{r} = fneg {} {v}", self.lty(&ty)));
                } else {
                    // Sema only allows signed integers and floats for `-`.
                    // Signed integer negation: in debug, INT_MIN cannot be negated;
                    // we emit `sub` and rely on Phase-3 hardening for that case.
                    self.emit(&format!("{r} = sub {} 0, {v}", self.lty(&ty)));
                }
                (r, ty)
            }
            UnaryOp::Not => {
                self.emit(&format!("{r} = xor i1 {v}, true"));
                (r, Ty::Bool)
            }
            _ => unreachable!("sema rejects ~ / & / * / &mut in Phase 1"),
        }
    }

    /// Lower `EnumName::Variant` to its integer literal value (the variant's
    /// declaration index, 0-based). Phase 2A always emits as `i32`.
    fn gen_path(&mut self, segments: &[Ident]) -> (String, Ty) {
        debug_assert_eq!(segments.len(), 2, "Phase 2A paths are 2 segments");
        let enum_name = &segments[0].name;
        let variant_name = &segments[1].name;
        let id = *self.types.enum_by_name.get(enum_name)
            .expect("sema validated enum name");
        let idx = self.types.enum_defs[id.0 as usize]
            .variants.get(variant_name)
            .copied()
            .expect("sema validated variant name");
        (idx.to_string(), Ty::Enum(id))
    }

    fn gen_cast(&mut self, expr: &Expr, target: &Type) -> (String, Ty) {
        let (v, from_actual) = self.gen_expr(expr).expect("cast operand has value");
        let to_actual = ty_from(target, self.types);
        // Enums lower to i32 at LLVM level. For cast instruction selection,
        // treat enum operands as their underlying i32 form. Sema disallows
        // int → enum, so we only need to handle the source side.
        let from = if from_actual.is_enum() { Ty::I32 } else { from_actual };
        let to = to_actual.clone();
        if from == to { return (v, to_actual); }
        let from_t = self.lty(&from);
        let to_t = self.lty(&to);
        let r = self.next_tmp();
        let inst: &'static str = match (&from, &to) {
            // int → int, same/diff width
            (a, b) if a.is_int() && b.is_int() => {
                let aw = ty_bit_width(a);
                let bw = ty_bit_width(b);
                if bw == aw {
                    // No-op (signed/unsigned reinterpret); emit a bitcast for IR validity.
                    self.emit(&format!("{r} = bitcast {from_t} {v} to {to_t}"));
                    return (r, to);
                } else if bw < aw {
                    "trunc"
                } else if a.is_signed_int() {
                    "sext"
                } else {
                    "zext"
                }
            }
            // bool → int
            (Ty::Bool, b) if b.is_int() => "zext",
            // int → float
            (a, b) if a.is_signed_int() && b.is_float() => "sitofp",
            (a, b) if a.is_unsigned_int() && b.is_float() => "uitofp",
            // float → int
            (a, b) if a.is_float() && b.is_signed_int() => "fptosi",
            (a, b) if a.is_float() && b.is_unsigned_int() => "fptoui",
            // float → float (different widths)
            (a, b) if a.is_float() && b.is_float() => {
                if ty_bit_width(b) > ty_bit_width(a) { "fpext" } else { "fptrunc" }
            }
            _ => unreachable!("sema rejects unsupported casts: {:?} → {:?}", from, to),
        };
        self.emit(&format!("{r} = {inst} {from_t} {v} to {to_t}"));
        (r, to)
    }

    fn gen_call(&mut self, callee: &Expr, args: &[Expr]) -> Option<(String, Ty)> {
        match &callee.kind {
            ExprKind::Ident(name) => self.gen_named_call(name, args),
            ExprKind::Field { receiver, name } => self.gen_method_call(receiver, name, args),
            ExprKind::Path { segments } => self.gen_assoc_call(segments, args),
            _ => unreachable!("sema validates callee shape"),
        }
    }

    fn gen_named_call(&mut self, name: &str, args: &[Expr]) -> Option<(String, Ty)> {
        // Special case: println(i32) → call printf with our %d\n format.
        if name == "println" {
            let (av, _) = self.gen_expr(&args[0]).expect("println arg");
            let v = self.next_tmp();
            self.emit(&format!(
                "{v} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_int_nl, i32 {av})"
            ));
            return None;
        }
        let sig = self.sigs.get(name).expect("sema validated function exists").clone();
        let mut arg_vals: Vec<(String, Ty)> = Vec::with_capacity(args.len());
        for a in args {
            arg_vals.push(self.gen_expr(a).expect("call arg is a value"));
        }
        let mut arg_str = String::new();
        for (i, ((v, _), ty)) in arg_vals.iter().zip(sig.params.iter()).enumerate() {
            if i > 0 { arg_str.push_str(", "); }
            arg_str.push_str(&format!("{} {v}", self.lty(&*ty)));
        }
        match sig.return_type {
            Ty::Unit => {
                self.emit(&format!("call void @{name}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = call {} @{name}({arg_str})", self.lty(&ret)));
                Some((v, ret))
            }
        }
    }

    fn gen_method_call(&mut self, receiver: &Expr, name: &Ident, args: &[Expr]) -> Option<(String, Ty)> {
        // Materialize the receiver as a place (pointer) — works for Ident,
        // Field chains, and value-producing temporaries (gen_place handles each).
        let (recv_ptr, recv_ty) = self.gen_place(receiver);
        let Ty::Struct(id) = recv_ty else { unreachable!("sema validated") };
        let struct_name = self.types.struct_defs[id.0 as usize].name.clone();
        let info = self.types.struct_defs[id.0 as usize]
            .methods.get(&name.name).expect("sema validated").clone();
        let _rcv = info.receiver.expect("sema validated instance call");
        let mangled = mangle(&struct_name, &name.name);

        // Build the LLVM call argument list. All three receiver kinds
        // (`self`, `mut self`, `move self`) pass the struct's address as a
        // `ptr`; the receiver kind only matters for sema-level mutability
        // and move-tracking checks.
        let mut arg_parts: Vec<String> = vec![format!("ptr {recv_ptr}")];
        for (a, pty) in args.iter().zip(info.params.iter()) {
            let (v, _) = self.gen_expr(a).expect("call arg has value");
            arg_parts.push(format!("{} {v}", self.lty(&*pty)));
        }
        let arg_str = arg_parts.join(", ");

        match info.return_type {
            Ty::Unit => {
                self.emit(&format!("call void @{mangled}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = call {} @{mangled}({arg_str})", self.lty(&ret)));
                Some((v, ret))
            }
        }
    }

    fn gen_assoc_call(&mut self, segments: &[Ident], args: &[Expr]) -> Option<(String, Ty)> {
        // Sema verified `Type::method` is an associated function (no receiver).
        let type_name = &segments[0].name;
        let method_name = &segments[1].name;
        let id = *self.types.struct_by_name.get(type_name).expect("sema validated");
        let info = self.types.struct_defs[id.0 as usize]
            .methods.get(method_name).expect("sema validated").clone();
        let mangled = mangle(type_name, method_name);

        let mut arg_parts: Vec<String> = Vec::new();
        for (a, pty) in args.iter().zip(info.params.iter()) {
            let (v, _) = self.gen_expr(a).expect("call arg has value");
            arg_parts.push(format!("{} {v}", self.lty(&*pty)));
        }
        let arg_str = arg_parts.join(", ");
        match info.return_type {
            Ty::Unit => {
                self.emit(&format!("call void @{mangled}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = call {} @{mangled}({arg_str})", self.lty(&ret)));
                Some((v, ret))
            }
        }
    }

    fn gen_if(&mut self, cond: &Expr, then: &Block, else_branch: Option<&Expr>) -> Option<(String, Ty)> {
        let (cond_v, _) = self.gen_expr(cond).expect("if cond is bool");
        let result_ty = block_value_ty(then).or_else(|| else_branch.and_then(expr_value_ty));
        let result_slot = match result_ty {
            Some(ty) if ty != Ty::Unit => Some((self.alloca_anon(ty.clone()), ty)),
            _ => None,
        };

        let then_lbl = self.next_block_label();
        let else_lbl = self.next_block_label();
        let merge_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {cond_v}, label %{then_lbl}, label %{else_lbl}"));

        self.open_block(&then_lbl);
        self.gen_block_into_slot(then, result_slot.as_ref(), &merge_lbl);

        self.open_block(&else_lbl);
        match else_branch {
            Some(eb) => match &eb.kind {
                ExprKind::Block(b) => self.gen_block_into_slot(b, result_slot.as_ref(), &merge_lbl),
                ExprKind::If { .. } => {
                    let v = self.gen_expr(eb);
                    if !self.terminated {
                        if let (Some((slot, ty)), Some((rv, _))) = (&result_slot, &v) {
                            self.emit(&format!("store {} {rv}, ptr {slot}", self.lty(&*ty)));
                        }
                        self.emit_terminator(&format!("br label %{merge_lbl}"));
                    }
                }
                _ => unreachable!("else branch is Block or If per parser"),
            }
            None => {
                self.emit_terminator(&format!("br label %{merge_lbl}"));
            }
        }

        self.open_block(&merge_lbl);
        match result_slot {
            Some((slot, ty)) => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = load {} , ptr {slot}", self.lty(&ty)));
                Some((v, ty))
            }
            None => None,
        }
    }

    fn gen_block_into_slot(&mut self, b: &Block, slot: Option<&(String, Ty)>, merge_lbl: &str) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &b.tail {
                let v = self.gen_expr(tail);
                if let (Some((s, ty)), Some((rv, _))) = (slot, v) {
                    self.emit(&format!("store {} {rv}, ptr {s}", self.lty(&*ty)));
                }
            }
            self.emit_terminator(&format!("br label %{merge_lbl}"));
        }
        self.pop_scope();
    }

    fn gen_block_expr(&mut self, b: &Block) -> Option<(String, Ty)> {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        let result = if self.terminated {
            None
        } else {
            match &b.tail {
                Some(t) => self.gen_expr(t),
                None => None,
            }
        };
        self.pop_scope();
        result
    }

    fn gen_assign(&mut self, target: &Expr, value: &Expr) {
        // Compute the place slot (Ident or Field chain). gen_place returns
        // a pointer that we can store to directly.
        let (slot, target_ty) = self.gen_place(target);
        let (v, _) = self.gen_expr(value).expect("assigned value");
        self.emit(&format!("store {} {v}, ptr {slot}", self.lty(&target_ty)));
    }
}

// ---- helpers ----

fn cmp_op_for_type(op: BinOp, ty: &Ty) -> &'static str {
    if ty.is_float() {
        // Ordered comparisons (NaN comparisons are false). Bool eq/ne handled via i1 icmp.
        return match op {
            BinOp::Eq => "oeq",
            BinOp::Ne => "one",
            BinOp::Lt => "olt",
            BinOp::Le => "ole",
            BinOp::Gt => "ogt",
            BinOp::Ge => "oge",
            _ => unreachable!(),
        };
    }
    match op {
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => if ty.is_unsigned_int() { "ult" } else { "slt" },
        BinOp::Le => if ty.is_unsigned_int() { "ule" } else { "sle" },
        BinOp::Gt => if ty.is_unsigned_int() { "ugt" } else { "sgt" },
        BinOp::Ge => if ty.is_unsigned_int() { "uge" } else { "sge" },
        _ => unreachable!(),
    }
}

/// Try to figure out the type of an expression structurally. Used to size the
/// alloca slot for `if` results when sema didn't hand us a side table.
/// Returns None if the type can't be determined cheaply (e.g. function call
/// without resolved sig). For Phase 1, this is enough; in Phase 2+ a typed-AST
/// side table is the right fix.
fn expr_value_ty(e: &Expr) -> Option<Ty> {
    use crate::lexer::NumSuffix;
    match &e.kind {
        ExprKind::IntLit(_, suf) => Some(match suf {
            NumSuffix::I8 => Ty::I8, NumSuffix::I16 => Ty::I16,
            NumSuffix::I32 => Ty::I32, NumSuffix::I64 => Ty::I64,
            NumSuffix::U8 => Ty::U8, NumSuffix::U16 => Ty::U16,
            NumSuffix::U32 => Ty::U32, NumSuffix::U64 => Ty::U64,
            NumSuffix::Isize => Ty::Isize, NumSuffix::Usize => Ty::Usize,
            _ => Ty::I32, // unsuffixed default
        }),
        ExprKind::FloatLit(_, suf) => Some(match suf {
            NumSuffix::F32 => Ty::F32,
            _ => Ty::F64,
        }),
        ExprKind::BoolLit(_) => Some(Ty::Bool),
        ExprKind::Block(b) => block_value_ty(b),
        ExprKind::If { then, .. } => block_value_ty(then),
        ExprKind::Binary { op, lhs, .. } => match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
            | BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => expr_value_ty(lhs),
            _ => Some(Ty::Bool),
        },
        ExprKind::Unary { op, operand } => match op {
            UnaryOp::Neg => expr_value_ty(operand),
            UnaryOp::Not => Some(Ty::Bool),
            _ => None,
        },
        // Path always names an enum variant, and every enum lowers to `i32`.
        // The exact `EnumId` matters for sema but not for codegen's slot
        // allocation, so we report `i32` here. (Sema has already verified
        // both arms of any `if` agree on the actual enum type.)
        ExprKind::Path { .. } => Some(Ty::I32),
        // For Cast we don't have the enum table in this free function;
        // callers that need the type should use `gen_expr`'s return value.
        _ => None,
    }
}

fn block_value_ty(b: &Block) -> Option<Ty> {
    b.tail.as_deref().and_then(expr_value_ty)
}

fn sanitize(s: &str) -> String {
    // LLVM names accept a wide set; identifiers from C+ (ASCII alnum + _) are fine.
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema;
    use std::path::PathBuf;

    fn gen_src(src: &str) -> String { gen_src_with(src, BuildMode::Debug) }

    fn gen_src_with(src: &str, mode: BuildMode) -> String {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(diags.is_empty(), "sema errors: {diags:#?}");
        generate(&prog, mode)
    }

    #[test]
    fn preamble_includes_intrinsics() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(ir.contains("declare i32 @printf(ptr noundef, ...)"));
        assert!(ir.contains("@.fmt_int_nl"));
        assert!(ir.contains("declare void @llvm.trap()"));
        assert!(ir.contains("declare {i32, i1} @llvm.sadd.with.overflow.i32"));
        assert!(ir.contains("declare {i32, i1} @llvm.ssub.with.overflow.i32"));
        assert!(ir.contains("declare {i32, i1} @llvm.smul.with.overflow.i32"));
    }

    #[test]
    fn main_returns_int_literal() {
        let ir = gen_src("fn main() -> i32 { return 42; }");
        assert!(ir.contains("define i32 @main()"));
        assert!(ir.contains("ret i32 42"));
    }

    #[test]
    fn debug_arithmetic_uses_overflow_intrinsics() {
        let ir = gen_src_with("fn main() -> i32 { return 1 + 2 * 3 - 4; }", BuildMode::Debug);
        assert!(ir.contains("call {i32, i1} @llvm.sadd.with.overflow.i32"));
        assert!(ir.contains("call {i32, i1} @llvm.ssub.with.overflow.i32"));
        assert!(ir.contains("call {i32, i1} @llvm.smul.with.overflow.i32"));
        assert!(ir.contains("call void @llvm.trap()"));
        assert!(ir.contains("unreachable"));
    }

    #[test]
    fn release_arithmetic_uses_plain_ops() {
        let ir = gen_src_with("fn main() -> i32 { return 1 + 2 * 3 - 4; }", BuildMode::Release);
        // Plain ops, no intrinsic calls in arithmetic body.
        assert!(ir.contains(" = add i32 "));
        assert!(ir.contains(" = sub i32 "));
        assert!(ir.contains(" = mul i32 "));
        // No sadd intrinsic *call* (declarations remain in preamble).
        assert!(!ir.contains("call {i32, i1} @llvm.sadd.with.overflow"));
        assert!(!ir.contains("call {i32, i1} @llvm.ssub.with.overflow"));
        assert!(!ir.contains("call {i32, i1} @llvm.smul.with.overflow"));
    }

    #[test]
    fn division_always_traps_on_zero() {
        // Both modes emit the zero-check.
        for mode in [BuildMode::Debug, BuildMode::Release] {
            let ir = gen_src_with("fn main() -> i32 { return 10 / 2; }", mode);
            assert!(ir.contains("icmp eq i32"), "mode={mode:?}: {ir}");
            assert!(ir.contains(" = sdiv i32 "), "mode={mode:?}");
            assert!(ir.contains("call void @llvm.trap()"), "mode={mode:?}");
        }
    }

    #[test]
    fn modulo_always_traps_on_zero() {
        let ir = gen_src("fn main() -> i32 { return 10 % 3; }");
        assert!(ir.contains("icmp eq i32"));
        assert!(ir.contains(" = srem i32 "));
    }

    #[test]
    fn let_emits_alloca_and_store() {
        let ir = gen_src("fn main() -> i32 { let x: i32 = 7; return x; }");
        assert!(ir.contains("alloca i32"));
        assert!(ir.contains("store i32 7, ptr"));
        assert!(ir.contains("load i32, ptr"));
    }

    #[test]
    fn comparison_emits_icmp() {
        let ir = gen_src("fn main() -> i32 { return if 1 < 2 { 1 } else { 0 }; }");
        assert!(ir.contains("icmp slt i32"));
        assert!(ir.contains("br i1"));
    }

    #[test]
    fn while_loop_has_header_and_exit() {
        let ir = gen_src(
            "fn main() -> i32 { let mut i: i32 = 0; while i < 5 { i = i + 1; } return i; }"
        );
        assert!(ir.contains("br label %bb"));
        assert!(ir.contains("icmp slt"));
    }

    #[test]
    fn for_range_inclusive_uses_sle() {
        let ir = gen_src(
            "fn main() -> i32 { let mut s: i32 = 0; for i in 0..=3 { s = s + i; } return s; }"
        );
        assert!(ir.contains("icmp sle i32"));
    }

    #[test]
    fn for_range_exclusive_uses_slt() {
        let ir = gen_src(
            "fn main() -> i32 { let mut s: i32 = 0; for i in 0..3 { s = s + i; } return s; }"
        );
        assert!(ir.contains("icmp slt i32"));
    }

    #[test]
    fn function_call_emits_call() {
        let ir = gen_src(
            "fn double(x: i32) -> i32 { return x + x; }\nfn main() -> i32 { return double(21); }"
        );
        assert!(ir.contains("define i32 @double"));
        assert!(ir.contains("call i32 @double"));
    }

    #[test]
    fn println_lowers_to_printf() {
        let ir = gen_src("fn main() -> i32 { println(42); return 0; }");
        assert!(ir.contains("call i32 (ptr, ...) @printf(ptr noundef @.fmt_int_nl, i32 42"));
    }

    #[test]
    fn negation_emits_sub_zero() {
        let ir = gen_src("fn main() -> i32 { let x: i32 = 5; return -x; }");
        assert!(ir.contains("sub i32 0,"));
    }

    #[test]
    fn logical_not_uses_xor() {
        let ir = gen_src("fn main() -> i32 { return if !(1 < 2) { 1 } else { 0 }; }");
        assert!(ir.contains("xor i1"));
    }

    #[test]
    fn factorial_compiles_to_ir() {
        let src = include_str!("../../docs/examples/factorial.cplus");
        let ir = gen_src(src);
        assert!(ir.contains("define i32 @factorial(i32"));
        assert!(ir.contains("define i32 @main()"));
    }

    #[test]
    fn fibonacci_compiles_to_ir() {
        let src = include_str!("../../docs/examples/fibonacci.cplus");
        let ir = gen_src(src);
        assert!(ir.contains("define i32 @fib(i32"));
    }

    #[test]
    fn sum_range_compiles_to_ir() {
        let src = include_str!("../../docs/examples/sum_range.cplus");
        let _ir = gen_src(src);
    }

    #[test]
    fn c_for_compiles_to_ir() {
        let src = include_str!("../../docs/examples/c_for.cplus");
        let _ir = gen_src(src);
    }

    // ---- Phase 2 slice 1 codegen ----

    #[test]
    fn preamble_declares_overflow_intrinsics_for_all_widths() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        for op in ["sadd", "ssub", "smul"] {
            for bits in [8, 16, 32, 64] {
                let needle = format!("declare {{i{bits}, i1}} @llvm.{op}.with.overflow.i{bits}");
                assert!(ir.contains(&needle), "missing {needle} in: {ir}");
            }
        }
    }

    #[test]
    fn i64_arithmetic_uses_64bit_overflow_intrinsic_in_debug() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: i64 = 5; let _b: i64 = a + a; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains("call {i64, i1} @llvm.sadd.with.overflow.i64"));
    }

    #[test]
    fn i8_arithmetic_uses_8bit_overflow_intrinsic_in_debug() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: i8 = 1; let _b: i8 = a + a; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains("call {i8, i1} @llvm.sadd.with.overflow.i8"));
    }

    #[test]
    fn unsigned_arithmetic_uses_plain_op_no_overflow_check() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: u32 = 5; let _b: u32 = a + a; return 0; }",
            BuildMode::Debug,
        );
        // Unsigned overflow is well-defined wrapping; no intrinsic *call*.
        // (Declarations in preamble are fine.)
        assert!(!ir.contains("call {i32, i1}"));
        assert!(ir.contains(" = add i32 "));
    }

    #[test]
    fn unsigned_division_uses_udiv_with_zero_check() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: u32 = 10; let b: u32 = 2; let _c: u32 = a / b; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains(" = udiv i32 "));
        assert!(ir.contains("icmp eq i32"));
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn float_arithmetic_uses_fadd_etc() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let _b: f64 = a + a * a; return 0; }");
        assert!(ir.contains(" = fadd double "));
        assert!(ir.contains(" = fmul double "));
        // No overflow-intrinsic *call* (the declaration in preamble is fine).
        assert_eq!(count(&ir, "call {"), 0, "no checked-arith calls expected for float ops");
    }

    #[test]
    fn float_division_no_zero_check() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let b: f64 = 2.0; let _c: f64 = a / b; return 0; }");
        assert!(ir.contains(" = fdiv double "));
        // Float div doesn't trap; no zero check.
        // (Other code paths may still have icmp eq for integer divs; assert
        // the fdiv lacks a preceding zero-check on a float.)
        let lines: Vec<&str> = ir.lines().collect();
        let fdiv_line = lines.iter().position(|l| l.contains(" = fdiv ")).unwrap();
        let preceding = &lines[fdiv_line.saturating_sub(3)..fdiv_line];
        for line in preceding {
            assert!(!line.contains("icmp eq double"), "float div should not have a zero-check");
        }
    }

    #[test]
    fn float_negation_uses_fneg() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 5.0; let _b: f64 = -a; return 0; }");
        assert!(ir.contains(" = fneg double "));
    }

    #[test]
    fn signed_comparison_uses_signed_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: i64 = 1; let b: i64 = 2; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = icmp slt i64 "));
    }

    #[test]
    fn unsigned_comparison_uses_unsigned_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: u64 = 1; let b: u64 = 2; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = icmp ult i64 "));
    }

    #[test]
    fn float_comparison_uses_ordered_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let b: f64 = 2.0; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = fcmp olt double "));
    }

    #[test]
    fn cast_int_widen_uses_sext() {
        let ir = gen_src("fn main() -> i32 { let a: i8 = 5; let _b: i32 = a as i32; return 0; }");
        assert!(ir.contains(" = sext i8 "));
    }

    #[test]
    fn cast_uint_widen_uses_zext() {
        let ir = gen_src("fn main() -> i32 { let a: u8 = 5; let _b: u32 = a as u32; return 0; }");
        assert!(ir.contains(" = zext i8 "));
    }

    #[test]
    fn cast_int_narrow_uses_trunc() {
        let ir = gen_src("fn main() -> i32 { let a: i64 = 5; let _b: i8 = a as i8; return 0; }");
        assert!(ir.contains(" = trunc i64 "));
    }

    #[test]
    fn cast_int_to_float_uses_sitofp_or_uitofp() {
        let ir1 = gen_src("fn main() -> i32 { let a: i32 = 5; let _b: f64 = a as f64; return 0; }");
        assert!(ir1.contains(" = sitofp "));
        let ir2 = gen_src("fn main() -> i32 { let a: u32 = 5; let _b: f64 = a as f64; return 0; }");
        assert!(ir2.contains(" = uitofp "));
    }

    #[test]
    fn cast_float_to_int_uses_fptosi_or_fptoui() {
        let ir1 = gen_src("fn main() -> i32 { let a: f64 = 1.5; let _b: i32 = a as i32; return 0; }");
        assert!(ir1.contains(" = fptosi "));
        let ir2 = gen_src("fn main() -> i32 { let a: f64 = 1.5; let _b: u32 = a as u32; return 0; }");
        assert!(ir2.contains(" = fptoui "));
    }

    #[test]
    fn cast_float_widths_uses_fpext_or_fptrunc() {
        let ir1 = gen_src("fn main() -> i32 { let a: f32 = 1.0; let _b: f64 = a as f64; return 0; }");
        assert!(ir1.contains(" = fpext "));
        let ir2 = gen_src("fn main() -> i32 { let a: f64 = 1.0; let _b: f32 = a as f32; return 0; }");
        assert!(ir2.contains(" = fptrunc "));
    }

    #[test]
    fn cast_bool_to_int_uses_zext() {
        let ir = gen_src("fn main() -> i32 { let _b: i32 = true as i32; return 0; }");
        assert!(ir.contains(" = zext i1 "));
    }

    #[test]
    fn cast_signed_to_unsigned_same_width_is_bitcast() {
        let ir = gen_src("fn main() -> i32 { let a: i32 = 5; let _b: u32 = a as u32; return 0; }");
        // Same-width int cast is a no-op; use bitcast for IR validity.
        assert!(ir.contains(" = bitcast i32 "));
    }

    #[test]
    fn phase2_samples_compile_to_ir() {
        for name in ["mixed_ints.cplus", "float_arith.cplus", "unsigned.cplus", "direction.cplus"] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    // ---- Phase 2 slice 2A: enums + paths ----

    #[test]
    fn enum_path_lowers_to_int_constant() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { return Color::Green as i32; }"
        );
        // Green is index 1; the cast is enum→i32 which is a no-op.
        // The ret should reference the constant `1`.
        assert!(ir.contains("ret i32 1"), "expected `ret i32 1`, got: {ir}");
    }

    #[test]
    fn enum_equality_uses_icmp_eq_i32() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let c: Color = Color::Red; return if c == Color::Green { 1 } else { 0 }; }"
        );
        assert!(ir.contains("icmp eq i32"));
    }

    #[test]
    fn enum_typed_local_is_i32_alloca() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let _c: Color = Color::Red; return 0; }"
        );
        // Should have an i32 alloca for the Color local.
        assert!(ir.contains("alloca i32"));
    }

    #[test]
    fn enum_passed_as_argument_uses_i32() {
        let ir = gen_src(include_str!("../../docs/examples/direction.cplus"));
        assert!(ir.contains("define i32 @opposite(i32"));
    }

    // ---- Phase 2 slice 2B: structs ----

    #[test]
    fn struct_decl_emits_named_type() {
        let ir = gen_src(
            "struct Point { x: i32, y: i32 }\nfn main() -> i32 { return 0; }"
        );
        assert!(
            ir.contains("%Point = type { i32, i32 }"),
            "expected struct decl in IR: {ir}"
        );
    }

    #[test]
    fn struct_literal_emits_alloca_and_per_field_store() {
        let ir = gen_src(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let _p: Point = Point { x: 1, y: 2 }; return 0; }"
        );
        assert!(ir.contains("alloca %Point"), "expected struct alloca: {ir}");
        assert!(ir.contains("getelementptr %Point"), "expected GEP into struct: {ir}");
        assert!(ir.contains("store i32 1, ptr"));
        assert!(ir.contains("store i32 2, ptr"));
    }

    #[test]
    fn struct_field_read_uses_gep_load() {
        let ir = gen_src(
            "struct Point { x: i32, y: i32 }\n\
             fn first(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { return 0; }"
        );
        assert!(ir.contains("getelementptr %Point"));
        assert!(ir.contains("load i32, ptr"));
    }

    #[test]
    fn struct_field_write_uses_gep_store() {
        let ir = gen_src(
            "struct Counter { count: i32 }\n\
             fn main() -> i32 { let mut c: Counter = Counter { count: 0 }; c.count = 5; return 0; }"
        );
        assert!(ir.contains("getelementptr %Counter"));
        assert!(ir.contains("store i32 5, ptr"));
    }

    #[test]
    fn struct_passed_by_value_in_signature() {
        let ir = gen_src(include_str!("../../docs/examples/point.cplus"));
        assert!(ir.contains("define i32 @distance_squared(%Point"));
    }

    #[test]
    fn nested_struct_chain_uses_chained_gep() {
        let ir = gen_src(include_str!("../../docs/examples/nested.cplus"));
        // The struct has fields { from: Point, to: Point }; the load chain
        // should GEP twice (Line.to then Point.x / Point.y).
        let geps = ir.matches("getelementptr").count();
        assert!(geps >= 4, "expected several GEPs in nested struct access; got {geps}: {ir}");
    }

    #[test]
    fn empty_struct_emits_empty_named_type() {
        let ir = gen_src(
            "struct Empty {}\nfn main() -> i32 { let _e: Empty = Empty {}; return 0; }"
        );
        assert!(ir.contains("%Empty = type {  }"), "expected empty struct type: {ir}");
    }

    #[test]
    fn phase2b_samples_compile_to_ir() {
        for name in ["point.cplus", "mutable_struct.cplus", "nested.cplus"] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    // ---- Phase 2 slice 2C: methods + impl blocks ----

    #[test]
    fn method_name_is_mangled() {
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn new(x: i32) -> P { return P { x: x }; } }\n\
             fn main() -> i32 { let _p: P = P::new(5); return 0; }"
        );
        assert!(ir.contains("define %P @P.new(i32 "), "expected mangled name: {ir}");
        assert!(ir.contains("call %P @P.new("), "expected mangled call: {ir}");
    }

    #[test]
    fn read_self_method_takes_ptr_param() {
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }"
        );
        assert!(ir.contains("define i32 @P.get(ptr "), "expected ptr param for self: {ir}");
    }

    #[test]
    fn mut_self_method_takes_ptr_param() {
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn set(mut self, v: i32) { self.x = v; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 0 }; p.set(5); return 0; }"
        );
        assert!(ir.contains("define void @P.set(ptr "), "expected void+ptr for mut self: {ir}");
        // Body should store through the ptr (GEP then store).
        assert!(ir.contains("getelementptr %P"));
    }

    #[test]
    fn instance_call_passes_pointer_to_local() {
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; return p.get(); }"
        );
        // call should use ptr to the local's alloca.
        assert!(ir.contains("call i32 @P.get(ptr "));
    }

    #[test]
    fn methods_sample_compiles_to_ir() {
        let _ir = gen_src(include_str!("../../docs/examples/methods.cplus"));
    }

    // ---- Phase 2 slice 2D: fixed-size arrays ----

    #[test]
    fn array_type_lowers_to_llvm_array() {
        let ir = gen_src(
            "fn main() -> i32 { let _xs: [i32; 5] = [1, 2, 3, 4, 5]; return 0; }"
        );
        assert!(ir.contains("alloca [5 x i32]"), "expected alloca for array: {ir}");
        // Five stores (one per element).
        assert_eq!(ir.matches("store i32").count() >= 5, true, "expected ≥5 stores: {ir}");
    }

    #[test]
    fn array_index_emits_bounds_check() {
        let ir = gen_src(
            "fn main() -> i32 { let xs: [i32; 3] = [10, 20, 30]; return xs[0 as usize]; }"
        );
        // Bounds check pattern: icmp uge i64 ..., 3
        assert!(ir.contains("icmp uge i64"), "expected bounds-check icmp: {ir}");
        assert!(ir.contains("call void @llvm.trap()"), "expected trap branch: {ir}");
        // GEP into the array.
        assert!(ir.contains("getelementptr [3 x i32]"));
    }

    #[test]
    fn array_indexed_assign_uses_gep_store() {
        let ir = gen_src(
            "fn main() -> i32 { let mut xs: [i32; 3] = [0, 0, 0]; xs[1 as usize] = 7; return 0; }"
        );
        assert!(ir.contains("getelementptr [3 x i32]"));
        assert!(ir.contains("store i32 7, ptr"));
    }

    #[test]
    fn array_as_param_uses_llvm_array_type() {
        let ir = gen_src(
            "fn first(xs: [i32; 3]) -> i32 { return xs[0 as usize]; }\n\
             fn main() -> i32 { return first([1, 2, 3]); }"
        );
        assert!(ir.contains("define i32 @first([3 x i32]"));
    }

    #[test]
    fn array_samples_compile_to_ir() {
        for name in ["array_sum.cplus", "array_struct.cplus"] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    #[test]
    fn function_body_terminates() {
        let ir = gen_src("fn f() { }\nfn main() -> i32 { return 0; }");
        assert!(ir.contains("ret void"));
        assert!(ir.contains("ret i32 0"));
    }

    #[test]
    fn wrapping_ops_use_plain_arithmetic_in_debug() {
        // Even in Debug mode, `+%`/`-%`/`*%` must NOT emit overflow-check
        // intrinsics — that's the whole point of the wrapping operators.
        let ir = gen_src_with(
            "fn main() -> i32 { return 1 +% 2 -% 3 *% 4; }",
            BuildMode::Debug,
        );
        assert!(ir.contains(" = add i32 "), "expected plain add, got: {ir}");
        assert!(ir.contains(" = sub i32 "));
        assert!(ir.contains(" = mul i32 "));
        // No checked-arithmetic call for the wrapping body. (The preamble
        // still declares the intrinsics for plain ops elsewhere, so we
        // can't just grep for "with.overflow" anywhere in the IR — instead
        // check that the body of `main` doesn't *call* the intrinsic.)
        let main_body_start = ir.find("define i32 @main()").unwrap();
        let main_body = &ir[main_body_start..];
        assert!(
            !main_body.contains("call {i32, i1} @llvm.sadd.with.overflow"),
            "wrapping op leaked an overflow-check intrinsic into @main"
        );
    }

    #[test]
    fn wrapping_op_on_u32_uses_plain_add() {
        let ir = gen_src_with(
            "fn main() -> i32 { let x: u32 = 4000000000u32; let _y: u32 = x +% 1u32; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains(" = add i32 "), "expected plain add i32, got: {ir}");
    }

    // Regression: gen_expr used to return Ty::I32 for every integer literal
    // regardless of suffix, which produced invalid LLVM IR for typed
    // destinations (array literals of non-i32 element types; arithmetic on
    // suffixed non-i32 literals).

    #[test]
    fn u8_array_literal_lowers_with_u8_element_type() {
        let ir = gen_src(
            "fn main() -> i32 { let a: [u8; 4] = [10u8, 20u8, 30u8, 40u8]; return a[0 as usize] as i32; }",
        );
        // The array's alloca must use i8 element type, not i32.
        assert!(
            ir.contains("alloca [4 x i8]"),
            "expected `alloca [4 x i8]` for the array literal, got: {ir}"
        );
        // And the per-element store must store an i8 value, not i32.
        assert!(
            ir.contains("store i8 "),
            "expected `store i8 ...` for each element, got: {ir}"
        );
    }

    #[test]
    fn suffixed_u64_arithmetic_uses_i64() {
        let ir = gen_src(
            "fn main() -> i32 { let x: u64 = 1u64 +% 2u64; return x as i32; }",
        );
        // u64 wrapping add must emit `add i64`, never `add i32`.
        assert!(
            ir.contains(" = add i64 "),
            "expected `add i64` for u64 wrapping add, got: {ir}"
        );
        assert!(
            !ir.contains(" = add i32 "),
            "u64 add must not lower to i32, got: {ir}"
        );
    }
}
