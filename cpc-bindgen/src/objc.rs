// cpc-bindgen ObjC front-end — Objective-C framework header -> C+ wrapper.
//
// Walks `clang -x objective-c -ast-dump=json` for ObjCInterfaceDecl / EnumDecl
// nodes in the target header and emits C+ wrappers calling through the shared
// `objc` runtime (`vendor/objc`): msgSend shims, the str/NSString bridge, the
// `Range` (NSRange) value type, and the +1/`drop` ownership model.
//
// Modelled: classes with init / factory (incl. nullable -> Option[Self]);
// instance/class methods over void / object / str / scalar (i64/u64/f64/BOOL) /
// NSRange / enum; nullable object & string returns -> Option; NS_ENUM -> C+ enum;
// NSArray<NSString*|NSValue*> <-> Vec (both directions); any-arity selectors;
// categories; ObjC blocks (usingBlock:); and delegate/data-source PROTOCOLS ->
// runtime class-synthesis helpers (void + non-void callbacks, multi-method,
// override-named). Naming is mechanical snake_case unless the override file
// renames it. Unmodelled constructs are emitted as `// SKIPPED` comments, never
// wrong code.
//
// Remaining gaps and how to close them are documented in cpc-bindgen/LIMITATIONS.md
// (the remaining gap is NSDictionary *params* — returns are done). SKIP sites
// point back to that file.

use std::collections::{HashMap, HashSet};

pub struct ObjcEmitter {
    header_path: String,
    prefix: String,
    overrides: serde_json::Value,
    body: String,
    block_helpers: String,
    needs_vec: bool,
    needs_string_map: bool,
    needs_synth: bool,
    synth_key: u64,
    typedefs: HashMap<String, String>,
    enums: HashMap<String, EnumInfo>,
    used_enums: Vec<String>,
}

// A delegate-callback return kind: how the value-returning IMP is shaped.
struct DelegateRet {
    tag: String,            // add_method suffix: v / id / i64 / u64 / i16 / i8
    ret_suffix: String,     // C+ return spelling: "" or " -> i64" / " -> *u8" ...
    enc: String,            // ObjC type-encoding char for the return
    default_ret: Option<String>, // noop's default return expr (None == void)
}

// (return tag, object-arg count) pairs vendor/objc/synthesis provides a
// class_addMethod shape for. Keep in lockstep with synthesis.cplus.
fn delegate_shape_known(tag: &str, n: usize) -> bool {
    const SHAPES: &[(&str, usize)] = &[
        ("v", 0), ("v", 1), ("v", 2), ("v", 3), ("v", 4), ("v", 5),
        ("id", 0), ("id", 1), ("id", 2), ("id", 3), ("id", 4),
        ("i64", 0), ("i64", 1), ("i64", 2),
        ("u64", 0), ("u64", 1),
        ("i16", 0),
        ("i8", 1), ("i8", 2), ("i8", 3),
    ];
    SHAPES.contains(&(tag, n))
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
    ScalarF64, // double / CGFloat / NSTimeInterval
    ScalarI32, // int
    ScalarU32, // unsigned int
    ScalarF32, // float
    EnumTy(String),
    Range,
    ValueArray, // NSArray<NSValue *> -> Vec[Range]
    TextArray,  // NSArray<NSString *> -> Vec[Text]
    TextMap(MapVal), // NSDictionary<NSString *, V> -> StringMap[V]
    Unsupported(String),
}

// The value side of a string-keyed `NSDictionary` the bindgen can bridge.
// (Keys are always `NSString` -> `Text`.) Numbers come out as `f64` via
// `-doubleValue`; nested strings come out as owned `Text`.
#[derive(Clone, Copy)]
enum MapVal {
    Text,      // NSString * value -> Text
    ScalarF64, // NSNumber * value -> f64 (via doubleValue)
}

enum Arg {
    Id(String),       // object / string already lowered to an id expression
    Bool(String),     // BOOL param (the C+ bool name; lowered to i8 on the wire)
    ScalarI64(String),
    ScalarU64(String),
    ScalarF64(String), // double / CGFloat / NSTimeInterval
    ScalarI32(String), // int
    ScalarU32(String), // unsigned int
    ScalarF32(String), // float
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
            block_helpers: String::new(),
            needs_vec: false,
            needs_string_map: false,
            needs_synth: false,
            synth_key: 0x7000,
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
        // Pass 2: collect header-local interfaces + the categories that extend
        // them (Foundation puts much of NSScanner/NSString/... in categories),
        // then emit each class with its interface + category methods merged.
        let target = base(&self.header_path);
        let mut current_file: Option<String> = None;
        let mut interfaces: Vec<serde_json::Value> = Vec::new();
        let mut categories: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
        let mut protocols: Vec<serde_json::Value> = Vec::new();
        for decl in inner {
            if let Some(loc) = decl.get("loc") {
                if let Some(f) = loc_file(loc) {
                    current_file = Some(f);
                }
            }
            let kind = decl.get("kind").and_then(|v| v.as_str());
            if kind != Some("ObjCInterfaceDecl")
                && kind != Some("ObjCCategoryDecl")
                && kind != Some("ObjCProtocolDecl")
            {
                continue;
            }
            if decl.get("loc").map(loc_included).unwrap_or(false) {
                continue;
            }
            if current_file.as_deref().map(base) != Some(target.clone()) {
                continue;
            }
            if kind == Some("ObjCInterfaceDecl") {
                interfaces.push(decl.clone());
            } else if kind == Some("ObjCProtocolDecl") {
                protocols.push(decl.clone());
            } else if let Some(cls) = decl
                .get("interface")
                .and_then(|i| i.get("name"))
                .and_then(|n| n.as_str())
            {
                categories.entry(cls.to_string()).or_default().push(decl.clone());
            }
        }
        // clang emits a forward `@interface` (no methods) alongside the real
        // definition under one name; keep only the richest per name so the type
        // isn't emitted twice. Order preserved (first appearance).
        let mut deduped: Vec<serde_json::Value> = Vec::new();
        for itf in &interfaces {
            let name = itf.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let count = itf.get("inner").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            match deduped.iter().position(|e| e.get("name").and_then(|n| n.as_str()).unwrap_or("") == name) {
                Some(pos) => {
                    let ec = deduped[pos].get("inner").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    if count > ec {
                        deduped[pos] = itf.clone();
                    }
                }
                None => deduped.push(itf.clone()),
            }
        }
        for itf in &deduped {
            let cls = itf.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let cats: &[serde_json::Value] =
                categories.get(cls).map(|v| v.as_slice()).unwrap_or(&[]);
            self.emit_interface(itf, cats);
        }
        // Delegate / data-source protocols -> runtime class-synthesis helpers.
        for proto in &protocols {
            self.emit_protocol_delegate(proto);
        }

        // Assemble: preamble + the enum defs actually used + interface bodies.
        let mut out = self.preamble();
        for objc_name in self.used_enums.clone() {
            if let Some(info) = self.enums.get(&objc_name).cloned() {
                out.push_str(&self.render_enum(&info));
            }
        }
        out.push_str(&self.block_helpers);
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
        if self.needs_string_map {
            p.push_str("import \"stdlib/string_map\" as string_map;\n");
        }
        if self.needs_synth {
            p.push_str("import \"objc/synthesis\" as synth;\n");
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

    fn emit_interface(&mut self, itf: &serde_json::Value, categories: &[serde_json::Value]) {
        let objc_name = match itf.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        let ty = self.cplus_type_name(&objc_name);
        // Methods from the @interface plus every category that extends it,
        // deduped by selector (a property may be redeclared across them).
        let mut methods: Vec<serde_json::Value> = Vec::new();
        let mut seen_sel: HashSet<String> = HashSet::new();
        for src in std::iter::once(itf).chain(categories.iter()) {
            for m in src.get("inner").and_then(|v| v.as_array()).into_iter().flatten() {
                if m.get("kind").and_then(|k| k.as_str()) != Some("ObjCMethodDecl") {
                    continue;
                }
                let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if seen_sel.insert(sel) {
                    methods.push(m.clone());
                }
            }
        }
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
            self.emit_method(m, &objc_name, &ty, is_init, owned);
        }

        if owned {
            self.body.push_str("    fn drop(ref this) {\n        rt::release(this._obj);\n    }\n");
        }
        self.body.push_str("}\n\n");
    }

    // A delegate / data-source protocol -> ONE runtime class-synthesis helper for
    // the whole protocol. The user fills a caller-owned `<Proto>` value (one
    // handler fn per callback + their state pointer) and `create_<Proto>(ctx)`
    // synthesizes a class whose IMPs are C+ trampolines that read the ctx back
    // off the instance (an associated object) and dispatch to the right handler.
    // Each handler defaults to a generated noop, so the user supplies (via named
    // params) only the callbacks they care about. Scoped to void-returning
    // callbacks whose every argument is an object (the v_Nid IMP shapes); other
    // shapes are listed as SKIPPED so coverage is explicit.
    fn emit_protocol_delegate(&mut self, proto: &serde_json::Value) {
        let proto_objc = match proto.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        let proto_ty = self.cplus_type_name(&proto_objc);
        let methods: Vec<serde_json::Value> = proto
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|m| {
                        m.get("kind").and_then(|k| k.as_str()) == Some("ObjCMethodDecl")
                            && m.get("instance").and_then(|b| b.as_bool()).unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if methods.is_empty() {
            return;
        }

        // Partition into modellable callbacks (sel, snake id, arg count, return
        // kind) and SKIPPED notes for the rest.
        let mut callbacks: Vec<(String, String, usize, DelegateRet)> = Vec::new();
        let mut skips: Vec<String> = Vec::new();
        for m in &methods {
            let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ret_qt = m
                .get("returnType")
                .and_then(|t| t.get("qualType"))
                .and_then(|v| v.as_str())
                .unwrap_or("void");
            let params: Vec<String> = m
                .get("inner")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("ParmVarDecl"))
                        .map(|p| p.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()).unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default();
            if self.is_skipped(&proto_objc, &sel) {
                skips.push(format!("// SKIPPED `{sel}`: override\n"));
                continue;
            }
            if params.len() > 5 || !params.iter().all(|qt| self.is_object_arg(qt)) {
                skips.push(format!("// SKIPPED `{sel}`: arg shape (only <=5 object args modelled)\n"));
                continue;
            }
            let ret = match self.delegate_ret(ret_qt) {
                Some(r) => r,
                None => {
                    skips.push(format!("// SKIPPED `{sel}`: return `{ret_qt}` not modelled\n"));
                    continue;
                }
            };
            if !delegate_shape_known(&ret.tag, params.len()) {
                skips.push(format!("// SKIPPED `{sel}`: no `{}`-return / {}-arg IMP shape\n", ret.tag, params.len()));
                continue;
            }
            // Override file supplies a short callback name; else the full
            // selector, snake-cased (long but unambiguous).
            let mid = self
                .method_override(&proto_objc, &sel)
                .and_then(|o| o.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| selector_ident(&sel));
            callbacks.push((sel.clone(), mid, params.len(), ret));
        }
        if callbacks.is_empty() {
            return;
        }
        self.needs_synth = true;
        let key = self.synth_key;
        self.synth_key += 1;

        // Handler type: fn(user, <n object args>) -> <ret> (ret_suffix is the
        // " -> X" or empty for void).
        let handler_ty = |n: usize, ret: &DelegateRet| format!("fn(*u8{}){}", ", *u8".repeat(n), ret.ret_suffix);
        let mut s = String::new();
        s.push_str(&format!("// `{proto_objc}` delegate protocol -> runtime class-synthesis helper.\n"));
        for sk in &skips {
            s.push_str(sk);
        }

        // A noop per distinct (return tag, arity), the default for each unset
        // handler. Non-void noops return a zero/nil default.
        let mut shapes: Vec<(String, usize)> = callbacks.iter().map(|(_, _, n, r)| (r.tag.clone(), *n)).collect();
        shapes.sort();
        shapes.dedup();
        for (tag, n) in &shapes {
            let ret = self.tag_ret(tag);
            let mut pp = vec!["user: *u8".to_string()];
            for i in 0..*n {
                pp.push(format!("a{i}: *u8"));
            }
            let body = match &ret.default_ret {
                Some(d) => format!("return {d};"),
                None => "return;".to_string(),
            };
            s.push_str(&format!("fn {proto_ty}_noop_{tag}_{n}({}){} {{ {body} }}\n", pp.join(", "), ret.ret_suffix));
        }
        s.push('\n');

        // The handler table the caller fills (one fn per callback + their state).
        s.push_str(&format!("struct {proto_ty} {{\n"));
        for (_, mid, n, r) in &callbacks {
            s.push_str(&format!("    {mid}: {},\n", handler_ty(*n, r)));
        }
        s.push_str("    opaque user: *u8,\n}\n\n");

        // Constructor: every handler starts as its noop (intra-module names, so
        // this resolves regardless of where `_new` is called); setters install
        // the callbacks the caller actually wants.
        s.push_str(&format!("fn {proto_ty}_new(user: *u8) -> {proto_ty} {{\n    return {proto_ty} {{ "));
        let mut inits: Vec<String> = callbacks.iter().map(|(_, mid, n, r)| format!("{mid}: {proto_ty}_noop_{}_{n}", r.tag)).collect();
        inits.push("user: user".to_string());
        s.push_str(&inits.join(", "));
        s.push_str(" };\n}\n\n");

        s.push_str(&format!("impl {proto_ty} {{\n"));
        for (_, mid, n, r) in &callbacks {
            s.push_str(&format!(
                "    fn set_{mid}(ref this, handler: {}) {{\n        this.{mid} = handler;\n        return;\n    }}\n",
                handler_ty(*n, r)
            ));
        }
        s.push_str("}\n\n");

        s.push_str(&format!("fn {proto_ty}_key() -> *u8 {{ return {{ {key} as *u8 }}; }}\n\n"));

        // One IMP trampoline per callback; all share the one associated ctx.
        for (_, mid, n, r) in &callbacks {
            let mut imp_params = vec!["self_obj: *u8".to_string(), "cmd: *u8".to_string()];
            let mut call_args = vec!["u".to_string()];
            for i in 0..*n {
                imp_params.push(format!("a{i}: *u8"));
                call_args.push(format!("a{i}"));
            }
            s.push_str(&format!("fn {proto_ty}_{mid}_imp({}){} {{\n", imp_params.join(", "), r.ret_suffix));
            s.push_str(&format!("    let c: *{proto_ty} = {{ synth::get_associated(self_obj, {proto_ty}_key()) as *{proto_ty} }};\n"));
            s.push_str(&format!("    let f: {} = {{ (*c).{mid} }};\n", handler_ty(*n, r)));
            s.push_str("    let u: *u8 = { (*c).user };\n");
            let dispatch = if r.default_ret.is_some() {
                format!("    return f({});\n", call_args.join(", "))
            } else {
                format!("    f({});\n    return;\n", call_args.join(", "))
            };
            s.push_str(&dispatch);
            s.push_str("}\n\n");
        }

        // create: build + register the class once (installing every callback),
        // instantiate, and attach the caller's handler table.
        s.push_str(&format!("fn create_{proto_ty}(ctx: *{proto_ty}) -> *u8 {{\n"));
        s.push_str(&format!("    let name: *u8 = #str_ptr(\"Cplus_{proto_ty}\\0\");\n"));
        s.push_str("    var cls: *u8 = rt::get_class(name);\n");
        s.push_str("    if cls == { 0 as *u8 } {\n");
        s.push_str("        cls = synth::allocate_class_pair(rt::get_class(#str_ptr(\"NSObject\\0\")), name, 0 as usize);\n");
        for (sel, mid, n, r) in &callbacks {
            let types = format!("{}@:{}", r.enc, "@".repeat(*n));
            s.push_str(&format!(
                "        let add_{mid}: i8 = synth::add_method_{}_{n}id(cls, rt::sel(#str_ptr(\"{sel}\\0\")), {proto_ty}_{mid}_imp, #str_ptr(\"{types}\\0\"));\n",
                r.tag
            ));
        }
        s.push_str("        synth::register_class_pair(cls);\n    }\n");
        s.push_str("    let d: *u8 = synth::alloc_init_class(cls);\n");
        s.push_str(&format!("    synth::set_associated(d, {proto_ty}_key(), {{ ctx as *u8 }});\n"));
        s.push_str("    return d;\n}\n\n");
        self.body.push_str(&s);
    }

    // Classify a delegate-callback return type into the IMP shape that carries
    // it: the `-> T` suffix, the add_method tag, the ObjC type-encoding char, and
    // the noop default (None == void). Enums ride their NSUInteger base.
    fn delegate_ret(&self, qt: &str) -> Option<DelegateRet> {
        let (b, _) = strip_nullability(qt);
        let b = b.trim();
        if b == "void" {
            return Some(self.tag_ret("v"));
        }
        if self.enum_of(b).is_some() {
            return Some(self.tag_ret("u64"));
        }
        let tag = match b {
            "NSInteger" | "long" => "i64",
            "NSUInteger" | "unsigned long" => "u64",
            "short" => "i16",
            "BOOL" | "_Bool" | "bool" => "i8",
            "instancetype" | "id" => "id",
            other if other.ends_with('*') => "id",
            _ => return None,
        };
        Some(self.tag_ret(tag))
    }

    fn tag_ret(&self, tag: &str) -> DelegateRet {
        let (suffix, enc, default): (&str, &str, Option<&str>) = match tag {
            "v" => ("", "v", None),
            "id" => (" -> *u8", "@", Some("{ 0 as *u8 }")),
            "i64" => (" -> i64", "q", Some("0 as i64")),
            "u64" => (" -> u64", "Q", Some("0 as u64")),
            "i16" => (" -> i16", "s", Some("0 as i16")),
            "i8" => (" -> i8", "c", Some("0 as i8")),
            _ => ("", "v", None),
        };
        DelegateRet {
            tag: tag.to_string(),
            ret_suffix: suffix.to_string(),
            enc: enc.to_string(),
            default_ret: default.map(String::from),
        }
    }

    fn is_object_arg(&self, qt: &str) -> bool {
        let (b, _) = strip_nullability(qt);
        let b = b.trim();
        if b.contains('^') {
            return false;
        }
        b == "id" || b.ends_with('*')
    }

    fn emit_method(&mut self, m: &serde_json::Value, objc_class: &str, ty: &str, is_init: bool, owned: bool) {
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

        // A `usingBlock:` parameter -> dedicated block-literal emission.
        if let Some(bidx) = params.iter().position(|(_, qt)| qt.contains("(^")) {
            self.emit_block_method(objc_class, &ty, &sel, is_instance, ret_qt, &params, bidx, &ov_name, &ov_params);
            return;
        }

        // Lower every argument: `sig_param` is the public C+ parameter list
        // (labels = override or AST param names, types = enum/str/Range/scalar),
        // `args` are the wire expressions (raw int / bridged NSString / ...).
        let mut sig_parts: Vec<String> = Vec::new();
        let mut args: Vec<Arg> = Vec::new();
        for (idx, (pname, pqt)) in params.iter().enumerate() {
            let pn = escape_keyword(ov_params.get(idx).cloned().unwrap_or_else(|| snake(pname)));
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
        let name = escape_keyword(ov_name.clone().unwrap_or_else(|| mechanical_name(&sel)));

        // A class factory returning the class's own type (`instancetype` or
        // `Class *`) -> a wrapped `Self` (or `Option[Self]` if nullable).
        // Factories hand back a +0 autoreleased object, so for an owned wrapper
        // we `retain` it to balance `drop`.
        let (ret_base, ret_nullable) = strip_nullability(ret_qt);
        let returns_self = !is_instance
            && matches!(ret, Ret::Object | Ret::ObjectOption)
            && (ret_base.trim() == "instancetype" || ret_base.trim() == format!("{objc_class} *"));
        if returns_self {
            if let Some(send) = self.send_expr(&recv, &sel, &Ret::Object, &args) {
                if ret_nullable {
                    let wrapped = if owned {
                        format!("{ty} {{ _obj: rt::retain(obj) }}")
                    } else {
                        format!("{ty} {{ _obj: obj }}")
                    };
                    self.body.push_str(&format!(
                        "    fn {name}({sig_param}) -> option::Option[{ty}] {{\n        let obj: *u8 = {send};\n        if obj == {{ 0 as *u8 }} {{\n            return option::Option[{ty}]::None;\n        }}\n        return option::some({wrapped});\n    }}\n\n"
                    ));
                } else {
                    let handle = if owned { format!("rt::retain({send})") } else { send };
                    self.body.push_str(&format!(
                        "    fn {name}({sig_param}) -> {ty} {{\n        return {ty} {{ _obj: {handle} }};\n    }}\n\n"
                    ));
                }
                return;
            }
        }

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

        if let Ret::TextArray = ret {
            let array_call = match self.send_expr(&recv, &sel, &Ret::Object, &args) {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: NSArray<NSString> arg shape not modelled\n"));
                    return;
                }
            };
            self.needs_vec = true;
            let sep = if receiver.is_empty() || sig_param.is_empty() { "" } else { ", " };
            self.body.push_str(&format!(
                "    fn {name}({receiver}{sep}{sig_param}) -> vec::Vec[text::Text] {{\n\
                 \x20       let arr: *u8 = {array_call};\n\
                 \x20       let n: u64 = rt::msg_u64(arr, rt::sel(#str_ptr(\"count\\0\")));\n\
                 \x20       var out: vec::Vec[text::Text] = vec::Vec[text::Text]::with_capacity(n as usize);\n\
                 \x20       let at_sel: *u8 = rt::sel(#str_ptr(\"objectAtIndex:\\0\"));\n\
                 \x20       var i: u64 = 0 as u64;\n\
                 \x20       while i < n {{\n\
                 \x20           let value: *u8 = rt::msg_id_u64(arr, at_sel, i);\n\
                 \x20           out.append(bridge::to_text(value));\n\
                 \x20           i = i +% (1 as u64);\n\
                 \x20       }}\n\
                 \x20       return out;\n    }}\n\n"
            ));
            return;
        }

        if let Ret::TextMap(val) = ret {
            let dict_call = match self.send_expr(&recv, &sel, &Ret::Object, &args) {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: NSDictionary arg shape not modelled\n"));
                    return;
                }
            };
            self.needs_string_map = true;
            // Key is always NSString -> Text; the value bridge depends on V.
            let (val_ty, val_bridge): (&str, String) = match val {
                MapVal::Text => ("text::Text", "bridge::to_text(nsval)".to_string()),
                MapVal::ScalarF64 => (
                    "f64",
                    "rt::msg_f64(nsval, rt::sel(#str_ptr(\"doubleValue\\0\")))".to_string(),
                ),
            };
            let sep = if receiver.is_empty() || sig_param.is_empty() { "" } else { ", " };
            self.body.push_str(&format!(
                "    fn {name}({receiver}{sep}{sig_param}) -> string_map::StringMap[{val_ty}] {{\n\
                 \x20       let dict: *u8 = {dict_call};\n\
                 \x20       let keys: *u8 = rt::msg_id(dict, rt::sel(#str_ptr(\"allKeys\\0\")));\n\
                 \x20       let n: u64 = rt::msg_u64(keys, rt::sel(#str_ptr(\"count\\0\")));\n\
                 \x20       var out: string_map::StringMap[{val_ty}] = string_map::new::[{val_ty}]();\n\
                 \x20       let at_sel: *u8 = rt::sel(#str_ptr(\"objectAtIndex:\\0\"));\n\
                 \x20       let obj_sel: *u8 = rt::sel(#str_ptr(\"objectForKey:\\0\"));\n\
                 \x20       var i: u64 = 0 as u64;\n\
                 \x20       while i < n {{\n\
                 \x20           let nskey: *u8 = rt::msg_id_u64(keys, at_sel, i);\n\
                 \x20           let nsval: *u8 = rt::msg_id_id(dict, obj_sel, nskey);\n\
                 \x20           out.insert(bridge::to_text(nskey), {val_bridge});\n\
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
            Ret::ScalarF64 => (" -> f64".into(), format!("        return {send};\n")),
            Ret::ScalarI32 => (" -> i32".into(), format!("        return {send};\n")),
            Ret::ScalarU32 => (" -> u32".into(), format!("        return {send};\n")),
            Ret::ScalarF32 => (" -> f32".into(), format!("        return {send};\n")),
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
            Ret::ValueArray | Ret::TextArray | Ret::TextMap(_) | Ret::Unsupported(_) => {
                unreachable!()
            }
        };

        self.body.push_str(&format!("    fn {name}({receiver}{sep}{sig_param}){ret_spelling} {{\n{body_line}    }}\n\n"));
    }

    /// A method with a trailing `usingBlock:` param: emit a per-method
    /// Block_literal struct + `invoke` trampoline (into `block_helpers`) and a
    /// wrapper taking a C+ `fn(*u8, ...block args)` + `*u8` ctx.
    fn emit_block_method(
        &mut self,
        objc_class: &str,
        ty: &str,
        sel: &str,
        is_instance: bool,
        ret_qt: &str,
        params: &[(String, String)],
        bidx: usize,
        ov_name: &Option<String>,
        ov_params: &[String],
    ) {
        let (mret, _) = strip_nullability(ret_qt);
        if mret.trim() != "void" {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: block method with non-void return\n"));
            return;
        }
        if bidx != params.len() - 1 {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: params after the block not modelled\n"));
            return;
        }
        let block_args = match self.parse_block_args(&params[bidx].1) {
            Some(a) => a,
            None => {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: unparseable block signature\n"));
                return;
            }
        };
        let block_sig = block_args.join(", ");
        let fn_ty = if block_sig.is_empty() {
            "fn(*u8)".to_string()
        } else {
            format!("fn(*u8, {block_sig})")
        };

        // Leading (non-block) params.
        let mut sig_parts: Vec<String> = Vec::new();
        let mut send_args: Vec<Arg> = Vec::new();
        for (idx, (pname, pqt)) in params[..bidx].iter().enumerate() {
            let pn = escape_keyword(ov_params.get(idx).cloned().unwrap_or_else(|| snake(pname)));
            let a = self.map_arg(pqt, &pn);
            if let Arg::Unsupported(why) = &a {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: leading param `{pqt}` — {why}\n"));
                return;
            }
            sig_parts.push(format!("{pn}: {}", self.param_sig_type(pqt)));
            send_args.push(a);
        }
        send_args.push(Arg::Id("bp".to_string()));

        let recv = if is_instance {
            "this._obj".to_string()
        } else {
            format!("rt::get_class(#str_ptr(\"{objc_class}\\0\"))")
        };
        let send = match self.send_expr(&recv, sel, &Ret::Void, &send_args) {
            Some(s) => s,
            None => {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: block-method msgSend shape not modelled\n"));
                return;
            }
        };

        let name = ov_name.clone().unwrap_or_else(|| mechanical_name(sel));
        let struct_name = format!("{ty}_{name}_block");
        let invoke_name = format!("{ty}_{name}_invoke");

        let named: Vec<String> = block_args.iter().enumerate().map(|(i, t)| format!("a{i}: {t}")).collect();
        let invoke_params = if named.is_empty() {
            "block: *u8".to_string()
        } else {
            format!("block: *u8, {}", named.join(", "))
        };
        let arg_names: Vec<String> = (0..block_args.len()).map(|i| format!("a{i}")).collect();
        let call_tail = if arg_names.is_empty() { String::new() } else { format!(", {}", arg_names.join(", ")) };

        // Top-level struct + trampoline.
        self.block_helpers.push_str(&format!(
            "#[repr(C)]\nstruct {struct_name} {{\n    opaque isa: *u8,\n    flags: i32,\n    reserved: i32,\n    invoke: {fn_ty},\n    opaque descriptor: *u8,\n    user_fn: {fn_ty},\n    opaque ctx: *u8,\n}}\n\n"
        ));
        self.block_helpers.push_str(&format!(
            "fn {invoke_name}({invoke_params}) {{\n    let bl: *{struct_name} = {{ block as *{struct_name} }};\n    let f: {fn_ty} = {{ (*bl).user_fn }};\n    let ctx: *u8 = {{ (*bl).ctx }};\n    f(ctx{call_tail});\n    return;\n}}\n\n"
        ));

        // Wrapper method.
        let receiver = if is_instance { "this" } else { "" };
        let mut sig = String::new();
        if !receiver.is_empty() {
            sig.push_str(receiver);
        }
        for part in &sig_parts {
            if !sig.is_empty() {
                sig.push_str(", ");
            }
            sig.push_str(part);
        }
        if !sig.is_empty() {
            sig.push_str(", ");
        }
        sig.push_str(&format!("cb: {fn_ty}, ctx: *u8"));
        self.body.push_str(&format!(
            "    fn {name}({sig}) {{\n        var desc: rt::BlockDescriptor = rt::BlockDescriptor {{ reserved: 0 as u64, size: 48 as u64 }};\n        var blk: {struct_name} = {struct_name} {{ isa: rt::stack_block_isa(), flags: 0 as i32, reserved: 0 as i32, invoke: {invoke_name}, descriptor: {{ #addr_of(desc) as *u8 }}, user_fn: cb, ctx: ctx }};\n        let bp: *u8 = {{ #addr_of(blk) as *u8 }};\n        {send};\n        return;\n    }}\n\n"
        ));
    }

    /// Parse a block parameter's C signature `RET (^...)(A0, A1, ...)` into the
    /// C+ wire types of its arguments. `None` if the shape doesn't parse.
    fn parse_block_args(&self, qt: &str) -> Option<Vec<String>> {
        let bytes = qt.as_bytes();
        let open1 = qt.find('(')?;
        let mut depth = 0i32;
        let mut i = open1;
        let mut close1 = None;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close1 = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let close1 = close1?;
        let open2 = qt[close1 + 1..].find('(')? + close1 + 1;
        let close2 = qt.rfind(')')?;
        if close2 <= open2 {
            return None;
        }
        let inside = &qt[open2 + 1..close2];
        let mut out: Vec<String> = Vec::new();
        let mut d = 0i32;
        let mut cur = String::new();
        for c in inside.chars() {
            match c {
                '<' | '(' => {
                    d += 1;
                    cur.push(c);
                }
                '>' | ')' => {
                    d -= 1;
                    cur.push(c);
                }
                ',' if d == 0 => {
                    let t = cur.trim().to_string();
                    if !t.is_empty() {
                        out.push(t);
                    }
                    cur.clear();
                }
                _ => cur.push(c),
            }
        }
        let last = cur.trim().to_string();
        if !last.is_empty() {
            out.push(last);
        }
        if out.len() == 1 && out[0] == "void" {
            out.clear();
        }
        Some(out.iter().map(|a| self.map_block_arg(a)).collect())
    }

    /// C+ wire type for one block-callback argument (what ObjC passes in).
    fn map_block_arg(&self, qt: &str) -> String {
        let (b, _) = strip_nullability(qt);
        let b = b.trim();
        if b == "NSRange" {
            return "rt::Range".to_string();
        }
        if b.ends_with('*') {
            return "*u8".to_string();
        }
        match b {
            "NSUInteger" | "unsigned long" => "u64".to_string(),
            "BOOL" => "i8".to_string(),
            // NSInteger, NS_ENUM/NS_OPTIONS, and other 8-byte scalars.
            _ => "i64".to_string(),
        }
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
            Ret::ScalarF64 => "f64",
            Ret::ScalarI32 => "i32",
            Ret::ScalarU32 => "u32",
            Ret::ScalarF32 => "f32",
            Ret::Range => "range",
            Ret::ValueArray | Ret::TextArray | Ret::TextMap(_) | Ret::Unsupported(_) => {
                return None
            }
        };
        let mut tags: Vec<&str> = vec![ret_tag];
        let mut exprs: Vec<String> = Vec::new();
        for a in args {
            let (t, e): (&str, String) = match a {
                Arg::Id(e) => ("id", e.clone()),
                Arg::Bool(e) => ("i8", format!("{e} as i8")),
                Arg::ScalarI64(e) => ("i64", e.clone()),
                Arg::ScalarU64(e) => ("u64", e.clone()),
                Arg::ScalarF64(e) => ("f64", e.clone()),
                Arg::ScalarI32(e) => ("i32", e.clone()),
                Arg::ScalarU32(e) => ("u32", e.clone()),
                Arg::ScalarF32(e) => ("f32", e.clone()),
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
            "void", "void_id", "void_i8", "void_f64", "void_range_id", "id",
            "id_id", "id_i64", "id_u64", "id_f64", "id_id_u64", "id_range",
            "i8", "i64", "u64", "f64", "range", "range_u64", "range_range",
            // 32-bit scalars (int / unsigned / float)
            "i32", "u32", "f32", "void_i32", "void_u32", "void_f32",
            "id_i32", "id_u32", "id_f32",
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
        if self.is_string_array(base_ty) {
            return Ret::TextArray;
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
            // CGFloat / NSTimeInterval are `double` on 64-bit Apple.
            "double" | "CGFloat" | "NSTimeInterval" => return Ret::ScalarF64,
            // 32-bit scalars ride their own msgSend widths (vendor/objc shims).
            "int" => return Ret::ScalarI32,
            "unsigned int" | "unsigned" => return Ret::ScalarU32,
            "float" => return Ret::ScalarF32,
            _ => {}
        }
        // String-keyed `NSDictionary` -> `StringMap[V]` (`stdlib/string_map`).
        // Only string keys are supported (the only `Text`-keyed map we have);
        // values bridge as `Text` (NSString) or `f64` (NSNumber).
        if let Some((k, v)) = self.parse_dict(base_ty) {
            if !self.is_nsstring(&k) {
                return Ret::Unsupported(format!("NSDictionary with non-string key `{k}`"));
            }
            if self.is_nsstring(&v) {
                self.needs_string_map = true;
                return Ret::TextMap(MapVal::Text);
            }
            if self.is_nsnumber(&v) {
                self.needs_string_map = true;
                return Ret::TextMap(MapVal::ScalarF64);
            }
            return Ret::Unsupported(format!(
                "NSDictionary value `{v}` not modelled (NSString / NSNumber only)"
            ));
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
        if self.is_string_array(base_ty) {
            self.needs_vec = true;
            return Arg::Id(format!("bridge::nsarray_of_text({pname})"));
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
            "double" | "CGFloat" | "NSTimeInterval" => return Arg::ScalarF64(pname.to_string()),
            "int" => return Arg::ScalarI32(pname.to_string()),
            "unsigned int" | "unsigned" => return Arg::ScalarU32(pname.to_string()),
            "float" => return Arg::ScalarF32(pname.to_string()),
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
        if self.is_string_array(b) {
            return "vec::Vec[text::Text]".to_string();
        }
        if let Some(objc_enum) = self.enum_of(b) {
            return self.enums.get(&objc_enum).unwrap().cplus_name.clone();
        }
        match b {
            "NSInteger" | "long" => "i64".to_string(),
            "NSUInteger" | "unsigned long" => "u64".to_string(),
            "double" | "CGFloat" | "NSTimeInterval" => "f64".to_string(),
            "BOOL" | "_Bool" | "bool" => "bool".to_string(),
            "int" => "i32".to_string(),
            "unsigned int" | "unsigned" => "u32".to_string(),
            "float" => "f32".to_string(),
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

    /// `NSArray<NSString *> *` (or a string-typedef element like NLLanguage).
    fn is_string_array(&self, ty: &str) -> bool {
        let t = ty.trim();
        if let Some(rest) = t.strip_prefix("NSArray<") {
            let elem = rest.strip_suffix("> *").or_else(|| rest.strip_suffix(">*"));
            if let Some(elem) = elem {
                return self.is_nsstring(elem.trim());
            }
        }
        false
    }

    /// `NSDictionary<K, V> *` / `NSMutableDictionary<K, V> *` -> the (key, value)
    /// element spellings. The split is on the top-level comma so a generic value
    /// (`NSArray<NSString *> *`) doesn't get cut at its own inner comma. None for
    /// anything that isn't a two-parameter dictionary.
    fn parse_dict(&self, ty: &str) -> Option<(String, String)> {
        let t = ty.trim();
        let rest = t
            .strip_prefix("NSDictionary<")
            .or_else(|| t.strip_prefix("NSMutableDictionary<"))?;
        let inner = rest.strip_suffix("> *").or_else(|| rest.strip_suffix(">*"))?;
        let mut depth: i32 = 0;
        let mut comma: Option<usize> = None;
        for (i, c) in inner.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                ',' if depth == 0 => {
                    comma = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let idx = comma?;
        let k = inner[..idx].trim().to_string();
        let v = inner[idx + 1..].trim().to_string();
        Some((k, v))
    }

    fn is_nsnumber(&self, ty: &str) -> bool {
        let t = ty.trim();
        t == "NSNumber *" || t == "NSNumber*"
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
    let mut s = qt.trim();
    // Clang leaves some attribute macros in the type spelling. `NS_REFINED_FOR_SWIFT`
    // in particular leaks as a leading token on properties (e.g.
    // `NS_REFINED_FOR_SWIFT NSDictionary<...> *`), which would otherwise hide the
    // real type from the dict / object matchers. Strip it so a property accessor's
    // type normalizes to the same spelling as the equivalent method return.
    if let Some(rest) = s.strip_prefix("NS_REFINED_FOR_SWIFT ") {
        s = rest.trim();
    }
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

/// A snake_case identifier for a whole selector (joining every keyword), e.g.
/// `parser:didStartElement:` -> `parser_did_start_element`.
fn selector_ident(sel: &str) -> String {
    sel.split(':')
        .filter(|s| !s.is_empty())
        .map(snake)
        .collect::<Vec<_>>()
        .join("_")
}

/// Append `_` when a generated identifier collides with a C+ keyword. An ObjC
/// method or parameter named `type` / `for` / `in` / ... is legal there but
/// reserved here, so `type` becomes `type_`.
fn escape_keyword(name: String) -> String {
    const KW: &[&str] = &[
        "as", "async", "await", "borrow", "break", "const", "continue", "defer",
        "else", "enum", "extern", "false", "fn", "for", "gen", "if", "impl",
        "import", "in", "let", "loop", "match", "move", "mut", "opaque", "pub",
        "ref", "restrict", "return", "self", "static", "struct", "take", "this",
        "true", "type", "unsafe", "var", "while",
    ];
    if KW.contains(&name.as_str()) {
        format!("{name}_")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter() -> ObjcEmitter {
        ObjcEmitter::new("test.h", "", serde_json::json!({}))
    }

    #[test]
    fn maps_32bit_scalar_returns_to_i32_u32_f32() {
        let mut e = emitter();
        assert!(matches!(e.map_ret("int"), Ret::ScalarI32));
        assert!(matches!(e.map_ret("unsigned int"), Ret::ScalarU32));
        assert!(matches!(e.map_ret("unsigned"), Ret::ScalarU32));
        assert!(matches!(e.map_ret("float"), Ret::ScalarF32));
    }

    #[test]
    fn maps_32bit_scalar_args_to_i32_u32_f32() {
        let mut e = emitter();
        assert!(matches!(e.map_arg("int", "n"), Arg::ScalarI32(_)));
        assert!(matches!(e.map_arg("unsigned int", "n"), Arg::ScalarU32(_)));
        assert!(matches!(e.map_arg("float", "x"), Arg::ScalarF32(_)));
    }

    #[test]
    fn param_sig_types_for_32bit_scalars() {
        let e = emitter();
        assert_eq!(e.param_sig_type("int"), "i32");
        assert_eq!(e.param_sig_type("unsigned int"), "u32");
        assert_eq!(e.param_sig_type("unsigned"), "u32");
        assert_eq!(e.param_sig_type("float"), "f32");
    }

    #[test]
    fn send_expr_emits_32bit_msgsend_shapes() {
        let e = emitter();
        // `int` getter -> rt::msg_i32
        let g = e
            .send_expr("this._obj", "scale", &Ret::ScalarI32, &[])
            .expect("i32 getter shape is KNOWN");
        assert!(g.contains("rt::msg_i32("), "{g}");
        // `float` setter -> rt::msg_void_f32
        let s = e
            .send_expr("this._obj", "setAlpha:", &Ret::Void, &[Arg::ScalarF32("alpha".into())])
            .expect("void_f32 setter shape is KNOWN");
        assert!(s.contains("rt::msg_void_f32("), "{s}");
        // id-returning factory with an `unsigned` arg -> rt::msg_id_u32
        let f = e
            .send_expr("cls", "withCount:", &Ret::Object, &[Arg::ScalarU32("count".into())])
            .expect("id_u32 factory shape is KNOWN");
        assert!(f.contains("rt::msg_id_u32("), "{f}");
    }

    #[test]
    fn unmodelled_widths_and_shapes_stay_unsupported() {
        let mut e = emitter();
        // `short` is still not a modelled width -> Unsupported (negative case).
        assert!(matches!(e.map_ret("short"), Ret::Unsupported(_)));
        assert!(matches!(e.map_arg("short", "n"), Arg::Unsupported(_)));
        // A shape with no msgSend shim (two i32 args) has no KNOWN tag -> None.
        let none = e.send_expr(
            "r",
            "a:b:",
            &Ret::Void,
            &[Arg::ScalarI32("a".into()), Arg::ScalarI32("b".into())],
        );
        assert!(none.is_none());
    }

    #[test]
    fn strips_ns_refined_for_swift_prefix() {
        assert_eq!(
            strip_nullability("NS_REFINED_FOR_SWIFT NSDictionary<NSString *, NSNumber *> *"),
            ("NSDictionary<NSString *, NSNumber *> *".to_string(), false)
        );
    }

    #[test]
    fn refined_dict_return_bridges_to_stringmap_like_a_method() {
        // A property getter whose type carries the leaked `NS_REFINED_FOR_SWIFT`
        // prefix must bridge a string-keyed dict return to StringMap, the same as
        // a plain method return. (The NLLanguageRecognizer languageHints /
        // languageHypotheses case.)
        let mut e = emitter();
        let plain = e.map_ret("NSDictionary<NSString *, NSNumber *> * _Nonnull");
        let refined = e.map_ret("NS_REFINED_FOR_SWIFT NSDictionary<NSString *, NSNumber *> *");
        assert!(matches!(plain, Ret::TextMap(MapVal::ScalarF64)));
        assert!(matches!(refined, Ret::TextMap(MapVal::ScalarF64)));
    }

    #[test]
    fn escapes_cplus_keyword_identifiers() {
        // An ObjC param/method named `type`/`for` is legal there, reserved here.
        assert_eq!(escape_keyword("type".to_string()), "type_");
        assert_eq!(escape_keyword("for".to_string()), "for_");
        // Non-keywords pass through untouched.
        assert_eq!(escape_keyword("language".to_string()), "language");
        assert_eq!(escape_keyword("token_range".to_string()), "token_range");
    }
}
