//! Semantic analysis: name resolution + type checking, single pass.
//!
//! Phase 1 scope: only `i32` and `bool` types. Reports every Phase-1 rejection
//! case from `docs/design/phase1-grammar.md` §7.2.
//!
//! Error code allocation:
//! - E0300: undefined name
//! - E0301: duplicate function definition
//! - E0302: type mismatch
//! - E0303: unknown type name
//! - E0304: condition must be `bool`
//! - E0305: assignment to immutable binding
//! - E0306: block produces no value but one is required
//! - E0307: `return` without a value when function returns non-`Unit`
//! - E0308: wrong number of arguments
//! - E0309: `main` must have signature `fn main() -> i32`
//! - E0310: float literals not supported in Phase 1
//! - E0311: non-`i32` integer suffix not supported in Phase 1
//! - E0312: feature parsed but not yet supported in Phase 1
//! - E0313: assignment target is not a place expression
//! - E0334: parameter has both `mut` and `move` (mutually exclusive)
//! - E0335: use of moved value
//! - E0337: cannot move out of non-binding place (partial moves deferred)

use crate::ast::*;
use crate::diagnostics::{DiagCode, DiagSink, Diagnostic, LineMap, Severity};
use crate::lexer::{NumSuffix, Span as ByteSpan};
use std::collections::HashMap;
use std::path::PathBuf;

/// Stable identifier for a user-defined enum. Indices into `SemaCx::enums`,
/// assigned in declaration order. Codegen rebuilds the same numbering by
/// walking `program.items` in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

/// Stable identifier for a user-defined struct. Same indexing convention
/// as `EnumId`, but in a separate index space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    // Signed integers
    I8, I16, I32, I64,
    // Unsigned integers
    U8, U16, U32, U64,
    // Pointer-sized
    Isize, Usize,
    // Floats
    F32, F64,
    // Other
    Bool,
    Unit,
    Enum(EnumId),
    Struct(StructId),
    /// Fixed-size array: element type + length.
    Array(Box<Ty>, u32),
    Error,   // sentinel for recovery; matches anything
}

impl Ty {
    /// Human-readable type name. For enums and structs we render a generic
    /// kind name; SemaCx has the actual table if higher-fidelity names are
    /// needed in a diagnostic message.
    pub fn name(&self) -> &'static str {
        match self {
            Ty::I8 => "i8", Ty::I16 => "i16", Ty::I32 => "i32", Ty::I64 => "i64",
            Ty::U8 => "u8", Ty::U16 => "u16", Ty::U32 => "u32", Ty::U64 => "u64",
            Ty::Isize => "isize", Ty::Usize => "usize",
            Ty::F32 => "f32", Ty::F64 => "f64",
            Ty::Bool => "bool",
            Ty::Unit => "()",
            Ty::Enum(_) => "enum",
            Ty::Struct(_) => "struct",
            Ty::Array(_, _) => "array",
            Ty::Error => "<error>",
        }
    }

    pub fn is_signed_int(&self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize)
    }
    pub fn is_unsigned_int(&self) -> bool {
        matches!(self, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize)
    }
    pub fn is_int(&self) -> bool { self.is_signed_int() || self.is_unsigned_int() }
    pub fn is_float(&self) -> bool { matches!(self, Ty::F32 | Ty::F64) }
    pub fn is_numeric(&self) -> bool { self.is_int() || self.is_float() }
    pub fn is_enum(&self) -> bool { matches!(self, Ty::Enum(_)) }
    pub fn is_struct(&self) -> bool { matches!(self, Ty::Struct(_)) }
    pub fn is_array(&self) -> bool { matches!(self, Ty::Array(_, _)) }

    /// Phase 3 conservative `Copy` rule: primitives, `bool`, `()`, and plain
    /// Atomic `Copy` rule: types whose `Copy`-ness is fixed by the type itself,
    /// not by its components. Primitives, plain enums, `bool`, `()`, and the
    /// `Error` sentinel (treated as Copy to avoid cascading move diagnostics on
    /// already-broken code). For composite types (`Array`, `Struct`) call
    /// `SemaCx::is_copy(&ty)` instead — the answer depends on the struct table.
    pub fn is_atomic_copy(&self) -> bool {
        match self {
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            | Ty::Isize | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool | Ty::Unit
            | Ty::Enum(_)
            | Ty::Error => true,
            Ty::Struct(_) | Ty::Array(_, _) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    /// Field name → (declaration order index, field type). Order matters for
    /// codegen to compute correct GEP indices.
    pub fields: Vec<(String, Ty)>,
    /// Methods declared in any `impl` block for this struct.
    pub methods: HashMap<String, MethodSig>,
    /// Cached `Copy` flag — structural auto-derive: true iff every field type
    /// is `Copy`. Computed by `compute_struct_copy_flags` after field types
    /// are resolved. See `docs/design/phase3-copy-derivation.md`.
    pub is_copy: bool,
}

/// Type + ownership marker for a single parameter. The `move_` flag indicates
/// the parameter was declared `move x: T` and consumes its argument (when the
/// argument's type is non-Copy). The `mutable` flag (`mut x: T`) is recorded
/// for completeness but is body-internal — call sites don't care.
#[derive(Debug, Clone)]
pub struct ParamSig {
    pub ty: Ty,
    pub mutable: bool,
    pub move_: bool,
}

#[derive(Debug, Clone)]
pub struct MethodSig {
    pub receiver: Option<Receiver>,
    /// Parameter signatures *excluding* the receiver.
    pub params: Vec<ParamSig>,
    pub return_type: Ty,
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<(u32, Ty)> {
        self.fields.iter().enumerate().find_map(|(i, (n, t))| {
            (n == name).then(|| (i as u32, t.clone()))
        })
    }
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<ParamSig>,
    pub return_type: Ty,
}

#[derive(Debug, Clone)]
struct LocalInfo {
    ty: Ty,
    mutable: bool,
    /// True iff this binding has been consumed by a move. Reads of a moved
    /// binding produce E0335. Move tracking is linear within the body in
    /// Phase 3; flow-sensitive merging across branches is Phase 5 work.
    moved: bool,
}

/// Run sema on a parsed program. Returns all diagnostics produced;
/// the program is well-typed iff none have severity `Error`.
pub fn check(program: &Program, file: PathBuf, src: &str) -> Vec<Diagnostic> {
    let lm = LineMap::new(src);
    let mut sink = DiagSink::new();
    let mut cx = SemaCx {
        file,
        src,
        lm,
        sink: &mut sink,
        fns: HashMap::new(),
        enums: Vec::new(),
        enum_by_name: HashMap::new(),
        structs: Vec::new(),
        struct_by_name: HashMap::new(),
        scopes: Vec::new(),
        current_return: Ty::Error,
    };
    cx.register_builtins();
    // Type collection: names, struct fields, struct Copy flags, methods.
    // Copy flags must be computed after fields are resolved but before
    // methods, so method signatures can ask about the Copy-ness of any
    // struct they mention.
    cx.collect_type_names(program);
    cx.collect_struct_fields(program);
    cx.compute_struct_copy_flags();
    cx.collect_methods(program);
    cx.collect_functions(program);
    cx.check_main_signature(program);
    cx.check_functions(program);
    cx.check_methods(program);
    sink.into_vec()
}

struct SemaCx<'a> {
    file: PathBuf,
    src: &'a str,
    lm: LineMap,
    sink: &'a mut DiagSink,
    fns: HashMap<String, FnSig>,
    enums: Vec<EnumDef>,
    enum_by_name: HashMap<String, EnumId>,
    structs: Vec<StructDef>,
    struct_by_name: HashMap<String, StructId>,
    scopes: Vec<HashMap<String, LocalInfo>>,
    current_return: Ty,
}

impl SemaCx<'_> {
    // ---- diagnostic helpers ----

    fn err(&mut self, code: &'static str, msg: String, span: ByteSpan) {
        let primary = self.lm.span(&self.file, span, self.src);
        self.sink.emit(Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message: msg,
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    // ---- setup ----

    fn register_builtins(&mut self) {
        // `println(n: i32)` — emitted by codegen as a call to `printf("%d\n", n)`.
        self.fns.insert(
            "println".to_string(),
            FnSig {
                params: vec![ParamSig { ty: Ty::I32, mutable: false, move_: false }],
                return_type: Ty::Unit,
            },
        );
    }

    /// First pass: register every enum and struct *name* (and enum variants),
    /// without resolving struct field types yet. This lets struct fields
    /// reference any user-defined type regardless of declaration order.
    fn collect_type_names(&mut self, p: &Program) {
        for item in &p.items {
            match &item.kind {
                ItemKind::Enum(e) => {
                    let mut seen: HashMap<String, ()> = HashMap::new();
                    let mut variants = Vec::new();
                    for v in &e.variants {
                        if seen.contains_key(&v.name) {
                            self.err(
                                "E0318",
                                format!("duplicate variant `{}` in enum `{}`", v.name, e.name.name),
                                v.span,
                            );
                            continue;
                        }
                        seen.insert(v.name.clone(), ());
                        variants.push(v.name.clone());
                    }
                    if self.type_name_taken(&e.name.name) {
                        self.err(
                            "E0301",
                            format!("duplicate type definition `{}`", e.name.name),
                            e.name.span,
                        );
                        continue;
                    }
                    let id = EnumId(self.enums.len() as u32);
                    self.enums.push(EnumDef { name: e.name.name.clone(), variants });
                    self.enum_by_name.insert(e.name.name.clone(), id);
                }
                ItemKind::Struct(s) => {
                    if self.type_name_taken(&s.name.name) {
                        self.err(
                            "E0301",
                            format!("duplicate type definition `{}`", s.name.name),
                            s.name.span,
                        );
                        continue;
                    }
                    let id = StructId(self.structs.len() as u32);
                    self.structs.push(StructDef {
                        name: s.name.name.clone(),
                        fields: Vec::new(),
                        methods: HashMap::new(),
                        is_copy: false,
                    });
                    self.struct_by_name.insert(s.name.name.clone(), id);
                }
                ItemKind::Function(_) | ItemKind::Impl(_) => {}
            }
        }
    }

    fn type_name_taken(&self, name: &str) -> bool {
        self.enum_by_name.contains_key(name) || self.struct_by_name.contains_key(name)
    }

    /// Second pass: resolve struct field types and populate `StructDef.fields`.
    /// Detects duplicate field names (E0319).
    fn collect_struct_fields(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Struct(s) = &item.kind else { continue; };
            let Some(&id) = self.struct_by_name.get(&s.name.name) else { continue; };
            let mut seen: HashMap<String, ()> = HashMap::new();
            let mut fields: Vec<(String, Ty)> = Vec::new();
            for f in &s.fields {
                if seen.contains_key(&f.name.name) {
                    self.err(
                        "E0319",
                        format!("duplicate field `{}` in struct `{}`", f.name.name, s.name.name),
                        f.name.span,
                    );
                    continue;
                }
                seen.insert(f.name.name.clone(), ());
                let ty = self.resolve_type(&f.ty);
                fields.push((f.name.name.clone(), ty));
            }
            self.structs[id.0 as usize].fields = fields;
        }
    }

    /// Compute `is_copy` for every user-defined struct: a struct is `Copy`
    /// iff every field type is `Copy`. The check is iterated to a fixpoint
    /// because struct A's `is_copy` may depend on struct B's, and the
    /// declaration order in source doesn't guarantee a useful evaluation
    /// order. Convergence: at most `N` iterations for `N` structs (each
    /// iteration either flips at least one struct's flag from false to true,
    /// or we stop). Once flipped to true, a flag never flips back — the rule
    /// is monotone.
    ///
    /// See `docs/design/phase3-copy-derivation.md`.
    fn compute_struct_copy_flags(&mut self) {
        loop {
            let mut changed = false;
            for i in 0..self.structs.len() {
                if self.structs[i].is_copy {
                    continue;
                }
                let all_fields_copy = self.structs[i]
                    .fields
                    .iter()
                    .all(|(_, ty)| self.is_copy(ty));
                if all_fields_copy {
                    self.structs[i].is_copy = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Decide whether a type is `Copy`. Structural auto-derive: every
    /// component must be `Copy`. For structs the answer is precomputed in
    /// `compute_struct_copy_flags`; arrays recurse on the element type.
    pub fn is_copy(&self, ty: &Ty) -> bool {
        if ty.is_atomic_copy() {
            return true;
        }
        match ty {
            Ty::Array(elem, _) => self.is_copy(elem),
            Ty::Struct(id) => self.structs[id.0 as usize].is_copy,
            _ => unreachable!("is_atomic_copy already handled non-composite cases"),
        }
    }

    /// Third pass: collect methods from `impl` blocks. Runs after structs
    /// are fully typed so methods can reference any type by name. Reports
    /// E0325 (unknown / non-struct impl target) and E0326 (duplicate method).
    fn collect_methods(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else { continue; };
            let Some(&id) = self.struct_by_name.get(&b.target.name) else {
                if self.enum_by_name.contains_key(&b.target.name) {
                    self.err(
                        "E0325",
                        format!("`impl` on enum type `{}` is not yet supported (Phase 2 supports inherent methods on structs only)", b.target.name),
                        b.target.span,
                    );
                } else {
                    self.err(
                        "E0325",
                        format!("`impl` target `{}` is not a known type", b.target.name),
                        b.target.span,
                    );
                }
                continue;
            };
            for m in &b.methods {
                let params: Vec<ParamSig> = m.params.iter().map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                }).collect();
                let return_type = match &m.return_type {
                    Some(t) => self.resolve_type(t),
                    None => Ty::Unit,
                };
                if self.structs[id.0 as usize].methods.contains_key(&m.name.name) {
                    self.err(
                        "E0326",
                        format!("duplicate method `{}` in impl `{}`", m.name.name, b.target.name),
                        m.name.span,
                    );
                    continue;
                }
                self.structs[id.0 as usize].methods.insert(
                    m.name.name.clone(),
                    MethodSig { receiver: m.receiver, params, return_type },
                );
            }
        }
    }

    /// Type-check every method body. Runs after function bodies.
    fn check_methods(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else { continue; };
            let Some(&id) = self.struct_by_name.get(&b.target.name) else { continue; };
            for m in &b.methods {
                self.check_method(id, m);
            }
        }
    }

    fn check_method(&mut self, struct_id: StructId, m: &Method) {
        let Some(sig) = self.structs[struct_id.0 as usize].methods.get(&m.name.name).cloned() else {
            return;
        };
        self.current_return = sig.return_type.clone();
        self.scopes.push(HashMap::new());

        // Register `self` if there's a receiver. `mut self` makes self
        // a mutable binding (enables `self.x = ...`); other forms don't.
        // `move self` is read-only inside the body — consumption happens at
        // the call site, not from within.
        if let Some(rcv) = sig.receiver {
            let mutable = matches!(rcv, Receiver::Mut);
            self.scopes.last_mut().unwrap().insert(
                "self".to_string(),
                LocalInfo { ty: Ty::Struct(struct_id), mutable, moved: false },
            );
        }
        // Register non-receiver params.
        for (param, psig) in m.params.iter().zip(sig.params.iter()) {
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo { ty: psig.ty.clone(), mutable: param.mutable, moved: false },
            );
        }
        self.check_function_body(&m.body, sig.return_type, m.body.span);
        self.scopes.pop();
    }

    fn collect_functions(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Function(f) = &item.kind else { continue; };
            let params: Vec<ParamSig> = f.params.iter().map(|p| ParamSig {
                ty: self.resolve_type(&p.ty),
                mutable: p.mutable,
                move_: p.move_,
            }).collect();
            let ret = match &f.return_type {
                Some(t) => self.resolve_type(t),
                None => Ty::Unit,
            };
            if self.fns.contains_key(&f.name.name) {
                self.err(
                    "E0301",
                    format!("duplicate function definition `{}`", f.name.name),
                    f.name.span,
                );
                continue;
            }
            self.fns.insert(f.name.name.clone(), FnSig { params, return_type: ret });
        }
    }

    fn check_main_signature(&mut self, p: &Program) {
        let Some(sig) = self.fns.get("main").cloned() else { return; };
        let Some((no_params, span)) = p.items.iter().find_map(|it| {
            let ItemKind::Function(f) = &it.kind else { return None; };
            (f.name.name == "main").then(|| (f.params.is_empty(), f.name.span))
        }) else { return; };
        // If we already errored resolving the return type, don't pile on.
        if sig.return_type == Ty::Error { return; }
        if !no_params || sig.return_type != Ty::I32 {
            self.err(
                "E0309",
                "`main` must have signature `fn main() -> i32` in Phase 1".to_string(),
                span,
            );
        }
    }

    fn check_functions(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Function(f) = &item.kind else { continue; };
            self.check_function(f);
        }
    }

    fn check_function(&mut self, f: &Function) {
        let sig = self.fns.get(&f.name.name).cloned();
        let Some(sig) = sig else { return; }; // duplicate def already errored
        self.current_return = sig.return_type.clone();
        self.scopes.push(HashMap::new());
        for (param, psig) in f.params.iter().zip(sig.params.iter()) {
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo { ty: psig.ty.clone(), mutable: param.mutable, moved: false },
            );
        }
        self.check_function_body(&f.body, sig.return_type, f.body.span);
        self.scopes.pop();
    }

    /// Function body: must produce a value matching the return type, OR end
    /// with an explicit `return`. Phase-1 heuristic; full divergence analysis
    /// is Phase 3 work.
    fn check_function_body(&mut self, body: &Block, expected: Ty, body_span: ByteSpan) {
        // Push the body scope.
        self.scopes.push(HashMap::new());
        for s in &body.stmts {
            self.check_stmt(s);
        }
        // C+ style: function bodies use explicit `return`, never an implicit
        // tail expression. Block expressions remain valid in let initializers,
        // assignments, and return expressions — just not at function-body level.
        if let Some(tail) = &body.tail {
            self.err(
                "E0333",
                "function body cannot end with an implicit tail expression; use `return ...;` instead".to_string(),
                tail.span,
            );
            // Still type-check the tail for cascading diagnostics.
            let _ = self.check_expr(tail, Some(expected.clone()));
        } else if expected != Ty::Unit && expected != Ty::Error && !body_ends_with_return(body) {
            self.err(
                "E0306",
                format!(
                    "function body must end with `return ...;` for type `{}`",
                    expected.name()
                ),
                body_span,
            );
        }
        self.scopes.pop();
    }

    // ---- statements ----

    fn check_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { mutable, name, ty, init } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let inferred = self.check_expr(init, declared.clone());
                let final_ty = declared.unwrap_or(inferred);
                self.scopes.last_mut().unwrap().insert(
                    name.name.clone(),
                    LocalInfo { ty: final_ty, mutable: *mutable, moved: false },
                );
            }
            StmtKind::Return(value) => {
                let ret = self.current_return.clone();
                match (value, &ret) {
                    (Some(e), _) => {
                        self.check_expr(e, Some(ret));
                    }
                    (None, &Ty::Unit) | (None, &Ty::Error) => {}
                    (None, _) => {
                        self.err(
                            "E0307",
                            format!("`return` without a value, but function returns `{}`", ret.name()),
                            s.span,
                        );
                    }
                }
            }
            StmtKind::While { cond, body } => {
                let _ = self.check_cond(cond);
                self.scopes.push(HashMap::new());
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
            StmtKind::For(fl) => self.check_for(fl),
            StmtKind::Expr(e) => {
                let _ = self.check_expr(e, None);
            }
        }
    }

    fn check_for(&mut self, fl: &ForLoop) {
        match fl {
            ForLoop::Range { var, iter, body } => {
                let (start, end) = match &iter.kind {
                    ExprKind::Range { start: Some(s), end: Some(e), .. } => (s.as_ref(), e.as_ref()),
                    _ => {
                        self.err(
                            "E0312",
                            "Phase 1 `for ... in` requires a closed range like `0..n` or `0..=n`".to_string(),
                            iter.span,
                        );
                        return;
                    }
                };
                self.check_expr(start, Some(Ty::I32));
                self.check_expr(end, Some(Ty::I32));
                self.scopes.push(HashMap::new());
                self.scopes.last_mut().unwrap().insert(
                    var.name.clone(),
                    LocalInfo { ty: Ty::I32, mutable: false, moved: false },
                );
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
            ForLoop::CStyle { init, cond, update, body } => {
                self.scopes.push(HashMap::new());
                if let Some(init) = init { self.check_stmt(init); }
                if let Some(cond) = cond { let _ = self.check_cond(cond); }
                for u in update { let _ = self.check_expr(u, None); }
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
        }
    }

    /// Type-check a block used in statement position (its value is discarded).
    fn check_block_as_stmt(&mut self, b: &Block) {
        self.scopes.push(HashMap::new());
        for s in &b.stmts { self.check_stmt(s); }
        if let Some(tail) = &b.tail {
            let _ = self.check_expr(tail, None);
        }
        self.scopes.pop();
    }

    /// Condition expressions must be `bool`.
    fn check_cond(&mut self, e: &Expr) -> Ty {
        let t = self.check_expr(e, None);
        if t != Ty::Bool && t != Ty::Error {
            self.err(
                "E0304",
                format!("condition must be `bool`, found `{}`", t.name()),
                e.span,
            );
        }
        Ty::Bool
    }

    // ---- expressions ----

    fn check_expr(&mut self, e: &Expr, expected: Option<Ty>) -> Ty {
        let actual = self.check_expr_kind(e, expected.clone());
        if let Some(exp) = expected {
            if exp != Ty::Error && actual != Ty::Error && exp != actual {
                self.err(
                    "E0302",
                    format!("type mismatch: expected `{}`, found `{}`", exp.name(), actual.name()),
                    e.span,
                );
            }
        }
        actual
    }

    fn check_expr_kind(&mut self, e: &Expr, expected: Option<Ty>) -> Ty {
        match &e.kind {
            ExprKind::IntLit(_, suf) => self.check_int_lit(*suf, expected),
            ExprKind::FloatLit(_, suf) => self.check_float_lit(*suf, expected),
            ExprKind::BoolLit(_) => Ty::Bool,
            ExprKind::Ident(name) => self.resolve_value_ident(name, e.span),
            ExprKind::Block(b) => self.check_block_as_expr(b),
            ExprKind::If { cond, then, else_branch } => {
                self.check_if(cond, then, else_branch.as_deref())
            }
            ExprKind::Call { callee, args } => self.check_call(callee, args, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, e.span),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, e.span),
            ExprKind::Assign { op, target, value } => self.check_assign(*op, target, value, e.span),
            ExprKind::Range { .. } => {
                self.err(
                    "E0312",
                    "range expressions are only supported as the iterator in `for ... in`".to_string(),
                    e.span,
                );
                Ty::Error
            }
            ExprKind::Cast { expr, ty } => self.check_cast(expr, ty, e.span),
            ExprKind::Path { segments } => self.check_path(segments, e.span),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, e.span),
            ExprKind::Field { receiver, name } => self.check_field(receiver, name),
            ExprKind::ArrayLit { elements } => self.check_array_lit(elements, expected, e.span),
            ExprKind::Index { receiver, index } => self.check_index(receiver, index, e.span),
        }
    }

    fn check_array_lit(&mut self, elements: &[Expr], expected: Option<Ty>, span: ByteSpan) -> Ty {
        if elements.is_empty() {
            self.err(
                "E0332",
                "empty array literals not supported in Phase 2; provide at least one element".to_string(),
                span,
            );
            return Ty::Error;
        }
        // Use the declared element type if we have an expected array; otherwise infer from first element.
        let expected_elem: Option<Ty> = match &expected {
            Some(Ty::Array(elem, _)) => Some((**elem).clone()),
            _ => None,
        };
        let first_ty = self.check_expr(&elements[0], expected_elem.clone());
        for e in &elements[1..] {
            let got = self.check_expr(e, Some(first_ty.clone()));
            if got != first_ty && got != Ty::Error && first_ty != Ty::Error {
                self.err(
                    "E0329",
                    format!("mixed element types in array literal: expected `{}`, found `{}`", first_ty.name(), got.name()),
                    e.span,
                );
            }
        }
        let len = elements.len() as u32;
        // If we had a declared length expectation, check it matches.
        if let Some(Ty::Array(_, declared_len)) = &expected {
            if *declared_len != len {
                self.err(
                    "E0330",
                    format!("array literal has {} element(s); expected {}", len, declared_len),
                    span,
                );
                return Ty::Error;
            }
        }
        Ty::Array(Box::new(first_ty), len)
    }

    fn check_index(&mut self, receiver: &Expr, index: &Expr, span: ByteSpan) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        // Index must be `usize`. Numeric literals will coerce via expected-type rule.
        let _ = self.check_expr(index, Some(Ty::Usize));
        match recv_ty {
            Ty::Array(elem, _) => (*elem).clone(),
            Ty::Error => Ty::Error,
            other => {
                self.err(
                    "E0331",
                    format!("cannot index non-array type `{}`", other.name()),
                    span,
                );
                Ty::Error
            }
        }
    }

    fn check_struct_lit(&mut self, name: &Ident, fields: &[StructLitField], span: ByteSpan) -> Ty {
        let Some(&id) = self.struct_by_name.get(&name.name) else {
            self.err("E0303", format!("unknown type `{}`", name.name), name.span);
            // Still walk the field exprs so we surface their errors.
            for f in fields { let _ = self.check_expr(&f.value, None); }
            return Ty::Error;
        };
        // Snapshot the declared fields so we can borrow self mutably below.
        let declared: Vec<(String, Ty)> = self.structs[id.0 as usize].fields.clone();
        let struct_name = self.structs[id.0 as usize].name.clone();

        // Detect duplicate-in-literal and unknown-field; type-check each provided value.
        let mut provided: HashMap<String, ()> = HashMap::new();
        for lit_field in fields {
            if provided.contains_key(&lit_field.name.name) {
                self.err(
                    "E0319",
                    format!("duplicate field `{}` in literal of struct `{}`",
                            lit_field.name.name, struct_name),
                    lit_field.name.span,
                );
                let _ = self.check_expr(&lit_field.value, None);
                continue;
            }
            provided.insert(lit_field.name.name.clone(), ());
            let expected_ty = declared
                .iter()
                .find(|(n, _)| n == &lit_field.name.name)
                .map(|(_, t)| t.clone());
            match expected_ty {
                Some(t) => { let _ = self.check_expr(&lit_field.value, Some(t)); }
                None => {
                    self.err(
                        "E0322",
                        format!("struct `{struct_name}` has no field `{}`", lit_field.name.name),
                        lit_field.name.span,
                    );
                    let _ = self.check_expr(&lit_field.value, None);
                }
            }
        }
        // Detect missing fields.
        for (declared_name, _) in &declared {
            if !provided.contains_key(declared_name) {
                self.err(
                    "E0321",
                    format!("missing field `{declared_name}` in literal of struct `{struct_name}`"),
                    span,
                );
            }
        }
        Ty::Struct(id)
    }

    fn check_field(&mut self, receiver: &Expr, name: &Ident) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        let Ty::Struct(id) = recv_ty else {
            if recv_ty != Ty::Error {
                self.err(
                    "E0323",
                    format!("field access on non-struct type `{}`", recv_ty.name()),
                    name.span,
                );
            }
            return Ty::Error;
        };
        let def = &self.structs[id.0 as usize];
        match def.field(&name.name) {
            Some((_, ty)) => ty,
            None => {
                self.err(
                    "E0320",
                    format!("struct `{}` has no field `{}`", def.name, name.name),
                    name.span,
                );
                Ty::Error
            }
        }
    }

    fn check_int_lit(&mut self, suffix: NumSuffix, expected: Option<Ty>) -> Ty {
        match suffix {
            NumSuffix::None => match expected {
                Some(t) if t.is_int() => t,
                _ => Ty::I32, // default
            },
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
            // Float suffix on integer literal shouldn't happen — the lexer
            // routes those to FloatLit. Treat defensively.
            NumSuffix::F32 | NumSuffix::F64 => unreachable!("float suffix on int literal"),
        }
    }

    fn check_float_lit(&mut self, suffix: NumSuffix, expected: Option<Ty>) -> Ty {
        match suffix {
            NumSuffix::F32 => Ty::F32,
            NumSuffix::F64 => Ty::F64,
            NumSuffix::None => match expected {
                Some(Ty::F32) => Ty::F32,
                _ => Ty::F64, // default
            },
            _ => unreachable!("integer suffix on float literal"),
        }
    }

    fn check_cast(&mut self, expr: &Expr, target: &Type, span: ByteSpan) -> Ty {
        let from = self.check_expr(expr, None);
        let to = self.resolve_type(target);
        if from == Ty::Error || to == Ty::Error {
            return to;
        }
        if !cast_allowed(&from, &to) {
            self.err(
                "E0315",
                format!("invalid cast: `{}` cannot be cast to `{}`", from.name(), to.name()),
                span,
            );
            return Ty::Error;
        }
        to
    }

    fn check_block_as_expr(&mut self, b: &Block) -> Ty {
        // Block-as-expression: if no tail, value is Unit. The surrounding
        // expected-type check will catch genuine mismatches with E0302.
        // E0306 fires only at the function-body level, where "value required"
        // is unambiguous.
        self.scopes.push(HashMap::new());
        for s in &b.stmts { self.check_stmt(s); }
        let ty = match &b.tail {
            Some(t) => self.check_expr(t, None),
            None => Ty::Unit,
        };
        self.scopes.pop();
        ty
    }

    fn check_if(&mut self, cond: &Expr, then: &Block, else_branch: Option<&Expr>) -> Ty {
        let _ = self.check_cond(cond);
        let then_ty = self.check_block_as_expr(then);
        let else_ty = match else_branch {
            Some(e) => match &e.kind {
                ExprKind::Block(b) => self.check_block_as_expr(b),
                ExprKind::If { .. } => self.check_expr(e, None),
                _ => Ty::Error,
            },
            None => Ty::Unit,
        };
        if then_ty == Ty::Error || else_ty == Ty::Error {
            return Ty::Error;
        }
        if then_ty != else_ty {
            self.err(
                "E0302",
                format!(
                    "`if` and `else` branches have incompatible types: `{}` vs `{}`",
                    then_ty.name(), else_ty.name()
                ),
                then.span,
            );
            return Ty::Error;
        }
        then_ty
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], call_span: ByteSpan) -> Ty {
        match &callee.kind {
            ExprKind::Ident(_) => self.check_named_call(callee, args, call_span),
            ExprKind::Field { receiver, name } => self.check_method_call(receiver, name, args, call_span),
            ExprKind::Path { segments } => self.check_assoc_call(segments, args, callee.span, call_span),
            _ => {
                self.err(
                    "E0312",
                    "callee must be a function name, a method, or a `Type::function` path".to_string(),
                    callee.span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                Ty::Error
            }
        }
    }

    fn check_named_call(&mut self, callee: &Expr, args: &[Expr], call_span: ByteSpan) -> Ty {
        let ExprKind::Ident(name) = &callee.kind else { unreachable!(); };
        let Some(sig) = self.fns.get(name).cloned() else {
            self.err("E0300", format!("undefined function `{name}`"), callee.span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("function `{}` takes {} argument(s), got {}", name, sig.params.len(), args.len()),
                call_span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    fn check_method_call(&mut self, receiver: &Expr, name: &Ident, args: &[Expr], call_span: ByteSpan) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        if recv_ty == Ty::Error {
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let Ty::Struct(id) = recv_ty else {
            self.err(
                "E0324",
                format!("no method `{}` on type `{}`", name.name, recv_ty.name()),
                name.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize].methods.get(&name.name).cloned() else {
            self.err(
                "E0324",
                format!("no method `{}` on struct `{}`", name.name, struct_name),
                name.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let Some(rcv) = sig.receiver else {
            self.err(
                "E0327",
                format!("`{}::{}` is an associated function; call it as `{}::{}(...)`", struct_name, name.name, struct_name, name.name),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        if matches!(rcv, Receiver::Mut) && !self.is_writable_place_quiet(receiver) {
            self.err(
                "E0328",
                format!("method `{}::{}` requires a mutable receiver", struct_name, name.name),
                receiver.span,
            );
        }
        // `move self` consumes the receiver place — but only if the struct
        // is non-`Copy`. For a `Copy` struct, `move self` is a redundant
        // marker (the receiver is bitwise-copied); leave the binding usable.
        // Same rule as for `move`-marked parameters.
        if matches!(rcv, Receiver::Move) && !self.structs[id.0 as usize].is_copy {
            self.consume_place(receiver, &struct_name, &name.name);
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("method `{}::{}` takes {} argument(s), got {}", struct_name, name.name, sig.params.len(), args.len()),
                call_span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    fn check_assoc_call(&mut self, segments: &[Ident], args: &[Expr], path_span: ByteSpan, call_span: ByteSpan) -> Ty {
        if segments.len() != 2 {
            self.err("E0312", "Phase 2 paths have exactly two segments".to_string(), path_span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let type_seg = &segments[0];
        let method_seg = &segments[1];
        // Enums: variants are values, not callable.
        if self.enum_by_name.contains_key(&type_seg.name) {
            self.err(
                "E0327",
                format!("enum variant `{}::{}` is a value, not a function", type_seg.name, method_seg.name),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let Some(&id) = self.struct_by_name.get(&type_seg.name) else {
            self.err("E0303", format!("unknown type `{}`", type_seg.name), type_seg.span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize].methods.get(&method_seg.name).cloned() else {
            self.err(
                "E0324",
                format!("struct `{}` has no method `{}`", struct_name, method_seg.name),
                method_seg.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        if sig.receiver.is_some() {
            self.err(
                "E0327",
                format!("`{}::{}` is an instance method; call it as `value.{}(...)`", struct_name, method_seg.name, method_seg.name),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("function `{}::{}` takes {} argument(s), got {}", struct_name, method_seg.name, sig.params.len(), args.len()),
                call_span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    /// Type-check a single call argument and apply move tracking. If the
    /// parameter is `move` and the argument's type is non-Copy, the source
    /// place is consumed:
    ///   - Plain Ident referencing a local: mark the binding as moved.
    ///   - Anything else (Field/Index/temp): reject as E0337 — partial moves
    ///     out of struct fields or array slots are deferred to Phase 5/6.
    /// `Copy`-typed arguments are unaffected — the `move` marker on a Copy
    /// parameter is redundant (a future E0336 lint will suggest removing it).
    fn check_arg_with_move(&mut self, arg: &Expr, expected: &ParamSig) {
        let _ = self.check_expr(arg, Some(expected.ty.clone()));
        if expected.move_ && !self.is_copy(&expected.ty) {
            self.consume_arg_place(arg);
        }
    }

    /// Mark the source binding of an argument as moved. Used by both
    /// `move`-param calls and `move self` receivers. Only plain Ident
    /// references to a local binding are accepted; anything else triggers
    /// E0337 (partial moves deferred).
    fn consume_arg_place(&mut self, arg: &Expr) {
        match &arg.kind {
            ExprKind::Ident(name) => {
                // Find the binding's scope and mark moved. `resolve_value_ident`
                // already ran via `check_expr` and would have produced E0335
                // if the binding was *already* moved; here we just record the
                // new move state.
                for scope in self.scopes.iter_mut().rev() {
                    if let Some(info) = scope.get_mut(name) {
                        info.moved = true;
                        return;
                    }
                }
                // Unknown name — error was already produced by check_expr.
            }
            _ => {
                self.err(
                    "E0337",
                    "cannot move out of this expression; only whole-binding moves are supported in Phase 3 (partial moves of fields or array slots are deferred)".to_string(),
                    arg.span,
                );
            }
        }
    }

    /// Same as `consume_arg_place` but for the receiver in a `move self`
    /// method call. Diagnostic phrasing names the method for clarity.
    fn consume_place(&mut self, receiver: &Expr, type_name: &str, method_name: &str) {
        match &receiver.kind {
            ExprKind::Ident(name) => {
                for scope in self.scopes.iter_mut().rev() {
                    if let Some(info) = scope.get_mut(name) {
                        info.moved = true;
                        return;
                    }
                }
            }
            _ => {
                self.err(
                    "E0337",
                    format!("method `{}::{}` consumes `self`; the receiver must be a whole binding (partial moves are deferred to a later phase)", type_name, method_name),
                    receiver.span,
                );
            }
        }
    }

    fn is_writable_place_quiet(&self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Ident(name) => {
                matches!(self.lookup_local(name), Some(info) if info.mutable)
            }
            ExprKind::Field { receiver, .. } => self.is_writable_place_quiet(receiver),
            ExprKind::Index { receiver, .. } => self.is_writable_place_quiet(receiver),
            _ => false,
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: ByteSpan) -> Ty {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let lhs_ty = self.check_expr(lhs, None);
                if !lhs_ty.is_numeric() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`{}` requires numeric operands, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::Mod => {
                let lhs_ty = self.check_expr(lhs, None);
                if lhs_ty.is_float() {
                    self.err(
                        "E0316",
                        "modulo (`%`) on float types is not supported".to_string(),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                if !lhs_ty.is_int() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`%` requires integer operands, found `{}`", lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::Eq | BinOp::Ne => {
                let lt = self.check_expr(lhs, None);
                if lt.is_struct() {
                    self.err(
                        "E0302",
                        format!("`==` / `!=` are not implemented for struct types in Phase 2; write your own equality function"),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Bool;
                }
                let _ = self.check_expr(rhs, Some(lt));
                Ty::Bool
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let lhs_ty = self.check_expr(lhs, None);
                if !lhs_ty.is_numeric() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("ordered comparison requires numeric operands, found `{}`", lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Bool;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty));
                Ty::Bool
            }
            BinOp::And | BinOp::Or => {
                self.check_expr(lhs, Some(Ty::Bool));
                self.check_expr(rhs, Some(Ty::Bool));
                Ty::Bool
            }
            BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => {
                let lhs_ty = self.check_expr(lhs, None);
                if !lhs_ty.is_int() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`{}` requires integer operands, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                self.err(
                    "E0312",
                    "bitwise and shift operators are not yet supported".to_string(),
                    span,
                );
                let _ = self.check_expr(lhs, None);
                let _ = self.check_expr(rhs, None);
                Ty::Error
            }
        }
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: ByteSpan) -> Ty {
        match op {
            UnaryOp::Neg => {
                let t = self.check_expr(operand, None);
                if t == Ty::Error { return Ty::Error; }
                if t.is_unsigned_int() {
                    self.err(
                        "E0302",
                        format!("cannot negate unsigned type `{}`; use a signed type instead", t.name()),
                        span,
                    );
                    return Ty::Error;
                }
                if !t.is_signed_int() && !t.is_float() {
                    self.err(
                        "E0302",
                        format!("unary `-` requires a numeric operand, found `{}`", t.name()),
                        span,
                    );
                    return Ty::Error;
                }
                t
            }
            UnaryOp::Not => { self.check_expr(operand, Some(Ty::Bool)); Ty::Bool }
            UnaryOp::BitNot => {
                self.err("E0312", "bitwise not (`~`) is not yet supported".to_string(), span);
                let _ = self.check_expr(operand, None);
                Ty::Error
            }
            UnaryOp::Ref { .. } => {
                self.err("E0312", "references are not yet supported (Phase 5/6)".to_string(), span);
                let _ = self.check_expr(operand, None);
                Ty::Error
            }
            UnaryOp::Deref => {
                self.err("E0312", "dereference (`*`) is not yet supported (Phase 2)".to_string(), span);
                let _ = self.check_expr(operand, None);
                Ty::Error
            }
        }
    }

    fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, span: ByteSpan) -> Ty {
        if !matches!(op, AssignOp::Assign) {
            self.err(
                "E0312",
                "compound assignment operators are not yet supported in Phase 1".to_string(),
                span,
            );
            let _ = self.check_expr(target, None);
            let _ = self.check_expr(value, None);
            return Ty::Error;
        }
        // Validate target is a place rooted at a mutable local. This walks
        // through Field accesses to find the root Ident.
        if !self.target_is_writable_place(target) {
            // err already emitted by the recursive walk
            let _ = self.check_expr(value, None);
            return Ty::Error;
        }
        // Get the leaf type of the place chain to type-check the rhs.
        let target_ty = self.check_expr(target, None);
        if target_ty != Ty::Error {
            self.check_expr(value, Some(target_ty));
        } else {
            let _ = self.check_expr(value, None);
        }
        Ty::Unit
    }

    /// A place is an Ident referring to a mutable local, or a Field chain
    /// rooted at one. Anything else errors with E0313 / E0305 / E0300.
    fn target_is_writable_place(&mut self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Ident(name) => {
                let local = self.lookup_local(name).cloned();
                let Some(info) = local else {
                    self.err("E0300", format!("undefined name `{name}`"), target.span);
                    return false;
                };
                if !info.mutable {
                    self.err(
                        "E0305",
                        format!("cannot assign to immutable binding `{name}`; declare it as `let mut`"),
                        target.span,
                    );
                    return false;
                }
                true
            }
            ExprKind::Field { receiver, .. } => self.target_is_writable_place(receiver),
            ExprKind::Index { receiver, .. } => self.target_is_writable_place(receiver),
            _ => {
                self.err(
                    "E0313",
                    "assignment target is not a place expression".to_string(),
                    target.span,
                );
                false
            }
        }
    }

    // ---- name + type resolution ----

    fn resolve_type(&mut self, t: &Type) -> Ty {
        let name = match &t.kind {
            TypeKind::Path(n) => n,
            TypeKind::Array { elem, len } => {
                let elem_ty = self.resolve_type(elem);
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
                if let Some(&id) = self.enum_by_name.get(name) {
                    return Ty::Enum(id);
                }
                if let Some(&id) = self.struct_by_name.get(name) {
                    return Ty::Struct(id);
                }
                self.err("E0303", format!("unknown type `{name}`"), t.span);
                Ty::Error
            }
        }
    }

    fn check_path(&mut self, segments: &[Ident], span: ByteSpan) -> Ty {
        // Phase 2A: paths are exactly two segments — `EnumName::Variant`.
        if segments.len() != 2 {
            self.err(
                "E0312",
                "Phase 2 paths must be `EnumName::Variant` (exactly two segments)".to_string(),
                span,
            );
            return Ty::Error;
        }
        let enum_seg = &segments[0];
        let variant_seg = &segments[1];
        let Some(&id) = self.enum_by_name.get(&enum_seg.name) else {
            self.err("E0303", format!("unknown type `{}`", enum_seg.name), enum_seg.span);
            return Ty::Error;
        };
        let def = &self.enums[id.0 as usize];
        if !def.variants.iter().any(|v| v == &variant_seg.name) {
            self.err(
                "E0317",
                format!("enum `{}` has no variant `{}`", def.name, variant_seg.name),
                variant_seg.span,
            );
            return Ty::Error;
        }
        Ty::Enum(id)
    }

    fn resolve_value_ident(&mut self, name: &str, span: ByteSpan) -> Ty {
        if let Some(info) = self.lookup_local(name) {
            let ty = info.ty.clone();
            let moved = info.moved;
            if moved {
                self.err(
                    "E0335",
                    format!("use of moved value `{name}`"),
                    span,
                );
            }
            return ty;
        }
        if self.fns.contains_key(name) {
            self.err(
                "E0312",
                format!("function `{name}` used as a value; first-class functions are not yet supported"),
                span,
            );
            return Ty::Error;
        }
        self.err("E0300", format!("undefined name `{name}`"), span);
        Ty::Error
    }

    fn lookup_local(&self, name: &str) -> Option<&LocalInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) { return Some(info); }
        }
        None
    }
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::AddWrap => "+%",
        BinOp::SubWrap => "-%",
        BinOp::MulWrap => "*%",
        _ => "?",
    }
}

fn cast_allowed(from: &Ty, to: &Ty) -> bool {
    if from == to { return true; }
    // numeric → numeric (any pair)
    if from.is_numeric() && to.is_numeric() { return true; }
    // bool → integer (zext to width)
    if *from == Ty::Bool && to.is_int() { return true; }
    // enum → integer (read the variant index)
    if from.is_enum() && to.is_int() { return true; }
    // Forbidden:
    //   - integer/float → bool (use `!= 0`)
    //   - bool → float
    //   - integer → enum (needs runtime range check)
    //   - any other combination
    false
}

fn body_ends_with_return(b: &Block) -> bool {
    b.stmts.last().is_some_and(|s| matches!(s.kind, StmtKind::Return(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use std::path::PathBuf;

    fn check_src(src: &str) -> Vec<Diagnostic> {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        check(&prog, PathBuf::from("test.cplus"), src)
    }

    fn errors(src: &str) -> Vec<&'static str> {
        check_src(src)
            .into_iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| {
                // We need a 'static str for assertions; leak the small string.
                Box::leak(d.code.0.to_string().into_boxed_str()) as &str
            })
            .collect()
    }

    fn assert_clean(src: &str) {
        let diags = check_src(src);
        assert!(
            diags.is_empty(),
            "expected clean type-check, got: {:#?}",
            diags
        );
    }

    fn assert_only_code(src: &str, code: &str) {
        let diags = check_src(src);
        assert_eq!(
            diags.len(),
            1,
            "expected exactly one diagnostic ({code}), got: {:#?}",
            diags
        );
        assert_eq!(diags[0].code.0, code);
    }

    // ---- happy paths: every Phase-1 sample type-checks ----

    #[test]
    fn factorial_clean() {
        assert_clean(include_str!("../../docs/examples/factorial.cplus"));
    }

    #[test]
    fn fibonacci_clean() {
        assert_clean(include_str!("../../docs/examples/fibonacci.cplus"));
    }

    #[test]
    fn sum_range_clean() {
        assert_clean(include_str!("../../docs/examples/sum_range.cplus"));
    }

    #[test]
    fn c_for_clean() {
        assert_clean(include_str!("../../docs/examples/c_for.cplus"));
    }

    #[test]
    fn return_with_value_clean() {
        assert_clean("fn main() -> i32 { return 42; }");
    }

    #[test]
    fn nested_if_expr_clean() {
        assert_clean("fn main() -> i32 { return if true { 1 } else if false { 2 } else { 3 }; }");
    }

    // ---- design-note §7.2 negative cases ----

    #[test]
    fn assign_to_immutable_e0305() {
        assert_only_code("fn main() -> i32 { let x = 1; x = 2; return 0; }", "E0305");
    }

    #[test]
    fn float_literal_in_i32_slot_is_type_mismatch() {
        // Phase 2: floats are supported, so `let x: i32 = 1.5` is a type
        // mismatch (f64 vs i32), not a "feature unsupported" error.
        assert_only_code("fn main() -> i32 { let x: i32 = 1.5; return 0; }", "E0302");
    }

    #[test]
    fn trailing_semi_discards_value_e0306() {
        assert_only_code("fn f() -> i32 { 1; }\nfn main() -> i32 { return f(); }", "E0306");
    }

    #[test]
    fn nonbool_condition_e0304() {
        assert_only_code("fn main() -> i32 { return if 1 { 1 } else { 2 }; }", "E0304");
    }

    #[test]
    fn u64_literal_now_supported() {
        // Phase 2: all integer suffixes supported.
        assert_clean("fn main() -> i32 { let x: u64 = 1u64; let y: u64 = x; let _z = y; return 0; }");
    }

    #[test]
    fn main_must_return_i32_e0309() {
        let codes = errors("fn main() { }");
        assert!(codes.contains(&"E0309"), "expected E0309 in {codes:?}");
    }

    #[test]
    fn return_without_value_e0307() {
        assert_only_code("fn f() -> i32 { return; }\nfn main() -> i32 { return f(); }", "E0307");
    }

    // ---- additional rules ----

    #[test]
    fn undefined_name_e0300() {
        assert_only_code("fn main() -> i32 { return x; }", "E0300");
    }

    #[test]
    fn undefined_function_e0300() {
        assert_only_code("fn main() -> i32 { return foo(1); }", "E0300");
    }

    #[test]
    fn duplicate_fn_e0301() {
        let src = "fn f() -> i32 { 0 }\nfn f() -> i32 { 1 }\nfn main() -> i32 { return f(); }";
        let codes = errors(src);
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn type_mismatch_e0302() {
        assert_only_code("fn main() -> i32 { let x: i32 = true; return 0; }", "E0302");
    }

    #[test]
    fn unknown_type_e0303() {
        assert_only_code("fn main() -> Foo { return 0; }", "E0303");
    }

    #[test]
    fn arg_count_mismatch_e0308() {
        // Wrap in a stmt + 0 tail so we don't also trigger E0302 from main's
        // i32 return type vs println's Unit return.
        assert_only_code("fn main() -> i32 { println(1, 2); return 0; }", "E0308");
    }

    #[test]
    fn arg_type_mismatch_e0302() {
        assert_only_code("fn main() -> i32 { println(true); return 0; }", "E0302");
    }

    #[test]
    fn float_literal_now_supported() {
        assert_clean("fn main() -> i32 { let x: f64 = 3.14; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn bitwise_not_supported_e0312() {
        assert_only_code("fn main() -> i32 { return 1 & 2; }", "E0312");
    }

    #[test]
    fn wrapping_ops_now_supported() {
        assert_clean("fn main() -> i32 { return (1 +% 2) -% 1 *% 1; }");
    }

    #[test]
    fn wrapping_op_on_float_e0302() {
        let codes = errors(
            "fn main() -> i32 { let x: f64 = 1.0; let y: f64 = x +% 2.0; return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn wrapping_op_on_bool_e0302() {
        let codes = errors(
            "fn main() -> i32 { let _b: bool = true +% false; return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn cast_now_supported() {
        assert_clean("fn main() -> i32 { return 1 as i32; }");
    }

    #[test]
    fn ref_not_supported_e0312() {
        assert_only_code("fn main() -> i32 { let x = 1; let y = &x; return 0; }", "E0312");
    }

    #[test]
    fn compound_assign_not_supported_e0312() {
        assert_only_code("fn main() -> i32 { let mut x = 1; x += 1; return x; }", "E0312");
    }

    #[test]
    fn assign_to_non_ident_e0313() {
        // Phase 1 has no field/index access yet, so we hit a parse error first
        // for most non-ident targets. Use a literal as a stand-in: parser
        // accepts `1 = 2` as Assign{IntLit, IntLit}.
        let codes = errors("fn main() -> i32 { 1 = 2; return 0; }");
        assert!(codes.contains(&"E0313"));
    }

    #[test]
    fn shadowing_in_inner_scope_clean() {
        assert_clean("fn main() -> i32 { let x = 1; { let x = 2; }; return x; }");
    }

    #[test]
    fn block_value_in_let_clean() {
        assert_clean("fn main() -> i32 { let x = { let y = 5; y + 1 }; return x; }");
    }

    #[test]
    fn while_loop_clean() {
        assert_clean("fn main() -> i32 { let mut i = 0; while i < 10 { i = i + 1; } return i; }");
    }

    #[test]
    fn comparison_returns_bool_clean() {
        assert_clean("fn main() -> i32 { let b: bool = 1 < 2; return if b { 1 } else { 0 }; }");
    }

    #[test]
    fn equality_on_bool_clean() {
        assert_clean("fn main() -> i32 { let b: bool = true == false; return if b { 1 } else { 0 }; }");
    }

    // ---- Phase 2 slice 1: full primitive types + casts ----

    #[test]
    fn all_integer_types_resolve() {
        for t in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "isize", "usize"] {
            let src = format!("fn main() -> i32 {{ let x: {t} = 0; let _y: {t} = x; return 0; }}");
            assert_clean(&src);
        }
    }

    #[test]
    fn float_types_resolve() {
        assert_clean("fn main() -> i32 { let x: f32 = 1.0; let _y: f32 = x; return 0; }");
        assert_clean("fn main() -> i32 { let x: f64 = 1.0; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn integer_literal_infers_from_expected_type() {
        // Unsuffixed `42` becomes u64 because the let annotation says so.
        assert_clean("fn main() -> i32 { let x: u64 = 42; let _y: u64 = x; return 0; }");
    }

    #[test]
    fn float_literal_infers_from_expected_type() {
        assert_clean("fn main() -> i32 { let x: f32 = 1.5; let _y: f32 = x; return 0; }");
    }

    #[test]
    fn mixed_int_arithmetic_rejected() {
        let codes = errors("fn main() -> i32 { let x: i32 = 1i32 + 1u32; return x; }");
        assert!(codes.contains(&"E0302"), "expected mixed-type error, got: {codes:?}");
    }

    #[test]
    fn float_arithmetic_clean() {
        assert_clean("fn main() -> i32 { let x: f64 = 1.0 + 2.0 * 3.0; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn float_modulo_rejected_e0316() {
        assert_only_code("fn main() -> i32 { let x: f64 = 1.0 % 2.0; let _y: f64 = x; return 0; }", "E0316");
    }

    #[test]
    fn negate_unsigned_rejected() {
        let codes = errors("fn main() -> i32 { let x: u32 = 5; let _y: u32 = -x; return 0; }");
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn negate_float_clean() {
        assert_clean("fn main() -> i32 { let x: f64 = 5.0; let _y: f64 = -x; return 0; }");
    }

    // Casts

    #[test]
    fn cast_int_to_int_widen_clean() {
        assert_clean("fn main() -> i32 { let a: i8 = 5; let _b: i32 = a as i32; return 0; }");
    }

    #[test]
    fn cast_int_to_int_narrow_clean() {
        assert_clean("fn main() -> i32 { let a: i64 = 5; let _b: i8 = a as i8; return 0; }");
    }

    #[test]
    fn cast_int_to_float_clean() {
        assert_clean("fn main() -> i32 { let a: u32 = 5; let _b: f64 = a as f64; return 0; }");
    }

    #[test]
    fn cast_float_to_int_clean() {
        assert_clean("fn main() -> i32 { let a: f64 = 3.7; let _b: i32 = a as i32; return 0; }");
    }

    #[test]
    fn cast_bool_to_int_clean() {
        assert_clean("fn main() -> i32 { let _b: i32 = true as i32; return 0; }");
    }

    #[test]
    fn cast_int_to_bool_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: bool = 1 as bool; return 0; }", "E0315");
    }

    #[test]
    fn cast_float_to_bool_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: bool = 1.0 as bool; return 0; }", "E0315");
    }

    #[test]
    fn cast_bool_to_float_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: f64 = true as f64; return 0; }", "E0315");
    }

    #[test]
    fn comparison_works_on_all_numeric_types() {
        assert_clean("fn main() -> i32 { return if 1u64 < 2u64 { 1 } else { 0 }; }");
        assert_clean("fn main() -> i32 { return if 1.0 < 2.0 { 1 } else { 0 }; }");
        assert_clean("fn main() -> i32 { let a: i8 = 1; let b: i8 = 2; return if a < b { 1 } else { 0 }; }");
    }

    // ---- Phase 2 slice 2A: plain enums + paths ----

    #[test]
    fn enum_decl_clean() {
        assert_clean("enum Color { Red, Green, Blue }\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn enum_variant_path_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let _c: Color = Color::Red; return 0; }"
        );
    }

    #[test]
    fn enum_variant_in_comparison_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let c: Color = Color::Red; return if c == Color::Green { 1 } else { 0 }; }"
        );
    }

    #[test]
    fn enum_argument_and_return_clean() {
        assert_clean(include_str!("../../docs/examples/direction.cplus"));
    }

    #[test]
    fn duplicate_enum_variant_e0318() {
        let codes = errors("enum E { A, A }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0318"));
    }

    #[test]
    fn unknown_enum_variant_e0317() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = Color::Purple; return 0; }"
        );
        assert!(codes.contains(&"E0317"));
    }

    #[test]
    fn unknown_enum_in_path_e0303() {
        // `Foo` not declared anywhere.
        let codes = errors("fn main() -> i32 { let _x: i32 = Foo::Bar as i32; return 0; }");
        assert!(codes.contains(&"E0303"), "expected E0303 in {codes:?}");
    }

    #[test]
    fn ordering_on_enum_rejected_e0302() {
        let codes = errors(
            "enum E { A, B }\nfn main() -> i32 { if E::A < E::B { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn enum_to_int_cast_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { return Color::Green as i32; }"
        );
    }

    #[test]
    fn int_to_enum_cast_rejected_e0315() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = 0 as Color; return 0; }"
        );
        assert!(codes.contains(&"E0315"));
    }

    #[test]
    fn assigning_int_to_enum_rejected_e0302() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = 0; return 0; }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn assigning_enum_to_int_rejected_e0302() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _x: i32 = Color::Red; return 0; }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn cross_enum_comparison_rejected_e0302() {
        let codes = errors(
            "enum A { X }\nenum B { Y }\n\
             fn main() -> i32 { if A::X == B::Y { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn duplicate_enum_name_e0301() {
        let codes = errors("enum E { A }\nenum E { B }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    // ---- Phase 2 slice 2B: structs (no methods) ----

    #[test]
    fn struct_decl_clean() {
        assert_clean("struct Point { x: i32, y: i32 }\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn struct_literal_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let _p: Point = Point { x: 1, y: 2 }; return 0; }"
        );
    }

    #[test]
    fn empty_struct_clean() {
        assert_clean(
            "struct Empty {}\n\
             fn main() -> i32 { let _e: Empty = Empty {}; return 0; }"
        );
    }

    #[test]
    fn struct_field_read_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let _v: i32 = p.x; return 0; }"
        );
    }

    #[test]
    fn struct_field_write_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let mut p: Point = Point { x: 1, y: 2 }; p.x = 10; return 0; }"
        );
    }

    #[test]
    fn struct_passed_by_value_clean() {
        assert_clean(include_str!("../../docs/examples/point.cplus"));
    }

    #[test]
    fn nested_struct_clean() {
        assert_clean(include_str!("../../docs/examples/nested.cplus"));
    }

    #[test]
    fn mutable_struct_loop_clean() {
        assert_clean(include_str!("../../docs/examples/mutable_struct.cplus"));
    }

    #[test]
    fn duplicate_field_e0319() {
        let codes = errors("struct E { x: i32, x: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0319"));
    }

    #[test]
    fn unknown_field_in_access_e0320() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; let _v: i32 = a.y; return 0; }"
        );
        assert!(codes.contains(&"E0320"));
    }

    #[test]
    fn missing_field_in_literal_e0321() {
        let codes = errors(
            "struct A { x: i32, y: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1 }; return 0; }"
        );
        assert!(codes.contains(&"E0321"));
    }

    #[test]
    fn extra_field_in_literal_e0322() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1, y: 2 }; return 0; }"
        );
        assert!(codes.contains(&"E0322"));
    }

    #[test]
    fn field_access_on_non_struct_e0323() {
        let codes = errors(
            "fn main() -> i32 { let x: i32 = 5; let _v: i32 = x.foo; return 0; }"
        );
        assert!(codes.contains(&"E0323"));
    }

    #[test]
    fn field_assign_on_immutable_e0305() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; a.x = 2; return 0; }"
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn assign_to_temporary_struct_e0313() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { A { x: 1 }.x = 2; return 0; }"
        );
        assert!(codes.contains(&"E0313"));
    }

    #[test]
    fn duplicate_struct_name_e0301() {
        let codes = errors("struct P { x: i32 }\nstruct P { y: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn enum_struct_name_collision_e0301() {
        let codes = errors("enum X { A }\nstruct X { x: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn struct_eq_rejected_e0302() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; let b: A = A { x: 1 }; if a == b { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn nested_field_write_on_mutable_root_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             struct Line  { from: Point, to: Point }\n\
             fn main() -> i32 { let mut l: Line = Line { from: Point { x: 0, y: 0 }, to: Point { x: 0, y: 0 } }; l.to.x = 5; return 0; }"
        );
    }

    #[test]
    fn forward_ref_struct_field_clean() {
        // Struct B references A which is declared later.
        assert_clean(
            "struct B { a: A }\nstruct A { x: i32 }\nfn main() -> i32 { return 0; }"
        );
    }

    // ---- Phase 2 slice 2C: methods + impl blocks ----

    #[test]
    fn empty_impl_block_clean() {
        assert_clean("struct P {}\nimpl P {}\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn associated_function_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn new(x: i32) -> P { return P { x: x }; } }\n\
             fn main() -> i32 { let _p: P = P::new(5); return 0; }"
        );
    }

    #[test]
    fn ref_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }"
        );
    }

    #[test]
    fn ref_mut_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn set(mut self, v: i32) { self.x = v; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 0 }; p.set(5); return p.x; }"
        );
    }

    #[test]
    fn value_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn into_x(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.into_x(); }"
        );
    }

    #[test]
    fn methods_sample_clean() {
        assert_clean(include_str!("../../docs/examples/methods.cplus"));
    }

    #[test]
    fn impl_on_unknown_type_e0325() {
        let codes = errors("impl Foo { fn f(self) {} }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0325"));
    }

    #[test]
    fn impl_on_enum_e0325() {
        let codes = errors(
            "enum E { A }\nimpl E { fn f(self) {} }\nfn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0325"));
    }

    #[test]
    fn duplicate_method_e0326() {
        let codes = errors(
            "struct P {}\nimpl P { fn f(self) {} fn f(self) {} }\nfn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0326"));
    }

    #[test]
    fn no_such_method_e0324() {
        let codes = errors(
            "struct P {}\nimpl P {}\nfn main() -> i32 { let p: P = P {}; return p.missing(); }"
        );
        assert!(codes.contains(&"E0324"));
    }

    #[test]
    fn calling_assoc_fn_as_method_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn make() -> P { return P { x: 0 }; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; let _q: P = p.make(); return 0; }"
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_method_via_type_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { return P::get(); }"
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_mut_method_on_immutable_e0328() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bump(mut self) { self.x = self.x + 1; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; p.bump(); return 0; }"
        );
        assert!(codes.contains(&"E0328"));
    }

    #[test]
    fn self_in_function_body_e0300() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bad() -> i32 { return self.x; } }\n\
             fn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0300"));
    }

    #[test]
    fn method_via_field_chain_clean() {
        assert_clean(
            "struct Inner { v: i32 }\n\
             struct Outer { inner: Inner }\n\
             impl Inner { fn get(self) -> i32 { return self.v; } }\n\
             fn main() -> i32 { let o: Outer = Outer { inner: Inner { v: 42 } }; return o.inner.get(); }"
        );
    }

    #[test]
    fn enum_variant_not_callable_e0327() {
        let codes = errors(
            "enum E { A }\n\
             fn main() -> i32 { return E::A(); }"
        );
        assert!(codes.contains(&"E0327"));
    }

    // ---- Phase 2 slice 2D: fixed-size arrays ----

    #[test]
    fn array_decl_and_literal_clean() {
        assert_clean(
            "fn main() -> i32 { let _xs: [i32; 3] = [1, 2, 3]; return 0; }"
        );
    }

    #[test]
    fn array_indexing_clean() {
        assert_clean(
            "fn main() -> i32 { let xs: [i32; 3] = [10, 20, 30]; return xs[0 as usize]; }"
        );
    }

    #[test]
    fn array_indexed_assign_clean() {
        assert_clean(
            "fn main() -> i32 { let mut xs: [i32; 3] = [0, 0, 0]; xs[1 as usize] = 5; return xs[1 as usize]; }"
        );
    }

    #[test]
    fn array_as_struct_field_clean() {
        assert_clean(include_str!("../../docs/examples/array_struct.cplus"));
    }

    #[test]
    fn array_sum_sample_clean() {
        assert_clean(include_str!("../../docs/examples/array_sum.cplus"));
    }

    #[test]
    fn array_literal_length_mismatch_e0330() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 3] = [1, 2]; return 0; }");
        assert!(codes.contains(&"E0330"), "expected E0330, got: {codes:?}");
    }

    #[test]
    fn array_literal_mixed_types_e0329() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 2] = [1, true]; return 0; }");
        assert!(codes.contains(&"E0329"));
    }

    #[test]
    fn indexing_non_array_e0331() {
        let codes = errors("fn main() -> i32 { let x: i32 = 5; return x[0 as usize]; }");
        assert!(codes.contains(&"E0331"));
    }

    #[test]
    fn empty_array_literal_e0332() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 0] = []; return 0; }");
        assert!(codes.contains(&"E0332"));
    }

    #[test]
    fn array_field_indexed_write_on_immutable_e0305() {
        let codes = errors(
            "struct C { xs: [i32; 2] }\n\
             fn main() -> i32 { let c: C = C { xs: [0, 0] }; c.xs[0 as usize] = 5; return 0; }"
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn array_in_function_signature_clean() {
        assert_clean(
            "fn first(xs: [i32; 3]) -> i32 { return xs[0 as usize]; }\n\
             fn main() -> i32 { return first([10, 20, 30]); }"
        );
    }

    #[test]
    fn diagnostic_includes_correct_span() {
        let diags = check_src("fn main() -> i32 { return foo(); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.0, "E0300");
        // span must point at `foo`, which starts at byte offset 26
        assert_eq!(diags[0].primary.start.byte, 26);
    }

    // ----- Phase 3 slice 3A: ownership markers on params -----

    #[test]
    fn mut_and_move_on_param_e0334() {
        let codes = errors(
            "fn f(mut move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return f(1); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    #[test]
    fn move_and_mut_on_param_e0334() {
        let codes = errors(
            "fn f(move mut x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return f(1); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    #[test]
    fn mut_param_makes_binding_mutable() {
        // `mut x: T` should allow writing `x = ...` inside the body without
        // E0305 (assignment to immutable binding).
        assert_clean(
            "fn inc(mut x: i32) -> i32 { x = x + 1; return x; }\n\
             fn main() -> i32 { return inc(1); }",
        );
    }

    #[test]
    fn plain_param_remains_immutable_e0305() {
        let codes = errors(
            "fn bad(x: i32) -> i32 { x = x + 1; return x; }\n\
             fn main() -> i32 { return bad(1); }",
        );
        assert!(codes.contains(&"E0305"), "expected E0305, got: {codes:?}");
    }

    #[test]
    fn move_param_parses_clean() {
        // `move x: T` is accepted; full move tracking is deferred to a later
        // slice of Phase 3, so this should currently behave like a plain param.
        assert_clean(
            "fn consume(move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return consume(7); }",
        );
    }

    #[test]
    fn move_self_method_parses_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn take(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 4 }; return p.take(); }",
        );
    }

    #[test]
    fn mut_and_move_on_method_param_e0334() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn f(self, mut move y: i32) -> i32 { return self.x + y; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; return p.f(2); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    // ----- Phase 3 slice 3A: move tracking + E0335 -----
    //
    // Six tests below are marked `#[ignore]`: their original inputs used
    // `struct P { x: i32 }`, but with Copy auto-derive (slice 3C) such a
    // struct is `Copy`, which makes `move` a no-op consumption. The move
    // tracking machinery is still wired — it's just dormant for these
    // inputs. Revive these tests when a non-Copy aggregate type exists
    // in the language (e.g. once strings, heap types, or an explicit
    // `nocopy` marker land). See `docs/design/phase3-copy-derivation.md`
    // §7.1 for the broader plan.

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_param_consumes_non_copy_binding_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); return p.x; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_param_double_call_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); let r: i32 = take(p); return 0; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_self_consumes_receiver_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = p.into_x(); return p.x; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_self_double_call_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = p.into_x(); let r: i32 = p.into_x(); return 0; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_on_copy_param_does_not_consume() {
        // `move x: i32` is redundant — `i32` is Copy, so the source remains
        // usable. (A future E0336 lint will suggest removing the keyword.)
        assert_clean(
            "fn take(move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { let x: i32 = 5; let r: i32 = take(x); return x; }",
        );
    }

    #[test]
    fn shared_borrow_does_not_consume() {
        // `p: P` (no `move`) is a shared borrow at the design level; in
        // Phase 3 it doesn't track borrows yet, but the source must remain
        // usable across calls.
        assert_clean(
            "struct P { x: i32 }\n\
             fn read(p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; let a: i32 = read(p); let b: i32 = read(p); return a + b; }",
        );
    }

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_from_field_e0337() {
        // Partial moves out of struct fields are deferred. `move`-arg from
        // a field expression must be rejected — but only if the consumption
        // would have been real (non-Copy). Under auto-derive Inner is Copy
        // so the consumption is a silent no-op; no E0337 fires.
        let codes = errors(
            "struct Inner { x: i32 }\n\
             struct Outer { i: Inner }\n\
             fn take(move i: Inner) -> i32 { return i.x; }\n\
             fn main() -> i32 { let o: Outer = Outer { i: Inner { x: 1 } }; return take(o.i); }",
        );
        assert!(codes.contains(&"E0337"), "expected E0337, got: {codes:?}");
    }

    #[test]
    fn move_chain_through_function_is_clean() {
        // Building owned values, threading them through one consuming call,
        // and producing an owned result: nothing should remain usable, but
        // also nothing should error.
        assert_clean(
            "struct P { x: i32 }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 42 }; return take(p); }",
        );
    }

    #[test]
    #[ignore = "needs a non-Copy aggregate type to exercise; revive when one exists"]
    fn move_then_assign_recovers_binding() {
        // Sanity check the boundary: once moved, the binding stays moved.
        // (A re-`let` would shadow it, but the same `p` cannot be revived.)
        let codes = errors(
            "struct P { x: i32 }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); let q: i32 = p.x; return q; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    // ----- Phase 3 slice 3C: Copy auto-derive -----

    #[test]
    fn copy_struct_remains_usable_after_pass() {
        // `Point { x: i32, y: i32 }` is Copy under auto-derive. Passing by
        // value (default shared) does not consume; the source stays usable.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn read(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: Point = Point { x: 3, y: 4 }; let a: i32 = read(p); let b: i32 = p.y; return a + b; }",
        );
    }

    #[test]
    fn copy_struct_with_array_field_is_copy() {
        // Array of Copy → Copy. Struct containing array of Copy → Copy.
        assert_clean(
            "struct C { xs: [i32; 3] }\n\
             fn first(c: C) -> i32 { return c.xs[0 as usize]; }\n\
             fn main() -> i32 { let c: C = C { xs: [1, 2, 3] }; let a: i32 = first(c); return a + c.xs[1 as usize]; }",
        );
    }

    #[test]
    fn nested_copy_struct_is_copy() {
        assert_clean(
            "struct Inner { x: i32 }\n\
             struct Outer { i: Inner, k: i32 }\n\
             fn read(o: Outer) -> i32 { return o.i.x + o.k; }\n\
             fn main() -> i32 { let o: Outer = Outer { i: Inner { x: 1 }, k: 2 }; let _a: i32 = read(o); return o.i.x; }",
        );
    }

    #[test]
    fn copy_struct_move_marker_is_silent_noop() {
        // `move p: Point` on a Copy struct: redundant marker, source still
        // usable. Same shape as the existing `move_on_copy_param_does_not_consume`
        // test for `i32` — now extended to aggregates under auto-derive.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn take(move p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let a: i32 = take(p); return a + p.y; }",
        );
    }

    #[test]
    fn copy_struct_move_self_is_silent_noop() {
        // `move self` on a Copy receiver: ditto.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             impl Point { fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let a: i32 = p.into_x(); return a + p.y; }",
        );
    }
}
