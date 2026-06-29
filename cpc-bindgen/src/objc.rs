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
    // Header basenames whose decls this emitter owns. Single-header mode: just
    // the target header. `--merge` mode: every framework-home header, so the
    // whole framework emits as one module (full co-resident types, no stubs).
    home_set: HashSet<String>,
    // `--merge` parses the framework umbrella, so every real decl is `#include`d
    // (loc_included == true). The `home_set` basename match is then the only
    // home test; the loc_included guard (single-header only) must be bypassed.
    merge: bool,
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
    // C+ method names already emitted in the current impl. Several ObjC selectors
    // can collapse to one C+ name (`-open` / `-open:`, `-init` / `+new`); C+ has no
    // overloading, so the second becomes a SKIP rather than a duplicate-method error.
    seen_methods: HashSet<String>,
    // Emitted struct / type names. A class and a protocol can share a name
    // (NSObject, NSTextAttachmentCell); the later one is renamed, not duplicated.
    seen_types: HashSet<String>,
    // Every wrapper-type name that WILL be defined in this module, registered up
    // front (Pass 2a) so a method returning a type whose definition comes later
    // (Device.newCommandQueue -> CommandQueue) can be typed, not degraded to *u8.
    // Kept separate from `seen_types` so the class/protocol disambiguator is not
    // tricked into renaming a type against its own pre-registration.
    known_types: HashSet<String>,
    // Wrapper names that are ObjC protocols — always non-owning (`opaque`, no
    // drop). An `NSArray<id<P>>` may be bridged to a `Vec[P]` only for these,
    // since a Vec of owning wrappers would over-release +0-borrowed elements.
    protocol_types: HashSet<String>,
    // By-value C structs (`typedef struct { … } MTLSize;`): ObjC name -> ordered
    // (field, C+ type). `used_value_structs` are the ones actually referenced (so
    // only those get a `#[repr(C)]` definition). `used_struct_shapes` are the
    // distinct (ret, arg) msgSend signatures involving a struct, each emitted as a
    // module-local `objc_msgSend` shim (these aren't in the shared rt:: zoo).
    value_structs: HashMap<String, Vec<(String, String)>>,
    used_value_structs: HashSet<String>,
    used_struct_shapes: HashSet<String>,
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
    // True when the ObjC declaration used NS_OPTIONS (flag_enum attribute).
    // These emit as u64 constants rather than a C+ enum, and callers combine
    // them with `|` directly.
    is_options: bool,
}

enum Ret {
    Void,
    // An object handle. `Some(name)` -> a typed wrapper (`-> Name`, wrapped via
    // `Name::from_raw`); `None` -> the raw `*u8` fallback.
    Object(Option<String>),
    ObjectOption(Option<String>), // nullable: Some -> Option[Name], None -> Option[*u8]
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
    Rect,  // NSRect / CGRect — rt::Rect HFA
    Point, // NSPoint / CGPoint — rt::Point HFA
    Size,  // NSSize / CGSize — rt::Size HFA
    ValueArray, // NSArray<NSValue *> -> Vec[Range]
    TextArray,  // NSArray<NSString *> -> Vec[Text]
    ObjectArray(String), // NSArray<id<P>> -> Vec[P]; only non-owning protocol elems
    Struct(String), // a by-value C struct (MTLSize) returned by value (sret)
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
    Rect(String),  // NSRect / CGRect — rt::Rect HFA
    Point(String), // NSPoint / CGPoint — rt::Point HFA
    Size(String),  // NSSize / CGSize — rt::Size HFA
    Struct(String, String), // (C+ struct type, arg expr) — by-value C struct (MTLSize)
    Unsupported(String),
}

impl ObjcEmitter {
    pub fn new(header_path: &str, prefix: &str, overrides: serde_json::Value) -> Self {
        ObjcEmitter {
            header_path: header_path.to_string(),
            home_set: std::iter::once(base(header_path)).collect(),
            merge: false,
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
            seen_methods: HashSet::new(),
            seen_types: HashSet::new(),
            known_types: HashSet::new(),
            protocol_types: HashSet::new(),
            value_structs: HashMap::new(),
            used_value_structs: HashSet::new(),
            used_struct_shapes: HashSet::new(),
        }
    }

    /// `--merge` mode: one emitter that owns every framework-home header, so the
    /// whole framework lands in one module with all wrapper types co-resident
    /// (full types, no cross-module stubs, no cyclic-import problem).
    pub fn new_merged(
        label: &str,
        prefix: &str,
        overrides: serde_json::Value,
        home_set: HashSet<String>,
    ) -> Self {
        let mut e = ObjcEmitter::new(label, prefix, overrides);
        e.home_set = home_set;
        e.merge = true;
        e
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
        // Pass 1: typedefs (name -> underlying), NS_ENUMs, and by-value struct
        // layouts. clang emits the anonymous `RecordDecl` immediately before the
        // `TypedefDecl` that names it (`typedef struct { … } MTLSize;`).
        let mut prev_record: Option<&serde_json::Value> = None;
        for decl in inner {
            let kind = decl.get("kind").and_then(|v| v.as_str());
            match kind {
                Some("TypedefDecl") => {
                    if let (Some(name), Some(under)) = (
                        decl.get("name").and_then(|v| v.as_str()),
                        decl.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()),
                    ) {
                        self.typedefs.insert(name.to_string(), under.to_string());
                        if under.starts_with("struct ") {
                            if let Some(rec) = prev_record {
                                self.collect_value_struct(name, rec);
                            }
                        }
                    }
                }
                Some("EnumDecl") => self.collect_enum(decl),
                _ => {}
            }
            prev_record = if kind == Some("RecordDecl") { Some(decl) } else { None };
        }
        // Pass 2: collect home-header interfaces + the categories that extend
        // them (Foundation puts much of NSScanner/NSString/... in categories),
        // then emit each class with its interface + category methods merged.
        // "Home" = the target header (single-header) or any framework header
        // (`--merge`); decls from #imported system headers are skipped either way.
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
            if !self.merge && decl.get("loc").map(loc_included).unwrap_or(false) {
                continue;
            }
            let in_home = current_file
                .as_deref()
                .map(base)
                .map(|f| self.home_set.contains(&f))
                .unwrap_or(false);
            if !in_home {
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
        // Same as interfaces: clang emits a forward `@protocol X;` (no methods)
        // alongside the real definition. Keep the richest per name, else the empty
        // forward decl races the full one and the class/protocol disambiguator
        // demotes the real API to `<X>Protocol` while a 2-fn stub claims `X`.
        let mut deduped_protocols: Vec<serde_json::Value> = Vec::new();
        for proto in &protocols {
            let name = proto.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let count = proto.get("inner").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            match deduped_protocols.iter().position(|e| e.get("name").and_then(|n| n.as_str()).unwrap_or("") == name) {
                Some(pos) => {
                    let ec = deduped_protocols[pos].get("inner").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    if count > ec {
                        deduped_protocols[pos] = proto.clone();
                    }
                }
                None => deduped_protocols.push(proto.clone()),
            }
        }
        // Pass 2a — pre-register every wrapper-type name BEFORE any body emits, so a
        // method returning a type whose definition comes later (the device factory
        // returns, emitted before Buffer/Texture/CommandQueue) is typed, not
        // degraded to *u8 by emission order. Delegate/data-source protocols are
        // synthesis helpers, not object wrappers with `from_raw`, so they are not
        // registered as typeable returns.
        for itf in &deduped {
            if let Some(n) = itf.get("name").and_then(|n| n.as_str()) {
                self.known_types.insert(self.cplus_type_name(n));
            }
        }
        for proto in &deduped_protocols {
            let pname = proto.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !(pname.ends_with("Delegate") || pname.ends_with("DataSource")) {
                let ty = self.cplus_type_name(pname);
                self.known_types.insert(ty.clone());
                self.protocol_types.insert(ty);
            }
        }
        for itf in &deduped {
            let cls = itf.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let cats: &[serde_json::Value] =
                categories.get(cls).map(|v| v.as_slice()).unwrap_or(&[]);
            self.emit_interface(itf, cats);
        }
        // A `…Delegate` / `…DataSource` protocol is a callback sink the user
        // *implements* -> a runtime class-synthesis helper. Every other protocol
        // (MTLDevice, MTLBuffer, … — the whole Metal API surface) is an object the
        // user *calls* -> an opaque-handle wrapper struct, same as a class.
        for proto in &deduped_protocols {
            let pname = proto.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if pname.ends_with("Delegate") || pname.ends_with("DataSource") {
                self.emit_protocol_delegate(proto);
            } else {
                self.emit_protocol_api(proto);
            }
        }

        // Assemble: preamble + the enum defs actually used + interface bodies.
        let mut out = self.preamble();
        for objc_name in self.used_enums.clone() {
            if let Some(info) = self.enums.get(&objc_name).cloned() {
                out.push_str(&self.render_enum(&info));
            }
        }
        out.push_str(&self.render_value_structs());
        out.push_str(&self.render_struct_shims());
        out.push_str(&self.block_helpers);
        out.push_str(&self.body);
        out
    }

    /// `#[repr(C)]` definitions for the by-value structs actually used, in a
    /// deterministic order (sorted) so regeneration is byte-stable.
    fn render_value_structs(&self) -> String {
        let mut names: Vec<&String> = self.used_value_structs.iter().collect();
        names.sort();
        let mut s = String::new();
        for objc_name in names {
            let Some(fields) = self.value_structs.get(objc_name) else { continue };
            let ty = self.cplus_type_name(objc_name);
            s.push_str(&format!("// `{objc_name}` — by-value C struct.\n#[repr(C)]\nstruct {ty} {{\n"));
            for (f, t) in fields {
                s.push_str(&format!("    {f}: {t},\n"));
            }
            s.push_str("}\n\n");
        }
        s
    }

    /// Module-local `objc_msgSend` shims for the by-value-struct call shapes used.
    /// The struct types ride registers/indirect per the platform ABI (verified
    /// against clang for the 24-byte case); cpc lowers the by-value extern the same.
    fn render_struct_shims(&self) -> String {
        let mut shapes: Vec<&String> = self.used_struct_shapes.iter().collect();
        shapes.sort();
        let mut s = String::new();
        for suffix in shapes {
            let parts: Vec<&str> = suffix.split('_').collect();
            let mut params = String::from("recv: *u8, sel: *u8");
            for (i, t) in parts[1..].iter().enumerate() {
                params.push_str(&format!(", a{i}: {}", tag_to_type(t)));
            }
            let ret = if parts[0] == "void" {
                String::new()
            } else {
                format!(" -> {}", tag_to_type(parts[0]))
            };
            s.push_str(&format!(
                "#[link_name = \"objc_msgSend\"]\nextern fn objc_msg_{suffix}({params}){ret};\n"
            ));
        }
        if !s.is_empty() {
            s.push('\n');
        }
        s
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
            variants.push((crate::sanitize_ident(&pascal(&variant)), val));
            next = val + 1;
        }
        if variants.is_empty() {
            return;
        }
        let is_options = decl
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter().any(|n| {
                    n.get("kind").and_then(|k| k.as_str()) == Some("FlagEnumAttr")
                })
            })
            .unwrap_or(false);
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
                is_options,
            },
        );
    }

    /// Record a `typedef struct {…} Name;` field layout (the anonymous record
    /// `rec` holds the fields). Only kept when every field maps to a C+ type (a
    /// scalar or an already-collected value struct), so the repr(C) is emittable.
    fn collect_value_struct(&mut self, name: &str, rec: &serde_json::Value) {
        let mut fields: Vec<(String, String)> = Vec::new();
        for f in rec.get("inner").and_then(|v| v.as_array()).into_iter().flatten() {
            if f.get("kind").and_then(|k| k.as_str()) != Some("FieldDecl") {
                continue;
            }
            let fname = f.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let fqt = f.get("type").and_then(|t| t.get("qualType")).and_then(|v| v.as_str()).unwrap_or("");
            match self.struct_field_type(fqt) {
                Some(ty) if !fname.is_empty() => fields.push((crate::sanitize_ident(fname), ty)),
                _ => return, // an unmappable field — don't record (method stays skipped)
            }
        }
        if !fields.is_empty() {
            self.value_structs.insert(name.to_string(), fields);
        }
    }

    /// A value-struct field's C+ type: a scalar, a nested value struct (the header
    /// always declares it first), or a typedef chain to either (`MTLGPUAddress` ->
    /// `uint64_t` -> u64). Returns None for anything else, so the struct isn't
    /// recorded (keeps its repr(C) layout exactly C-ABI or absent — never wrong).
    fn struct_field_type(&self, fqt: &str) -> Option<String> {
        let mut cur = fqt.trim().to_string();
        for _ in 0..8 {
            let mapped = match cur.as_str() {
                "NSUInteger" | "unsigned long" | "unsigned long long" | "uint64_t" | "size_t" => "u64",
                "NSInteger" | "long" | "long long" | "int64_t" => "i64",
                "double" | "CGFloat" | "NSTimeInterval" | "CFTimeInterval" => "f64",
                "float" => "f32",
                "int" | "int32_t" => "i32",
                "unsigned int" | "unsigned" | "uint32_t" => "u32",
                "BOOL" | "_Bool" | "bool" => "bool",
                "uint16_t" | "unsigned short" => "u16",
                "int16_t" | "short" => "i16",
                "uint8_t" | "unsigned char" => "u8",
                "int8_t" | "signed char" => "i8",
                _ => "",
            };
            if !mapped.is_empty() {
                return Some(mapped.to_string());
            }
            if self.value_structs.contains_key(&cur) {
                return Some(self.cplus_type_name(&cur));
            }
            match self.typedefs.get(&cur) {
                Some(u) => cur = u.trim().to_string(),
                None => break,
            }
        }
        None
    }

    /// Mark a value struct (and, transitively, its struct-typed fields) as used,
    /// so its repr(C) definition is emitted.
    fn mark_value_struct_used(&mut self, objc_name: &str) {
        if !self.used_value_structs.insert(objc_name.to_string()) {
            return;
        }
        let nested: Vec<String> = self
            .value_structs
            .get(objc_name)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|(_, ty)| {
                        self.value_structs.keys().find(|k| self.cplus_type_name(k) == *ty).cloned()
                    })
                    .collect()
            })
            .unwrap_or_default();
        for n in nested {
            self.mark_value_struct_used(&n);
        }
    }

    fn render_enum(&self, e: &EnumInfo) -> String {
        let mut s = String::new();
        if e.is_options {
            // NS_OPTIONS: emit u64 constants so callers combine flags with `|`.
            s.push_str(&format!("// `{}` (NS_OPTIONS) — bitfield; combine with `|`.\n", e.objc_name));
            let prefix = snake(&e.cplus_name);
            for (v, val) in &e.variants {
                let const_name = format!("{}_{}", prefix, snake(v));
                s.push_str(&format!("const {const_name}: u64 = {val} as u64;\n"));
            }
            s.push('\n');
            return s;
        }
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
        let stripped = objc_name.strip_prefix(&self.prefix).unwrap_or(objc_name);
        // Keep the prefix when stripping it would open the name with a digit
        // (`MTL4Archive` -> `4Archive`), which is not a valid type identifier.
        // sanitize_ident is the final guard (residual digit / keyword collision).
        let chosen = if stripped.starts_with(|c: char| c.is_ascii_digit()) {
            objc_name
        } else {
            stripped
        };
        crate::sanitize_ident(chosen)
    }

    /// The C+ wrapper-type name for an object spelling, or None when it has no
    /// single typeable wrapper. Some only for: single-protocol `id<X>`, a single
    /// class pointer `Foo *`. None for bare `id`/`instancetype` (no protocol info),
    /// multi-protocol `id<A,B>`, `Class`/`SEL`, blocks (`^`), generic collections,
    /// and pointer-to-pointer out-params (`NSError **`). Pure name computation —
    /// the existence gate is applied separately by `typed_object`.
    fn wrapper_name_of(&self, base_ty: &str) -> Option<String> {
        // Pointer-to-pointer (`NSError * _Nullable *`, `id *`): an out-param, not
        // an object value — never a wrapper. (203 NSError** in scope; the single
        // biggest mistyping trap.)
        if base_ty.matches('*').count() >= 2 {
            return None;
        }
        if let Some(rest) = base_ty.strip_prefix("id<").or_else(|| base_ty.strip_prefix("id <")) {
            let inner = rest.strip_suffix('>')?.trim();
            // Multi-protocol (`id<A,B,C>`): no single wrapper; don't guess.
            if inner.is_empty() || inner.contains(',') {
                return None;
            }
            return Some(self.cplus_type_name(inner));
        }
        if base_ty == "id" || base_ty == "instancetype" || base_ty == "Class" || base_ty == "SEL" {
            return None;
        }
        if base_ty.contains('<') || base_ty.contains('^') {
            return None;
        }
        // A single class pointer `Foo *` (one token, one star). A space in the
        // token means a primitive (`unsigned char *`) or qualifier, not a class.
        if let Some(token) = base_ty.strip_suffix('*') {
            let token = token.trim();
            if token.is_empty() || token.contains(['*', '<', ' ']) {
                return None;
            }
            return Some(self.cplus_type_name(token));
        }
        None
    }

    /// `wrapper_name_of` gated on the wrapper actually being defined in this
    /// module (`known_types`). This is THE soundness gate: it makes a typed
    /// return/arg legal (the struct exists) and degrades foreign-framework types
    /// (NSURL/NSError/CAMetalLayer — never declared here) to `*u8`.
    fn typed_object(&self, base_ty: &str) -> Option<String> {
        self.wrapper_name_of(base_ty).filter(|n| self.known_types.contains(n))
    }

    fn emit_interface(&mut self, itf: &serde_json::Value, categories: &[serde_json::Value]) {
        let objc_name = match itf.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        let ty = self.cplus_type_name(&objc_name);
        self.seen_types.insert(ty.clone());
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
        self.body.push_str(&format!("    fn from_raw(ptr: *u8) -> {ty} {{ return {ty} {{ _obj: ptr }}; }}\n\n"));

        // Fresh method-name scope per impl; `raw`/`from_raw` are always emitted
        // above, `drop` below (owned), so reserve them so a selector can't collide.
        self.seen_methods.clear();
        self.seen_methods.insert("raw".to_string());
        self.seen_methods.insert("from_raw".to_string());
        if owned {
            self.seen_methods.insert("drop".to_string());
        }

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

    // A non-delegate protocol (an object the user CALLS — MTLDevice, MTLBuffer,
    // every Metal encoder/pipeline/resource) -> an opaque-handle wrapper struct,
    // exactly like a non-owning class: `from_raw`/`raw` + one method per protocol
    // method, dispatched through the same typed `objc_msgSend` shims. Protocols
    // have no `init` and Metal objects are +0-borrowed (owned by their creator),
    // so the handle is `opaque` with no `drop`.
    fn emit_protocol_api(&mut self, proto: &serde_json::Value) {
        let objc_name = match proto.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return,
        };
        let mut ty = self.cplus_type_name(&objc_name);
        // A class and a protocol can carry the same name (NSObject,
        // NSTextAttachmentCell). The class wrapper already claimed it, so the
        // protocol's wrapper takes a suffixed name rather than redefine it.
        if self.seen_types.contains(&ty) {
            let mut disambig = format!("{ty}Protocol");
            let mut n = 2;
            while self.seen_types.contains(&disambig) {
                disambig = format!("{ty}Protocol{n}");
                n += 1;
            }
            ty = disambig;
        }
        self.seen_types.insert(ty.clone());
        let mut methods: Vec<serde_json::Value> = Vec::new();
        let mut seen_sel: HashSet<String> = HashSet::new();
        for m in proto.get("inner").and_then(|v| v.as_array()).into_iter().flatten() {
            if m.get("kind").and_then(|k| k.as_str()) != Some("ObjCMethodDecl") {
                continue;
            }
            let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if seen_sel.insert(sel) {
                methods.push(m.clone());
            }
        }
        self.body.push_str(&format!(
            "// `{objc_name}` (ObjC protocol). Non-owning handle; `opaque`, no drop.\n"
        ));
        self.body.push_str(&format!("struct {ty} {{\n    opaque _obj: *u8,\n}}\n\n"));
        self.body.push_str(&format!("impl {ty} {{\n"));
        self.body.push_str("    fn raw(this) -> *u8 { return this._obj; }\n\n");
        self.body.push_str(&format!(
            "    fn from_raw(ptr: *u8) -> {ty} {{ return {ty} {{ _obj: ptr }}; }}\n\n"
        ));
        self.seen_methods.clear();
        self.seen_methods.insert("raw".to_string());
        self.seen_methods.insert("from_raw".to_string());
        for m in &methods {
            self.emit_method(m, &objc_name, &ty, false, false);
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
        let mut proto_ty = self.cplus_type_name(&proto_objc);
        // A class and a protocol can carry the same name (NSObject,
        // NSTextAttachmentCell). The class wrapper already claimed it, so the
        // protocol's synthesis helper takes a suffixed name rather than collide.
        if self.seen_types.contains(&proto_ty) {
            let mut disambig = format!("{proto_ty}Protocol");
            let mut n = 2;
            while self.seen_types.contains(&disambig) {
                disambig = format!("{proto_ty}Protocol{n}");
                n += 1;
            }
            proto_ty = disambig;
        }
        self.seen_types.insert(proto_ty.clone());
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
            // The callback name becomes a struct field and accessor; escape it so
            // a selector like `type` doesn't land on a keyword (`type` -> `type_`).
            callbacks.push((sel.clone(), crate::sanitize_ident(&mid), params.len(), ret));
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
            let pn = crate::sanitize_ident(&ov_params.get(idx).cloned().unwrap_or_else(|| snake(pname)));
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
            let send = self.send_expr("alloced", &sel, &Ret::Object(None), &args);
            let send = match send {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: init arg shape not yet modelled\n"));
                    return;
                }
            };
            if !self.seen_methods.insert("new".to_string()) {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: `new` already defined (extra constructor)\n"));
                return;
            }
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
        let name = crate::sanitize_ident(&ov_name.clone().unwrap_or_else(|| mechanical_name(&sel)));
        // C+ has no overloading: if this name is taken (`-open` then `-open:`, or
        // `-init` then `+new`), skip rather than emit a duplicate method.
        if !self.seen_methods.insert(name.clone()) {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: method `{name}` already defined\n"));
            return;
        }

        // A class factory returning the class's own type (`instancetype` or
        // `Class *`) -> a wrapped `Self` (or `Option[Self]` if nullable).
        // Factories hand back a +0 autoreleased object, so for an owned wrapper
        // we `retain` it to balance `drop`.
        let (ret_base, ret_nullable) = strip_nullability(ret_qt);
        let returns_self = !is_instance
            && matches!(ret, Ret::Object(_) | Ret::ObjectOption(_))
            && (ret_base.trim() == "instancetype" || ret_base.trim() == format!("{objc_class} *"));
        if returns_self {
            if let Some(send) = self.send_expr(&recv, &sel, &Ret::Object(None), &args) {
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
            let array_call = match self.send_expr(&recv, &sel, &Ret::Object(None), &args) {
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

        if let Ret::ObjectArray(elem_ty) = &ret {
            let array_call = match self.send_expr(&recv, &sel, &Ret::Object(None), &args) {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: NSArray arg shape not modelled\n"));
                    return;
                }
            };
            self.needs_vec = true;
            let sep = if receiver.is_empty() || sig_param.is_empty() { "" } else { ", " };
            self.body.push_str(&format!(
                "    fn {name}({receiver}{sep}{sig_param}) -> vec::Vec[{elem_ty}] {{\n\
                 \x20       let arr: *u8 = {array_call};\n\
                 \x20       let n: u64 = rt::msg_u64(arr, rt::sel(#str_ptr(\"count\\0\")));\n\
                 \x20       var out: vec::Vec[{elem_ty}] = vec::Vec[{elem_ty}]::with_capacity(n as usize);\n\
                 \x20       let at_sel: *u8 = rt::sel(#str_ptr(\"objectAtIndex:\\0\"));\n\
                 \x20       var i: u64 = 0 as u64;\n\
                 \x20       while i < n {{\n\
                 \x20           out.append({elem_ty}::from_raw(rt::msg_id_u64(arr, at_sel, i)));\n\
                 \x20           i = i +% (1 as u64);\n\
                 \x20       }}\n\
                 \x20       return out;\n    }}\n\n"
            ));
            return;
        }

        if let Ret::TextMap(val) = ret {
            let dict_call = match self.send_expr(&recv, &sel, &Ret::Object(None), &args) {
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
                // Name the missing shape so the gap is actionable (which shim to add).
                let shape = msg_shape(&ret, &args)
                    .map(|s| format!("`{s}`"))
                    .unwrap_or_else(|| "value-type".to_string());
                self.body.push_str(&format!("    // SKIPPED `{sel}`: msgSend shape {shape} not yet modelled\n"));
                return;
            }
        };

        let sep = if receiver.is_empty() || sig_param.is_empty() { "" } else { ", " };
        let (ret_spelling, body_line) = match &ret {
            Ret::Void => (String::new(), format!("        {send};\n")),
            Ret::Struct(n) => (format!(" -> {n}"), format!("        return {send};\n")),
            Ret::Bool => (" -> bool".into(), format!("        return {send} != (0 as i8);\n")),
            Ret::Object(None) => (" -> *u8".into(), format!("        return {send};\n")),
            Ret::Object(Some(n)) => (
                format!(" -> {n}"),
                format!("        return {n}::from_raw({send});\n"),
            ),
            Ret::ObjectOption(None) => (
                " -> option::Option[*u8]".into(),
                format!("        let obj: *u8 = {send};\n        return bridge::obj_option(obj);\n"),
            ),
            Ret::ObjectOption(Some(n)) => (
                format!(" -> option::Option[{n}]"),
                format!("        let obj: *u8 = {send};\n        if obj == {{ 0 as *u8 }} {{\n            return option::Option[{n}]::None;\n        }}\n        return option::some({n}::from_raw(obj));\n"),
            ),
            Ret::ScalarI64 => (" -> i64".into(), format!("        return {send};\n")),
            Ret::ScalarU64 => (" -> u64".into(), format!("        return {send};\n")),
            Ret::ScalarF64 => (" -> f64".into(), format!("        return {send};\n")),
            Ret::ScalarI32 => (" -> i32".into(), format!("        return {send};\n")),
            Ret::ScalarU32 => (" -> u32".into(), format!("        return {send};\n")),
            Ret::ScalarF32 => (" -> f32".into(), format!("        return {send};\n")),
            Ret::Range => (" -> rt::Range".into(), format!("        return {send};\n")),
            Ret::Rect => (" -> rt::Rect".into(), format!("        return {send};\n")),
            Ret::Point => (" -> rt::Point".into(), format!("        return {send};\n")),
            Ret::Size => (" -> rt::Size".into(), format!("        return {send};\n")),
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
            Ret::ValueArray | Ret::TextArray | Ret::ObjectArray(_) | Ret::TextMap(_) | Ret::Unsupported(_) => unreachable!(),
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
            let pn = crate::sanitize_ident(&ov_params.get(idx).cloned().unwrap_or_else(|| snake(pname)));
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
    fn send_expr(&mut self, recv: &str, sel: &str, ret: &Ret, args: &[Arg]) -> Option<String> {
        let suffix = msg_shape(ret, args)?;
        // A by-value-struct shape uses a module-local `objc_msg_*` shim (emitted
        // alongside the struct); everything else uses the shared rt:: zoo, which
        // only models a fixed set of shapes (others SKIP to stay compilable).
        let prefix = if msg_shape_has_struct(&suffix) {
            // Record it here (not in the caller) so every path — factory/init/
            // collection helpers included — gets its shim emitted.
            self.used_struct_shapes.insert(suffix.clone());
            "objc_msg_"
        } else if msg_shape_is_known(&suffix) {
            "rt::msg_"
        } else {
            return None;
        };
        let sl = format!("rt::sel(#str_ptr(\"{sel}\\0\"))");
        let mut call = format!("{prefix}{suffix}({recv}, {sl}");
        for a in args {
            call.push_str(&format!(", {}", arg_expr(a)?));
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
        if base_ty == "NSRect" || base_ty == "CGRect" {
            return Ret::Rect;
        }
        if base_ty == "NSPoint" || base_ty == "CGPoint" {
            return Ret::Point;
        }
        if base_ty == "NSSize" || base_ty == "CGSize" {
            return Ret::Size;
        }
        if self.is_value_array(base_ty) {
            return Ret::ValueArray;
        }
        if self.is_string_array(base_ty) {
            return Ret::TextArray;
        }
        // `NSArray<id<P>> *` whose element is a non-owning protocol wrapper ->
        // `Vec[P]`. (Class-element arrays may be owning, so they stay skipped.)
        if let Some(elem) = array_element(base_ty) {
            if let Some(w) = self.wrapper_name_of(&elem) {
                if self.protocol_types.contains(&w) {
                    return Ret::ObjectArray(w);
                }
            }
        }
        if self.is_nsstring(base_ty) {
            return Ret::Text { nullable };
        }
        if let Some(objc_enum) = self.enum_of(base_ty) {
            if !self.used_enums.contains(&objc_enum) {
                self.used_enums.push(objc_enum.clone());
            }
            if self.enums.get(&objc_enum).map(|e| e.is_options).unwrap_or(false) {
                return Ret::ScalarU64;
            }
            return Ret::EnumTy(objc_enum);
        }
        match base_ty {
            "NSInteger" | "long" | "long long" | "int64_t" => return Ret::ScalarI64,
            // `MTLResourceID` is `{ uint64_t _impl; }` — one integer eightbyte,
            // ABI-identical to `u64`. `MTLGPUAddress` is a `uint64_t` typedef.
            "NSUInteger" | "unsigned long" | "unsigned long long" | "uint64_t"
            | "MTLGPUAddress" | "MTLResourceID" => return Ret::ScalarU64,
            "BOOL" | "_Bool" | "bool" => return Ret::Bool,
            "instancetype" => return Ret::Object(None), // a fresh +1, never nil; wrapped as Self
            "id" => {
                // Bare `id` carries no type -> untyped handle.
                return if nullable { Ret::ObjectOption(None) } else { Ret::Object(None) };
            }
            // CGFloat / NSTimeInterval / CFTimeInterval are `double` on 64-bit Apple.
            "double" | "CGFloat" | "NSTimeInterval" | "CFTimeInterval" => return Ret::ScalarF64,
            // 32-bit scalars ride their own msgSend widths (vendor/objc shims).
            "int" | "int32_t" => return Ret::ScalarI32,
            "unsigned int" | "unsigned" | "uint32_t" => return Ret::ScalarU32,
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
        // A protocol-qualified id (`id<MTLDevice>`, `id<A,B>`) is an ordinary ObjC
        // object pointer — ABI-identical to `id`. Must be recognized before the
        // generic-collection guard (it ends in `>`, not `*`).
        if base_ty.starts_with("id<") || base_ty.starts_with("id <") {
            let typed = self.typed_object(base_ty);
            return if nullable { Ret::ObjectOption(typed) } else { Ret::Object(typed) };
        }
        if base_ty.contains('<') {
            return Ret::Unsupported("generic collection".into());
        }
        if base_ty.contains('^') {
            return Ret::Unsupported("block".into());
        }
        if base_ty.ends_with('*') {
            let typed = self.typed_object(base_ty);
            return if nullable { Ret::ObjectOption(typed) } else { Ret::Object(typed) };
        }
        // A by-value C struct (MTLSize) returned by value.
        if self.value_structs.contains_key(base_ty) {
            self.mark_value_struct_used(base_ty);
            return Ret::Struct(self.cplus_type_name(base_ty));
        }
        Ret::Unsupported(format!("unmapped type `{base_ty}`"))
    }

    fn map_arg(&mut self, qt: &str, pname: &str) -> Arg {
        let (base_ty, _) = strip_nullability(qt);
        let base_ty = base_ty.trim();
        if base_ty == "NSRange" {
            return Arg::Range(pname.to_string());
        }
        if base_ty == "NSRect" || base_ty == "CGRect" {
            return Arg::Rect(pname.to_string());
        }
        if base_ty == "NSPoint" || base_ty == "CGPoint" {
            return Arg::Point(pname.to_string());
        }
        if base_ty == "NSSize" || base_ty == "CGSize" {
            return Arg::Size(pname.to_string());
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
            let info = self.enums.get(&objc_enum).unwrap();
            if info.is_options {
                return Arg::ScalarU64(pname.to_string());
            }
            let raw = info.raw_fn.clone();
            return Arg::ScalarI64(format!("{raw}({pname})"));
        }
        match base_ty {
            "NSInteger" | "long" | "long long" | "int64_t" => {
                return Arg::ScalarI64(pname.to_string())
            }
            "NSUInteger" | "unsigned long" | "unsigned long long" | "uint64_t"
            | "MTLGPUAddress" | "MTLResourceID" => return Arg::ScalarU64(pname.to_string()),
            "BOOL" | "_Bool" | "bool" => return Arg::Bool(pname.to_string()),
            "id" => return Arg::Id(pname.to_string()),
            "double" | "CGFloat" | "NSTimeInterval" | "CFTimeInterval" => {
                return Arg::ScalarF64(pname.to_string())
            }
            "int" | "int32_t" => return Arg::ScalarI32(pname.to_string()),
            "unsigned int" | "unsigned" | "uint32_t" => return Arg::ScalarU32(pname.to_string()),
            "float" => return Arg::ScalarF32(pname.to_string()),
            _ => {}
        }
        if base_ty.contains('^') {
            return Arg::Unsupported("block".into());
        }
        // Protocol-qualified id -> ordinary object pointer (before the
        // generic-collection guard). A typed wrapper param passes `.raw()`.
        if base_ty.starts_with("id<") || base_ty.starts_with("id <") {
            if self.typed_object(base_ty).is_some() {
                return Arg::Id(format!("{pname}.raw()"));
            }
            return Arg::Id(pname.to_string());
        }
        if base_ty.contains('<') {
            return Arg::Unsupported("generic collection".into());
        }
        if base_ty.ends_with('*') {
            if self.typed_object(base_ty).is_some() {
                return Arg::Id(format!("{pname}.raw()"));
            }
            return Arg::Id(pname.to_string());
        }
        // A by-value C struct (MTLSize) passed by value.
        if self.value_structs.contains_key(base_ty) {
            self.mark_value_struct_used(base_ty);
            return Arg::Struct(self.cplus_type_name(base_ty), pname.to_string());
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
        if b == "NSRect" || b == "CGRect" {
            return "rt::Rect".to_string();
        }
        if b == "NSPoint" || b == "CGPoint" {
            return "rt::Point".to_string();
        }
        if b == "NSSize" || b == "CGSize" {
            return "rt::Size".to_string();
        }
        if self.is_nsstring(b) {
            return "str".to_string();
        }
        if self.is_string_array(b) {
            return "vec::Vec[text::Text]".to_string();
        }
        if let Some(objc_enum) = self.enum_of(b) {
            let info = self.enums.get(&objc_enum).unwrap();
            if info.is_options {
                return "u64".to_string();
            }
            return info.cplus_name.clone();
        }
        // A typed object param shows the wrapper type (`descriptor: TextureDescriptor`).
        // Same gate as map_arg, so the public signature and the `.raw()` wire expr
        // never disagree.
        if let Some(name) = self.typed_object(b) {
            return name;
        }
        match b {
            "NSInteger" | "long" | "long long" | "int64_t" => "i64".to_string(),
            "NSUInteger" | "unsigned long" | "unsigned long long" | "uint64_t"
            | "MTLGPUAddress" | "MTLResourceID" => "u64".to_string(),
            "double" | "CGFloat" | "NSTimeInterval" | "CFTimeInterval" => "f64".to_string(),
            "BOOL" | "_Bool" | "bool" => "bool".to_string(),
            "int" | "int32_t" => "i32".to_string(),
            "unsigned int" | "unsigned" | "uint32_t" => "u32".to_string(),
            "float" => "f32".to_string(),
            // A by-value C struct (after the explicit scalar typedefs above, so
            // single-integer wrappers like MTLResourceID stay u64, matching map_arg).
            _ if self.value_structs.contains_key(b) => self.cplus_type_name(b),
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

/// The element spelling of an `NSArray<ELEM> *` (`NSArray<id<MTLBuffer>> *` ->
/// `id<MTLBuffer>`), or None when `ty` is not a single-parameter NSArray.
fn array_element(ty: &str) -> Option<String> {
    let t = ty.trim();
    let rest = t.strip_prefix("NSArray<")?;
    let elem = rest.strip_suffix("> *").or_else(|| rest.strip_suffix(">*"))?;
    Some(elem.trim().to_string())
}

/// The msgSend ABI shape `<ret>[_<arg>...]` for a (return, args) pair — the key
/// into the runtime's typed `objc_msgSend` shims. None when a return or arg is a
/// value type with no shim tag (NSArray/NSDictionary/Unsupported). Shared by
/// `send_expr` (to pick the shim) and the skip diagnostic (to name the gap).
fn msg_shape(ret: &Ret, args: &[Arg]) -> Option<String> {
    // Scalar/geometry tags are lowercase; a by-value struct contributes its C+
    // type name (PascalCase) so the two can never collide (`size` vs `Size`).
    let ret_tag: String = match ret {
        Ret::Void => "void".into(),
        Ret::Object(_) | Ret::ObjectOption(_) | Ret::Text { .. } => "id".into(),
        Ret::Bool => "i8".into(),
        Ret::ScalarI64 | Ret::EnumTy(_) => "i64".into(),
        Ret::ScalarU64 => "u64".into(),
        Ret::ScalarF64 => "f64".into(),
        Ret::ScalarI32 => "i32".into(),
        Ret::ScalarU32 => "u32".into(),
        Ret::ScalarF32 => "f32".into(),
        Ret::Range => "range".into(),
        Ret::Rect => "rect".into(),
        Ret::Point => "point".into(),
        Ret::Size => "size".into(),
        Ret::Struct(name) => name.clone(),
        Ret::ValueArray | Ret::TextArray | Ret::ObjectArray(_) | Ret::TextMap(_) | Ret::Unsupported(_) => return None,
    };
    let mut tags: Vec<String> = vec![ret_tag];
    for a in args {
        tags.push(arg_tag(a)?);
    }
    Some(tags.join("_"))
}

fn arg_tag(a: &Arg) -> Option<String> {
    Some(match a {
        Arg::Id(_) => "id".into(),
        Arg::Bool(_) => "i8".into(),
        Arg::ScalarI64(_) => "i64".into(),
        Arg::ScalarU64(_) => "u64".into(),
        Arg::ScalarF64(_) => "f64".into(),
        Arg::ScalarI32(_) => "i32".into(),
        Arg::ScalarU32(_) => "u32".into(),
        Arg::ScalarF32(_) => "f32".into(),
        Arg::Range(_) => "range".into(),
        Arg::Rect(_) => "rect".into(),
        Arg::Point(_) => "point".into(),
        Arg::Size(_) => "size".into(),
        Arg::Struct(name, _) => name.clone(),
        Arg::Unsupported(_) => return None,
    })
}

/// True if a shape involves a by-value struct (a PascalCase tag) — those use a
/// module-local `objc_msg_*` shim, not the shared rt:: zoo.
fn msg_shape_has_struct(suffix: &str) -> bool {
    suffix.split('_').any(|t| t.starts_with(|c: char| c.is_ascii_uppercase()))
}

/// A shape tag -> its C+ type, for generating a shim's extern signature. A
/// PascalCase tag is a by-value struct name (used verbatim).
fn tag_to_type(tag: &str) -> String {
    match tag {
        "id" => "*u8",
        "i64" => "i64",
        "u64" => "u64",
        "i8" => "i8",
        "i32" => "i32",
        "u32" => "u32",
        "f32" => "f32",
        "f64" => "f64",
        "range" => "rt::Range",
        "rect" => "rt::Rect",
        "point" => "rt::Point",
        "size" => "rt::Size",
        other => other, // a by-value struct name (PascalCase)
    }
    .to_string()
}

/// The wire expression for an argument (raw int / bridged NSString / `bool as i8`).
fn arg_expr(a: &Arg) -> Option<String> {
    Some(match a {
        Arg::Bool(e) => format!("{e} as i8"),
        Arg::Id(e)
        | Arg::ScalarI64(e)
        | Arg::ScalarU64(e)
        | Arg::ScalarF64(e)
        | Arg::ScalarI32(e)
        | Arg::ScalarU32(e)
        | Arg::ScalarF32(e)
        | Arg::Range(e)
        | Arg::Rect(e)
        | Arg::Point(e)
        | Arg::Size(e)
        | Arg::Struct(_, e) => e.clone(),
        Arg::Unsupported(_) => return None,
    })
}

/// The typed `objc_msgSend` shims the runtime provides (vendor/objc/src/runtime.cplus).
/// Grow the two in lockstep.
fn msg_shape_is_known(suffix: &str) -> bool {
    const KNOWN: &[&str] = &[
        "void", "void_id", "void_i8", "void_i64", "void_f64", "void_range_id", "id",
        "id_id", "id_i64", "id_u64", "id_f64", "id_id_u64", "id_range",
        "i8", "i8_i64", "i64", "u64", "f64", "range", "range_u64", "range_range",
        "void_id_id_i8",
        // 32-bit scalars (int / unsigned / float)
        "i32", "u32", "f32", "void_i32", "void_u32", "void_f32",
        "id_i32", "id_u32", "id_f32",
        // Metal setter / encoder / resource surface (integer-eightbyte + Range + f32).
        "void_u64", "void_u64_u64", "void_u64_u64_u64", "void_i64_u64", "u64_i64",
        "id_id_id", "id_id_id_id", "i8_id_id", "void_id_id",
        "void_id_u64", "void_id_u64_u64", "void_id_u64_u64_u64", "void_id_u64_i8",
        "void_range", "void_id_range", "void_id_id_range", "void_id_id_id_range",
        "void_f32_f32_f32", "void_id_f32_f32_u64",
        // Geometry (NSRect/NSPoint/NSSize) — arm64 HFA, passes in v-regs.
        "rect", "void_rect", "id_rect",
        "rect_rect", "rect_rect_id", "rect_rect_u64",
        "void_rect_i8", "void_rect_i8_i8",
        "id_rect_u64_i64_i8", "f64_rect",
        "size", "void_size",
        "point", "void_point", "point_point",
    ];
    KNOWN.contains(&suffix)
}

/// clang's `loc` puts the file directly, or (for macro-expanded decls like
/// NS_ENUM) under `expansionLoc`/`spellingLoc`. Prefer the expansion site.
pub(crate) fn loc_file(loc: &serde_json::Value) -> Option<String> {
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
    // Strip leading annotation tokens that don't affect the C+ type.
    // `NS_REFINED_FOR_SWIFT` leaks on properties; `__kindof` is an ObjC
    // subclass qualifier (`__kindof NSApplication *` means "NSApplication or
    // a subclass") — both collapse to the plain type for binding purposes.
    for prefix in &["NS_REFINED_FOR_SWIFT ", "__kindof "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim();
        }
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
        let mut e = emitter();
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
            .send_expr("cls", "withCount:", &Ret::Object(None), &[Arg::ScalarU32("count".into())])
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
        // ObjC `type`/`for` param/method, or a C `opaque` field: legal there,
        // reserved here. One shared escaper (crate::sanitize_ident).
        assert_eq!(crate::sanitize_ident("type"), "type_");
        assert_eq!(crate::sanitize_ident("for"), "for_");
        assert_eq!(crate::sanitize_ident("opaque"), "opaque_");
        // The full lexer keyword set must be covered, not just a sample:
        // `convertFont:toHaveTrait:` -> param `trait` is the one that bit AppKit.
        assert_eq!(crate::sanitize_ident("trait"), "trait_");
        assert_eq!(crate::sanitize_ident("union"), "union_");
        assert_eq!(crate::sanitize_ident("interface"), "interface_");
        assert_eq!(crate::sanitize_ident("assert"), "assert_");
        // A leading digit (an enum constant `...10_0` stripped to `10_0`) is not
        // a valid identifier start; prefix `_`.
        assert_eq!(crate::sanitize_ident("10_0"), "_10_0");
        // Non-keywords pass through untouched.
        assert_eq!(crate::sanitize_ident("language"), "language");
        assert_eq!(crate::sanitize_ident("token_range"), "token_range");
    }

    #[test]
    fn enum_constant_starting_with_a_digit_is_escaped() {
        // NS_ENUM `NSDateFormatterBehavior` has `...Behavior10_0` / `...10_4`;
        // stripping the common prefix leaves `10_0`, which can't open an
        // identifier. (Foundation's NSDateFormatter surfaced this.)
        let mut e = emitter();
        let decl = serde_json::json!({
            "kind": "EnumDecl",
            "name": "NSDateFormatterBehavior",
            "inner": [
                { "kind": "EnumConstantDecl", "name": "NSDateFormatterBehaviorDefault" },
                { "kind": "EnumConstantDecl", "name": "NSDateFormatterBehavior10_0" },
                { "kind": "EnumConstantDecl", "name": "NSDateFormatterBehavior10_4" },
            ],
        });
        e.collect_enum(&decl);
        let info = e.enums.get("NSDateFormatterBehavior").cloned().expect("enum collected");
        let names: Vec<&str> = info.variants.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Default", "_10_0", "_10_4"]);
        // Rendered body is spellable: `_10_0,`, never a bare `10_0,`.
        let rendered = e.render_enum(&info);
        assert!(rendered.contains("_10_0,"), "{rendered}");
        assert!(!rendered.contains("    10_0,"), "{rendered}");
    }

    #[test]
    fn delegate_callback_named_type_escapes_to_a_valid_field() {
        // A delegate-protocol callback `type` becomes a synthesized struct field +
        // accessor; it must not land on the `type` keyword.
        let tu = serde_json::json!({
            "inner": [{
                "kind": "ObjCProtocolDecl",
                "name": "MTLThingDelegate",
                "loc": { "file": "test.h" },
                "inner": [{
                    "kind": "ObjCMethodDecl",
                    "name": "type",
                    "instance": true,
                    "loc": { "file": "test.h" },
                    "returnType": { "qualType": "NSUInteger" },
                    "inner": []
                }]
            }]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("type_: fn"), "field escaped:\n{out}");
        assert!(out.contains("fn set_type_(ref this"), "setter escaped:\n{out}");
        assert!(!out.contains("\n    type: fn"), "no bare keyword field:\n{out}");
    }

    #[test]
    fn non_delegate_protocol_becomes_a_callable_wrapper() {
        // A non-delegate protocol is an object the user CALLS (the whole Metal API
        // surface), so it emits an opaque-handle wrapper struct with one method per
        // protocol method — NOT a synthesized delegate. A method named `type`
        // (Metal's MTL4CounterHeap.type getter) must escape to `fn type_(`.
        let tu = serde_json::json!({
            "inner": [{
                "kind": "ObjCProtocolDecl",
                "name": "MTLBuffer",
                "loc": { "file": "test.h" },
                "inner": [
                    { "kind": "ObjCMethodDecl", "name": "length", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "NSUInteger" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "type", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "NSUInteger" }, "inner": [] }
                ]
            }]
        });
        // `MTL` prefix is stripped by cplus_type_name -> `Buffer`.
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("struct Buffer {\n    opaque _obj: *u8,\n}"), "opaque wrapper:\n{out}");
        assert!(out.contains("fn from_raw(ptr: *u8) -> Buffer"), "from_raw:\n{out}");
        assert!(out.contains("fn length(this)"), "length method:\n{out}");
        assert!(out.contains("fn type_(this)"), "keyword-escaped method:\n{out}");
        // A callable wrapper, never a delegate-synthesis helper.
        assert!(!out.contains("create_Buffer"), "not a delegate:\n{out}");
    }

    #[test]
    fn forward_protocol_decl_does_not_shadow_the_full_api() {
        // clang emits a forward `@protocol X;` (empty) alongside the real one.
        // Without dedup, the empty decl claims `X` and the disambiguator demotes
        // the real 72-fn API to `XProtocol` (the Metal `Device` stub-inversion bug).
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "length", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "NSUInteger" }, "inner": [] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn length(this)"), "full API lost:\n{out}");
        assert!(!out.contains("ThingProtocol"), "forward decl shadowed the real API:\n{out}");
        assert_eq!(out.matches("struct Thing {").count(), 1, "duplicate struct:\n{out}");
    }

    #[test]
    fn object_returns_and_args_are_typed_when_the_wrapper_is_local() {
        // A return/arg of a type defined in THIS module is typed to its wrapper;
        // a foreign type (no local def) and a pointer-to-pointer out-param degrade
        // to *u8. Buffer is forward-declared (a stub) so its NAME is known.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCProtocolDecl", "name": "MTLBuffer", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCProtocolDecl", "name": "MTLDevice", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "newBuffer", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "id<MTLBuffer> _Nullable" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "useBuffer:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "buffer", "type": { "qualType": "id<MTLBuffer>" } }] },
                    { "kind": "ObjCMethodDecl", "name": "sourceURL", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSURL * _Nullable" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "loadWithError:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "error", "type": { "qualType": "NSError * _Nullable * _Nullable" } }] },
                ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        // Local wrapper -> typed return + from_raw wrap.
        assert!(out.contains("fn new_buffer(this) -> option::Option[Buffer]"), "typed return:\n{out}");
        assert!(out.contains("Buffer::from_raw(obj)"), "from_raw wrap:\n{out}");
        // Local wrapper arg -> typed param + .raw() on the wire.
        assert!(out.contains("fn use_buffer(this, buffer: Buffer)"), "typed arg:\n{out}");
        assert!(out.contains("buffer.raw()"), "arg unwrapped via .raw():\n{out}");
        // Foreign type (no local def) stays raw.
        assert!(out.contains("fn source_u_r_l(this) -> option::Option[*u8]"), "foreign stays raw:\n{out}");
        // Pointer-to-pointer out-param stays raw.
        assert!(out.contains("error: *u8"), "NSError** out-param stays raw:\n{out}");
    }

    #[test]
    fn by_value_struct_emits_repr_c_and_a_local_msgsend_shim() {
        // `typedef struct {…} MTLSize;` used by value -> a #[repr(C)] struct plus a
        // module-local objc_msgSend shim (PascalCase tag, never the rt:: zoo).
        let tu = serde_json::json!({
            "inner": [
                { "kind": "RecordDecl", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "FieldDecl", "name": "width", "type": { "qualType": "NSUInteger" } },
                    { "kind": "FieldDecl", "name": "height", "type": { "qualType": "NSUInteger" } },
                    { "kind": "FieldDecl", "name": "depth", "type": { "qualType": "NSUInteger" } } ] },
                { "kind": "TypedefDecl", "name": "MTLSize", "loc": { "file": "test.h" },
                  "type": { "qualType": "struct MTLSize" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "size", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "MTLSize" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "setSize:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "size", "type": { "qualType": "MTLSize" } }] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("#[repr(C)]\nstruct Size {\n    width: u64,\n    height: u64,\n    depth: u64,\n}"), "repr(C):\n{out}");
        assert!(out.contains("fn size(this) -> Size"), "sret return:\n{out}");
        assert!(out.contains("fn set_size(this, size: Size)"), "by-value arg:\n{out}");
        // Module-local shims (objc_msg_*, not rt::msg_*), with the by-value struct typed.
        assert!(out.contains("extern fn objc_msg_Size(recv: *u8, sel: *u8) -> Size;"), "sret shim:\n{out}");
        assert!(out.contains("extern fn objc_msg_void_Size(recv: *u8, sel: *u8, a0: Size);"), "arg shim:\n{out}");
        assert!(out.contains("objc_msg_void_Size(this._obj"), "uses local shim:\n{out}");
    }

    #[test]
    fn nsarray_of_protocol_becomes_a_typed_vec() {
        // NSArray<id<P>> (P a non-owning protocol) -> Vec[P], wrapping each element.
        // A class-element array stays skipped (its wrapper may be owning -> a Vec
        // of owning wrappers would over-release +0-borrowed elements).
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCProtocolDecl", "name": "MTLBuffer", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCProtocolDecl", "name": "MTLDevice", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "buffers", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSArray<id<MTLBuffer>> * _Nonnull" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "descriptors", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSArray<MTLThing *> * _Nonnull" }, "inner": [] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn buffers(this) -> vec::Vec[Buffer]"), "typed vec:\n{out}");
        assert!(out.contains("out.append(Buffer::from_raw("), "element wrap:\n{out}");
        // Class-element array isn't typed (MTLThing wrapper could be owning) -> skipped.
        assert!(out.contains("// SKIPPED `descriptors`"), "class array not skipped:\n{out}");
    }

    #[test]
    fn colliding_selectors_emit_one_method_then_skip() {
        // `-open` and `-open:` both map to `open`; C+ has no overloading, so the
        // first wins and the second is SKIPPED. (AppKit's NSDrawer surfaced this,
        // along with a cascade `undefined name sender` from the rejected dup.)
        let tu = serde_json::json!({
            "inner": [{
                "kind": "ObjCInterfaceDecl",
                "name": "NSThing",
                "loc": { "file": "test.h" },
                "inner": [
                    { "kind": "ObjCMethodDecl", "name": "open", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "void" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "open:", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "sender", "type": { "qualType": "id" } }] }
                ]
            }]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        assert_eq!(out.matches("    fn open(").count(), 1, "exactly one `open`:\n{out}");
        assert!(out.contains("// SKIPPED `open:`: method `open` already defined"), "{out}");
    }

    #[test]
    fn class_and_protocol_sharing_a_name_do_not_collide() {
        // NSTextAttachmentCell is both a class and a protocol; the protocol's
        // synthesis helper must not redefine the class's struct.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCInterfaceDecl", "name": "NSThing", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCProtocolDecl", "name": "NSThing", "loc": { "file": "test.h" },
                  "inner": [{ "kind": "ObjCMethodDecl", "name": "tick", "instance": true,
                    "loc": { "file": "test.h" }, "returnType": { "qualType": "NSUInteger" }, "inner": [] }] }
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        assert_eq!(out.matches("struct Thing {").count(), 1, "one class struct:\n{out}");
        assert!(out.contains("struct ThingProtocol {"), "protocol renamed:\n{out}");
    }
}
