// cpc-bindgen --gobject: GObject-Introspection (GIR) -> C+ bindings.
//
// GIR is the GObject world's machine-readable API description (the analog of
// clang's ObjC AST for Apple, or `swift symbolgraph-extract` for Swift). Unlike
// a raw C header it records exactly what a correct binding needs and headers
// cannot express: `transfer-ownership` (who frees), `nullable` (Option), the
// class hierarchy (`parent=`, legal upcasts), constructor-vs-method, and signal
// signatures. So the GObject binder reads GIR, not headers.
//
// Emitted code targets `vendor/gobject` for the cross-cutting machinery
// (lifetime, signals, gchar*<->Text), exactly as the ObjC emitter targets
// `vendor/objc`. Every construct we cannot model becomes `// SKIPPED <name>:
// <reason>`, never wrong code — the same invariant as the C/ObjC paths.
//
// This file is two halves: a minimal DOM-style XML parser (GIR is regular
// enough not to need a full XML crate, and cpc-bindgen stays dependency-free),
// and an emitter that walks the parsed <namespace>.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

/// Bare function names already occupied by the modules a generated binding
/// imports (`vendor/gobject` + the libc/allocator externs it declares, plus the
/// `stdlib` module-level API). C+ registers free functions in one global,
/// unqualified name space, so a generated wrapper with any of these names would
/// collide (E0301). Our `__c_`-prefixed externs never collide; only the
/// ergonomic wrappers can, and those become `// SKIPPED`. Missing an entry is
/// safe — `cpc check` catches it — this just makes the SKIP explicit up front.
const RESERVED: &[&str] = &[
    // vendor/gobject/runtime
    "object_ref", "object_unref", "object_ref_sink", "object_is_floating",
    "set_data", "get_data", "type_from_name", "is_a", "instance_type_name", "free",
    // vendor/gobject/signal
    "connect", "connect_bool", "disconnect",
    // vendor/gobject/bridge (+ its libc externs)
    "c_strlen", "cstr_to_text", "cstr_to_text_full", "cstr_to_str_unsafe",
    "str_to_cstring", "free_cstring", "malloc", "memcpy", "realloc", "memcmp",
    // stdlib/text + stdlib/option module-level API
    "from_str", "some", "new", "with_capacity",
    // the program entry point — a module-level `fn main` must be `fn main() -> i32`
    "main",
];

// ---------------------------------------------------------------------------
// XML parser (DOM-lite). We only care about elements + attributes; all text
// content in GIR is <doc> prose, which we skip. Namespaced names ("c:identifier",
// "glib:signal") are kept verbatim as the element/attribute key.
// ---------------------------------------------------------------------------

pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    fn new(name: String, attrs: Vec<(String, String)>) -> Self {
        Node { name, attrs, children: Vec::new() }
    }
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn child_named(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Parse a GIR document into a synthetic root whose children are the top-level
/// elements (`<repository>`). Robust to multi-line tags, comments, CDATA, the
/// XML prolog, and DOCTYPE; ignores text nodes.
pub fn parse(src: &str) -> Node {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut stack: Vec<Node> = vec![Node::new("#root".to_string(), Vec::new())];
    while i < b.len() {
        if b[i] != b'<' {
            i += 1; // skip text between tags (GIR text is <doc> prose only)
            continue;
        }
        // Dispatch on what follows '<'.
        if src[i..].starts_with("<?") {
            i = find(src, i, "?>").map(|e| e + 2).unwrap_or(b.len());
            continue;
        }
        if src[i..].starts_with("<!--") {
            i = find(src, i, "-->").map(|e| e + 3).unwrap_or(b.len());
            continue;
        }
        if src[i..].starts_with("<![CDATA[") {
            i = find(src, i, "]]>").map(|e| e + 3).unwrap_or(b.len());
            continue;
        }
        if src[i..].starts_with("<!") {
            i = find(src, i, ">").map(|e| e + 1).unwrap_or(b.len());
            continue;
        }
        // A real tag: scan to the matching '>' respecting quotes.
        let end = tag_end(b, i);
        let inner = &src[i + 1..end]; // between '<' and '>'
        i = end + 1;
        if let Some(closed) = inner.strip_prefix('/') {
            // Close tag: pop, attaching the finished node to its parent.
            let name = closed.trim();
            if stack.len() > 1 {
                let node = stack.pop().unwrap();
                if node.name == name || true {
                    stack.last_mut().unwrap().children.push(node);
                }
            }
            continue;
        }
        let self_closing = inner.ends_with('/');
        let inner = inner.trim_end_matches('/').trim();
        let (name, attrs) = parse_tag(inner);
        let node = Node::new(name, attrs);
        if self_closing {
            stack.last_mut().unwrap().children.push(node);
        } else {
            stack.push(node);
        }
    }
    // Collapse any unclosed nodes into the root (defensive).
    while stack.len() > 1 {
        let node = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(node);
    }
    stack.pop().unwrap()
}

fn find(src: &str, from: usize, needle: &str) -> Option<usize> {
    src[from..].find(needle).map(|p| from + p)
}

/// Index of the '>' that closes the tag starting at `open` (byte '<'),
/// skipping any '>' inside single/double-quoted attribute values.
fn tag_end(b: &[u8], open: usize) -> usize {
    let mut i = open + 1;
    let mut quote: u8 = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return i;
        }
        i += 1;
    }
    b.len()
}

/// Split a tag's inner text into element name + attributes. Attribute values are
/// quoted and may span newlines; entities in values are decoded.
fn parse_tag(inner: &str) -> (String, Vec<(String, String)>) {
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    // Element name: up to first whitespace.
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let name = inner[..i].to_string();
    let mut attrs = Vec::new();
    while i < bytes.len() {
        // skip whitespace
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // attribute key up to '=' or whitespace
        let ks = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = inner[ks..i].to_string();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            if !key.is_empty() {
                attrs.push((key, String::new()));
            }
            continue;
        }
        i += 1; // '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let q = bytes[i];
        if q == b'"' || q == b'\'' {
            i += 1;
            let vs = i;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            let val = decode_entities(&inner[vs..i]);
            i += 1; // closing quote
            attrs.push((key, val));
        }
    }
    (name, attrs)
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        if let Some(semi) = tail.find(';') {
            let ent = &tail[1..semi];
            match ent {
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "amp" => out.push('&'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                _ if ent.starts_with('#') => {
                    let code = if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        ent[1..].parse::<u32>().ok()
                    };
                    if let Some(ch) = code.and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                _ => {
                    out.push('&');
                    out.push_str(ent);
                    out.push(';');
                }
            }
            rest = &tail[semi + 1..];
        } else {
            out.push('&');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

/// Resolve the GIR file for a path or a `Namespace[-Version]` name. GIR ships in
/// two dirs on Debian/Ubuntu: /usr/share/gir-1.0 and the arch libdir.
fn find_gir_file(arg: &str) -> Option<PathBuf> {
    let p = PathBuf::from(arg);
    if p.is_file() {
        return Some(p);
    }
    let dirs = [
        "/usr/share/gir-1.0",
        "/usr/lib/x86_64-linux-gnu/gir-1.0",
        "/usr/lib64/gir-1.0",
    ];
    for d in dirs {
        let exact = PathBuf::from(d).join(format!("{arg}.gir"));
        if exact.is_file() {
            return Some(exact);
        }
        // `arg` may be a bare namespace ("Gtk"); pick the HIGHEST version among
        // `arg-<ver>.gir` (so bare `Gtk` binds Gtk-4.0, not Gtk-2.0 — a plain
        // lexicographic sort would wrongly prefer the older 2.0).
        if let Ok(rd) = std::fs::read_dir(d) {
            let prefix = format!("{arg}-");
            let mut hits: Vec<(Vec<u32>, PathBuf)> = rd
                .flatten()
                .map(|e| e.path())
                .filter_map(|p| {
                    let n = p.file_name()?.to_str()?.to_string();
                    let ver = n.strip_prefix(&prefix)?.strip_suffix(".gir")?;
                    Some((parse_version(ver), p))
                })
                .collect();
            hits.sort();
            if let Some((_, h)) = hits.into_iter().next_back() {
                return Some(h);
            }
        }
    }
    None
}

/// Parse a GIR version stem (`"4.0"`, `"2.0"`) into comparable numeric parts;
/// non-numeric segments sort as 0 so a numeric compare orders 4.0 > 2.0 > 1.
fn parse_version(v: &str) -> Vec<u32> {
    v.split('.').map(|s| s.parse::<u32>().unwrap_or(0)).collect()
}

pub fn generate(arg: &str) -> Result<String, String> {
    let path = find_gir_file(arg).ok_or_else(|| format!("cannot find GIR for `{arg}` (looked in /usr/share/gir-1.0 and the arch libdir)"))?;
    let src = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let root = parse(&src);
    let repo = root.child_named("repository").ok_or("no <repository> in GIR")?;
    let ns = repo.child_named("namespace").ok_or("no <namespace> in GIR")?;
    let mut em = Emitter::new(ns, &path.display().to_string());
    Ok(em.run())
}

/// `--out DIR`: generate a whole C+ package (the GObject sibling of
/// `--framework`). Writes `DIR/src/<pkg>.cplus` (the bindings) and `DIR/Cplus.toml`
/// (deps on gobject + stdlib, `[link]` libs derived from the GIR
/// `shared-library`, and a provenance header). `<pkg>` is the output directory's
/// basename, so it satisfies the vendor "name matches directory" rule.
pub fn generate_package(arg: &str, out_dir: &str) -> Result<(), String> {
    let path = find_gir_file(arg).ok_or_else(|| format!("cannot find GIR for `{arg}`"))?;
    let src = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let root = parse(&src);
    let repo = root.child_named("repository").ok_or("no <repository> in GIR")?;
    let ns = repo.child_named("namespace").ok_or("no <namespace> in GIR")?;
    let ns_name = ns.attr("name").unwrap_or("Unknown").to_string();
    let ns_ver = ns.attr("version").unwrap_or("").to_string();
    let libs = link_libs(ns);

    let mut em = Emitter::new(ns, &path.display().to_string());
    let module = em.run();
    let (emitted, skips) = (em.emitted, em.skips);

    let out = PathBuf::from(out_dir);
    let pkg = out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("--out DIR has no basename")?
        .to_string();
    let srcdir = out.join("src");
    std::fs::create_dir_all(&srcdir).map_err(|e| format!("mkdir {}: {e}", srcdir.display()))?;

    let libs_toml = libs.iter().map(|l| format!("\"{l}\"")).collect::<Vec<_>>().join(", ");
    let manifest = format!(
        "[package]\n\
         name    = \"{pkg}\"\n\
         version = \"0.0.1\"\n\
         edition = \"2026\"\n\n\
         # Auto-generated by cpc-bindgen --gobject.\n\
         # GIR:       {gir} (namespace {ns_name} {ns_ver})\n\
         # Reproduce: cpc-bindgen --gobject {arg} --out {out_dir}\n\
         # Coverage:  {emitted} items, {skips} SKIPPED (see `// SKIPPED` in src).\n\n\
         [dependencies]\n\
         gobject = \"*\"\n\
         stdlib  = \"*\"\n\n\
         [link]\n\
         libs = [{libs_toml}]\n",
        gir = path.display(),
    );

    std::fs::write(out.join("Cplus.toml"), manifest).map_err(|e| format!("write Cplus.toml: {e}"))?;
    std::fs::write(srcdir.join(format!("{pkg}.cplus")), module).map_err(|e| format!("write module: {e}"))?;
    eprintln!("cpc-bindgen --gobject: wrote {out_dir}/ ({pkg}: {emitted} items, {skips} SKIPPED, links {libs:?})");
    Ok(())
}

/// The linker library names for a namespace, from its GIR `shared-library`
/// (`libgtk-4.so.1,libglib-2.0.so.0` -> `["gtk-4", "glib-2.0"]`).
fn link_libs(ns: &Node) -> Vec<String> {
    ns.attr("shared-library")
        .map(|s| s.split(',').filter_map(so_to_lib).collect())
        .unwrap_or_default()
}

/// `libgtk-4.so.1` -> `gtk-4` (strip the `lib` prefix and the `.so[.N]` suffix).
fn so_to_lib(so: &str) -> Option<String> {
    let base = so.trim().strip_prefix("lib")?;
    let stem = base.split(".so").next()?;
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

struct Emitter<'a> {
    ns: &'a Node,
    out: String,
    skips: usize,
    emitted: usize,
    /// Bare wrapper names already defined (this module + reserved imports), so a
    /// later collision becomes a SKIP instead of an E0301.
    seen: HashSet<String>,
    /// Names of `<class>`/`<interface>` in this namespace — the set an object
    /// type must be in to resolve to a wrapper struct (else it stays foreign).
    wrapper_types: HashSet<String>,
    /// Wrapper struct type names already emitted (types are a separate name
    /// space from fns); a collision is skipped.
    seen_types: HashSet<String>,
    /// In-namespace enum/bitfield names -> their C+ integer repr (`i32` for an
    /// `<enumeration>`, `u32` for a `<bitfield>`). Used to bind enum-typed params
    /// and returns as their ABI integer (callers pass the emitted constant fns).
    enum_types: HashMap<String, String>,
}

impl<'a> Emitter<'a> {
    fn new(ns: &'a Node, source: &str) -> Self {
        let ns_name = ns.attr("name").unwrap_or("Unknown").to_string();
        let mut out = String::new();
        out.push_str("// Auto-generated by cpc-bindgen --gobject. DO NOT EDIT.\n");
        out.push_str(&format!("// GIR: {source}\n"));
        out.push_str(&format!("// Namespace: {ns_name}\n//\n"));
        out.push_str("// GObject bindings: methods are direct C symbols; lifetime, signals, and\n");
        out.push_str("// gchar*<->Text conversion come from `vendor/gobject`.\n\n");
        out.push_str("import \"gobject/runtime\" as g;\n");
        out.push_str("import \"gobject/signal\" as sig;\n");
        out.push_str("import \"gobject/bridge\" as bridge;\n");
        out.push_str("import \"stdlib/text\" as text;\n");
        out.push_str("import \"stdlib/option\" as option;\n\n");
        let seen: HashSet<String> = RESERVED.iter().map(|s| s.to_string()).collect();
        // Every class/interface name is a candidate wrapper type — collected up
        // front so an object return/param anywhere resolves regardless of order.
        let mut wrapper_types = HashSet::new();
        for c in ns.children_named("class") {
            if let Some(n) = c.attr("name") {
                wrapper_types.insert(n.to_string());
            }
        }
        for c in ns.children_named("interface") {
            if let Some(n) = c.attr("name") {
                wrapper_types.insert(n.to_string());
            }
        }
        let mut enum_types = HashMap::new();
        for e in ns.children_named("enumeration") {
            if let Some(n) = e.attr("name") {
                enum_types.insert(n.to_string(), "i32".to_string());
            }
        }
        for e in ns.children_named("bitfield") {
            if let Some(n) = e.attr("name") {
                enum_types.insert(n.to_string(), "u32".to_string());
            }
        }
        Emitter {
            ns,
            out,
            skips: 0,
            emitted: 0,
            seen,
            wrapper_types,
            seen_types: HashSet::new(),
            enum_types,
        }
    }

    /// Reserve a wrapper name; returns false if it was already taken (caller
    /// should SKIP). Reserved-import names are pre-seeded, so this also rejects
    /// collisions with `vendor/gobject` / `stdlib`.
    fn claim(&mut self, name: &str) -> bool {
        self.seen.insert(name.to_string())
    }

    /// Map a `<type>` node, resolving in-namespace object types to their wrapper
    /// struct on top of the scalar/string vocabulary. A namespaced name (`Gdk.`,
    /// `GObject.`) is foreign -> None. Arrays/callbacks (no `name`) -> None.
    fn map(&self, t: &Node) -> Option<Mapped> {
        if let Some(m) = map_type(t) {
            return Some(m);
        }
        let name = t.attr("name")?;
        if name.contains('.') {
            return None; // foreign namespace (Gdk.Rectangle, GObject.Object, GLib.List)
        }
        // In-namespace enum/bitfield -> its ABI integer. Callers pass the emitted
        // constant fns (e.g. `orientation_horizontal()`).
        if let Some(repr) = self.enum_types.get(name) {
            return Some(Mapped { cat: Cat::Scalar, extern_ty: repr.clone(), obj: None });
        }
        if self.wrapper_types.contains(name) {
            return Some(Mapped { cat: Cat::Obj, extern_ty: "*u8".to_string(), obj: Some(ident_type(name)) });
        }
        None
    }

    fn run(&mut self) -> String {
        // Slice 1: namespace-level free functions + enum/bitfield constants.
        self.out.push_str("// === Enumerations & flags ===\n\n");
        for e in self.ns.children_named("enumeration") {
            self.emit_enum(e, "i32");
        }
        for e in self.ns.children_named("bitfield") {
            self.emit_enum(e, "u32");
        }
        self.out.push_str("\n// === Free functions ===\n\n");
        for f in self.ns.children_named("function") {
            self.emit_function(f);
        }
        self.out.push_str("\n// === Interfaces ===\n\n");
        for c in self.ns.children_named("interface") {
            self.emit_class(c);
        }
        self.out.push_str("\n// === Classes ===\n\n");
        for c in self.ns.children_named("class") {
            self.emit_class(c);
        }
        self.out.push_str(&format!(
            "\n// cpc-bindgen --gobject: {} items emitted, {} SKIPPED.\n",
            self.emitted, self.skips
        ));
        std::mem::take(&mut self.out)
    }

    fn skip(&mut self, kind: &str, name: &str, reason: &str) {
        self.out.push_str(&format!("// SKIPPED {kind} `{name}`: {reason}\n"));
        self.skips += 1;
    }

    // --- enumerations / bitfields -> module-level integer constant fns ---
    fn emit_enum(&mut self, e: &Node, repr: &str) {
        let ty = match e.attr("name") {
            Some(n) => n,
            None => return,
        };
        let prefix = snake(ty);
        self.out.push_str(&format!("// {} `{}` — constants as {repr}.\n", if repr == "u32" { "flags" } else { "enum" }, ty));
        for m in e.children_named("member") {
            let (mname, val) = match (m.attr("name"), m.attr("value")) {
                (Some(n), Some(v)) => (n, v),
                _ => continue,
            };
            let val: i64 = match val.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let fname = ident(&format!("{prefix}_{mname}"));
            if !self.claim(&fname) {
                continue;
            }
            self.out.push_str(&format!("fn {fname}() -> {repr} {{ return {val} as {repr}; }}\n"));
            self.emitted += 1;
        }
        self.out.push('\n');
    }

    // --- free functions ---
    fn emit_function(&mut self, f: &Node) {
        let name = match f.attr("name") {
            Some(n) => n,
            None => return,
        };
        let cid = match f.attr("c:identifier") {
            Some(c) => c,
            None => {
                self.skip("fn", name, "no c:identifier");
                return;
            }
        };
        // Return type.
        let rv = match f.child_named("return-value") {
            Some(r) => r,
            None => {
                self.skip("fn", name, "no return-value");
                return;
            }
        };
        let ret_ty = rv.child_named("type");
        let ret = match ret_ty.and_then(|t| self.map(t)) {
            Some(m) => m,
            None => {
                self.skip("fn", name, &format!("return type — {}", type_reason(ret_ty)));
                return;
            }
        };
        let ret_full = matches!(rv.attr("transfer-ownership"), Some("full"));
        let ret_nullable = matches!(rv.attr("nullable"), Some("1"));

        let params = match self.map_params(f, "fn", name) {
            Some(p) => p,
            None => return,
        };

        // C+ free functions share one global name space; a wrapper whose bare
        // name is already taken (a sibling function or a reserved import symbol)
        // can't be redefined.
        let wname = ident(name);
        if !self.claim(&wname) {
            self.skip("fn", name, &format!("name `{wname}` already defined (would collide with a sibling or `vendor/gobject`/`stdlib`)"));
            return;
        }
        let (ext, wrap) = self.render(&wname, cid, None, &ret, ret_full, ret_nullable, &params);
        self.out.push_str(&ext);
        // `render` indents for an `impl` body; a free function sits at column 0.
        self.out.push_str(&dedent4(&wrap));
        self.emitted += 1;
    }

    /// Build the `extern fn __c_<symbol>` declaration + the ergonomic wrapper for
    /// one callable. `recv = Some("this._raw")` makes it a method (leading opaque
    /// receiver). Returns `(extern_decl, wrapper_fn)` as separate strings so the
    /// caller can place the extern at module level and the wrapper inside an
    /// `impl`. Handles scalar/bool/string/gpointer/object params & returns; string
    /// params allocate a temp cstring freed after the call; object params take a
    /// `ref` and pass `.raw()`; object returns wrap via `from_raw`.
    fn render(
        &self,
        wname: &str,
        cid: &str,
        recv: Option<&str>,
        ret: &Mapped,
        ret_full: bool,
        ret_nullable: bool,
        params: &[(String, Mapped)],
    ) -> (String, String) {
        let ext_name = format!("__c_{}", sanitize_sym(cid));

        // --- extern declaration (wire types) ---
        let mut ext_params: Vec<String> = Vec::new();
        if recv.is_some() {
            ext_params.push("__recv: *u8".to_string());
        }
        for (n, m) in params {
            ext_params.push(format!("{n}: {}", m.extern_ty));
        }
        let ext_ret = if ret.extern_ty == "()" { String::new() } else { format!(" -> {}", ret.extern_ty) };
        let ext = format!(
            "#[link_name = \"{cid}\"]\nextern fn {ext_name}({}){ext_ret};\n",
            ext_params.join(", ")
        );

        // --- wrapper signature (ergonomic types) ---
        let mut wrap_params: Vec<String> = Vec::new();
        if recv.is_some() {
            wrap_params.push("this".to_string());
        }
        for (n, m) in params {
            // Object params bind by reference (`ref name: T`) so the caller keeps
            // its wrapper; scalars/strings/bools bind by value.
            if m.cat == Cat::Obj {
                wrap_params.push(format!("ref {n}: {}", m.wrap_param_ty()));
            } else {
                wrap_params.push(format!("{n}: {}", m.wrap_param_ty()));
            }
        }
        let wrap_ret = ret.wrap_ret_ty(ret_nullable);
        let wrap_ret_sig = if wrap_ret == "()" { String::new() } else { format!(" -> {wrap_ret}") };
        let mut w = format!("    fn {wname}({}){wrap_ret_sig} {{\n", wrap_params.join(", "));

        // --- marshal params in ---
        let mut call_args: Vec<String> = Vec::new();
        if let Some(r) = recv {
            call_args.push(r.to_string());
        }
        let mut frees: Vec<String> = Vec::new();
        for (n, m) in params {
            match m.cat {
                Cat::Str => {
                    w.push_str(&format!("        let __cs_{n}: *u8 = bridge::str_to_cstring({n});\n"));
                    call_args.push(format!("__cs_{n}"));
                    frees.push(format!("__cs_{n}"));
                }
                Cat::Bool => call_args.push(format!("(if {n} {{ 1 as i32 }} else {{ 0 as i32 }})")),
                Cat::Obj => call_args.push(format!("{n}.raw()")),
                _ => call_args.push(n.clone()),
            }
        }
        let call = format!("{{ {ext_name}({}) }}", call_args.join(", "));

        let emit_frees = |out: &mut String| {
            for fr in &frees {
                out.push_str(&format!("        bridge::free_cstring({fr});\n"));
            }
        };

        // --- invoke + convert the return ---
        match ret.cat {
            Cat::Void => {
                w.push_str(&format!("        {call};\n"));
                emit_frees(&mut w);
                w.push_str("        return;\n");
            }
            Cat::Str => {
                w.push_str(&format!("        let __r: *u8 = {call};\n"));
                emit_frees(&mut w);
                let conv = if ret_full { "bridge::cstr_to_text_full(__r)" } else { "bridge::cstr_to_text(__r)" };
                if ret_nullable {
                    w.push_str("        if __r == { 0 as *u8 } { return option::Option[text::Text]::None; }\n");
                    w.push_str(&format!("        return option::some({conv});\n"));
                } else {
                    w.push_str(&format!("        return {conv};\n"));
                }
            }
            Cat::Bool => {
                w.push_str(&format!("        let __r: i32 = {call};\n"));
                emit_frees(&mut w);
                w.push_str("        return __r != (0 as i32);\n");
            }
            Cat::Obj => {
                let o = ret.obj.clone().unwrap();
                w.push_str(&format!("        let __r: *u8 = {call};\n"));
                emit_frees(&mut w);
                if ret_nullable {
                    w.push_str(&format!("        if __r == {{ 0 as *u8 }} {{ return option::Option[{o}]::None; }}\n"));
                    w.push_str(&format!("        return option::some({o}::from_raw(__r));\n"));
                } else {
                    w.push_str(&format!("        return {o}::from_raw(__r);\n"));
                }
            }
            _ => {
                w.push_str(&format!("        let __r: {} = {call};\n", ret.extern_ty));
                emit_frees(&mut w);
                w.push_str("        return __r;\n");
            }
        }
        w.push_str("    }\n\n");
        (ext, w)
    }

    // --- classes: wrapper struct over a GObject handle ---
    fn emit_class(&mut self, c: &Node) {
        let name = match c.attr("name") {
            Some(n) => n,
            None => return,
        };
        let ty = ident_type(name);
        if !self.seen_types.insert(ty.clone()) {
            self.skip("class", name, &format!("type `{ty}` already defined"));
            return;
        }
        let parent = c.attr("parent").unwrap_or("");
        let doc = if parent.is_empty() {
            format!("// `{name}` — GObject wrapper (non-owning handle).\n")
        } else {
            format!("// `{name}` — GObject wrapper (non-owning handle); parent `{parent}`.\n")
        };

        // Wrapper method names are scoped to this impl (not the global fn space),
        // so a fresh set per class; raw/from_raw are always present.
        let mut methods: HashSet<String> = HashSet::new();
        methods.insert("raw".to_string());
        methods.insert("from_raw".to_string());

        let mut body = String::new();
        body.push_str("    fn raw(this) -> *u8 { return this._raw; }\n");
        body.push_str(&format!("    fn from_raw(ptr: *u8) -> {ty} {{ return {ty} {{ _raw: ptr }}; }}\n\n"));

        for ctor in c.children_named("constructor") {
            self.emit_ctor(&ty, ctor, &mut methods, &mut body);
        }
        for m in c.children_named("method") {
            self.emit_method(name, m, &mut methods, &mut body);
        }
        for s in c.children_named("glib:signal") {
            self.emit_signal(&ty, s, &mut methods, &mut body);
        }

        // Safe upcasts. C+ has no struct inheritance, so a subclass wrapper does
        // not carry its parent's or interfaces' methods. But the handle *is* an
        // instance of each (a GtkButton* IS-A GtkWidget*), so expose zero-cost
        // `from_raw` views: `button.upcast().show()` reaches Widget's methods,
        // `box.as_orientable().set_orientation(..)` reaches an interface's. Only
        // in-namespace supertypes are bridged; a foreign parent (GObject.Object)
        // is left to `.raw()`.
        if let Some(parent) = c.attr("parent") {
            if !parent.contains('.') && self.wrapper_types.contains(parent) && methods.insert("upcast".to_string()) {
                let pty = ident_type(parent);
                body.push_str(&format!(
                    "    // upcast to parent `{parent}` (safe; the handle is-a {parent}).\n    fn upcast(this) -> {pty} {{ return {pty}::from_raw(this._raw); }}\n\n"
                ));
                self.emitted += 1;
            }
        }
        for imp in c.children_named("implements") {
            let iname = match imp.attr("name") {
                Some(n) if !n.contains('.') && self.wrapper_types.contains(n) => n,
                _ => continue,
            };
            let mname = ident(&format!("as_{}", snake(iname)));
            if methods.insert(mname.clone()) {
                let ity = ident_type(iname);
                body.push_str(&format!(
                    "    fn {mname}(this) -> {ity} {{ return {ity}::from_raw(this._raw); }}\n\n"
                ));
                self.emitted += 1;
            }
        }

        // struct + impl. Externs for this class were already pushed to self.out
        // by the emit_* calls above (module level, before the impl).
        self.out.push_str(&doc);
        self.out.push_str(&format!("struct {ty} {{\n    opaque _raw: *u8,\n}}\n\n"));
        self.out.push_str(&format!("impl {ty} {{\n{body}}}\n\n"));
    }

    fn emit_ctor(&mut self, ty: &str, ctor: &Node, methods: &mut HashSet<String>, body: &mut String) {
        let name = match ctor.attr("name") {
            Some(n) => n,
            None => return,
        };
        let cid = match ctor.attr("c:identifier") {
            Some(c) => c,
            None => return,
        };
        let wname = ident(name);
        if !methods.insert(wname.clone()) {
            self.skip("ctor", &format!("{ty}::{name}"), &format!("method name `{wname}` already defined"));
            return;
        }
        let params = match self.map_params(ctor, "ctor", &format!("{ty}::{name}")) {
            Some(p) => p,
            None => return,
        };
        // A constructor always yields an instance of its class; the declared
        // return is the base (`Widget`), so force the wrapper return to Self.
        let nullable = ctor
            .child_named("return-value")
            .and_then(|r| r.attr("nullable"))
            .map(|v| v == "1")
            .unwrap_or(false);
        let ret = Mapped { cat: Cat::Obj, extern_ty: "*u8".to_string(), obj: Some(ty.to_string()) };
        let (ext, wrap) = self.render(&wname, cid, None, &ret, false, nullable, &params);
        self.out.push_str(&ext);
        body.push_str(&wrap);
        self.emitted += 1;
    }

    fn emit_method(&mut self, class: &str, m: &Node, methods: &mut HashSet<String>, body: &mut String) {
        let name = match m.attr("name") {
            Some(n) => n,
            None => return,
        };
        let cid = match m.attr("c:identifier") {
            Some(c) => c,
            None => return,
        };
        let label = format!("{class}::{name}");
        let rv = match m.child_named("return-value") {
            Some(r) => r,
            None => {
                self.skip("method", &label, "no return-value");
                return;
            }
        };
        let ret_ty = rv.child_named("type");
        let ret = match ret_ty.and_then(|t| self.map(t)) {
            Some(m) => m,
            None => {
                self.skip("method", &label, &format!("return type — {}", type_reason(ret_ty)));
                return;
            }
        };
        let ret_full = matches!(rv.attr("transfer-ownership"), Some("full"));
        let ret_nullable = matches!(rv.attr("nullable"), Some("1"));
        let params = match self.map_params(m, "method", &label) {
            Some(p) => p,
            None => return,
        };
        let wname = ident(name);
        if !methods.insert(wname.clone()) {
            self.skip("method", &label, &format!("method name `{wname}` already defined"));
            return;
        }
        let (ext, wrap) = self.render(&wname, cid, Some("this._raw"), &ret, ret_full, ret_nullable, &params);
        self.out.push_str(&ext);
        body.push_str(&wrap);
        self.emitted += 1;
    }

    /// Map the non-instance parameters of a method/ctor/function. Returns None
    /// (after emitting a SKIP) if any param is variadic, out/inout, or an
    /// unmapped type — the whole callable is then skipped.
    fn map_params(&mut self, node: &Node, kind: &str, label: &str) -> Option<Vec<(String, Mapped)>> {
        let mut params: Vec<(String, Mapped)> = Vec::new();
        if let Some(ps) = node.child_named("parameters") {
            for p in ps.children_named("parameter") {
                if p.child_named("varargs").is_some() {
                    self.skip(kind, label, "variadic");
                    return None;
                }
                if matches!(p.attr("direction"), Some("out") | Some("inout")) {
                    self.skip(kind, label, "out/inout parameter");
                    return None;
                }
                let pty = p.child_named("type");
                let m = match pty.and_then(|t| self.map(t)) {
                    Some(m) if m.usable_as_param() => m,
                    _ => {
                        self.skip(kind, label, &format!("param `{}` — {}", p.attr("name").unwrap_or("?"), type_reason(pty)));
                        return None;
                    }
                };
                let pname = ident(p.attr("name").unwrap_or("arg"));
                params.push((pname, m));
            }
        }
        Some(params)
    }

    /// Bind a signal as `connect_<name>`. Slice 2 handles the two handler shapes
    /// vendor/gobject provides: a void handler `(instance, user_data)` and a
    /// gboolean handler; anything with extra signal args is skipped.
    fn emit_signal(&mut self, ty: &str, s: &Node, methods: &mut HashSet<String>, body: &mut String) {
        let sig_name = match s.attr("name") {
            Some(n) => n,
            None => return,
        };
        let label = format!("{ty}::signal {sig_name}");
        // Extra signal args (beyond the implicit instance + user_data) aren't
        // modelled yet.
        let has_extra_args = s
            .child_named("parameters")
            .map(|ps| ps.children_named("parameter").next().is_some())
            .unwrap_or(false);
        if has_extra_args {
            self.skip("signal", &label, "handler has extra arguments");
            return;
        }
        let ret_name = s
            .child_named("return-value")
            .and_then(|r| r.child_named("type"))
            .and_then(|t| t.attr("name"))
            .unwrap_or("none");
        let (helper, htype) = match ret_name {
            "none" => ("connect", "fn(*u8, *u8)"),
            "gboolean" => ("connect_bool", "fn(*u8, *u8) -> i32"),
            other => {
                self.skip("signal", &label, &format!("handler return `{other}` not modelled"));
                return;
            }
        };
        let wname = ident(&format!("connect_{}", sig_name.replace('-', "_")));
        if !methods.insert(wname.clone()) {
            self.skip("signal", &label, &format!("name `{wname}` already defined"));
            return;
        }
        body.push_str(&format!("    fn {wname}(this, handler: {htype}, user: *u8) -> u64 {{\n"));
        body.push_str(&format!(
            "        return sig::{helper}(this._raw, #str_ptr(\"{sig_name}\\0\"), handler, user);\n    }}\n\n"
        ));
        self.emitted += 1;
    }
}

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Cat {
    Void,
    Scalar,
    Bool,
    Str,
    Ptr,
    Obj,
}

#[derive(Clone)]
struct Mapped {
    cat: Cat,
    extern_ty: String,
    /// For `Cat::Obj`: the wrapper struct name (a class/interface in this
    /// namespace). None otherwise.
    obj: Option<String>,
}

impl Mapped {
    fn usable_as_param(&self) -> bool {
        self.cat != Cat::Void
    }
    /// Ergonomic parameter type (the type only — the `ref` binding-mode for
    /// object params is applied at the call site, before the parameter name).
    fn wrap_param_ty(&self) -> String {
        match self.cat {
            Cat::Str => "str".to_string(),
            Cat::Bool => "bool".to_string(),
            Cat::Obj => self.obj.clone().unwrap(),
            _ => self.extern_ty.clone(),
        }
    }
    fn wrap_ret_ty(&self, nullable: bool) -> String {
        match self.cat {
            Cat::Void => "()".to_string(),
            Cat::Bool => "bool".to_string(),
            Cat::Str => {
                if nullable {
                    "option::Option[text::Text]".to_string()
                } else {
                    "text::Text".to_string()
                }
            }
            Cat::Obj => {
                let o = self.obj.clone().unwrap();
                if nullable {
                    format!("option::Option[{o}]")
                } else {
                    o
                }
            }
            _ => self.extern_ty.clone(),
        }
    }
}

/// Map a `<type>` node against a scalar/string/pointer vocabulary. Object,
/// record, array, and callback types return None here; object resolution (into
/// a wrapper struct) is layered on in `Emitter::map`, which knows the namespace's
/// class set.
fn map_type(t: &Node) -> Option<Mapped> {
    let name = t.attr("name")?;
    let scalar = |s: &str| Some(Mapped { cat: Cat::Scalar, extern_ty: s.to_string(), obj: None });
    match name {
        "none" => Some(Mapped { cat: Cat::Void, extern_ty: "()".to_string(), obj: None }),
        "gboolean" => Some(Mapped { cat: Cat::Bool, extern_ty: "i32".to_string(), obj: None }),
        "utf8" | "filename" => Some(Mapped { cat: Cat::Str, extern_ty: "*u8".to_string(), obj: None }),
        "gpointer" => Some(Mapped { cat: Cat::Ptr, extern_ty: "*u8".to_string(), obj: None }),
        "gint" | "gint32" => scalar("i32"),
        "guint" | "guint32" => scalar("u32"),
        "gint8" | "gchar" => scalar("i8"),
        "guint8" | "guchar" => scalar("u8"),
        "gint16" => scalar("i16"),
        "guint16" => scalar("u16"),
        "gint64" | "glong" => scalar("i64"),
        "guint64" | "gulong" => scalar("u64"),
        "gsize" => scalar("usize"),
        "gssize" => scalar("isize"),
        "gfloat" => scalar("f32"),
        "gdouble" => scalar("f64"),
        "gunichar" => scalar("u32"),
        _ => None,
    }
}

/// Human reason for a SKIP, naming the offending GIR type.
fn type_reason(t: Option<&Node>) -> String {
    match t {
        Some(n) => match n.attr("name") {
            Some(nm) => format!("unmapped type `{nm}`"),
            None => "unnamed/complex type (array or callback)".to_string(),
        },
        None => "no <type> (array, callback, or varargs)".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// PascalCase / CamelCase -> snake_case (for enum-constant prefixes).
fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Make a C+-safe identifier: escape the reserved word set and leading digits.
fn ident(s: &str) -> String {
    let s = s.replace('-', "_");
    let safe = if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        format!("_{s}")
    } else {
        s
    };
    if is_keyword(&safe) {
        format!("{safe}_")
    } else {
        safe
    }
}

/// A wrapper struct type name from a GIR class/interface name. GObject class
/// names are already PascalCase (`Button`, `ApplicationWindow`); we only guard a
/// leading digit. Type names live in their own name space, so keywords don't
/// clash here — but a `_` suffix is added defensively for the rare keyword-cased
/// type.
fn ident_type(s: &str) -> String {
    let s = s.replace('-', "_");
    let s = if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        format!("_{s}")
    } else {
        s
    };
    if is_keyword(&s) {
        format!("{s}_")
    } else {
        s
    }
}

/// Strip up to four leading spaces from every line — turns an `impl`-indented
/// wrapper (from `render`) into a column-0 free function.
fn dedent4(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let trimmed = line.strip_prefix("    ").unwrap_or(line);
        out.push_str(trimmed);
    }
    out
}

/// Sanitize a C symbol (`c:identifier`) into the tail of an `extern fn` name.
/// C identifiers are already `[A-Za-z0-9_]`, so this is near-identity; it exists
/// to centralize the rule and guard anything unexpected.
fn sanitize_sym(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "fn" | "let" | "var" | "if" | "else" | "while" | "loop" | "for" | "return"
            | "break" | "continue" | "match" | "struct" | "enum" | "trait" | "impl"
            | "type" | "as" | "in" | "true" | "false" | "extern" | "import" | "pub"
            | "mut" | "ref" | "self" | "this" | "move" | "take" | "str" | "usize"
            | "isize" | "bool" | "unsafe" | "const" | "static" | "where" | "use"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r#"<?xml version="1.0"?>
<!-- a comment -->
<repository version="1.2">
  <namespace name="Test" c:identifier-prefixes="Test">
    <enumeration name="Color" c:type="TestColor">
      <member name="red" value="0" c:identifier="TEST_COLOR_RED"/>
      <member name="green" value="1" c:identifier="TEST_COLOR_GREEN"/>
    </enumeration>
    <function name="get_name" c:identifier="test_get_name">
      <return-value transfer-ownership="full">
        <type name="utf8" c:type="char*"/>
      </return-value>
    </function>
    <function name="set_count" c:identifier="test_set_count">
      <return-value transfer-ownership="none"><type name="none"/></return-value>
      <parameters>
        <parameter name="n" transfer-ownership="none"><type name="gint" c:type="gint"/></parameter>
      </parameters>
    </function>
    <function name="take_widget" c:identifier="test_take_widget">
      <return-value><type name="none"/></return-value>
      <parameters>
        <parameter name="w"><type name="Widget" c:type="GtkWidget*"/></parameter>
      </parameters>
    </function>
  </namespace>
</repository>"#;

    fn emit(src: &str) -> String {
        let root = parse(src);
        let repo = root.child_named("repository").unwrap();
        let ns = repo.child_named("namespace").unwrap();
        Emitter::new(ns, "test.gir").run()
    }

    #[test]
    fn parser_builds_tree_with_attrs_and_nesting() {
        let root = parse(MINI);
        let repo = root.child_named("repository").unwrap();
        assert_eq!(repo.attr("version"), Some("1.2"));
        let ns = repo.child_named("namespace").unwrap();
        assert_eq!(ns.attr("name"), Some("Test"));
        // namespaced attribute key preserved verbatim
        assert_eq!(ns.attr("c:identifier-prefixes"), Some("Test"));
        let f = ns.children_named("function").next().unwrap();
        assert_eq!(f.attr("c:identifier"), Some("test_get_name"));
        // nested return-value > type
        let ty = f.child_named("return-value").unwrap().child_named("type").unwrap();
        assert_eq!(ty.attr("name"), Some("utf8"));
    }

    #[test]
    fn shared_library_maps_to_link_libs() {
        assert_eq!(so_to_lib("libgtk-4.so.1"), Some("gtk-4".to_string()));
        assert_eq!(so_to_lib("libglib-2.0.so.0"), Some("glib-2.0".to_string()));
        assert_eq!(so_to_lib("libadwaita-1.so.0"), Some("adwaita-1".to_string()));
        let root = parse(r#"<namespace shared-library="libgtk-4.so.1,libc.so.6"/>"#);
        let ns = root.child_named("namespace").unwrap();
        assert_eq!(link_libs(ns), vec!["gtk-4".to_string(), "c".to_string()]);
    }

    #[test]
    fn entities_are_decoded_in_attr_values() {
        let n = parse(r#"<a v="x &lt; y &amp; z &#65;"/>"#);
        assert_eq!(n.child_named("a").unwrap().attr("v"), Some("x < y & z A"));
    }

    #[test]
    fn transfer_full_string_return_uses_full_bridge() {
        let out = emit(MINI);
        assert!(out.contains("#[link_name = \"test_get_name\"]"));
        assert!(out.contains("fn get_name() -> text::Text"));
        assert!(out.contains("bridge::cstr_to_text_full(__r)"));
    }

    #[test]
    fn scalar_param_and_void_return() {
        let out = emit(MINI);
        assert!(out.contains("fn set_count(n: i32) {"));
        assert!(out.contains("#[link_name = \"test_set_count\"]"));
    }

    #[test]
    fn enum_members_become_constant_fns() {
        let out = emit(MINI);
        assert!(out.contains("fn color_red() -> i32 { return 0 as i32; }"));
        assert!(out.contains("fn color_green() -> i32 { return 1 as i32; }"));
    }

    #[test]
    fn enum_typed_param_binds_as_integer() {
        // a function taking the in-namespace enum `Color` binds it as i32 (the
        // ABI integer), not a SKIP — callers pass `color_red()` etc.
        let src = r#"<repository><namespace name="Test">
          <enumeration name="Color" c:type="TestColor">
            <member name="red" value="0" c:identifier="TEST_COLOR_RED"/>
          </enumeration>
          <function name="paint" c:identifier="test_paint">
            <return-value><type name="Color"/></return-value>
            <parameters><parameter name="c"><type name="Color"/></parameter></parameters>
          </function></namespace></repository>"#;
        let out = emit(src);
        assert!(out.contains("fn paint(c: i32) -> i32 {"));
        assert!(out.contains("extern fn __c_test_paint(c: i32) -> i32"));
    }

    #[test]
    fn unmapped_object_param_is_skipped_not_wrong() {
        let out = emit(MINI);
        assert!(out.contains("// SKIPPED fn `take_widget`"));
        assert!(out.contains("unmapped type `Widget`"));
        // never emit a wrapper for the skipped function
        assert!(!out.contains("fn take_widget("));
    }

    const CLASSES: &str = r#"<repository><namespace name="Gtk">
      <interface name="Editable"/>
      <class name="Widget" c:type="GtkWidget" glib:type-name="GtkWidget">
        <method name="set_visible" c:identifier="gtk_widget_set_visible">
          <return-value><type name="none"/></return-value>
          <parameters>
            <instance-parameter name="widget"><type name="Widget"/></instance-parameter>
            <parameter name="visible"><type name="gboolean"/></parameter>
          </parameters>
        </method>
        <glib:signal name="destroy"><return-value><type name="none"/></return-value></glib:signal>
      </class>
      <class name="Button" c:type="GtkButton" parent="Widget" glib:type-name="GtkButton">
        <implements name="Editable"/>
        <constructor name="new_with_label" c:identifier="gtk_button_new_with_label">
          <return-value transfer-ownership="none"><type name="Widget" c:type="GtkWidget*"/></return-value>
          <parameters><parameter name="label"><type name="utf8" c:type="const char*"/></parameter></parameters>
        </constructor>
        <method name="get_child" c:identifier="gtk_button_get_child">
          <return-value transfer-ownership="none" nullable="1"><type name="Widget" c:type="GtkWidget*"/></return-value>
          <parameters><instance-parameter name="button"><type name="Button"/></instance-parameter></parameters>
        </method>
        <method name="set_child" c:identifier="gtk_button_set_child">
          <return-value><type name="none"/></return-value>
          <parameters>
            <instance-parameter name="button"><type name="Button"/></instance-parameter>
            <parameter name="child"><type name="Widget" c:type="GtkWidget*"/></parameter>
          </parameters>
        </method>
        <glib:signal name="clicked"><return-value><type name="none"/></return-value></glib:signal>
      </class></namespace></repository>"#;

    #[test]
    fn class_becomes_wrapper_struct_with_raw() {
        let out = emit(CLASSES);
        assert!(out.contains("struct Button {\n    opaque _raw: *u8,\n}"));
        assert!(out.contains("impl Button {"));
        assert!(out.contains("fn raw(this) -> *u8 { return this._raw; }"));
        assert!(out.contains("fn from_raw(ptr: *u8) -> Button"));
    }

    #[test]
    fn constructor_returns_self_not_declared_base() {
        // gtk_button_new_with_label is declared to return Widget; the wrapper
        // must return Button (Self), wrapping via from_raw.
        let out = emit(CLASSES);
        assert!(out.contains("fn new_with_label(label: str) -> Button {"));
        assert!(out.contains("return Button::from_raw(__r);"));
        assert!(out.contains("#[link_name = \"gtk_button_new_with_label\"]"));
    }

    #[test]
    fn method_has_receiver_and_marshals_bool() {
        let out = emit(CLASSES);
        // instance-parameter is dropped; bool param becomes a `bool` wrapper arg
        assert!(out.contains("fn set_visible(this, visible: bool) {"));
        assert!(out.contains("extern fn __c_gtk_widget_set_visible(__recv: *u8, visible: i32)"));
        assert!(out.contains("{ __c_gtk_widget_set_visible(this._raw, (if visible { 1 as i32 } else { 0 as i32 })) }"));
    }

    #[test]
    fn object_return_wraps_and_nullable_is_option() {
        let out = emit(CLASSES);
        // get_child returns nullable Widget -> Option[Widget], wrapped via from_raw
        assert!(out.contains("fn get_child(this) -> option::Option[Widget] {"));
        assert!(out.contains("return option::some(Widget::from_raw(__r));"));
    }

    #[test]
    fn object_param_binds_by_ref_and_passes_raw() {
        let out = emit(CLASSES);
        assert!(out.contains("fn set_child(this, ref child: Widget) {"));
        assert!(out.contains("__c_gtk_button_set_child(this._raw, child.raw())"));
    }

    #[test]
    fn signals_become_connect_helpers() {
        let out = emit(CLASSES);
        assert!(out.contains("fn connect_clicked(this, handler: fn(*u8, *u8), user: *u8) -> u64 {"));
        assert!(out.contains("sig::connect(this._raw, #str_ptr(\"clicked\\0\"), handler, user)"));
    }

    #[test]
    fn upcast_to_parent_and_interface_views() {
        let out = emit(CLASSES);
        // Button parent Widget -> upcast(); implements Editable -> as_editable()
        assert!(out.contains("fn upcast(this) -> Widget { return Widget::from_raw(this._raw); }"));
        assert!(out.contains("fn as_editable(this) -> Editable { return Editable::from_raw(this._raw); }"));
        // Widget itself has no in-namespace parent, so no upcast on it
        let widget_impl = out.split("impl Widget {").nth(1).unwrap().split("impl ").next().unwrap();
        assert!(!widget_impl.contains("fn upcast("));
    }

    #[test]
    fn interface_referenced_as_type_is_emitted() {
        // Editable is an interface with no methods; referencing it must still
        // resolve (it is emitted as a wrapper struct).
        let out = emit(CLASSES);
        assert!(out.contains("struct Editable {"));
    }

    #[test]
    fn reserved_names_do_not_collide() {
        // a namespace function literally named `free` must be skipped, not
        // emitted (it would clash with vendor/gobject's `free`).
        let src = r#"<repository><namespace name="T">
          <function name="free" c:identifier="t_free">
            <return-value><type name="none"/></return-value>
            <parameters><parameter name="p"><type name="gpointer"/></parameter></parameters>
          </function></namespace></repository>"#;
        let out = emit(src);
        assert!(out.contains("// SKIPPED fn `free`"));
        assert!(!out.contains("\nfn free("));
    }
}
