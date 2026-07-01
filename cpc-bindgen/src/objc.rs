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
    // Wrapper names whose handle is NON-OWNING (`opaque`, no `drop`): every
    // non-delegate protocol (Metal objects are +0-borrowed) plus every interface
    // with no `init` (factory/singleton classes). An object array — `NSArray<id<P>>`
    // or `NSArray<Foo *>` — bridges to `Vec[W]` only for these: a Vec of *owning*
    // wrappers would over-release the +0-borrowed array elements when it drops.
    non_owning_types: HashSet<String>,
    // Wrapper names that are OWNING (interfaces with an `init`: `drop` releases the
    // +1). An object-array RETURN of an owning element still binds to `Vec[W]`, but
    // each element is `retain`ed on wrap so the +1 balances the wrapper's `drop`.
    // (Owning array *params* stay skipped: iterating the Vec needs `at`, which is
    // `T: Copy`-bound, and owning wrappers are non-Copy.)
    owning_types: HashSet<String>,
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
    // C+ scalar of the enum's fixed underlying type (NS_ENUM(uint8_t, ...) -> "u8"),
    // for laying out a value struct with an enum field. Empty if unknown.
    underlying: String,
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
    IdArray { elem: String, pname: String }, // NSArray<id<P>> param built from Vec[P]
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
            non_owning_types: HashSet::new(),
            owning_types: HashSet::new(),
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
                let ty = self.cplus_type_name(n);
                self.known_types.insert(ty.clone());
                // A class with no `init`/`initWith*` is a factory/singleton: its
                // handle is `opaque` (non-owning, no drop, same as a protocol), so
                // an `NSArray<Foo *>` of it bridges to `Vec[W]` safely. Owned
                // classes (have init) would over-release the +0-borrowed array
                // elements, so they stay out and their arrays keep SKIPPING.
                // Computed here (Pass 2a) so it's complete before any body emits.
                let cats: &[serde_json::Value] =
                    categories.get(n).map(|v| v.as_slice()).unwrap_or(&[]);
                if interface_is_owned(itf, cats) {
                    // Owning: bindable as an array RETURN element (retain-on-wrap).
                    self.owning_types.insert(ty);
                } else {
                    self.non_owning_types.insert(ty);
                }
            }
        }
        for proto in &deduped_protocols {
            let pname = proto.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !(pname.ends_with("Delegate") || pname.ends_with("DataSource")) {
                let ty = self.cplus_type_name(pname);
                self.known_types.insert(ty.clone());
                self.non_owning_types.insert(ty);
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

    /// Discriminator prepended to module-local `objc_msg_*` value-struct shim
    /// names. `extern fn`s share one global bare-name namespace (they bind to a
    /// literal C symbol; resolver Slice 10.FFI.1 leaves them un-qualified), and
    /// same-named externs are deduped keeping the first declaration. In
    /// per-header framework mode a by-value struct (`NSEdgeInsets`) recurs across
    /// many modules, each emitting its own `objc_msg_EdgeInsets` shim returning
    /// *its* local struct — identical names, different signatures. Without a
    /// per-module discriminator the dedup binds later modules' calls to the first
    /// module's signature, so the return type mismatches its own struct (E0302).
    /// `--merge` emits the whole framework as one module: no recurrence, no
    /// collision, so the bare prefix is kept and merged output stays byte-stable.
    fn struct_shim_prefix(&self) -> String {
        if self.merge {
            return "objc_msg_".to_string();
        }
        let b = base(&self.header_path);
        let stem = b.strip_suffix(".h").unwrap_or(&b);
        let stripped = stem.strip_prefix(self.prefix.as_str()).unwrap_or(stem);
        let chosen = if stripped.starts_with(|c: char| c.is_ascii_digit()) { stem } else { stripped };
        format!("objc_msg_{}_", crate::sanitize_ident(&snake(chosen)))
    }

    /// Module-local `objc_msgSend` shims for the by-value-struct call shapes used.
    /// The struct types ride registers/indirect per the platform ABI (verified
    /// against clang for the 24-byte case); cpc lowers the by-value extern the same.
    fn render_struct_shims(&self) -> String {
        let mut shapes: Vec<&String> = self.used_struct_shapes.iter().collect();
        shapes.sort();
        let shim_prefix = self.struct_shim_prefix();
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
                "#[link_name = \"objc_msgSend\"]\nextern fn {shim_prefix}{suffix}({params}){ret};\n"
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
        // The fixed underlying integer type (NS_ENUM(uint8_t, ...)), for laying out a
        // value struct with an enum field. Prefer the desugared (canonical) spelling.
        let underlying = decl
            .get("fixedUnderlyingType")
            .and_then(|t| {
                t.get("desugaredQualType")
                    .or_else(|| t.get("qualType"))
                    .and_then(|v| v.as_str())
            })
            .and_then(c_scalar_to_cplus)
            .unwrap_or("")
            .to_string();
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
                underlying,
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
            if let Some(m) = c_scalar_to_cplus(&cur) {
                return Some(m.to_string());
            }
            if self.value_structs.contains_key(&cur) {
                return Some(self.cplus_type_name(&cur));
            }
            // An enum-typed field lays out at its fixed underlying integer width
            // (NS_ENUM(uint8_t, MTLTextureSwizzle) -> u8). Unknown width => None, so
            // the containing struct isn't recorded rather than risk a wrong layout.
            if let Some(info) = self.enums.get(&cur) {
                return (!info.underlying.is_empty()).then(|| info.underlying.clone());
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
        // Shared with Pass 2a's `non_owning_types` registration so the two never
        // drift: a type emitted `opaque` (no drop) here must be the one Pass 2a
        // judged safe to put in a `Vec`.
        let owned = interface_is_owned(itf, categories);

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

        // Every `init`/`initWith*` becomes a named constructor. The primary — the
        // first init in AST order, exactly as before — keeps the plain `new` name so
        // existing `Type::new(...)` bindings are byte-stable; each further variant is
        // `new_with_<selector>` (`initWithCoder:` -> `new_with_coder`). Was: only the
        // primary bound, every other init skipped as "extra init variant".
        let primary_init: Option<String> = methods
            .iter()
            .filter_map(|m| m.get("name").and_then(|v| v.as_str()))
            .find(|s| *s == "init" || s.starts_with("initWith"))
            .map(String::from);
        for m in &methods {
            let sel = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let is_init = sel == "init" || sel.starts_with("initWith");
            let ctor: Option<String> = if is_init {
                if primary_init.as_deref() == Some(sel.as_str()) {
                    Some("new".to_string())
                } else {
                    Some(ctor_name(&sel))
                }
            } else {
                None
            };
            self.emit_method(m, &objc_name, &ty, is_init, owned, ctor.as_deref());
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
            self.emit_method(m, &objc_name, &ty, false, false, None);
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

    fn emit_method(&mut self, m: &serde_json::Value, objc_class: &str, ty: &str, is_init: bool, owned: bool, ctor: Option<&str>) {
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

        // A block parameter -> dedicated block-literal emission. Handles both an
        // inline `(^)(...)` type (`usingBlock:`) and a typedef alias to one
        // (completion handlers: `typedef void (^MTLNewLibraryCompletionHandler)(...)`),
        // resolving the alias to the underlying block signature for parse_block_args.
        if let Some((bidx, block_qt)) = params
            .iter()
            .enumerate()
            .find_map(|(i, (_, qt))| self.resolve_block_type(qt).map(|bt| (i, bt)))
        {
            self.emit_block_method(objc_class, &ty, &sel, is_instance, ret_qt, &params, bidx, &block_qt, &ov_name, &ov_params);
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
        let has_id_array = args.iter().any(|a| matches!(a, Arg::IdArray { .. }));
        if is_init {
            // The Vec[P]->NSArray prologue is only wired into the general path; an
            // init taking one is rare — skip rather than emit a dangling local.
            if has_id_array {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: init with an NSArray<id> param not modelled\n"));
                return;
            }
            let send = self.send_expr("alloced", &sel, &Ret::Object(None), &args);
            let send = match send {
                Some(s) => s,
                None => {
                    self.body.push_str(&format!("    // SKIPPED `{sel}`: init arg shape not yet modelled\n"));
                    return;
                }
            };
            let ctor = ctor.unwrap_or("new");
            if !self.seen_methods.insert(ctor.to_string()) {
                self.body.push_str(&format!("    // SKIPPED `{sel}`: `{ctor}` already defined (extra constructor)\n"));
                return;
            }
            let header = if sig_param.is_empty() { String::new() } else { sig_param.clone() };
            self.body.push_str(&format!(
                "    fn {ctor}({header}) -> {ty} {{\n        let cls: *u8 = rt::get_class(#str_ptr(\"{objc_class}\\0\"));\n        let alloced: *u8 = rt::msg_id(cls, rt::sel(#str_ptr(\"alloc\\0\")));\n        return {ty} {{ _obj: {send} }};\n    }}\n\n"
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
            && !has_id_array // the prologue is built only in the general path
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

        // The `Vec[W] -> NSMutableArray` prologue (the `arr_<pname>` locals) is
        // built only in the general scalar/object path below. The collection-return
        // paths build their own multi-statement bodies and would reference an
        // undefined `arr_<pname>`, so skip an NSArray<id> param combined with a
        // collection return rather than emit dangling code.
        if has_id_array
            && matches!(ret, Ret::ValueArray | Ret::TextArray | Ret::ObjectArray(_) | Ret::TextMap(_))
        {
            self.body.push_str(&format!(
                "    // SKIPPED `{sel}`: NSArray<id> param with a collection return not modelled\n"
            ));
            return;
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
            // Array elements come back +0 (the array owns them). An owning wrapper's
            // `drop` releases, so retain each on wrap to balance it; a non-owning
            // wrapper has no `drop`, so wrap the +0 borrow directly.
            let elem_handle = if self.owning_types.contains(elem_ty) {
                "rt::retain(rt::msg_id_u64(arr, at_sel, i))".to_string()
            } else {
                "rt::msg_id_u64(arr, at_sel, i)".to_string()
            };
            self.body.push_str(&format!(
                "    fn {name}({receiver}{sep}{sig_param}) -> vec::Vec[{elem_ty}] {{\n\
                 \x20       let arr: *u8 = {array_call};\n\
                 \x20       let n: u64 = rt::msg_u64(arr, rt::sel(#str_ptr(\"count\\0\")));\n\
                 \x20       var out: vec::Vec[{elem_ty}] = vec::Vec[{elem_ty}]::with_capacity(n as usize);\n\
                 \x20       let at_sel: *u8 = rt::sel(#str_ptr(\"objectAtIndex:\\0\"));\n\
                 \x20       var i: u64 = 0 as u64;\n\
                 \x20       while i < n {{\n\
                 \x20           out.append({elem_ty}::from_raw({elem_handle}));\n\
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
        let (ret_spelling, body_line) = self
            .return_spelling(&ret, &send)
            .expect("collection/unsupported returns are handled earlier in emit_method");

        // Prologue: build an NSMutableArray from each Vec[P] param (the send call
        // already references the `arr_<pname>` local via arg_expr).
        let mut prologue = String::new();
        for a in &args {
            if let Arg::IdArray { elem, pname } = a {
                prologue.push_str(&format!(
                    "        let arr_{pname}: *u8 = rt::msg_id(rt::get_class(#str_ptr(\"NSMutableArray\\0\")), rt::sel(#str_ptr(\"array\\0\")));\n\
                     \x20       let add_sel_{pname}: *u8 = rt::sel(#str_ptr(\"addObject:\\0\"));\n\
                     \x20       let n_{pname}: usize = {pname}.count();\n\
                     \x20       var i_{pname}: usize = 0 as usize;\n\
                     \x20       while i_{pname} < n_{pname} {{\n\
                     \x20           match {pname}.at_ptr(i_{pname}) {{\n\
                     \x20               option::Option[*{elem}]::Some(e_{pname}) => {{ rt::msg_void_id(arr_{pname}, add_sel_{pname}, (*e_{pname}).raw()); }}\n\
                     \x20               option::Option[*{elem}]::None => {{}}\n\
                     \x20           }}\n\
                     \x20           i_{pname} = i_{pname} +% (1 as usize);\n\
                     \x20       }}\n"
                ));
            }
        }

        self.body.push_str(&format!("    fn {name}({receiver}{sep}{sig_param}){ret_spelling} {{\n{prologue}{body_line}    }}\n\n"));
    }

    /// A method with a trailing `usingBlock:` param: emit a per-method
    /// Block_literal struct + `invoke` trampoline (into `block_helpers`) and a
    /// wrapper taking a C+ `fn(*u8, ...block args)` + `*u8` ctx.
    /// The `-> T` return spelling and the body line(s) that wrap a raw `send`
    /// expression per the return classification. `None` for the multi-statement
    /// collection returns (ValueArray/TextArray/ObjectArray/TextMap) and Unsupported,
    /// which callers handle (or skip) separately. Shared by emit_method and
    /// emit_block_method so a block method's non-void return wraps identically.
    fn return_spelling(&self, ret: &Ret, send: &str) -> Option<(String, String)> {
        Some(match ret {
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
            Ret::ValueArray | Ret::TextArray | Ret::ObjectArray(_) | Ret::TextMap(_) | Ret::Unsupported(_) => return None,
        })
    }

    fn emit_block_method(
        &mut self,
        objc_class: &str,
        ty: &str,
        sel: &str,
        is_instance: bool,
        ret_qt: &str,
        params: &[(String, String)],
        bidx: usize,
        block_qt: &str,
        ov_name: &Option<String>,
        ov_params: &[String],
    ) {
        let ret = self.map_ret(ret_qt);
        if let Ret::Unsupported(why) = &ret {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: block method return `{ret_qt}` — {why}\n"));
            return;
        }
        // Collection returns need the multi-statement Vec/Map builders that only the
        // general path emits; a block method returning one is rare — skip rather than
        // emit a partial body.
        if matches!(ret, Ret::ValueArray | Ret::TextArray | Ret::ObjectArray(_) | Ret::TextMap(_)) {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: block method with a collection return not modelled\n"));
            return;
        }
        if bidx != params.len() - 1 {
            self.body.push_str(&format!("    // SKIPPED `{sel}`: params after the block not modelled\n"));
            return;
        }
        // `block_qt` is the underlying `RET (^)(...)` type — resolved through a
        // typedef alias by the caller for completion-handler params.
        let block_args = match self.parse_block_args(block_qt) {
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
        // The block path doesn't build the `Vec[W] -> NSMutableArray` prologue, so
        // an NSArray<id> leading param would reference an undefined `arr_<pname>`.
        if send_args.iter().any(|a| matches!(a, Arg::IdArray { .. })) {
            self.body.push_str(&format!(
                "    // SKIPPED `{sel}`: block method with an NSArray<id> param not modelled\n"
            ));
            return;
        }
        send_args.push(Arg::Id("bp".to_string()));

        let recv = if is_instance {
            "this._obj".to_string()
        } else {
            format!("rt::get_class(#str_ptr(\"{objc_class}\\0\"))")
        };
        let send = match self.send_expr(&recv, sel, &ret, &send_args) {
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
        // Same return-wrapping as a plain method. Void keeps its explicit trailing
        // `return;` so existing usingBlock: bindings stay byte-stable; a non-void
        // return (completion-handler factories like newBufferWithBytesNoCopy:...
        // deallocator: -> Option[Buffer]) wraps via return_spelling. The block slot
        // `bp` is set up first, so `body_line`'s embedded send runs after it.
        let (ret_spelling, body_line) = self
            .return_spelling(&ret, &send)
            .expect("block-method collection/unsupported returns handled above");
        let body_line = if matches!(ret, Ret::Void) {
            format!("{body_line}        return;\n")
        } else {
            body_line
        };
        self.body.push_str(&format!(
            "    fn {name}({sig}){ret_spelling} {{\n        var desc: rt::BlockDescriptor = rt::BlockDescriptor {{ reserved: 0 as u64, size: 48 as u64 }};\n        var blk: {struct_name} = {struct_name} {{ isa: rt::stack_block_isa(), flags: 0 as i32, reserved: 0 as i32, invoke: {invoke_name}, descriptor: {{ #addr_of(desc) as *u8 }}, user_fn: cb, ctx: ctx }};\n        let bp: *u8 = {{ #addr_of(blk) as *u8 }};\n{body_line}    }}\n\n"
        ));
    }

    /// The underlying `RET (^)(...)` block type of a param — directly, or through a
    /// typedef alias (completion handlers: `typedef void (^MTLNewLibraryCompletionHandler)(
    /// id<MTLLibrary>, NSError *)`). None if the type is not (and does not alias) a
    /// block, so ordinary params are unaffected. A function-pointer typedef spells
    /// `(*)`, never `(^)`, so it can't be mistaken for a block.
    fn resolve_block_type(&self, qt: &str) -> Option<String> {
        let (base, _) = strip_nullability(qt);
        let mut cur = base.trim().to_string();
        for _ in 0..8 {
            if cur.contains("(^") {
                return Some(cur);
            }
            match self.typedefs.get(&cur) {
                Some(u) => cur = u.trim().to_string(),
                None => return None,
            }
        }
        None
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
        let prefix: String = if msg_shape_has_struct(&suffix) {
            // Record it here (not in the caller) so every path — factory/init/
            // collection helpers included — gets its shim emitted. The shim name
            // is module-discriminated (see `struct_shim_prefix`) so per-header
            // modules sharing a value struct don't collide on one extern name.
            self.used_struct_shapes.insert(suffix.clone());
            self.struct_shim_prefix()
        } else if msg_shape_is_known(&suffix) {
            "rt::msg_".to_string()
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
        // An object array RETURN binds to `Vec[W]` for any module-defined wrapper:
        // non-owning elements (protocol / factory class) wrap +0 via `from_raw`;
        // owning elements (class with init) are `retain`ed on wrap so the +1
        // balances the wrapper's `drop` (the retain is emitted in the codegen).
        if let Some(elem) = array_element(base_ty) {
            if let Some(w) = self.wrapper_name_of(&elem) {
                if self.non_owning_types.contains(&w) || self.owning_types.contains(&w) {
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
            // `SEL` (selector) and `Class` are ObjC pointer types with no C+ wrapper.
            // ABI-identical to a raw pointer; the runtime already trades selectors as
            // `rt::sel(...) -> *u8` and classes as `rt::get_class(...) -> *u8`, so a
            // bare `*u8` handle is the natural, consistent binding.
            "SEL" | "Class" => return Ret::Object(None),
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
        // A by-value C struct (MTLSize) returned by value — through typedef aliases.
        if let Some(vs) = self.value_struct_of(base_ty) {
            self.mark_value_struct_used(&vs);
            return Ret::Struct(self.cplus_type_name(&vs));
        }
        // Last resort: follow the typedef chain to a supported scalar / pointer.
        match self.typedef_canon(base_ty) {
            Some(TdCanon::I64) => return Ret::ScalarI64,
            Some(TdCanon::U64) => return Ret::ScalarU64,
            Some(TdCanon::F64) => return Ret::ScalarF64,
            Some(TdCanon::F32) => return Ret::ScalarF32,
            Some(TdCanon::I32) => return Ret::ScalarI32,
            Some(TdCanon::U32) => return Ret::ScalarU32,
            Some(TdCanon::Ptr) => {
                return if nullable { Ret::ObjectOption(None) } else { Ret::Object(None) }
            }
            None => {}
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
        // Object array param -> build an NSMutableArray from a Vec[W]. Sound for both
        // non-owning AND owning element wrappers: the prologue borrow-reads each
        // element's handle through `at_ptr` (never moving/dropping it — cpc's borrowck
        // proves it), `addObject:` takes its own +1 retain, and the caller's Vec keeps
        // ownership. (The *return* direction is the one that needs retain-on-wrap; a
        // param only reads what the caller already owns.)
        if let Some(elem) = array_element(base_ty) {
            if let Some(w) = self.wrapper_name_of(&elem) {
                if self.non_owning_types.contains(&w) || self.owning_types.contains(&w) {
                    self.needs_vec = true;
                    return Arg::IdArray { elem: w, pname: pname.to_string() };
                }
            }
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
            // `SEL`/`Class`: raw ObjC pointer types (no wrapper). Pass the handle
            // through verbatim — callers supply `rt::sel(...)` / `rt::get_class(...)`,
            // both already `*u8`. Mirrors the `id` untyped-handle arg.
            "id" | "SEL" | "Class" => return Arg::Id(pname.to_string()),
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
        // A by-value C struct (MTLSize) passed by value — through typedef aliases.
        if let Some(vs) = self.value_struct_of(base_ty) {
            self.mark_value_struct_used(&vs);
            return Arg::Struct(self.cplus_type_name(&vs), pname.to_string());
        }
        // Last resort: follow the typedef chain to a supported scalar / pointer.
        // Must mirror map_ret + param_sig_type so the wire type and the public
        // signature never disagree.
        match self.typedef_canon(base_ty) {
            Some(TdCanon::I64) => return Arg::ScalarI64(pname.to_string()),
            Some(TdCanon::U64) => return Arg::ScalarU64(pname.to_string()),
            Some(TdCanon::F64) => return Arg::ScalarF64(pname.to_string()),
            Some(TdCanon::F32) => return Arg::ScalarF32(pname.to_string()),
            Some(TdCanon::I32) => return Arg::ScalarI32(pname.to_string()),
            Some(TdCanon::U32) => return Arg::ScalarU32(pname.to_string()),
            Some(TdCanon::Ptr) => return Arg::Id(pname.to_string()),
            None => {}
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
        // Object array param -> Vec[W], non-owning or owning element (same gate as map_arg).
        if let Some(elem) = array_element(b) {
            if let Some(w) = self.wrapper_name_of(&elem) {
                if self.non_owning_types.contains(&w) || self.owning_types.contains(&w) {
                    return format!("vec::Vec[{w}]");
                }
            }
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
            // Follows typedef aliases (MTLCoordinate2D -> MTLSamplePosition).
            _ if self.value_struct_of(b).is_some() => {
                self.cplus_type_name(&self.value_struct_of(b).unwrap())
            }
            // Last resort mirrors map_arg's typedef fallback so the public signature
            // matches the wire type. Pointer typedefs (and unknown leaves) stay `*u8`.
            _ => match self.typedef_canon(b) {
                Some(TdCanon::I64) => "i64".to_string(),
                Some(TdCanon::U64) => "u64".to_string(),
                Some(TdCanon::F64) => "f64".to_string(),
                Some(TdCanon::F32) => "f32".to_string(),
                Some(TdCanon::I32) => "i32".to_string(),
                Some(TdCanon::U32) => "u32".to_string(),
                Some(TdCanon::Ptr) | None => "*u8".to_string(),
            },
        }
    }

    /// Last-resort mapping for an otherwise-unmapped type: follow the Pass-1
    /// typedef chain until it bottoms out in a scalar the rt:: shim zoo supports,
    /// or any pointer (→ untyped `*u8` handle, like bare `id`). Returns None for
    /// 8/16-bit scalars, function pointers, and unknown leaves — those stay
    /// skipped rather than risk a wrong ABI width. Following the *declared*
    /// underlying type (not a hardcoded name→width guess) keeps it C-ABI-correct
    /// by construction: `NSLayoutPriority`→float→f32, `NSModalResponse`→NSInteger→i64,
    /// `dispatch_queue_t`→`… *`→handle. Used as the tail of map_ret/map_arg/param_sig_type
    /// so those three stay in lockstep.
    fn typedef_canon(&self, base_ty: &str) -> Option<TdCanon> {
        let mut cur = base_ty.trim().to_string();
        for _ in 0..8 {
            let c = cur.trim().trim_start_matches("const ").trim();
            match c {
                "NSInteger" | "long" | "long long" | "int64_t" | "ptrdiff_t" => return Some(TdCanon::I64),
                "NSUInteger" | "unsigned long" | "unsigned long long" | "uint64_t" | "size_t" => {
                    return Some(TdCanon::U64)
                }
                "double" | "CGFloat" | "NSTimeInterval" | "CFTimeInterval" => return Some(TdCanon::F64),
                "float" => return Some(TdCanon::F32),
                "int" | "int32_t" => return Some(TdCanon::I32),
                "unsigned int" | "unsigned" | "uint32_t" => return Some(TdCanon::U32),
                _ => {}
            }
            if c.ends_with('*') {
                return Some(TdCanon::Ptr);
            }
            match self.typedefs.get(c) {
                Some(u) => cur = u.trim().to_string(),
                None => return None,
            }
        }
        None
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

    /// The registered by-value struct a type resolves to, following typedef
    /// aliases (`MTLCoordinate2D` -> `MTLSamplePosition`). Mirrors `enum_of`:
    /// without it a `typedef Struct Alias;` param/return skips as an unmapped
    /// type even though the layout is a known repr(C) struct.
    fn value_struct_of(&self, ty: &str) -> Option<String> {
        let mut cur = ty.to_string();
        for _ in 0..8 {
            if self.value_structs.contains_key(&cur) {
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
/// True if an interface (with its categories) declares an `init` / `initWith*`,
/// i.e. it's an owning class that carries a +1 and releases in `drop`. A class
/// with none is a factory/singleton: non-owning, `opaque`, no drop. Used by both
/// `emit_interface` (to pick the field/drop shape) and Pass 2a (to decide whether
/// the type is safe as a `Vec` element) — one source of truth so they can't drift.
fn interface_is_owned(itf: &serde_json::Value, categories: &[serde_json::Value]) -> bool {
    std::iter::once(itf).chain(categories.iter()).any(|src| {
        src.get("inner").and_then(|v| v.as_array()).into_iter().flatten().any(|m| {
            m.get("kind").and_then(|k| k.as_str()) == Some("ObjCMethodDecl")
                && m.get("name").and_then(|v| v.as_str())
                    .is_some_and(|sel| sel == "init" || sel.starts_with("initWith"))
        })
    })
}

fn array_element(ty: &str) -> Option<String> {
    let t = ty.trim();
    let rest = t.strip_prefix("NSArray<")?;
    let elem = rest.strip_suffix("> *").or_else(|| rest.strip_suffix(">*"))?;
    // `NSArray<__kindof NSView *>` — the element carries the subclass qualifier;
    // strip it so the element resolves to its wrapper (`NSView *` -> View).
    let elem = elem.trim().strip_prefix("__kindof ").unwrap_or(elem.trim());
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
        Arg::IdArray { .. } => "id".into(), // the built NSArray is an id on the wire
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
        // The NSMutableArray built in the method prologue (see emit_method).
        Arg::IdArray { pname, .. } => format!("arr_{pname}"),
        Arg::Unsupported(_) => return None,
    })
}

/// The scalar/geometry `objc_msgSend` shapes the shared rt:: zoo models — every
/// entry MUST have a matching `fn msg_<tag>` wrapper in vendor/objc/src/runtime.cplus
/// (enforced in lockstep by `every_known_shape_has_a_runtime_wrapper`). By-value
/// value-struct shapes (PascalCase tags) are NOT listed here — they route to
/// module-local shims via `msg_shape_has_struct`.
const KNOWN_MSG_SHAPES: &[&str] = &[
        "void", "void_id", "void_i8", "void_i64", "void_f64", "void_range_id", "id",
        "id_id", "id_i64", "id_u64", "id_f64", "id_id_u64", "id_range",
        "i8", "i8_i64", "i8_id", "i64", "u64", "f64", "range", "range_u64", "range_range",
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
        // High-frequency AppKit / Metal selector signatures (integer-eightbyte /
        // f64 / HFA-geometry classes only — see vendor/objc runtime shim block).
        "i64_id", "u64_id", "i64_i64", "f64_i64", "i8_u64",
        "void_id_i64", "void_id_i8", "void_i64_i64", "void_i8_i64",
        "id_id_i8", "id_id_i64", "id_id_f64", "id_i64_i64",
        "i64_id_id", "i8_id_id_id", "id_f64_f64_f64_f64",
        "void_rect_id", "rect_i64", "size_size",
        // Batch: all remaining scalar/geometry shapes used >=3x across
        // metal/appkit. Grown in lockstep with vendor/objc/src/runtime.cplus.
        "void_id_id_id", "void_id_id_id_id", "id_id_id_id_id", "void_id_id_id_id_id", "i64_point", "void_id_point",
        "void_id_id_u64", "void_f64_i64", "u64_u64", "point_id", "id_id_id_i64", "f64_id",
        "void_id_id_id_u64", "rect_i8", "id_point", "id_id_u64_id_id", "id_i8", "i64_u64",
        "f64_f64", "void_u64_id", "void_u64_i8", "void_id_id_id_id_id_id", "void_id_i64_i64", "void_id_f64",
        "void_f32_f32", "id_id_rect_id_id", "id_f64_f64", "i8_rect", "void_point_point", "void_id_rect_id",
        "void_id_range_id_u64", "void_id_i64_id", "void_i64_u64_u64_u64_u64", "void_i64_range", "void_i64_i8", "u64_range_id_id_id_id",
        "u64_point", "u64_id_u64", "u64_id_id", "rect_id", "rect_i64_i64", "range_id_u64",
        "id_u64_u64", "id_id_u64_u64", "id_id_rect_id", "id_id_id_id_id_id", "id_id_f64_f64", "id_i8_id",
        "id_f64_f64_f64_f64_f64", "i8_point", "i8_id_u64", "i8_id_id_i64", "i8_id_i8", "f64_u64_rect",
        // Metal encoder/resource wide-arg shapes (all integer-eightbyte / id / NSRange).
        "i32_u32", "i8_u64_u64", "id_i64_i64_range_range", "id_i64_u64_i8", "id_i64_u64_u64_i8",
        "id_i64_u64_u64_u64", "id_id_i64_id", "id_id_id_u64", "id_id_u64_id", "id_u64_id",
        "id_u64_u64_i64", "id_u64_u64_u64", "u64_id_id_u64", "u64_id_id_u64_u64_u64", "u64_id_range_u64",
        "void_f32_f32_f32_f32", "void_i64_i64_id_u64_id_u64", "void_i64_i64_u64_u64_u64", "void_i64_id_u64", "void_i64_u64_i64_id_u64",
        "void_i64_u64_i64_id_u64_u64", "void_i64_u64_i64_id_u64_u64_i64_u64", "void_i64_u64_i64_u64_u64", "void_i64_u64_i64_u64_u64_u64", "void_i64_u64_i64_u64_u64_u64_i64_u64",
        "void_i64_u64_id_u64", "void_i64_u64_u64", "void_i64_u64_u64_u64", "void_id_id_id_id_u64", "void_id_id_id_id_u64_u64",
        "void_id_id_u64_i64", "void_id_u64_id", "void_id_u64_id_u64", "void_id_u64_id_u64_u64", "void_id_u64_u64_id_u64",
        "void_id_u64_u64_id_u64_u64_u64_u64", "void_u32_u32", "void_u64_id_u64_id_u64", "void_u64_id_u64_id_u64_id_u64", "void_u64_range",
        "void_u64_u64_u64_id_u64_id_u64_u64_u64", "void_u64_u64_u64_id_u64_id_u64_u64_u64_id_u64_u64", "void_u64_u64_u64_id_u64_u64_u64", "void_u64_u64_u64_id_u64_u64_u64_id_u64_u64",
        // Metal shapes surfaced after MTLArgument.h + typedef resolution landed.
        "void_id_i64_u64", "void_id_i64_range", "void_id_i64_id_u64", "void_id_i64_id_id_id_u64",
        // Object-returning block factory (newBufferWithBytesNoCopy:...deallocator:).
        "id_id_u64_u64_id",
        // AppKit-surface shapes (batch: all scalar/geometry shapes appkit uses).
        "f32_i64", "f32_id", "f32_id_id", "f32_u64", "f64_i64_i64",
        "f64_id_id_f64", "f64_id_id_i64", "f64_id_u64", "f64_point", "f64_point_id",
        "f64_size", "f64_u64", "i32_id_id", "i64_i64_i64", "i64_i64_id",
        "i64_i64_u64", "i64_id_i8", "i64_id_id_id", "i64_id_u64", "i64_point_i64",
        "i64_rect_id_id", "i64_rect_id_id_i8", "i8_i8", "i8_id_i64", "i8_id_i64_i64",
        "i8_id_i64_id", "i8_id_i8_i8", "i8_id_i8_id", "i8_id_id_f64_id", "i8_id_id_i64_id",
        "i8_id_id_i64_id_id", "i8_id_id_i8", "i8_id_id_id_i64", "i8_id_id_id_id_id", "i8_id_id_id_id_id_id",
        "i8_id_id_point", "i8_id_id_point_id", "i8_id_id_u64", "i8_id_id_u64_id", "i8_id_point",
        "i8_id_point_id", "i8_id_rect", "i8_id_rect_i8_id", "i8_id_rect_id_i8", "i8_id_rect_id_i8_id",
        "i8_id_rect_id_u64", "i8_id_rect_id_u64_i8", "i8_id_size_i8", "i8_id_u64_id", "i8_id_u64_id_id",
        "i8_point_id", "i8_point_point_id", "i8_point_rect", "i8_range", "i8_range_id",
        "i8_rect_rect", "i8_u64_point_u64_id", "id_f64_f64_f64", "id_f64_f64_f64_id_id", "id_f64_f64_i64",
        "id_f64_i64_f64", "id_f64_i8", "id_f64_id", "id_i64_f32", "id_i64_i64_i8",
        "id_i64_i8", "id_i64_id", "id_i64_point_id", "id_i64_point_u64_f64_i64_id_i64_i64_f32", "id_i64_point_u64_f64_i64_id_i64_i64_id",
        "id_i64_u64", "id_id_f64_f64_f64", "id_id_f64_f64_f64_f64", "id_id_f64_id", "id_id_i64_i64_i8_i8",
        "id_id_i64_i64_id_i64_f64_f64", "id_id_i64_id_id", "id_id_i64_point_id", "id_id_i8_id", "id_id_i8_id_id",
        "id_id_id_f64", "id_id_id_i64_id_id", "id_id_id_i64_id_id_id", "id_id_id_i64_point", "id_id_id_i8",
        "id_id_id_id_i64", "id_id_id_id_i8", "id_id_point_point_point", "id_id_range", "id_id_u64_i64_f64",
        "id_id_u64_id_range", "id_point_rect_id", "id_range_id", "id_range_id_id", "id_range_id_id_i64",
        "id_range_range_id_id", "id_rect_f64_f64", "id_rect_i64", "id_rect_id", "id_rect_id_u64",
        "id_u32_id_id", "id_u64_i64_id", "id_u64_i8_id", "id_u64_id_i8", "id_u64_id_id_i8",
        "id_u64_point", "point_i64", "point_point_id", "point_point_point", "point_rect",
        "point_u64", "range_id", "range_id_i64", "range_id_i64_id_i8_i64_id", "range_range_i64",
        "range_range_id", "range_rect", "range_rect_id", "rect_id_i64", "rect_id_id",
        "rect_id_point_rect_id_range", "rect_id_range", "rect_id_rect_id", "rect_id_rect_point_u64", "rect_id_rect_rect_i64",
        "rect_id_rect_rect_id_range", "rect_id_u64_id", "rect_point_rect", "rect_point_rect_id_range", "rect_range",
        "rect_range_id", "rect_rect_i64_i64_id", "rect_rect_rect_id_range", "rect_rect_u64_i64_id", "rect_size_u64",
        "rect_size_u64_id", "rect_u32", "rect_u64", "rect_u64_id", "rect_u64_id_i8",
        "rect_u64_id_rect_point_u64", "rect_u64_u64", "size_i8", "size_id", "size_id_id",
        "size_id_id_i64", "size_id_id_id", "size_rect", "size_size_i8_i8_i64", "size_size_id",
        "size_size_id_id_i64_i64_i64", "size_u32", "size_u64", "u32_id", "u32_u64",
        "u32_u64_id", "u64_id_i64", "u64_id_range", "u64_id_rect_id", "u64_point_id",
        "u64_point_id_id", "u64_range_id_id_id_id_id", "u64_u64_i8", "u64_u64_i8_i8_id_id", "u64_u64_range",
        "void_f32_f64", "void_f32_i64", "void_f32_id", "void_f64_f64", "void_f64_i64_i64",
        "void_f64_i64_i64_i64", "void_f64_id", "void_f64_id_id_id", "void_f64_point", "void_i64_f32",
        "void_i64_i64_i64", "void_i64_i64_i64_i8", "void_i64_i64_id_i8", "void_i64_i64_u64", "void_i64_id_i64_id",
        "void_i64_id_id_id", "void_i64_id_id_id_i64", "void_i64_rect", "void_i64_rect_id", "void_i8_i64_i64",
        "void_i8_id", "void_i8_range", "void_i8_rect", "void_i8_rect_id", "void_i8_u64",
        "void_i8_u64_u64", "void_id_f32", "void_id_f64_f64_f64", "void_id_i32_i32_i32", "void_id_i64_f64",
        "void_id_i64_i8", "void_id_i64_id_id_id", "void_id_i8_id_id_id", "void_id_id_f64_id", "void_id_id_i64",
        "void_id_id_i64_id_id_id", "void_id_id_id_id_id_id_id", "void_id_id_id_id_range", "void_id_id_range_point", "void_id_id_u64_range",
        "void_id_point_point", "void_id_point_size_id_id_id_i8", "void_id_point_u64", "void_id_range_i64_i8", "void_id_range_id_id",
        "void_id_range_range", "void_id_rect_id_i64_i64", "void_id_rect_id_i64_i64_i8", "void_id_rect_id_range_id", "void_id_rect_u64",
        "void_id_u32", "void_id_u32_i32", "void_id_u32_i32_i32", "void_id_u64_i64", "void_id_u64_id_id",
        "void_id_u64_range_i64_range", "void_id_u64_range_id", "void_id_u64_range_point_id_id_size", "void_point_f64", "void_point_f64_f64_f64",
        "void_point_f64_f64_f64_i8", "void_point_f64_point_f64_u64", "void_point_i64", "void_point_i64_f64", "void_point_id",
        "void_point_point_f64", "void_point_point_id_i8", "void_point_point_point", "void_point_point_u64", "void_point_range",
        "void_point_rect_f64", "void_point_rect_i64", "void_point_rect_i64_f64", "void_range_i64_i8", "void_range_i64_id",
        "void_range_i8_id", "void_range_id_i64_id", "void_range_point", "void_range_range", "void_range_u64_f64_rect_range_point",
        "void_range_u64_rect_range_point", "void_rect_f64", "void_rect_f64_f64", "void_rect_id_i64", "void_rect_id_i8",
        "void_rect_id_i8_i64", "void_rect_id_id_i64_i64", "void_rect_id_id_id", "void_rect_id_id_id_i64_i64", "void_rect_id_id_id_id",
        "void_rect_id_range", "void_rect_id_range_id", "void_rect_id_u64", "void_rect_id_u64_id", "void_rect_point",
        "void_rect_range_rect", "void_rect_rect_i64_f64", "void_rect_rect_id", "void_rect_size", "void_rect_u64",
        "void_rect_u64_id", "void_size_range", "void_u32_id", "void_u32_id_u32", "void_u32_u64_u64",
        "void_u64_point_u64_id", "void_u64_range_i64", "void_u64_u32",
];

/// The typed `objc_msgSend` shims the runtime provides (vendor/objc/src/runtime.cplus).
/// Grow `KNOWN_MSG_SHAPES` and the runtime wrappers in lockstep.
fn msg_shape_is_known(suffix: &str) -> bool {
    KNOWN_MSG_SHAPES.contains(&suffix)
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
    // Strip leading annotation tokens that don't affect the C+ type, repeatedly
    // (they can stack, e.g. `const __kindof`). `NS_REFINED_FOR_SWIFT` leaks on
    // properties; `__kindof` is an ObjC subclass qualifier (`__kindof NSApplication *`
    // = "NSApplication or a subclass"); `const` on a by-value param/return is a
    // no-op for binding purposes — all collapse to the plain type.
    loop {
        let before = s;
        for prefix in &["NS_REFINED_FOR_SWIFT ", "__kindof ", "const "] {
            if let Some(rest) = s.strip_prefix(prefix) {
                s = rest.trim();
            }
        }
        // Availability / Swift-annotation macros that leak into a clang type spelling
        // (`API_AVAILABLE NSArray<NSString *>`, `API_DEPRECATED(...) id<...>`). Strip
        // the macro name + an optional balanced `(...)` group. Longest names first, with
        // a word-boundary check so `API_DEPRECATED` doesn't eat an `_WITH_...` tail.
        for macro_name in &[
            "API_DEPRECATED_WITH_REPLACEMENT",
            "NS_SWIFT_UNAVAILABLE",
            "API_UNAVAILABLE",
            "API_DEPRECATED",
            "API_AVAILABLE",
        ] {
            if let Some(rest) = s.strip_prefix(macro_name) {
                if !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                    let rest = rest.trim_start();
                    // skip_balanced_parens consumes the leading `(` itself — don't strip it first.
                    let rest = if rest.starts_with('(') { skip_balanced_parens(rest) } else { rest };
                    s = rest.trim();
                }
            }
        }
        if s == before {
            break;
        }
    }
    for (suf, nul) in [(" _Nullable", true), (" _Nonnull", false), (" _Null_unspecified", true)] {
        if let Some(stripped) = s.strip_suffix(suf) {
            return (stripped.to_string(), nul);
        }
    }
    (s.to_string(), false)
}

/// A C scalar type spelling -> its fixed-width C+ type, for value-struct field
/// layout. None for anything that isn't a plain integer/float scalar.
fn c_scalar_to_cplus(name: &str) -> Option<&'static str> {
    Some(match name.trim() {
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
        _ => return None,
    })
}

/// Given a slice that starts with `(`, return the slice past the matching `)`.
/// Handles nesting (`API_AVAILABLE(macos(11.0))`); returns the input unchanged if
/// the parens are unbalanced.
fn skip_balanced_parens(s: &str) -> &str {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &s[i + 1..];
                }
            }
            _ => {}
        }
    }
    s
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

/// Canonical ABI kind an otherwise-unmapped typedef bottoms out in (see
/// `ObjcEmitter::typedef_canon`). Only the widths the rt:: shim zoo models —
/// `Ptr` is any pointer typedef, lowered to an untyped `*u8` handle.
enum TdCanon {
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Ptr,
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

/// Constructor name for an `init`/`initWith*` selector: the `init` stem becomes
/// `new`, so bare `init` -> `new` and `initWithFrame:size:` -> `new_with_frame_size`.
/// Derived purely from the selector (order-independent), so a type's constructor
/// set is stable across regenerations.
fn ctor_name(sel: &str) -> String {
    let mech = mechanical_name(sel); // "init" | "init_with_frame_size" | ...
    match mech.strip_prefix("init") {
        Some(rest) => format!("new{rest}"),
        None => format!("new_{mech}"),
    }
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
    fn send_expr_emits_newly_added_msgsend_shapes() {
        let mut e = emitter();
        // i64-returning, one object arg (e.g. -[NSString compare:]) -> rt::msg_i64_id
        let a = e
            .send_expr("this._obj", "compare:", &Ret::ScalarI64, &[Arg::Id("other".into())])
            .expect("i64_id shape is KNOWN");
        assert!(a.contains("rt::msg_i64_id("), "{a}");
        // void, (object, i64) -> rt::msg_void_id_i64
        let b = e
            .send_expr("this._obj", "insertObject:atIndex:", &Ret::Void,
                       &[Arg::Id("obj".into()), Arg::ScalarI64("i".into())])
            .expect("void_id_i64 shape is KNOWN");
        assert!(b.contains("rt::msg_void_id_i64("), "{b}");
        // BOOL-returning, one object arg -> rt::msg_i8_id (the previously-unadvertised shim)
        let c = e
            .send_expr("this._obj", "isEqual:", &Ret::Bool, &[Arg::Id("x".into())])
            .expect("i8_id shape is KNOWN");
        assert!(c.contains("rt::msg_i8_id("), "{c}");
        // HFA geometry: Size in, Size out -> rt::msg_size_size
        let d = e
            .send_expr("this._obj", "sizeForSize:", &Ret::Size, &[Arg::Size("s".into())])
            .expect("size_size shape is KNOWN");
        assert!(d.contains("rt::msg_size_size("), "{d}");
    }

    #[test]
    fn every_known_shape_has_a_runtime_wrapper() {
        // Guards the lockstep invariant: a tag in KNOWN with no matching
        // `fn msg_<tag>` in vendor/objc/src/runtime.cplus would emit a call to a
        // function that doesn't exist. Parse the shipped runtime and diff.
        let runtime = include_str!("../../vendor/objc/src/runtime.cplus");
        let have: std::collections::HashSet<&str> = runtime
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("fn msg_"))
            .filter_map(|l| l.split('(').next())
            .collect();
        // EVERY shape the predicate accepts must have a wrapper — iterate the whole
        // list so newly-added shapes can't drift out of lockstep unnoticed.
        for tag in KNOWN_MSG_SHAPES {
            assert!(msg_shape_is_known(tag), "{tag} should be KNOWN");
            assert!(
                have.contains(tag),
                "KNOWN tag `{tag}` has no `fn msg_{tag}` wrapper in runtime.cplus"
            );
        }
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
    fn unmapped_scalar_and_pointer_typedefs_resolve_through_the_chain() {
        // A method whose return/params are SDK typedefs the direct match doesn't
        // know: they must follow the Pass-1 typedef chain to the declared underlying
        // width (never a guessed one) — scalar typedefs to their scalar, pointer
        // typedefs to an untyped `*u8` handle. An 8/16-bit typedef stays skipped.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "TypedefDecl", "name": "NSLayoutPriority", "type": { "qualType": "float" } },
                { "kind": "TypedefDecl", "name": "NSModalResponse", "type": { "qualType": "NSInteger" } },
                { "kind": "TypedefDecl", "name": "CGImageRef", "type": { "qualType": "struct CGImage *" } },
                { "kind": "TypedefDecl", "name": "CGGlyph", "type": { "qualType": "unsigned short" } },
                { "kind": "ObjCProtocolDecl", "name": "NSThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "priority", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSLayoutPriority" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "applyImage:response:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [
                        { "kind": "ParmVarDecl", "name": "image", "type": { "qualType": "CGImageRef" } },
                        { "kind": "ParmVarDecl", "name": "response", "type": { "qualType": "NSModalResponse" } } ] },
                    { "kind": "ObjCMethodDecl", "name": "glyphAt:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "g", "type": { "qualType": "CGGlyph" } }] },
                ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        // Scalar typedef return -> f32 (float), via the chain.
        assert!(out.contains("fn priority(this) -> f32"), "NSLayoutPriority->f32:\n{out}");
        // Pointer typedef param -> *u8 handle; scalar typedef param -> i64 (shape void_id_i64).
        assert!(out.contains("fn apply_image_response(this, image: *u8, response: i64)"),
            "pointer + scalar typedef params:\n{out}");
        assert!(out.contains("rt::msg_void_id_i64(this._obj"), "wires to the void_id_i64 shim:\n{out}");
        // 8/16-bit typedef stays unmapped (skipped, never mis-typed).
        assert!(out.contains("SKIPPED `glyphAt:`: param `CGGlyph` — unmapped type"), "CGGlyph stays skipped:\n{out}");
    }

    #[test]
    fn availability_macros_and_kindof_arrays_are_stripped() {
        // API_AVAILABLE-family macros and __kindof leak into clang type spellings and
        // must be stripped so the underlying type resolves.
        assert_eq!(strip_nullability("API_AVAILABLE NSArray<NSString *> * _Nonnull").0, "NSArray<NSString *> *");
        assert_eq!(strip_nullability("API_DEPRECATED(\"x\", macos(10.0, 11.0)) id<MTLBuffer>").0, "id<MTLBuffer>");
        // Longest-name-first + word boundary: the shorter prefix doesn't half-eat it.
        assert_eq!(strip_nullability("API_DEPRECATED_WITH_REPLACEMENT(\"y\") id").0, "id");
        // __kindof inside an NSArray element resolves via array_element.
        assert_eq!(array_element("NSArray<__kindof NSView *> *").as_deref(), Some("NSView *"));
    }

    #[test]
    fn completion_handler_typedef_binds_as_a_block_param() {
        // `typedef void (^MTLHandler)(id, NSError *)` used as a param must be detected
        // as a block *through the typedef* and emit the block struct + invoke
        // trampoline + (cb, ctx) wrapper — not skip as an unmapped type. Reuses the
        // same stack-block mechanism as inline `usingBlock:`.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "TypedefDecl", "name": "MTLHandler",
                  "type": { "qualType": "void (^)(id, NSError *)" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "compileWithSource:completionHandler:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [
                        { "kind": "ParmVarDecl", "name": "source", "type": { "qualType": "NSString *" } },
                        { "kind": "ParmVarDecl", "name": "handler", "type": { "qualType": "MTLHandler" } } ] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("cb: fn(*u8, i64, *u8), ctx: *u8"), "block wrapper sig:\n{out}");
        assert!(out.contains("_block {"), "block struct emitted:\n{out}");
        assert!(out.contains("rt::stack_block_isa()"), "stack block:\n{out}");
        assert!(!out.contains("unmapped type `MTLHandler`"), "not skipped as unmapped:\n{out}");
    }

    #[test]
    fn value_struct_with_enum_field_lays_out_at_the_enum_width() {
        // MTLTextureSwizzleChannels { MTLTextureSwizzle red,green } where the enum is
        // NS_ENUM(uint8_t) -> each field lays out as u8 (the enum's fixed underlying
        // width, read from the AST), so the repr(C) struct matches the C ABI. Without
        // the width the whole struct would skip as an unmapped type.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "EnumDecl", "name": "MTLSwizzle", "loc": { "file": "test.h" },
                  "fixedUnderlyingType": { "desugaredQualType": "unsigned char", "qualType": "uint8_t" },
                  "inner": [ { "kind": "EnumConstantDecl", "name": "MTLSwizzleZero" } ] },
                { "kind": "RecordDecl", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "FieldDecl", "name": "red", "type": { "qualType": "MTLSwizzle" } },
                    { "kind": "FieldDecl", "name": "green", "type": { "qualType": "MTLSwizzle" } } ] },
                { "kind": "TypedefDecl", "name": "MTLSwizzleChannels", "loc": { "file": "test.h" },
                  "type": { "qualType": "struct MTLSwizzleChannels" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "swizzle", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "MTLSwizzleChannels" }, "inner": [] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("#[repr(C)]\nstruct SwizzleChannels {\n    red: u8,\n    green: u8,\n}"),
            "enum fields lay out at their u8 underlying width:\n{out}");
        assert!(out.contains("fn swizzle(this) -> SwizzleChannels"), "struct return binds:\n{out}");
    }

    #[test]
    fn value_struct_typedef_alias_resolves_to_the_underlying_struct() {
        // `typedef MTLSamplePosition MTLCoordinate2D;` — a method using the alias
        // must resolve to the registered value struct (value_struct_of follows the
        // typedef), not skip as an unmapped type.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "RecordDecl", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "FieldDecl", "name": "x", "type": { "qualType": "float" } },
                    { "kind": "FieldDecl", "name": "y", "type": { "qualType": "float" } } ] },
                { "kind": "TypedefDecl", "name": "MTLSamplePosition", "loc": { "file": "test.h" },
                  "type": { "qualType": "struct MTLSamplePosition" } },
                { "kind": "TypedefDecl", "name": "MTLCoordinate2D", "loc": { "file": "test.h" },
                  "type": { "qualType": "MTLSamplePosition" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "originFor:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "MTLCoordinate2D" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "coord", "type": { "qualType": "MTLCoordinate2D" } }] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn origin_for(this, coord: SamplePosition) -> SamplePosition"),
            "alias resolves to the value struct:\n{out}");
        assert!(!out.contains("unmapped type `MTLCoordinate2D`"), "no unmapped skip:\n{out}");
    }

    #[test]
    fn const_qualifier_is_stripped_before_type_mapping() {
        // `const` on a by-value param/return is an ABI no-op; a const-qualified
        // typedef must resolve like the bare type (here MTLMode -> NSUInteger -> u64),
        // not skip as an unmapped type.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "TypedefDecl", "name": "MTLMode", "type": { "qualType": "NSUInteger" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "useMode:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "mode", "type": { "qualType": "const MTLMode" } }] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn use_mode(this, mode: u64)"), "const stripped + typedef resolved:\n{out}");
        assert!(!out.contains("unmapped type"), "no unmapped skip:\n{out}");
    }

    #[test]
    fn multiple_init_variants_each_bind_as_a_named_constructor() {
        // A class with several `init*` selectors: the first in AST order keeps the
        // plain `new` (byte-stable with the old single-`new` behavior); every other
        // variant becomes `new_with_<selector>`. Was: only the first bound, the rest
        // skipped as "extra init variant".
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCInterfaceDecl", "name": "NSThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "initWithName:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "instancetype" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "name", "type": { "qualType": "long" } }] },
                    { "kind": "ObjCMethodDecl", "name": "initWithCoder:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "instancetype" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "coder", "type": { "qualType": "id" } }] },
                    { "kind": "ObjCMethodDecl", "name": "init", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "instancetype" }, "inner": [] },
                ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        // Primary (first in AST order) keeps the plain `new`.
        assert!(out.contains("fn new(name: i64) -> Thing"), "primary init -> new:\n{out}");
        // Every other variant gets a distinct `new_with_<selector>` constructor.
        assert!(out.contains("fn new_with_coder(coder: *u8) -> Thing"), "second init -> new_with_coder:\n{out}");
        // The legacy blanket skip is gone; the bare `init` here collides with the
        // primary `new` so it drops as an already-defined constructor, not a dangling fn.
        assert!(!out.contains("extra init variant"), "no legacy extra-init skip:\n{out}");
        assert!(out.contains("`new` already defined"), "bare init collides with primary new:\n{out}");
    }

    #[test]
    fn nsarray_of_protocol_param_builds_from_a_typed_vec() {
        // NSArray<id<P>> (P a non-owning protocol) param -> Vec[P]; the method body
        // builds an NSMutableArray from the Vec before the send.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCProtocolDecl", "name": "MTLBuffer", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCProtocolDecl", "name": "MTLDevice", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "useBuffers:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "buffers",
                        "type": { "qualType": "NSArray<id<MTLBuffer>> * _Nonnull" } }] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "MTL", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn use_buffers(this, buffers: vec::Vec[Buffer])"), "typed Vec param:\n{out}");
        assert!(out.contains("let arr_buffers: *u8 = rt::msg_id(rt::get_class(#str_ptr(\"NSMutableArray\\0\"))"), "builds NSArray:\n{out}");
        assert!(out.contains("buffers.at_ptr(i_buffers)"), "iterates the Vec by borrow:\n{out}");
        assert!(out.contains("(*e_buffers).raw()"), "adds each element's handle (borrow-read):\n{out}");
        assert!(out.contains("\"useBuffers:\\0\")), arr_buffers)"), "passes the built array:\n{out}");
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
        // In per-header mode the shim name is module-discriminated (here `test`) so the
        // same value struct used by sibling modules doesn't collide on one extern name.
        assert!(out.contains("extern fn objc_msg_test_Size(recv: *u8, sel: *u8) -> Size;"), "sret shim:\n{out}");
        assert!(out.contains("extern fn objc_msg_test_void_Size(recv: *u8, sel: *u8, a0: Size);"), "arg shim:\n{out}");
        assert!(out.contains("objc_msg_test_void_Size(this._obj"), "uses local shim:\n{out}");
    }

    #[test]
    fn value_struct_shims_are_module_discriminated_per_header_but_bare_when_merged() {
        // Regression: extern fns share one global bare-name namespace, so two
        // per-header modules each emitting `objc_msg_Size` for their *own* local
        // `Size` struct collide — the dedup binds one module's call to the other's
        // signature (E0302). The shim name must be discriminated by module in
        // per-header mode, and stay bare under `--merge` (one module, no collision).
        let tu = |home: &str| serde_json::json!({
            "inner": [
                { "kind": "RecordDecl", "loc": { "file": home }, "inner": [
                    { "kind": "FieldDecl", "name": "width", "type": { "qualType": "NSUInteger" } },
                    { "kind": "FieldDecl", "name": "height", "type": { "qualType": "NSUInteger" } } ] },
                { "kind": "TypedefDecl", "name": "MTLSize", "loc": { "file": home },
                  "type": { "qualType": "struct MTLSize" } },
                { "kind": "ObjCProtocolDecl", "name": "MTLThing", "loc": { "file": home }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "size", "instance": true, "loc": { "file": home },
                      "returnType": { "qualType": "MTLSize" }, "inner": [] } ] },
            ]
        });
        // Two different headers -> two different shim names (no cross-module clash).
        let a = ObjcEmitter::new("MTLAlpha.h", "MTL", serde_json::json!({})).run(&tu("MTLAlpha.h"));
        let b = ObjcEmitter::new("MTLBeta.h", "MTL", serde_json::json!({})).run(&tu("MTLBeta.h"));
        assert!(a.contains("extern fn objc_msg_alpha_Size("), "module-A shim name:\n{a}");
        assert!(b.contains("extern fn objc_msg_beta_Size("), "module-B shim name:\n{b}");
        assert!(!a.contains("objc_msg_beta_Size") && !b.contains("objc_msg_alpha_Size"),
            "shim names must not collide across modules");
        // `--merge`: one module, so the bare name is kept (output stays byte-stable).
        let home: HashSet<String> = std::iter::once("MTLAlpha.h".to_string()).collect();
        let m = ObjcEmitter::new_merged("merged", "MTL", serde_json::json!({}), home).run(&tu("MTLAlpha.h"));
        assert!(m.contains("extern fn objc_msg_Size("), "merged keeps the bare shim name:\n{m}");
        assert!(!m.contains("objc_msg_alpha_Size"), "merged must not discriminate:\n{m}");
    }

    #[test]
    fn nsarray_of_protocol_becomes_a_typed_vec() {
        // NSArray<id<P>> (P a non-owning protocol) -> Vec[P], wrapping each element.
        // An array whose element type is NOT declared in this module (MTLThing here)
        // stays skipped — the wrapper must exist to be a typed Vec element.
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
    fn nsarray_class_element_returns_bind_factory_plain_owning_retains() {
        // Class-element array RETURNS bind to Vec[W] for any module-defined wrapper.
        // `NSScreen` (no init -> factory/singleton, `opaque`, no drop) wraps each +0
        // element directly. `NSWindow` (has `initWith*` -> owning, `drop` releases)
        // retains each element on wrap so the +1 balances the drop. An owning array
        // *param* still skips: Vec iteration uses `at`, which is `T: Copy`-bound and
        // owning wrappers are non-Copy.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCInterfaceDecl", "name": "NSScreen", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCInterfaceDecl", "name": "NSWindow", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "initWithFrame:", "instance": true,
                      "loc": { "file": "test.h" }, "returnType": { "qualType": "instancetype" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "frame", "type": { "qualType": "NSRect" } }] } ] },
                { "kind": "ObjCInterfaceDecl", "name": "NSApplication", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "screens", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSArray<NSScreen *> * _Nonnull" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "windows", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSArray<NSWindow *> * _Nonnull" }, "inner": [] },
                    { "kind": "ObjCMethodDecl", "name": "setOrderedWindows:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "ordered", "type": { "qualType": "NSArray<NSWindow *> *" } }] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        // Factory class: opaque wrapper, Vec[Screen] return, plain from_raw (no retain).
        assert!(out.contains("struct Screen {\n    opaque _obj: *u8,\n}"), "factory class opaque:\n{out}");
        assert!(out.contains("fn screens(this) -> vec::Vec[Screen]"), "factory-class array return:\n{out}");
        assert!(out.contains("out.append(Screen::from_raw(rt::msg_id_u64("), "non-owning wrap, no retain:\n{out}");
        // Owning class: Vec[Window] return, each element retained on wrap.
        assert!(out.contains("fn windows(this) -> vec::Vec[Window]"), "owning-class array return binds:\n{out}");
        assert!(out.contains("out.append(Window::from_raw(rt::retain(rt::msg_id_u64("), "owning wrap retains:\n{out}");
        // Owning array PARAM now binds too: the prologue borrow-reads each element's
        // handle via `at_ptr` (no move/drop — cpc's borrowck proves it), so the caller's
        // Vec keeps ownership while `addObject:` takes its own retain. (The *return*
        // direction is the one needing retain-on-wrap; a param only reads owned elements.)
        assert!(out.contains("fn set_ordered_windows(this, ordered: vec::Vec[Window])"), "owning array param binds:\n{out}");
        assert!(out.contains("match ordered.at_ptr(i_ordered)"), "borrow-reads each element:\n{out}");
    }

    #[test]
    fn nsarray_id_param_with_collection_return_or_block_skips_not_breaks() {
        // The `Vec[W] -> NSMutableArray` prologue is only built in the general
        // path. A method that pairs an NSArray<id> param with a collection return
        // or a block builds its body elsewhere and would reference an undefined
        // `arr_<pname>` — those combos must SKIP, not emit broken code.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCInterfaceDecl", "name": "NSScreen", "loc": { "file": "test.h" }, "inner": [] },
                { "kind": "ObjCInterfaceDecl", "name": "NSThing", "loc": { "file": "test.h" }, "inner": [
                    // NSArray<id> param + NSArray return -> skip
                    { "kind": "ObjCMethodDecl", "name": "filter:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "NSArray<NSScreen *> * _Nonnull" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "items", "type": { "qualType": "NSArray<NSScreen *> *" } }] },
                    // NSArray<id> param + block -> skip
                    { "kind": "ObjCMethodDecl", "name": "process:completion:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "void" },
                      "inner": [
                        { "kind": "ParmVarDecl", "name": "items", "type": { "qualType": "NSArray<NSScreen *> *" } },
                        { "kind": "ParmVarDecl", "name": "completion", "type": { "qualType": "void (^)(void)" } } ] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        assert!(out.contains("// SKIPPED `filter:`: NSArray<id> param with a collection return not modelled"),
            "collection-return combo must skip:\n{out}");
        assert!(out.contains("// SKIPPED `process:completion:`: block method with an NSArray<id> param not modelled"),
            "block combo must skip:\n{out}");
        // And no dangling `arr_items` reference leaked into emitted code.
        assert!(!out.contains("arr_items"), "no undefined array local emitted:\n{out}");
    }

    #[test]
    fn sel_and_class_bind_as_raw_pointers() {
        // `SEL` and `Class` are ObjC pointer types with no wrapper; they bind as
        // raw `*u8` handles (callers pass `rt::sel(...)` / `rt::get_class(...)`),
        // not SKIPPED. A BOOL-returning SEL predicate uses the i8_id shim.
        let tu = serde_json::json!({
            "inner": [
                { "kind": "ObjCInterfaceDecl", "name": "NSThing", "loc": { "file": "test.h" }, "inner": [
                    { "kind": "ObjCMethodDecl", "name": "respondsToSelector:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "BOOL" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "aSelector", "type": { "qualType": "SEL" } }] },
                    { "kind": "ObjCMethodDecl", "name": "isKindOfClass:", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "BOOL" },
                      "inner": [{ "kind": "ParmVarDecl", "name": "aClass", "type": { "qualType": "Class" } }] },
                    { "kind": "ObjCMethodDecl", "name": "class", "instance": true, "loc": { "file": "test.h" },
                      "returnType": { "qualType": "Class" }, "inner": [] } ] },
            ]
        });
        let out = ObjcEmitter::new("test.h", "NS", serde_json::json!({})).run(&tu);
        assert!(out.contains("fn responds_to_selector(this, a_selector: *u8) -> bool"), "SEL param:\n{out}");
        assert!(out.contains("fn is_kind_of_class(this, a_class: *u8) -> bool"), "Class param:\n{out}");
        assert!(out.contains("-> *u8"), "Class return is a raw handle:\n{out}");
        assert!(!out.contains("unmapped type `SEL`") && !out.contains("unmapped type `Class`"),
            "SEL/Class must not be skipped:\n{out}");
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
