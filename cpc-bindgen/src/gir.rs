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
        if inner.starts_with('/') {
            // Close tag: pop, attaching the finished node to its parent.
            // We stay lenient and don't verify the close-tag name matches.
            if stack.len() > 1 {
                let node = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(node);
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

/// The set of symbols a namespace's shared libraries actually EXPORT, plus a
/// human-readable list of the libraries consulted. `None` when the check cannot
/// be performed (no `shared-library` in the GIR, none of the libraries found, or
/// no `nm` on this host) — the caller then binds everything, as before.
///
/// WHY THIS EXISTS. A GIR is not a symbol table. `g-ir-scanner` records the API
/// a library DECLARES, from its headers and sources, and three kinds of entry
/// come out the other side with a `c:identifier` that no `.so` ever exports:
///
///   - a `static inline` function the library deliberately shows the scanner.
///     GTK does this on purpose and says so in `gtkenums.h`: under
///     `__GI_SCANNER__` it declares a prototype for `gtk_ordering_from_cmpfunc`
///     and otherwise defines it `static inline`.
///   - a deprecated function REMOVED from the library whose GIR entry stayed
///     (`g_thread_init`, gone since GLib 2.32).
///   - the plugin side of an ABI, declared in the host's headers because a
///     module must implement it (`g_io_module_load`), never defined by the host.
///
/// Binding one of those emits an `extern fn` for a symbol that does not exist.
/// Nothing catches it until link time — and because cpc emits ONE OBJECT PER
/// PACKAGE, the failure is not confined to the caller: any program that calls
/// ANY function in the package drags in the whole object and fails on the dead
/// reference. One bad binding breaks every consumer of the binding.
///
/// The GIR flag `introspectable="0"` is on all of these and is NOT a usable
/// filter on its own — across GLib/Gio/Gtk it marks 469 functions, of which 464
/// really do exist (variadics like `g_strdup_printf` carry it). Filtering on it
/// would delete 464 working bindings to catch 5 broken ones. The library's own
/// dynamic symbol table is the only ground truth, so that is what we read.
fn exported_symbols(ns: &Node) -> Option<(HashSet<String>, String)> {
    let attr = ns.attr("shared-library")?;
    let direct: Vec<String> = attr
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if direct.is_empty() {
        return None;
    }

    // The closure over DT_NEEDED, not just the libraries the GIR names. A
    // namespace routinely binds symbols that live in a sibling its own library
    // depends on: cairo's GIR declares only `libcairo-gobject.so.2`, while
    // `cairo_image_surface_create` is in `libcairo.so.2` underneath it. Reading
    // only the named library would call that function nonexistent and delete a
    // working binding — the one failure mode this check must not have, since a
    // false positive silently removes API and a false negative only restores
    // the status quo.
    let mut syms = HashSet::new();
    let mut queue: Vec<String> = direct.clone();
    let mut seen: HashSet<String> = HashSet::new();
    let mut read_any = false;
    while let Some(so) = queue.pop() {
        if !seen.insert(so.clone()) {
            continue;
        }
        let Some(path) = find_shared_library(&so) else {
            continue;
        };
        let Ok(out) = std::process::Command::new("nm")
            .args(["-D", "--defined-only", &path])
            .output()
        else {
            return None; // no `nm` here — disable rather than guess
        };
        if !out.status.success() {
            return None;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.split_whitespace();
            let count = line.split_whitespace().count();
            if count >= 3 {
                if let Some(name) = parts.next_back() {
                    syms.insert(name.split('@').next().unwrap_or(name).to_string());
                }
            }
        }
        read_any = true;
        // Walk one hop further. If the dependency list cannot be read the
        // closure is incomplete, and an incomplete closure produces exactly the
        // false positives described above — so give up on the whole check.
        let Ok(dp) = std::process::Command::new("objdump").args(["-p", &path]).output() else {
            return None;
        };
        if !dp.status.success() {
            return None;
        }
        for line in String::from_utf8_lossy(&dp.stdout).lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("NEEDED") {
                let need = rest.trim();
                if !need.is_empty() {
                    queue.push(need.to_string());
                }
            }
        }
    }
    if !read_any || syms.is_empty() {
        return None;
    }
    Some((syms, direct.join(", ")))
}

/// Locate a shared library by its soname (`libgtk-4.so.1`). Absolute or already
/// on the loader path in the usual multiarch places; `None` when it is not
/// installed, which is the normal case when generating on a host that does not
/// have the GNOME stack.
fn find_shared_library(so: &str) -> Option<String> {
    let p = PathBuf::from(so);
    if p.is_file() {
        return Some(so.to_string());
    }
    for d in [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/usr/local/lib",
    ] {
        let c = PathBuf::from(d).join(so);
        if c.is_file() {
            return Some(c.to_string_lossy().into_owned());
        }
    }
    None
}

/// Parse a GIR version stem (`"4.0"`, `"2.0"`) into comparable numeric parts;
/// non-numeric segments sort as 0 so a numeric compare orders 4.0 > 2.0 > 1.
fn parse_version(v: &str) -> Vec<u32> {
    v.split('.').map(|s| s.parse::<u32>().unwrap_or(0)).collect()
}

/// A foreign namespace mapped in via `--use`: which of its types are wrapper
/// (class/interface) types and which are enums, plus the C+ package that
/// provides them (the import alias).
struct Foreign {
    alias: String,
    classes: HashSet<String>,
    enums: HashMap<String, String>,
    records: HashSet<String>,
    /// Namespace `<alias>` typedefs that resolve to a plain scalar (e.g.
    /// `GLib.Quark` -> u32, `Pango.Glyph` -> u32). Only scalar targets are kept —
    /// an alias to a string/array/object (`GLib.Strv` is `gchar**`, not a string)
    /// would mis-bind, so those stay foreign SKIPs.
    aliases: HashMap<String, Mapped>,
}

/// Load the `--use NS=pkg` foreign registries by parsing each named GIR and
/// collecting its class/interface + enum sets. A namespace that can't be found
/// is skipped with a warning (its types stay SKIPs).
fn load_foreign(uses: &[(String, String)]) -> HashMap<String, Foreign> {
    let mut map = HashMap::new();
    for (ns_arg, pkg) in uses {
        let path = match find_gir_file(ns_arg) {
            Some(p) => p,
            None => {
                eprintln!("cpc-bindgen --gobject: --use {ns_arg}: GIR not found, its types stay SKIPs");
                continue;
            }
        };
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let root = parse(&src);
        let ns = match root.child_named("repository").and_then(|r| r.child_named("namespace")) {
            Some(n) => n,
            None => continue,
        };
        let ns_name = ns.attr("name").unwrap_or(ns_arg).to_string();
        let mut classes = HashSet::new();
        for c in ns.children_named("class").chain(ns.children_named("interface")) {
            if let Some(n) = c.attr("name") {
                classes.insert(n.to_string());
            }
        }
        let mut enums = HashMap::new();
        for e in ns.children_named("enumeration") {
            if let Some(n) = e.attr("name") {
                enums.insert(n.to_string(), "i32".to_string());
            }
        }
        for e in ns.children_named("bitfield") {
            if let Some(n) = e.attr("name") {
                enums.insert(n.to_string(), "u32".to_string());
            }
        }
        let mut records = HashSet::new();
        for r in ns.children_named("record") {
            if r.attr("glib:type-name").is_some() {
                if let Some(n) = r.attr("name") {
                    records.insert(n.to_string());
                }
            }
        }
        let mut aliases = HashMap::new();
        for a in ns.children_named("alias") {
            if let (Some(n), Some(ty)) = (a.attr("name"), a.child_named("type")) {
                if let Some(m) = map_type(ty) {
                    if matches!(m.cat, Cat::Scalar) {
                        aliases.insert(n.to_string(), m);
                    }
                }
            }
        }
        map.insert(ns_name, Foreign { alias: pkg.clone(), classes, enums, records, aliases });
    }
    map
}

pub fn generate(arg: &str, uses: &[(String, String)]) -> Result<String, String> {
    let path = find_gir_file(arg).ok_or_else(|| format!("cannot find GIR for `{arg}` (looked in /usr/share/gir-1.0 and the arch libdir)"))?;
    let src = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let foreign = load_foreign(uses);
    let root = parse(&src);
    let repo = root.child_named("repository").ok_or("no <repository> in GIR")?;
    let ns = repo.child_named("namespace").ok_or("no <namespace> in GIR")?;
    let mut em = Emitter::new(ns, &path.display().to_string(), foreign);
    Ok(em.run())
}

/// `--out DIR`: generate a whole C+ package (the GObject sibling of
/// `--framework`). Writes `DIR/src/<pkg>.cplus` (the bindings) and `DIR/Cplus.toml`
/// (deps on gobject + stdlib, `[link]` libs derived from the GIR
/// `shared-library`, and a provenance header). `<pkg>` is the output directory's
/// basename, so it satisfies the vendor "name matches directory" rule.
pub fn generate_package(arg: &str, out_dir: &str, uses: &[(String, String)]) -> Result<(), String> {
    let path = find_gir_file(arg).ok_or_else(|| format!("cannot find GIR for `{arg}`"))?;
    let src = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let foreign = load_foreign(uses);
    let root = parse(&src);
    let repo = root.child_named("repository").ok_or("no <repository> in GIR")?;
    let ns = repo.child_named("namespace").ok_or("no <namespace> in GIR")?;
    let ns_name = ns.attr("name").unwrap_or("Unknown").to_string();
    let ns_ver = ns.attr("version").unwrap_or("").to_string();
    let libs = link_libs(ns);

    let mut em = Emitter::new(ns, &path.display().to_string(), foreign);
    let module = em.run();
    let (emitted, skips) = (em.emitted, em.skips);
    let imported = em.imported.clone();

    let out = PathBuf::from(out_dir);
    let pkg = out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("--out DIR has no basename")?
        .to_string();
    let srcdir = out.join("src");
    std::fs::create_dir_all(&srcdir).map_err(|e| format!("mkdir {}: {e}", srcdir.display()))?;

    let libs_toml = libs.iter().map(|l| format!("\"{l}\"")).collect::<Vec<_>>().join(", ");
    // Only foreign packages actually imported (referenced) become dependencies —
    // a `--use` whose types never appear must not add a dangling dep.
    let mut foreign_deps = String::new();
    for dep_pkg in &imported {
        foreign_deps.push_str(&format!("{dep_pkg:<7} = \"*\"\n"));
    }
    let use_flags = uses
        .iter()
        .map(|(ns, pkg)| format!(" --use {ns}={pkg}"))
        .collect::<String>();
    let manifest = format!(
        "[package]\n\
         name    = \"{pkg}\"\n\
         version = \"0.0.1\"\n\
         edition = \"2026\"\n\n\
         # Auto-generated by cpc-bindgen --gobject.\n\
         # GIR:       {gir} (namespace {ns_name} {ns_ver})\n\
         # Reproduce: cpc-bindgen --gobject {arg} --out {out_dir}{use_flags}\n\
         # Coverage:  {emitted} items, {skips} SKIPPED (see `// SKIPPED` in src).\n\n\
         [dependencies]\n\
         gobject = \"*\"\n\
         stdlib  = \"*\"\n\
         {foreign_deps}\n\
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
    /// Foreign namespaces mapped in via `--use` (namespace name -> its wrapper/
    /// enum sets + import alias), so `Gtk.Widget` resolves to `gtk4::Widget`.
    foreign: HashMap<String, Foreign>,
    /// Symbols this namespace's shared libraries actually export, and the list
    /// of libraries read. `None` disables the check (library not installed, or
    /// no `nm`) — see `exported_symbols` for why the check exists at all.
    symbols: Option<(HashSet<String>, String)>,
    /// In-namespace boxed `<record>` names (those with a `glib:type-name`).
    /// They're bound as opaque handles, but only where the C ABI passes them by
    /// pointer (`c:type` ends with `*`) — a by-value value-struct must not become
    /// a handle.
    record_types: HashSet<String>,
    /// Foreign package aliases actually imported (referenced in the output) —
    /// the subset of `--use` that becomes a real dependency.
    imported: Vec<String>,
    /// Module-local `g_signal_connect_data` alias names already emitted (one per
    /// distinct arg-carrying handler shape), so they aren't re-declared.
    sig_shapes: HashSet<String>,
    /// In-namespace `<alias>` typedefs -> their inner `<type>` node. Resolved
    /// lazily (and transitively) in `map`, gated to scalar targets so a `bool_t`/
    /// `codepoint_t`/`Quark`-style typedef binds as its underlying integer.
    aliases: HashMap<String, &'a Node>,
}

impl<'a> Emitter<'a> {
    fn new(ns: &'a Node, source: &str, foreign: HashMap<String, Foreign>) -> Self {
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
        let mut record_types = HashSet::new();
        for r in ns.children_named("record") {
            if r.attr("glib:type-name").is_some() {
                if let Some(n) = r.attr("name") {
                    record_types.insert(n.to_string());
                }
            }
        }
        let mut aliases = HashMap::new();
        for a in ns.children_named("alias") {
            if let (Some(n), Some(ty)) = (a.attr("name"), a.child_named("type")) {
                aliases.insert(n.to_string(), ty);
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
            foreign,
            symbols: exported_symbols(ns),
            record_types,
            imported: Vec::new(),
            sig_shapes: HashSet::new(),
            aliases,
        }
    }

    /// The qualified C+ wrapper type for a GIR class/interface name, or None if
    /// it isn't a wrapper we bind. Handles both in-namespace (`Widget` ->
    /// `Widget`) and foreign (`Gtk.Widget` -> `gtk4::Widget`, when `--use`d).
    fn wrapper_type_of(&self, name: &str) -> Option<String> {
        if let Some((ns, local)) = name.split_once('.') {
            let f = self.foreign.get(ns)?;
            if f.classes.contains(local) {
                return Some(format!("{}::{}", f.alias, ident_type(local)));
            }
            return None;
        }
        if self.wrapper_types.contains(name) {
            return Some(ident_type(name));
        }
        None
    }

    /// The integer repr for a GIR enum/bitfield name (in-namespace or foreign
    /// via `--use`), or None if it isn't an enum we know.
    fn enum_repr_of(&self, name: &str) -> Option<String> {
        if let Some((ns, local)) = name.split_once('.') {
            return self.foreign.get(ns).and_then(|f| f.enums.get(local).cloned());
        }
        self.enum_types.get(name).cloned()
    }

    /// The qualified C+ wrapper type for a boxed `<record>` name (in-namespace or
    /// foreign via `--use`), or None. Only tells you it IS a record wrapper; the
    /// caller still gates on the use-site `c:type` being a pointer.
    fn record_type_of(&self, name: &str) -> Option<String> {
        if let Some((ns, local)) = name.split_once('.') {
            let f = self.foreign.get(ns)?;
            if f.records.contains(local) {
                return Some(format!("{}::{}", f.alias, ident_type(local)));
            }
            return None;
        }
        if self.record_types.contains(name) {
            return Some(ident_type(name));
        }
        None
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
        // Typedef aliases -> their underlying type, gated to scalars (an alias to
        // a string/array/object would mis-bind). In-namespace names resolve
        // (transitively) through the local `<alias>` table; a foreign `Ns.Local`
        // name resolves through that package's pre-computed scalar aliases.
        if let Some((ns, local)) = name.split_once('.') {
            if let Some(m) = self.foreign.get(ns).and_then(|f| f.aliases.get(local)) {
                return Some(m.clone());
            }
        } else if let Some(inner) = self.aliases.get(name) {
            if let Some(m) = self.map(inner) {
                if matches!(m.cat, Cat::Scalar) {
                    return Some(m);
                }
            }
        }
        // A few foreign scalar typedefs used pervasively across the stack.
        let foreign_scalar = match name {
            "GLib.Quark" => Some("u32"),
            "GObject.Type" | "GLib.Type" => Some("usize"),
            _ => None,
        };
        if let Some(s) = foreign_scalar {
            return Some(Mapped { cat: Cat::Scalar, extern_ty: s.to_string(), obj: None, record: false });
        }
        // Enum/bitfield (in-namespace or foreign) -> its ABI integer. Callers pass
        // the emitted constant fns (e.g. `orientation_horizontal()`).
        if let Some(repr) = self.enum_repr_of(name) {
            return Some(Mapped { cat: Cat::Scalar, extern_ty: repr, obj: None, record: false });
        }
        // Class/interface (in-namespace or foreign via --use) -> wrapper struct.
        if let Some(wt) = self.wrapper_type_of(name) {
            return Some(Mapped { cat: Cat::Obj, extern_ty: "*u8".to_string(), obj: Some(wt), record: false });
        }
        // Boxed record (in-namespace or foreign) -> opaque handle, but ONLY where
        // the ABI passes it by pointer. A by-value value-struct can't be a handle,
        // so it stays a SKIP.
        if ctype_is_pointer(t) {
            if let Some(rt) = self.record_type_of(name) {
                return Some(Mapped { cat: Cat::Obj, extern_ty: "*u8".to_string(), obj: Some(rt), record: true });
            }
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
        self.out.push_str("\n// === Boxed records ===\n\n");
        for r in self.ns.children_named("record") {
            self.emit_record(r);
        }
        self.out.push_str("\n// === Classes ===\n\n");
        for c in self.ns.children_named("class") {
            self.emit_class(c);
        }
        self.out.push_str(&format!(
            "\n// cpc-bindgen --gobject: {} items emitted, {} SKIPPED.\n",
            self.emitted, self.skips
        ));
        self.inject_foreign_imports();
        std::mem::take(&mut self.out)
    }

    /// A foreign `--use` package is imported only if its wrappers were actually
    /// referenced (`<alias>::` appears in the output). The imports are inserted
    /// after the fixed ones, sorted for byte-stable output.
    fn inject_foreign_imports(&mut self) {
        let mut aliases: Vec<&str> = self
            .foreign
            .values()
            .map(|f| f.alias.as_str())
            .filter(|a| self.out.contains(&format!("{a}::")))
            .collect();
        aliases.sort();
        aliases.dedup();
        self.imported = aliases.iter().map(|a| a.to_string()).collect();
        if aliases.is_empty() {
            return;
        }
        let imports: String = aliases
            .iter()
            .map(|a| format!("import \"{a}/{a}\" as {a};\n"))
            .collect();
        if let Some(pos) = self.out.find("\n// === ") {
            self.out.insert_str(pos, &format!("\n{imports}"));
        }
    }

    /// True (and records a SKIP) when `cid` is a symbol the shared library does
    /// not export — see `exported_symbols`. False when the check is disabled,
    /// so a host without the libraries generates exactly what it did before.
    fn symbol_missing(&mut self, kind: &str, label: &str, cid: &str) -> bool {
        let Some((syms, libs)) = &self.symbols else {
            return false;
        };
        if syms.contains(cid) {
            return false;
        }
        let libs = libs.clone();
        self.skip(kind, label, &format!("`{cid}` is not exported by {libs}"));
        true
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
        if self.symbol_missing("fn", name, cid) {
            return;
        }
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
        if !self.fold_gate("fn", name, &params, &ret) {
            return;
        }

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
        params: &[Param],
    ) -> (String, String) {
        let ext_name = format!("__c_{}", sanitize_sym(cid));

        // --- extern declaration (wire types) ---
        let mut ext_params: Vec<String> = Vec::new();
        if recv.is_some() {
            ext_params.push("__recv: *u8".to_string());
        }
        for p in params {
            // An out scalar is a pointer to the scalar in the C ABI.
            let ty = if p.out { format!("*{}", p.m.extern_ty) } else { p.m.extern_ty.clone() };
            ext_params.push(format!("{}: {ty}", p.name));
        }
        let ext_ret = if ret.extern_ty == "()" { String::new() } else { format!(" -> {}", ret.extern_ty) };
        let ext = format!(
            "#[link_name = \"{cid}\"]\nextern fn {ext_name}({}){ext_ret};\n",
            ext_params.join(", ")
        );

        // A single string/class-object out-param folds into the wrapper's return
        // (`fold_gate` guarantees at most one, and a void/gboolean callable
        // return). The out is dropped from the signature; its filled `T*`/`char*`
        // becomes `Option[T]`, `None` iff the callee left the slot NULL.
        let fold = params.iter().position(|p| p.out && is_foldable_out(&p.m));
        let fold_out = fold.map(|i| &params[i]);

        // --- wrapper signature (ergonomic types) ---
        let mut wrap_params: Vec<String> = Vec::new();
        if recv.is_some() {
            wrap_params.push("this".to_string());
        }
        for (i, p) in params.iter().enumerate() {
            if Some(i) == fold {
                continue; // folded into the return
            }
            // Out scalar -> `ref name: T` (C+ `ref` is a mutable write-back
            // borrow, exactly an out-param). Others take the wrapper/scalar by
            // value (object wrappers are non-owning handles, so the move is safe).
            if p.out {
                wrap_params.push(format!("ref {}: {}", p.name, p.m.wrap_param_ty()));
            } else {
                wrap_params.push(format!("{}: {}", p.name, p.m.wrap_param_ty()));
            }
        }
        // Return signature: a folded out overrides the callable's own return.
        let (fold_ret_ty, fold_none, fold_conv) = match fold_out {
            Some(p) => {
                let (t, conv) = match p.m.cat {
                    Cat::Str => {
                        let c = if p.full { "bridge::cstr_to_text_full(__out)" } else { "bridge::cstr_to_text(__out)" };
                        ("text::Text".to_string(), c.to_string())
                    }
                    _ => {
                        let o = p.m.obj.clone().unwrap();
                        (o.clone(), format!("{o}::from_raw(__out)"))
                    }
                };
                (Some(format!("option::Option[{t}]")), format!("option::Option[{t}]::None"), conv)
            }
            None => (None, String::new(), String::new()),
        };
        let wrap_ret = match &fold_ret_ty {
            Some(t) => t.clone(),
            None => ret.wrap_ret_ty(ret_nullable),
        };
        let wrap_ret_sig = if wrap_ret == "()" { String::new() } else { format!(" -> {wrap_ret}") };
        let mut w = format!("    fn {wname}({}){wrap_ret_sig} {{\n", wrap_params.join(", "));
        if fold.is_some() {
            w.push_str("        let __out: *u8 = { 0 as *u8 };\n");
        }

        // --- marshal params in ---
        let mut call_args: Vec<String> = Vec::new();
        if let Some(r) = recv {
            call_args.push(r.to_string());
        }
        let mut frees: Vec<String> = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let n = &p.name;
            if p.out {
                // Pass the address of the write-back slot — the folded out's local
                // `__out`, or the caller's `ref` place for a scalar out.
                let place = if Some(i) == fold { "__out" } else { n.as_str() };
                call_args.push(format!("({{ #addr_of({place}) as *{} }})", p.m.extern_ty));
                continue;
            }
            match p.m.cat {
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

        // --- folded out-param: convert `__out` into the wrapper's Option[T] ---
        if fold.is_some() {
            // The callable's own return (void, or a gboolean) is discarded;
            // presence is inferred from whether the callee filled the slot. The
            // slot starts NULL, so a function that leaves its out untouched on
            // failure yields `None`, while one that always writes it (e.g.
            // `g_get_charset`, whose gboolean reports "is UTF-8", not success)
            // yields `Some`. Gating on the gboolean instead would wrongly drop
            // that second class of result.
            w.push_str(&format!("        {call};\n"));
            emit_frees(&mut w);
            w.push_str(&format!("        if __out == {{ 0 as *u8 }} {{ return {fold_none}; }}\n"));
            w.push_str(&format!("        return option::some({fold_conv});\n"));
            w.push_str("    }\n\n");
            return (ext, w);
        }

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
            if let Some(pty) = self.wrapper_type_of(parent) {
                if methods.insert("upcast".to_string()) {
                    body.push_str(&format!(
                        "    // upcast to parent `{parent}` (safe; the handle is-a {parent}).\n    fn upcast(this) -> {pty} {{ return {pty}::from_raw(this._raw); }}\n\n"
                    ));
                    self.emitted += 1;
                }
            }
        }
        for imp in c.children_named("implements") {
            let iname = match imp.attr("name") {
                Some(n) => n,
                None => continue,
            };
            let ity = match self.wrapper_type_of(iname) {
                Some(t) => t,
                None => continue,
            };
            // as_<iface>: strip any namespace prefix for the method name.
            let local = iname.rsplit('.').next().unwrap_or(iname);
            let mname = ident(&format!("as_{}", snake(local)));
            if methods.insert(mname.clone()) {
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

    /// Bind a boxed `<record>` (e.g. GtkTextIter) as an opaque handle wrapper —
    /// like a class but with no parent/interfaces/signals. Only records with a
    /// `glib:type-name` reach here (plain value-structs are left as SKIPs, since
    /// treating a by-value struct as a handle would be ABI-wrong).
    fn emit_record(&mut self, r: &Node) {
        let name = match r.attr("name") {
            Some(n) => n,
            None => return,
        };
        if r.attr("glib:type-name").is_none() {
            return; // value struct, not a boxed handle
        }
        let ty = ident_type(name);
        if !self.seen_types.insert(ty.clone()) {
            self.skip("record", name, &format!("type `{ty}` already defined"));
            return;
        }
        let mut methods: HashSet<String> = HashSet::new();
        methods.insert("raw".to_string());
        methods.insert("from_raw".to_string());
        let mut body = String::new();
        body.push_str("    fn raw(this) -> *u8 { return this._raw; }\n");
        body.push_str(&format!("    fn from_raw(ptr: *u8) -> {ty} {{ return {ty} {{ _raw: ptr }}; }}\n\n"));
        for ctor in r.children_named("constructor") {
            self.emit_ctor(&ty, ctor, &mut methods, &mut body);
        }
        for m in r.children_named("method") {
            self.emit_method(name, m, &mut methods, &mut body);
        }
        self.out.push_str(&format!("// `{name}` — boxed record (non-owning handle).\n"));
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
        if self.symbol_missing("ctor", &format!("{ty}::{name}"), cid) {
            return;
        }
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
        let ret = Mapped { cat: Cat::Obj, extern_ty: "*u8".to_string(), obj: Some(ty.to_string()), record: false };
        if !self.fold_gate("ctor", &format!("{ty}::{name}"), &params, &ret) {
            return;
        }
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
        if self.symbol_missing("method", &label, cid) {
            return;
        }
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
        if !self.fold_gate("method", &label, &params, &ret) {
            return;
        }
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
    /// (after emitting a SKIP) if any param is variadic, inout, an unmodelled out
    /// param, or an unmapped type — the whole callable is then skipped.
    fn map_params(&mut self, node: &Node, kind: &str, label: &str) -> Option<Vec<Param>> {
        let mut params: Vec<Param> = Vec::new();
        if let Some(ps) = node.child_named("parameters") {
            for p in ps.children_named("parameter") {
                if p.child_named("varargs").is_some() {
                    self.skip(kind, label, "variadic");
                    return None;
                }
                let dir = p.attr("direction");
                if matches!(dir, Some("inout")) {
                    self.skip(kind, label, "inout parameter");
                    return None;
                }
                let is_out = matches!(dir, Some("out"));
                let pty = p.child_named("type");
                let m = match pty.and_then(|t| self.map(t)) {
                    Some(m) if m.usable_as_param() => m,
                    _ => {
                        self.skip(kind, label, &format!("param `{}` — {}", p.attr("name").unwrap_or("?"), type_reason(pty)));
                        return None;
                    }
                };
                // Out params: a plain scalar becomes `ref x: T` (write-back). A
                // string or class-object out (ABI `char**` / `T**`) is *foldable*
                // — the render pass turns a single such out into the wrapper's
                // `Option[T]` return (gated by `fold_gate`). Everything else (bool,
                // gpointer, or a boxed/value-struct record out — often a caller-
                // allocated `T*` we can't safely fill) skips the whole callable.
                if is_out && !is_foldable_out(&m) && m.cat != Cat::Scalar {
                    self.skip(kind, label, &format!("out parameter `{}` — non-foldable out ({})", p.attr("name").unwrap_or("?"), out_reason(&m)));
                    return None;
                }
                let pname = ident(p.attr("name").unwrap_or("arg"));
                let full = matches!(p.attr("transfer-ownership"), Some("full"));
                params.push(Param { name: pname, m, out: is_out, full });
            }
        }
        Some(params)
    }

    /// After both the return type and params are mapped, decide whether a
    /// callable carrying foldable out-param(s) can be emitted. A single foldable
    /// out folds into the wrapper's `Option[T]` return, but only when the callable
    /// has no competing real return value (void or a gboolean success flag). More
    /// than one foldable out, or one alongside a value return, is beyond what a
    /// single `Option[T]` can carry, so the callable is skipped. Returns true when
    /// OK to emit (emitting a SKIP itself when not).
    fn fold_gate(&mut self, kind: &str, label: &str, params: &[Param], ret: &Mapped) -> bool {
        let folds = params.iter().filter(|p| p.out && is_foldable_out(&p.m)).count();
        if folds == 0 {
            return true;
        }
        if folds > 1 {
            self.skip(kind, label, "multiple string/object out-params — only a single out folds into the return");
            return false;
        }
        if !matches!(ret.cat, Cat::Void | Cat::Bool) {
            self.skip(kind, label, "string/object out-param alongside a value return — cannot fold");
            return false;
        }
        true
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

        // Map the handler's return + extra args to their WIRE types (objects and
        // strings arrive as raw `*u8`, enums as ints). A handler is
        // `fn(instance, ...args..., user_data)`; the GIR <parameters> list only
        // the extra args. Anything unmappable (array/callback) skips the signal.
        // A handler arg/return arrives at its raw C-ABI wire type. A pointer arg
        // (a pointer `c:type`, `gpointer`, or an out/inout param — which GIR
        // spells `<type name="gint" c:type="gpointer"/>`) is passed as `*u8`, not
        // the pointee's by-value type; else the handler ABI is wrong.
        let wire = |ptr: bool, m: &Mapped| -> String {
            if ptr && !m.extern_ty.starts_with('*') {
                "*u8".to_string()
            } else {
                m.extern_ty.clone()
            }
        };
        let ret_wire = match s.child_named("return-value").and_then(|r| r.child_named("type")) {
            Some(t) => match self.map(t) {
                Some(m) => wire(ctype_is_pointer(t), &m),
                None => {
                    self.skip("signal", &label, "handler return not modelled");
                    return;
                }
            },
            None => "()".to_string(),
        };
        let mut arg_wires: Vec<String> = Vec::new();
        if let Some(ps) = s.child_named("parameters") {
            for p in ps.children_named("parameter") {
                let t = match p.child_named("type") {
                    Some(t) => t,
                    None => {
                        self.skip("signal", &label, "handler argument not modelled");
                        return;
                    }
                };
                let ptr = matches!(p.attr("direction"), Some("out") | Some("inout")) || ctype_is_pointer(t);
                match self.map(t) {
                    Some(m) if m.cat != Cat::Void => arg_wires.push(wire(ptr, &m)),
                    _ => {
                        self.skip("signal", &label, "handler argument not modelled");
                        return;
                    }
                }
            }
        }

        let wname = ident(&format!("connect_{}", sig_name.replace('-', "_")));
        if !methods.insert(wname.clone()) {
            self.skip("signal", &label, &format!("name `{wname}` already defined"));
            return;
        }

        // No extra args: use vendor/gobject's ready-made connect / connect_bool.
        if arg_wires.is_empty() && (ret_wire == "()" || ret_wire == "i32") {
            let (helper, htype) = if ret_wire == "()" {
                ("connect", "fn(*u8, *u8)".to_string())
            } else {
                ("connect_bool", "fn(*u8, *u8) -> i32".to_string())
            };
            body.push_str(&format!("    fn {wname}(this, handler: {htype}, user: *u8) -> u64 {{\n"));
            body.push_str(&format!(
                "        return sig::{helper}(this._raw, #str_ptr(\"{sig_name}\\0\"), handler, user);\n    }}\n\n"
            ));
            self.emitted += 1;
            return;
        }

        // Arg-carrying (or non-void/bool return) signal: emit a module-local
        // typed alias of g_signal_connect_data for this exact handler shape
        // (deduped across the module), then the connect wrapper.
        let ret_sig = if ret_wire == "()" { String::new() } else { format!(" -> {ret_wire}") };
        let mut handler_args = vec!["*u8".to_string()];
        handler_args.extend(arg_wires.iter().cloned());
        handler_args.push("*u8".to_string());
        let htype = format!("fn({}){ret_sig}", handler_args.join(", "));
        let key = sig_shape_key(&arg_wires, &ret_wire);
        let alias = format!("__sigc_{key}");
        if self.sig_shapes.insert(alias.clone()) {
            self.out.push_str(&format!(
                "#[link_name = \"g_signal_connect_data\"]\nextern fn {alias}(instance: *u8, detailed: *u8, handler: {htype}, data: *u8, destroy: *u8, flags: i32) -> u64;\n"
            ));
        }
        body.push_str(&format!("    fn {wname}(this, handler: {htype}, user: *u8) -> u64 {{\n"));
        body.push_str(&format!(
            "        return {{ {alias}(this._raw, #str_ptr(\"{sig_name}\\0\"), handler, user, {{ 0 as *u8 }}, 0 as i32) }};\n    }}\n\n"
        ));
        self.emitted += 1;
    }
}

/// A short, deterministic key for a signal handler shape (arg wire types + ret),
/// used to name+dedup the module-local `g_signal_connect_data` alias.
fn sig_shape_key(args: &[String], ret: &str) -> String {
    let tag = |t: &str| -> String {
        match t {
            "()" => "v".to_string(),
            "*u8" => "p".to_string(),
            _ => t.replace(|c: char| !c.is_ascii_alphanumeric(), ""),
        }
    };
    let a: String = args.iter().map(|t| tag(t)).collect::<Vec<_>>().join("");
    format!("{a}_{}", tag(ret))
}

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Default)]
enum Cat {
    #[default]
    Void,
    Scalar,
    Bool,
    Str,
    Ptr,
    Obj,
}

#[derive(Clone, Default)]
struct Mapped {
    cat: Cat,
    extern_ty: String,
    /// For `Cat::Obj`: the wrapper struct name (a class/interface in this
    /// namespace). None otherwise.
    obj: Option<String>,
    /// For `Cat::Obj`: true when the wrapper is a boxed `<record>` (opaque
    /// handle over a value-struct) rather than a `<class>`/`<interface>`. A
    /// class-object out-param is a real `T**` and can be folded into the return;
    /// a record out-param is often a caller-allocated value-struct (`T*`), so it
    /// must not be folded.
    record: bool,
}

/// One bound parameter: its name, mapped type, and whether it is an out
/// parameter (`direction="out"`), which becomes a `ref` write-back.
#[derive(Clone)]
struct Param {
    name: String,
    m: Mapped,
    out: bool,
    /// Out param with `transfer-ownership="full"` — a folded string out then
    /// takes ownership of the returned buffer (`cstr_to_text_full`).
    full: bool,
}

impl Mapped {
    fn usable_as_param(&self) -> bool {
        self.cat != Cat::Void
    }
    /// Ergonomic parameter type. Object params are passed by value (the wrapper
    /// is a non-owning handle; the callee just reads its `.raw()`).
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
    let scalar = |s: &str| Some(Mapped { cat: Cat::Scalar, extern_ty: s.to_string(), obj: None, record: false });
    match name {
        "none" => Some(Mapped { cat: Cat::Void, extern_ty: "()".to_string(), obj: None, record: false }),
        "gboolean" => Some(Mapped { cat: Cat::Bool, extern_ty: "i32".to_string(), obj: None, record: false }),
        "utf8" | "filename" => Some(Mapped { cat: Cat::Str, extern_ty: "*u8".to_string(), obj: None, record: false }),
        "gpointer" => Some(Mapped { cat: Cat::Ptr, extern_ty: "*u8".to_string(), obj: None, record: false }),
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
        "guintptr" => scalar("usize"),
        "gintptr" => scalar("isize"),
        "gfloat" => scalar("f32"),
        "gdouble" => scalar("f64"),
        "gunichar" => scalar("u32"),
        // Pervasive integer typedefs from the type system.
        "GType" => scalar("usize"),   // gsize-wide type id
        "GQuark" => scalar("u32"),    // interned-string id
        _ => None,
    }
}

/// True if a `<type>`'s `c:type` denotes a pointer — `GtkTextIter*`,
/// `const GtkTextIter*`, or the introspection spelling `gpointer`/`gconstpointer`
/// (used for e.g. an inout `gint*`). Absent `c:type` -> not a pointer (safe
/// default: skip rather than mis-bind a by-value struct as a handle).
fn ctype_is_pointer(t: &Node) -> bool {
    match t.attr("c:type") {
        Some(c) => {
            let c = c.trim_end();
            c.ends_with('*') || c == "gpointer" || c == "gconstpointer"
        }
        None => false,
    }
}

/// True if an out-param can be folded into the wrapper's return value: a string
/// (`char**`) or a real class/interface object (`T**`). A boxed/value-struct
/// record out is excluded — it is frequently a caller-allocated `T*` we cannot
/// fill through an 8-byte slot without corrupting the stack.
fn is_foldable_out(m: &Mapped) -> bool {
    matches!(m.cat, Cat::Str) || (matches!(m.cat, Cat::Obj) && !m.record)
}

/// Reason a non-scalar, non-foldable out-param can't be bound.
fn out_reason(m: &Mapped) -> &'static str {
    match m.cat {
        Cat::Obj => "value-struct/record out-param",
        Cat::Bool => "bool out-param",
        Cat::Ptr => "raw-pointer out-param",
        _ => "unsupported out-param",
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

/// Collapse every run of 2+ underscores to a single `_`. An interior `__` is
/// reserved by the compiler for monomorphization mangling (`Box[i32]` →
/// `Box__i32`), so NO generated C+ identifier may contain it or it fails E0917 —
/// e.g. GLib's `cclosure_marshal_BOOLEAN__BOXED_BOXED` marshaller wrappers,
/// which used to break the whole gobject/gtk stack at compile time. The extern
/// keeps the real C symbol via `#[link_name]` (and `__c_`-prefixed extern names
/// are E0917-exempt), so only the ergonomic wrapper identifier is normalized.
/// A rare collapse-collision (`a__b` vs `a_b`) is caught by the caller's
/// name-reservation dedup and SKIPped, not silently merged.
fn no_double_underscore(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_us = false;
    for ch in s.chars() {
        if ch == '_' {
            if !prev_us {
                out.push('_');
            }
            prev_us = true;
        } else {
            out.push(ch);
            prev_us = false;
        }
    }
    out
}

/// Make a C+-safe identifier: escape the reserved word set and leading digits,
/// and collapse reserved `__` runs (see `no_double_underscore`).
fn ident(s: &str) -> String {
    let s = s.replace('-', "_");
    let safe = if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        format!("_{s}")
    } else {
        s
    };
    let safe = if is_keyword(&safe) {
        format!("{safe}_")
    } else {
        safe
    };
    no_double_underscore(&safe)
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
    let s = if is_keyword(&s) {
        format!("{s}_")
    } else {
        s
    };
    no_double_underscore(&s)
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

/// The C+ reserved words (the lexer keyword set in `cplus-core/src/lexer.rs`).
/// A GIR name equal to one of these can't be an identifier, so it is escaped
/// with a trailing `_`.
fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "_" | "fn" | "let" | "mut" | "const" | "static" | "if" | "else" | "while"
            | "for" | "in" | "return" | "true" | "false" | "as" | "unsafe" | "extern"
            | "struct" | "enum" | "union" | "match" | "trait" | "impl" | "pub"
            | "export" | "use" | "mod" | "import" | "this" | "defer" | "try" | "break"
            | "continue" | "loop" | "move" | "restrict" | "guard" | "assert" | "borrow"
            | "opaque" | "interface" | "type" | "async" | "gen" | "yield" | "await"
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
        emit_with(src, HashMap::new())
    }

    fn emit_with(src: &str, foreign: HashMap<String, Foreign>) -> String {
        let root = parse(src);
        let repo = root.child_named("repository").unwrap();
        let ns = repo.child_named("namespace").unwrap();
        Emitter::new(ns, "test.gir", foreign).run()
    }

    /// A GIR whose `shared-library` is real, declaring one function that the
    /// library exports and one it does not. `strlen` is in every libc; the
    /// other name cannot be.
    const REAL_LIB: &str = r#"<?xml version="1.0"?>
<repository>
  <namespace name="Probe" version="1.0" shared-library="libc.so.6" c:identifier-prefixes="Probe">
    <function name="len" c:identifier="strlen">
      <return-value><type name="guint64" c:type="gsize"/></return-value>
      <parameters><parameter name="s"><type name="guint64" c:type="gsize"/></parameter></parameters>
    </function>
    <function name="ghost" c:identifier="probe_symbol_that_cannot_exist_9f3a">
      <return-value><type name="guint64" c:type="gsize"/></return-value>
    </function>
  </namespace>
</repository>"#;

    #[test]
    fn a_c_identifier_the_library_does_not_export_is_skipped() {
        // THE BUG THIS CLOSES: a GIR declares API, not symbols. GTK shows the
        // scanner a prototype for `gtk_ordering_from_cmpfunc` and then defines
        // it `static inline`; GLib kept `g_thread_init` in the GIR after
        // deleting it; Gio declares `g_io_module_load`, which modules
        // implement and libgio never defines. Each bound to an `extern fn` for
        // a symbol that is not there, and since cpc emits one object per
        // package, ONE of them fails the link of every program that calls
        // anything in the package.
        if find_shared_library("libc.so.6").is_none() {
            return; // not a glibc host; the check is disabled there anyway
        }
        let out = emit(REAL_LIB);
        assert!(
            out.contains("// SKIPPED fn `ghost`"),
            "a non-exported c:identifier must be skipped; got:\n{out}"
        );
        assert!(
            out.contains("is not exported by libc.so.6"),
            "the skip must name the library it consulted; got:\n{out}"
        );
        // ...and it must not be BOUND — the name may appear in the skip
        // comment (that is the point of the comment), but never as a
        // `#[link_name]` extern or an ergonomic wrapper.
        assert!(!out.contains("#[link_name = \"probe_symbol_that_cannot_exist_9f3a\"]"));
        assert!(!out.contains("fn ghost("));
        // The real one is unaffected — this filter must not cost coverage.
        assert!(out.contains("strlen"), "an exported symbol must still bind; got:\n{out}");
    }

    #[test]
    fn the_symbol_check_disables_itself_when_it_cannot_run() {
        // Generation must not depend on the target's libraries being installed
        // on the GENERATOR's host: cpc-bindgen runs on macOS against GIRs that
        // describe Linux libraries. With no `shared-library`, or one that is
        // not installed, every declaration binds exactly as it did before.
        let root = parse(MINI);
        let ns = root
            .child_named("repository")
            .unwrap()
            .child_named("namespace")
            .unwrap();
        assert!(exported_symbols(ns).is_none(), "no shared-library => no check");

        let absent = MINI.replace(
            "<namespace name=\"Test\"",
            "<namespace shared-library=\"libnot_installed_9f3a.so.7\" name=\"Test\"",
        );
        let root = parse(&absent);
        let ns = root
            .child_named("repository")
            .unwrap()
            .child_named("namespace")
            .unwrap();
        assert!(
            exported_symbols(ns).is_none(),
            "an uninstalled library must disable the check, not skip everything"
        );
        // and the emitted output is unchanged by its presence
        assert_eq!(emit(&absent), emit(MINI));
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
    fn wrapper_idents_never_contain_reserved_double_underscore() {
        // E0917: a generated C+ identifier must not contain interior `__`
        // (reserved for monomorphization mangling). GLib marshaller names like
        // `cclosure_marshal_BOOLEAN__BOXED_BOXED` used to leak `__` into the
        // wrapper fn name and break the whole gobject/gtk stack at compile time.
        assert_eq!(
            ident("cclosure_marshal_BOOLEAN__BOXED_BOXED"),
            "cclosure_marshal_BOOLEAN_BOXED_BOXED"
        );
        assert_eq!(no_double_underscore("a___b"), "a_b");
        assert_eq!(no_double_underscore("a__b__c"), "a_b_c");
        assert_eq!(no_double_underscore("plain_name"), "plain_name");
        assert_eq!(ident_type("Foo__Bar"), "Foo_Bar");
        // keyword escape + collapse compose without producing `__`.
        assert!(!ident("type").contains("__"));
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
    fn object_param_binds_by_value_and_passes_raw() {
        let out = emit(CLASSES);
        assert!(out.contains("fn set_child(this, child: Widget) {"));
        assert!(out.contains("__c_gtk_button_set_child(this._raw, child.raw())"));
    }

    #[test]
    fn signals_become_connect_helpers() {
        let out = emit(CLASSES);
        assert!(out.contains("fn connect_clicked(this, handler: fn(*u8, *u8), user: *u8) -> u64 {"));
        assert!(out.contains("sig::connect(this._raw, #str_ptr(\"clicked\\0\"), handler, user)"));
    }

    #[test]
    fn signal_with_args_emits_typed_handler_and_alias() {
        let src = r#"<repository><namespace name="Gtk">
          <class name="View" c:type="GtkView" glib:type-name="GtkView">
            <glib:signal name="moved">
              <return-value><type name="gboolean"/></return-value>
              <parameters>
                <parameter name="steps"><type name="gint" c:type="gint"/></parameter>
                <parameter name="pos" direction="inout"><type name="gint" c:type="gpointer"/></parameter>
              </parameters>
            </glib:signal>
          </class></namespace></repository>"#;
        let out = emit(src);
        // scalar arg stays i32, the inout pointer arg becomes *u8, gboolean ret -> i32
        assert!(out.contains("fn connect_moved(this, handler: fn(*u8, i32, *u8, *u8) -> i32, user: *u8) -> u64 {"));
        // a module-local typed alias of g_signal_connect_data for this shape
        assert!(out.contains("#[link_name = \"g_signal_connect_data\"]"));
        assert!(out.contains("extern fn __sigc_i32p_i32(instance: *u8, detailed: *u8, handler: fn(*u8, i32, *u8, *u8) -> i32"));
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
    fn foreign_record_resolves_by_pointer() {
        let mut foreign = HashMap::new();
        foreign.insert(
            "Gdk".to_string(),
            Foreign {
                alias: "gdk".to_string(),
                classes: HashSet::new(),
                enums: HashMap::new(),
                records: ["Rectangle".to_string()].into_iter().collect(),
                aliases: HashMap::new(),
            },
        );
        let src = r#"<repository><namespace name="Gtk">
          <class name="Widget" c:type="GtkWidget" glib:type-name="GtkWidget">
            <method name="set_clip" c:identifier="gtk_widget_set_clip">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="w"><type name="Widget"/></instance-parameter>
                <parameter name="clip"><type name="Gdk.Rectangle" c:type="const GdkRectangle*"/></parameter>
              </parameters>
            </method>
          </class></namespace></repository>"#;
        let out = emit_with(src, foreign);
        // foreign record by pointer -> `gdk::Rectangle` (by value at call, .raw() passed)
        assert!(out.contains("fn set_clip(this, clip: gdk::Rectangle) {"));
        assert!(out.contains("gtk_widget_set_clip(this._raw, clip.raw())"));
        assert!(out.contains("import \"gdk/gdk\" as gdk;"));
    }

    #[test]
    fn scalar_out_param_becomes_ref_writeback() {
        let src = r#"<repository><namespace name="Gtk">
          <class name="Widget" c:type="GtkWidget" glib:type-name="GtkWidget">
            <method name="get_size_request" c:identifier="gtk_widget_get_size_request">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="w"><type name="Widget"/></instance-parameter>
                <parameter name="width" direction="out"><type name="gint" c:type="gint*"/></parameter>
                <parameter name="height" direction="out"><type name="gint" c:type="gint*"/></parameter>
              </parameters>
            </method>
            <method name="get_name" c:identifier="gtk_widget_get_name_out">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="w"><type name="Widget"/></instance-parameter>
                <parameter name="name" direction="out"><type name="utf8" c:type="char**"/></parameter>
              </parameters>
            </method>
          </class></namespace></repository>"#;
        let out = emit(src);
        // scalar out -> `ref name: T`, extern takes `*T`, call passes #addr_of
        assert!(out.contains("fn get_size_request(this, ref width: i32, ref height: i32) {"));
        assert!(out.contains("extern fn __c_gtk_widget_get_size_request(__recv: *u8, width: *i32, height: *i32)"));
        assert!(out.contains("#addr_of(width) as *i32"));
        // a single string out folds into the return: `-> Option[Text]`, the out
        // drops from the signature, and a NULL slot yields None.
        assert!(out.contains("fn get_name(this) -> option::Option[text::Text] {"));
        assert!(out.contains("extern fn __c_gtk_widget_get_name_out(__recv: *u8, name: **u8)"));
        assert!(out.contains("let __out: *u8 = { 0 as *u8 };"));
        assert!(out.contains("if __out == { 0 as *u8 } { return option::Option[text::Text]::None; }"));
    }

    #[test]
    fn scalar_alias_resolves_to_underlying_type() {
        // A namespace `<alias>` to a fundamental scalar (`hb_codepoint_t` ->
        // guint32) resolves so params/returns of the alias type bind. An alias to
        // a non-scalar (a string) must NOT resolve (it would mis-bind).
        let src = r#"<repository><namespace name="HarfBuzz">
          <alias name="codepoint_t" c:type="hb_codepoint_t"><type name="guint32" c:type="uint32_t"/></alias>
          <alias name="strv_t" c:type="hb_strv_t"><type name="utf8" c:type="char**"/></alias>
          <function name="glyph_id" c:identifier="hb_glyph_id">
            <return-value><type name="codepoint_t" c:type="hb_codepoint_t"/></return-value>
            <parameters><parameter name="cp"><type name="codepoint_t" c:type="hb_codepoint_t"/></parameter></parameters>
          </function>
          <function name="wants_strv" c:identifier="hb_wants_strv">
            <return-value><type name="none"/></return-value>
            <parameters><parameter name="s"><type name="strv_t" c:type="hb_strv_t"/></parameter></parameters>
          </function>
        </namespace></repository>"#;
        let out = emit(src);
        // scalar alias -> u32 on both sides
        assert!(out.contains("fn glyph_id(cp: u32) -> u32 {"));
        // non-scalar alias stays unmapped -> the callable is skipped
        assert!(out.contains("// SKIPPED fn `wants_strv`"));
        assert!(!out.contains("fn wants_strv("));
    }

    #[test]
    fn record_out_param_is_not_folded() {
        // A record (value-struct) out-param must never fold — it is frequently a
        // caller-allocated `T*`, not a `T**`, so filling an 8-byte slot would
        // corrupt the stack. The whole callable stays skipped.
        let src = r#"<repository><namespace name="Gtk">
          <record name="TextIter" glib:type-name="GtkTextIter"/>
          <class name="TextBuffer" c:type="GtkTextBuffer" glib:type-name="GtkTextBuffer">
            <method name="get_start_iter" c:identifier="gtk_text_buffer_get_start_iter">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="b"><type name="TextBuffer"/></instance-parameter>
                <parameter name="iter" direction="out"><type name="TextIter" c:type="GtkTextIter*"/></parameter>
              </parameters>
            </method>
          </class></namespace></repository>"#;
        let out = emit(src);
        assert!(out.contains("// SKIPPED method `TextBuffer::get_start_iter`"));
        assert!(out.contains("value-struct/record out-param"));
        assert!(!out.contains("fn get_start_iter("));
    }

    #[test]
    fn boxed_record_binds_by_pointer_not_by_value() {
        let src = r#"<repository><namespace name="Gtk">
          <record name="TextIter" glib:type-name="GtkTextIter">
            <method name="get_offset" c:identifier="gtk_text_iter_get_offset">
              <return-value><type name="gint"/></return-value>
              <parameters><instance-parameter name="iter"><type name="TextIter" c:type="GtkTextIter*"/></instance-parameter></parameters>
            </method>
          </record>
          <record name="Border" glib:type-name="GtkBorder"/>
          <class name="Buffer" c:type="GtkTextBuffer" glib:type-name="GtkTextBuffer">
            <method name="place_cursor" c:identifier="gtk_text_buffer_place_cursor">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="buffer"><type name="Buffer"/></instance-parameter>
                <parameter name="pos"><type name="TextIter" c:type="const GtkTextIter*"/></parameter>
              </parameters>
            </method>
            <method name="set_border" c:identifier="gtk_text_buffer_set_border">
              <return-value><type name="none"/></return-value>
              <parameters>
                <instance-parameter name="buffer"><type name="Buffer"/></instance-parameter>
                <parameter name="b"><type name="Border" c:type="GtkBorder"/></parameter>
              </parameters>
            </method>
          </class></namespace></repository>"#;
        let out = emit(src);
        // boxed record -> wrapper struct with its methods
        assert!(out.contains("struct TextIter {"));
        assert!(out.contains("fn get_offset(this) -> i32 {"));
        // pointer use resolves to the wrapper (by value)
        assert!(out.contains("fn place_cursor(this, pos: TextIter) {"));
        assert!(out.contains("__c_gtk_text_buffer_place_cursor(this._raw, pos.raw())"));
        // by-value use of a boxed record must be a SKIP, never a mis-bound handle
        assert!(out.contains("// SKIPPED method `Buffer::set_border`"));
        assert!(!out.contains("fn set_border("));
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
