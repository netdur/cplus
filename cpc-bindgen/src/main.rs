// cpc-bindgen — C header → C+ FFI declarations.
//
// MVP (v0.0.3 Phase 4): walks a C header via `clang -Xclang -ast-dump=json
// -fsyntax-only`, emits `extern fn` declarations for every top-level
// function and `#[repr(C)] struct` for every named record. Type mapping
// covers the C scalars, raw pointers, function pointers, and fixed arrays.
//
// Usage: `cpc-bindgen <header.h> [-- <extra clang args>...]`
//   - Output is on stdout. Redirect to a `.cplus` file to use.
//
// Scope notes:
// - Unions are emitted as `#[repr(C)] struct U { _bytes: [u8; N] }` with no
//   typed field accessors (caller writes reinterpret-casts in `unsafe`).
//   The byte-array shim was the locked decision from v0.0.3 plan §4B.
// - Bitfields produce mask/shift accessor functions next to the parent
//   struct definition (§4C).
// - Functions taking/returning unsupported C types (long double, vector,
//   complex, etc.) are emitted with a `// SKIPPED: <reason>` comment.

mod framework;
mod objc;

use std::process::Command;

fn main() {
    // Flags (`--objc`, `--prefix P`) precede the header; clang args follow `--`.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut objc_mode = false;
    let mut prefix = String::new();
    let mut overrides_path: Option<String> = None;
    let mut framework: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut header: Option<String> = None;
    let mut clang_args: Vec<String> = Vec::new();
    let mut seen_dashdash = false;
    let mut i = 0;
    while i < raw.len() {
        let a = &raw[i];
        if !seen_dashdash {
            if a == "--" {
                seen_dashdash = true;
                i += 1;
                continue;
            }
            if a == "--objc" {
                objc_mode = true;
                i += 1;
                continue;
            }
            if a == "--prefix" {
                prefix = raw.get(i + 1).cloned().unwrap_or_default();
                i += 2;
                continue;
            }
            if let Some(p) = a.strip_prefix("--prefix=") {
                prefix = p.to_string();
                i += 1;
                continue;
            }
            if a == "--overrides" {
                overrides_path = raw.get(i + 1).cloned();
                i += 2;
                continue;
            }
            if let Some(p) = a.strip_prefix("--overrides=") {
                overrides_path = Some(p.to_string());
                i += 1;
                continue;
            }
            if a == "--framework" {
                framework = raw.get(i + 1).cloned();
                i += 2;
                continue;
            }
            if let Some(p) = a.strip_prefix("--framework=") {
                framework = Some(p.to_string());
                i += 1;
                continue;
            }
            if a == "--out" {
                out_dir = raw.get(i + 1).cloned();
                i += 2;
                continue;
            }
            if let Some(p) = a.strip_prefix("--out=") {
                out_dir = Some(p.to_string());
                i += 1;
                continue;
            }
            if header.is_none() && !a.starts_with('-') {
                header = Some(a.clone());
                i += 1;
                continue;
            }
        }
        clang_args.push(a.clone());
        i += 1;
    }
    // Framework mode: generate a whole package from an Apple system framework
    // (no single header — the framework's umbrella header drives discovery).
    if let Some(fw) = &framework {
        std::process::exit(framework::generate(
            fw,
            &prefix,
            overrides_path.as_deref(),
            out_dir.as_deref(),
        ));
    }

    let header = match header {
        Some(h) => h,
        None => {
            eprintln!("cpc-bindgen — native header → C+ binding");
            eprintln!();
            eprintln!("usage: cpc-bindgen [--objc] [--prefix P] <header.h> [-- <clang args>...]");
            eprintln!("       cpc-bindgen --framework <Name> [--prefix P] [--overrides F] [--out DIR]");
            std::process::exit(2);
        }
    };

    // ObjC needs the SDK sysroot; default it from `xcrun` when not supplied.
    if objc_mode && !clang_args.iter().any(|a| a == "-isysroot") {
        if let Ok(out) = Command::new("xcrun").arg("--show-sdk-path").output() {
            if out.status.success() {
                let sdk = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !sdk.is_empty() {
                    clang_args.push("-isysroot".into());
                    clang_args.push(sdk);
                }
            }
        }
    }

    // Shell out to clang for a JSON AST dump. `-fsyntax-only` skips codegen;
    // the JSON is what we want. We filter to decls actually in `header`.
    let lang = if objc_mode { "objective-c" } else { "c" };
    let mut cmd = Command::new("clang");
    cmd.arg("-Xclang")
        .arg("-ast-dump=json")
        .arg("-fsyntax-only")
        .arg("-x")
        .arg(lang);
    for a in &clang_args {
        cmd.arg(a);
    }
    cmd.arg(&header);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("cpc-bindgen: failed to invoke clang: {e}");
            std::process::exit(1);
        }
    };
    if !out.status.success() {
        eprintln!("cpc-bindgen: clang failed:");
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        std::process::exit(1);
    }

    let v: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cpc-bindgen: clang JSON parse failed: {e}");
            std::process::exit(1);
        }
    };

    if objc_mode {
        let overrides = match &overrides_path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                    eprintln!("cpc-bindgen: overrides `{p}` parse failed: {e}");
                    std::process::exit(1);
                }),
                Err(e) => {
                    eprintln!("cpc-bindgen: cannot read overrides `{p}`: {e}");
                    std::process::exit(1);
                }
            },
            None => serde_json::Value::Null,
        };
        let emitter = objc::ObjcEmitter::new(&header, &prefix, overrides);
        print!("{}", emitter.run(&v));
    } else {
        let mut emitter = Emitter::new(&header);
        emitter.walk(&v);
        print!("{}", emitter.finish());
    }
}

struct Emitter {
    header_path: String,
    out: String,
    seen_records: std::collections::HashSet<String>,
    // Function names already emitted — some headers (clapack.h) redeclare a
    // symbol; we bind it once.
    seen_fns: std::collections::HashSet<String>,
    // Functions / typedefs deferred until structs they depend on are emitted.
    deferred_fns: Vec<String>,
    /// Cached top-level decls so emit_typedef can resolve an anonymous
    /// RecordDecl by id.
    last_tu_inner: Option<Vec<serde_json::Value>>,
    /// Every typedef in the TU (including dependency headers), name -> underlying
    /// spelling, so types from included headers (vDSP_Length, FFTSetup, ...)
    /// resolve to concrete C+ even though we only *emit* header-local decls.
    typedefs: std::collections::HashMap<String, String>,
}

impl Emitter {
    fn new(header_path: &str) -> Self {
        let mut out = String::new();
        out.push_str("// Auto-generated by cpc-bindgen. DO NOT EDIT.\n");
        out.push_str(&format!("// Source header: {header_path}\n"));
        out.push_str("//\n");
        out.push_str("// Every declaration below targets the C ABI. Calls into these from\n");
        out.push_str("// C+ require `{ ... }` blocks.\n\n");
        Emitter {
            header_path: header_path.to_string(),
            out,
            seen_records: std::collections::HashSet::new(),
            seen_fns: std::collections::HashSet::new(),
            deferred_fns: Vec::new(),
            last_tu_inner: None,
            typedefs: std::collections::HashMap::new(),
        }
    }

    fn walk(&mut self, tu: &serde_json::Value) {
        let inner = match tu.get("inner").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return,
        };
        self.last_tu_inner = Some(inner.clone());
        // Pre-pass: index every typedef in the TU (incl. dependency headers) so
        // cross-header types resolve during emission.
        for decl in inner {
            if decl.get("kind").and_then(|v| v.as_str()) == Some("TypedefDecl") {
                if let (Some(name), Some(qt)) = (
                    decl.get("name").and_then(|v| v.as_str()),
                    decl.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()),
                ) {
                    self.typedefs.insert(name.to_string(), qt.to_string());
                }
            }
        }
        // Two-pass: structs/typedefs first so functions reference defined types.
        for decl in inner {
            if !self.decl_in_header(decl) {
                continue;
            }
            match decl.get("kind").and_then(|v| v.as_str()) {
                Some("RecordDecl") => self.emit_record(decl),
                Some("TypedefDecl") => self.emit_typedef(decl),
                Some("EnumDecl") => self.emit_enum(decl),
                _ => {}
            }
        }
        for decl in inner {
            if !self.decl_in_header(decl) {
                continue;
            }
            if decl.get("kind").and_then(|v| v.as_str()) == Some("FunctionDecl") {
                self.emit_function(decl);
            }
        }
        // Flush any deferred items.
        for line in std::mem::take(&mut self.deferred_fns) {
            self.out.push_str(&line);
        }
    }

    /// Replace typedef-name tokens with their underlying spelling (recursively),
    /// so a C type built from dependency-header typedefs resolves to a concrete
    /// spelling before mapping. `fuel` guards against self-referential typedefs.
    fn expand(&self, qt: &str, fuel: u32) -> String {
        if fuel == 0 {
            return qt.to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        for tok in qt.replace('*', " * ").split_whitespace() {
            match self.typedefs.get(tok) {
                Some(under) if under.trim() != tok => parts.push(self.expand(under, fuel - 1)),
                _ => parts.push(tok.to_string()),
            }
        }
        parts.join(" ")
    }

    /// Map a C type to C+, first expanding typedefs from the whole TU, and
    /// treating the structs we've emitted (`seen_records`) as complete (so
    /// they're usable by value and as typed pointers).
    fn map_type(&self, qt: &str) -> Result<String, String> {
        map_c_type_to_cplus(&self.expand(qt, 16), &self.seen_records)
    }

    /// True iff `decl` originated from the user's header (not a system include).
    /// Filter on `loc.file` matching the header path's basename — clang's
    /// JSON elides file fields for repeated locations, so we treat absent
    /// fields as "same file as previous decl" (sticky). To keep MVP small
    /// we approximate: if the loc has an explicit file, it must match our
    /// header; if no file, we assume it's from our header too (stays
    /// sticky to the last loc, which started at our TU).
    fn decl_in_header(&self, decl: &serde_json::Value) -> bool {
        let loc = decl.get("loc");
        let file = loc.and_then(|l| l.get("file")).and_then(|f| f.as_str());
        let included_from = loc.and_then(|l| l.get("includedFrom"));
        if included_from.is_some() {
            // Anything in an included sub-header is system / dependency code.
            return false;
        }
        match file {
            Some(f) => {
                let basename = |p: &str| -> String {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                };
                basename(f) == basename(&self.header_path)
            }
            None => true,
        }
    }

    fn finish(self) -> String {
        self.out
    }

    fn emit_function(&mut self, decl: &serde_json::Value) {
        let name = match decl.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        // Implicit-decl noise from <stdarg.h> etc. — clang sometimes emits a
        // declaration with isImplicit=true that we should ignore.
        if decl.get("isImplicit").and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        // `static`/`static inline` functions (e.g. Foundation's NS_INLINE
        // NSMakeRange) have no external symbol to link against — skip them.
        if decl.get("storageClass").and_then(|v| v.as_str()) == Some("static") {
            return;
        }
        // Some headers (e.g. clapack.h) declare a symbol more than once; bind it
        // only the first time.
        if !self.seen_fns.insert(name.clone()) {
            return;
        }
        let qual_type = decl
            .get("type")
            .and_then(|t| t.get("qualType"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let (ret_c, params_c, variadic) = match parse_fn_qual_type(qual_type) {
            Some(parts) => parts,
            None => {
                self.out.push_str(&format!(
                    "// SKIPPED `{name}`: unparseable function type `{qual_type}`\n"
                ));
                return;
            }
        };
        let ret_cplus = match self.map_type(&ret_c) {
            Ok(t) => t,
            Err(why) => {
                self.out
                    .push_str(&format!("// SKIPPED `{name}`: return type — {why}\n"));
                return;
            }
        };
        // Pull parameter names from the AST if present (better-than-anon).
        let param_names: Vec<String> = decl
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|d| d.get("kind").and_then(|k| k.as_str()) == Some("ParmVarDecl"))
                    .map(|d| {
                        d.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut params_out: Vec<String> = Vec::with_capacity(params_c.len());
        let mut arg_names: Vec<String> = Vec::with_capacity(params_c.len());
        for (i, p_c) in params_c.iter().enumerate() {
            let p_cplus = match self.map_type(p_c) {
                Ok(t) => t,
                Err(why) => {
                    self.out.push_str(&format!(
                        "// SKIPPED `{name}`: param {i} type `{p_c}` — {why}\n"
                    ));
                    return;
                }
            };
            let pname = param_names
                .get(i)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("arg{i}"));
            let pname = sanitize_ident(&pname);
            arg_names.push(pname.clone());
            params_out.push(format!("{}: {}", pname, p_cplus));
        }
        if variadic {
            let mut line = format!("extern fn {name}(");
            line.push_str(&params_out.join(", "));
            if !params_out.is_empty() {
                line.push_str(", ");
            }
            line.push_str("...");
            line.push(')');
            if ret_cplus != "()" {
                line.push_str(&format!(" -> {ret_cplus}"));
            }
            line.push_str(";\n");
            self.out.push_str(&line);
            return;
        }

        let c_name = format!("__c_{name}");
        let mut line = format!("#[link_name = \"{name}\"]\nextern fn {c_name}(");
        line.push_str(&params_out.join(", "));
        line.push(')');
        if ret_cplus != "()" {
            line.push_str(&format!(" -> {ret_cplus}"));
        }
        line.push_str(";\n");
        line.push_str(&format!("fn {name}("));
        line.push_str(&params_out.join(", "));
        line.push(')');
        if ret_cplus != "()" {
            line.push_str(&format!(" -> {ret_cplus}"));
        }
        line.push_str(" {\n");
        let call = format!("{}({})", c_name, arg_names.join(", "));
        if ret_cplus == "()" {
            line.push_str(&format!("    {{ {call}; }}\n    return;\n"));
        } else {
            line.push_str(&format!("    return {{ {call} }};\n"));
        }
        line.push_str("}\n");
        self.out.push_str(&line);
    }

    fn emit_record(&mut self, decl: &serde_json::Value) {
        // RecordDecl: struct or union, possibly anonymous. We emit named
        // structs only; anonymous records get materialized inline when a
        // typedef wraps them.
        let name = decl.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return;
        }
        let kind = decl
            .get("tagUsed")
            .and_then(|v| v.as_str())
            .unwrap_or("struct");
        if !decl
            .get("completeDefinition")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            // Forward declaration only. Skip — typedefs may reference it
            // and we map them as opaque pointers.
            return;
        }
        if self.seen_records.insert(name.to_string()) {
            // already emitted check
        } else {
            return;
        }
        if kind == "union" {
            // §4B: byte-array shim. Compute size as max(field size).
            // Without layout info from clang we use the `_size` from the
            // record's `definitionData` if present, else punt with a comment.
            let size_bytes = guess_record_size(decl);
            let size = size_bytes.unwrap_or(8); // conservative fallback
            self.out.push_str(&format!(
                "#[repr(C)] struct {name} {{ _bytes: [u8; {size}] }}\n"
            ));
            self.out.push_str(&format!(
                "// `{name}` is a C union — fields share storage. Access fields\n\
                 // via `unsafe` reinterpret cast: `let p = (&u._bytes) as *<FieldTy>;`\n"
            ));
            return;
        }
        // struct: walk fields, emit a #[repr(C)] struct.
        self.out.push_str(&format!("#[repr(C)] struct {name} {{\n"));
        let inner = decl
            .get("inner")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut bitfields: Vec<(String, String, u32, u32)> = Vec::new(); // (parent, name, bit_offset, width)
        let mut bit_cursor: u32 = 0;
        let mut storage_field_idx = 0u32;
        let mut plain_fields: Vec<(String, String)> = Vec::new();
        let mut has_ptr_field = false;
        for field in &inner {
            if field.get("kind").and_then(|k| k.as_str()) != Some("FieldDecl") {
                continue;
            }
            let fname = sanitize_ident(field.get("name").and_then(|n| n.as_str()).unwrap_or(""));
            let qt = field
                .get("type")
                .and_then(|t| t.get("qualType"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // Bitfield: `isBitfield=true` with a width sub-expression.
            if field.get("isBitfield").and_then(|v| v.as_bool()) == Some(true) {
                let width = bitfield_width(field).unwrap_or(0);
                bitfields.push((name.to_string(), fname.clone(), bit_cursor, width));
                bit_cursor += width;
                // Continue scanning; we collapse all bitfields into a
                // single u32 storage slot named `_packed{idx}`.
                continue;
            } else if !bitfields.is_empty() {
                // Flush the accumulated bitfield run into a storage field.
                let bytes = ((bit_cursor + 7) / 8).max(1);
                let _ = bytes;
                self.out
                    .push_str(&format!("    _packed{storage_field_idx}: u32,\n"));
                storage_field_idx += 1;
                bit_cursor = 0;
            }
            let cplus_ty = match self.map_type(qt) {
                Ok(t) => t,
                Err(why) => {
                    self.out
                        .push_str(&format!("    // SKIPPED field `{fname}: {qt}` — {why}\n"));
                    continue;
                }
            };
            // Raw-pointer fields must be `opaque` (C+ ownership: the struct
            // doesn't own them). That makes them un-settable cross-module, so a
            // constructor fn is emitted below for structs that have any.
            let is_ptr = cplus_ty.starts_with('*');
            has_ptr_field = has_ptr_field || is_ptr;
            let prefix = if is_ptr { "opaque " } else { "" };
            let cname = sanitize_ident(&fname);
            self.out.push_str(&format!("    {prefix}{cname}: {cplus_ty},\n"));
            plain_fields.push((cname, cplus_ty));
        }
        if !bitfields.is_empty() {
            self.out
                .push_str(&format!("    _packed{storage_field_idx}: u32,\n"));
        }
        self.out.push_str("}\n");
        // Constructor for structs with opaque (pointer) fields, since callers in
        // other modules can't set opaque fields via a struct literal.
        if has_ptr_field && !plain_fields.is_empty() {
            let params = plain_fields
                .iter()
                .map(|(n, t)| format!("{n}: {t}"))
                .collect::<Vec<_>>()
                .join(", ");
            let inits = plain_fields
                .iter()
                .map(|(n, _)| format!("{n}: {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            self.out
                .push_str(&format!("fn {name}_new({params}) -> {name} {{\n    return {name} {{ {inits} }};\n}}\n"));
        }
        // Emit bitfield accessors after the struct.
        for (parent, fname, off, width) in bitfields {
            if width == 0 {
                continue;
            }
            let mask: u64 = if width >= 32 {
                0xFFFF_FFFF
            } else {
                (1u64 << width) - 1
            };
            self.out.push_str(&format!(
                "impl {parent} {{\n\
                 \x20   fn {fname}(self) -> u32 {{ return (self._packed0 >> ({off} as u32)) & ({mask} as u32); }}\n\
                 }}\n",
            ));
        }
    }

    fn emit_typedef(&mut self, decl: &serde_json::Value) {
        let name = decl.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return;
        }
        // Simple typedef (`typedef int32_t llama_token;`) — emit a public C+
        // alias so later generated declarations can refer to it.
        let qual_type = decl
            .get("type")
            .and_then(|t| t.get("qualType"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Function-pointer typedefs (`void (*Proc)(int)`) — bind as an opaque
        // code pointer so functions taking them still resolve.
        if qual_type.contains("(*") {
            self.out
                .push_str(&format!("type {} = *u8;\n", sanitize_ident(name)));
            return;
        }
        if let Some((lhs, rhs)) = qual_type.rsplit_once(' ') {
            if rhs == name
                && lhs != "struct"
                && lhs != "union"
                && !lhs.contains("struct ")
                && !lhs.contains("union ")
            {
                if let Ok(cplus_ty) = map_c_type_to_cplus(lhs.trim(), &self.seen_records) {
                    self.out
                        .push_str(&format!("type {} = {};\n", sanitize_ident(name), cplus_ty));
                    return;
                }
            }
        }
        // If the typedef wraps an anonymous record (the `typedef struct
        // { int x; int y; } Point;` shape), emit the struct under the
        // typedef's name. Clang represents this as:
        //   TypedefDecl name="Point"
        //     inner[0] = ElaboratedType
        //       ownedTagDecl = RecordDecl (anonymous, with fields)
        //       inner[0] = RecordType → decl points back at the same record
        let inner = decl.get("inner").and_then(|v| v.as_array());
        let elaborated = inner.and_then(|a| {
            a.iter()
                .find(|x| x.get("kind").and_then(|v| v.as_str()) == Some("ElaboratedType"))
        });
        let Some(elab) = elaborated else {
            return;
        };
        // The owned RecordDecl carries `kind` + `name`. If it's anonymous
        // (name empty), pull its fields by id from the TU. Simpler: clang
        // sometimes inlines the fields directly under the ElaboratedType's
        // RecordType.decl — but for the anonymous case we have to look
        // up by id. For MVP, leverage the fact that the typedef body
        // includes the full RecordDecl as an inner item in newer clangs.
        let record = elab
            .get("inner")
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|x| x.get("kind").and_then(|v| v.as_str()) == Some("RecordType"))
            })
            .and_then(|rt| rt.get("decl"))
            .and_then(|d| d.get("id"))
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());
        let Some(rec_id) = record else {
            return;
        };
        // Find the original RecordDecl in the TU by id.
        let Some(tu_inner) = self.last_tu_inner.as_ref() else {
            return;
        };
        let original = tu_inner.iter().find(|d| {
            d.get("kind").and_then(|v| v.as_str()) == Some("RecordDecl")
                && d.get("id").and_then(|v| v.as_str()) == Some(rec_id.as_str())
        });
        let Some(orig) = original else {
            return;
        };
        // Synthesize a named record by cloning + injecting the name.
        let mut synth = orig.clone();
        synth["name"] = serde_json::Value::String(name.to_string());
        self.emit_record(&synth);
    }

    fn emit_enum(&mut self, decl: &serde_json::Value) {
        // C enums pass as `int` at the ABI, and C+ has no top-level const, so
        // each constant becomes an `i32`-returning accessor fn (the stdlib idiom,
        // cf. `events`/`layout`): `cblas_dgemm(cblas::CblasRowMajor(), ...)`.
        let name = decl.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let label = if name.is_empty() { "(anonymous)" } else { name };
        self.out
            .push_str(&format!("// C enum `{label}` — constants as i32 accessors.\n"));
        let mut next: i64 = 0;
        for c in decl
            .get("inner")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if c.get("kind").and_then(|k| k.as_str()) != Some("EnumConstantDecl") {
                continue;
            }
            let cname = match c.get("name").and_then(|n| n.as_str()) {
                Some(n) if !n.is_empty() => n,
                _ => continue,
            };
            let val = read_enum_value(c).unwrap_or(next);
            next = val + 1;
            self.out.push_str(&format!(
                "fn {}() -> i32 {{ return {} as i32; }}\n",
                sanitize_ident(cname),
                val
            ));
        }
    }
}

/// Parse a clang `qualType` of a function: `RET (P1, P2, ...)`.
/// Returns `(ret, params, is_variadic)` or `None` if the shape doesn't match.
fn parse_fn_qual_type(qt: &str) -> Option<(String, Vec<String>, bool)> {
    // Find the outermost paren group. Clang's qualType uses standard C
    // declaration ordering — return type then `(arg, arg, ...)`.
    let open = qt.find('(')?;
    let close = qt.rfind(')')?;
    if close <= open {
        return None;
    }
    let ret = qt[..open].trim().to_string();
    let inside = &qt[open + 1..close];
    let mut params: Vec<String> = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in inside.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    params.push(t);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let last = cur.trim().to_string();
    if !last.is_empty() {
        params.push(last);
    }
    // Detect `...` variadic marker.
    let mut variadic = false;
    if let Some(last) = params.last() {
        if last == "..." {
            variadic = true;
        }
    }
    if variadic {
        params.pop();
    }
    // `(void)` is an empty param list in C.
    if params.len() == 1 && params[0] == "void" {
        params.clear();
    }
    Some((ret, params, variadic))
}

/// Map a C type spelling (as it appears in clang's `qualType`) to a C+ type.
/// Returns a string suitable for the C+ source, or `Err` with a reason
/// when the type isn't mappable.
fn map_c_type_to_cplus(
    c_ty: &str,
    complete: &std::collections::HashSet<String>,
) -> Result<String, String> {
    // Function pointers (inline `RET (*)(args)` or typedef'd) -> opaque code
    // pointer. Checked on the raw spelling before `*` normalization below.
    if c_ty.contains("(*") {
        return Ok("*u8".to_string());
    }
    // Normalize the spelling: put spaces around every `*`, then drop the
    // qualifier tokens (`const`, nullability, ...) — they don't affect the
    // C+ FFI type. This handles interior const (`float * const *`), nullability
    // (`float * _Nonnull`), and odd spacing uniformly.
    let normalized: String = c_ty
        .replace('*', " * ")
        .split_whitespace()
        .filter(|t| {
            !matches!(
                *t,
                "const" | "volatile" | "restrict"
                    | "_Nonnull" | "_Nullable" | "_Null_unspecified"
                    | "__nonnull" | "__nullable"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let s = normalized.trim();
    if s.is_empty() {
        return Err("empty type".to_string());
    }
    // Pointer types — `T *` or `T*`. Be greedy: each `*` consumes one
    // level. Recurse on the inner.
    if let Some(inner) = s.strip_suffix('*') {
        let inner = inner.trim();
        // `void *` is an opaque byte pointer in C+, not a pointer-to-unit.
        if inner == "void" {
            return Ok("*u8".to_string());
        }
        // A pointer to an unknown / opaque / incomplete type is an opaque handle;
        // a pointer to a known complete struct is a typed pointer.
        return match map_c_type_to_cplus(inner, complete) {
            Ok(t) => Ok(format!("*{t}")),
            Err(_) => Ok("*u8".to_string()),
        };
    }
    // Fixed-size array — `T[N]`. Treat as pointer at FFI boundary
    // (matches C decay rules for function params).
    if let Some(bracket) = s.find('[') {
        let inner = s[..bracket].trim();
        return match map_c_type_to_cplus(inner, complete) {
            Ok(t) => Ok(format!("*{t}")),
            Err(_) => Ok("*u8".to_string()),
        };
    }
    Ok(match s {
        "void" => "()".to_string(),
        "_Bool" | "bool" => "bool".to_string(),
        "char" | "signed char" => "i8".to_string(),
        "unsigned char" => "u8".to_string(),
        "short" | "signed short" | "short int" | "signed short int" => "i16".to_string(),
        "unsigned short" | "unsigned short int" => "u16".to_string(),
        "int" | "signed" | "signed int" => "i32".to_string(),
        "unsigned" | "unsigned int" => "u32".to_string(),
        "long"
        | "signed long"
        | "long int"
        | "signed long int"
        | "long long"
        | "signed long long"
        | "long long int"
        | "signed long long int" => "i64".to_string(),
        "unsigned long" | "unsigned long int" | "unsigned long long" | "unsigned long long int" => {
            "u64".to_string()
        }
        "float" => "f32".to_string(),
        "double" => "f64".to_string(),
        "size_t" => "usize".to_string(),
        "ssize_t" => "isize".to_string(),
        "intptr_t" => "isize".to_string(),
        "uintptr_t" => "usize".to_string(),
        "int8_t" => "i8".to_string(),
        "uint8_t" => "u8".to_string(),
        "int16_t" => "i16".to_string(),
        "uint16_t" => "u16".to_string(),
        "int32_t" => "i32".to_string(),
        "uint32_t" => "u32".to_string(),
        "int64_t" => "i64".to_string(),
        "uint64_t" => "u64".to_string(),
        "long double" => return Err("long double unsupported".to_string()),
        // C enums pass as `int` at the ABI.
        s if s.starts_with("enum ") => "i32".to_string(),
        // Struct/union: usable by value (or as a typed pointer above) only if we
        // emitted a complete definition; otherwise Err -> opaque `*u8` (pointer)
        // or `// SKIPPED` (by value).
        s if s.starts_with("struct ") || s.starts_with("union ") => {
            let name = s.trim_start_matches("struct ").trim_start_matches("union ");
            if complete.contains(name) {
                name.to_string()
            } else {
                return Err(format!("incomplete record `{name}`"));
            }
        }
        s if complete.contains(s) => s.to_string(),
        _ => return Err(format!("unsupported type `{s}`")),
    })
}

fn guess_record_size(decl: &serde_json::Value) -> Option<u64> {
    // Clang's JSON output occasionally includes a `definitionData.sizeof`
    // key. If not present, scan fields and sum sized scalars (rough).
    if let Some(d) = decl.get("definitionData") {
        if let Some(n) = d.get("sizeof").and_then(|v| v.as_u64()) {
            return Some(n);
        }
    }
    None
}

/// Read an enum constant's value from its initializer (clang nests it under
/// ConstantExpr/IntegerLiteral). `None` if implicit (caller uses the counter).
fn read_enum_value(node: &serde_json::Value) -> Option<i64> {
    fn search(v: &serde_json::Value) -> Option<i64> {
        if let Some(s) = v.get("value").and_then(|x| x.as_str()) {
            if let Ok(n) = s.parse::<i64>() {
                return Some(n);
            }
        }
        for c in v.get("inner").and_then(|x| x.as_array()).into_iter().flatten() {
            if let Some(n) = search(c) {
                return Some(n);
            }
        }
        None
    }
    node.get("inner")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .find_map(search)
}

fn bitfield_width(field: &serde_json::Value) -> Option<u32> {
    // Clang's JSON nests the bitfield width as either:
    //   inner[0].kind == "ConstantExpr" with `value: "N"` (modern clang)
    //   inner[0].kind == "ConstantExpr" → inner[0].inner[0].kind == "IntegerLiteral" with `value: "N"`
    //   inner[0].kind == "IntegerLiteral" with `value: "N"` (older clang)
    fn read_value(v: &serde_json::Value) -> Option<u32> {
        v.get("value")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<u32>().ok())
    }
    let inner = field.get("inner")?.as_array()?;
    for entry in inner {
        let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "ConstantExpr" || kind == "IntegerLiteral" {
            if let Some(n) = read_value(entry) {
                return Some(n);
            }
            // Recurse one level for the nested IntegerLiteral case.
            if let Some(arr) = entry.get("inner").and_then(|v| v.as_array()) {
                for sub in arr {
                    if let Some(n) = read_value(sub) {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

/// Append `_` when a C / Objective-C identifier collides with a C+ keyword (an
/// ObjC `type`/`for` param, a C `void *opaque` field, ...). Shared by both the C
/// and ObjC front-ends.
pub(crate) fn sanitize_ident(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }
    const RESERVED: &[&str] = &[
        "as", "async", "await", "borrow", "break", "const", "continue", "defer", "else", "enum",
        "export", "extern", "false", "fn", "for", "gen", "guard", "if", "impl", "import", "in",
        "let", "loop", "match", "move", "mut", "opaque", "pub", "ref", "restrict", "return", "self",
        "Self", "static", "struct", "take", "this", "true", "type", "unsafe", "var", "while",
    ];
    if RESERVED.iter().any(|r| *r == name) {
        return format!("{name}_");
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_map() {
        let c: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(map_c_type_to_cplus("int", &c).unwrap(), "i32");
        assert_eq!(map_c_type_to_cplus("unsigned int", &c).unwrap(), "u32");
        assert_eq!(map_c_type_to_cplus("size_t", &c).unwrap(), "usize");
        assert_eq!(map_c_type_to_cplus("char *", &c).unwrap(), "*i8");
        assert_eq!(map_c_type_to_cplus("const char *", &c).unwrap(), "*i8");
        assert_eq!(map_c_type_to_cplus("void", &c).unwrap(), "()");
        assert_eq!(map_c_type_to_cplus("uint32_t **", &c).unwrap(), "**u32");
    }

    #[test]
    fn fn_qual_type_parses() {
        let (ret, params, var) = parse_fn_qual_type("int (int, const char *)").unwrap();
        assert_eq!(ret, "int");
        assert_eq!(params, vec!["int", "const char *"]);
        assert!(!var);
    }

    #[test]
    fn fn_qual_type_variadic() {
        let (ret, params, var) = parse_fn_qual_type("int (const char *, ...)").unwrap();
        assert_eq!(ret, "int");
        assert_eq!(params, vec!["const char *"]);
        assert!(var);
    }

    #[test]
    fn fn_qual_type_void_params() {
        let (ret, params, var) = parse_fn_qual_type("void (void)").unwrap();
        assert_eq!(ret, "void");
        assert!(params.is_empty());
        assert!(!var);
    }
}
