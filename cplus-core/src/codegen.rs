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
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write;

/// Slice 1B (v0.0.2): module-level metadata table. `!range` nodes need
/// module-unique IDs and must appear outside any function definition.
/// `register_range` allocates an ID, records the definition, and returns
/// the ID so codegen can splice `, !range !N` onto the relevant load/call.
///
/// IDs start at 100_000 to leave the 0..6 + 6.. range untouched for the
/// DWARF metadata block (see `emit_dwarf_metadata`). The two allocators do
/// not interleave because DWARF emits at module-end after codegen, and the
/// DWARF range never reaches 100_000 — that would require ~50k functions
/// in one module.
#[derive(Default)]
struct ModuleMetadata {
    next_id: Cell<u32>,
    nodes: RefCell<Vec<String>>,
    /// Cache so equal (lo, hi, ty_str) tuples share one MD node.
    cache: RefCell<HashMap<(i64, i64, &'static str), u32>>,
}

impl ModuleMetadata {
    fn new() -> Self {
        Self {
            next_id: Cell::new(100_000),
            ..Self::default()
        }
    }
    /// Allocate a `!N = !{<ty> <lo>, <ty> <hi>}` range metadata node and
    /// return `N`. `hi` is exclusive per LLVM convention.
    fn register_range(&self, lo: i64, hi: i64, ty: &'static str) -> u32 {
        if let Some(&id) = self.cache.borrow().get(&(lo, hi, ty)) {
            return id;
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes.borrow_mut().push(format!("!{id} = !{{{ty} {lo}, {ty} {hi}}}"));
        self.cache.borrow_mut().insert((lo, hi, ty), id);
        id
    }

    /// Slice 1C: allocate a self-referential `!alias.scope` domain node.
    /// Each function that has >= 2 noalias pointer params gets one domain.
    /// IR form: `!N = distinct !{!N, !"label"}` — the self-reference makes
    /// it distinct from any other domain even if labels collide.
    fn register_alias_domain(&self, label: &str) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes.borrow_mut().push(
            format!("!{id} = distinct !{{!{id}, !\"{label}\"}}")
        );
        id
    }

    /// Slice 1C: allocate a self-referential scope node tied to `domain_id`.
    /// IR form: `!N = distinct !{!N, !D, !"label"}`.
    fn register_alias_scope(&self, domain_id: u32, label: &str) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes.borrow_mut().push(
            format!("!{id} = distinct !{{!{id}, !{domain_id}, !\"{label}\"}}")
        );
        id
    }

    /// Slice 1C: allocate a list of scope ids used by `!alias.scope` or
    /// `!noalias`. Both clauses take a list of scope refs.
    /// IR form: `!N = !{!S1, !S2, ...}`. Empty list returns the "empty
    /// scope-list" id, which has no effect — caller should skip emitting
    /// the attribute entirely in that case.
    fn register_alias_scope_list(&self, scopes: &[u32]) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let items: Vec<String> = scopes.iter().map(|s| format!("!{s}")).collect();
        self.nodes.borrow_mut().push(
            format!("!{id} = !{{{}}}", items.join(", "))
        );
        id
    }

    /// Drain the accumulated metadata definitions into the output. Must be
    /// called once at module-end, after all function bodies are emitted.
    fn emit_into(&self, out: &mut String) {
        let nodes = self.nodes.borrow();
        if nodes.is_empty() { return; }
        for line in nodes.iter() {
            out.push_str(line);
            out.push('\n');
        }
    }
}

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
    generate_inner(program, mode, None, None, &[], false)
}

/// Phase 5 Slice 5.B: generate IR for a library target. Non-`pub` items
/// get `internal` linkage so LTO can strip unused implementation detail
/// from the final `.dylib` / `.a`. `pub` items keep external linkage and
/// form the C-callable public ABI.
///
/// Distinct from `generate` so existing executable builds (and the
/// substring-pinned test suite that goes with them) keep their current
/// linkage exactly. Eventually the bin path can share this rule too —
/// internal linkage is correct everywhere — but ship + verify first.
pub fn generate_lib(program: &Program, mode: BuildMode) -> String {
    generate_inner(program, mode, None, None, &[], true)
}

/// Phase 11 polish (2026-05-13): emit LLVM IR with DWARF debug
/// metadata. v1 ships function-level info only — DICompileUnit, DIFile,
/// and one DISubprogram per function. Per-instruction `!DILocation`
/// (for line-by-line stepping) is a follow-up. lldb can still identify
/// function symbols in stack traces and set breakpoints by name.
pub fn generate_with_debug(program: &Program, mode: BuildMode, source_file: &std::path::Path) -> String {
    generate_inner(program, mode, None, Some(source_file), &[], false)
}

/// Phase 11 polish (2026-05-13): emit LLVM IR with sanitizer function
/// attributes (`sanitize_address`, `sanitize_thread`, `sanitize_memory`)
/// attached to every user-defined `define`. Required for clang's
/// sanitizer passes to instrument code that originates from a `.ll`
/// (the C path auto-attaches these; the IR path doesn't).
pub fn generate_with_options(
    program: &Program,
    mode: BuildMode,
    source_file: Option<&std::path::Path>,
    sanitizers: &[&str],
) -> String {
    generate_inner(program, mode, None, source_file, sanitizers, false)
}

/// Slice 5ATTR.4: emit a test-runner binary. The output IR contains every
/// `#[test]` function in the program plus a synthesized `main` that:
///   1. Zeroes `@cpc_test_failed` before each test,
///   2. Calls the test,
///   3. For `fn() -> i32` tests, ORs the return value into the failure check,
///   4. Prints `test <name> ... ok` / `... FAILED` (or one JSON line per
///      test when `json` is true),
///   5. Tracks pass / fail counts and returns the fail count.
///
/// `assert` statements throughout the project lower to a flag write instead
/// of `llvm.trap` so a failed assertion sets the flag, the test function
/// falls through (Phase 5 has no raw pointers — fall-through is safe), and
/// the driver reads the flag after the call.
///
/// The project's user-defined `fn main` (if any) is *not* emitted; the
/// synthesized driver replaces it.
pub fn generate_test_binary(
    program: &Program,
    mode: BuildMode,
    tests: &[crate::attrs::TestFn],
    json: bool,
) -> String {
    generate_inner(program, mode, Some(TestDriverConfig { tests, json }), None, &[], false)
}

struct TestDriverConfig<'a> {
    tests: &'a [crate::attrs::TestFn],
    json: bool,
}

fn generate_inner(
    program: &Program,
    mode: BuildMode,
    test_cfg: Option<TestDriverConfig<'_>>,
    debug_source: Option<&std::path::Path>,
    sanitizers: &[&str],
    is_lib: bool,
) -> String {
    let types = collect_types(program);
    let sigs = collect_sigs(program, &types);
    let test_mode = test_cfg.is_some();
    let mut out = String::new();
    // Slice 1B: module-level `!range` metadata table. Allocated as we
    // codegen each function; flushed to `out` after every function body is
    // written and before DWARF (which has its own ID range).
    let md = ModuleMetadata::new();
    write_preamble(&mut out);
    if test_mode {
        // Shared per-test failure flag — written by `assert` in any function
        // called by a test, read by the driver `main` after each test call.
        out.push_str("@cpc_test_failed = global i32 0\n\n");
    }
    // Phase 8 slice 8.STR.1: collect every unique string literal in the
    // program, emit one `@.str.N` global per unique payload, build a
    // lookup table so gen_expr can resolve a literal to its global.
    let str_lits = collect_and_emit_str_lits(&mut out, program);
    write_struct_decls(&mut out, &types, program);
    // Phase 11 / ObjC interop: multiple `extern fn` declarations may share
    // a single linker symbol via `#[link_name = "..."]`. Track emitted
    // symbols so we never emit two `declare`s with the same name (LLVM
    // rejects that as a redefinition).
    let mut emitted_extern_symbols: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) => {
                // Test driver replaces the user's `main`. Other functions go
                // through unchanged so tests can call helpers, and so a
                // `#[test]` function's own body is emitted normally.
                if test_mode && f.name.name == "main" { continue; }
                // Slice 7GEN.4: generic functions don't emit pre-monomorphization.
                // Slice 7GEN.5 will walk a work-queue of instantiations.
                if !f.generic_params.is_empty() { continue; }
                gen_function(&mut out, f, &sigs, &types, &str_lits, mode, test_mode, &mut emitted_extern_symbols, &md, is_lib);
            }
            ItemKind::Impl(b) => {
                let Some(&id) = types.struct_by_name.get(&b.target.name) else { continue; };
                for m in &b.methods {
                    // Slice 7GEN.5e: generic methods are codegen-skipped
                    // pre-monomorphization. Their Ty::Param-bearing
                    // signatures and bodies are emitted as concrete
                    // copies by the monomorphize pass.
                    if !m.generic_params.is_empty() { continue; }
                    gen_method(&mut out, id, m, &sigs, &types, &str_lits, mode, test_mode, &md, is_lib);
                }
            }
            // Slice 7GEN.3: interface declarations have no runtime
            // presence — they're sema-time contracts. No IR emission.
            ItemKind::Interface(_) => {}
            // Phase 11 polish: type aliases are sema-only — resolved away
            // before codegen ever sees them.
            ItemKind::TypeAlias(_) => {}
            ItemKind::Enum(_) | ItemKind::Struct(_) => {
                // Enum types are erased to i32; struct types are declared
                // upfront in `write_struct_decls`. Nothing to emit per-item.
            }
        }
    }
    if let Some(cfg) = test_cfg {
        emit_test_driver_main(&mut out, cfg.tests, cfg.json);
    }
    // Slice 1B: flush the accumulated `!N = !{...}` range metadata table
    // before DWARF (which writes its own metadata block). DWARF allocates
    // IDs starting at 0; our range table starts at 100_000 — disjoint.
    md.emit_into(&mut out);
    // Phase 11 polish (2026-05-13): DWARF debug metadata. v1 emits
    // module flags + DICompileUnit + DIFile + one DISubprogram per
    // function (named, line-numbered). Per-instruction DILocation is
    // a follow-up — for now lldb identifies function symbols in stack
    // traces and accepts `break <fn>` by name; line-stepping is
    // degenerate (lands at function entry).
    if let Some(path) = debug_source {
        let src = std::fs::read_to_string(path).ok();
        emit_dwarf_metadata(&mut out, program, path, src.as_deref());
    }
    if !sanitizers.is_empty() {
        attach_sanitizer_attrs(&mut out, sanitizers);
    }
    out
}

/// Phase 11 polish (2026-05-13): attach `sanitize_*` attributes to
/// every user-defined `define` line. clang's sanitizer passes only
/// instrument functions carrying these attributes; for source compiled
/// via clang the C frontend auto-attaches them, but cpc emits IR
/// directly so we do it here. Inline-attribute syntax keeps the IR
/// trivially diff-able without dragging in attribute groups.
fn attach_sanitizer_attrs(out: &mut String, sanitizers: &[&str]) {
    let attrs: Vec<&str> = sanitizers.iter().filter_map(|s| match *s {
        "address" => Some("sanitize_address"),
        "thread" => Some("sanitize_thread"),
        "memory" => Some("sanitize_memory"),
        // UBSan doesn't gate on a function attribute — its checks are
        // inserted unconditionally by the pass.
        "undefined" => None,
        _ => None,
    }).collect();
    if attrs.is_empty() { return; }
    let attr_str = attrs.join(" ");
    let original = std::mem::take(out);
    for line in original.lines() {
        if line.starts_with("define ") {
            // Insert ` <attrs>` after the `)` and before the `{` (or
            // before any trailing `!dbg !N` attached for DWARF).
            if let Some(close_paren) = line.find(')') {
                let after = &line[close_paren + 1..];
                let head = &line[..=close_paren];
                out.push_str(head);
                out.push(' ');
                out.push_str(&attr_str);
                out.push_str(after);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
}

/// Emit module-flag + !llvm.dbg.cu + DICompileUnit + DIFile metadata,
/// plus one DISubprogram per program function. Post-processes the IR
/// to attach `!dbg !N` to each `define` line.
fn emit_dwarf_metadata(
    out: &mut String,
    program: &Program,
    source_file: &std::path::Path,
    src: Option<&str>,
) {
    let line_map = src.map(crate::diagnostics::LineMap::new);
    let abs = source_file.canonicalize().unwrap_or_else(|_| source_file.to_path_buf());
    let filename = abs.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("source.cplus")
        .to_string();
    let directory = abs.parent()
        .and_then(|p| p.to_str())
        .unwrap_or(".")
        .to_string();
    // Reserve metadata ids:
    //   !0 = DwarfVersion flag
    //   !1 = DebugInfoVersion flag
    //   !2 = DICompileUnit
    //   !3 = DIFile
    //   !4 = DISubroutineType (shared "unknown type" placeholder)
    //   !5 = empty types array
    //   !6.. = pairs of (DISubprogram, DILocation) per fn
    let cu_id = 2u32;
    let file_id = 3u32;
    let sub_type_id = 4u32;
    let empty_types_id = 5u32;
    let mut next_md = 6u32;
    // First pass: collect function names + their start line.
    // Per function we allocate two metadata ids: a DISubprogram (the
    // `define ... !dbg !S` anchor) and a DILocation (the `!dbg !L`
    // attached to every call inside that function — required so clang
    // doesn't drop the whole DI block).
    struct FnDi { sub_id: u32, loc_id: u32, line: u32 }
    let mut fn_meta: std::collections::HashMap<String, FnDi> = std::collections::HashMap::new();
    let alloc_pair = |name: String, line: u32, fn_meta: &mut std::collections::HashMap<String, FnDi>, next_md: &mut u32| {
        let sub_id = *next_md;
        let loc_id = *next_md + 1;
        *next_md += 2;
        fn_meta.insert(name, FnDi { sub_id, loc_id, line });
    };
    for item in &program.items {
        let ItemKind::Function(f) = &item.kind else { continue; };
        if !f.generic_params.is_empty() { continue; }
        let line = line_for_span(f.name.span, line_map.as_ref(), src);
        let user_name = if f.name.name == "main" { "main".to_string() } else { f.name.name.clone() };
        alloc_pair(user_name, line, &mut fn_meta, &mut next_md);
    }
    // Methods on impl blocks: emit one DISubprogram per method too.
    // The IR's `define` line uses the mangled `Type.method` name.
    for item in &program.items {
        let ItemKind::Impl(b) = &item.kind else { continue; };
        for m in &b.methods {
            if !m.generic_params.is_empty() { continue; }
            let mangled = format!("{}.{}", b.target.name, m.name.name);
            let line = line_for_span(m.name.span, line_map.as_ref(), src);
            alloc_pair(mangled, line, &mut fn_meta, &mut next_md);
        }
    }
    // Post-process the IR. Two attachments:
    //   1. `define ... !dbg !S { ... }` — attach the function's
    //      DISubprogram to the `define` line.
    //   2. `<call|invoke> ..., !dbg !L` — attach the function's
    //      DILocation to every call inside its body. clang rejects DI
    //      blocks where a call instruction inside a debug-info'd
    //      function lacks a `!dbg`.
    let original = std::mem::take(out);
    let mut current_loc_id: Option<u32> = None;
    for line in original.lines() {
        // Function definition line.
        if let Some(stripped) = line.strip_prefix("define ") {
            if let Some(at) = stripped.find('@') {
                let after_at = &stripped[at + 1..];
                if let Some(paren) = after_at.find('(') {
                    let raw_name = &after_at[..paren];
                    if let Some(meta) = fn_meta.get(raw_name) {
                        current_loc_id = Some(meta.loc_id);
                        if let Some(brace) = line.rfind('{') {
                            let (head, tail) = line.split_at(brace);
                            out.push_str(head.trim_end());
                            out.push_str(&format!(" !dbg !{} ", meta.sub_id));
                            out.push_str(tail);
                            out.push('\n');
                            continue;
                        }
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // End of a function body: a sole `}` at column 0.
        if line == "}" {
            current_loc_id = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Inside a debug-info'd function: attach !dbg to any `call`
        // or `invoke` instruction so clang accepts the DI block.
        if let Some(loc) = current_loc_id {
            let trimmed = line.trim_start();
            let is_call = trimmed.starts_with("call ")
                || trimmed.starts_with("invoke ")
                // SSA-assignment form: `%v = call ...`.
                || (trimmed.starts_with('%')
                    && trimmed.contains("= call ")
                    && !trimmed.contains("!dbg"));
            if is_call && !line.contains("!dbg") {
                out.push_str(line);
                out.push_str(&format!(", !dbg !{loc}"));
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    // Emit the metadata block.
    out.push('\n');
    out.push_str("!llvm.module.flags = !{!0, !1}\n");
    out.push_str("!llvm.dbg.cu = !{!2}\n");
    out.push('\n');
    out.push_str("!0 = !{i32 2, !\"Dwarf Version\", i32 4}\n");
    out.push_str("!1 = !{i32 2, !\"Debug Info Version\", i32 3}\n");
    out.push_str(&format!(
        "!{cu_id} = distinct !DICompileUnit(language: DW_LANG_C99, file: !{file_id}, \
         producer: \"cpc\", isOptimized: false, runtimeVersion: 0, \
         emissionKind: FullDebug, splitDebugInlining: false)\n"
    ));
    out.push_str(&format!(
        "!{file_id} = !DIFile(filename: \"{}\", directory: \"{}\")\n",
        escape_dwarf_str(&filename), escape_dwarf_str(&directory)
    ));
    out.push_str(&format!(
        "!{sub_type_id} = !DISubroutineType(types: !{empty_types_id})\n"
    ));
    out.push_str(&format!("!{empty_types_id} = !{{null}}\n"));
    // Sort fn_meta entries by sub_id for stable output. Each entry
    // emits a DISubprogram immediately followed by its DILocation.
    let mut sorted: Vec<(String, u32, u32, u32)> = fn_meta.iter()
        .map(|(n, m)| (n.clone(), m.sub_id, m.loc_id, m.line))
        .collect();
    sorted.sort_by_key(|e| e.1);
    for (name, sub_id, loc_id, line) in sorted {
        out.push_str(&format!(
            "!{sub_id} = distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", \
             scope: !{file_id}, file: !{file_id}, line: {line}, type: !{sub_type_id}, \
             scopeLine: {line}, spFlags: DISPFlagDefinition, unit: !{cu_id})\n",
            escape_dwarf_str(&name), escape_dwarf_str(&name),
        ));
        out.push_str(&format!(
            "!{loc_id} = !DILocation(line: {line}, column: 1, scope: !{sub_id})\n"
        ));
    }
}

fn line_for_span(span: crate::lexer::Span, line_map: Option<&crate::diagnostics::LineMap>, src: Option<&str>) -> u32 {
    match (line_map, src) {
        (Some(lm), Some(s)) => lm.position(span.start, s).line,
        _ => 1,
    }
}

fn escape_dwarf_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone)]
struct FnSig {
    /// Parameter info: type, `move_` flag, `mutable` flag.
    ///   - `move_` decides whether a Drop-bearing argument transfers ownership
    ///     across the call (caller flips its drop-flag, callee registers one).
    ///   - `mutable` paired with non-Copy struct type triggers the §2.9
    ///     exclusive-borrow ABI: callee receives a `ptr`, not a value copy,
    ///     so field writes propagate back to the caller (slice 5BC.codegen).
    params: Vec<(Ty, bool, bool)>,
    return_type: Ty,
    /// Slice 10.FFI.4: variadic extern fn. Call sites for these emit
    /// `call ret_ty (fixed_types, ...) @name(args)` — the full
    /// function-type prefix is required by LLVM for varargs.
    is_variadic: bool,
    /// Phase 11 / ObjC interop: `#[link_name = "..."]` symbol alias.
    /// When `Some(s)`, codegen emits `declare ... @s(...)` and call
    /// sites use `@s` instead of `@<source_name>`. Only ever set on
    /// extern fns; sema rejects on non-extern.
    link_name: Option<String>,
}

fn collect_sigs(p: &Program, types: &TypeTable) -> HashMap<String, FnSig> {
    let mut sigs = HashMap::new();
    // builtin: println(i32) -> ()
    sigs.insert(
        "println".to_string(),
        FnSig { params: vec![(Ty::I32, false, false)], return_type: Ty::Unit, is_variadic: false, link_name: None },
    );
    for item in &p.items {
        let ItemKind::Function(f) = &item.kind else { continue; };
        // Slice 7GEN.4: generic fns are not emitted pre-monomorphization;
        // their signatures aren't part of the concrete call graph yet.
        if !f.generic_params.is_empty() { continue; }
        let params: Vec<(Ty, bool, bool)> = f.params.iter()
            .map(|p| (ty_from(&p.ty, types), p.move_, p.mutable))
            .collect();
        let ret = match &f.return_type {
            Some(t) => ty_from(t, types),
            None => Ty::Unit,
        };
        let link_name = f.attributes.iter().find_map(|a| {
            if a.path.name != "link_name" { return None; }
            match a.args.as_slice() {
                [AttrArg::Str(s, _)] => Some(s.clone()),
                _ => None,
            }
        });
        sigs.insert(f.name.name.clone(), FnSig { params, return_type: ret, is_variadic: f.is_variadic, link_name });
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
    /// Variant name → declaration-order index (the runtime tag value).
    variants: HashMap<String, u32>,
    /// Variants in declaration order, with payload type lists. Plain enums
    /// have all-empty payloads; tagged enums have at least one non-empty.
    variant_payloads: Vec<Vec<Ty>>,
    /// True iff at least one variant carries a payload. Codegen branches on
    /// this: plain enums stay bare `i32` (Phase-2A fast path); tagged enums
    /// use the `{ i32 tag, [N x i64] payload }` layout.
    is_tagged: bool,
    /// Number of 8-byte slots in the tagged-enum payload area. 0 for plain
    /// enums. For tagged enums, this is the max across variants of
    /// `payload.len()` — Phase 3 uses one 8-byte slot per payload value
    /// regardless of the value's actual size (simple, wastes some bytes;
    /// alignment is naturally 8 everywhere).
    payload_slots: u32,
    /// Mirror of sema's Copy fixpoint. Plain enums are always Copy; tagged
    /// enums are Copy iff every variant's payload type list is all-Copy.
    is_copy: bool,
}

#[derive(Debug, Clone)]
struct StructInfo {
    name: String,
    /// Fields in declaration order. The pair is (field name, field type).
    fields: Vec<(String, Ty)>,
    /// Methods declared in `impl` blocks for this struct.
    methods: HashMap<String, MethodInfo>,
    /// True iff this struct has a destructor — a method named `drop` with
    /// signature `fn drop(mut self)`. Sema validates the signature; codegen
    /// mirrors the flag to decide whether `let x: T = ...` registers a
    /// scope-exit drop call. See `docs/design/phase3-drop.md`.
    is_drop: bool,
    /// Mirror of sema's Copy fixpoint. A struct is Copy iff it has no Drop
    /// destructor and every field is Copy. Used by the §2.9 mutable-borrow
    /// ABI in `param_passes_by_ptr` — non-Copy `mut x: T` is pointer-passed
    /// so the callee's writes propagate back to the caller.
    is_copy: bool,
}

#[derive(Debug, Clone)]
struct MethodInfo {
    receiver: Option<Receiver>,
    /// Parameter info, excluding the receiver: `(ty, move_, mutable)`.
    /// `move_` drives call-site drop-flag flips; `mutable` drives the §2.9
    /// pointer-pass ABI for non-Copy struct params (slice 5BC.codegen).
    params: Vec<(Ty, bool, bool)>,
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
                // Slice 7GEN.4: generic enum templates are not emitted
                // pre-monomorphization. Slice 7GEN.5 will register a
                // per-instantiation type entry as the work-queue drains.
                if !e.generic_params.is_empty() { continue; }
                let id = EnumId(t.enum_defs.len() as u32);
                let mut variants = HashMap::new();
                let mut empty_payloads: Vec<Vec<Ty>> = Vec::new();
                for (idx, v) in e.variants.iter().enumerate() {
                    variants.entry(v.name.name.clone()).or_insert(idx as u32);
                    empty_payloads.push(Vec::new());   // resolved in pass 2 below
                }
                let is_tagged = e.variants.iter().any(|v| !v.payload.is_empty());
                t.enum_defs.push(EnumInfo {
                    variants,
                    variant_payloads: empty_payloads,
                    is_tagged,
                    payload_slots: 0,   // computed in pass 2 below
                    // Plain enums are Copy unconditionally; tagged enums are
                    // resolved by the fixpoint in `compute_copy_flags`.
                    is_copy: !is_tagged,
                });
                t.enum_by_name.insert(e.name.name.clone(), id);
            }
            ItemKind::Struct(s) => {
                if t.enum_by_name.contains_key(&s.name.name) || t.struct_by_name.contains_key(&s.name.name) {
                    continue;
                }
                // Slice 7GEN.4: generic struct templates are not emitted
                // pre-monomorphization. Slice 7GEN.5 lands the work-queue.
                if !s.generic_params.is_empty() { continue; }
                let id = StructId(t.struct_defs.len() as u32);
                t.struct_defs.push(StructInfo {
                    name: s.name.name.clone(),
                    fields: Vec::new(),
                    methods: HashMap::new(),
                    is_drop: false,
                    is_copy: false,   // computed in `compute_copy_flags`
                });
                t.struct_by_name.insert(s.name.name.clone(), id);
            }
            ItemKind::Function(_) | ItemKind::Impl(_) | ItemKind::Interface(_) | ItemKind::TypeAlias(_) => {}
        }
    }
    // Second pass: resolve struct field types.
    for item in &p.items {
        let ItemKind::Struct(s) = &item.kind else { continue; };
        if !s.generic_params.is_empty() { continue; }
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
    // Second-and-a-half pass: resolve enum variant payload types now that
    // every struct and enum name is registered. Also compute payload_slots
    // for tagged enums (max of variant payload arities).
    for item in &p.items {
        let ItemKind::Enum(e) = &item.kind else { continue; };
        if !e.generic_params.is_empty() { continue; }
        let Some(&id) = t.enum_by_name.get(&e.name.name) else { continue; };
        let mut max_slots: u32 = 0;
        let mut payloads: Vec<Vec<Ty>> = Vec::with_capacity(e.variants.len());
        for v in &e.variants {
            let p: Vec<Ty> = v.payload.iter().map(|ty| ty_from(ty, &t)).collect();
            max_slots = max_slots.max(p.len() as u32);
            payloads.push(p);
        }
        t.enum_defs[id.0 as usize].variant_payloads = payloads;
        t.enum_defs[id.0 as usize].payload_slots = max_slots;
    }
    // Third pass: collect methods from impl blocks.
    for item in &p.items {
        let ItemKind::Impl(b) = &item.kind else { continue; };
        let Some(&id) = t.struct_by_name.get(&b.target.name) else { continue; };
        for m in &b.methods {
            if t.struct_defs[id.0 as usize].methods.contains_key(&m.name.name) {
                continue;
            }
            // Slice 7GEN.5e: skip generic method templates in codegen
            // type collection. Monomorphized concrete copies will be
            // emitted via the monomorphize pass.
            if !m.generic_params.is_empty() { continue; }
            let params: Vec<(Ty, bool, bool)> = m.params.iter()
                .map(|p| (ty_from(&p.ty, &t), p.move_, p.mutable))
                .collect();
            let return_type = match &m.return_type {
                Some(ty) => ty_from(ty, &t),
                None => Ty::Unit,
            };
            t.struct_defs[id.0 as usize].methods.insert(
                m.name.name.clone(),
                MethodInfo { receiver: m.receiver, params, return_type },
            );
            // Mirror sema's Drop detection so codegen knows which bindings
            // need scope-exit drop emission. Sema has already validated the
            // signature; we trust it here.
            if m.name.name == "drop" {
                t.struct_defs[id.0 as usize].is_drop = true;
            }
        }
    }
    // Fourth pass: fixpoint Copy resolution across structs and tagged enums.
    // Mirrors sema's `compute_struct_copy_flags` and `compute_enum_copy_flags`
    // — the answer must be identical so the borrow-checker's classification
    // (sema's `is_copy`) matches the ABI choice codegen makes here.
    compute_copy_flags(&mut t);
    t
}

/// Mirror of sema's Copy fixpoint, on codegen's own `TypeTable`.
/// A struct is Copy iff it has no `drop` destructor and every field is Copy.
/// A tagged enum is Copy iff every variant payload type is Copy.
/// Plain enums are pre-marked Copy at construction; primitives and arrays of
/// Copy elements are handled directly by `is_copy_ty`.
fn compute_copy_flags(t: &mut TypeTable) {
    loop {
        let mut changed = false;
        for i in 0..t.struct_defs.len() {
            if t.struct_defs[i].is_copy || t.struct_defs[i].is_drop { continue; }
            let all_fields_copy = t.struct_defs[i].fields.iter()
                .all(|(_, ty)| is_copy_ty(ty, t));
            if all_fields_copy {
                t.struct_defs[i].is_copy = true;
                changed = true;
            }
        }
        for i in 0..t.enum_defs.len() {
            if t.enum_defs[i].is_copy { continue; }
            // Only tagged enums reach here (plain enums were pre-marked).
            let all_payloads_copy = t.enum_defs[i].variant_payloads.iter()
                .all(|p| p.iter().all(|ty| is_copy_ty(ty, t)));
            if all_payloads_copy {
                t.enum_defs[i].is_copy = true;
                changed = true;
            }
        }
        if !changed { break; }
    }
}

/// True iff `ty` is Copy under the current `TypeTable`. Primitives and unit
/// are Copy; arrays inherit element copy-ness; structs and enums consult the
/// pre-computed flags. Sema is the source of truth — this mirror exists so
/// codegen can answer the question without re-importing sema's state.
fn is_copy_ty(ty: &Ty, t: &TypeTable) -> bool {
    match ty {
        Ty::Unit | Ty::Bool
        | Ty::I8  | Ty::I16 | Ty::I32 | Ty::I64
        | Ty::U8  | Ty::U16 | Ty::U32 | Ty::U64
        | Ty::Usize | Ty::Isize
        | Ty::F32 | Ty::F64
        | Ty::Str
        | Ty::Slice(_)
        | Ty::RawPtr(_)
        | Ty::FnPtr { .. } => true,
        Ty::Array(elem, _) => is_copy_ty(elem, t),
        Ty::Struct(id)     => t.struct_defs[id.0 as usize].is_copy,
        Ty::Enum(id)       => t.enum_defs[id.0 as usize].is_copy,
        // Phase 8 slice 8.STR.3: owned `string` is non-Copy + Drop.
        Ty::String         => false,
        Ty::Error          => false,
        // Slice 7GEN.4: generic type parameters never reach codegen
        // pre-monomorphization (slice 7GEN.5). Treat as non-Copy to
        // keep the helper total.
        Ty::Param(_)       => false,
    }
}

/// §2.9 borrow-ABI choice for a parameter. Returns true when the LLVM signature
/// should use `ptr` for this parameter and the callee binds it directly (no
/// alloca, no initial store, no drop registration), so that the callee's
/// writes propagate back to the caller's place (for `mut`) or so the caller
/// avoids an aggregate byte-copy (for shared).
///
/// Fires on:
/// - `mut x: T` where T is a non-Copy struct (slice 5BC.codegen) — exclusive
///   borrow ABI; writes propagate back, paired with LLVM `noalias` in 6BC.codegen.
/// - `x: T` where T is a non-Copy struct (slice 6BC.codegen) — shared borrow
///   pointer-pass paired with LLVM `readonly`. Eliminates the byte-copy at
///   call sites of large non-Copy aggregates.
///
/// `move x: T` stays value-passed (the value is the transfer; the caller's
/// drop flag flip suppresses the caller-side drop).
fn param_passes_by_ptr(ty: &Ty, move_: bool, _mutable: bool, t: &TypeTable) -> bool {
    if move_ { return false; }
    matches!(ty, Ty::Struct(_)) && !is_copy_ty(ty, t)
}

/// Slice 1A: full parameter attribute set (v0.0.2 LLVM information dividend).
///
/// Supersedes the original `param_attr_prefix` (which returned just
/// `noalias`/`readonly`). Composes every attribute the frontend has already
/// proven sound:
/// - **Pointer-passed (non-Copy struct, by §2.9 borrow ABI):**
///   - `noalias` (`mut`/`move`) or `readonly` (shared `self`/`x: T`) — the
///     borrow checker proves disjointness for the first, write-freeness for
///     the second.
///   - `nonnull` — C+ has no null in safe code (cross-ref
///     [feedback_cplus_no_null.md]); the address always comes from an
///     `alloca` or a previously-typed place.
///   - `noundef` — definite-assignment (sema slice 3J) guarantees the bytes
///     reachable through the pointer are fully defined.
///   - `dereferenceable(N)` + `align A` — `(N, A)` come from `static_layout`,
///     keyed on the slice-11.LAYOUT type table. Exact, not lower bounds.
/// - **Value-passed scalar (integers, bool, floats, raw `*T`, fn-pointer,
///   plain enum `iN'`):** `noundef` alone. Definite assignment justifies it
///   and LLVM's `-O2` uses `noundef` to fold redundant freeze/select
///   patterns.
/// - **Value-passed aggregate (`str`, `string`, `T[]`, Copy struct, tagged
///   enum):** no attributes. Padding bytes are LLVM-`poison` after
///   `insertvalue` construction, so `noundef` would be unsound at the
///   aggregate level.
///
/// Returned string has no trailing space; callers append a separator before
/// the SSA name (e.g. `"ptr {attrs} %{i}"`).
fn param_attrs(
    ty: &Ty,
    move_: bool,
    mutable: bool,
    pointer_passed: bool,
    types: &TypeTable,
) -> String {
    if pointer_passed {
        let mut s = String::new();
        s.push_str(if move_ || mutable { "noalias" } else { "readonly" });
        s.push_str(" nonnull noundef");
        if let Some((sz, al)) = static_layout(ty, types) {
            if sz > 0 {
                let _ = write!(s, " dereferenceable({sz})");
            }
            let _ = write!(s, " align {al}");
        }
        s
    } else if is_scalar_ty(ty, types) {
        "noundef".to_string()
    } else {
        String::new()
    }
}

/// Slice 1D (v0.0.2): decide whether the return value should use the LLVM
/// `sret` calling convention instead of a value-returned aggregate.
///
/// The plan describes `sret` for "non-Copy structs, slices, owned strings,
/// or any aggregate exceeding a target-specific size threshold (start with
/// > 16 bytes)". This implementation ships the **narrow** version: only
/// owned `string` (24 bytes, has Drop, the canonical case where copy
/// elision matters most). Generic non-Copy struct sret is deferred — it
/// has substantial test-surface impact and the wins for small aggregates
/// (≤ 16 bytes) are negligible at -O2 because LLVM already lowers them
/// through ABI-appropriate registers.
///
/// `extern fn` boundaries are never sret-modified — those keep the C ABI
/// the user declared. The callers of this predicate check `is_extern`
/// before calling.
/// Phase 5 Slice 5.D: classify a `pub extern fn` parameter or return type
/// against the platform C ABI. Today we target aarch64-apple-darwin —
/// the AArch64 Procedure Call Standard — and treat all aggregates as
/// integer-class (no HFA detection; the plan defers float-class to v2).
///
/// The rule is:
/// - Scalar (primitive, raw `*T`, fn-ptr, plain enum): pass unchanged.
/// - Aggregate ≤ 8 bytes: coerce to `i64`. Caller packs into a single GPR;
///   callee re-interprets the bits via an alloca'd buffer.
/// - Aggregate 9..=16 bytes: coerce to `[2 x i64]`. Two GPRs.
/// - Aggregate > 16 bytes: pass indirectly via a pointer. Return via
///   `sret(<ty>)` to a caller-allocated slot.
///
/// `Indirect` returns are handled in tandem with Slice 1D's `sret` path —
/// the function signature drops the value return and gains a `ptr sret(...)`
/// first parameter. Indirect *args* are bare `ptr` (no `byval` on
/// aarch64-darwin; the caller-callee contract owns the memory layout).
#[derive(Debug, Clone, PartialEq)]
enum CAbiClass {
    /// Pass as-is; no coercion needed.
    Direct,
    /// Coerce to the given LLVM type. The coerced size is ≥ the original
    /// size and is required for the alloca's storage so the coerced
    /// store doesn't overflow into adjacent memory.
    Coerce { llvm_ty: String, size: u64, align: u64 },
    /// Pass indirectly via a hidden pointer (param side) / `sret` (return).
    Indirect,
}

fn classify_c_abi(ty: &Ty, types: &TypeTable) -> CAbiClass {
    // Aggregates only need ABI coercion. Everything else is a single
    // register class and passes through cleanly.
    let is_aggregate = match ty {
        // Plain enums lower to i32 (scalar). Tagged enums are aggregates —
        // but sema's 5.C predicate rejects them at the `pub extern fn`
        // boundary, so we never see one here. Defensively handle anyway:
        // a tagged enum reaching codegen for an extern fn would still
        // need coercion (and a future spec for the layout); treat as
        // aggregate-by-size.
        Ty::Enum(id) => types.enum_defs[id.0 as usize].is_tagged,
        Ty::Struct(_)
        | Ty::Array(_, _)
        | Ty::Str
        | Ty::String
        | Ty::Slice(_) => true,
        _ => false,
    };
    if !is_aggregate {
        return CAbiClass::Direct;
    }
    let Some((size, _align)) = static_layout(ty, types) else {
        return CAbiClass::Direct;
    };
    if size == 0 {
        return CAbiClass::Direct;
    }
    if size <= 8 {
        CAbiClass::Coerce { llvm_ty: "i64".to_string(), size: 8, align: 8 }
    } else if size <= 16 {
        CAbiClass::Coerce { llvm_ty: "[2 x i64]".to_string(), size: 16, align: 8 }
    } else {
        CAbiClass::Indirect
    }
}

fn return_passes_by_sret(ty: &Ty) -> bool {
    matches!(ty, Ty::String)
}

/// Compute static (size, align) in bytes. Matches LLVM's default natural
/// layout for the 64-bit targets C+ supports (x86_64, arm64).
///
/// Returns `None` only for non-codegen types (`Ty::Error`, `Ty::Param`) —
/// those should never reach codegen anyway. Callers can `.unwrap_or` the
/// dereferenceable/align attrs away when the layout is unknown.
fn static_layout(ty: &Ty, types: &TypeTable) -> Option<(u64, u64)> {
    fn align_up(off: u64, al: u64) -> u64 { (off + al - 1) & !(al - 1) }
    match ty {
        Ty::I8 | Ty::U8 | Ty::Bool => Some((1, 1)),
        Ty::I16 | Ty::U16 => Some((2, 2)),
        Ty::I32 | Ty::U32 | Ty::F32 => Some((4, 4)),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => Some((8, 8)),
        Ty::RawPtr(_) | Ty::FnPtr { .. } => Some((8, 8)),
        // Fat pointers (Phase 8 / 11): { ptr, i64 } and { ptr, i64, i64 }.
        Ty::Str | Ty::Slice(_) => Some((16, 8)),
        Ty::String => Some((24, 8)),
        Ty::Unit => Some((0, 1)),
        Ty::Array(elem, n) => {
            let (esz, ea) = static_layout(elem, types)?;
            // No trailing pad on arrays — LLVM lays out [N x T] as N * size(T).
            Some((esz.saturating_mul(*n as u64), ea))
        }
        Ty::Struct(id) => {
            let info = &types.struct_defs[id.0 as usize];
            let mut off: u64 = 0;
            let mut max_al: u64 = 1;
            for (_, fty) in &info.fields {
                let (sz, al) = static_layout(fty, types)?;
                if al > max_al { max_al = al; }
                off = align_up(off, al);
                off = off.saturating_add(sz);
            }
            // Pad to struct alignment.
            let total = if max_al == 0 { off } else { align_up(off, max_al) };
            Some((total, max_al.max(1)))
        }
        Ty::Enum(id) => {
            let info = &types.enum_defs[id.0 as usize];
            if !info.is_tagged {
                // Plain enum: bare i32.
                Some((4, 4))
            } else {
                // Tagged enum: { i32 tag, [N x i64] payload } — align 8.
                let payload_bytes = info.payload_slots as u64 * 8;
                // tag is i32, padded up to 8 before the array.
                let size = 8u64.saturating_add(payload_bytes);
                Some((size, 8))
            }
        }
        Ty::Error | Ty::Param(_) => None,
    }
}

/// Slice 1C: scoped `!alias.scope` / `!noalias` metadata publication.
///
/// The borrow checker proves that for every pointer-passed `mut`/`move`
/// param (the ones that carried `noalias` from Slice 1A), no other live
/// pointer in the same function reaches the same memory. That fact is
/// already encoded as the `noalias` parameter attribute — but parameter
/// attrs degrade after inlining. Scoped alias metadata survives inlining
/// because the inliner imports the callee's scopes into the caller's
/// metadata universe.
///
/// This function does a single linear pass over the emitted function body
/// running a mini-dataflow:
///   1. Seed: each scoped param's SSA name (`%0`, `%1`, ...) → scope id.
///   2. On `getelementptr ..., ptr %src, ...` → propagate %src's scope
///      to the GEP's result.
///   3. On `load .., ptr %src` / `store .., ptr %src` → if %src has a
///      scope, append `, !alias.scope !L, !noalias !O` to the line.
///
/// Returns the rewritten body. `scope_idx_for_ssa` maps the param SSA name
/// to an index into `this_lists`/`other_lists`; both lists are indexed by
/// scope index so callers don't have to re-thread which scope is which.
fn annotate_alias_scope_metadata(
    body: &str,
    seed: &HashMap<String, usize>,
    this_lists: &[u32],
    other_lists: &[u32],
) -> String {
    let mut scope_map: HashMap<String, usize> = seed.clone();
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        out.push_str(&annotate_one_line(line, &mut scope_map, this_lists, other_lists));
        out.push('\n');
    }
    out
}

fn annotate_one_line(
    line: &str,
    scope_map: &mut HashMap<String, usize>,
    this_lists: &[u32],
    other_lists: &[u32],
) -> String {
    // Split on " = " to find an SSA def (load / GEP / etc.). Stores have
    // no LHS — handle separately below.
    let trimmed = line.trim_start();
    if let Some(eq_idx) = trimmed.find(" = ") {
        let lhs = &trimmed[..eq_idx];
        let rhs = &trimmed[eq_idx + 3..];
        if lhs.starts_with('%') {
            if rhs.starts_with("getelementptr ") {
                // GEP: find `, ptr %src` and propagate that scope to lhs.
                if let Some(src) = extract_ptr_operand(rhs) {
                    if let Some(&s) = scope_map.get(&src) {
                        scope_map.insert(lhs.to_string(), s);
                    }
                }
                return line.to_string();
            }
            if rhs.starts_with("load ") {
                if let Some(src) = extract_ptr_operand(rhs) {
                    if let Some(&s) = scope_map.get(&src) {
                        return format!(
                            "{line}, !alias.scope !{}, !noalias !{}",
                            this_lists[s], other_lists[s]
                        );
                    }
                }
                return line.to_string();
            }
            // bitcast / ptrtoint / inttoptr / select / phi etc. — could
            // propagate but the current language rarely generates these
            // for our scope-source ptrs. Leave conservative.
        }
    } else if trimmed.starts_with("store ") {
        if let Some(src) = extract_ptr_operand(trimmed) {
            if let Some(&s) = scope_map.get(&src) {
                return format!(
                    "{line}, !alias.scope !{}, !noalias !{}",
                    this_lists[s], other_lists[s]
                );
            }
        }
    }
    line.to_string()
}

/// Find the first `, ptr %X` operand in `s` and return `"%X"`. Used by
/// `annotate_one_line` to locate the address operand of load/store/GEP.
fn extract_ptr_operand(s: &str) -> Option<String> {
    let key = ", ptr ";
    let idx = s.find(key)?;
    let rest = &s[idx + key.len()..];
    if !rest.starts_with('%') { return None; }
    let end = rest.find(|c: char| c == ',' || c == ')' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// True iff `ty` lowers to a single LLVM scalar (one register class).
/// Aggregates (`str`, `string`, `T[]`, structs, tagged enums) are not
/// scalars even when small. Used to decide whether `noundef` is sound on a
/// value-passed parameter — aggregates carry `poison` padding so the
/// whole-value `noundef` would be unsound.
fn is_scalar_ty(ty: &Ty, types: &TypeTable) -> bool {
    match ty {
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
        | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
        | Ty::Isize | Ty::Usize
        | Ty::F32 | Ty::F64
        | Ty::Bool
        | Ty::RawPtr(_) | Ty::FnPtr { .. } => true,
        // Plain enums lower to `i32` (scalar); tagged enums to a struct.
        Ty::Enum(id) => !types.enum_defs[id.0 as usize].is_tagged,
        _ => false,
    }
}


/// Slice 6BC.opt move-scanning walker. Walks a function body collecting
/// the names of bindings used at any `move`-position argument or `move
/// self` receiver. Pure syntactic + callee-signature consultation —
/// doesn't reason about flow, so a binding moved inside an `if` arm
/// still counts as "moved somewhere." That's correct: if there's ANY
/// path that moves the binding, the drop flag must be runtime-checked.
fn scan_moves_in_block(
    b: &Block,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    for s in &b.stmts { scan_moves_in_stmt(s, sigs, types, set); }
    if let Some(t) = &b.tail { scan_moves_in_expr(t, sigs, types, set); }
}

fn scan_moves_in_stmt(
    s: &Stmt,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    match &s.kind {
        StmtKind::Let { init, .. } => {
            if let Some(e) = init { scan_moves_in_expr(e, sigs, types, set); }
        }
        StmtKind::Return(Some(e))
        | StmtKind::Expr(e)
        | StmtKind::Defer(e)
        | StmtKind::Assert(e) => scan_moves_in_expr(e, sigs, types, set),
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::While { cond, body } => {
            scan_moves_in_expr(cond, sigs, types, set);
            scan_moves_in_block(body, sigs, types, set);
        }
        StmtKind::For(fl) => match fl {
            ForLoop::CStyle { init, cond, update, body } => {
                if let Some(i) = init.as_deref() { scan_moves_in_stmt(i, sigs, types, set); }
                if let Some(c) = cond.as_ref() { scan_moves_in_expr(c, sigs, types, set); }
                for u in update { scan_moves_in_expr(u, sigs, types, set); }
                scan_moves_in_block(body, sigs, types, set);
            }
            ForLoop::Range { iter, body, .. } => {
                scan_moves_in_expr(iter, sigs, types, set);
                scan_moves_in_block(body, sigs, types, set);
            }
        }
        StmtKind::Loop(body) => scan_moves_in_block(body, sigs, types, set),
        // Lowered before codegen — should not appear here, but be safe.
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {}
    }
}

fn scan_moves_in_expr(
    e: &Expr,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    match &e.kind {
        ExprKind::Call { callee, args, .. } => {
            // If callee is a known free function, consult sigs for
            // its move flags. Each move-arg of a plain `Ident` name
            // adds that binding to the moved set.
            if let ExprKind::Ident(fn_name) = &callee.kind {
                if let Some(sig) = sigs.get(fn_name) {
                    for (arg, (_pty, move_flag, _mut_flag)) in args.iter().zip(sig.params.iter()) {
                        if *move_flag {
                            if let ExprKind::Ident(n) = &arg.kind { set.insert(n.clone()); }
                        }
                    }
                }
            }
            // Method calls: `recv.method(args)` — when `method` has
            // `move self`, the receiver binding is moved. We don't
            // try to resolve method sigs from sigs; method-move is
            // detected via the type table lookup that codegen does
            // at the call site. Simplest conservative rule: if the
            // callee is a `Field` expression on an Ident receiver,
            // look up the method's receiver kind via types.
            if let ExprKind::Field { receiver, name: m } = &callee.kind {
                if let ExprKind::Ident(recv) = &receiver.kind {
                    // Need receiver's struct type to look up method
                    // sig. Codegen doesn't track binding types
                    // statically here; we walk all struct defs for
                    // a method matching `m.name`. Conservative —
                    // multiple matches just mean the name is added
                    // to the moved set, which is safe (no false
                    // optimization).
                    for sdef in &types.struct_defs {
                        if let Some(mi) = sdef.methods.get(&m.name) {
                            if matches!(mi.receiver, Some(crate::ast::Receiver::Move)) {
                                set.insert(recv.clone());
                            }
                        }
                    }
                }
            }
            scan_moves_in_expr(callee, sigs, types, set);
            for a in args { scan_moves_in_expr(a, sigs, types, set); }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            scan_moves_in_expr(lhs, sigs, types, set);
            scan_moves_in_expr(rhs, sigs, types, set);
        }
        ExprKind::Unary { operand, .. } => scan_moves_in_expr(operand, sigs, types, set),
        ExprKind::Cast { expr, .. } => scan_moves_in_expr(expr, sigs, types, set),
        ExprKind::Field { receiver, .. } => scan_moves_in_expr(receiver, sigs, types, set),
        ExprKind::Index { receiver, index } => {
            scan_moves_in_expr(receiver, sigs, types, set);
            scan_moves_in_expr(index, sigs, types, set);
        }
        ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
            for f in fields { scan_moves_in_expr(&f.value, sigs, types, set); }
        }
        ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
            for el in elements { scan_moves_in_expr(el, sigs, types, set); }
        }
        ExprKind::Block(b) => scan_moves_in_block(b, sigs, types, set),
        ExprKind::Unsafe(b) => scan_moves_in_block(b, sigs, types, set),
        ExprKind::If { cond, then, else_branch } => {
            scan_moves_in_expr(cond, sigs, types, set);
            scan_moves_in_block(then, sigs, types, set);
            if let Some(eb) = else_branch.as_deref() { scan_moves_in_expr(eb, sigs, types, set); }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start { scan_moves_in_expr(s, sigs, types, set); }
            if let Some(e) = end   { scan_moves_in_expr(e, sigs, types, set); }
        }
        ExprKind::Assign { target, value, .. } => {
            scan_moves_in_expr(target, sigs, types, set);
            scan_moves_in_expr(value, sigs, types, set);
        }
        ExprKind::Match { scrutinee, arms } => {
            scan_moves_in_expr(scrutinee, sigs, types, set);
            for a in arms { scan_moves_in_expr(&a.body, sigs, types, set); }
        }
        _ => {}
    }
}

fn write_struct_decls(out: &mut String, types: &TypeTable, _p: &Program) {
    let any_struct = !types.struct_defs.is_empty();
    let any_tagged_enum = types.enum_defs.iter().any(|e| e.is_tagged);
    if !any_struct && !any_tagged_enum { return; }
    // Struct named-type declarations (Phase 2B).
    for s in &types.struct_defs {
        let inner: Vec<String> = s.fields.iter().map(|(_, t)| llvm_ty(t, types)).collect();
        writeln!(out, "%{} = type {{ {} }}", s.name, inner.join(", ")).unwrap();
    }
    // Tagged-enum named-type declarations (Phase 3I). Layout is
    // `{ i32 tag, [N x i64] payload }` where N is the max payload-slot
    // count across variants. Each payload value occupies one i64-aligned
    // slot — Phase 3 simplification that wastes some bytes but guarantees
    // 8-byte alignment everywhere.
    for (i, info) in types.enum_defs.iter().enumerate() {
        if !info.is_tagged { continue; }
        let id = EnumId(i as u32);
        let name = enum_struct_name(id, types);
        writeln!(
            out,
            "%{} = type {{ i32, [{} x i64] }}",
            name, info.payload_slots
        ).unwrap();
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
        // Slice 6BC.5: region annotations are transparent at codegen
        // time. `borrow A T` lowers exactly like T — the region is
        // borrow-checker metadata, not a runtime construct.
        TypeKind::Borrowed { inner, .. } => return ty_from(inner, types),
        // Slice 7GEN.5c: monomorphize rewrites every `TypeKind::Generic`
        // to a concrete `TypeKind::Path(mangled_name)` before codegen.
        // If we reach here it means the rewrite missed a site.
        TypeKind::Generic { .. } => panic!("codegen reached TypeKind::Generic — monomorphize did not rewrite this site"),
        // Slice 10.FFI.1: raw pointer lowers to LLVM `ptr` regardless
        // of pointee. Pointee info is sema-level only.
        TypeKind::RawPtr(inner) => {
            let inner_ty = ty_from(inner, types);
            return Ty::RawPtr(Box::new(inner_ty));
        }
        // Slice 11.FN_PTR: fn-ptr lowers to LLVM `ptr` regardless of signature.
        TypeKind::FnPtr { params, return_type } => {
            let resolved_params: Vec<Ty> = params.iter().map(|p| ty_from(p, types)).collect();
            let resolved_ret = match return_type {
                Some(rt) => ty_from(rt, types),
                None => Ty::Unit,
            };
            return Ty::FnPtr { params: resolved_params, return_type: Box::new(resolved_ret) };
        }
        // Phase 11 polish (2026-05-14): slice type.
        TypeKind::Slice(inner) => {
            let inner_ty = ty_from(inner, types);
            return Ty::Slice(Box::new(inner_ty));
        }
    };
    match name.as_str() {
        "i8" => Ty::I8, "i16" => Ty::I16, "i32" => Ty::I32, "i64" => Ty::I64,
        "u8" => Ty::U8, "u16" => Ty::U16, "u32" => Ty::U32, "u64" => Ty::U64,
        "isize" => Ty::Isize, "usize" => Ty::Usize,
        "f32" => Ty::F32, "f64" => Ty::F64,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "string" => Ty::String,
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
        Ty::I32 | Ty::U32 => "i32".to_string(),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => "i64".to_string(),
        Ty::F32 => "float".to_string(),
        Ty::F64 => "double".to_string(),
        Ty::Bool => "i1".to_string(),
        Ty::Unit => "void".to_string(),
        Ty::Struct(id) => format!("%{}", types.struct_defs[id.0 as usize].name),
        Ty::Enum(id) => {
            let info = &types.enum_defs[id.0 as usize];
            // Plain enums (no variant has a payload) stay bare `i32` —
            // Phase-2A fast path. Tagged enums use a named struct type
            // emitted in the preamble: `%E = type { i32, [N x i64] }`.
            if info.is_tagged {
                format!("%{}", enum_struct_name(*id, types))
            } else {
                "i32".to_string()
            }
        }
        Ty::Array(elem, n) => format!("[{n} x {}]", llvm_ty(elem, types)),
        // Slice 10.FFI.1: raw pointers lower to LLVM `ptr` (opaque,
        // 8 bytes on 64-bit). Pointee info is sema-only.
        Ty::RawPtr(_) => "ptr".to_string(),
        // Slice 11.FN_PTR: fn pointers also lower to LLVM `ptr`. Sema
        // carries the param/return type info; codegen indirect-calls
        // know the call signature from the FnPtr Ty, not from the LLVM IR.
        Ty::FnPtr { .. } => "ptr".to_string(),
        // Phase 8 slice 8.STR.1: `str` is a fat pointer { ptr, len }.
        // 16 bytes on 64-bit platforms; passed by value.
        Ty::Str => "{ ptr, i64 }".to_string(),
        // Phase 8 slice 8.STR.3: owned `string` is { ptr, len, cap } —
        // 24 bytes on 64-bit. Passed by value; the ptr is the only field
        // codegen ever sees per-call, but the cap field is what `drop`
        // reads when freeing the buffer.
        Ty::String => "{ ptr, i64, i64 }".to_string(),
        // Phase 11 polish (2026-05-14): slice type `T[]` is a fat
        // pointer { ptr, len } — same shape as `str`. The element type
        // `T` is sema-only; LLVM sees just the pair.
        Ty::Slice(_) => "{ ptr, i64 }".to_string(),
        Ty::Error => panic!("codegen reached Ty::Error — sema should have rejected the program"),
        // Slice 7GEN.4: `Ty::Param` must not reach codegen. Until
        // monomorphization (slice 7GEN.5) lowers generic items, the
        // parser+sema admit generic surface but no generic item is
        // codegen-emitted — sema's reachability prevents calling a
        // generic from a concrete-typed context (its return type
        // would carry `Ty::Param`).
        Ty::Param(_) => panic!("codegen reached Ty::Param — generics not yet monomorphized (slice 7GEN.5)"),
    }
}

/// LLVM type name for a tagged enum. We don't track the enum's source name
/// in `EnumInfo`, so synthesize a stable identifier from the EnumId. The
/// preamble emits `%enum.0 = type { ... }` etc.
fn enum_struct_name(id: EnumId, _types: &TypeTable) -> String {
    format!("enum.{}", id.0)
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
    // Phase 8 slice 8.STR.2: format string for `println(str)`. Uses
    // `%.*s` so the pointer + length are passed verbatim (no NUL
    // assumption — strings may legitimately contain embedded NULs).
    out.push_str("@.fmt_str_nl = private unnamed_addr constant [6 x i8] c\"%.*s\\0A\\00\", align 1\n");
    out.push_str("\n");
    out.push_str("declare i32 @printf(ptr noundef, ...)\n");
    // Phase 8 slice 8.STR.3: byte-level string comparison.
    out.push_str("declare i32 @memcmp(ptr, ptr, i64)\n");
    // Phase 8 slice 8.STR.3: owned `string` runtime. malloc + free for
    // construction + Drop; memcpy for clone. realloc reserved for future
    // mutation API (not used in v1).
    out.push_str("declare ptr @malloc(i64)\n");
    out.push_str("declare void @free(ptr)\n");
    out.push_str("declare ptr @memcpy(ptr, ptr, i64)\n");
    // Phase 8 slice 8.STR.6: snprintf for blessed `to_string()` on
    // numeric primitives. Returns the number of bytes that *would have*
    // been written (excluding NUL); we use that as the resulting
    // `string.len`. The 32-byte buffer comfortably covers every 64-bit
    // integer decimal plus a sign + the `%g` float format.
    out.push_str("declare i32 @snprintf(ptr, i64, ptr, ...)\n");
    // Format strings the to_string intrinsics use.
    out.push_str("@.fmt_i64    = private unnamed_addr constant [5 x i8] c\"%lld\\00\", align 1\n");
    out.push_str("@.fmt_u64    = private unnamed_addr constant [5 x i8] c\"%llu\\00\", align 1\n");
    out.push_str("@.fmt_f64    = private unnamed_addr constant [3 x i8] c\"%g\\00\", align 1\n");
    out.push_str("@.bool_true  = private unnamed_addr constant [4 x i8] c\"true\", align 1\n");
    out.push_str("@.bool_false = private unnamed_addr constant [5 x i8] c\"false\", align 1\n");
    // Trap intrinsic — used for both overflow (debug) and divide-by-zero (always).
    out.push_str("declare void @llvm.trap()\n");
    // Slice 1B (v0.0.2): assume intrinsic — used to publish facts the
    // frontend has proven (bounds-check success, slice-length non-negative)
    // so `-O2`'s ConstraintElimination/InstCombine can elide downstream
    // redundant checks. At -O0 this is a no-op call.
    out.push_str("declare void @llvm.assume(i1 noundef)\n");
    // Phase 3A (v0.0.2): byte-swap intrinsics. Used by `bswap16/32/64`
    // and `htons`/`htonl`/`ntohs`/`ntohl` aliases. All declared so DCE
    // can strip the unused widths.
    out.push_str("declare i16 @llvm.bswap.i16(i16)\n");
    out.push_str("declare i32 @llvm.bswap.i32(i32)\n");
    out.push_str("declare i64 @llvm.bswap.i64(i64)\n");
    // Phase 5 Slice 5.D: memset intrinsic for zero-initializing C-ABI
    // return coercion slots. Used so tail bytes (beyond the original
    // struct's footprint) read as 0 instead of poison when packed into
    // the coerced integer-class return register.
    out.push_str("declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)\n");
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

/// Phase 8 slice 8.STR.1: walk the program, find every `ExprKind::StrLit`,
/// dedupe by content, and emit one `@.str.N = private unnamed_addr constant`
/// per unique literal. Returns a map from literal payload → (symbol, len).
/// `len` is the visible byte length, NOT counting the NUL terminator.
/// The NUL is appended in the IR so `to_cstring()` (slice 8.STR.4) can
/// hand the same buffer to C with no copy.
fn collect_and_emit_str_lits(out: &mut String, program: &Program) -> StrLitTable {
    let mut table: StrLitTable = HashMap::new();
    let mut next_id: u32 = 0;
    fn emit_str_global(s: &str, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        if table.contains_key(s) { return; }
        let symbol = format!("@.str.{}", *next_id);
        *next_id += 1;
        let len = s.len();
        let total = len + 1;
        let mut escaped = String::new();
        for byte in s.bytes() {
            if byte == b'"' || byte == b'\\' || !(0x20..0x7F).contains(&byte) {
                escaped.push_str(&format!("\\{byte:02X}"));
            } else {
                escaped.push(byte as char);
            }
        }
        escaped.push_str("\\00");
        out.push_str(&format!(
            "{symbol} = private unnamed_addr constant [{total} x i8] c\"{escaped}\", align 1\n"
        ));
        table.insert(s.to_string(), (symbol, len));
    }
    fn walk_expr(e: &Expr, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        match &e.kind {
            ExprKind::StrLit(s) => {
                emit_str_global(s, table, next_id, out);
            }
            ExprKind::InterpStr { parts } => {
                // Phase 8 slice 8.STR.B: each Lit segment gets the same
                // @.str.N treatment as a plain StrLit so codegen at the
                // use site can reuse the existing fat-pointer machinery.
                for p in parts {
                    match p {
                        crate::ast::InterpStrPart::Lit(s) => emit_str_global(s, table, next_id, out),
                        crate::ast::InterpStrPart::Expr(e) => walk_expr(e, table, next_id, out),
                    }
                }
            }
            ExprKind::Block(b) => walk_block(b, table, next_id, out),
            ExprKind::Unsafe(b) => walk_block(b, table, next_id, out),
            ExprKind::If { cond, then, else_branch } => {
                walk_expr(cond, table, next_id, out);
                walk_block(then, table, next_id, out);
                if let Some(eb) = else_branch { walk_expr(eb, table, next_id, out); }
            }
            ExprKind::Call { callee, args, .. } => {
                walk_expr(callee, table, next_id, out);
                for a in args { walk_expr(a, table, next_id, out); }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                walk_expr(lhs, table, next_id, out);
                walk_expr(rhs, table, next_id, out);
            }
            ExprKind::Unary { operand, .. } => walk_expr(operand, table, next_id, out),
            ExprKind::Field { receiver, .. } => walk_expr(receiver, table, next_id, out),
            ExprKind::Index { receiver, index } => {
                walk_expr(receiver, table, next_id, out);
                walk_expr(index, table, next_id, out);
            }
            ExprKind::Assign { target, value, .. } => {
                walk_expr(target, table, next_id, out);
                walk_expr(value, table, next_id, out);
            }
            ExprKind::Cast { expr: inner, .. } => walk_expr(inner, table, next_id, out),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { walk_expr(s, table, next_id, out); }
                if let Some(e) = end { walk_expr(e, table, next_id, out); }
            }
            ExprKind::Match { scrutinee, arms } => {
                walk_expr(scrutinee, table, next_id, out);
                for a in arms { walk_expr(&a.body, table, next_id, out); }
            }
            ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields { walk_expr(&f.value, table, next_id, out); }
            }
            ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
                for e in elements { walk_expr(e, table, next_id, out); }
            }
            _ => {}
        }
    }
    fn walk_block(b: &Block, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        for s in &b.stmts { walk_stmt(s, table, next_id, out); }
        if let Some(t) = &b.tail { walk_expr(t, table, next_id, out); }
    }
    fn walk_stmt(s: &Stmt, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        match &s.kind {
            StmtKind::Let { init, .. } => {
                if let Some(e) = init { walk_expr(e, table, next_id, out); }
            }
            StmtKind::Expr(e) | StmtKind::Assert(e) => walk_expr(e, table, next_id, out),
            StmtKind::Return(e) => { if let Some(e) = e { walk_expr(e, table, next_id, out); } }
            StmtKind::While { cond, body } => {
                walk_expr(cond, table, next_id, out);
                walk_block(body, table, next_id, out);
            }
            StmtKind::For(forloop) => match forloop {
                crate::ast::ForLoop::Range { iter, body, .. } => {
                    walk_expr(iter, table, next_id, out);
                    walk_block(body, table, next_id, out);
                }
                crate::ast::ForLoop::CStyle { init, cond, update, body } => {
                    if let Some(s) = init { walk_stmt(s, table, next_id, out); }
                    if let Some(c) = cond { walk_expr(c, table, next_id, out); }
                    for u in update { walk_expr(u, table, next_id, out); }
                    walk_block(body, table, next_id, out);
                }
            }
            StmtKind::Defer(e) => walk_expr(e, table, next_id, out),
            // Phase 3B follow-up (2026-05-15): plain `loop { ... }` blocks
            // were silently skipped by the str-literal pre-pass, so any
            // literal inside a `loop` body tripped a codegen `expect`. Walk
            // the body the same way as `while` / `for`.
            StmtKind::Loop(body) => walk_block(body, table, next_id, out),
            _ => {}
        }
    }
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) if f.generic_params.is_empty() => {
                walk_block(&f.body, &mut table, &mut next_id, out);
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if m.generic_params.is_empty() {
                        walk_block(&m.body, &mut table, &mut next_id, out);
                    }
                }
            }
            _ => {}
        }
    }
    out.push_str("\n");
    table
}

/// Emit a `private unnamed_addr constant` LLVM string literal with a NUL
/// terminator. Used by both `println` (the existing `@.fmt_int_nl`) and the
/// slice 5ATTR.4 test driver. Returns the byte length including the null
/// terminator (the `[N x i8]` length in the declaration).
fn emit_cstr(out: &mut String, name: &str, s: &str) -> usize {
    let mut escaped = String::new();
    let mut len: usize = 0;
    for byte in s.bytes() {
        if byte == b'"' || byte == b'\\' || !(0x20..0x7F).contains(&byte) {
            escaped.push_str(&format!("\\{byte:02X}"));
        } else {
            escaped.push(byte as char);
        }
        len += 1;
    }
    escaped.push_str("\\00");
    len += 1;
    out.push_str(&format!(
        "@{name} = private unnamed_addr constant [{len} x i8] c\"{escaped}\", align 1\n"
    ));
    len
}

/// Slice 5ATTR.4 — emit the synthesized test-driver `main`. Called only when
/// `generate_test_binary` is the entry point; the user's own `main` is
/// suppressed in `generate_inner` so this one is the linker's choice.
///
/// IR shape (per test, in source order):
///   - clear `@cpc_test_failed`
///   - call the test fn
///   - for `fn() -> i32` tests, fold the return into the failure check
///   - print one pass/fail line (human or JSON per `json`)
///   - bump a local pass/fail counter
/// Final block prints the summary and returns the fail count as the process
/// exit status (so `cpc test` can short-circuit on any failure).
fn emit_test_driver_main(out: &mut String, tests: &[crate::attrs::TestFn], json: bool) {
    out.push('\n');
    // Format strings. Use distinct names per mode to keep the IR readable.
    let (pass_fmt, fail_fmt, summary_fmt) = if json {
        (
            "{\"name\":\"%s\",\"result\":\"pass\"}\n",
            "{\"name\":\"%s\",\"result\":\"fail\"}\n",
            "{\"passed\":%d,\"failed\":%d}\n",
        )
    } else {
        (
            "test %s ... ok\n",
            "test %s ... FAILED\n",
            "\ntest result: %d passed; %d failed\n",
        )
    };
    let pass_fmt_len = emit_cstr(out, ".fmt_test_pass", pass_fmt);
    let fail_fmt_len = emit_cstr(out, ".fmt_test_fail", fail_fmt);
    let summary_fmt_len = emit_cstr(out, ".fmt_test_summary", summary_fmt);
    // Per-test display-name constant. Numbered by source order to match the
    // tests vec; codegen never reads it as a Rust value, only emits a printf
    // arg, so the index is the only required key.
    let mut name_lens: Vec<usize> = Vec::with_capacity(tests.len());
    for (i, t) in tests.iter().enumerate() {
        let n = emit_cstr(out, &format!(".tn_{i}"), &t.display_name);
        name_lens.push(n);
    }
    out.push('\n');
    out.push_str("define i32 @main() {\n");
    out.push_str("entry:\n");
    out.push_str("  %passed = alloca i32\n");
    out.push_str("  %failed = alloca i32\n");
    out.push_str("  store i32 0, ptr %passed\n");
    out.push_str("  store i32 0, ptr %failed\n");
    for (i, t) in tests.iter().enumerate() {
        let pass_lbl = format!("p{i}");
        let fail_lbl = format!("fl{i}");
        let next_lbl = format!("n{i}");
        out.push_str(&format!("\n  ; test {} {}\n", i, t.display_name));
        out.push_str("  store i32 0, ptr @cpc_test_failed\n");
        if t.returns_i32 {
            out.push_str(&format!("  %ret{i} = call i32 @{}()\n", t.qualified_name));
            out.push_str(&format!("  %flag{i} = load i32, ptr @cpc_test_failed\n"));
            out.push_str(&format!("  %combined{i} = or i32 %ret{i}, %flag{i}\n"));
            out.push_str(&format!("  %ok{i} = icmp eq i32 %combined{i}, 0\n"));
        } else {
            out.push_str(&format!("  call void @{}()\n", t.qualified_name));
            out.push_str(&format!("  %flag{i} = load i32, ptr @cpc_test_failed\n"));
            out.push_str(&format!("  %ok{i} = icmp eq i32 %flag{i}, 0\n"));
        }
        out.push_str(&format!("  br i1 %ok{i}, label %{pass_lbl}, label %{fail_lbl}\n"));
        out.push('\n');
        out.push_str(&format!("{pass_lbl}:\n"));
        out.push_str(&format!(
            "  %pcall{i} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_pass, ptr noundef @.tn_{i})\n"
        ));
        out.push_str(&format!("  %pold{i} = load i32, ptr %passed\n"));
        out.push_str(&format!("  %pnew{i} = add i32 %pold{i}, 1\n"));
        out.push_str(&format!("  store i32 %pnew{i}, ptr %passed\n"));
        out.push_str(&format!("  br label %{next_lbl}\n"));
        out.push('\n');
        out.push_str(&format!("{fail_lbl}:\n"));
        out.push_str(&format!(
            "  %fcall{i} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_fail, ptr noundef @.tn_{i})\n"
        ));
        out.push_str(&format!("  %fold{i} = load i32, ptr %failed\n"));
        out.push_str(&format!("  %fnew{i} = add i32 %fold{i}, 1\n"));
        out.push_str(&format!("  store i32 %fnew{i}, ptr %failed\n"));
        out.push_str(&format!("  br label %{next_lbl}\n"));
        out.push('\n');
        out.push_str(&format!("{next_lbl}:\n"));
    }
    out.push_str("\n  ; summary\n");
    out.push_str("  %final_passed = load i32, ptr %passed\n");
    out.push_str("  %final_failed = load i32, ptr %failed\n");
    out.push_str(
        "  %scall = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_summary, i32 %final_passed, i32 %final_failed)\n",
    );
    out.push_str("  ret i32 %final_failed\n");
    out.push_str("}\n");
    // Silence unused warnings if `tests` is empty — the length values are
    // useful when debugging IR layout but otherwise discarded.
    let _ = (pass_fmt_len, fail_fmt_len, summary_fmt_len, name_lens);
}

fn gen_function(
    out: &mut String,
    f: &Function,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    emitted_extern_symbols: &mut std::collections::HashSet<String>,
    md: &ModuleMetadata,
    is_lib: bool,
) {
    // Builtin name: codegen never emits a definition for it; clang links printf.
    if f.name.name == "println" {
        return;
    }

    let sig = sigs.get(&f.name.name).expect("sig was collected");
    let return_ty = sig.return_type.clone();

    // Slice 10.FFI.1: extern fn declarations emit `declare TYPE @name(...)`
    // and no body. LLVM matches against the platform C ABI at link time.
    // Param attributes (noalias/readonly) are skipped — they're only sound
    // on C+ fns whose call sites the borrow checker has analyzed.
    //
    // Phase 5 Slice 5.C: `pub extern fn name(...) { body }` is the export
    // form (definition). Parser sets `is_pub` only on that shape. Fall
    // through to normal `define` emission for those — they're regular
    // function bodies that happen to commit to a stable C-callable
    // name. Slice 5.D will adjust the LLVM signature to match the
    // platform C ABI for value-passed aggregates.
    if f.is_extern && !f.is_pub {
        // Slice 10.FFI.4: some C symbols are already declared in the
        // codegen preamble (printf for `println`, memcmp for `str ==`).
        // Re-declaring them would clash at link time; skip if the
        // user's extern fn matches a preamble-emitted name. The sema
        // signature still flows through the call-site routing.
        // Phase 11 / ObjC interop: dedup also against the resolved
        // link_name (e.g. a user could declare `#[link_name = "printf"]
        // extern fn my_printf(...)` — same symbol, would clash).
        let resolved_symbol = sig.link_name.as_deref().unwrap_or(&f.name.name);
        // Preamble-declared libc symbols. Skip re-emission if a user's
        // `extern fn malloc/free/memcpy` shadows; we trust the user's
        // signature matches the preamble's (i64 args / ptr returns). The
        // preamble shapes are the ones the `string` runtime emits calls
        // against — a divergent user signature would mis-link anyway.
        if matches!(resolved_symbol, "printf" | "memcmp" | "malloc" | "free" | "memcpy") {
            return;
        }
        // Phase 11 / ObjC interop: multiple `extern fn` declarations
        // may share a single linker symbol via `#[link_name = "..."]`.
        // The codegen-side dedup prevents emitting the same `declare`
        // twice (which LLVM rejects).
        if emitted_extern_symbols.contains(resolved_symbol) {
            return;
        }
        emitted_extern_symbols.insert(resolved_symbol.to_string());
        write!(out, "declare {} @{}(", llvm_ty(&return_ty, types), resolved_symbol).unwrap();
        for (i, (_param, (pty, _move_flag, _mut_flag))) in f.params.iter().zip(sig.params.iter()).enumerate() {
            if i > 0 { out.push_str(", "); }
            out.push_str(&llvm_ty(pty, types));
        }
        // Slice 10.FFI.4: trailing `, ...` for variadic extern fns.
        if f.is_variadic {
            if !f.params.is_empty() { out.push_str(", "); }
            out.push_str("...");
        }
        out.push_str(")\n");
        return;
    }

    // Function header. Non-Copy `mut x: T` params lower to a `ptr noalias`
    // parameter (§2.9 exclusive borrow ABI, with 6BC.codegen's `noalias`
    // attribute proving uniqueness to LLVM). Non-Copy shared `x: T` params
    // lower to `ptr readonly` — pointer-pass avoids the byte-copy and the
    // callee provably can't write through the pointer (sema rejects).
    // Everything else stays value-passed.
    //
    // Slice 1D: when the return type triggers `return_passes_by_sret`
    // (currently: owned `string` only), rewrite the signature so the
    // result lands at a caller-provided slot. The sret pointer is the
    // first param (%0), and the user-declared params shift by one. The
    // function returns `void`.
    // Phase 5 Slice 5.D: classify return + params against the C ABI when
    // this is a `pub extern fn` export. Indirect returns flow through the
    // existing Slice 1D `sret` path; ≤16-byte aggregate returns coerce
    // to integer-class types; scalar returns pass through.
    let is_c_export = f.is_extern && f.is_pub;
    let ret_abi = if is_c_export { classify_c_abi(&return_ty, types) } else { CAbiClass::Direct };
    let param_abis: Vec<CAbiClass> = if is_c_export {
        sig.params.iter().map(|(pty, _, _)| classify_c_abi(pty, types)).collect()
    } else {
        vec![CAbiClass::Direct; sig.params.len()]
    };

    // Existing Slice 1D path: owned `string` returns use sret. 5.D adds
    // sret for any C-export with an Indirect-class return (>16 bytes).
    let uses_sret = return_passes_by_sret(&return_ty)
        || matches!(ret_abi, CAbiClass::Indirect);
    let coerce_ret_ty: Option<String> = if let CAbiClass::Coerce { llvm_ty, .. } = &ret_abi {
        Some(llvm_ty.clone())
    } else {
        None
    };
    let sig_return_ty: String = if uses_sret {
        "void".to_string()
    } else if let Some(t) = &coerce_ret_ty {
        t.clone()
    } else {
        llvm_ty(&return_ty, types)
    };
    let ret_ty_str = llvm_ty(&return_ty, types); // raw underlying type (e.g. for sret(...))
    let sret_param_offset: u32 = if uses_sret { 1 } else { 0 };
    // Phase 5 Slice 5.B: in library builds, non-`pub` items get
    // `internal` linkage so LTO can strip them out of the final
    // `.dylib` / `.a`. Executable builds keep external linkage (matches
    // pre-5.B behavior; the existing test substring assertions pin that).
    // `main` is the linker entry point and always external. `pub` items
    // form the public ABI and stay external in lib mode.
    let linkage = if !is_lib || f.name.name == "main" || f.is_pub { "" } else { "internal " };
    write!(out, "define {}{} @{}(", linkage, sig_return_ty, f.name.name).unwrap();
    if uses_sret {
        // sret slot: caller-allocated, callee-writable, exact size + align.
        let (sz, al) = static_layout(&return_ty, types)
            .expect("sret return type must have a known layout");
        write!(
            out,
            "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {} %0",
            ret_ty_str, sz, al
        ).unwrap();
        if !f.params.is_empty() { out.push_str(", "); }
    }
    for (i, (_param, (pty, move_flag, mut_flag))) in f.params.iter().zip(sig.params.iter()).enumerate() {
        if i > 0 { out.push_str(", "); }
        let llvm_idx = i as u32 + sret_param_offset;
        // Phase 5 Slice 5.D: when this fn is a C-ABI export, override the
        // LLVM signature for value-passed aggregates per the platform PCS:
        //   ≤8 bytes → i64
        //   9..16   → [2 x i64]
        //   >16     → ptr (caller-allocated; no `byval` on aarch64-darwin)
        //
        // Pointer-passed `mut`/`move` params are not C-ABI exportable
        // anyway (sema 5.C rejects non-Copy aggregates that aren't
        // `#[repr(C)]` and rejects Drop entirely), so the `param_passes_by_ptr`
        // path doesn't co-occur with non-Direct ABI classes here.
        match &param_abis[i] {
            CAbiClass::Coerce { llvm_ty, .. } => {
                write!(out, "{} %{}", llvm_ty, llvm_idx).unwrap();
            }
            CAbiClass::Indirect => {
                // Caller's by-value slot. No `byval` on aarch64-darwin;
                // the callee may freely mutate (it's a copy for this call).
                write!(out, "ptr %{}", llvm_idx).unwrap();
            }
            CAbiClass::Direct => {
                let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
                let attrs = param_attrs(pty, *move_flag, *mut_flag, by_ptr, types);
                let base_ty = if by_ptr { "ptr".to_string() } else { llvm_ty(pty, types) };
                if attrs.is_empty() {
                    write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
                } else {
                    write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
                }
            }
        }
    }
    out.push_str(") {\n");
    out.push_str("entry:\n");

    // Build the function body
    let mut state = FnState::new(return_ty.clone(), sigs, types, str_lits, mode, test_mode, md);
    state.collect_moved_bindings(&f.body);
    // Slice 1E: record this fn's parameter types so the Return-statement
    // predicate can check musttail signature equality against the callee.
    state.enclosing_params = sig.params.iter().map(|(t, _, _)| t.clone()).collect();
    state.tail_call_eligible = true;
    // Slice 1D: if this fn uses sret, remember the slot's SSA name (%0) so
    // StmtKind::Return can store-into it before `ret void`.
    if uses_sret {
        state.sret_slot = Some("%0".to_string());
    }
    // Phase 5 Slice 5.D: coerced returns flow through StmtKind::Return.
    state.coerce_ret = coerce_ret_ty.clone();

    // Bind params. Pointer-passed params (`mut x: T` non-Copy) bind directly
    // to the SSA argument — no alloca, no initial store — exactly like
    // receivers. Value-passed params copy into an alloca; `move`-marked Drop
    // params register a scope-exit drop. Non-`move` value-passed params are
    // left unregistered to avoid double-free of the caller's value.
    //
    // Slice 1D: when sret is in effect, the user-declared params are at
    // SSA indices 1..N instead of 0..N-1 — the sret slot occupies %0.
    //
    // Phase 5 Slice 5.D: `pub extern fn` exports apply C-ABI param
    // coercions per `param_abis`:
    //   - Coerce: alloca a slot sized for the coerced type (≥ struct size,
    //     so the coerced store doesn't overflow), store the coerced SSA
    //     value into it, bind as the original struct type — subsequent
    //     field GEPs use the original-type's offsets and read valid bytes.
    //   - Indirect: the SSA arg IS a pointer to the C caller's slot.
    //     Bind directly; gen_field GEPs off it like any other place.
    for (i, (param, (pty, move_flag, mut_flag))) in f.params.iter().zip(sig.params.iter()).enumerate() {
        let llvm_idx = i as u32 + sret_param_offset;
        // C-ABI coerced param: alloca with coerced size, store the coerced
        // value, bind as original struct type. The alloca uses the
        // coerced LLVM type because it dominates the size + align needed.
        if let CAbiClass::Coerce { llvm_ty: clty, align, .. } = &param_abis[i] {
            let slot = state.alloca_named_raw(&param.name.name, clty, *align);
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                clty, llvm_idx, slot
            ));
            state.bind(&param.name.name, slot, pty.clone());
            continue;
        }
        // C-ABI indirect param: the SSA arg is a pointer to the caller's
        // by-value slot. Bind directly; no alloca, no initial copy.
        if matches!(param_abis[i], CAbiClass::Indirect) {
            state.bind(&param.name.name, format!("%{llvm_idx}"), pty.clone());
            continue;
        }
        if param_passes_by_ptr(pty, *move_flag, *mut_flag, types) {
            state.bind(&param.name.name, format!("%{llvm_idx}"), pty.clone());
            continue;
        }
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types), llvm_idx, slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
        if *move_flag {
            if let Ty::Struct(id) = pty {
                if types.struct_defs[id.0 as usize].is_drop {
                    state.register_drop(&param.name.name, &slot, *id);
                }
            }
        }
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

    // Slice 1C: scoped alias metadata for noalias-shaped params. Run the
    // dataflow over `state.body` (allocas in `state.allocas` never touch
    // these ptrs — they're fresh slots, not derived from a param).
    let noalias_params: Vec<u32> = f.params.iter().zip(sig.params.iter()).enumerate()
        .filter_map(|(i, (_, (pty, mv, mu)))|
            (param_passes_by_ptr(pty, *mv, *mu, types) && (*mv || *mu)).then_some(i as u32)
        ).collect();
    if noalias_params.len() >= 2 {
        let domain = md.register_alias_domain(&f.name.name);
        let scopes: Vec<u32> = noalias_params.iter().enumerate()
            .map(|(i, _)| md.register_alias_scope(domain, &format!("p{i}"))).collect();
        let this_lists: Vec<u32> = scopes.iter()
            .map(|&s| md.register_alias_scope_list(&[s])).collect();
        let other_lists: Vec<u32> = (0..scopes.len()).map(|i| {
            let others: Vec<u32> = scopes.iter().enumerate()
                .filter(|(j, _)| *j != i).map(|(_, &s)| s).collect();
            md.register_alias_scope_list(&others)
        }).collect();
        let mut seed: HashMap<String, usize> = HashMap::new();
        for (idx, &param_ssa) in noalias_params.iter().enumerate() {
            seed.insert(format!("%{param_ssa}"), idx);
        }
        state.body = annotate_alias_scope_metadata(&state.body, &seed, &this_lists, &other_lists);
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
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    is_lib: bool,
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
    //
    // Slice 1F (v0.0.2): destructors are compiler-synthesized cold paths.
    // Apply `preserve_nonecc` (no callee-save register saves at the call
    // boundary) plus `cold` (the optimizer biases hot paths away from
    // them). Drop runs once per object at scope exit — it's the canonical
    // cold helper. `preserve_nonecc` requires clang/LLVM 17+; macOS
    // shipped that in Xcode 15.3 (Feb 2024).
    let is_drop_method = m.name.name == "drop";
    let cc_prefix = if is_drop_method { "preserve_nonecc " } else { "" };
    let fn_attrs = if is_drop_method { " cold" } else { "" };
    // Phase 5 Slice 5.B: in library builds, non-`pub` methods get
    // `internal` linkage. `drop` is compiler-synthesized infrastructure —
    // not part of the public C-ABI surface even when `pub`; always
    // internal in lib mode. Executable builds keep external linkage on
    // every method (matches pre-5.B behavior).
    let linkage = if !is_lib || (m.is_pub && !is_drop_method) { "" } else { "internal " };
    write!(out, "define {}{}{} @{}(", linkage, cc_prefix, llvm_ty(&return_ty, types), mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    if let Some(rcv) = sig.receiver {
        // Slice 1A: receiver gets the full pointer attr set. Map Receiver
        // kind onto (move_, mutable) for `param_attrs`:
        //   Read => (false, false) → readonly
        //   Mut  => (false, true)  → noalias
        //   Move => (true,  true)  → noalias (callee owns; exclusive)
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut  => (false, true),
            Receiver::Move => (true,  true),
        };
        let attrs = param_attrs(&struct_ty, mv, mu, true, types);
        if attrs.is_empty() {
            write!(out, "ptr %{llvm_idx}").unwrap();
        } else {
            write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first { out.push_str(", "); }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, by_ptr, types);
        let base_ty = if by_ptr { "ptr".to_string() } else { llvm_ty(pty, types) };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(")");
    out.push_str(fn_attrs);
    out.push_str(" {\n");
    out.push_str("entry:\n");

    let mut state = FnState::new(return_ty.clone(), sigs, types, str_lits, mode, test_mode, md);
    state.collect_moved_bindings(&m.body);
    // Destructors don't auto-drop their receiver — we *are* the destructor.
    if m.name.name == "drop" {
        state.in_destructor = true;
    }

    // Bind the receiver: `self` is the pointer parameter directly.
    let mut next_idx: u32 = 0;
    if let Some(rcv) = sig.receiver {
        state.bind("self", "%0".to_string(), struct_ty.clone());
        next_idx = 1;
        // `move self` consumes the receiver: the method body owns it, so
        // we register a scope-exit drop for `self` (unless we *are* the
        // destructor — see `in_destructor` above). For `self` / `mut self`
        // the receiver is non-owning (post-§2.8a pointer-pass), so no drop.
        if matches!(rcv, Receiver::Move) && !state.in_destructor {
            if let Ty::Struct(id) = struct_ty {
                if types.struct_defs[id.0 as usize].is_drop {
                    state.register_drop("self", "%0", id);
                }
            }
        }
    }

    // Bind non-receiver params. Pointer-passed (`mut x: T` non-Copy) bind
    // directly to the SSA argument so writes propagate to the caller's
    // place. Value-passed params copy into an alloca; `move`-marked Drop
    // params register a scope-exit drop. Non-`move` value-passed params are
    // bit-duplicates of the caller's value, so codegen does NOT register a
    // drop for them (the caller still owns the original).
    for (i, (param, (pty, move_flag, mut_flag))) in m.params.iter().zip(sig.params.iter()).enumerate() {
        let idx = next_idx + i as u32;
        if param_passes_by_ptr(pty, *move_flag, *mut_flag, types) {
            state.bind(&param.name.name, format!("%{idx}"), pty.clone());
            continue;
        }
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types), idx, slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
        if *move_flag {
            if let Ty::Struct(id) = pty {
                if types.struct_defs[id.0 as usize].is_drop {
                    state.register_drop(&param.name.name, &slot, *id);
                }
            }
        }
    }

    state.gen_body_block(&m.body);

    if !state.terminated {
        match &return_ty {
            Ty::Unit => state.emit_terminator("ret void"),
            _ => state.emit_terminator("unreachable"),
        }
    }

    // Slice 1C: scoped alias metadata. The receiver counts as a noalias
    // param when it's `mut self` or `move self`; `self` (Read) is
    // `readonly` and does NOT participate in the scope set (two shared
    // refs may alias).
    let mut noalias_ssas: Vec<u32> = Vec::new();
    if let Some(rcv) = sig.receiver {
        if matches!(rcv, Receiver::Mut | Receiver::Move) {
            noalias_ssas.push(0);
        }
    }
    for (i, (_, (pty, mv, mu))) in m.params.iter().zip(sig.params.iter()).enumerate() {
        let idx = next_idx + i as u32;
        if param_passes_by_ptr(pty, *mv, *mu, types) && (*mv || *mu) {
            noalias_ssas.push(idx);
        }
    }
    if noalias_ssas.len() >= 2 {
        let domain = md.register_alias_domain(&mangled);
        let scopes: Vec<u32> = noalias_ssas.iter().enumerate()
            .map(|(i, _)| md.register_alias_scope(domain, &format!("p{i}"))).collect();
        let this_lists: Vec<u32> = scopes.iter()
            .map(|&s| md.register_alias_scope_list(&[s])).collect();
        let other_lists: Vec<u32> = (0..scopes.len()).map(|i| {
            let others: Vec<u32> = scopes.iter().enumerate()
                .filter(|(j, _)| *j != i).map(|(_, &s)| s).collect();
            md.register_alias_scope_list(&others)
        }).collect();
        let mut seed: HashMap<String, usize> = HashMap::new();
        for (idx_in_set, &ssa_idx) in noalias_ssas.iter().enumerate() {
            seed.insert(format!("%{ssa_idx}"), idx_in_set);
        }
        state.body = annotate_alias_scope_metadata(&state.body, &seed, &this_lists, &other_lists);
    }

    for line in &state.allocas {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// A Drop binding registered in some scope frame. At scope exit (or `return`)
/// codegen walks these in reverse-registration order and emits a conditional
/// call to `T::drop(value_slot)` gated on `flag_slot`. The flag is initialized
/// to `true` when the binding is created and flipped to `false` whenever the
/// binding is moved out (via a `move`-marked param or `move self` receiver).
#[derive(Debug, Clone)]
struct DropEntry {
    binding_name: String,
    value_slot: String,
    flag_slot: String,
    /// Phase 8 slice 8.STR.3 follow-up (2026-05-14): which kind of
    /// Drop are we registering? `Struct(id)` is the original case —
    /// emits `call @<type>.drop(ptr)`. `String` is the owned-string
    /// case — loads the `ptr` field and emits `call @free(ptr)`.
    kind: DropKind,
    /// Slice 6BC.opt: static drop-flag specialization. When `Always`,
    /// emit an unconditional drop call at scope exit (no flag-load, no
    /// branch). `Runtime` is the Phase-5 default — the load + branch on
    /// the per-binding flag handles the MaybePartial case where the
    /// binding may or may not have been moved on different paths.
    disposition: DropDisposition,
}

#[derive(Debug, Clone, Copy)]
enum DropKind {
    Struct(StructId),
    /// Phase 8 slice 8.STR.3 follow-up: owned `string`. The drop body
    /// loads the `ptr` field from the value slot and calls `@free(ptr)`.
    /// `@free(null)` is a libc no-op so `string::new()` (which stores
    /// `null`) drops cleanly without a separate null-check.
    String,
}

/// Slice 6BC.opt: per-Drop-binding lowering choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    /// Binding is never moved on any path through the function — the
    /// drop flag would always be true at scope exit, so we elide both
    /// the flag and the conditional branch and emit a direct
    /// `call @T.drop(ptr)`. The bigger win on common code: most Drop
    /// bindings are "let it and drop it at end" without any moves.
    Always,
    /// Phase-5 default: emit the i1 alloca + flag-load + conditional
    /// drop. Needed when the binding may be moved on some paths.
    Runtime,
}

/// A scope-exit hook. Drop and `defer` share one LIFO stack per scope
/// (see `docs/design/phase3-drop.md` §4.4). At scope exit codegen walks
/// the frame in reverse-registration order and dispatches:
///   - `Drop` → conditional call gated on the drop flag
///   - `Defer` → re-emit the deferred expression (value discarded)
#[derive(Debug, Clone)]
enum ScopeExit {
    Drop(DropEntry),
    Defer(Expr),
}

/// Phase 8 slice 8.STR.1: registry of unique string literals collected
/// by a pre-pass and emitted as `@.str.N` globals. Maps literal payload
/// (decoded UTF-8 bytes) → `(global_symbol, byte_len_without_nul)`.
type StrLitTable = HashMap<String, (String, usize)>;

struct FnState<'a> {
    body: String,
    allocas: Vec<String>,
    scopes: Vec<HashMap<String, (String, Ty)>>,
    /// Phase 8 slice 8.STR.1: shared lookup of string-literal globals.
    str_lits: &'a StrLitTable,
    /// Slice 1B: module-level metadata table. Codegen registers `!range`
    /// nodes here; the table emits its accumulated definitions at module-end.
    md: &'a ModuleMetadata,
    /// Parallel stack to `scopes`. Each frame collects scope-exit hooks
    /// (Drop bindings + `defer` statements) in registration order. At scope
    /// close codegen walks the frame in reverse and dispatches each entry.
    scope_exits: Vec<Vec<ScopeExit>>,
    return_ty: Ty,
    sigs: &'a HashMap<String, FnSig>,
    types: &'a TypeTable,
    mode: BuildMode,
    /// Slice 6BC.opt: precomputed set of binding names that ARE moved
    /// somewhere in this function body. Computed once at FnState
    /// construction. A binding name not in this set is provably never
    /// moved, so `register_drop` picks `DropDisposition::Always`.
    moved_bindings: std::collections::HashSet<String>,
    tmp_counter: u32,
    block_counter: u32,
    terminated: bool,
    /// True iff we are currently emitting the body of a destructor (a method
    /// named `drop`). The receiver `self` of a destructor is *not* registered
    /// as a Drop binding — running drop at end of drop would recurse. Other
    /// local Drop bindings inside the destructor body still register normally.
    in_destructor: bool,
    /// Slice 5ATTR.4: `assert` lowering depends on whether we're emitting a
    /// `cpc test` binary. In test mode the trap is replaced by a write to
    /// `@cpc_test_failed` so the driver's `main` can read which test failed
    /// without unwinding. In normal builds (false), `assert` traps.
    test_mode: bool,
    /// Slice 4-end: stack of `(continue_label, break_label)` for the
    /// enclosing loops. `break` jumps to `break_label`; `continue` jumps
    /// to `continue_label` (the loop's back-edge / cond-check / increment
    /// trampoline). Pushed when entering a loop body, popped on exit.
    loop_labels: Vec<(String, String)>,
    /// Slice 1E (v0.0.2): single-use hint set by `StmtKind::Return` when the
    /// statement is `return foo(args);` and the call qualifies for
    /// `musttail` (return type matches enclosing fn; no Drop/defer entries
    /// pending; non-variadic; default CC). gen_named_call consults this
    /// flag, emits `musttail call`, and clears it.
    pending_musttail: bool,
    /// Slice 1E: enclosing function's parameter types in declaration order.
    /// `musttail` requires the *caller*'s parameter signature to match the
    /// *callee*'s — LLVM's verifier rejects musttail across mismatched
    /// arities. Filled in by `gen_function` before body codegen.
    enclosing_params: Vec<Ty>,
    /// Slice 1E: true iff this body is a free function (eligible for
    /// `musttail`). Methods carry a receiver in their LLVM signature, so
    /// even matching the call's arg list isn't enough — the receiver
    /// position would mismatch. Disable musttail in method bodies to keep
    /// the predicate simple. Set by `gen_function`; defaults false.
    tail_call_eligible: bool,
    /// Slice 1D (v0.0.2): SSA name of this fn's sret parameter (the
    /// caller-allocated result slot) when `return_passes_by_sret` fires.
    /// `StmtKind::Return` consults it: `Some(slot)` → store the value to
    /// the slot and `ret void`; `None` → emit `ret <ty> <val>` as usual.
    sret_slot: Option<String>,
    /// Phase 5 Slice 5.D: when emitting a `pub extern fn` whose return
    /// type lowers to a coerced C-ABI integer class (≤8 → i64, 9..16 →
    /// `[2 x i64]`), `StmtKind::Return` packs the value through an
    /// alloca and emits `ret <coerced>` instead of `ret <original>`.
    /// `None` means no coercion is needed (Direct return).
    coerce_ret: Option<String>,
}

impl<'a> FnState<'a> {
    fn new(return_ty: Ty, sigs: &'a HashMap<String, FnSig>, types: &'a TypeTable, str_lits: &'a StrLitTable, mode: BuildMode, test_mode: bool, md: &'a ModuleMetadata) -> Self {
        Self {
            body: String::new(),
            allocas: Vec::new(),
            scopes: vec![HashMap::new()],
            scope_exits: vec![Vec::new()],
            return_ty,
            sigs,
            types,
            str_lits,
            md,
            mode,
            moved_bindings: std::collections::HashSet::new(),
            tmp_counter: 0,
            block_counter: 0,
            terminated: false,
            in_destructor: false,
            test_mode,
            loop_labels: Vec::new(),
            pending_musttail: false,
            enclosing_params: Vec::new(),
            tail_call_eligible: false,
            sret_slot: None,
            coerce_ret: None,
        }
    }

    /// Slice 6BC.opt: scan the function body for `move`-position
    /// argument bindings. Returns a set of every binding name that is
    /// moved somewhere in the body. Used by `register_drop` to pick
    /// `Always` (never-moved) vs `Runtime` (may-be-moved) drop
    /// lowering. The walk is purely syntactic — it doesn't need
    /// type information because the callee's signature tells us
    /// which arg positions are `move`. We consult `sigs` to know
    /// each callee's `move_` flags.
    fn collect_moved_bindings(&mut self, body: &Block) {
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        scan_moves_in_block(body, self.sigs, self.types, &mut set);
        self.moved_bindings = set;
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
        // Uniquify across the function so the same source-level name in
        // different scopes (e.g. a function param `s` and a match-arm
        // payload binding `s`) gets distinct LLVM SSA names. Bump the
        // anonymous counter for the suffix to keep names deterministic.
        self.tmp_counter += 1;
        let slot = format!("%{}.addr{}", sanitize(name_hint), self.tmp_counter);
        self.allocas.push(format!("{slot} = alloca {}", self.lty(&ty)));
        slot
    }

    fn alloca_anon(&mut self, ty: Ty) -> String {
        self.tmp_counter += 1;
        let slot = format!("%a{}", self.tmp_counter);
        self.allocas.push(format!("{slot} = alloca {}", self.lty(&ty)));
        slot
    }

    /// Phase 5 Slice 5.D: alloca a slot whose LLVM type is given as a raw
    /// string (e.g. `i64`, `[2 x i64]`) with an explicit alignment. Used
    /// by C-ABI param coercion where the alloca's size + align must
    /// match the *coerced* type (which is at least as large as the
    /// original struct), even though the binding's logical type is the
    /// original C+ struct.
    fn alloca_named_raw(&mut self, name_hint: &str, llvm_ty_str: &str, align: u64) -> String {
        self.tmp_counter += 1;
        let slot = format!("%{}.addr{}", sanitize(name_hint), self.tmp_counter);
        self.allocas.push(format!("{slot} = alloca {llvm_ty_str}, align {align}"));
        slot
    }

    // ---- locals / scopes ----

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.scope_exits.push(Vec::new());
    }

    /// Close the innermost scope, emitting scope-exit hooks (Drop calls and
    /// `defer` expressions) for everything registered in this scope, in
    /// reverse registration order. If the current block is already terminated
    /// (e.g. by an early `return`) the hooks are skipped — the `return` path
    /// already emitted them.
    fn pop_scope(&mut self) {
        if !self.terminated {
            let frame = self.scope_exits.last().cloned().unwrap_or_default();
            for entry in frame.iter().rev() {
                self.emit_scope_exit(entry);
            }
        }
        self.scopes.pop();
        self.scope_exits.pop();
    }

    fn bind(&mut self, name: &str, slot: String, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), (slot, ty));
    }

    fn lookup(&self, name: &str) -> Option<&(String, Ty)> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) { return Some(entry); }
        }
        None
    }

    // ---- Drop registration + emission ----

    /// Register a Drop binding in the current scope. Slice 6BC.opt
    /// picks the lowering disposition based on whether the binding is
    /// moved anywhere in the function:
    ///   - **Always** (binding NOT in `moved_bindings`): no flag at
    ///     all. Scope-exit emits a direct unconditional drop call.
    ///     Saves the alloca, the initial store, the flag-load, the
    ///     conditional branch, and the post-drop label.
    ///   - **Runtime** (binding IS in `moved_bindings`): allocate the
    ///     `i1` drop flag, init to `true`, record for runtime check.
    ///
    /// Returns the flag slot string. For `Always` dispositions the
    /// returned string is a placeholder that's never used (callers
    /// that flip the flag via `find_drop_flag` would skip dropped
    /// bindings naturally — but the precondition is no moves happen).
    fn register_drop(&mut self, binding_name: &str, value_slot: &str, struct_id: StructId) -> String {
        self.register_drop_kind(binding_name, value_slot, DropKind::Struct(struct_id))
    }

    /// Phase 8 slice 8.STR.3 follow-up (2026-05-14): register a scope-
    /// exit free for an owned `string` local. Same flag mechanism as
    /// `register_drop`; the only difference is the lowered IR (load
    /// the ptr field, call free, no struct-drop dispatch).
    fn register_string_drop(&mut self, binding_name: &str, value_slot: &str) -> String {
        self.register_drop_kind(binding_name, value_slot, DropKind::String)
    }

    fn register_drop_kind(&mut self, binding_name: &str, value_slot: &str, kind: DropKind) -> String {
        let disposition = if self.moved_bindings.contains(binding_name) {
            DropDisposition::Runtime
        } else {
            DropDisposition::Always
        };
        let flag_slot = match disposition {
            DropDisposition::Runtime => {
                self.tmp_counter += 1;
                let s = format!("%{}.drop_flag", sanitize(binding_name));
                self.allocas.push(format!("{s} = alloca i1"));
                self.emit(&format!("store i1 true, ptr {s}"));
                s
            }
            DropDisposition::Always => {
                format!("%{}.drop_flag.unused", sanitize(binding_name))
            }
        };
        self.scope_exits.last_mut().unwrap().push(ScopeExit::Drop(DropEntry {
            binding_name: binding_name.to_string(),
            value_slot: value_slot.to_string(),
            flag_slot: flag_slot.clone(),
            kind,
            disposition,
        }));
        flag_slot
    }

    /// Register a `defer EXPR;` hook in the current scope. The expression
    /// fires at scope exit (lexical), in LIFO order with surrounding Drop
    /// calls.
    fn register_defer(&mut self, expr: Expr) {
        self.scope_exits.last_mut().unwrap().push(ScopeExit::Defer(expr));
    }

    /// Look up a Drop binding's flag slot by binding name. Walks scope
    /// frames from innermost to outermost (matches `lookup` semantics).
    fn find_drop_flag(&self, name: &str) -> Option<String> {
        for frame in self.scope_exits.iter().rev() {
            for entry in frame.iter().rev() {
                if let ScopeExit::Drop(d) = entry {
                    if d.binding_name == name {
                        return Some(d.flag_slot.clone());
                    }
                }
            }
        }
        None
    }

    /// Flip a Drop binding's flag to `false`, suppressing its scope-exit
    /// drop. Called when codegen emits a `move`-marked argument or a
    /// `move self` receiver and the source is a plain Ident.
    fn mark_moved(&mut self, name: &str) {
        if let Some(flag) = self.find_drop_flag(name) {
            self.emit(&format!("store i1 false, ptr {flag}"));
        }
        // If there's no flag, the binding isn't Drop — nothing to do.
    }

    /// Emit a drop call for a Drop binding at scope-exit. Slice
    /// 6BC.opt's disposition decides the lowering:
    /// - **Always**: direct unconditional `call @T.drop(ptr)`. No
    ///   flag-load, no conditional, no extra basic blocks. Most
    ///   common case — bindings that get let-and-dropped.
    /// - **Runtime**: load the flag, branch on it, drop in the true
    ///   arm, fall through. The Phase-5 default for bindings that
    ///   may have been moved on some paths.
    fn emit_conditional_drop(&mut self, entry: &DropEntry) {
        // Build the drop-body emitter once; the disposition switch only
        // decides whether to gate it on the flag.
        let body = |state: &mut Self| {
            match entry.kind {
                DropKind::Struct(struct_id) => {
                    let struct_name = state.types.struct_defs[struct_id.0 as usize].name.clone();
                    let mangled = format!("{struct_name}.drop");
                    // Slice 1F: matching `preserve_nonecc` on the call site
                    // — caller and callee must agree on CC. (The function
                    // attribute `cold` is callee-only and has no caller
                    // syntax mirror.)
                    state.emit(&format!("call preserve_nonecc void @{mangled}(ptr {})", entry.value_slot));
                }
                DropKind::String => {
                    // Load the `ptr` field (offset 0 in the {ptr, i64,
                    // i64} aggregate) and free it. free(null) is a libc
                    // no-op so `string::new()`'s null ptr is safe.
                    let pp = state.next_tmp();
                    state.emit(&format!(
                        "{pp} = getelementptr {{ ptr, i64, i64 }}, ptr {}, i32 0, i32 0",
                        entry.value_slot
                    ));
                    let pv = state.next_tmp();
                    state.emit(&format!("{pv} = load ptr, ptr {pp}"));
                    state.emit(&format!("call void @free(ptr {pv})"));
                }
            }
        };
        match entry.disposition {
            DropDisposition::Always => {
                body(self);
            }
            DropDisposition::Runtime => {
                let flag_val = self.next_tmp();
                self.emit(&format!("{flag_val} = load i1, ptr {}", entry.flag_slot));
                let drop_lbl = self.next_block_label();
                let skip_lbl = self.next_block_label();
                self.emit_terminator(&format!(
                    "br i1 {flag_val}, label %{drop_lbl}, label %{skip_lbl}"
                ));
                self.open_block(&drop_lbl);
                body(self);
                self.open_block(&skip_lbl);
            }
        }
    }

    /// Dispatch one scope-exit entry: Drop → conditional call;
    /// Defer → re-emit the expression (value discarded).
    fn emit_scope_exit(&mut self, entry: &ScopeExit) {
        match entry {
            ScopeExit::Drop(d) => self.emit_conditional_drop(d),
            ScopeExit::Defer(e) => {
                // Re-emit the deferred expression as a discard-value
                // expression statement. Side effects fire; the result
                // (if any) is dropped on the floor.
                let _ = self.gen_expr(e);
            }
        }
    }

    /// Emit scope-exit hooks for *every* live scope (all frames, innermost
    /// first, each frame reverse-registered). Used by `return` so all
    /// destructors + defers run on the early-exit path before `ret`.
    fn emit_all_scope_exits(&mut self) {
        let frames: Vec<Vec<ScopeExit>> = self.scope_exits.iter().rev().cloned().collect();
        for frame in &frames {
            for entry in frame.iter().rev() {
                if self.terminated { return; }
                self.emit_scope_exit(entry);
            }
        }
    }

    // ---- function body ----

    fn gen_body_block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        if !self.terminated {
            // Implicit function-exit path: evaluate the optional tail first
            // (post-§2.8a, function bodies must use explicit `return`, but
            // Unit-returning bodies are allowed to fall off the end), then
            // run scope-exit drops, then ret.
            match &b.tail {
                Some(t) => {
                    let val = self.gen_expr(t);
                    self.emit_all_scope_exits();
                    if !self.terminated {
                        match self.return_ty {
                            Ty::Unit => self.emit_terminator("ret void"),
                            _ => {
                                let (v, _) = val.expect("non-Unit fn requires tail value");
                                self.emit_terminator(&format!("ret {} {}", self.lty(&self.return_ty), v));
                            }
                        }
                    }
                }
                None => {
                    if self.return_ty == Ty::Unit {
                        self.emit_all_scope_exits();
                        if !self.terminated {
                            self.emit_terminator("ret void");
                        }
                    }
                    // For non-Unit returns sema requires an explicit
                    // `return`; the last stmt already terminated and ran
                    // drops via the StmtKind::Return path.
                }
            }
        }
        self.pop_scope();
    }

    // ---- statements ----

    fn gen_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, ty, init, .. } => {
                // Resolve declared type up front (always present for the
                // uninitialized case — sema enforced that).
                let var_ty = match (ty, init) {
                    (Some(t), _) => ty_from(t, self.types),
                    (None, Some(init_expr)) => {
                        // Inferred from the init expression's type.
                        let (val, val_ty) = self.gen_expr(init_expr).expect("let init produces a value");
                        let slot = self.alloca_named(&name.name, val_ty.clone());
                        self.emit(&format!(
                            "store {} {}, ptr {}",
                            self.lty(&val_ty), val, slot
                        ));
                        match &val_ty {
                            Ty::Struct(id) if self.types.struct_defs[id.0 as usize].is_drop => {
                                self.register_drop(&name.name, &slot, *id);
                            }
                            Ty::String => {
                                self.register_string_drop(&name.name, &slot);
                            }
                            _ => {}
                        }
                        self.bind(&name.name, slot, val_ty);
                        return;
                    }
                    (None, None) => unreachable!("sema rejected uninit `let` without annotation"),
                };
                let slot = self.alloca_named(&name.name, var_ty.clone());
                if let Some(init_expr) = init {
                    let (val, _) = self.gen_expr(init_expr).expect("let init produces a value");
                    self.emit(&format!("store {} {}, ptr {}", self.lty(&var_ty), val, slot));
                }
                // If the type carries a destructor, register a scope-exit
                // drop hook before binding the name (so the flag exists by
                // the time anything references this binding). For an
                // uninitialized Drop binding this is currently safe because
                // sema rejects any path that would read it before it's
                // assigned — so drop only runs after assignment.
                match &var_ty {
                    Ty::Struct(id) if self.types.struct_defs[id.0 as usize].is_drop => {
                        self.register_drop(&name.name, &slot, *id);
                    }
                    Ty::String => {
                        self.register_string_drop(&name.name, &slot);
                    }
                    _ => {}
                }
                self.bind(&name.name, slot, var_ty);
            }
            StmtKind::Return(value) => {
                let ret_ty = self.return_ty.clone();
                // Slice 1E (v0.0.2): musttail eligibility for direct calls.
                // The statement must be `return foo(args);` where:
                //   - foo is a known named function (Ident callee in `sigs`)
                //   - foo's return type matches the enclosing fn's return type
                //   - foo is non-variadic (musttail demands matching arity)
                //   - no Drop/defer entries are pending — musttail requires
                //     the ret to immediately follow the call, with nothing
                //     between (the LLVM verifier rejects otherwise)
                //   - foo is not a builtin (`println` lowers to printf)
                // Methods, indirect (FnPtr) calls, and assoc-fn calls are not
                // currently handled (small surface; revisit if measured).
                if self.tail_call_eligible {
                    if let Some(e) = value {
                        if let ExprKind::Call { callee, args: _, .. } = &e.kind {
                            if let ExprKind::Ident(name) = &callee.kind {
                                let pending_drops = self.scope_exits.iter().any(|frame| !frame.is_empty());
                                if !pending_drops {
                                    if let Some(sig) = self.sigs.get(name) {
                                        let callee_params: Vec<&Ty> =
                                            sig.params.iter().map(|(t, _, _)| t).collect();
                                        let enclosing: Vec<&Ty> =
                                            self.enclosing_params.iter().collect();
                                        if !sig.is_variadic
                                            && sig.return_type == self.return_ty
                                            && callee_params == enclosing
                                            && name != "println"
                                        {
                                            self.pending_musttail = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Evaluate the return value first so any moves it triggers
                // (e.g. `return f(move_x)`) flip drop flags before scope drops.
                //
                // Slice 1D: when `gen_expr` lowers a musttail+sret call, it
                // emits `ret void` itself (the caller's sret slot is the
                // callee's sret slot, so the value has already landed by
                // the time control returns) and sets `self.terminated`.
                // Don't `.expect` a value in that case — gen_expr returned
                // None *because* it terminated the block early.
                let ret_val = match value {
                    Some(e) => {
                        let v = self.gen_expr(e);
                        if self.terminated { return; }
                        Some(v.expect("non-Unit return value").0)
                    }
                    None => None,
                };
                // Defensive: clear the flag in case gen_expr didn't reach
                // gen_named_call (e.g. ExprKind::Call routed through a
                // different lowering path).
                self.pending_musttail = false;
                // Run destructors for all live Drop bindings in every scope
                // before the `ret`. The conditional drop respects each
                // binding's flag, so values moved into the return expr are
                // not double-dropped.
                self.emit_all_scope_exits();
                if self.terminated { return; }
                match (ret_val, &ret_ty) {
                    (Some(v), _) => {
                        // Slice 1D: when this fn uses sret, the result
                        // lands in the caller-provided slot — store the
                        // value there and return void.
                        // Phase 5 Slice 5.D: when this fn is a C-ABI export
                        // with a coerced return (≤16 byte aggregate
                        // packed into i64 / [2 x i64]), stage the value
                        // through a temp alloca and reload as the coerced
                        // type before returning.
                        if let Some(slot) = self.sret_slot.clone() {
                            let lty = self.lty(&ret_ty);
                            self.emit(&format!("store {lty} {v}, ptr {slot}"));
                            self.emit_terminator("ret void");
                        } else if let Some(coerced) = self.coerce_ret.clone() {
                            // Stage the original-typed value through a
                            // stack slot, then reload via the coerced
                            // LLVM type. The slot must be sized for the
                            // coerced type (which is ≥ the original) so
                            // the wide load doesn't read OOB. Bytes
                            // beyond the original size are caller-side
                            // undefined per the C ABI — `0`-initializing
                            // them keeps the load deterministic for the
                            // common scalar-output case.
                            let lty = self.lty(&ret_ty);
                            // Use coerce_ret's name to size the alloca.
                            // Convention: i64 → 8 bytes align 8; [2 x i64] → 16/8.
                            let (sz, al) = if coerced == "i64" { (8u64, 8u64) }
                                else if coerced.contains("[2 x i64]") { (16, 8) }
                                else { (8, 8) };
                            let tmp = self.alloca_named_raw("ret.coerce", &coerced, al);
                            // Zero-initialize so unused tail bytes are 0,
                            // not poison. memset via i8 store is cheap;
                            // -O2 will fold it together with the user store.
                            self.emit(&format!(
                                "call void @llvm.memset.p0.i64(ptr {tmp}, i8 0, i64 {sz}, i1 false)"
                            ));
                            self.emit(&format!("store {lty} {v}, ptr {tmp}"));
                            let coerced_v = self.next_tmp();
                            self.emit(&format!("{coerced_v} = load {coerced}, ptr {tmp}"));
                            self.emit_terminator(&format!("ret {coerced} {coerced_v}"));
                        } else {
                            self.emit_terminator(&format!("ret {} {}", self.lty(&ret_ty), v));
                        }
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
            StmtKind::Defer(e) => {
                // Lexical defer: register the expression to run at the
                // enclosing scope's exit, LIFO with surrounding Drop calls.
                // Sema has already type-checked the expression; we clone the
                // AST node into the scope-exit frame and re-emit it later.
                self.register_defer(e.clone());
            }
            // Lowering replaces these with match-using forms before sema
            // and codegen ever see them.
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {
                panic!("codegen saw an un-lowered if-let/guard-let/while-let — driver must call crate::lower before codegen");
            }
            StmtKind::Break => {
                let (_, break_lbl) = self.loop_labels.last()
                    .expect("sema rejects `break` outside a loop (E0353)")
                    .clone();
                self.emit_terminator(&format!("br label %{break_lbl}"));
            }
            StmtKind::Continue => {
                let (cont_lbl, _) = self.loop_labels.last()
                    .expect("sema rejects `continue` outside a loop (E0353)")
                    .clone();
                self.emit_terminator(&format!("br label %{cont_lbl}"));
            }
            StmtKind::Loop(body) => self.gen_loop(body),
            // Phase 5 slice 5ATTR.3: `assert EXPR;` — branch on the bool
            // and trap on the false path. Sema guarantees the expression
            // type is bool. Phase-5 trap-only behavior; slice 5ATTR.4 will
            // replace the trap with a per-test failure-flag write inside
            // synthesized test-driver builds.
            StmtKind::Assert(e) => self.gen_assert(e),
        }
    }

    fn gen_assert(&mut self, cond: &Expr) {
        let (v, _) = self.gen_expr(cond).expect("assert cond is a bool value");
        let fail_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {v}, label %{ok_lbl}, label %{fail_lbl}"));
        self.open_block(&fail_lbl);
        if self.test_mode {
            // Slice 5ATTR.4: under `cpc test`, an `assert` failure sets the
            // shared `@cpc_test_failed` flag and falls through. The driver's
            // `main` reads the flag after each test's call to decide pass/fail.
            // We do *not* return early — Phase 5 has no raw pointers, so
            // continuing past a failed assertion can't segfault, and a flag
            // write is cheaper than synthesizing a return-of-default per type.
            self.emit("store i32 1, ptr @cpc_test_failed");
            self.emit_terminator(&format!("br label %{ok_lbl}"));
        } else {
            self.emit("call void @llvm.trap()");
            self.emit_terminator("unreachable");
        }
        self.open_block(&ok_lbl);
    }

    /// `loop { body }` — emit:
    ///   head:
    ///     body            ; may `br exit` (break) or `br head` (continue) or fall through
    ///     br head
    ///   exit:
    fn gen_loop(&mut self, body: &Block) {
        let head = self.next_block_label();
        let exit = self.next_block_label();
        self.emit_terminator(&format!("br label %{head}"));
        self.open_block(&head);
        // `continue` in a `loop` jumps back to `head`; `break` jumps to `exit`.
        self.loop_labels.push((head.clone(), exit.clone()));
        self.push_scope();
        for s in &body.stmts {
            if self.terminated { break; }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &body.tail {
                let _ = self.gen_expr(tail);
            }
            self.emit_terminator(&format!("br label %{head}"));
        }
        self.pop_scope();
        self.loop_labels.pop();
        self.open_block(&exit);
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
        // `continue` re-evaluates the cond → branches to `head`. `break`
        // exits to `exit`. Slice 4-end.
        self.loop_labels.push((head.clone(), exit.clone()));
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
        self.loop_labels.pop();

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
                let step = self.next_block_label();
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
                // `continue` in a for-range loop must run the increment;
                // route it through `step`, not back to `head`. Slice 4-end.
                self.loop_labels.push((step.clone(), exit.clone()));
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated { break; }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail { let _ = self.gen_expr(tail); }
                    self.emit_terminator(&format!("br label %{step}"));
                }
                self.pop_scope();
                self.loop_labels.pop();

                // Step block: increment then back to head.
                self.open_block(&step);
                let cur_i = self.next_tmp();
                self.emit(&format!("{cur_i} = load i32, ptr {i_slot}"));
                let next_i = self.next_tmp();
                self.emit(&format!("{next_i} = add i32 {cur_i}, 1"));
                self.emit(&format!("store i32 {next_i}, ptr {i_slot}"));
                self.emit_terminator(&format!("br label %{head}"));

                self.pop_scope();
                self.open_block(&exit);
            }
            ForLoop::CStyle { init, cond, update, body } => {
                self.push_scope();
                if let Some(init) = init { self.gen_stmt(init); }

                let head = self.next_block_label();
                let body_lbl = self.next_block_label();
                let step = self.next_block_label();
                let exit = self.next_block_label();

                self.emit_terminator(&format!("br label %{head}"));
                self.open_block(&head);
                let cond_v = match cond {
                    Some(c) => self.gen_expr(c).expect("for-cond produces bool").0,
                    None => "true".to_string(),
                };
                self.emit_terminator(&format!("br i1 {cond_v}, label %{body_lbl}, label %{exit}"));

                self.open_block(&body_lbl);
                // `continue` in a C-style for must run the update list;
                // route through `step`. Slice 4-end.
                self.loop_labels.push((step.clone(), exit.clone()));
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated { break; }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail { let _ = self.gen_expr(tail); }
                    self.emit_terminator(&format!("br label %{step}"));
                }
                self.pop_scope();
                self.loop_labels.pop();

                // Step block: run update list, branch back to head.
                self.open_block(&step);
                for u in update { let _ = self.gen_expr(u); }
                self.emit_terminator(&format!("br label %{head}"));

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
            ExprKind::StrLit(s) => {
                // Phase 8 slice 8.STR.1: lower a string literal to a fat-pointer
                // value `{ ptr, i64 }`. The bytes live in a `@.str.N` global
                // emitted by the pre-pass; we just look up the symbol + length
                // and build the struct via `insertvalue`.
                let (symbol, len) = self.str_lits.get(s).expect("str literal not in table").clone();
                let t1 = self.next_tmp();
                let t2 = self.next_tmp();
                self.body.push_str(&format!(
                    "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
                ));
                self.body.push_str(&format!(
                    "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
                ));
                Some((t2, Ty::Str))
            }
            ExprKind::InterpStr { parts } => {
                Some(self.gen_interp_str(parts))
            }
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
                // Slice 11.FN_PTR: bare-ident referring to a fn (sema
                // coerced it via the expected-FnPtr context) produces
                // the symbol's address as a `ptr` SSA value. Use the
                // link_name if `#[link_name = "..."]` was set; otherwise
                // the source-level name.
                if let Some(sig) = self.sigs.get(name).cloned() {
                    let symbol: String = sig.link_name.clone().unwrap_or_else(|| name.to_string());
                    let params: Vec<Ty> = sig.params.iter().map(|(t, _, _)| t.clone()).collect();
                    let ty = Ty::FnPtr { params, return_type: Box::new(sig.return_type.clone()) };
                    return Some((format!("@{symbol}"), ty));
                }
                let (slot, ty) = self.lookup(name).expect("sema validated").clone();
                let v = self.next_tmp();
                self.emit(&format!("{v} = load {}, ptr {slot}", self.lty(&ty)));
                Some((v, ty))
            }

            ExprKind::Block(b) => self.gen_block_expr(b),
            // Slice 10.FFI.3: `unsafe { ... }` is a marker for sema;
            // codegen treats it as a regular block.
            ExprKind::Unsafe(b) => self.gen_block_expr(b),

            ExprKind::If { cond, then, else_branch } => {
                self.gen_if(cond, then, else_branch.as_deref())
            }

            ExprKind::Call { callee, args, type_args } => self.gen_call(callee, args, type_args),

            ExprKind::Binary { op, lhs, rhs } => Some(self.gen_binary(*op, lhs, rhs)),

            ExprKind::Unary { op, operand } => Some(self.gen_unary(*op, operand)),

            ExprKind::Assign { target, value, .. } => {
                self.gen_assign(target, value);
                None
            }

            ExprKind::Cast { expr, ty } => Some(self.gen_cast(expr, ty)),
            ExprKind::Path { segments } => Some(self.gen_path(segments)),
            ExprKind::StructLit { name, fields } => Some(self.gen_struct_lit(name, fields)),
            // Slice 7GEN.5c: GenericStructLit must not reach codegen —
            // monomorphize rewrites every instance to a regular StructLit
            // with the mangled name. If we get here, that pass missed.
            ExprKind::GenericStructLit { .. } => {
                panic!("codegen reached GenericStructLit — monomorphize did not rewrite this site")
            }
            ExprKind::Field { receiver, name } => Some(self.gen_field(receiver, name)),
            ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => Some(self.gen_array_lit(elements)),
            ExprKind::Index { receiver, index } => Some(self.gen_index(receiver, index)),
            ExprKind::Range { .. } => {
                unreachable!("sema rejects ranges outside `for ... in`")
            }
            ExprKind::Match { scrutinee, arms } => self.gen_match(scrutinee, arms),
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
        // Slice 10.FFI.2: raw-pointer indexing is unchecked pointer
        // arithmetic — no bounds check, no array-style outer GEP.
        if let Ty::RawPtr(inner_box) = recv_ty.clone() {
            let inner = (*inner_box).clone();
            let loaded_ptr = self.next_tmp();
            self.emit(&format!("{loaded_ptr} = load ptr, ptr {recv_ptr}"));
            let (idx_val, _) = self.gen_expr(index).expect("index has value");
            let inner_lt = self.lty(&inner);
            let ptr = self.next_tmp();
            self.emit(&format!(
                "{ptr} = getelementptr inbounds {inner_lt}, ptr {loaded_ptr}, i64 {idx_val}"
            ));
            let v = self.next_tmp();
            self.emit(&format!("{v} = load {inner_lt}, ptr {ptr}"));
            return (v, inner);
        }
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
        // Slice 1B: publish the post-bounds-check fact `idx < N` via
        // `llvm.assume`. -O2's ConstraintElimination uses this to drop
        // redundant checks on subsequent uses of `idx` against `n`.
        let in_bounds = self.next_tmp();
        self.emit(&format!("{in_bounds} = icmp ult i64 {idx_val}, {n}"));
        self.emit(&format!("call void @llvm.assume(i1 {in_bounds})"));
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
                // Slice 10.FFI.2: indexing on raw pointers is unchecked
                // pointer arithmetic. `p[i]` loads the pointer value from
                // its slot, then GEPs by i64 offset — no bounds check
                // (the pointer's length is unknown).
                if let Ty::RawPtr(inner_box) = recv_ty.clone() {
                    let inner = (*inner_box).clone();
                    let loaded_ptr = self.next_tmp();
                    self.emit(&format!("{loaded_ptr} = load ptr, ptr {recv_slot}"));
                    let (idx_val, _) = self.gen_expr(index).expect("index has value");
                    let inner_lt = self.lty(&inner);
                    let ptr = self.next_tmp();
                    self.emit(&format!(
                        "{ptr} = getelementptr inbounds {inner_lt}, ptr {loaded_ptr}, i64 {idx_val}"
                    ));
                    return (ptr, inner);
                }
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
                // Slice 1B: publish post-check fact `idx < N` via assume.
                let in_bounds = self.next_tmp();
                self.emit(&format!("{in_bounds} = icmp ult i64 {idx_val}, {n}"));
                self.emit(&format!("call void @llvm.assume(i1 {in_bounds})"));
                let ptr = self.next_tmp();
                self.emit(&format!("{ptr} = getelementptr {llvm_arr}, ptr {recv_slot}, i64 0, i64 {idx_val}"));
                (ptr, (*elem).clone())
            }
            // Slice 10.FFI.2: `*p` as an assignment target. `gen_place`
            // returns the pointer value itself (which IS the slot to
            // store into); the pointee type comes from the unwrapped
            // RawPtr.
            ExprKind::Unary { op: UnaryOp::Deref, operand } => {
                let (v, ty) = self.gen_expr(operand).expect("deref target has value");
                let inner = match ty {
                    Ty::RawPtr(i) => (*i).clone(),
                    _ => unreachable!("sema validated operand is RawPtr"),
                };
                (v, inner)
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
        let (r, rt) = self.gen_expr(rhs).expect("binary rhs has value");
        // Slice 10.FFI.2: pointer arithmetic `p + n` / `p - n`.
        // Lowers to `getelementptr inbounds T, ptr %p, i64 %n` where
        // T is the pointee. Subtract negates the index first.
        if let Ty::RawPtr(inner_box) = lt.clone() {
            if matches!(op, BinOp::Add | BinOp::Sub) {
                let inner = (*inner_box).clone();
                let inner_lt = self.lty(&inner);
                let idx = if matches!(op, BinOp::Sub) {
                    let neg = self.next_tmp();
                    self.emit(&format!("{neg} = sub i64 0, {r}"));
                    neg
                } else {
                    r.clone()
                };
                let ptr = self.next_tmp();
                self.emit(&format!(
                    "{ptr} = getelementptr inbounds {inner_lt}, ptr {l}, i64 {idx}"
                ));
                return (ptr, lt);
            }
        }
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
                // Phase 8 slice 8.STR.3: byte-level equality for `str`.
                // Lowering: (len_a == len_b) && (memcmp(p_a, p_b, len_a) == 0).
                // We pre-check length so unequal lengths short-circuit
                // without touching the bytes — same alloca+branch shape
                // used by gen_short_circuit.
                if matches!(lt, Ty::Str) && matches!(op, BinOp::Eq | BinOp::Ne) {
                    let result_slot = self.alloca_anon(Ty::Bool);
                    let lp = self.next_tmp();
                    let ll = self.next_tmp();
                    let rp = self.next_tmp();
                    let rl = self.next_tmp();
                    self.emit(&format!("{lp} = extractvalue {{ ptr, i64 }} {l}, 0"));
                    self.emit(&format!("{ll} = extractvalue {{ ptr, i64 }} {l}, 1"));
                    self.emit(&format!("{rp} = extractvalue {{ ptr, i64 }} {r}, 0"));
                    self.emit(&format!("{rl} = extractvalue {{ ptr, i64 }} {r}, 1"));
                    let len_eq = self.next_tmp();
                    self.emit(&format!("{len_eq} = icmp eq i64 {ll}, {rl}"));
                    let cmp_lbl = self.next_block_label();
                    let unequal_lbl = self.next_block_label();
                    let merge_lbl = self.next_block_label();
                    self.emit_terminator(&format!("br i1 {len_eq}, label %{cmp_lbl}, label %{unequal_lbl}"));
                    self.open_block(&cmp_lbl);
                    let mc = self.next_tmp();
                    self.emit(&format!("{mc} = call i32 @memcmp(ptr {lp}, ptr {rp}, i64 {ll})"));
                    let mc_eq = self.next_tmp();
                    self.emit(&format!("{mc_eq} = icmp eq i32 {mc}, 0"));
                    self.emit(&format!("store i1 {mc_eq}, ptr {result_slot}"));
                    self.emit_terminator(&format!("br label %{merge_lbl}"));
                    self.open_block(&unequal_lbl);
                    self.emit(&format!("store i1 false, ptr {result_slot}"));
                    self.emit_terminator(&format!("br label %{merge_lbl}"));
                    self.open_block(&merge_lbl);
                    let v = self.next_tmp();
                    self.emit(&format!("{v} = load i1, ptr {result_slot}"));
                    if matches!(op, BinOp::Ne) {
                        let inv = self.next_tmp();
                        self.emit(&format!("{inv} = xor i1 {v}, true"));
                        return (inv, Ty::Bool);
                    }
                    return (v, Ty::Bool);
                }
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
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // Phase 3A: plain LLVM `and` / `or` / `xor` on integers.
                // No overflow / range checks — bit ops can't overflow.
                let v = self.next_tmp();
                let iop = match op {
                    BinOp::BitAnd => "and",
                    BinOp::BitOr  => "or",
                    BinOp::BitXor => "xor",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::Shl | BinOp::Shr => {
                // Phase 3A: `shl` for left shift; right shift picks
                // `ashr` for signed (arithmetic — preserves sign bit) or
                // `lshr` for unsigned (logical — fills with zero).
                //
                // Shift count: sema allows any integer type. LLVM
                // requires both operands to have the same type, so we
                // truncate / zero-extend the RHS to the LHS width here
                // using the real evaluated RHS type.
                let lhs_t = self.lty(&lt);
                let coerced_r = self.coerce_int_to_width(&r, &rt, &lt);
                let v = self.next_tmp();
                let iop = match (op, lt.is_signed_int()) {
                    (BinOp::Shl, _) => "shl",
                    (BinOp::Shr, true)  => "ashr",
                    (BinOp::Shr, false) => "lshr",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {lhs_t} {l}, {coerced_r}"));
                (v, lt)
            }
        }
    }

    /// Phase 3A: coerce an SSA integer `val` of type `from_ty` to the width
    /// of `to_ty`. Used by shift codegen, which lets the RHS be any
    /// integer type but LLVM `shl/lshr/ashr` requires same-width operands.
    /// Zero-extends widening, truncates narrowing. Returns the SSA name of
    /// the coerced value (the original `val` when widths already match).
    fn coerce_int_to_width(&mut self, val: &str, from_ty: &Ty, to_ty: &Ty) -> String {
        let from_bits = ty_bit_width(from_ty);
        let to_bits   = ty_bit_width(to_ty);
        if from_bits == to_bits { return val.to_string(); }
        let from_lt = self.lty(from_ty);
        let to_lt   = self.lty(to_ty);
        let r = self.next_tmp();
        // Shift counts are inherently unsigned. zext is the right widening
        // even for signed sema types: i8 -> i64 with zext keeps the count
        // semantically equal (shift amounts >= 0 in valid programs).
        let op = if from_bits < to_bits { "zext" } else { "trunc" };
        self.emit(&format!("{r} = {op} {from_lt} {val} to {to_lt}"));
        r
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
            UnaryOp::Deref => {
                // Slice 10.FFI.2: `*p` lowers to a `load` from the
                // pointer. The pointee type comes from the operand's
                // `Ty::RawPtr` payload, NOT from `ty` (which IS the
                // raw-pointer type for the operand). `gen_expr`
                // returned `(v, RawPtr(inner))`; we load `<inner>` from
                // the pointer.
                let inner = match &ty {
                    Ty::RawPtr(i) => (**i).clone(),
                    _ => unreachable!("sema validated operand is RawPtr"),
                };
                let inner_lt = self.lty(&inner);
                self.emit(&format!("{r} = load {inner_lt}, ptr {v}"));
                (r, inner)
            }
            UnaryOp::BitNot => {
                // Phase 3A: `~v` lowers to `xor v, -1` (all-bits-set
                // constant), the standard LLVM idiom. Works on every
                // integer width; LLVM picks the right `-1` from the
                // operand type.
                self.emit(&format!("{r} = xor {} {v}, -1", self.lty(&ty)));
                (r, ty)
            }
            _ => unreachable!("sema rejects & / &mut in Phase 1"),
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
        let info = &self.types.enum_defs[id.0 as usize];
        let idx = info.variants.get(variant_name)
            .copied()
            .expect("sema validated variant name");
        // Plain enum (no payloads anywhere): bare i32 tag — Phase 2A path.
        if !info.is_tagged {
            return (idx.to_string(), Ty::Enum(id));
        }
        // Tagged enum, payload-less variant (e.g. `Maybe::None`): construct
        // the full tagged-enum value with the tag set and the payload area
        // left undefined. Result is the loaded aggregate.
        self.gen_tagged_construct(id, idx, &[])
    }

    /// Build a tagged-enum value. Strategy:
    ///   1. alloca `%enum.N` (the named tagged-enum struct).
    ///   2. GEP to field 0 (the i32 tag), store the variant index.
    ///   3. For each payload value, GEP to the payload byte array's i64 slot
    ///      at index k, bitcast to a pointer of the payload type, store.
    ///   4. Load the aggregate to produce the SSA value for the result.
    ///
    /// Payload slot 0 lives at field 1 of the enum struct, then GEP'd by i64
    /// index. Each payload value occupies one i64-aligned slot regardless
    /// of its actual width (Phase 3 simplification — see `write_struct_decls`).
    fn gen_tagged_construct(&mut self, id: EnumId, tag: u32, args: &[(String, Ty)]) -> (String, Ty) {
        let enum_ty = Ty::Enum(id);
        let llvm_enum = self.lty(&enum_ty);
        let slot = self.alloca_anon(enum_ty.clone());
        // Store tag at field 0.
        let tag_ptr = self.next_tmp();
        self.emit(&format!(
            "{tag_ptr} = getelementptr {llvm_enum}, ptr {slot}, i32 0, i32 0"
        ));
        self.emit(&format!("store i32 {tag}, ptr {tag_ptr}"));
        // Store each payload value in its slot.
        for (i, (val, ty)) in args.iter().enumerate() {
            // GEP to the i64 payload array, then to slot i.
            let slot_ptr = self.next_tmp();
            self.emit(&format!(
                "{slot_ptr} = getelementptr {llvm_enum}, ptr {slot}, i32 0, i32 1, i64 {i}"
            ));
            // Opaque pointers: storing as the payload type is a no-op cast.
            self.emit(&format!(
                "store {} {val}, ptr {slot_ptr}",
                self.lty(ty)
            ));
        }
        // Load the aggregate value.
        let v = self.next_tmp();
        self.emit(&format!("{v} = load {llvm_enum}, ptr {slot}"));
        (v, enum_ty)
    }

    /// Materialize a tagged-enum value as a pointer (for match destructuring).
    /// If the scrutinee is already a place expression (Ident, Field, Index),
    /// return its slot directly. Otherwise compute the value into a temp
    /// alloca and return that pointer.
    fn enum_scrutinee_ptr(&mut self, scrutinee: &Expr) -> (String, EnumId) {
        // Try a place-form first.
        match &scrutinee.kind {
            ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                let (ptr, ty) = self.gen_place(scrutinee);
                let Ty::Enum(id) = ty else { unreachable!("sema validated") };
                (ptr, id)
            }
            _ => {
                let (val, ty) = self.gen_expr(scrutinee).expect("match scrutinee has value");
                let Ty::Enum(id) = ty.clone() else { unreachable!("sema validated") };
                let slot = self.alloca_anon(ty.clone());
                self.emit(&format!("store {} {val}, ptr {slot}", self.lty(&ty)));
                (slot, id)
            }
        }
    }

    fn gen_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Option<(String, Ty)> {
        let (scr_ptr, enum_id) = self.enum_scrutinee_ptr(scrutinee);
        let info = self.types.enum_defs[enum_id.0 as usize].clone();
        let llvm_enum = self.lty(&Ty::Enum(enum_id));

        // The result slot is allocated lazily: when the first arm body
        // produces an SSA value, we observe its type and alloca a slot for
        // the match result. All subsequent value-producing arms store into
        // the same slot. (`alloca` lives in entry block regardless of where
        // we emit the request, so creating it mid-function is fine.)
        let mut result_slot: Option<(String, Ty)> = None;

        // Load the tag once. Slice 1B: publish the tag's `[0, N)` range
        // metadata so `-O2`'s switch-simplifier and ConstraintElimination
        // can drop the default arm when sema's exhaustiveness check
        // already covered every variant.
        let n_variants = info.variants.len() as i64;
        let range_md = self.md.register_range(0, n_variants, "i32");
        let tag_val = {
            if info.is_tagged {
                let tag_ptr = self.next_tmp();
                self.emit(&format!(
                    "{tag_ptr} = getelementptr {llvm_enum}, ptr {scr_ptr}, i32 0, i32 0"
                ));
                let v = self.next_tmp();
                self.emit(&format!("{v} = load i32, ptr {tag_ptr}, !range !{range_md}"));
                v
            } else {
                // Plain enum: scrutinee is already an i32 tag value.
                let v = self.next_tmp();
                self.emit(&format!("{v} = load i32, ptr {scr_ptr}, !range !{range_md}"));
                v
            }
        };

        // Build labels per arm + a merge label.
        let merge_lbl = self.next_block_label();
        let mut arm_labels: Vec<String> = Vec::with_capacity(arms.len());
        for _ in arms { arm_labels.push(self.next_block_label()); }
        let default_lbl = self.next_block_label();

        // Find the catch-all arm (Wildcard or Binding) — its label becomes
        // the switch default. If absent, point default at `unreachable`.
        // Sema's exhaustiveness check has already verified the match covers
        // every variant or has a catch-all.
        let catchall_idx = arms.iter().position(|a| matches!(
            a.pattern.kind,
            PatternKind::Wildcard | PatternKind::Binding(_)
        ));
        let switch_default = match catchall_idx {
            Some(i) => arm_labels[i].clone(),
            None => default_lbl.clone(),
        };

        // Emit switch: one case per concrete variant arm.
        let mut cases = String::new();
        for (i, arm) in arms.iter().enumerate() {
            if let PatternKind::Variant { variant_name, .. } = &arm.pattern.kind {
                let tag = info.variants.get(&variant_name.name)
                    .copied().expect("sema validated variant");
                cases.push_str(&format!("    i32 {tag}, label %{}\n", arm_labels[i]));
            }
        }
        self.emit_terminator(&format!(
            "switch i32 {tag_val}, label %{switch_default} [\n{cases}  ]"
        ));

        // Emit each arm body.
        for (i, arm) in arms.iter().enumerate() {
            self.open_block(&arm_labels[i]);
            self.push_scope();
            // Bind pattern values into the arm scope.
            match &arm.pattern.kind {
                PatternKind::Wildcard => {}
                PatternKind::Binding(name) => {
                    // Bind the whole scrutinee to `name`. For an enum that's
                    // a load of the aggregate from the slot we already have.
                    let v = self.next_tmp();
                    self.emit(&format!("{v} = load {llvm_enum}, ptr {scr_ptr}"));
                    let local_slot = self.alloca_named(&name.name, Ty::Enum(enum_id));
                    self.emit(&format!(
                        "store {llvm_enum} {v}, ptr {local_slot}"
                    ));
                    self.bind(&name.name, local_slot, Ty::Enum(enum_id));
                }
                PatternKind::Variant { variant_name, payload, .. } => {
                    let tag = info.variants.get(&variant_name.name)
                        .copied().expect("sema validated variant");
                    let variant_payload_tys = info.variant_payloads.get(tag as usize)
                        .cloned().unwrap_or_default();
                    for (pi, pp) in payload.iter().enumerate() {
                        if let PatternKind::Binding(name) = &pp.kind {
                            let pty = variant_payload_tys.get(pi).cloned()
                                .unwrap_or(Ty::I32);
                            // GEP to the i64 payload slot, load as the
                            // payload's actual type.
                            let slot_ptr = self.next_tmp();
                            self.emit(&format!(
                                "{slot_ptr} = getelementptr {llvm_enum}, ptr {scr_ptr}, i32 0, i32 1, i64 {pi}"
                            ));
                            let v = self.next_tmp();
                            self.emit(&format!(
                                "{v} = load {}, ptr {slot_ptr}",
                                self.lty(&pty)
                            ));
                            let local_slot = self.alloca_named(&name.name, pty.clone());
                            self.emit(&format!(
                                "store {} {v}, ptr {local_slot}",
                                self.lty(&pty)
                            ));
                            self.bind(&name.name, local_slot, pty);
                        }
                        // Wildcard payload patterns bind nothing.
                    }
                }
            }
            // Emit the arm body. If it produces a value, lazily allocate
            // the result slot (on first value) and store the arm's value.
            let body_val = self.gen_expr(&arm.body);
            if let Some((v, ty)) = body_val {
                if result_slot.is_none() {
                    let s = self.alloca_anon(ty.clone());
                    result_slot = Some((s, ty.clone()));
                }
                let (rs, rt) = result_slot.as_ref().unwrap();
                self.emit(&format!("store {} {v}, ptr {rs}", self.lty(rt)));
            }
            self.pop_scope();
            if !self.terminated {
                self.emit_terminator(&format!("br label %{merge_lbl}"));
            }
        }

        // Default block (only reachable if no catch-all). Sema rejects
        // non-exhaustive matches, so this is dead code — emit `unreachable`
        // for completeness.
        if catchall_idx.is_none() {
            self.open_block(&default_lbl);
            self.emit_terminator("unreachable");
        }

        // Merge: load result if there is one.
        self.open_block(&merge_lbl);
        match &result_slot {
            Some((rs, rt)) => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = load {} , ptr {rs}", self.lty(rt)));
                Some((v, rt.clone()))
            }
            None => None,
        }
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
            // Phase 11: raw-pointer → raw-pointer reinterpretation.
            // Both ends lower to LLVM `ptr` (opaque pointer model), so the
            // cast is a no-op at the IR level — the SSA value is identical.
            // Return the existing value unchanged with the new Ty.
            (Ty::RawPtr(_), Ty::RawPtr(_)) => {
                return (v, to);
            }
            // Phase 11 / P3: integer → raw pointer. Sema gates on `unsafe`.
            // If the source integer is narrower than i64, zero-extend it first
            // (`inttoptr` requires its operand to match the target pointer
            // width, which is i64 on our supported 64-bit targets).
            (a, Ty::RawPtr(_)) if a.is_int() => {
                let aw = ty_bit_width(a);
                let widened: String = if aw < 64 {
                    let w = self.next_tmp();
                    let zext_inst = if a.is_signed_int() { "sext" } else { "zext" };
                    self.emit(&format!("{w} = {zext_inst} {from_t} {v} to i64"));
                    w
                } else {
                    v.clone()
                };
                self.emit(&format!("{r} = inttoptr i64 {widened} to {to_t}"));
                return (r, to);
            }
            _ => unreachable!("sema rejects unsupported casts: {:?} → {:?}", from, to),
        };
        self.emit(&format!("{r} = {inst} {from_t} {v} to {to_t}"));
        (r, to)
    }

    fn gen_call(&mut self, callee: &Expr, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        // Slice 11.FN_PTR: detect indirect calls. Two shapes:
        //   1. Callee is an Ident bound to a local of FnPtr type — load
        //      the pointer from the local's slot, then `call ret %ptr(args)`.
        //   2. Callee is a Field expression where the field's resolved type
        //      is FnPtr — load the pointer from the field address, then
        //      indirect-call. Same struct-of-callbacks pattern.
        // For both, the FnPtr's params/return give us the call signature.
        // Direct named calls (callee is an Ident matching a sig) fall through
        // to the existing gen_named_call path.
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some((slot, ty)) = self.lookup(name).cloned() {
                if let Ty::FnPtr { params, return_type } = ty {
                    let v = self.next_tmp();
                    self.emit(&format!("{v} = load ptr, ptr {slot}"));
                    return self.gen_indirect_call(&v, &params, &return_type, args);
                }
            }
        }
        if let ExprKind::Field { receiver, name } = &callee.kind {
            // Check if the field type is FnPtr — if so, indirect call.
            // gen_place returns the address; we load the pointer value.
            let (recv_addr, recv_ty) = self.gen_place(receiver);
            if let Ty::Struct(id) = recv_ty {
                let info = &self.types.struct_defs[id.0 as usize];
                if let Some((idx, ft)) = info.fields.iter().enumerate()
                    .find(|(_, (fname, _))| fname == &name.name)
                    .map(|(i, (_, t))| (i as u32, t.clone()))
                {
                    if matches!(ft, Ty::FnPtr { .. }) {
                        let Ty::FnPtr { params, return_type } = ft else { unreachable!() };
                        let llvm_struct = llvm_ty(&Ty::Struct(id), self.types);
                        let field_ptr = self.next_tmp();
                        self.emit(&format!(
                            "{field_ptr} = getelementptr inbounds {llvm_struct}, ptr {recv_addr}, i32 0, i32 {idx}"
                        ));
                        let fn_val = self.next_tmp();
                        self.emit(&format!("{fn_val} = load ptr, ptr {field_ptr}"));
                        return self.gen_indirect_call(&fn_val, &params, &return_type, args);
                    }
                }
            }
        }
        match &callee.kind {
            ExprKind::Ident(name) => self.gen_named_call(name, args, type_args),
            ExprKind::Field { receiver, name } => self.gen_method_call(receiver, name, args),
            ExprKind::Path { segments } => self.gen_assoc_call(segments, args),
            _ => unreachable!("sema validates callee shape"),
        }
    }

    /// Slice 11.FN_PTR: indirect call through an SSA pointer value.
    /// Lowers each arg, emits `call <retty> <ptr>(<args>)`. Mirrors the
    /// shape of `gen_named_call` for the per-arg lowering but uses the
    /// callee's `%v` SSA name instead of `@<name>`.
    fn gen_indirect_call(
        &mut self,
        callee_val: &str,
        params: &[Ty],
        return_type: &Ty,
        args: &[Expr],
    ) -> Option<(String, Ty)> {
        // Evaluate each arg to a value. FnPtr params are always value-passed
        // (no `mut`/`move` machinery at the call site — those are call-site
        // contracts, not type-level facts; the fn-pointer abstraction
        // erases them). All callable signatures here are C-ABI (ccc).
        let mut arg_vals: Vec<(String, String)> = Vec::with_capacity(args.len());
        for (a, pty) in args.iter().zip(params.iter()) {
            let (v, _) = self.gen_expr(a).expect("indirect call arg has value");
            arg_vals.push((v, self.lty(pty)));
        }
        let mut arg_str = String::new();
        for (i, (v, t)) in arg_vals.iter().enumerate() {
            if i > 0 { arg_str.push_str(", "); }
            arg_str.push_str(&format!("{t} {v}"));
        }
        match return_type {
            Ty::Unit => {
                self.emit(&format!("call void {callee_val}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = call {} {callee_val}({arg_str})", self.lty(ret)));
                Some((v, ret.clone()))
            }
        }
    }

    fn gen_named_call(&mut self, name: &str, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        // Special case: println(i32) → call printf with our %d\n format.
        // Phase 8 slice 8.STR.2: also handle println(str) by extracting
        // (ptr, len) from the fat-pointer value and passing both to
        // printf with the `%.*s\n` format string.
        if name == "println" {
            let (av, aty) = self.gen_expr(&args[0]).expect("println arg");
            let v = self.next_tmp();
            match aty {
                Ty::Str => {
                    // Extract ptr (field 0) and len (field 1).
                    let ptr_tmp = self.next_tmp();
                    let len_tmp = self.next_tmp();
                    self.emit(&format!(
                        "{ptr_tmp} = extractvalue {{ ptr, i64 }} {av}, 0"
                    ));
                    self.emit(&format!(
                        "{len_tmp} = extractvalue {{ ptr, i64 }} {av}, 1"
                    ));
                    // printf's `%.*s` takes (int width, ptr); width is i32.
                    let len_i32 = self.next_tmp();
                    self.emit(&format!(
                        "{len_i32} = trunc i64 {len_tmp} to i32"
                    ));
                    self.emit(&format!(
                        "{v} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_str_nl, i32 {len_i32}, ptr {ptr_tmp})"
                    ));
                }
                _ => {
                    self.emit(&format!(
                        "{v} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_int_nl, i32 {av})"
                    ));
                }
            }
            return None;
        }
        // Slice 10.FFI.2: `str_ptr(s)` and `str_len(s)` intrinsics.
        // Lower to `extractvalue` from the `{ ptr, i64 }` fat pointer.
        if name == "str_ptr" {
            let (av, _) = self.gen_expr(&args[0]).expect("str_ptr arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 0"));
            return Some((r, Ty::RawPtr(Box::new(Ty::U8))));
        }
        if name == "str_len" {
            let (av, _) = self.gen_expr(&args[0]).expect("str_len arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 1"));
            return Some((r, Ty::Usize));
        }
        if name == "str_from_raw_parts" {
            let (p_val, _) = self.gen_expr(&args[0]).expect("str_from_raw_parts ptr");
            let (n_val, _) = self.gen_expr(&args[1]).expect("str_from_raw_parts len");
            let t1 = self.next_tmp();
            let t2 = self.next_tmp();
            self.emit(&format!("{t1} = insertvalue {{ ptr, i64 }} undef, ptr {p_val}, 0"));
            self.emit(&format!("{t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {n_val}, 1"));
            return Some((t2, Ty::Str));
        }
        // Phase 11 polish (2026-05-14): slice intrinsics. Lower exactly
        // like the str equivalents — same `{ ptr, i64 }` aggregate
        // shape — but propagate the element type for the returned Ty.
        if name == "slice_ptr" {
            let (av, ty) = self.gen_expr(&args[0]).expect("slice_ptr arg");
            let elem_ty = match ty {
                Ty::Slice(inner) => *inner,
                _ => unreachable!("sema validated slice_ptr arg type"),
            };
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 0"));
            return Some((r, Ty::RawPtr(Box::new(elem_ty))));
        }
        // Phase 3A: byte-swap intrinsics. `bswap{16,32,64}` lower directly
        // to `llvm.bswap.i{16,32,64}`. `htons`/`htonl`/`ntohs`/`ntohl` are
        // aliases — on little-endian targets (every C+ target today) they
        // bswap; on a future big-endian target we'd lower as identity.
        if let Some((bits, ret_ty)) = match name {
            "bswap16" | "htons" | "ntohs" => Some((16u32, Ty::U16)),
            "bswap32" | "htonl" | "ntohl" => Some((32u32, Ty::U32)),
            "bswap64"                     => Some((64u32, Ty::U64)),
            _ => None,
        } {
            let (av, _) = self.gen_expr(&args[0]).expect("bswap arg");
            let r = self.next_tmp();
            self.emit(&format!(
                "{r} = call i{bits} @llvm.bswap.i{bits}(i{bits} {av})"
            ));
            return Some((r, ret_ty));
        }
        if name == "slice_len" {
            let (av, _) = self.gen_expr(&args[0]).expect("slice_len arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 1"));
            // Slice 1B: publish the proven invariant that slice lengths are
            // non-negative (signed). LLVM `!range` metadata is only legal
            // on `load`/`call`/`invoke`/`atomicrmw`/`cmpxchg` — not on
            // `extractvalue` — so we publish the fact via `llvm.assume`
            // instead. At -O2 the optimizer rewrites this into range
            // metadata after the value is rematerialized through a load.
            let nn = self.next_tmp();
            self.emit(&format!("{nn} = icmp sge i64 {r}, 0"));
            self.emit(&format!("call void @llvm.assume(i1 {nn})"));
            return Some((r, Ty::Usize));
        }
        if name == "slice_from_raw_parts" {
            let (p_val, p_ty) = self.gen_expr(&args[0]).expect("slice_from_raw_parts ptr");
            let (n_val, _) = self.gen_expr(&args[1]).expect("slice_from_raw_parts len");
            let elem_ty = match p_ty {
                Ty::RawPtr(inner) => *inner,
                _ => unreachable!("sema validated slice_from_raw_parts ptr type"),
            };
            let t1 = self.next_tmp();
            let t2 = self.next_tmp();
            self.emit(&format!("{t1} = insertvalue {{ ptr, i64 }} undef, ptr {p_val}, 0"));
            self.emit(&format!("{t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {n_val}, 1"));
            return Some((t2, Ty::Slice(Box::new(elem_ty))));
        }
        // Phase 11 slice 11.LAYOUT: `size_of[T]()` and `align_of[T]()`.
        // The GEP-null trick gives a constant the LLVM optimizer folds
        // at -O1+. At -O0 it becomes a real two-instruction sequence
        // (getelementptr + ptrtoint) that returns the layout value.
        //
        // size_of:  ptrtoint (getelementptr T, ptr null, i64 1) to i64
        // align_of: ptrtoint (getelementptr {i1, T}, ptr null, i64 0, i32 1) to i64
        //
        // The align_of trick exploits LLVM's struct layout: in `{i1, T}`,
        // T starts at the alignment boundary of T after the i1's 1-byte
        // storage + padding, so the offset of T is exactly alignof(T).
        if name == "size_of" {
            let t = ty_from(&type_args[0], &self.types);
            let llvm_t = llvm_ty(&t, &self.types);
            let ptr_tmp = self.next_tmp();
            let int_tmp = self.next_tmp();
            self.emit(&format!("{ptr_tmp} = getelementptr {llvm_t}, ptr null, i64 1"));
            self.emit(&format!("{int_tmp} = ptrtoint ptr {ptr_tmp} to i64"));
            return Some((int_tmp, Ty::Usize));
        }
        if name == "align_of" {
            let t = ty_from(&type_args[0], &self.types);
            let llvm_t = llvm_ty(&t, &self.types);
            let ptr_tmp = self.next_tmp();
            let int_tmp = self.next_tmp();
            self.emit(&format!(
                "{ptr_tmp} = getelementptr {{ i1, {llvm_t} }}, ptr null, i64 0, i32 1"
            ));
            self.emit(&format!("{int_tmp} = ptrtoint ptr {ptr_tmp} to i64"));
            return Some((int_tmp, Ty::Usize));
        }
        let sig = self.sigs.get(name).expect("sema validated function exists").clone();
        // Per-arg lowering. `arg_vals[i]` is (ssa-value, llvm-type-string).
        // For pointer-passed `mut x: T` params we take the address of the
        // source place; for value-passed params we evaluate the value and
        // flip the source's drop flag on a `move`.
        let mut arg_vals: Vec<(String, String)> = Vec::with_capacity(args.len());
        // Fixed (declared) params first.
        for (a, (pty, move_flag, mut_flag)) in args.iter().zip(sig.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                arg_vals.push((addr, "ptr".to_string()));
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg is a value");
                arg_vals.push((v, self.lty(pty)));
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        // Slice 10.FFI.4: variadic tail args. Each tail arg evaluated
        // at its natural type and passed by value (the C varargs ABI).
        // No `move` semantics — varargs are inherently bit-copies.
        if sig.is_variadic {
            for a in args.iter().skip(sig.params.len()) {
                let (v, ty) = self.gen_expr(a).expect("varargs tail arg has value");
                arg_vals.push((v, self.lty(&ty)));
            }
        }
        let mut arg_str = String::new();
        for (i, (v, ty)) in arg_vals.iter().enumerate() {
            if i > 0 { arg_str.push_str(", "); }
            arg_str.push_str(&format!("{ty} {v}"));
        }
        // Slice 10.FFI.4: LLVM requires the full function type for
        // variadic call sites. `call retty (fixed_types, ...) @name(args)`.
        let type_prefix = if sig.is_variadic {
            let mut s = String::from(" (");
            for (i, (pty, _, _)) in sig.params.iter().enumerate() {
                if i > 0 { s.push_str(", "); }
                s.push_str(&self.lty(pty));
            }
            if !sig.params.is_empty() { s.push_str(", "); }
            s.push_str("...)");
            s
        } else {
            String::new()
        };
        // Phase 11 / ObjC interop: `#[link_name = "..."]` aliases the
        // linker symbol. Call sites use the link_name when present so the
        // call resolves to the same C symbol as the user wrote in the
        // attribute, not the C+ source-level name.
        let symbol: &str = sig.link_name.as_deref().unwrap_or(name);
        // Slice 1E: tail-call optimization. `pending_musttail` is set by
        // `StmtKind::Return` when this call is the last expression before
        // a return and the signatures match. LLVM's verifier rejects
        // `musttail` IR that doesn't truly qualify (e.g. variadic
        // mismatch), so the predicate in StmtKind::Return is conservative.
        let want_musttail = self.pending_musttail;
        self.pending_musttail = false;
        // Slice 1D: detect sret callee. Only applies when the callee is
        // user-defined (sig has no link_name pointing at a C symbol and
        // the callee is non-variadic) and the return type triggers the
        // predicate. The current narrow predicate fires only for `string`
        // returns (24-byte aggregate with Drop).
        let uses_sret = !sig.is_variadic
            && sig.link_name.is_none()
            && return_passes_by_sret(&sig.return_type);
        if uses_sret {
            // musttail + sret would require the caller's own sret slot to
            // be forwarded as the callee's sret arg. Supported when caller
            // and callee both use sret with matching types; the predicate
            // in StmtKind::Return already verified return-type equality.
            let ret = sig.return_type.clone();
            let lty = self.lty(&ret);
            if want_musttail {
                if let Some(caller_slot) = self.sret_slot.clone() {
                    // Forward caller's sret slot into the callee. After
                    // `musttail call void @foo(ptr %caller_slot, ...)` the
                    // function's `ret void` will see the value already
                    // landed at the caller's caller's slot.
                    let mut head = format!("ptr {caller_slot}");
                    if !arg_str.is_empty() { head.push_str(", "); head.push_str(&arg_str); }
                    self.emit(&format!("musttail call void{type_prefix} @{symbol}({head})"));
                    // Return type signaled to upstream — but musttail in
                    // tail position is always followed by `ret void`
                    // emitted by StmtKind::Return. We must NOT supply a
                    // value; emit the terminator now and return None so
                    // StmtKind::Return's value path becomes a no-op.
                    // (StmtKind::Return reads `ret_val` only — passing it
                    // None lands in the (None, Ty::Unit) arm... but
                    // ret_ty is `string`, not Unit. Simpler: emit the
                    // terminator + signal terminated.)
                    self.emit_terminator("ret void");
                    return None;
                }
                // No caller sret — can't forward. Fall through to
                // non-musttail sret call.
            }
            let slot = self.alloca_anon(ret.clone());
            let mut head = format!("ptr {slot}");
            if !arg_str.is_empty() { head.push_str(", "); head.push_str(&arg_str); }
            self.emit(&format!("call void{type_prefix} @{symbol}({head})"));
            let v = self.next_tmp();
            self.emit(&format!("{v} = load {lty}, ptr {slot}"));
            return Some((v, ret));
        }
        let call_kind = if want_musttail { "musttail call" } else { "call" };
        match sig.return_type {
            Ty::Unit => {
                self.emit(&format!("{call_kind} void{type_prefix} @{symbol}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = {call_kind} {}{type_prefix} @{symbol}({arg_str})", self.lty(&ret)));
                Some((v, ret))
            }
        }
    }

    fn gen_method_call(&mut self, receiver: &Expr, name: &Ident, args: &[Expr]) -> Option<(String, Ty)> {
        // Phase 8 slice 8.STR.6: blessed `to_string()` on primitives + `str`.
        // The receiver is a primitive value, not a place — handle before
        // gen_place (which expects a place-producing expression).
        if name.name == "to_string" && args.is_empty() {
            let (rv, rt) = self.gen_expr(receiver).expect("to_string receiver has value");
            if Self::is_blessed_to_string_receiver_codegen(&rt) {
                return Some(self.gen_to_string_intrinsic(&rv, &rt));
            }
        }
        // Materialize the receiver as a place (pointer) — works for Ident,
        // Field chains, and value-producing temporaries (gen_place handles each).
        let (recv_ptr, recv_ty) = self.gen_place(receiver);
        // Phase 8 slice 8.STR.3: blessed methods on `string` are
        // intrinsic — no MethodSig lookup, no mangled-name call.
        if matches!(recv_ty, Ty::String) {
            return Some(self.gen_string_method_call(&recv_ptr, &name.name, args));
        }
        let Ty::Struct(id) = recv_ty else { unreachable!("sema validated") };
        let struct_name = self.types.struct_defs[id.0 as usize].name.clone();
        let info = self.types.struct_defs[id.0 as usize]
            .methods.get(&name.name).expect("sema validated").clone();
        let rcv = info.receiver.expect("sema validated instance call");
        let mangled = mangle(&struct_name, &name.name);

        // Build the LLVM call argument list. All three receiver kinds
        // (`self`, `mut self`, `move self`) pass the struct's address as a
        // `ptr`; the receiver kind only matters for sema-level mutability
        // and move-tracking checks.
        let mut arg_parts: Vec<String> = vec![format!("ptr {recv_ptr}")];
        for (a, (pty, move_flag, mut_flag)) in args.iter().zip(info.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                arg_parts.push(format!("ptr {addr}"));
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg has value");
                arg_parts.push(format!("{} {v}", self.lty(pty)));
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        let arg_str = arg_parts.join(", ");

        // `move self` consumes the receiver: flip its drop flag if the
        // receiver expression was a plain Ident bound as a Drop value.
        if matches!(rcv, Receiver::Move) {
            if let ExprKind::Ident(name) = &receiver.kind {
                self.mark_moved(name);
            }
        }

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
        // Sema verified `Type::method` is either an associated function
        // (struct path) or a tagged-enum variant constructor (enum path).
        // Dispatch on the type-segment's kind.
        let type_name = &segments[0].name;
        let method_name = &segments[1].name;
        // Phase 8 slice 8.STR.3: blessed string assoc fns.
        if type_name == "string" {
            return Some(self.gen_string_assoc_call(method_name, args));
        }
        if let Some(&enum_id) = self.types.enum_by_name.get(type_name) {
            // Tagged-enum variant construction with payload.
            let info = &self.types.enum_defs[enum_id.0 as usize];
            let tag = *info.variants.get(method_name).expect("sema validated variant");
            let mut payload_vals: Vec<(String, Ty)> = Vec::new();
            for a in args {
                let (v, t) = self.gen_expr(a).expect("variant payload has value");
                payload_vals.push((v, t));
            }
            let (v, ty) = self.gen_tagged_construct(enum_id, tag, &payload_vals);
            return Some((v, ty));
        }
        let id = *self.types.struct_by_name.get(type_name).expect("sema validated");
        let info = self.types.struct_defs[id.0 as usize]
            .methods.get(method_name).expect("sema validated").clone();
        let mangled = mangle(type_name, method_name);

        let mut arg_parts: Vec<String> = Vec::new();
        for (a, (pty, move_flag, mut_flag)) in args.iter().zip(info.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                arg_parts.push(format!("ptr {addr}"));
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg has value");
                arg_parts.push(format!("{} {v}", self.lty(pty)));
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
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

    // ---------- Phase 8 slice 8.STR.B.2: interpolation codegen ----------
    //
    // Lowers `"hello ${name}, n is ${n}"` to:
    //   1. For each Lit part: lookup the @.str.N global, take (ptr, len).
    //   2. For each Expr part: evaluate. If primitive/str, invoke the
    //      blessed to_string intrinsic to produce a `string`. If
    //      already `string`, use as-is.
    //   3. Compute total length = sum of all part lengths.
    //   4. malloc(total).
    //   5. memcpy each part's bytes into the buffer at the running offset.
    //   6. Build the result aggregate `{ buf, total, total }`.
    //
    // v1 leak: any Expr-derived `string`'s buffer is malloc'd inside
    // to_string and never freed. Matches the broader 8.STR.3 leak
    // policy; Drop integration is a follow-up.

    fn gen_interp_str(&mut self, parts: &[crate::ast::InterpStrPart]) -> (String, Ty) {
        use crate::ast::InterpStrPart;
        // First pass: produce a (ptr, len) pair per part.
        let mut piece_ptrs: Vec<String> = Vec::with_capacity(parts.len());
        let mut piece_lens: Vec<String> = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                InterpStrPart::Lit(s) => {
                    let (symbol, len) = self.str_lits.get(s)
                        .expect("interp lit part missing from str_lits table")
                        .clone();
                    piece_ptrs.push(symbol);
                    piece_lens.push(format!("{len}"));
                }
                InterpStrPart::Expr(e) => {
                    let (v, t) = self.gen_expr(e).expect("interp expr has value");
                    // Convert to a string aggregate.
                    let (sv, _) = match t {
                        Ty::String => (v, Ty::String),
                        Ty::Str => self.gen_to_string_str(&v),
                        Ty::Bool => self.gen_to_string_bool(&v),
                        Ty::F32 | Ty::F64 => self.gen_to_string_float(&v, &t),
                        ref rt if rt.is_signed_int() => self.gen_to_string_signed(&v, rt),
                        ref rt if rt.is_unsigned_int() => self.gen_to_string_unsigned(&v, rt),
                        _ => unreachable!("sema validated interp expr type"),
                    };
                    // Extract ptr+len.
                    let pp = self.next_tmp();
                    self.emit(&format!("{pp} = extractvalue {{ ptr, i64, i64 }} {sv}, 0"));
                    let lp = self.next_tmp();
                    self.emit(&format!("{lp} = extractvalue {{ ptr, i64, i64 }} {sv}, 1"));
                    piece_ptrs.push(pp);
                    piece_lens.push(lp);
                }
            }
        }
        // Compute total length via accumulating adds.
        let mut total = String::from("0");
        for l in &piece_lens {
            let next = self.next_tmp();
            self.emit(&format!("{next} = add i64 {total}, {l}"));
            total = next;
        }
        // Allocate the output buffer.
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {total})"));
        // memcpy each piece at the running offset.
        let mut offset = String::from("0");
        for (ptr, len) in piece_ptrs.iter().zip(piece_lens.iter()) {
            let dst = self.next_tmp();
            self.emit(&format!("{dst} = getelementptr i8, ptr {buf}, i64 {offset}"));
            let _cpy = self.next_tmp();
            self.emit(&format!("{_cpy} = call ptr @memcpy(ptr {dst}, ptr {ptr}, i64 {len})"));
            let next_off = self.next_tmp();
            self.emit(&format!("{next_off} = add i64 {offset}, {len}"));
            offset = next_off;
        }
        let v = self.string_aggregate(&buf, &total, &total);
        (v, Ty::String)
    }

    // ---------- Phase 8 slice 8.STR.6: blessed `to_string()` ----------

    fn is_blessed_to_string_receiver_codegen(ty: &Ty) -> bool {
        matches!(ty,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool
            | Ty::Str)
    }

    /// Emit IR that produces a `string` aggregate for an arbitrary
    /// primitive (or `str`) receiver value. Strategy:
    ///   - signed int: sign-extend to i64, snprintf("%lld") into a
    ///     32-byte malloc'd buffer, take snprintf's returned length.
    ///   - unsigned int: zero-extend to i64, snprintf("%llu").
    ///   - f32: fpext to f64, snprintf("%g"). f64: direct snprintf.
    ///   - bool: branch on the i1, malloc 4/5 bytes, memcpy "true"/"false".
    ///   - str: extract ptr+len from the fat-pointer, malloc(len),
    ///     memcpy. The result owns the bytes; old bytes untouched.
    fn gen_to_string_intrinsic(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        match rt {
            Ty::Bool => self.gen_to_string_bool(rv),
            Ty::Str => self.gen_to_string_str(rv),
            Ty::F32 | Ty::F64 => self.gen_to_string_float(rv, rt),
            _ if rt.is_signed_int() => self.gen_to_string_signed(rv, rt),
            _ if rt.is_unsigned_int() => self.gen_to_string_unsigned(rv, rt),
            _ => unreachable!("sema validated to_string receiver: {:?}", rt),
        }
    }

    fn gen_to_string_signed(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        // Widen to i64 for the format spec.
        let widened = match rt {
            Ty::I64 | Ty::Isize => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = sext {} {rv} to i64", self.lty(rt)));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_i64, i64 {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_unsigned(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        let widened = match rt {
            Ty::U64 | Ty::Usize => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = zext {} {rv} to i64", self.lty(rt)));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_u64, i64 {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_float(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        // Widen f32 → f64 for "%g".
        let widened = match rt {
            Ty::F64 => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = fpext float {rv} to double"));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_f64, double {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_bool(&mut self, rv: &str) -> (String, Ty) {
        // Avoid the branch entirely: select between the two static
        // pointers and lengths. The buffer must still be owned (callers
        // can later `free` it), so unconditionally malloc 5 bytes
        // (covers both `"true"` and `"false"`), pick which static blob
        // and how many bytes to copy via `select`, then memcpy.
        let len = self.next_tmp();
        self.emit(&format!("{len} = select i1 {rv}, i64 4, i64 5"));
        let src = self.next_tmp();
        self.emit(&format!("{src} = select i1 {rv}, ptr @.bool_true, ptr @.bool_false"));
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {len})"));
        let _cpy = self.next_tmp();
        self.emit(&format!("{_cpy} = call ptr @memcpy(ptr {buf}, ptr {src}, i64 {len})"));
        let v = self.string_aggregate(&buf, &len, &len);
        (v, Ty::String)
    }

    fn gen_to_string_str(&mut self, rv: &str) -> (String, Ty) {
        // Extract ptr+len from the str fat-pointer, malloc(len), memcpy.
        let src_ptr = self.next_tmp();
        self.emit(&format!("{src_ptr} = extractvalue {{ ptr, i64 }} {rv}, 0"));
        let len = self.next_tmp();
        self.emit(&format!("{len} = extractvalue {{ ptr, i64 }} {rv}, 1"));
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {len})"));
        let _cpy = self.next_tmp();
        self.emit(&format!("{_cpy} = call ptr @memcpy(ptr {buf}, ptr {src_ptr}, i64 {len})"));
        let v = self.string_aggregate(&buf, &len, &len);
        (v, Ty::String)
    }

    // ---------- Phase 8 slice 8.STR.3: owned `string` intrinsics ----------
    //
    // The `string` runtime value is a 24-byte struct `{ ptr, i64, i64 }` —
    // (data pointer, length in bytes, capacity in bytes). Stored by value
    // in locals; produced as an LLVM aggregate via `insertvalue` so the
    // call site doesn't need to allocate a separate slot.
    //
    // Drop is NOT integrated in this initial cut: `string` locals leak
    // their buffer at scope exit. Wiring Drop alongside the existing
    // struct-Drop machinery is a follow-up slice — see plan.md resolved-log.

    /// Build a `string` SSA value from three components.
    fn string_aggregate(&mut self, ptr: &str, len: &str, cap: &str) -> String {
        let v0 = self.next_tmp();
        self.emit(&format!("{v0} = insertvalue {{ ptr, i64, i64 }} undef, ptr {ptr}, 0"));
        let v1 = self.next_tmp();
        self.emit(&format!("{v1} = insertvalue {{ ptr, i64, i64 }} {v0}, i64 {len}, 1"));
        let v2 = self.next_tmp();
        self.emit(&format!("{v2} = insertvalue {{ ptr, i64, i64 }} {v1}, i64 {cap}, 2"));
        v2
    }

    /// `string::new()` — empty string. `ptr=null, len=0, cap=0`. No heap.
    /// `string::with_capacity(n)` — `malloc(n)` buffer, `len=0, cap=n`.
    fn gen_string_assoc_call(&mut self, method: &str, args: &[Expr]) -> (String, Ty) {
        match method {
            "new" => {
                let _ = args;
                let v = self.string_aggregate("null", "0", "0");
                (v, Ty::String)
            }
            "with_capacity" => {
                let (n, _) = self.gen_expr(&args[0]).expect("with_capacity arg has value");
                let buf = self.next_tmp();
                self.emit(&format!("{buf} = call ptr @malloc(i64 {n})"));
                let v = self.string_aggregate(&buf, "0", &n);
                (v, Ty::String)
            }
            _ => unreachable!("sema validated method `string::{method}`"),
        }
    }

    /// Methods on a `string` receiver. The receiver is materialized as a
    /// `ptr` to the local slot (24-byte aggregate); we load whichever
    /// fields the method needs via `getelementptr`/`load`.
    fn gen_string_method_call(&mut self, recv_ptr: &str, method: &str, args: &[Expr]) -> (String, Ty) {
        let _ = args; // every v1 method is zero-arg
        match method {
            "len" => {
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.emit(&format!("{lv} = load i64, ptr {lp}"));
                (lv, Ty::Usize)
            }
            "is_empty" => {
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.emit(&format!("{lv} = load i64, ptr {lp}"));
                let cmp = self.next_tmp();
                self.emit(&format!("{cmp} = icmp eq i64 {lv}, 0"));
                (cmp, Ty::Bool)
            }
            "as_str" => {
                // Extract ptr + len; package as `str` fat-pointer `{ ptr, i64 }`.
                let pp = self.next_tmp();
                self.emit(&format!("{pp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 0"));
                let pv = self.next_tmp();
                self.emit(&format!("{pv} = load ptr, ptr {pp}"));
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.emit(&format!("{lv} = load i64, ptr {lp}"));
                let s0 = self.next_tmp();
                self.emit(&format!("{s0} = insertvalue {{ ptr, i64 }} undef, ptr {pv}, 0"));
                let s1 = self.next_tmp();
                self.emit(&format!("{s1} = insertvalue {{ ptr, i64 }} {s0}, i64 {lv}, 1"));
                (s1, Ty::Str)
            }
            "clone" => {
                // Load len, malloc a fresh buffer of size len (cap = len in
                // the clone), memcpy bytes, build a new aggregate.
                let pp = self.next_tmp();
                self.emit(&format!("{pp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 0"));
                let pv = self.next_tmp();
                self.emit(&format!("{pv} = load ptr, ptr {pp}"));
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.emit(&format!("{lv} = load i64, ptr {lp}"));
                let buf = self.next_tmp();
                self.emit(&format!("{buf} = call ptr @malloc(i64 {lv})"));
                let _cpy = self.next_tmp();
                self.emit(&format!("{_cpy} = call ptr @memcpy(ptr {buf}, ptr {pv}, i64 {lv})"));
                let v = self.string_aggregate(&buf, &lv, &lv);
                (v, Ty::String)
            }
            _ => unreachable!("sema validated `string.{method}`"),
        }
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
            | BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap
            | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor
            | BinOp::Shl | BinOp::Shr => expr_value_ty(lhs),
            _ => Some(Ty::Bool),
        },
        ExprKind::Unary { op, operand } => match op {
            UnaryOp::Neg | UnaryOp::BitNot => expr_value_ty(operand),
            UnaryOp::Not => Some(Ty::Bool),
            _ => None,
        },
        // Path always names an enum variant, and every enum lowers to `i32`.
        // The exact `EnumId` matters for sema but not for codegen's slot
        // allocation, so we report `i32` here. (Sema has already verified
        // both arms of any `if` agree on the actual enum type.)
        ExprKind::Path { .. } => Some(Ty::I32),
        // Cast: target type is directly visible. Resolve primitives by
        // name (we don't have the TypeTable here, so aggregates return
        // None and the result-slot machinery falls back). This unblocks
        // if-expressions whose arms are `... as usize` / `... as *T` /
        // etc. — previously returned None and the if produced no value.
        ExprKind::Cast { ty, .. } => match &ty.kind {
            crate::ast::TypeKind::Path(name) => match name.as_str() {
                "i8" => Some(Ty::I8), "i16" => Some(Ty::I16),
                "i32" => Some(Ty::I32), "i64" => Some(Ty::I64),
                "u8" => Some(Ty::U8), "u16" => Some(Ty::U16),
                "u32" => Some(Ty::U32), "u64" => Some(Ty::U64),
                "isize" => Some(Ty::Isize), "usize" => Some(Ty::Usize),
                "f32" => Some(Ty::F32), "f64" => Some(Ty::F64),
                "bool" => Some(Ty::Bool),
                _ => None,
            },
            crate::ast::TypeKind::RawPtr(inner) => {
                // Recover the pointee for `Ty::RawPtr` so two `as *T` casts
                // produce the same Ty key for slot allocation.
                expr_value_ty(&Expr {
                    kind: ExprKind::Cast {
                        expr: Box::new(Expr { kind: ExprKind::BoolLit(false), span: e.span }),
                        ty: (**inner).clone(),
                    },
                    span: e.span,
                }).map(|t| Ty::RawPtr(Box::new(t)))
            }
            _ => None,
        },
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

    // ---- Phase 8 slice 8.STR.1–.3: strings ----

    #[test]
    fn str_literal_emits_global_constant() {
        // Each unique literal gets a `@.str.N = private unnamed_addr constant`.
        let ir = gen_src("fn main() -> i32 { let s: str = \"hi\"; return 0; }");
        assert!(ir.contains("@.str.0 = private unnamed_addr constant"),
            "expected @.str.0 global, got:\n{ir}");
        // Bytes plus NUL: 2 + 1 = 3.
        assert!(ir.contains("[3 x i8] c\"hi\\00\""),
            "expected NUL-terminated payload, got:\n{ir}");
    }

    #[test]
    fn str_literal_inside_loop_block_collected() {
        // Phase 3B regression guard (2026-05-15): the str-literal pre-pass
        // used to skip plain `loop { ... }` statements, so any literal
        // inside one tripped a codegen `expect` at use time. Walk it.
        let ir = gen_src(
            "fn main() -> i32 { loop { let s: str = \"x\"; break; } return 0; }"
        );
        assert!(ir.contains("@.str.0 = private unnamed_addr constant"),
            "expected @.str.0 to be emitted for the loop-body literal, got:\n{ir}");
    }

    #[test]
    fn str_literal_dedupes_by_content() {
        // Two uses of the same literal share one global.
        let ir = gen_src(
            "fn main() -> i32 { let a: str = \"x\"; let b: str = \"x\"; return 0; }"
        );
        let count = ir.matches("@.str.0 = private unnamed_addr").count();
        assert_eq!(count, 1, "expected one @.str.0 declaration");
        // No @.str.1 should appear from the second use of the same literal.
        assert!(!ir.contains("@.str.1 = private unnamed_addr"),
            "expected dedup, second literal not to allocate a new symbol");
    }

    #[test]
    fn str_value_builds_fat_pointer() {
        // The literal expression's SSA value is an `insertvalue` chain
        // into `{ ptr, i64 }`.
        let ir = gen_src(
            "fn main() -> i32 { let s: str = \"ab\"; return 0; }"
        );
        assert!(ir.contains("insertvalue { ptr, i64 } undef, ptr @.str.0, 0"));
        assert!(ir.contains("insertvalue { ptr, i64 }"));
        // Length stored is 2 (bytes), not 3 (including NUL).
        assert!(ir.contains("i64 2, 1"));
    }

    #[test]
    fn println_str_uses_dotstar_format() {
        // Slice 8.STR.2: `println(str)` lowers to printf with `%.*s\n`.
        let ir = gen_src("fn main() -> i32 { println(\"hi\"); return 0; }");
        assert!(ir.contains("@.fmt_str_nl"));
        assert!(ir.contains("call i32 (ptr, ...) @printf(ptr noundef @.fmt_str_nl, i32"));
    }

    #[test]
    fn str_equality_uses_memcmp() {
        // Slice 8.STR.3: `==` on `str` lowers to a length-prechecked
        // memcmp call.
        let ir = gen_src(
            "fn main() -> i32 { if \"a\" == \"a\" { return 0; } return 1; }"
        );
        assert!(ir.contains("declare i32 @memcmp(ptr, ptr, i64)"));
        assert!(ir.contains("call i32 @memcmp(ptr"));
    }

    #[test]
    fn str_escape_sequences_in_global() {
        // `\n` in source becomes a real newline byte in the global blob,
        // encoded in the IR as `\0A`.
        let ir = gen_src("fn main() -> i32 { println(\"a\\nb\"); return 0; }");
        assert!(ir.contains("\\0A"), "expected newline byte (\\0A) in IR, got:\n{ir}");
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

    // ---- Phase 5 slice 5BC.codegen — §2.9 mut-borrow pointer ABI ----
    //
    // `mut x: T` on a non-Copy struct is an exclusive borrow per §2.9: the
    // callee's writes must propagate back to the caller's place. Codegen
    // lowers the parameter to a `ptr` and the call site takes the source's
    // address, so the callee operates on the caller's storage directly.
    //
    // Copy types, `move`-marked params, and shared (`x: T`) params are
    // unaffected — they stay value-passed.

    #[test]
    fn mut_param_noncopy_struct_lowers_to_ptr_noalias() {
        // Slice 6BC.codegen: Drop forces non-Copy. `bump` takes
        // `mut t: Tag` as `ptr noalias` — the borrow checker proves
        // uniqueness, so LLVM gets the strong promise.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }"
        );
        assert!(
            ir.contains("define void @bump(ptr noalias "),
            "expected `mut t: Tag` to lower to `ptr noalias` param, got: {ir}"
        );
        // Call site still passes a pointer, not a struct value.
        assert!(
            ir.contains("call void @bump(ptr "),
            "expected call site to pass ptr for non-Copy mut arg, got: {ir}"
        );
    }

    #[test]
    fn shared_param_noncopy_struct_lowers_to_ptr_readonly() {
        // Slice 6BC.codegen: non-Copy shared `x: T` is now
        // pointer-passed (avoids the byte-copy) and tagged
        // `readonly` (callee provably can't write). `noalias` would
        // be unsound — two shared args can be the same place.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn peek(t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 7 }; return peek(x); }"
        );
        assert!(
            ir.contains("define i32 @peek(ptr readonly "),
            "expected `t: Tag` to lower to `ptr readonly` param, got: {ir}"
        );
        assert!(
            ir.contains("call i32 @peek(ptr "),
            "expected call site to pass ptr for non-Copy shared arg, got: {ir}"
        );
    }

    #[test]
    fn move_param_noncopy_struct_stays_value_passed() {
        // `move x: T` transfers ownership; the by-value LLVM signature is
        // correct (callee owns the bytes and registers a scope-exit drop;
        // caller's drop-flag is flipped at the call site).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn take(move t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 9 }; return take(x); }"
        );
        assert!(
            ir.contains("define i32 @take(%Tag "),
            "expected `move t: Tag` to stay struct-by-value, got: {ir}"
        );
    }

    #[test]
    fn mut_param_copy_struct_stays_value_passed() {
        // Copy structs: `mut p: P` is local mutability per §2.9, not an
        // exclusive borrow. The LLVM signature must remain by-value so the
        // caller's storage is unaffected by the callee's writes.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             fn bump(mut p: P) -> i32 { p.v = p.v + 1; return p.v; }\n\
             fn main() -> i32 { let q: P = P { v: 5 }; return bump(q); }"
        );
        assert!(
            ir.contains("define i32 @bump(%P "),
            "expected `mut p: P` on Copy struct to stay struct-by-value, got: {ir}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_no_alloca_in_callee() {
        // Pointer-passed params must NOT be re-alloca'd in the callee —
        // we bind directly to the SSA argument so writes hit the caller's
        // storage. Search the function body for an alloca of %Tag (would
        // indicate a stray re-allocation).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 0 }; bump(x); return x.v; }"
        );
        // Find the @bump body and confirm it has no `alloca %Tag` inside.
        let body_start = ir.find("define void @bump(").expect("@bump must be emitted");
        let body_tail = &ir[body_start..];
        let body_end = body_tail.find("\n}\n").expect("function close");
        let bump_body = &body_tail[..body_end];
        assert!(
            !bump_body.contains("alloca %Tag"),
            "ptr-passed `mut t: Tag` must not re-alloca in callee, got: {bump_body}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_no_double_drop() {
        // The callee bound a pointer to a non-Copy `mut` param must NOT
        // register a scope-exit drop — only the caller owns the value.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 0 }; bump(x); return x.v; }"
        );
        let body_start = ir.find("define void @bump(").expect("@bump must be emitted");
        let body_tail = &ir[body_start..];
        let body_end = body_tail.find("\n}\n").expect("function close");
        let bump_body = &body_tail[..body_end];
        // @bump must NOT call @Tag.drop on the ptr — the caller will.
        assert!(
            !bump_body.contains("@Tag.drop"),
            "callee must not drop a non-Copy mut-borrow param, got: {bump_body}"
        );
    }

    // ---- Phase 5 slice 5ATTR.3: `assert EXPR;` ----

    #[test]
    fn assert_emits_conditional_trap() {
        let ir = gen_src(
            "fn main() -> i32 { assert 1 == 1; return 0; }"
        );
        // Branch on the bool, trap on the false path.
        assert!(ir.contains("br i1 "), "expected branch on i1: {ir}");
        assert!(ir.contains("call void @llvm.trap()"), "expected trap on false path: {ir}");
        assert!(ir.contains("unreachable"), "expected unreachable after trap: {ir}");
    }

    #[test]
    fn assert_in_test_fn_compiles_clean() {
        // A `#[test]` fn with `assert` lowers like any other fn — no
        // special test-driver synthesis yet (that's slice 5ATTR.4).
        let ir = gen_src(
            "#[test] fn ok() { assert 2 == 2; return; }\n\
             fn main() -> i32 { return 0; }"
        );
        assert!(ir.contains("define void @ok("), "expected @ok defined: {ir}");
        assert!(ir.contains("call void @llvm.trap()"));
    }

    #[test]
    fn mut_param_noncopy_struct_via_method_call() {
        // Same ABI rule applies to non-receiver method params.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             struct Tool {}\n\
             impl Tool { fn poke(self, mut t: Tag) { t.v = t.v + 1; return; } }\n\
             fn main() -> i32 {\n\
                 let mut x: Tag = Tag { v: 1 };\n\
                 let tool: Tool = Tool {};\n\
                 tool.poke(x);\n\
                 return x.v;\n\
             }"
        );
        // Tool.poke signature: receiver ptr, mut param ptr.
        assert!(
            ir.contains("define void @Tool.poke(ptr "),
            "expected method to declare `mut t: Tag` as ptr param, got: {ir}"
        );
        // Two `ptr ` arguments at the call site (receiver + mut param).
        assert!(
            ir.contains("call void @Tool.poke(ptr "),
            "expected call to method to pass ptr args, got: {ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1A: LLVM information dividend ----
    //
    // Verifies that every fact the frontend has already proven is published
    // as an LLVM parameter attribute: noalias/readonly (existing), nonnull,
    // noundef, dereferenceable(N), align A on pointer-passed params; noundef
    // on value-passed scalar primitives.

    #[test]
    fn static_layout_primitives() {
        let t = TypeTable::default();
        assert_eq!(static_layout(&Ty::I8, &t),   Some((1, 1)));
        assert_eq!(static_layout(&Ty::U8, &t),   Some((1, 1)));
        assert_eq!(static_layout(&Ty::Bool, &t), Some((1, 1)));
        assert_eq!(static_layout(&Ty::I16, &t),  Some((2, 2)));
        assert_eq!(static_layout(&Ty::U32, &t),  Some((4, 4)));
        assert_eq!(static_layout(&Ty::F32, &t),  Some((4, 4)));
        assert_eq!(static_layout(&Ty::I64, &t),  Some((8, 8)));
        assert_eq!(static_layout(&Ty::Usize, &t),Some((8, 8)));
        assert_eq!(static_layout(&Ty::F64, &t),  Some((8, 8)));
        assert_eq!(static_layout(&Ty::RawPtr(Box::new(Ty::U8)), &t), Some((8, 8)));
        // Fat pointers.
        assert_eq!(static_layout(&Ty::Str, &t),                Some((16, 8)));
        assert_eq!(static_layout(&Ty::Slice(Box::new(Ty::I32)), &t), Some((16, 8)));
        assert_eq!(static_layout(&Ty::String, &t),             Some((24, 8)));
        // Fixed-size array.
        assert_eq!(static_layout(&Ty::Array(Box::new(Ty::I32), 4), &t), Some((16, 4)));
    }

    #[test]
    fn static_layout_struct_with_padding() {
        // struct S { a: i8, b: i32, c: i8 } → size 12, align 4.
        // Layout: a at 0, pad to 4, b at 4..8, c at 8, pad to align 4 → 12.
        let src = "struct S { a: i8, b: i32, c: i8 }\nfn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["S"];
        assert_eq!(static_layout(&Ty::Struct(id), &types), Some((12, 4)));
    }

    #[test]
    fn is_scalar_ty_distinguishes_scalars_from_aggregates() {
        let t = TypeTable::default();
        assert!(is_scalar_ty(&Ty::I32, &t));
        assert!(is_scalar_ty(&Ty::Bool, &t));
        assert!(is_scalar_ty(&Ty::RawPtr(Box::new(Ty::U8)), &t));
        assert!(!is_scalar_ty(&Ty::Str, &t));
        assert!(!is_scalar_ty(&Ty::String, &t));
        assert!(!is_scalar_ty(&Ty::Slice(Box::new(Ty::I32)), &t));
        assert!(!is_scalar_ty(&Ty::Array(Box::new(Ty::I32), 4), &t));
        // Plain enum (no payloads) is scalar (i32); tagged enum is aggregate.
        let src = "enum Plain { A, B, C }\nenum Tagged { S(i32), N }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let plain = Ty::Enum(types.enum_by_name["Plain"]);
        let tagged = Ty::Enum(types.enum_by_name["Tagged"]);
        assert!(is_scalar_ty(&plain, &types));
        assert!(!is_scalar_ty(&tagged, &types));
    }

    #[test]
    fn primitive_value_param_gets_noundef() {
        // Definite-assignment + scalar → noundef.
        let ir = gen_src(
            "fn double(x: i32) -> i32 { return x + x; }\n\
             fn main() -> i32 { return double(21); }"
        );
        assert!(
            ir.contains("define i32 @double(i32 noundef %0)"),
            "expected i32 param to carry noundef, got:\n{ir}"
        );
    }

    #[test]
    fn aggregate_value_param_does_not_get_noundef() {
        // `str` is an aggregate ({ ptr, i64 }) — noundef at aggregate level
        // would be unsound because padding/components may carry poison
        // through `insertvalue` chains. Skip noundef on aggregates.
        let ir = gen_src(
            "fn echo(s: str) -> str { return s; }\n\
             fn main() -> i32 { let r: str = echo(\"hi\"); return 0; }"
        );
        assert!(
            !ir.contains("{ ptr, i64 } noundef"),
            "value-passed aggregate must not carry noundef, got:\n{ir}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_emits_full_attr_set() {
        // `mut t: Tag` (non-Copy) gets the full pointer-attribute set:
        // noalias nonnull noundef dereferenceable(N) align A.
        // Tag = { i32 v } → size 4, align 4.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }"
        );
        assert!(
            ir.contains("define void @bump(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "expected full attr set on mut ptr param, got:\n{ir}"
        );
    }

    #[test]
    fn shared_param_noncopy_struct_emits_readonly_attr_set() {
        // Shared `t: Tag` (non-Copy) gets readonly (not noalias) plus the
        // rest. Two shared params may legally point at the same place, so
        // noalias would be unsound.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn peek(t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 7 }; return peek(x); }"
        );
        assert!(
            ir.contains("define i32 @peek(ptr readonly nonnull noundef dereferenceable(4) align 4 %0)"),
            "expected readonly+rest on shared ptr param, got:\n{ir}"
        );
    }

    #[test]
    fn method_receiver_emits_receiver_attrs() {
        // Self / mut self / move self map to readonly / noalias / noalias.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn read(self) -> i32 { return self.v; }\n\
               fn bump(mut self) { self.v = self.v + 1; return; }\n\
               fn into(move self) -> i32 { return self.v; }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 1 }; a.bump();\n\
               let b: T = T { v: 2 }; let _r: i32 = b.read();\n\
               let c: T = T { v: 3 }; let _n: i32 = c.into();\n\
               return 0;\n\
             }"
        );
        // `self` (Read)  → readonly
        assert!(
            ir.contains("define i32 @T.read(ptr readonly nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.read receiver attrs missing, got:\n{ir}"
        );
        // `mut self` (Mut) → noalias
        assert!(
            ir.contains("define void @T.bump(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.bump receiver attrs missing, got:\n{ir}"
        );
        // `move self` (Move) → noalias (callee owns; exclusive)
        assert!(
            ir.contains("define i32 @T.into(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.into receiver attrs missing, got:\n{ir}"
        );
    }

    #[test]
    fn dereferenceable_size_matches_type_layout() {
        // Struct with mixed-size fields: { i8, i32 } → size 8, align 4.
        // (i8 at 0, padding to 4, i32 at 4..8.)
        let ir = gen_src(
            "struct Big { tag: i8, n: i32 }\n\
             impl Big { fn drop(mut self) { return; } }\n\
             fn use_it(b: Big) -> i32 { return b.n; }\n\
             fn main() -> i32 { let x: Big = Big { tag: 1, n: 42 }; return use_it(x); }"
        );
        assert!(
            ir.contains("ptr readonly nonnull noundef dereferenceable(8) align 4"),
            "expected dereferenceable(8) align 4 for Big, got:\n{ir}"
        );
    }

    #[test]
    fn raw_pointer_param_intptr_does_not_get_nonnull() {
        // Slice 1A negative: a `*T` value-passed param (Copy, by-value) is
        // a scalar — it gets `noundef` but NOT `nonnull`/`dereferenceable`.
        // Raw pointers may be null in `unsafe` (slice 11.INTPTR via `0 as *T`).
        let ir = gen_src(
            "fn take(p: *u8) -> i32 { return 0; }\n\
             fn main() -> i32 { return take(unsafe { 0 as *u8 }); }"
        );
        // Look for the @take signature line; it must say `ptr noundef` but
        // not the pointer-target attribute set.
        let line = ir.lines().find(|l| l.starts_with("define i32 @take("))
            .expect("@take must be emitted");
        assert!(line.contains("ptr noundef"), "expected noundef on *u8 param: {line}");
        assert!(!line.contains("nonnull"),       "*u8 param must not carry nonnull: {line}");
        assert!(!line.contains("dereferenceable"),"*u8 param must not carry dereferenceable: {line}");
    }

    #[test]
    fn copy_struct_value_param_no_aggregate_noundef() {
        // Copy struct stays value-passed; aggregate value → no noundef.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             fn use_p(p: P) -> i32 { return p.v; }\n\
             fn main() -> i32 { let q: P = P { v: 5 }; return use_p(q); }"
        );
        let line = ir.lines().find(|l| l.starts_with("define i32 @use_p("))
            .expect("@use_p must be emitted");
        assert!(line.contains("%P "), "expected by-value P param: {line}");
        assert!(!line.contains("noundef"),
            "value-passed aggregate must not carry noundef: {line}");
    }

    // ---- Phase v0.0.2 Slice 1B: !range / llvm.assume publication ----
    //
    // The borrow checker, exhaustiveness check, and bounds-check lowering
    // already prove tag-in-range / length-non-negative / index-in-bounds.
    // Slice 1B publishes those facts to LLVM as `!range` metadata and
    // `llvm.assume` calls so `-O2`'s ConstraintElimination / InstCombine
    // can fold redundant checks downstream.

    #[test]
    fn preamble_declares_assume_intrinsic() {
        // Used by both slice-len and bounds-check publication. Declared in
        // the preamble so unused programs drop it via DCE.
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(
            ir.contains("declare void @llvm.assume(i1 noundef)"),
            "missing llvm.assume declaration in preamble:\n{ir}"
        );
    }

    #[test]
    fn enum_tag_load_carries_range_metadata() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 {\n\
               let c: Color = Color::Green;\n\
               let r: i32 = match c { Color::Red => 1, Color::Green => 2, Color::Blue => 3 };\n\
               return r;\n\
             }"
        );
        // The tag is loaded with `, !range !N`.
        assert!(
            ir.contains("load i32, ptr") && ir.contains(", !range !"),
            "expected tag load with !range, got:\n{ir}"
        );
        // The metadata node is `!{i32 0, i32 3}` for the 3-variant enum.
        assert!(
            ir.contains("= !{i32 0, i32 3}"),
            "expected !{{i32 0, i32 3}} for 3-variant enum, got:\n{ir}"
        );
    }

    #[test]
    fn tagged_enum_tag_load_carries_range_metadata() {
        // Tagged enum (Option-like): match dispatch on the payload-bearing
        // variant. The tag GEP + load pattern is different from a plain
        // enum, but the `!range` attachment should still happen.
        let ir = gen_src(
            "enum Opt { Some(i32), None }\n\
             fn main() -> i32 {\n\
               let o: Opt = Opt::Some(7);\n\
               let r: i32 = match o { Opt::Some(v) => v, Opt::None => 0 };\n\
               return r;\n\
             }"
        );
        assert!(
            ir.contains("load i32, ptr") && ir.contains(", !range !"),
            "expected tagged-enum tag load with !range, got:\n{ir}"
        );
        assert!(
            ir.contains("= !{i32 0, i32 2}"),
            "expected !{{i32 0, i32 2}} for 2-variant Opt, got:\n{ir}"
        );
    }

    #[test]
    fn slice_len_emits_nonneg_assume() {
        // `slice_len` extractvalue gets a paired `icmp sge + llvm.assume`
        // because `!range` doesn't apply to extractvalue. -O2 propagates
        // the assume into range metadata downstream.
        let ir = gen_src(
            "extern fn malloc(n: usize) -> *u8;\n\
             fn main() -> i32 {\n\
               let buf: *u8 = unsafe { malloc(16 as usize) };\n\
               let p: *i32 = unsafe { buf as *i32 };\n\
               let s: i32[] = unsafe { slice_from_raw_parts(p, 3 as usize) };\n\
               let n: usize = slice_len(s);\n\
               return n as i32;\n\
             }"
        );
        assert!(
            ir.contains("icmp sge i64") && ir.contains("call void @llvm.assume(i1"),
            "expected slice_len followed by assume(sge ..., 0), got:\n{ir}"
        );
    }

    #[test]
    fn bounds_check_emits_in_bounds_assume() {
        // After the bounds-check branch lands in the ok-block, codegen
        // emits `assume(idx < N)` so -O2 can drop downstream redundant
        // checks. The IR contains BOTH the trap path and the assume.
        let ir = gen_src(
            "fn main() -> i32 {\n\
               let arr: [i32; 3] = [10, 20, 30];\n\
               let i: usize = 1 as usize;\n\
               return arr[i];\n\
             }"
        );
        // Trap path preserved.
        assert!(ir.contains("icmp uge i64"), "expected trap-side uge, got:\n{ir}");
        assert!(ir.contains("call void @llvm.trap()"), "expected trap, got:\n{ir}");
        // Assume on the ok side.
        assert!(ir.contains("icmp ult i64"), "expected ok-side ult, got:\n{ir}");
        assert!(
            ir.contains("call void @llvm.assume(i1"),
            "expected llvm.assume after bounds check, got:\n{ir}"
        );
    }

    #[test]
    fn module_metadata_ids_start_at_high_offset() {
        // The range MD table uses `!100000+` to avoid colliding with
        // DWARF's `!0..!5` reserved + `!6..` function block. A program
        // that has any !range emission should have `!100000` defined.
        let ir = gen_src(
            "enum E { A, B }\n\
             fn main() -> i32 { let e: E = E::A; let _r: i32 = match e { E::A => 0, E::B => 1 }; return 0; }"
        );
        assert!(
            ir.contains("!100000 = !{"),
            "expected !100000 range MD node, got:\n{ir}"
        );
    }

    #[test]
    fn no_range_metadata_when_no_match_or_slice_or_index() {
        // Negative: a trivial program that never matches, never indexes
        // an array, never queries slice_len shouldn't emit any !range or
        // assume calls. (The `declare void @llvm.assume(...)` lives in
        // the preamble unconditionally — DCE handles it.)
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(
            !ir.contains("!range "),
            "trivial program must not carry !range, got:\n{ir}"
        );
        assert!(
            !ir.contains("call void @llvm.assume("),
            "trivial program must not call assume, got:\n{ir}"
        );
    }

    #[test]
    fn range_metadata_cache_reuses_node_id() {
        // Two matches on the same 3-variant enum should share one MD
        // node (the `register_range` cache key is (lo, hi, ty)).
        let ir = gen_src(
            "enum E { A, B, C }\n\
             fn main() -> i32 {\n\
               let a: E = E::A;\n\
               let b: E = E::B;\n\
               let r1: i32 = match a { E::A => 1, E::B => 2, E::C => 3 };\n\
               let r2: i32 = match b { E::A => 4, E::B => 5, E::C => 6 };\n\
               return r1 + r2;\n\
             }"
        );
        // Both match loads reference the same MD id (!100000).
        let occurrences = ir.matches(", !range !100000").count();
        assert!(
            occurrences >= 2,
            "expected MD id to be shared across two matches on same enum, got {occurrences}\n{ir}"
        );
        // Only one definition of !100000.
        let defs = ir.matches("!100000 = !{").count();
        assert_eq!(defs, 1, "expected one definition of !100000, got {defs}\n{ir}");
    }

    // ---- Phase v0.0.2 Slice 1C: scoped !alias.scope / !noalias ----
    //
    // Borrowck proves that for every pointer-passed `mut`/`move` non-Copy
    // param, no other live pointer in the same function reaches the same
    // memory. Slice 1A encodes this as the `noalias` param attribute —
    // which degrades after inlining. Scoped alias metadata survives
    // inlining and feeds the loop vectorizer.

    #[test]
    fn two_mut_noncopy_params_emit_domain_and_scopes() {
        // `swap(mut a: Tag, mut b: Tag)` has two noalias-shaped pointers.
        // The function gets one domain MD node and two scope nodes.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn swap(mut a: Tag, mut b: Tag) {\n\
               let ta: i32 = a.v;\n\
               let tb: i32 = b.v;\n\
               a.v = tb;\n\
               b.v = ta;\n\
               return;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: Tag = Tag { v: 1 };\n\
               let mut y: Tag = Tag { v: 2 };\n\
               swap(x, y);\n\
               return x.v;\n\
             }"
        );
        // Domain (self-referential, labeled with fn name).
        assert!(
            ir.contains("distinct !{") && ir.contains("\"swap\""),
            "expected swap domain MD node, got:\n{ir}"
        );
        // Two scopes labeled `p0` and `p1`.
        assert!(ir.contains("\"p0\""), "expected scope p0, got:\n{ir}");
        assert!(ir.contains("\"p1\""), "expected scope p1, got:\n{ir}");
        // Loads through the params carry alias.scope+noalias.
        assert!(
            ir.contains(", !alias.scope ") && ir.contains(", !noalias !"),
            "expected alias-scope annotated loads/stores, got:\n{ir}"
        );
    }

    #[test]
    fn scope_propagates_through_gep_to_field_loads() {
        // A direct field read on a `mut` non-Copy param GEPs off the param's
        // SSA, then loads. The post-pass should propagate the scope from the
        // GEP source to the load.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             impl P { fn drop(mut self) { return; } }\n\
             fn pair(mut a: P, mut b: P) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let p: P = P { v: 1 };\n\
               let q: P = P { v: 2 };\n\
               return pair(p, q);\n\
             }"
        );
        // Both loads (one per param) must be annotated.
        let load_count = ir.lines()
            .filter(|l| l.contains("load i32") && l.contains("!alias.scope"))
            .count();
        assert!(
            load_count >= 2,
            "expected >=2 scope-tagged loads, got {load_count}:\n{ir}"
        );
    }

    #[test]
    fn single_mut_param_does_not_emit_scope_metadata() {
        // With only one noalias-shaped param, no aliasing-pair exists, so
        // there's nothing useful to publish — skip the metadata to keep IR
        // small.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }"
        );
        // The function body must not carry alias.scope on its loads.
        let body_start = ir.find("define void @bump(").expect("@bump emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("@bump close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "single-mut-param fn should not carry alias.scope, got:\n{body}"
        );
    }

    #[test]
    fn shared_params_do_not_participate_in_scope_set() {
        // Shared (`x: T`, non-Copy) is `readonly` — two shared params may
        // legally alias each other (§2.9). They MUST NOT show up as
        // alias-scope sources.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             impl P { fn drop(mut self) { return; } }\n\
             fn both_shared(a: P, b: P) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let p: P = P { v: 1 };\n\
               let q: P = P { v: 2 };\n\
               return both_shared(p, q);\n\
             }"
        );
        let body_start = ir.find("define i32 @both_shared(").expect("@both_shared emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("@both_shared close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "shared (readonly) params must not get alias.scope, got:\n{body}"
        );
    }

    #[test]
    fn method_receiver_and_mut_param_participate_in_scope_set() {
        // `mut self` is pointer-passed-and-exclusive; combined with a
        // separate `mut other: T` param, we have two scopes.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn merge(mut self, mut other: T) {\n\
                 self.v = self.v + other.v;\n\
                 other.v = 0;\n\
                 return;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 10 };\n\
               let mut b: T = T { v: 20 };\n\
               a.merge(b);\n\
               return a.v;\n\
             }"
        );
        assert!(
            ir.contains("\"T.merge\""),
            "expected T.merge domain MD, got:\n{ir}"
        );
        // Both p0 (self) and p1 (other) scopes present.
        assert!(ir.contains("\"p0\""), "expected p0 (self) scope, got:\n{ir}");
        assert!(ir.contains("\"p1\""), "expected p1 (other) scope, got:\n{ir}");
        // Annotated load+store pairs in body.
        assert!(
            ir.lines().any(|l| l.contains("load") && l.contains("!alias.scope")),
            "expected scope-tagged load in T.merge, got:\n{ir}"
        );
        assert!(
            ir.lines().any(|l| l.contains("store") && l.contains("!alias.scope")),
            "expected scope-tagged store in T.merge, got:\n{ir}"
        );
    }

    #[test]
    fn shared_self_receiver_does_not_get_scope() {
        // `self` (Read) is readonly, NOT noalias. A method with `self` +
        // a mut param has only one noalias-shaped pointer → no metadata.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn read_and_bump(self, mut other: T) -> i32 {\n\
                 other.v = self.v;\n\
                 return self.v;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let a: T = T { v: 5 };\n\
               let mut b: T = T { v: 0 };\n\
               return a.read_and_bump(b);\n\
             }"
        );
        let body_start = ir.find("define i32 @T.read_and_bump(").expect("emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "self (Read) + one mut param = one noalias only → no scope metadata, got:\n{body}"
        );
    }

    #[test]
    fn extract_ptr_operand_basic() {
        // Hand the helper a synthetic instruction; verify it pulls out
        // the pointer operand.
        let p = extract_ptr_operand("load i32, ptr %t5, align 4");
        assert_eq!(p.as_deref(), Some("%t5"));
        let p = extract_ptr_operand("getelementptr inbounds %T, ptr %0, i32 0, i32 1");
        assert_eq!(p.as_deref(), Some("%0"));
        let p = extract_ptr_operand("store i32 7, ptr %dst");
        assert_eq!(p.as_deref(), Some("%dst"));
        // No `, ptr` operand → None.
        let p = extract_ptr_operand("add i32 %a, %b");
        assert!(p.is_none());
    }

    #[test]
    fn alias_scope_dataflow_propagates_through_chained_gep() {
        // GEPs feed off other GEPs (nested struct access). The dataflow
        // should propagate the scope along the chain so the final load is
        // annotated.
        let ir = gen_src(
            "struct Inner { x: i32 }\n\
             struct Outer { inner: Inner, tag: i32 }\n\
             impl Outer { fn drop(mut self) { return; } }\n\
             fn touch_both(mut a: Outer, mut b: Outer) -> i32 {\n\
               return a.inner.x + b.tag;\n\
             }\n\
             fn main() -> i32 {\n\
               let p: Outer = Outer { inner: Inner { x: 7 }, tag: 1 };\n\
               let q: Outer = Outer { inner: Inner { x: 3 }, tag: 2 };\n\
               return touch_both(p, q);\n\
             }"
        );
        // Nested load (a.inner.x) must carry alias.scope.
        assert!(
            ir.lines().any(|l| l.contains("load i32") && l.contains("!alias.scope")),
            "expected nested-load scope annotation, got:\n{ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1F: cold + preserve_nonecc on drop glue ----
    //
    // Destructors are compiler-synthesized cold-path helpers. Marking them
    // `preserve_nonecc cold` lets the optimizer skip callee-save register
    // saves at the call boundary and biases hot paths away from drops.

    #[test]
    fn drop_method_emits_cold_and_preserve_none_cc() {
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn main() -> i32 { let r: R = R { v: 7 }; return r.v; }"
        );
        // `define preserve_nonecc void @R.drop(...) cold {`
        assert!(
            ir.contains("define preserve_nonecc void @R.drop("),
            "expected preserve_nonecc on drop definition, got:\n{ir}"
        );
        // The `cold` attribute lands after the param list, before `{`.
        let drop_line = ir.lines()
            .find(|l| l.contains("@R.drop("))
            .expect("drop definition emitted");
        assert!(
            drop_line.ends_with(") cold {"),
            "drop definition must carry `cold`, got: {drop_line}"
        );
    }

    #[test]
    fn drop_call_sites_match_callee_cc() {
        // LLVM rejects IR where the call site's CC disagrees with the
        // callee's. The Always-disposition path emits the call.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn main() -> i32 { let r: R = R { v: 7 }; return r.v; }"
        );
        assert!(
            ir.contains("call preserve_nonecc void @R.drop("),
            "drop call site must match preserve_nonecc CC, got:\n{ir}"
        );
    }

    #[test]
    fn non_drop_methods_keep_default_cc() {
        // Only `drop` methods get the cold CC. Regular methods continue
        // to use the default C calling convention.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R {\n\
               fn drop(mut self) { return; }\n\
               fn bump(mut self) -> i32 { self.v = self.v + 1; return self.v; }\n\
             }\n\
             fn main() -> i32 { let mut r: R = R { v: 0 }; return r.bump(); }"
        );
        // R.bump must NOT have preserve_nonecc or `) cold {`.
        let bump_line = ir.lines()
            .find(|l| l.contains("@R.bump("))
            .expect("@R.bump emitted");
        assert!(
            !bump_line.contains("preserve_nonecc"),
            "non-drop method must not get preserve_nonecc, got: {bump_line}"
        );
        assert!(
            !bump_line.contains("cold"),
            "non-drop method must not get cold, got: {bump_line}"
        );
    }

    #[test]
    fn non_drop_call_sites_have_no_cc_prefix() {
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R {\n\
               fn drop(mut self) { return; }\n\
               fn bump(mut self) -> i32 { self.v = self.v + 1; return self.v; }\n\
             }\n\
             fn main() -> i32 { let mut r: R = R { v: 0 }; return r.bump(); }"
        );
        // R.bump call must be plain `call i32 @R.bump(...)`, no CC.
        assert!(
            ir.contains("call i32 @R.bump("),
            "expected default-CC call to R.bump, got:\n{ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1E: musttail on tail-position direct calls ----
    //
    // `return foo(args);` where caller and callee have matching signature
    // can be a guaranteed tail call. LLVM's verifier rejects musttail when
    // the param-count/type signature doesn't match exactly, so the
    // predicate is conservative.

    #[test]
    fn recursive_tail_call_uses_musttail() {
        // `sum_to(n, acc)` recurses with `return sum_to(n-1, acc+n)`. The
        // caller and callee have identical signatures, so musttail fires.
        let ir = gen_src(
            "fn sum_to(n: i32, acc: i32) -> i32 {\n\
               if n == 0 { return acc; }\n\
               return sum_to(n - 1, acc + n);\n\
             }\n\
             fn main() -> i32 { return sum_to(10, 0); }"
        );
        // The recursive call must be musttail.
        let line = ir.lines()
            .find(|l| l.contains("call i32 @sum_to") && l.contains("musttail"))
            .expect("expected musttail recursive call");
        assert!(line.contains("musttail call i32 @sum_to"), "got: {line}");
    }

    #[test]
    fn entry_call_with_mismatched_signature_does_not_use_musttail() {
        // `main() -> i32` returning `sum_to(args) -> i32` doesn't qualify:
        // caller has 0 params, callee has 2. LLVM would reject; the
        // predicate must bail.
        let ir = gen_src(
            "fn sum_to(n: i32, acc: i32) -> i32 {\n\
               if n == 0 { return acc; }\n\
               return sum_to(n - 1, acc + n);\n\
             }\n\
             fn main() -> i32 { return sum_to(10, 0); }"
        );
        // The main's call must be a plain `call`, not `musttail`.
        let main_start = ir.find("define i32 @main()").expect("@main emitted");
        let main_end = ir[main_start..].find("\n}\n").expect("@main close");
        let main_body = &ir[main_start..main_start + main_end];
        assert!(
            main_body.contains("call i32 @sum_to"),
            "expected call to sum_to in main: {main_body}"
        );
        assert!(
            !main_body.contains("musttail"),
            "main → sum_to (mismatched sig) must not be musttail: {main_body}"
        );
    }

    #[test]
    fn mismatched_return_type_does_not_use_musttail() {
        // Caller returns i32, callee returns i64 → no musttail.
        let ir = gen_src(
            "fn long_id(n: i64) -> i64 { return n; }\n\
             fn short_id(n: i32) -> i32 { return n; }\n\
             fn caller(n: i32) -> i32 { return short_id(n); }\n\
             fn main() -> i32 { return caller(0); }"
        );
        // caller → short_id: same signature, same return → musttail.
        assert!(
            ir.contains("musttail call i32 @short_id"),
            "expected matching-sig musttail, got:\n{ir}"
        );
        // long_id is never tail-called.
        assert!(
            !ir.contains("musttail call i64 @long_id"),
            "i64 fn must not appear as musttail from i32-returning caller, got:\n{ir}"
        );
    }

    #[test]
    fn return_with_no_call_does_not_set_musttail_flag() {
        // Plain `return x;` (not a call) should produce a plain `ret`.
        let ir = gen_src("fn id(n: i32) -> i32 { return n + 1; }");
        assert!(!ir.contains("musttail"), "no call → no musttail, got:\n{ir}");
        assert!(ir.contains("ret i32"), "expected ret i32: {ir}");
    }

    #[test]
    fn methods_do_not_emit_musttail() {
        // Method bodies carry an implicit receiver, so even if the body
        // does `return helper()`, the receiver-vs-no-receiver mismatch
        // would make musttail invalid. The eligibility flag suppresses.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             fn helper() -> i32 { return 1; }\n\
             impl T {\n\
               fn get(self) -> i32 { return helper(); }\n\
             }\n\
             fn main() -> i32 { let t: T = T { v: 0 }; return t.get(); }"
        );
        // T.get must NOT use musttail.
        let m_start = ir.find("define i32 @T.get(").expect("T.get emitted");
        let m_end = ir[m_start..].find("\n}\n").expect("T.get close");
        let m_body = &ir[m_start..m_start + m_end];
        assert!(
            !m_body.contains("musttail"),
            "method body must not emit musttail (receiver shape would mismatch): {m_body}"
        );
    }

    #[test]
    fn return_drop_value_does_not_use_musttail() {
        // A Drop-bound local creates a pending scope-exit. musttail
        // requires the ret to immediately follow the call (no drop
        // emission between), so the predicate must bail.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn id(n: i32) -> i32 { return n; }\n\
             fn caller(n: i32) -> i32 {\n\
               let r: R = R { v: 1 };\n\
               return id(n);\n\
             }\n\
             fn main() -> i32 { return caller(5); }"
        );
        // caller's body has a Drop binding, so its `return id(n)` cannot
        // be musttail (drop runs between the call and the ret).
        let c_start = ir.find("define i32 @caller(").expect("caller emitted");
        let c_end = ir[c_start..].find("\n}\n").expect("caller close");
        let c_body = &ir[c_start..c_start + c_end];
        assert!(
            !c_body.contains("musttail"),
            "pending Drop must suppress musttail in caller: {c_body}"
        );
    }

    // ---- Phase v0.0.2 Slice 1D: sret for owned-string returns ----
    //
    // Owned `string` is the canonical sret case: 24-byte aggregate with
    // Drop. Returning by value forces LLVM to copy through registers +
    // memory; `sret` collapses to a single write into the caller's slot.
    // The narrow scope (string only) bounds the ABI-change blast radius.

    #[test]
    fn return_passes_by_sret_predicate() {
        // Today only Ty::String triggers sret; primitives and slices stay
        // value-returned. Generic non-Copy struct sret is deferred.
        assert!(return_passes_by_sret(&Ty::String));
        assert!(!return_passes_by_sret(&Ty::I32));
        assert!(!return_passes_by_sret(&Ty::Str));
        assert!(!return_passes_by_sret(&Ty::Slice(Box::new(Ty::I32))));
        assert!(!return_passes_by_sret(&Ty::Unit));
    }

    #[test]
    fn string_returning_fn_uses_sret_definition() {
        let ir = gen_src(
            "fn greet() -> string { return \"hi\".to_string(); }\n\
             fn main() -> i32 { let s: string = greet(); return 0; }"
        );
        // The function returns void and takes a sret pointer as %0.
        assert!(
            ir.contains("define void @greet(ptr sret({ ptr, i64, i64 }) noalias nonnull noundef writable dereferenceable(24) align 8 %0)"),
            "expected sret definition, got:\n{ir}"
        );
        // The body stores into %0 then returns void.
        assert!(
            ir.contains("store { ptr, i64, i64 }") && ir.contains(", ptr %0"),
            "expected store-to-sret-slot, got:\n{ir}"
        );
    }

    #[test]
    fn string_returning_fn_call_site_uses_sret_slot() {
        // The caller allocates a 24-byte slot, passes it as the sret
        // arg, and loads the result back for value-semantics consumers.
        let ir = gen_src(
            "fn greet() -> string { return \"hi\".to_string(); }\n\
             fn main() -> i32 { let s: string = greet(); return 0; }"
        );
        assert!(
            ir.contains("call void @greet(ptr "),
            "expected void-returning call to greet, got:\n{ir}"
        );
        // After the call, the caller loads the value back from the slot.
        assert!(
            ir.contains("load { ptr, i64, i64 }, ptr"),
            "expected load-from-slot after sret call, got:\n{ir}"
        );
    }

    #[test]
    fn extern_fn_returning_string_keeps_value_abi() {
        // sret is a C+ convention; extern fns keep the C ABI the user
        // declared. (No real C fn returns a `string` aggregate, but the
        // ABI invariant must hold.)
        let ir = gen_src(
            "extern fn make_str() -> string;\n\
             fn main() -> i32 { let s: string = unsafe { make_str() }; return 0; }"
        );
        // The declare line must use the value-return form, not sret.
        assert!(
            ir.contains("declare { ptr, i64, i64 } @make_str()"),
            "extern fn must keep value ABI, got:\n{ir}"
        );
        assert!(
            !ir.contains("declare void @make_str(ptr sret"),
            "extern fn must not be sret-converted, got:\n{ir}"
        );
    }

    #[test]
    fn primitive_returning_fn_keeps_value_abi() {
        // Slice 1D narrow scope: only `string` triggers sret. i32 returns
        // continue to use the value form.
        let ir = gen_src(
            "fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
             fn main() -> i32 { return add(2, 3); }"
        );
        assert!(
            ir.contains("define i32 @add("),
            "primitive return must keep value form, got:\n{ir}"
        );
        assert!(
            !ir.contains("sret"),
            "no sret on primitive-return fn, got:\n{ir}"
        );
    }

    #[test]
    fn string_sret_with_args_shifts_param_indices() {
        // With sret, the user-declared params live at %1, %2, ... rather
        // than %0, %1, .... Verify the body references the shifted SSA.
        let ir = gen_src(
            "fn pick(n: i32) -> string { return \"x\".to_string(); }\n\
             fn main() -> i32 { let s: string = pick(7); return 0; }"
        );
        // Definition: %0 is sret, %1 is `n`.
        assert!(
            ir.contains("define void @pick(ptr sret") && ir.contains(", i32 noundef %1)"),
            "expected sret-then-i32 param indices, got:\n{ir}"
        );
    }

    #[test]
    fn musttail_with_sret_forwards_caller_slot() {
        // Caller's sret slot can be forwarded to callee on a tail call.
        // `caller() -> string` returning `helper()` (both sret) should
        // not need an intermediate slot+load — forward the caller's slot.
        let ir = gen_src(
            "fn helper() -> string { return \"hi\".to_string(); }\n\
             fn caller() -> string { return helper(); }\n\
             fn main() -> i32 { let s: string = caller(); return 0; }"
        );
        // Caller's body must musttail-call helper using its own sret slot.
        let c_start = ir.find("define void @caller(").expect("caller emitted");
        let c_end = ir[c_start..].find("\n}\n").expect("caller close");
        let c_body = &ir[c_start..c_start + c_end];
        assert!(
            c_body.contains("musttail call void @helper(ptr %0)"),
            "expected musttail call forwarding caller's sret slot, got:\n{c_body}"
        );
    }

    // ---- Phase 3A: bitwise + shift + byte-swap ----

    #[test]
    fn bitand_emits_llvm_and() {
        let ir = gen_src("fn main() -> i32 { return 0xff & 0x0f; }");
        assert!(ir.contains(" = and i32 "),
            "expected `and i32`, got:\n{ir}");
    }

    #[test]
    fn bitor_emits_llvm_or() {
        let ir = gen_src("fn main() -> i32 { return 0xff | 0x0f; }");
        assert!(ir.contains(" = or i32 "),
            "expected `or i32`, got:\n{ir}");
    }

    #[test]
    fn bitxor_emits_llvm_xor() {
        let ir = gen_src("fn main() -> i32 { return 0xff ^ 0x0f; }");
        assert!(ir.contains(" = xor i32 "),
            "expected `xor i32`, got:\n{ir}");
    }

    #[test]
    fn bit_not_emits_xor_minus_one() {
        // Phase 3A: `~x` lowers to `xor x, -1` per LLVM idiom.
        let ir = gen_src("fn main() -> i32 { let x: i32 = 5; return ~x; }");
        assert!(ir.contains("xor i32 ") && ir.contains(", -1"),
            "expected `xor i32 ..., -1`, got:\n{ir}");
    }

    #[test]
    fn shl_emits_llvm_shl() {
        let ir = gen_src("fn main() -> i32 { return 1 << 3; }");
        assert!(ir.contains(" = shl i32 "),
            "expected `shl i32`, got:\n{ir}");
    }

    #[test]
    fn signed_shr_emits_arithmetic_shift() {
        // i32 is signed → `ashr` (preserves sign bit).
        let ir = gen_src("fn main() -> i32 { let x: i32 = -8; return x >> 2; }");
        assert!(ir.contains(" = ashr i32 "),
            "expected `ashr` for signed shift right, got:\n{ir}");
    }

    #[test]
    fn unsigned_shr_emits_logical_shift() {
        // u32 is unsigned → `lshr` (zero-fill).
        let ir = gen_src(
            "fn main() -> i32 { let x: u32 = 8 as u32; let y: u32 = x >> (2 as u32); return 0; }"
        );
        assert!(ir.contains(" = lshr i32 "),
            "expected `lshr` for unsigned shift right, got:\n{ir}");
    }

    #[test]
    fn shift_count_different_width_gets_coerced() {
        // `i64 << u8` — RHS gets zext from i8 to i64 before the shift.
        let ir = gen_src(
            "fn main() -> i32 {\n\
               let x: i64 = 1 as i64;\n\
               let n: u8 = 3 as u8;\n\
               let y: i64 = x << n;\n\
               return 0;\n\
             }"
        );
        // zext from i8 to i64 of the count, followed by shl i64.
        assert!(ir.contains(" = zext i8 ") && ir.contains(" to i64"),
            "expected zext i8 -> i64, got:\n{ir}");
        assert!(ir.contains(" = shl i64 "),
            "expected `shl i64`, got:\n{ir}");
    }

    #[test]
    fn bswap16_emits_intrinsic_call() {
        let ir = gen_src("fn main() -> i32 { let p: u16 = 0x1234 as u16; let q: u16 = bswap16(p); return 0; }");
        assert!(ir.contains("call i16 @llvm.bswap.i16(i16 "),
            "expected llvm.bswap.i16 call, got:\n{ir}");
    }

    #[test]
    fn bswap32_emits_intrinsic_call() {
        let ir = gen_src("fn main() -> i32 { let p: u32 = 0x12345678 as u32; let q: u32 = bswap32(p); return 0; }");
        assert!(ir.contains("call i32 @llvm.bswap.i32(i32 "),
            "expected llvm.bswap.i32 call, got:\n{ir}");
    }

    #[test]
    fn bswap64_emits_intrinsic_call() {
        let ir = gen_src("fn main() -> i32 { let p: u64 = 1 as u64; let q: u64 = bswap64(p); return 0; }");
        assert!(ir.contains("call i64 @llvm.bswap.i64(i64 "),
            "expected llvm.bswap.i64 call, got:\n{ir}");
    }

    #[test]
    fn htons_aliases_bswap16() {
        // htons/htonl/ntohs/ntohl are aliases that lower to bswap on LE.
        let ir = gen_src("fn main() -> i32 { let p: u16 = 8080 as u16; let q: u16 = htons(p); return 0; }");
        assert!(ir.contains("call i16 @llvm.bswap.i16(i16 "),
            "expected htons to lower to llvm.bswap.i16, got:\n{ir}");
    }

    #[test]
    fn htonl_aliases_bswap32() {
        let ir = gen_src("fn main() -> i32 { let p: u32 = 1 as u32; let q: u32 = htonl(p); return 0; }");
        assert!(ir.contains("call i32 @llvm.bswap.i32(i32 "),
            "expected htonl to lower to llvm.bswap.i32, got:\n{ir}");
    }

    #[test]
    fn preamble_declares_bswap_intrinsics() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(ir.contains("declare i16 @llvm.bswap.i16(i16)"));
        assert!(ir.contains("declare i32 @llvm.bswap.i32(i32)"));
        assert!(ir.contains("declare i64 @llvm.bswap.i64(i64)"));
    }

    #[test]
    // ---- Phase 5 Slice 5.D: C-ABI aggregate coercion ----

    #[test]
    fn classify_c_abi_scalars_pass_direct() {
        let t = TypeTable::default();
        assert_eq!(classify_c_abi(&Ty::I32, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::U64, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::Bool, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::F32, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::RawPtr(Box::new(Ty::U8)), &t), CAbiClass::Direct);
    }

    #[test]
    fn classify_c_abi_small_struct_coerces_to_i64() {
        // `#[repr(C)] struct Point { x: i32, y: i32 }` is 8 bytes.
        let src = "#[repr(C)] struct Point { x: i32, y: i32 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Point"];
        let abi = classify_c_abi(&Ty::Struct(id), &types);
        match abi {
            CAbiClass::Coerce { llvm_ty, size, align } => {
                assert_eq!(llvm_ty, "i64");
                assert_eq!(size, 8);
                assert_eq!(align, 8);
            }
            _ => panic!("expected Coerce(i64), got {abi:?}"),
        }
    }

    #[test]
    fn classify_c_abi_mid_struct_coerces_to_array_i64() {
        let src = "#[repr(C)] struct Pair { a: i64, b: i64 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Pair"];
        match classify_c_abi(&Ty::Struct(id), &types) {
            CAbiClass::Coerce { llvm_ty, size, .. } => {
                assert_eq!(llvm_ty, "[2 x i64]");
                assert_eq!(size, 16);
            }
            other => panic!("expected Coerce([2 x i64]), got {other:?}"),
        }
    }

    #[test]
    fn classify_c_abi_large_struct_passes_indirect() {
        let src = "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Triple"];
        assert_eq!(classify_c_abi(&Ty::Struct(id), &types), CAbiClass::Indirect);
    }

    #[test]
    fn pub_extern_fn_with_small_struct_param_coerces_to_i64() {
        // Codegen-level: the LLVM signature must use `i64` for the
        // 8-byte `Point` param (matching clang's aarch64-darwin output).
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             pub extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }"
        );
        // Look for the @square define with coerced i64 param.
        assert!(
            ir.contains("define i32 @square(i64"),
            "expected `define i32 @square(i64 ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_small_struct_return_coerces_to_i64() {
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             pub extern fn make(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }"
        );
        // 8-byte struct return: `define i64 @make(...)` with packed coerce on ret.
        assert!(
            ir.contains("define i64 @make("),
            "expected `define i64 @make(...)`, got:\n{ir}"
        );
        // The Return statement should stage through alloca + load-as-i64.
        assert!(
            ir.contains("load i64, ptr") && ir.contains("ret i64"),
            "expected coerce-on-return path emitted, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_mid_struct_param_coerces_to_array_i64() {
        let ir = gen_src(
            "#[repr(C)] struct Pair { a: i64, b: i64 }\n\
             pub extern fn sum(p: Pair) -> i64 { return p.a + p.b; }"
        );
        assert!(
            ir.contains("define i64 @sum([2 x i64]"),
            "expected `define i64 @sum([2 x i64] ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_large_struct_param_passes_indirect() {
        let ir = gen_src(
            "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
             pub extern fn sum(t: Triple) -> i64 { return t.a + t.b + t.c; }"
        );
        // >16 byte struct param: bare `ptr` (no byval on aarch64-darwin).
        assert!(
            ir.contains("define i64 @sum(ptr"),
            "expected `define i64 @sum(ptr ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_large_struct_return_uses_sret() {
        // >16-byte aggregate returns go through Slice 1D's sret path —
        // generalized in 5.D from `Ty::String` only to any Indirect class.
        let ir = gen_src(
            "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
             pub extern fn make() -> Triple { return Triple { a: 1 as i64, b: 2 as i64, c: 3 as i64 }; }"
        );
        assert!(
            ir.contains("define void @make(ptr sret("),
            "expected sret-form return for >16 byte struct, got:\n{ir}"
        );
        assert!(
            ir.contains("ret void"),
            "expected `ret void` for sret path, got:\n{ir}"
        );
    }

    #[test]
    fn non_extern_fn_unaffected_by_5d() {
        // Regression guard: 5.D coercion fires ONLY on `pub extern fn`.
        // A regular C+ fn `fn use_p(p: Point) -> i32` keeps the C+ ABI
        // (LLVM first-class aggregate, `%Point %0`).
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             fn use_p(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let q: Point = Point { x: 1, y: 2 }; return use_p(q); }"
        );
        // The non-extern path keeps `%Point %0` (Copy struct, by-value).
        assert!(
            ir.contains("define i32 @use_p(%Point"),
            "non-extern fn must keep C+ ABI, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_scalar_args_no_coercion() {
        // Pure-scalar signatures don't trigger any coercion machinery —
        // the C ABI and C+ ABI agree on scalar passing.
        let ir = gen_src(
            "pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }"
        );
        assert!(
            ir.contains("define i32 @add(i32 noundef") && !ir.contains("@add(i64"),
            "scalar-only export must not coerce, got:\n{ir}"
        );
    }

    #[test]
    fn existing_substring_checks_still_match() {
        // Backward-compat: pre-1A tests assert on substrings like
        // `define void @bump(ptr noalias ` — confirm those still hold after
        // the attr set widened (the noalias prefix is still left-anchored).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }"
        );
        assert!(ir.contains("define void @bump(ptr noalias "));
    }

    // ---- Phase v0.0.2 Slice 1C: scoped !alias.scope / !noalias ----
    //
    // For every pointer-passed `mut`/`move` param (the ones already
    // carrying `noalias` from Slice 1A), publish a unique scope inside a
    // per-function domain. Loads/stores derived from each param carry
    // `!alias.scope` of their own scope and `!noalias` of all other
    // function-local scopes. Survives -O2 inlining where `noalias` would
    // be lost.

    #[test]
    fn two_mut_params_get_distinct_alias_scopes() {
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn swap_bump(mut a: T, mut b: T) {\n\
               let tmp: i32 = a.v;\n\
               a.v = b.v;\n\
               b.v = tmp;\n\
               return;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: T = T { v: 1 };\n\
               let mut y: T = T { v: 2 };\n\
               swap_bump(x, y);\n\
               return x.v + y.v;\n\
             }"
        );
        // One domain, two scopes for the function.
        assert!(
            ir.contains("distinct !{!100000, !\"swap_bump\"}"),
            "expected swap_bump domain, got:\n{ir}"
        );
        assert!(
            ir.contains(", !100000, !\"p0\"}") && ir.contains(", !100000, !\"p1\"}"),
            "expected p0 and p1 scopes tied to the domain, got:\n{ir}"
        );
        // Loads/stores through both params carry alias.scope + noalias.
        let scope_lines = ir.lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(scope_lines >= 4,
            "expected at least 4 annotated load/store lines (2 loads + 2 stores), got {scope_lines}:\n{ir}");
    }

    #[test]
    fn single_mut_param_no_alias_scope() {
        // With only one noalias-capable param, there's nothing to be
        // disjoint *from*; emitting alias.scope wastes IR space without
        // a payoff. Confirm the optimization is gated on count >= 2.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn bump(mut t: T) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: T = T { v: 1 }; bump(x); return x.v; }"
        );
        assert!(
            !ir.contains("!alias.scope"),
            "single mut param shouldn't trigger alias.scope, got:\n{ir}"
        );
    }

    #[test]
    fn shared_params_do_not_participate_in_alias_scope() {
        // Two `t: T` shared params get `readonly`, not `noalias`. The
        // borrow checker doesn't prove they're disjoint, so we don't
        // publish alias.scope (would be unsound — two shared refs may
        // legally alias).
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn sum(a: T, b: T) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let x: T = T { v: 1 };\n\
               let y: T = T { v: 2 };\n\
               return sum(x, y);\n\
             }"
        );
        assert!(
            !ir.contains("!alias.scope"),
            "two readonly shared params must not trigger alias.scope, got:\n{ir}"
        );
    }

    #[test]
    fn method_mut_self_plus_mut_param_get_scopes() {
        // `mut self` (Receiver::Mut → noalias-shaped, idx 0) and a
        // non-Copy mut param both participate.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn merge(mut self, mut other: T) {\n\
                 self.v = self.v + other.v;\n\
                 other.v = 0;\n\
                 return;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 1 };\n\
               let mut b: T = T { v: 2 };\n\
               a.merge(b);\n\
               return a.v;\n\
             }"
        );
        // Method-mangled domain.
        assert!(
            ir.contains("distinct !{") && ir.contains("\"T.merge\"}"),
            "expected T.merge domain, got:\n{ir}"
        );
        // Loads/stores annotated.
        let scope_lines = ir.lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(scope_lines >= 2,
            "expected at least 2 annotated load/store lines in T.merge, got {scope_lines}:\n{ir}");
    }

    #[test]
    fn alias_scope_propagates_through_gep_chain() {
        // gen_field GEPs off the param's slot. The post-pass dataflow
        // should propagate the scope through the GEP so the eventual
        // load carries it.
        let ir = gen_src(
            "struct Inner { n: i32 }\n\
             struct Outer { inner: Inner, tag: i32 }\n\
             impl Outer { fn drop(mut self) { return; } }\n\
             fn touch(mut a: Outer, mut b: Outer) -> i32 {\n\
               a.inner.n = b.tag;\n\
               return a.inner.n + b.tag;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: Outer = Outer { inner: Inner { n: 0 }, tag: 7 };\n\
               let mut y: Outer = Outer { inner: Inner { n: 0 }, tag: 9 };\n\
               return touch(x, y);\n\
             }"
        );
        // Two-level GEP chain (Outer → Inner → n) — both loads/stores
        // through the chain should carry scope metadata.
        let touched = ir.lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(touched >= 2,
            "expected at least 2 scope-annotated loads/stores through GEP chains, got {touched}:\n{ir}");
    }

    #[test]
    fn extract_ptr_operand_finds_address_arg() {
        // White-box: confirm the parser used by annotate_one_line picks
        // off the `, ptr %X` operand from load/store/GEP forms.
        assert_eq!(
            extract_ptr_operand("load i32, ptr %t2, align 4").as_deref(),
            Some("%t2"),
        );
        assert_eq!(
            extract_ptr_operand("getelementptr inbounds %T, ptr %0, i32 0, i32 0").as_deref(),
            Some("%0"),
        );
        assert_eq!(
            extract_ptr_operand("store i32 7, ptr %slot").as_deref(),
            Some("%slot"),
        );
        // No address operand → None.
        assert_eq!(extract_ptr_operand("add i32 1, 2"), None);
    }
}
