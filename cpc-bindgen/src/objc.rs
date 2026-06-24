// cpc-bindgen ObjC front-end — Objective-C framework header -> C+ wrapper.
//
// Walks `clang -x objective-c -ast-dump=json` for ObjCInterfaceDecl / EnumDecl
// nodes in the target header and emits C+ wrappers calling through the shared
// `objc` runtime (`vendor/objc`): msgSend shims, the str/NSString bridge, the
// `Range` (NSRange) value type, and the +1/`drop` ownership model.
//
// Modelled so far: classes with init / initWith… constructors; instance and
// class methods over void / object / string / scalar / NSRange / enum types;
// nullable string returns -> Option[Text]; NS_ENUM -> C+ enum (+ raw/from_raw);
// NSArray<NSValue *> -> Vec[Range]. Everything else (blocks, multi-arg
// selectors, generic collections) is emitted as a `// SKIPPED` comment rather
// than wrong code. Naming is mechanical snake_case; guideline-level renames are
// a later override file's job.

use std::collections::HashMap;

pub struct ObjcEmitter {
    header_path: String,
    prefix: String,
    overrides: serde_json::Value,
    body: String,
    needs_vec: bool,
    typedefs: HashMap<String, String>,
    enums: HashMap<String, EnumInfo>,
    used_enums: Vec<String>,
}

#[derive(Clone)]
struct EnumInfo {
    objc_name: String,
    cplus_name: String,
    raw_fn: String,
    from_raw_fn: String,
    variants: Vec<(String, i64)>,
}

enum Ret {
    Void,
    Object,
    ObjectOption, // nullable object handle -> Option[*u8]
    Text { nullable: bool },
    Bool,
    ScalarI64,
    ScalarU64,
    EnumTy(String),
    Range,
    ValueArray, // NSArray<NSValue *> -> Vec[Range]
    Unsupported(String),
}

enum Arg {
    Id(String),       // object / string already lowered to an id expression
    Bool(String),     // BOOL param (the C+ bool name; lowered to i8 on the wire)
    ScalarI64(String),
    ScalarU64(String),
    Range(String),
    Unsupported(String),
}

impl ObjcEmitter {
    pub fn new(header_path: &str, prefix: &str, overrides: serde_json::Value) -> Self {
        ObjcEmitter {
            header_path: header_path.to_string(),
            prefix: prefix.to_string(),
            overrides,
            body: String::new(),
            needs_vec: false,
            typedefs: HashMap::new(),
            enums: HashMap::new(),
            used_enums: Vec::new(),
        }
    }

    // --- override-file lookups (the hand-curated naming taste) ---

    fn type_override(&self, objc: &str) -> Option<String> {
        self.overrides.get("types").and_then(|t| t.get(objc)).and_then(|v| v.as_str()).map(String::from)
    }

    fn method_override<'a>(&'a self, class: &str, sel: &str) -> Option<&'a serde_json::Value> {
        self.overrides.get("methods").and_then(|m| m.get(class)).and_then(|c| c.get(sel))
    }

    fn is_skipped(&self, class: &str, sel: &str) -> bool {
        self.overrides
            .get("skip")
            .and_then(|s| s.get(class))
            .and_then(|a| a.as_array())
            .map(|a| a.iter().any(|x| x.as_str() == Some(sel)))
            .unwrap_or(false)
    }

    pub fn run(mut self, tu: &serde_json::Value) -> String {
        let inner = match tu.get("inner").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return self.preamble(),
        };
        // Pass 1: typedefs (name -> underlying) and NS_ENUM declarations.
        for decl in inner {
            match decl.get("kind").and_then(|v| v.as_str()) {
                Some("TypedefDecl") => {
                    if let (Some(name), Some(under)) = (
                        decl.get("name").and_then(|v| v.as_str()),
                        decl.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()),
                    ) {
                        self.typedefs.insert(name.to_string(), under.to_string());
                    }
                }
                Some("EnumDecl") => self.collect_enum(decl),
                _ => {}
            }
        }
        // Pass 2: interface wrappers (sticky-file filter excludes builtins).
        let target = base(&self.header_path);
        let mut current_file: Option<String> = None;
        for decl in inner {
            if let Some(loc) = decl.get("loc") {
                if let Some(f) = loc_file(loc) {
                    current_file = Some(f);
                }
            }
            if decl.get("kind").and_then(|v| v.as_str()) != Some("ObjCInterfaceDecl") {
                continue;
            }
            if decl.get("loc").map(loc_included).unwrap_or(false) {
                continue;
            }
            if current_file.as_deref().map(base) == Some(target.clone()) {
                self.emit_interface(decl);
            }
        }

        // Assemble: preamble + the enum defs actually used + interface bodies.
        let mut out = self.preamble();
        for objc_name in self.used_enums.clone() {
            if let Some(info) = self.enums.get(&objc_name).cloned() {
                out.push_str(&self.render_enum(&info));
            }
        }
        out.push_str(&self.body);
        out
    }

    fn preamble(&self) -> String {
        let mut p = String::new();
        p.push_str("// Auto-generated by cpc-bindgen (--objc). DO NOT EDIT.\n");
        p.push_str(&format!("// Source header: {}\n//\n", self.header_path));
        p.push_str("import \"objc/runtime\" as rt;\n");
        p.push_str("import \"objc/bridge\" as bridge;\n");
        p.push_str("import \"stdlib/text\" as text;\n");
        p.push_str("import \"stdlib/option\" as option;\n");
        if self.needs_vec {
            p.push_str("import \"stdlib/vec\" as vec;\n");
        }
        p.push('\n');
        p
    }

    fn collect_enum(&mut self, decl: &serde_json::Value) {
        let objc_name = match decl.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => return,
        };
        if self.enums.contains_key(&objc_name) {
            return; // clang lists the EnumDecl twice (standalone + typedef'd)
        }
        // Only integer-backed enums; others (rare) are skipped.
        let mut variants: Vec<(String, i64)> = Vec::new();
        let mut next: i64 = 0;
        for c in decl.get("inner").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if c.get("kind").and_then(|k| k.as_str()) != Some("EnumConstantDecl") {
                continue;
            }
            let cname = c.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if cname.is_empty() {
                continue;
            }
            let val = read_int(&c).unwrap_or(next);
            let variant = strip_prefix(cname, &objc_name);
            variants.push((pascal(&variant), val));
            next = val + 1;
        }
        if variants.is_empty() {
            return;
        }
        let cplus_name = self.cplus_type_name(&objc_name);
        let snake_name = snake(&cplus_name);
        self.enums.insert(
            objc_name.clone(),
            EnumInfo {
                objc_name,
                cplus_name,
                raw_fn: format!("{snake_name}_raw"),
                from_raw_fn: format!("{snake_name}_from_raw"),
                variants,
            },
        );
    }

    fn render_enum(&self, e: &EnumInfo) -> String {
        let mut s = String::new();
        s.push_str(&format!("// `{}` (NS_ENUM)\n", e.objc_name));
        s.push_str(&format!("enum {} {{\n", e.cplus_name));
        for (v, _) in &e.variants {
            s.push_str(&format!("    {v},\n"));
        }
        s.push_str("}\n\n");
        // raw: enum -> integer
        s.push_str(&format!("fn {}(v: {}) -> i64 {{\n    return match v {{\n", e.raw_fn, e.cplus_name));
        for (v, val) in &e.variants {
            s.push_str(&format!("        {}::{} => {{ {} as i64 }},\n", e.cplus_name, v, val));
        }
        s.push_str("    };\n}\n\n");
        // from_raw: integer -> enum (first variant is the default arm)
        s.push_str(&format!("fn {}(raw: i64) -> {} {{\n", e.from_raw_fn, e.cplus_name));
        for (v, val) in e.variants.iter().skip(1) {
            s.push_str(&format!("    if raw == ({} as i64) {{ return {}::{}; }}\n", val, e.cplus_name, v));
        }
        s.push_str(&format!("    return {}::{};\n}}\n\n", e.cplus_name, e.variants[0].0));
        s
    }

    fn cplus_type_name(&self, objc_name: &str) -> String {
        if let Some(over) = self.type_override(objc_name) {
            return over;
        }
        objc_name.strip_prefix(&self.prefix).unwrap_or(objc_name).to_string()
    }

    fn emit_interface(&mut self, itf: &serde_json::Value) {
        let objc_name = match itf.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        let ty = self.cplus_type_name(&objc_name);
        let methods: Vec<serde_json::Value> = itf
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter(|m| m.get("kind").and_then(|k| k.as_str()) == Some("ObjCMethodDecl")).cloned().collect())
            .unwrap_or_default();
        let owned = methods.iter().any(|m| {
            let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
            sel == "init" || sel.starts_with("initWith")
        });

        // Owned classes (have an init) carry the +1 and release in `drop`;
        // factory/singleton classes are non-owning, so the handle is `opaque`
        // (no drop — another owner frees it).
        let note = if owned {
            "Owned via alloc/init; `drop` releases the +1."
        } else {
            "Non-owning handle (factory/singleton); `opaque`, no drop."
        };
        let field = if owned { "_obj: *u8" } else { "opaque _obj: *u8" };
        self.body.push_str(&format!("// `{objc_name}` (Foundation/ObjC). {note}\n"));
        self.body.push_str(&format!("struct {ty} {{\n    {field},\n}}\n\n"));
        self.body.push_str(&format!("impl {ty} {{\n"));
        self.body.push_str("    fn raw(this) -> *u8 { return this._obj; }\n\n");

        let mut init_done = false;
        for m in &methods {
            let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let is_init = sel == "init" || sel.starts_with("initWith");
            if is_init {
                if init_done {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: extra init variant (one `new` per type)\n"));
                    continue;
                }
                init_done = true;
            }
            self.emit_method(m, &objc_name, &ty, is_init);
        }

        if owned {
            self.body.push_str("    fn drop(ref this) {\n        rt::release(this._obj);\n    }\n");
        }
        self.body.push_str("}\n\n");
    }

    fn emit_method(&mut self, m: &serde_json::Value, objc_class: &str, ty: &str, is_init: bool) {
        let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let is_instance = m.get("instance").and_then(|v| v.as_bool()).unwrap_or(false);
        let ret_qt = m.get("returnType").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()).unwrap_or("void");
        let params: Vec<(String, String)> = m
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("ParmVarDecl"))
                    .map(|p| {
                        (
                            p.get("name").and_then(|n| n.as_str()).unwrap_or("arg").to_string(),
                            p.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Override-file taste: skip directive, method-name and param-label renames.
        if self.is_skipped(objc_class, &sel) {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: override\n"));
            return;
        }
        let mov = self.method_override(objc_class, &sel).cloned();
        let ov_name: Option<String> = mov.as_ref().and_then(|o| o.get("name")).and_then(|v| v.as_str()).map(String::from);
        let ov_params: Vec<String> = mov
            .as_ref()
            .and_then(|o| o.get("params"))
            .and_then(|p| p.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Lower every argument: `sig_param` is the public C+ parameter list
        // (labels = override or AST param names, types = enum/str/Range/scalar),
        // `args` are the wire expressions (raw int / bridged NSString / ...).
        let mut sig_parts: Vec<String> = Vec::new();
        let mut args: Vec<Arg> = Vec::new();
        for (idx, (pname, pqt)) in params.iter().enumerate() {
            let pn = ov_params.get(idx).cloned().unwrap_or_else(|| snake(pname));
            let a = self.map_arg(pqt, &pn);
            if let Arg::Unsupported(why) = &a {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: param `{pqt}` — {why}\n"));
                return;
            }
            sig_parts.push(format!("{pn}: {}", self.param_sig_type(pqt)));
            args.push(a);
        }
        let sig_param = sig_parts.join(", ");

        // Constructors: alloc + send the init selector, wrap in Self.
        if is_init {
            let send = self.send_expr("alloced", &sel, &Ret::Object, &args);
            let send = match send {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: init arg shape not yet modelled\n"));
                    return;
                }
            };
            let header = if sig_param.is_empty() { String::new() } else { sig_param.clone() };
            self.body.push_str(&format!(
                "    fn new({header}) -> {ty} {{\n        let cls: *u8 = rt::get_class(#str_ptr(\"{objc_class}\\0\"));\n        let alloced: *u8 = rt::msg_id(cls, rt::sel(#str_ptr(\"alloc\\0\")));\n        return {ty} {{ _obj: {send} }};\n    }}\n\n"
            ));
            return;
        }

        let ret = self.map_ret(ret_qt);
        if let Ret::Unsupported(why) = &ret {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: return `{ret_qt}` — {why}\n"));
            return;
        }

        let recv = if is_instance {
            "this._obj".to_string()
        } else {
            format!("rt::get_class(#str_ptr(\"{objc_class}\\0\"))")
        };
        let receiver = if is_instance { "this" } else { "" };
        let name = ov_name.clone().unwrap_or_else(|| mechanical_name(&sel));

        // ValueArray is a multi-statement body; handle it separately.
        if let Ret::ValueArray = ret {
            let arg = match args.as_slice() {
                [Arg::Range(e)] => e.clone(),
                _ => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: NSArray return needs a single NSRange arg\n"));
                    return;
                }
            };
            self.needs_vec = true;
            let sep = if receiver.is_empty() { "" } else { ", " };
            self.body.push_str(&format!(
                "    fn {name}({receiver}{sep}{sig_param}) -> vec::Vec[rt::Range] {{\n\
                 \x20       let arr: *u8 = rt::msg_id_range({recv}, rt::sel(#str_ptr(\"{sel}\\0\")), {arg});\n\
                 \x20       let n: u64 = rt::msg_u64(arr, rt::sel(#str_ptr(\"count\\0\")));\n\
                 \x20       var out: vec::Vec[rt::Range] = vec::Vec[rt::Range]::with_capacity(n as usize);\n\
                 \x20       let at_sel: *u8 = rt::sel(#str_ptr(\"objectAtIndex:\\0\"));\n\
                 \x20       let range_sel: *u8 = rt::sel(#str_ptr(\"rangeValue\\0\"));\n\
                 \x20       var i: u64 = 0 as u64;\n\
                 \x20       while i < n {{\n\
                 \x20           let value: *u8 = rt::msg_id_u64(arr, at_sel, i);\n\
                 \x20           out.append(rt::msg_range(value, range_sel));\n\
                 \x20           i = i +% (1 as u64);\n\
                 \x20       }}\n\
                 \x20       return out;\n    }}\n\n"
            ));
            return;
        }

        let send = match self.send_expr(&recv, &sel, &ret, &args) {
            Some(s) => s,
            None => {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: (return, arg) shape not yet modelled\n"));
                return;
            }
        };

        let sep = if receiver.is_empty() || sig_param.is_empty() { "" } else { ", " };
        let (ret_spelling, body_line) = match &ret {
            Ret::Void => (String::new(), format!("        {send};\n")),
            Ret::Bool => (" -> bool".into(), format!("        return {send} != (0 as i8);\n")),
            Ret::Object => (" -> *u8".into(), format!("        return {send};\n")),
            Ret::ObjectOption => (
                " -> option::Option[*u8]".into(),
                format!("        let obj: *u8 = {send};\n        return bridge::obj_option(obj);\n"),
            ),
            Ret::ScalarI64 => (" -> i64".into(), format!("        return {send};\n")),
            Ret::ScalarU64 => (" -> u64".into(), format!("        return {send};\n")),
            Ret::Range => (" -> rt::Range".into(), format!("        return {send};\n")),
            Ret::Text { nullable } => {
                if *nullable {
                    (" -> option::Option[text::Text]".into(), format!("        let ns: *u8 = {send};\n        return bridge::to_text_option(ns);\n"))
                } else {
                    (" -> text::Text".into(), format!("        let ns: *u8 = {send};\n        return bridge::to_text(ns);\n"))
                }
            }
            Ret::EnumTy(objc_enum) => {
                let info = self.enums.get(objc_enum).unwrap();
                (format!(" -> {}", info.cplus_name), format!("        return {}({send});\n", info.from_raw_fn))
            }
            Ret::ValueArray | Ret::Unsupported(_) => unreachable!(),
        };

        self.body.push_str(&format!("    fn {name}({receiver}{sep}{sig_param}){ret_spelling} {{\n{body_line}    }}\n\n"));
    }

    /// The `rt::msg_*` call expression for a (receiver, selector, return, args)
    /// combination. The runtime wrappers are named `msg_<ret>_<arg>…` by their
    /// ABI signature, so the name is derived mechanically and works for any
    /// arity (the vendor/objc shim must exist; it's added per signature). For
    /// enum returns the raw integer call is produced (caller wraps in from_raw).
    fn send_expr(&self, recv: &str, sel: &str, ret: &Ret, args: &[Arg]) -> Option<String> {
        let sl = format!("rt::sel(#str_ptr(\"{sel}\\0\"))");
        let ret_tag = match ret {
            Ret::Void => "void",
            Ret::Object | Ret::ObjectOption | Ret::Text { .. } => "id",
            Ret::Bool => "i8",
            Ret::ScalarI64 | Ret::EnumTy(_) => "i64",
            Ret::ScalarU64 => "u64",
            Ret::Range => "range",
            Ret::ValueArray | Ret::Unsupported(_) => return None,
        };
        let mut tags: Vec<&str> = vec![ret_tag];
        let mut exprs: Vec<String> = Vec::new();
        for a in args {
            let (t, e): (&str, String) = match a {
                Arg::Id(e) => ("id", e.clone()),
                Arg::Bool(e) => ("i8", format!("{e} as i8")),
                Arg::ScalarI64(e) => ("i64", e.clone()),
                Arg::ScalarU64(e) => ("u64", e.clone()),
                Arg::Range(e) => ("range", e.clone()),
                Arg::Unsupported(_) => return None,
            };
            tags.push(t);
            exprs.push(e);
        }
        // The runtime provides a fixed set of msgSend ABI shapes; only emit a
        // call when its shape exists, otherwise SKIP (keeps output compilable).
        // Grow this in lockstep with vendor/objc/src/runtime.cplus.
        let suffix = tags.join("_");
        const KNOWN: &[&str] = &[
            "void", "void_id", "void_i8", "id", "id_id", "id_i64", "id_u64",
            "id_id_u64", "id_range", "i8", "i64", "u64", "range", "range_u64",
            "range_range",
        ];
        if !KNOWN.contains(&suffix.as_str()) {
            return None;
        }
        let mut call = format!("rt::msg_{suffix}({recv}, {sl}");
        for e in &exprs {
            call.push_str(&format!(", {e}"));
        }
        call.push(')');
        Some(call)
    }

    fn map_ret(&mut self, qt: &str) -> Ret {
        let (base_ty, nullable) = strip_nullability(qt);
        let base_ty = base_ty.trim();
        if base_ty == "void" {
            return Ret::Void;
        }
        if base_ty == "NSRange" {
            return Ret::Range;
        }
        if self.is_value_array(base_ty) {
            return Ret::ValueArray;
        }
        if self.is_nsstring(base_ty) {
            return Ret::Text { nullable };
        }
        if let Some(objc_enum) = self.enum_of(base_ty) {
            if !self.used_enums.contains(&objc_enum) {
                self.used_enums.push(objc_enum.clone());
            }
            return Ret::EnumTy(objc_enum);
        }
        match base_ty {
            "NSInteger" | "long" => return Ret::ScalarI64,
            "NSUInteger" | "unsigned long" => return Ret::ScalarU64,
            "BOOL" | "_Bool" | "bool" => return Ret::Bool,
            "instancetype" => return Ret::Object, // a fresh +1, never nil
            "id" => {
                return if nullable { Ret::ObjectOption } else { Ret::Object };
            }
            // 32-bit / float scalars need their own msgSend widths; defer.
            "int" | "unsigned int" | "unsigned" | "double" | "float" => {
                return Ret::Unsupported(format!("scalar `{base_ty}` not yet modelled"))
            }
            _ => {}
        }
        if base_ty.contains('<') {
            return Ret::Unsupported("generic collection".into());
        }
        if base_ty.contains('^') {
            return Ret::Unsupported("block".into());
        }
        if base_ty.ends_with('*') {
            return if nullable { Ret::ObjectOption } else { Ret::Object };
        }
        Ret::Unsupported(format!("unmapped type `{base_ty}`"))
    }

    fn map_arg(&mut self, qt: &str, pname: &str) -> Arg {
        let (base_ty, _) = strip_nullability(qt);
        let base_ty = base_ty.trim();
        if base_ty == "NSRange" {
            return Arg::Range(pname.to_string());
        }
        if self.is_nsstring(base_ty) {
            return Arg::Id(format!("bridge::nsstring({pname})"));
        }
        if let Some(objc_enum) = self.enum_of(base_ty) {
            if !self.used_enums.contains(&objc_enum) {
                self.used_enums.push(objc_enum.clone());
            }
            let raw = self.enums.get(&objc_enum).unwrap().raw_fn.clone();
            return Arg::ScalarI64(format!("{raw}({pname})"));
        }
        match base_ty {
            "NSInteger" | "long" => return Arg::ScalarI64(pname.to_string()),
            "NSUInteger" | "unsigned long" => return Arg::ScalarU64(pname.to_string()),
            "BOOL" | "_Bool" | "bool" => return Arg::Bool(pname.to_string()),
            "id" => return Arg::Id(pname.to_string()),
            "int" | "unsigned int" | "unsigned" | "double" | "float" => {
                return Arg::Unsupported(format!("scalar `{base_ty}` not yet modelled"))
            }
            _ => {}
        }
        if base_ty.contains('^') {
            return Arg::Unsupported("block".into());
        }
        if base_ty.contains('<') {
            return Arg::Unsupported("generic collection".into());
        }
        if base_ty.ends_with('*') {
            return Arg::Id(pname.to_string());
        }
        Arg::Unsupported(format!("unmapped type `{base_ty}`"))
    }

    /// The public C+ parameter type for an ObjC param spelling (str / Range /
    /// the enum type / a scalar / a raw object handle).
    fn param_sig_type(&self, qt: &str) -> String {
        let (b, _) = strip_nullability(qt);
        let b = b.trim();
        if b == "NSRange" {
            return "rt::Range".to_string();
        }
        if self.is_nsstring(b) {
            return "str".to_string();
        }
        if let Some(objc_enum) = self.enum_of(b) {
            return self.enums.get(&objc_enum).unwrap().cplus_name.clone();
        }
        match b {
            "NSInteger" | "long" => "i64".to_string(),
            "NSUInteger" | "unsigned long" => "u64".to_string(),
            "BOOL" | "_Bool" | "bool" => "bool".to_string(),
            "int" => "i32".to_string(),
            "unsigned int" | "unsigned" => "u32".to_string(),
            _ => "*u8".to_string(),
        }
    }

    fn enum_of(&self, ty: &str) -> Option<String> {
        if self.enums.contains_key(ty) {
            return Some(ty.to_string());
        }
        // Follow typedefs to an enum name.
        let mut cur = ty.to_string();
        for _ in 0..8 {
            if self.enums.contains_key(&cur) {
                return Some(cur);
            }
            match self.typedefs.get(&cur) {
                Some(u) => cur = u.trim().to_string(),
                None => return None,
            }
        }
        None
    }

    fn is_value_array(&self, ty: &str) -> bool {
        let t = ty.replace(' ', "");
        t == "NSArray<NSValue*>*"
    }

    fn is_nsstring(&self, ty: &str) -> bool {
        let t = ty.trim();
        if t == "NSString *" || t == "NSString*" {
            return true;
        }
        let mut cur = t.to_string();
        for _ in 0..8 {
            match self.typedefs.get(&cur) {
                Some(under) => {
                    let u = under.trim();
                    if u == "NSString *" || u == "NSString*" {
                        return true;
                    }
                    cur = u.to_string();
                }
                None => return false,
            }
        }
        false
    }
}

fn base(p: &str) -> String {
    std::path::Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// clang's `loc` puts the file directly, or (for macro-expanded decls like
/// NS_ENUM) under `expansionLoc`/`spellingLoc`. Prefer the expansion site.
fn loc_file(loc: &serde_json::Value) -> Option<String> {
    loc.get("file")
        .and_then(|f| f.as_str())
        .or_else(|| loc.get("expansionLoc").and_then(|e| e.get("file")).and_then(|f| f.as_str()))
        .or_else(|| loc.get("spellingLoc").and_then(|e| e.get("file")).and_then(|f| f.as_str()))
        .map(|s| s.to_string())
}

/// True if the decl came in via an #include/#import (so it isn't ours).
fn loc_included(loc: &serde_json::Value) -> bool {
    loc.get("includedFrom").is_some()
        || loc.get("expansionLoc").and_then(|e| e.get("includedFrom")).is_some()
}

fn strip_nullability(qt: &str) -> (String, bool) {
    let s = qt.trim();
    for (suf, nul) in [(" _Nullable", true), (" _Nonnull", false), (" _Null_unspecified", true)] {
        if let Some(stripped) = s.strip_suffix(suf) {
            return (stripped.to_string(), nul);
        }
    }
    (s.to_string(), false)
}

/// Strip a leading enum-name prefix from a constant (NLTokenUnitWord -> Word).
fn strip_prefix(name: &str, enum_name: &str) -> String {
    name.strip_prefix(enum_name).filter(|s| !s.is_empty()).unwrap_or(name).to_string()
}

fn pascal(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// Read an explicit integer value from an EnumConstantDecl's initializer.
fn read_int(node: &serde_json::Value) -> Option<i64> {
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
    node.get("inner").and_then(|x| x.as_array()).into_iter().flatten().find_map(search)
}

/// Mechanical C+ method name from a selector. Single-colon selectors keep their
/// one segment (`tokensForRange:` -> `tokens_for_range`); multi-part selectors
/// camel-join every segment so they stay collision-free (`a:b:` -> `a_b`, never
/// clashing with `a:`). The override file supplies nicer labels on top.
fn mechanical_name(sel: &str) -> String {
    let parts: Vec<&str> = sel.split(':').filter(|s| !s.is_empty()).collect();
    let joined: String = parts
        .iter()
        .enumerate()
        .map(|(i, p)| if i == 0 { p.to_string() } else { pascal(p) })
        .collect();
    snake(&joined)
}

/// camelCase / PascalCase -> snake_case.
fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
