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
        // `arg` may be a bare namespace ("Gtk"); match the first `arg-*.gir`.
        if let Ok(rd) = std::fs::read_dir(d) {
            let mut hits: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(&format!("{arg}-")) && n.ends_with(".gir"))
                        .unwrap_or(false)
                })
                .collect();
            hits.sort();
            if let Some(h) = hits.into_iter().next() {
                return Some(h);
            }
        }
    }
    None
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

struct Emitter<'a> {
    ns: &'a Node,
    ns_name: String,
    out: String,
    skips: usize,
    emitted: usize,
    /// Bare wrapper names already defined (this module + reserved imports), so a
    /// later collision becomes a SKIP instead of an E0301.
    seen: HashSet<String>,
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
        Emitter { ns, ns_name, out, skips: 0, emitted: 0, seen }
    }

    /// Reserve a wrapper name; returns false if it was already taken (caller
    /// should SKIP). Reserved-import names are pre-seeded, so this also rejects
    /// collisions with `vendor/gobject` / `stdlib`.
    fn claim(&mut self, name: &str) -> bool {
        self.seen.insert(name.to_string())
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
        let ret = match ret_ty.and_then(|t| map_type(t)) {
            Some(m) => m,
            None => {
                self.skip("fn", name, &format!("return type — {}", type_reason(ret_ty)));
                return;
            }
        };
        let ret_full = matches!(rv.attr("transfer-ownership"), Some("full"));
        let ret_nullable = matches!(rv.attr("nullable"), Some("1"));

        // Parameters.
        let mut params: Vec<(String, Mapped)> = Vec::new();
        if let Some(ps) = f.child_named("parameters") {
            for p in ps.children_named("parameter") {
                if p.child_named("varargs").is_some() {
                    self.skip("fn", name, "variadic");
                    return;
                }
                if matches!(p.attr("direction"), Some("out") | Some("inout")) {
                    self.skip("fn", name, "out/inout parameter");
                    return;
                }
                let pty = p.child_named("type");
                let m = match pty.and_then(|t| map_type(t)) {
                    Some(m) if m.usable_as_param() => m,
                    _ => {
                        self.skip("fn", name, &format!("param `{}` — {}", p.attr("name").unwrap_or("?"), type_reason(pty)));
                        return;
                    }
                };
                let pname = ident(p.attr("name").unwrap_or("arg"));
                params.push((pname, m));
            }
        }

        // C+ free functions share one global name space; a wrapper whose bare
        // name is already taken (a sibling function or a reserved import symbol)
        // can't be redefined.
        let wname = ident(name);
        if !self.claim(&wname) {
            self.skip("fn", name, &format!("name `{wname}` already defined (would collide with a sibling or `vendor/gobject`/`stdlib`)"));
            return;
        }
        self.render_fn(name, cid, &ret, ret_full, ret_nullable, &params);
        self.emitted += 1;
    }

    /// Emit the `extern fn` + ergonomic wrapper for a fully-mapped function.
    fn render_fn(&mut self, name: &str, cid: &str, ret: &Mapped, ret_full: bool, ret_nullable: bool, params: &[(String, Mapped)]) {
        let wname = ident(name);
        // extern signature (wire types).
        let ext_params: Vec<String> = params.iter().map(|(n, m)| format!("{n}: {}", m.extern_ty)).collect();
        let ext_ret = if ret.extern_ty == "()" { String::new() } else { format!(" -> {}", ret.extern_ty) };
        self.out.push_str(&format!("#[link_name = \"{cid}\"]\n"));
        self.out.push_str(&format!("extern fn __c_{wname}({}){ext_ret};\n", ext_params.join(", ")));

        // wrapper signature (ergonomic types).
        let wrap_params: Vec<String> = params.iter().map(|(n, m)| format!("{n}: {}", m.wrap_param_ty())).collect();
        let wrap_ret = ret.wrap_ret_ty(ret_nullable);
        let wrap_ret_sig = if wrap_ret == "()" { String::new() } else { format!(" -> {wrap_ret}") };
        self.out.push_str(&format!("fn {wname}({}){wrap_ret_sig} {{\n", wrap_params.join(", ")));

        // marshal string params in, remember cstrings to free after the call.
        let mut call_args: Vec<String> = Vec::new();
        let mut frees: Vec<String> = Vec::new();
        for (n, m) in params {
            match m.cat {
                Cat::Str => {
                    self.out.push_str(&format!("    let __cs_{n}: *u8 = bridge::str_to_cstring({n});\n"));
                    call_args.push(format!("__cs_{n}"));
                    frees.push(format!("__cs_{n}"));
                }
                Cat::Bool => call_args.push(format!("(if {n} {{ 1 as i32 }} else {{ 0 as i32 }})")),
                _ => call_args.push(n.clone()),
            }
        }
        let call = format!("{{ __c_{wname}({}) }}", call_args.join(", "));

        // Invoke + convert the return, freeing any temp cstrings first.
        let emit_frees = |out: &mut String| {
            for fr in &frees {
                out.push_str(&format!("    bridge::free_cstring({fr});\n"));
            }
        };
        match ret.cat {
            Cat::Void => {
                self.out.push_str(&format!("    {call};\n"));
                emit_frees(&mut self.out);
                self.out.push_str("    return;\n");
            }
            Cat::Str => {
                self.out.push_str(&format!("    let __r: *u8 = {call};\n"));
                emit_frees(&mut self.out);
                let conv = if ret_full { "bridge::cstr_to_text_full(__r)" } else { "bridge::cstr_to_text(__r)" };
                if ret_nullable {
                    self.out.push_str("    if __r == { 0 as *u8 } { return option::Option[text::Text]::None; }\n");
                    self.out.push_str(&format!("    return option::some({conv});\n"));
                } else {
                    self.out.push_str(&format!("    return {conv};\n"));
                }
            }
            Cat::Bool => {
                self.out.push_str(&format!("    let __r: i32 = {call};\n"));
                emit_frees(&mut self.out);
                self.out.push_str("    return __r != (0 as i32);\n");
            }
            _ => {
                self.out.push_str(&format!("    let __r: {} = {call};\n", ret.extern_ty));
                emit_frees(&mut self.out);
                self.out.push_str("    return __r;\n");
            }
        }
        self.out.push_str("}\n\n");
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
}

#[derive(Clone)]
struct Mapped {
    cat: Cat,
    extern_ty: String,
}

impl Mapped {
    fn usable_as_param(&self) -> bool {
        self.cat != Cat::Void
    }
    fn wrap_param_ty(&self) -> String {
        match self.cat {
            Cat::Str => "str".to_string(),
            Cat::Bool => "bool".to_string(),
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
            _ => self.extern_ty.clone(),
        }
    }
}

/// Map a `<type>` node to a wire/ergonomic pair, or None if unmodelled (slice 1:
/// scalars, bool, strings, gpointer, void). Object/record/array/callback types
/// return None and become SKIPs until the class-graph pass lands.
fn map_type(t: &Node) -> Option<Mapped> {
    let name = t.attr("name")?;
    let scalar = |s: &str| Some(Mapped { cat: Cat::Scalar, extern_ty: s.to_string() });
    match name {
        "none" => Some(Mapped { cat: Cat::Void, extern_ty: "()".to_string() }),
        "gboolean" => Some(Mapped { cat: Cat::Bool, extern_ty: "i32".to_string() }),
        "utf8" | "filename" => Some(Mapped { cat: Cat::Str, extern_ty: "*u8".to_string() }),
        "gpointer" => Some(Mapped { cat: Cat::Ptr, extern_ty: "*u8".to_string() }),
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
    fn unmapped_object_param_is_skipped_not_wrong() {
        let out = emit(MINI);
        assert!(out.contains("// SKIPPED fn `take_widget`"));
        assert!(out.contains("unmapped type `Widget`"));
        // never emit a wrapper for the skipped function
        assert!(!out.contains("fn take_widget("));
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
