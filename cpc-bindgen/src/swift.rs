// cpc-bindgen — Swift symbol-graph → C+ binding front-end.
//
// Reads the JSON emitted by `swift symbolgraph-extract` (the documented,
// stable machine-readable description of a Swift module's public API — the
// Swift analog of clang's `-ast-dump=json` that the C/ObjC front-ends use).
//
// Why a separate front-end, and why it SKIPs so much:
//   Objective-C is bindable because it has ONE universal dynamic entry point,
//   `objc_msgSend(recv, sel, args)`; the ObjC front-end just emits a selector
//   string per method. Swift has no such thing. Methods use the Swift calling
//   convention (dedicated self/error registers, async continuations), names
//   are mangled, and value types / generics / `async` / `throws` / move-only
//   (`~Copyable`/`~Escapable`) types have no C ABI at all. There is simply no
//   C symbol to call for an ordinary Swift declaration.
//
//   So this front-end emits bindings only for the subset that has a guaranteed
//   C ABI — raw-value enums (named integer constants) and functions explicitly
//   marked `@_cdecl` / `@convention(c)` — and writes `// SKIPPED <path>: <reason>`
//   for everything else. Each skip names exactly what a hand-written `@_cdecl`
//   Swift bridge would have to cover, so the output doubles as the bridge spec.

use crate::sanitize_ident;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Concatenate a symbol's `declarationFragments` spellings into the source text.
fn frags(sym: &Value) -> String {
    sym.get("declarationFragments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.get("spelling").and_then(|s| s.as_str()))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn kind_of(sym: &Value) -> &str {
    sym.get("kind")
        .and_then(|k| k.get("identifier"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn precise(sym: &Value) -> &str {
    sym.get("identifier")
        .and_then(|i| i.get("precise"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn path_of(sym: &Value) -> String {
    sym.get("pathComponents")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_else(|| {
            sym.get("names")
                .and_then(|n| n.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("(anonymous)")
                .to_string()
        })
}

fn access(sym: &Value) -> &str {
    sym.get("accessLevel").and_then(|v| v.as_str()).unwrap_or("")
}

/// True when a whole-word `needle` appears in `hay` (so `throws` doesn't match
/// inside an identifier). Cheap word-boundary check on ASCII source text.
fn has_word(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let n = needle.len();
    let mut i = 0;
    while let Some(off) = hay[i..].find(needle) {
        let start = i + off;
        let end = start + n;
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = start + 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Map a Swift type spelling (a single `typeIdentifier` spelling, possibly a
/// pointer wrapper) to a C+ FFI type, or `Err(reason)` when it has no C ABI.
fn map_swift_type(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() || s == "Void" || s == "()" {
        return Ok("()".to_string());
    }
    // Pointer wrappers. `UnsafePointer<T>` → `*T`, raw pointers → `*u8`.
    for (wrap, _mut) in [
        ("UnsafePointer<", false),
        ("UnsafeMutablePointer<", true),
    ] {
        if let Some(rest) = s.strip_prefix(wrap) {
            let inner = rest.strip_suffix('>').unwrap_or(rest);
            return Ok(match map_swift_type(inner) {
                Ok(t) => format!("*{t}"),
                Err(_) => "*u8".to_string(),
            });
        }
    }
    if matches!(
        s,
        "UnsafeRawPointer"
            | "UnsafeMutableRawPointer"
            | "OpaquePointer"
            | "UnsafeMutableRawPointer?"
            | "UnsafeRawPointer?"
    ) {
        return Ok("*u8".to_string());
    }
    Ok(match s {
        "Int" => "i64".to_string(),
        "UInt" => "u64".to_string(),
        "Int8" | "CChar" => "i8".to_string(),
        "UInt8" => "u8".to_string(),
        "Int16" => "i16".to_string(),
        "UInt16" => "u16".to_string(),
        "Int32" | "CInt" => "i32".to_string(),
        "UInt32" => "u32".to_string(),
        "Int64" => "i64".to_string(),
        "UInt64" => "u64".to_string(),
        "Float" | "Float32" | "CFloat" => "f32".to_string(),
        "Double" | "Float64" | "CDouble" => "f64".to_string(),
        "Bool" => "bool".to_string(),
        other => return Err(format!("non-C type `{other}`")),
    })
}

/// The reason a function-like symbol can't be bound, or `None` if it carries an
/// explicit C convention and a fully C-mappable signature (then `Some(Ok(..))`).
enum FnVerdict {
    /// Emit this C+ `extern fn` line.
    Emit(String),
    /// Skip with this reason.
    Skip(String),
}

fn classify_function(sym: &Value) -> FnVerdict {
    let decl = frags(sym);
    // Ordering matters: report the most fundamental blocker first.
    if has_word(&decl, "async") {
        return FnVerdict::Skip("async — needs a synchronous @_cdecl bridge".into());
    }
    if has_word(&decl, "throws") || has_word(&decl, "rethrows") {
        return FnVerdict::Skip("throws — Swift error register has no C ABI".into());
    }
    if sym.get("swiftGenerics").is_some() || decl.contains('<') {
        return FnVerdict::Skip("generic — no concrete C ABI".into());
    }
    for marker in ["consuming ", "borrowing ", "inout ", "~Copyable", "~Escapable"] {
        if decl.contains(marker) {
            return FnVerdict::Skip(format!("ownership/move-only (`{}`)", marker.trim()));
        }
    }
    if has_word(&decl, "some") || has_word(&decl, "any") {
        return FnVerdict::Skip("opaque/existential parameter (`some`/`any`)".into());
    }
    let c_convention = decl.contains("@convention(c)")
        || decl.contains("@_cdecl")
        || decl.contains("@cdecl");
    let is_member = matches!(kind_of(sym), "swift.method" | "swift.type.method" | "swift.init");
    if !c_convention {
        if is_member {
            return FnVerdict::Skip(
                "instance/type method — Swift `self` calling convention, no C symbol".into(),
            );
        }
        return FnVerdict::Skip(
            "no @_cdecl/@convention(c) — Swift calling convention, no C entry point".into(),
        );
    }
    // C-callable: extract the signature from `functionSignature` and map it.
    let base = sym
        .get("pathComponents")
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|c| c.as_str())
        .unwrap_or("");
    // The C symbol is the base name up to the first '(' (the Swift label list).
    let c_name = base.split('(').next().unwrap_or(base);
    let c_name = sanitize_ident(c_name);

    let sig = sym.get("functionSignature");
    let ret = sig
        .and_then(|s| s.get("returns"))
        .and_then(|v| v.as_array())
        .map(|frs| type_id_in_fragments(frs))
        .unwrap_or_default();
    let ret_cplus = match map_swift_type(&ret) {
        Ok(t) => t,
        Err(why) => return FnVerdict::Skip(format!("return — {why}")),
    };
    let mut params_out: Vec<String> = Vec::new();
    if let Some(params) = sig.and_then(|s| s.get("parameters")).and_then(|v| v.as_array()) {
        for (i, p) in params.iter().enumerate() {
            let pty = p
                .get("declarationFragments")
                .and_then(|v| v.as_array())
                .map(|frs| type_id_in_fragments(frs))
                .unwrap_or_default();
            let pty_cplus = match map_swift_type(&pty) {
                Ok(t) => t,
                Err(why) => return FnVerdict::Skip(format!("param {i} — {why}")),
            };
            let pname = p
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| sanitize_ident(s))
                .unwrap_or_else(|| format!("arg{i}"));
            params_out.push(format!("{pname}: {pty_cplus}"));
        }
    }
    let mut line = format!("#[link_name = \"{c_name}\"]\nextern fn {c_name}(");
    line.push_str(&params_out.join(", "));
    line.push(')');
    if ret_cplus != "()" {
        line.push_str(&format!(" -> {ret_cplus}"));
    }
    line.push_str(";\n");
    FnVerdict::Emit(line)
}

/// Pull the first `typeIdentifier` spelling out of a fragment array (the return
/// or parameter type). Falls back to the last `text`-ish token.
fn type_id_in_fragments(frs: &[Value]) -> String {
    for f in frs {
        if f.get("kind").and_then(|v| v.as_str()) == Some("typeIdentifier") {
            if let Some(s) = f.get("spelling").and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Read an explicit integer raw value from an enum case's fragments
/// (`case foo = 3`). `None` when the case has no `= <int>` (plain enum) or
/// carries an associated value.
fn case_raw_value(sym: &Value) -> Option<i64> {
    let decl = frags(sym);
    let eq = decl.find('=')?;
    decl[eq + 1..].trim().parse::<i64>().ok()
}

fn case_has_payload(sym: &Value) -> bool {
    // `case ndArray(NDArrayDescriptor)` — a paren in the title means associated
    // values. The bare `()` of a no-arg case never appears in the title.
    sym.get("names")
        .and_then(|n| n.get("title"))
        .and_then(|v| v.as_str())
        .map(|t| t.contains('('))
        .unwrap_or(false)
}

/// Generate C+ bindings from a parsed `*.symbols.json` symbol graph.
pub fn generate(graph: &Value, module: &str) -> String {
    let symbols: Vec<Value> = graph
        .get("symbols")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // memberOf: child precise -> parent precise (cases→enum, members→type).
    let mut member_of: HashMap<String, String> = HashMap::new();
    if let Some(rels) = graph.get("relationships").and_then(|v| v.as_array()) {
        for r in rels {
            if r.get("kind").and_then(|v| v.as_str()) == Some("memberOf") {
                if let (Some(s), Some(t)) = (
                    r.get("source").and_then(|v| v.as_str()),
                    r.get("target").and_then(|v| v.as_str()),
                ) {
                    member_of.insert(s.to_string(), t.to_string());
                }
            }
        }
    }
    // Index enum cases by their parent enum's precise id.
    let mut cases_by_enum: HashMap<String, Vec<&Value>> = HashMap::new();
    for s in &symbols {
        if kind_of(s) == "swift.enum.case" {
            if let Some(parent) = member_of.get(precise(s)) {
                cases_by_enum.entry(parent.clone()).or_default().push(s);
            }
        }
    }

    let mut out = String::new();
    out.push_str("// Auto-generated by cpc-bindgen (--swift). DO NOT EDIT.\n");
    out.push_str(&format!("// Swift module: {module}\n"));
    out.push_str("//\n");
    out.push_str("// Source: `swift symbolgraph-extract` JSON. Only constructs with a\n");
    out.push_str("// guaranteed C ABI are bound; everything else is `// SKIPPED` with the\n");
    out.push_str("// reason it needs a hand-written @_cdecl Swift bridge. See swift.rs.\n\n");

    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let skip = |out: &mut String,
                    skipped: &mut usize,
                    reasons: &mut BTreeMap<String, usize>,
                    path: &str,
                    reason: String| {
        // Bucket the reason by its leading phrase (before any '(' or '`') for
        // the summary histogram.
        let bucket = reason
            .split(|c| c == '(' || c == '`' || c == '—')
            .next()
            .unwrap_or(&reason)
            .trim()
            .to_string();
        *reasons.entry(bucket).or_insert(0) += 1;
        *skipped += 1;
        out.push_str(&format!("// SKIPPED {path}: {reason}\n"));
    };

    // Stable order: sort every public symbol by its dotted path.
    let mut ordered: Vec<&Value> = symbols
        .iter()
        .filter(|s| access(s) == "public" || access(s) == "open")
        .collect();
    ordered.sort_by(|a, b| path_of(a).cmp(&path_of(b)));

    for s in ordered {
        let k = kind_of(s);
        let path = path_of(s);
        match k {
            "swift.enum.case" => { /* handled with the parent enum */ }
            "swift.enum" => {
                let cases = cases_by_enum.get(precise(s)).cloned().unwrap_or_default();
                if cases.iter().any(|c| case_has_payload(c)) {
                    skip(
                        &mut out,
                        &mut skipped,
                        &mut skip_reasons,
                        &path,
                        "enum with associated values (sum type) — not a C enum".into(),
                    );
                    continue;
                }
                let raws: Vec<(String, i64)> = cases
                    .iter()
                    .filter_map(|c| {
                        let cn = c
                            .get("pathComponents")
                            .and_then(|v| v.as_array())
                            .and_then(|a| a.last())
                            .and_then(|x| x.as_str())?;
                        let cn = cn.split('(').next().unwrap_or(cn);
                        case_raw_value(c).map(|v| (cn.to_string(), v))
                    })
                    .collect();
                if cases.is_empty() || raws.len() != cases.len() {
                    skip(
                        &mut out,
                        &mut skipped,
                        &mut skip_reasons,
                        &path,
                        "plain enum, no integer raw values — no stable C ABI".into(),
                    );
                    continue;
                }
                // Raw-value enum → named integer constant accessors, the same
                // idiom the C front-end uses for C enums.
                out.push_str(&format!("// Swift enum `{path}` — raw values as i64 accessors.\n"));
                for (cn, v) in raws {
                    out.push_str(&format!(
                        "fn {}() -> i64 {{ return {} as i64; }}\n",
                        sanitize_ident(&cn),
                        v
                    ));
                    emitted += 1;
                }
            }
            "swift.func" | "swift.method" | "swift.type.method" | "swift.init" => {
                match classify_function(s) {
                    FnVerdict::Emit(line) => {
                        out.push_str(&line);
                        emitted += 1;
                    }
                    FnVerdict::Skip(reason) => {
                        skip(&mut out, &mut skipped, &mut skip_reasons, &path, reason)
                    }
                }
            }
            "swift.struct" => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                "Swift value type (no @repr(C) layout) — fields need a bridge".into(),
            ),
            "swift.class" => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                "Swift reference type — needs a swift_retain/release bridge".into(),
            ),
            "swift.protocol" => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                "protocol (existential) — not C-representable".into(),
            ),
            "swift.property" | "swift.type.property" => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                "property accessor — Swift calling convention".into(),
            ),
            "swift.func.op" => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                "operator — no C symbol".into(),
            ),
            "swift.typealias" => {
                // Emit a C+ alias when the underlying type is C-mappable.
                let decl = frags(s);
                let under = decl.rsplit('=').next().unwrap_or("").trim();
                match map_swift_type(under) {
                    Ok(t) if !under.is_empty() => {
                        out.push_str(&format!("type {} = {};\n", sanitize_ident(&path.replace('.', "_")), t));
                        emitted += 1;
                    }
                    _ => skip(
                        &mut out,
                        &mut skipped,
                        &mut skip_reasons,
                        &path,
                        "typealias to a non-C type".into(),
                    ),
                }
            }
            other if !other.is_empty() => skip(
                &mut out,
                &mut skipped,
                &mut skip_reasons,
                &path,
                format!("unhandled symbol kind `{other}`"),
            ),
            _ => {}
        }
    }

    out.push_str(&format!(
        "\n// ── Summary ──────────────────────────────────────────────\n// {emitted} emitted, {skipped} skipped.\n"
    ));
    for (reason, n) in &skip_reasons {
        out.push_str(&format!("//   {n:4}  {reason}\n"));
    }
    if emitted == 0 {
        out.push_str(
            "// Nothing in this module has a C ABI. To call it from C+, write a\n\
             // Swift @_cdecl bridge covering the symbols above, compile it to a\n\
             // dylib, and run cpc-bindgen on the bridge's C header.\n",
        );
    }
    out
}

/// The umbrella module for a generated Swift package: imports each per-module
/// binding so a consumer can `import "<pkg>/<module>"`.
pub fn package_umbrella(pkg: &str, modules: &[String]) -> String {
    let mut s = format!(
        "// {pkg} — C+ binding for the {pkg} Swift module(s).\n\
         // Auto-generated by cpc-bindgen (--swift). DO NOT EDIT.\n\n"
    );
    for m in modules {
        let lower = m.to_lowercase();
        s.push_str(&format!("import \"./{lower}\" as {lower};\n"));
    }
    s
}

/// `Cplus.toml` for a generated Swift package, with the same provenance header
/// shape `--framework` writes.
pub fn package_toml(
    pkg: &str,
    link_framework: &str,
    sdk_version: &str,
    modules: &[String],
    reproduce: &str,
) -> String {
    format!(
        "# Auto-generated by cpc-bindgen --swift. Regenerate; do not hand-edit src/.\n\
         #\n\
         # framework = \"{link_framework}\"\n\
         # modules   = \"{mods}\"\n\
         # sdk       = \"{sdk_version}\"\n\
         # generator = \"cpc-bindgen {ver}\"\n\
         # reproduce = \"{reproduce}\"\n\
         #\n\
         # NOTE: a pure-Swift framework has no C ABI; the generated modules are\n\
         # SKIP manifests (see each src file's summary). To call it from C+, write\n\
         # a Swift @_cdecl bridge covering the skipped symbols and bind its C header.\n\
         \n\
         [package]\nname    = \"{pkg}\"\nversion = \"0.0.0\"\nedition = \"2026\"\n\n\
         [dependencies]\nstdlib = \"*\"\n\n\
         [link]\nframeworks = [\"{link_framework}\"]\n",
        mods = modules.join(", "),
        ver = env!("CARGO_PKG_VERSION"),
    )
}

/// Run `swift symbolgraph-extract` for `module` and return the parsed graph.
/// `extra_args` are forwarded verbatim (e.g. `-target`, `-sdk`, `-F`) so the
/// caller can point at a specific SDK/toolchain (such as an Xcode-beta SDK).
pub fn extract(module: &str, extra_args: &[String]) -> Result<Value, String> {
    let tmp = std::env::temp_dir().join(format!("cpc-bindgen-sg-{module}"));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("temp dir: {e}"))?;
    let mut cmd = std::process::Command::new("xcrun");
    cmd.arg("swift")
        .arg("symbolgraph-extract")
        .arg("-module-name")
        .arg(module)
        .arg("-output-dir")
        .arg(&tmp)
        .arg("-minimum-access-level")
        .arg("public");
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run `xcrun swift symbolgraph-extract`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "symbolgraph-extract failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let path = tmp.join(format!("{module}.symbols.json"));
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parsing symbol graph JSON: {e}"))
}

// ──────────────────────────────────────────────────────────────────────────
// Bridge emitter (M1)
//
// Instead of the all-SKIP manifest above, generate a compiled `@_cdecl` Swift
// bridge + the matching C+ bindings — the only stable Swift→C path. Per
// bindable symbol we emit TWO artifacts in lockstep: a `@_cdecl` thunk (Swift,
// into `<Module>Bridge.swift`) and the C+ `extern fn` + ergonomic wrapper (into
// `<module>.cplus`). See plans/plan.swift-bridge-gen.md.
//
// M1 marshaling: reference (`class`) and value (`struct`) types → opaque handles
// boxed in a Swift class; scalars by value; `String` params as (ptr,len);
// `throws` → nil + error channel; `async` → a blocking semaphore; scalar
// property getters; raw-value enums. Everything else is SKIPPED (honest
// residual) with a reason histogram, exactly as the classifier does.

/// The four files of a generated Swift-bridge package.
pub struct BridgeFiles {
    pub swift: String,
    pub cplus: String,
    pub header: String,
    pub build_sh: String,
    pub emitted: usize,
    pub skipped: usize,
}

/// Human-supplied facts the symbol graph cannot provide. Loaded from
/// `--bridge-spec FILE` (a JSON object).
///
/// - `copyable`: value types the author vouches are `Copyable`, so a handle
///   *property* getter can safely copy the value out of `self`. Noncopyability
///   is not graph-detectable; a wrong guess fails to compile. Classes are always
///   safe and need no entry.
/// - `raw_enums`: enums with an integer `RawValue`. They bind as usable `i64`
///   scalars (with per-case accessor constants) instead of opaque handles, so a
///   method taking such an enum becomes callable. The integer width is irrelevant
///   to the spec — the bridge funnels every raw value through `Int64`.
/// - `enum_cases`: enums *without* a raw value (a plain `case a, b, c`). They
///   stay opaque handles, but each case gets a constructor `Enum_case() ->
///   Option[Enum]` so C+ can build a value to pass on. Use this for an enum a
///   method consumes when the enum has no `RawValue`.
/// - `noncopyable_owners`: types the author vouches are `~Copyable` (move-only).
///   Their box uses optional storage, and a member read becomes a consuming
///   `take_<member>` (single-use; the C+ handle is invalidated after) — because
///   reading any member through the box otherwise consumes a borrowed value.
/// - `view_copy`: for a handle type with a `view(as:)` over a `~Escapable`
///   element view, emit a bulk-copy accessor (`<Type>_copy_<elem>`) that takes
///   the view *inside* the thunk and memcpy's a contiguous run into a caller
///   buffer — the view never escapes, so it never needs to be boxed.
/// - `instantiate`: concrete element types for a generic method whose generic
///   parameter is carried by a `some Sequence`/`[T]` element. One binding is
///   emitted per type (`<base>_<Type>`); the `some Sequence` param becomes a
///   `[Type]` slice. A generic that still returns a `~Escapable` view after
///   substitution self-gates (its `<…>` return keeps it skipped).
#[derive(Default)]
pub struct BridgeSpec {
    pub copyable: std::collections::BTreeSet<String>,
    pub raw_enums: std::collections::BTreeSet<String>,
    pub enum_cases: std::collections::BTreeSet<String>,
    pub noncopyable_owners: std::collections::BTreeSet<String>,
    pub view_copy: std::collections::BTreeMap<String, Vec<String>>,
    pub instantiate: std::collections::BTreeMap<String, Vec<String>>,
}

impl BridgeSpec {
    /// Parse a `--bridge-spec` JSON object: `{ "copyable": [...],
    /// "raw_enums": [...], "enum_cases": [...], "noncopyable_owners": [...],
    /// "view_copy": { "NDArray": ["Float", "Int32"] } }`.
    pub fn from_json(v: &Value) -> Self {
        let str_set = |key: &str| {
            v.get(key)
                .and_then(|c| c.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let map_of_lists = |key: &str| {
            v.get(key)
                .and_then(|o| o.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, val)| {
                            val.as_array().map(|a| {
                                (
                                    k.clone(),
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                        .collect::<Vec<_>>(),
                                )
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        BridgeSpec {
            copyable: str_set("copyable"),
            raw_enums: str_set("raw_enums"),
            enum_cases: str_set("enum_cases"),
            noncopyable_owners: str_set("noncopyable_owners"),
            view_copy: map_of_lists("view_copy"),
            instantiate: map_of_lists("instantiate"),
        }
    }
}

#[derive(Clone)]
enum Marshal {
    Void,
    Scalar { cplus: String, swift: String },
    Str,
    Handle(String),
    /// `T?` where `T` is a known handle type — null pointer means `nil`/`None`.
    OptHandle(String),
    /// `[Elem]` of a scalar element — crosses as `(ptr, count)` (param only).
    Slice { cplus: String, swift: String },
    /// A spec-declared integer raw-value enum — crosses as `i64` (the case's
    /// `.rawValue`), with the Swift type name kept to build/read it.
    EnumScalar(String),
}

/// Map a single Swift type spelling to its marshaling, or `None` when it has no
/// bridge (dictionaries, optionals of non-handles, nested/generic types, raw
/// pointers). `?`-optionals of a known handle and `[scalar]` arrays are handled.
fn marshal_of(
    swift_ty: &str,
    types: &std::collections::BTreeSet<String>,
    raw_enums: &std::collections::BTreeSet<String>,
) -> Option<Marshal> {
    let t = swift_ty.trim();
    if t.is_empty() || t == "Void" || t == "()" {
        return Some(Marshal::Void);
    }
    // A spec-declared raw-value enum binds as an `i64` scalar, not a handle.
    if raw_enums.contains(t) {
        return Some(Marshal::EnumScalar(t.to_string()));
    }
    // `T?` — only known-handle optionals are modelled (null = nil).
    if let Some(inner) = t.strip_suffix('?') {
        let inner = inner.trim();
        if types.contains(inner) {
            return Some(Marshal::OptHandle(inner.to_string()));
        }
        return None;
    }
    // `[Elem]` — only a scalar element array (a contiguous buffer).
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(elem) = rest.strip_suffix(']') {
            let elem = elem.trim();
            if !elem.contains(':') {
                if let Ok(c) = map_swift_type(elem) {
                    if c != "()" && !c.starts_with('*') {
                        return Some(Marshal::Slice {
                            cplus: c,
                            swift: elem.to_string(),
                        });
                    }
                }
            }
        }
        return None;
    }
    if t == "String" {
        return Some(Marshal::Str);
    }
    if let Ok(c) = map_swift_type(t) {
        // A scalar maps to a non-pointer, non-void C+ type.
        if c != "()" && !c.starts_with('*') {
            return Some(Marshal::Scalar {
                cplus: c,
                swift: t.to_string(),
            });
        }
    }
    if types.contains(t) {
        return Some(Marshal::Handle(t.to_string()));
    }
    None
}

fn last_component(sym: &Value) -> String {
    sym.get("pathComponents")
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string()
}

/// Concatenate all fragment spellings — the faithful type/decl text, including
/// `?`, `.Nested`, `<...>`, `[...]` (unlike `type_id_in_fragments`, which keeps
/// only the first `typeIdentifier` and silently drops optionals/qualifiers).
fn frag_spellings(frs: &[Value]) -> String {
    frs.iter()
        .filter_map(|f| f.get("spelling").and_then(|s| s.as_str()))
        .collect()
}

/// The type of a `label: Type` declaration (parameter or property): everything
/// after the first `": "`. The first colon is always the label separator, so a
/// dictionary type (`[K: V]`) in the type is preserved intact. A trailing
/// accessor block on a property (`var n: Int { get }`) is dropped — a type
/// never contains `{`, so cutting there is safe.
fn type_after_colon(frs: &[Value]) -> String {
    let joined = frag_spellings(frs);
    let after = match joined.split_once(": ") {
        Some((_, rest)) => rest,
        None => &joined,
    };
    after.split('{').next().unwrap_or(after).trim().to_string()
}

/// Split a Swift member selector into its base name and external argument
/// labels: `run(input:)` → (`run`, [`input`]); `init(rank:scale:)` →
/// (`init`, [`rank`,`scale`]); `name` → (`name`, []).
fn selector_parts(sel: &str) -> (String, Vec<String>) {
    let base = sel.split('(').next().unwrap_or(sel).to_string();
    let labels = match (sel.find('('), sel.rfind(')')) {
        (Some(o), Some(c)) if c > o => {
            let inner = &sel[o + 1..c];
            inner
                .split(':')
                .filter(|x| !x.is_empty())
                .map(|x| x.to_string())
                .collect()
        }
        _ => vec![],
    };
    (base, labels)
}

fn scalar_default(swift: &str) -> &'static str {
    if swift == "Bool" {
        "false"
    } else {
        "0"
    }
}

/// A spelling that is a legal Swift/C+ identifier (so it can name a `@_cdecl`
/// function). Operator members (`==`, `+`) and the like are not.
fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Flatten a (possibly nested, dotted) Swift type name into one C+/Swift
/// identifier: `InferenceValue.Descriptor` → `InferenceValue_Descriptor`. The
/// Swift *type spelling* stays dotted; only the C+ struct / box-class names and
/// generated symbols use this flattened form.
fn cident(name: &str) -> String {
    sanitize_ident(&name.replace('.', "_"))
}

fn box_name(ty: &str) -> String {
    format!("{}Box", cident(ty))
}

/// C+ wrapper body that turns a `*u8` (a malloc'd C string from `strdup`, or
/// null) returned by `callexpr` into `Option[Text]`, copying then `free`-ing the
/// buffer. Shared by String method returns and String property getters.
fn str_copyout_body(callexpr: &str) -> String {
    format!(
        "        let p: *u8 = {{ {callexpr} }};\n\
         \x20       if is_null(p) {{ return option::Option[text::Text]::None; }}\n\
         \x20       let n: usize = {{ strlen(p) }};\n\
         \x20       let v: str = {{ #str_from_raw_parts(p, n) }};\n\
         \x20       let t: text::Text = v.to_text();\n\
         \x20       {{ free(p); }}\n\
         \x20       return option::some(t);\n"
    )
}

/// One bound member: the Swift `@_cdecl` thunk(s), the C+ `extern fn` line(s),
/// and the C+ wrapper(s) (to live inside `impl <owner>`). A property can yield a
/// getter and a setter, so each field may hold more than one declaration. `Err`
/// carries a skip reason. `uses_text` flags a `Text` copy-out (String return).
struct MemberEmit {
    thunk: String,
    extern_line: String,
    wrapper: String,
    uses_text: bool,
}

fn emit_member(
    sym: &Value,
    owner: &str,       // flattened C+ identifier for the owning type
    owner_swift: &str, // dotted Swift type spelling (e.g. `NDArray.RawView`)
    prefix: &str,
    types: &std::collections::BTreeSet<String>,
    copy_safe: &std::collections::BTreeSet<String>,
    raw_enums: &std::collections::BTreeSet<String>,
    noncopyable_owner: bool,
    suffix: &str, // appended to the C symbol + wrapper name (generic instantiation)
) -> Result<MemberEmit, String> {
    let kind = kind_of(sym);
    let decl = frags(sym);
    let is_init = kind == "swift.init";
    let is_static = kind == "swift.type.method";
    let is_property = kind == "swift.property" || kind == "swift.type.property";

    // Blanket skips (same precedence as the classifier).
    if sym.get("swiftGenerics").is_some() || decl.contains('<') {
        return Err("generic — needs a concrete instantiation (spec)".into());
    }
    for m in [
        "consuming ",
        "borrowing ",
        "inout ",
        "~Copyable",
        "~Escapable",
    ] {
        if decl.contains(m) {
            return Err(format!("ownership/move-only (`{}`)", m.trim()));
        }
    }
    if has_word(&decl, "some") || has_word(&decl, "any") {
        return Err("opaque/existential — needs a concrete type (spec)".into());
    }

    let sel = last_component(sym);
    let (base, labels) = selector_parts(&sel);
    let owner_box = box_name(owner);

    // ── Properties: getter (+ setter when a stored `var`) ─────────────────
    if is_property {
        let pty = type_after_colon(
            sym.get("declarationFragments")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]),
        );
        let g = sanitize_ident(&base);
        // A `~Copyable` owner can't expose a borrow-read getter (reading any
        // member through the box consumes it). Instead, a `take_<member>` thunk
        // moves the value out of the box's optional slot once and reads the
        // member off the owned local — the C+ handle becomes an empty husk.
        if noncopyable_owner {
            let (t, optional) = match marshal_of(&pty, types, raw_enums) {
                Some(Marshal::Handle(t)) => (t, false),
                Some(Marshal::OptHandle(t)) => (t, true),
                _ => return Err("~Copyable owner: take_ supports only handle properties".into()),
            };
            let ct = cident(&t);
            let tb = box_name(&t);
            let cn = format!("{prefix}_{owner}_take_{g}");
            let read = if optional {
                format!("if let _v = iv.{base} {{ return cpRetained({tb}(_v)) }} else {{ return nil }}")
            } else {
                format!("let _v = iv.{base}\n    return cpRetained({tb}(_v))")
            };
            let thunk = format!(
                "@_cdecl(\"{cn}\")\npublic func {cn}(_ self_: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? {{\n    guard let _box = cpObject(self_, as: {owner_box}.self) else {{ return nil }}\n    guard let iv = cpTakeOut(&_box.value) else {{ cpSetError(\"value already taken\"); return nil }}\n    {read}\n}}\n\n"
            );
            let extern_line = format!("#[link_name = \"{cn}\"]\nextern fn {cn}(receiver: *u8) -> *u8;\n");
            let wrapper = format!(
                "    fn take_{g}(this) -> option::Option[{ct}] {{\n        let raw: *u8 = {{ {cn}(this._raw) }};\n        if is_null(raw) {{ return option::Option[{ct}]::None; }}\n        return option::some({ct} {{ _raw: raw }});\n    }}\n"
            );
            return Ok(MemberEmit { thunk, extern_line, wrapper, uses_text: false });
        }
        let cget = format!("{prefix}_{owner}_{g}");
        let cset = format!("{prefix}_{owner}_set_{g}");
        // A stored `var` is settable; `let` and computed `{ get }` are not.
        let settable = decl.split_whitespace().next() == Some("var") && !decl.contains("{ get }");
        let mut thunk = String::new();
        let mut extern_line = String::new();
        let mut wrapper = String::new();
        let mut uses_text = false;
        match marshal_of(&pty, types, raw_enums) {
            Some(Marshal::Scalar { cplus, swift }) => {
                let dflt = scalar_default(&swift);
                thunk.push_str(&format!(
                    "@_cdecl(\"{cget}\")\npublic func {cget}(_ self_: UnsafeMutableRawPointer?) -> {swift} {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return {dflt} }}\n    return _self.value.{base}\n}}\n\n"
                ));
                extern_line.push_str(&format!("#[link_name = \"{cget}\"]\nextern fn {cget}(receiver: *u8) -> {cplus};\n"));
                wrapper.push_str(&format!("    fn {g}(this) -> {cplus} {{\n        return {{ {cget}(this._raw) }};\n    }}\n"));
                if settable {
                    thunk.push_str(&format!(
                        "@_cdecl(\"{cset}\")\npublic func {cset}(_ self_: UnsafeMutableRawPointer?, _ value: {swift}) {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return }}\n    _self.value.{base} = value\n}}\n\n"
                    ));
                    extern_line.push_str(&format!("#[link_name = \"{cset}\"]\nextern fn {cset}(receiver: *u8, value: {cplus});\n"));
                    wrapper.push_str(&format!("    fn set_{g}(this, value: {cplus}) {{\n        {{ {cset}(this._raw, value); }}\n        return;\n    }}\n"));
                }
            }
            Some(Marshal::Str) => {
                uses_text = true;
                thunk.push_str(&format!(
                    "@_cdecl(\"{cget}\")\npublic func {cget}(_ self_: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return nil }}\n    return strdup(_self.value.{base})\n}}\n\n"
                ));
                extern_line.push_str(&format!("#[link_name = \"{cget}\"]\nextern fn {cget}(receiver: *u8) -> *u8;\n"));
                wrapper.push_str(&format!(
                    "    fn {g}(this) -> option::Option[text::Text] {{\n{}    }}\n",
                    str_copyout_body(&format!("{cget}(this._raw)"))
                ));
                if settable {
                    thunk.push_str(&format!(
                        "@_cdecl(\"{cset}\")\npublic func {cset}(_ self_: UnsafeMutableRawPointer?, _ value: UnsafePointer<UInt8>?, _ value_len: Int) {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return }}\n    guard let s = cpStringFromBytes(value, value_len) else {{ return }}\n    _self.value.{base} = s\n}}\n\n"
                    ));
                    extern_line.push_str(&format!("#[link_name = \"{cset}\"]\nextern fn {cset}(receiver: *u8, value: *u8, value_len: usize);\n"));
                    wrapper.push_str(&format!("    fn set_{g}(this, value: str) {{\n        {{ {cset}(this._raw, #str_ptr(value), #str_len(value)); }}\n        return;\n    }}\n"));
                }
            }
            // Handle / optional-handle property: reading copies the value out of
            // `self`, which is only safe when the value type is Copyable. Classes
            // are always safe; other types must be vouched for in `--bridge-spec`.
            Some(Marshal::Handle(t)) | Some(Marshal::OptHandle(t)) => {
                let optional = matches!(marshal_of(&pty, types, raw_enums), Some(Marshal::OptHandle(_)));
                // Both the owner (whose member is read through the box — which
                // consumes a `~Copyable` value) and the value type (copied into a
                // new box) must be Copyable.
                if !copy_safe.contains(owner_swift) || !copy_safe.contains(&t) {
                    return Err("handle property needs owner + value in `copyable` (--bridge-spec)".into());
                }
                let ct = cident(&t);
                let tb = box_name(&t);
                // Bind a local first: the box init is `consuming`, and a property
                // projection of the borrowed `_self.value` can't be consumed in
                // place. The bind copies it out (safe — the type is copy-safe).
                let read = if optional {
                    format!("if let _v = _self.value.{base} {{ return cpRetained({tb}(_v)) }} else {{ return nil }}")
                } else {
                    format!("let _v = _self.value.{base}\n    return cpRetained({tb}(_v))")
                };
                thunk.push_str(&format!(
                    "@_cdecl(\"{cget}\")\npublic func {cget}(_ self_: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return nil }}\n    {read}\n}}\n\n"
                ));
                extern_line.push_str(&format!("#[link_name = \"{cget}\"]\nextern fn {cget}(receiver: *u8) -> *u8;\n"));
                wrapper.push_str(&format!(
                    "    fn {g}(this) -> option::Option[{ct}] {{\n        let raw: *u8 = {{ {cget}(this._raw) }};\n        if is_null(raw) {{ return option::Option[{ct}]::None; }}\n        return option::some({ct} {{ _raw: raw }});\n    }}\n"
                ));
                // Setter only for a non-optional stored `var` handle.
                if settable && !optional {
                    thunk.push_str(&format!(
                        "@_cdecl(\"{cset}\")\npublic func {cset}(_ self_: UnsafeMutableRawPointer?, _ value: UnsafeMutableRawPointer?) {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return }}\n    guard let _value = cpObject(value, as: {tb}.self) else {{ return }}\n    _self.value.{base} = _value.value\n}}\n\n"
                    ));
                    extern_line.push_str(&format!("#[link_name = \"{cset}\"]\nextern fn {cset}(receiver: *u8, value: *u8);\n"));
                    wrapper.push_str(&format!("    fn set_{g}(this, value: {ct}) {{\n        {{ {cset}(this._raw, value._raw); }}\n        return;\n    }}\n"));
                }
            }
            // A raw-enum property reads as `i64` (the case's `.rawValue`). Like a
            // handle property it reads through the box, so the owner must be
            // Copyable; the enum value itself is a plain integer.
            Some(Marshal::EnumScalar(name)) => {
                if !copy_safe.contains(owner_swift) {
                    return Err("raw-enum property needs the owner in `copyable` (--bridge-spec)".into());
                }
                thunk.push_str(&format!(
                    "@_cdecl(\"{cget}\")\npublic func {cget}(_ self_: UnsafeMutableRawPointer?) -> Int64 {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return 0 }}\n    return Int64(_self.value.{base}.rawValue)\n}}\n\n"
                ));
                extern_line.push_str(&format!("#[link_name = \"{cget}\"]\nextern fn {cget}(receiver: *u8) -> i64;\n"));
                wrapper.push_str(&format!("    fn {g}(this) -> i64 {{\n        return {{ {cget}(this._raw) }};\n    }}\n"));
                if settable {
                    thunk.push_str(&format!(
                        "@_cdecl(\"{cset}\")\npublic func {cset}(_ self_: UnsafeMutableRawPointer?, _ value: Int64) {{\n    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ return }}\n    guard let _e = {name}(rawValue: numericCast(value)) else {{ return }}\n    _self.value.{base} = _e\n}}\n\n"
                    ));
                    extern_line.push_str(&format!("#[link_name = \"{cset}\"]\nextern fn {cset}(receiver: *u8, value: i64);\n"));
                    wrapper.push_str(&format!("    fn set_{g}(this, value: i64) {{\n        {{ {cset}(this._raw, value); }}\n        return;\n    }}\n"));
                }
            }
            _ => return Err("property type not bridgeable".into()),
        }
        return Ok(MemberEmit { thunk, extern_line, wrapper, uses_text });
    }

    // ── Methods / initializers ─────────────────────────────────────────────
    let is_method = kind == "swift.method" || kind == "swift.type.method";
    if !is_init && !is_method {
        // Subscripts, operator decls, deinit, etc. — not M1 shapes.
        return Err(format!("unsupported member kind `{kind}`"));
    }
    if !is_ident(&base) {
        return Err("operator/non-identifier member — needs a named bridge".into());
    }
    let is_async = has_word(&decl, "async");
    let throws = has_word(&decl, "throws") || has_word(&decl, "rethrows");

    // Parameters: zip the external labels with the signature's typed params.
    let params: Vec<&Value> = sym
        .get("functionSignature")
        .and_then(|s| s.get("parameters"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    if !labels.is_empty() && labels.len() != params.len() {
        return Err("default/variadic parameters not modelled (M1)".into());
    }

    struct P {
        cname: String,   // C+/thunk identifier
        label: String,   // Swift external label ("_" = none)
        marshal: Marshal,
    }
    let mut ps: Vec<P> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        let pty = type_after_colon(
            p.get("declarationFragments")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]),
        );
        let m = marshal_of(&pty, types, raw_enums)
            .ok_or_else(|| format!("param `{pty}` has no C ABI"))?;
        match m {
            Marshal::Void => return Err("void parameter".into()),
            Marshal::OptHandle(_) => {
                return Err("optional handle parameter not modelled".into())
            }
            _ => {}
        }
        let label = labels.get(i).cloned().unwrap_or_else(|| "_".to_string());
        let internal = p
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(sanitize_ident)
            .unwrap_or_else(|| format!("arg{i}"));
        // Prefer the label as the visible C+ name; fall back to the internal name.
        let cname = if label != "_" {
            sanitize_ident(&label)
        } else {
            internal
        };
        ps.push(P {
            cname,
            label,
            marshal: m,
        });
    }

    // Return marshaling. An init constructs the owning type — a failable
    // `init?` yields `Owner?`, so it takes the optional (nil-unwrap) path.
    let ret = if is_init {
        if decl.contains("init?") {
            Marshal::OptHandle(owner.to_string())
        } else {
            Marshal::Handle(owner.to_string())
        }
    } else {
        let r = sym
            .get("functionSignature")
            .and_then(|s| s.get("returns"))
            .and_then(|v| v.as_array())
            .map(|frs| frag_spellings(frs).trim().to_string())
            .unwrap_or_default();
        marshal_of(&r, types, raw_enums).ok_or_else(|| format!("return `{r}` has no C ABI"))?
    };
    if matches!(ret, Marshal::Slice { .. }) {
        return Err("array return not modelled".into());
    }

    let cname = if is_init {
        // One `new` per type in M1; extra inits are skipped to avoid collisions.
        format!("{prefix}_{owner}_new{suffix}")
    } else {
        format!("{prefix}_{owner}_{}{suffix}", sanitize_ident(&base))
    };

    // --- Build the Swift thunk ---------------------------------------------
    let mut swift_params: Vec<String> = Vec::new();
    if !is_init && !is_static {
        swift_params.push("_ self_: UnsafeMutableRawPointer?".to_string());
    }
    for p in &ps {
        match &p.marshal {
            Marshal::Scalar { swift, .. } => {
                swift_params.push(format!("_ {}: {}", p.cname, swift))
            }
            Marshal::Str => {
                swift_params.push(format!("_ {}: UnsafePointer<UInt8>?", p.cname));
                swift_params.push(format!("_ {}_len: Int", p.cname));
            }
            Marshal::Handle(_) => {
                swift_params.push(format!("_ {}: UnsafeMutableRawPointer?", p.cname))
            }
            Marshal::Slice { swift, .. } => {
                swift_params.push(format!("_ {}: UnsafePointer<{swift}>?", p.cname));
                swift_params.push(format!("_ {}_count: Int", p.cname));
            }
            Marshal::EnumScalar(_) => swift_params.push(format!("_ {}: Int64", p.cname)),
            Marshal::OptHandle(_) | Marshal::Void => {}
        }
    }

    let (swift_ret, fail_ret): (String, String) = match &ret {
        Marshal::Handle(_) | Marshal::OptHandle(_) => {
            ("UnsafeMutableRawPointer?".into(), "return nil".into())
        }
        Marshal::Scalar { swift, .. } => {
            (swift.clone(), format!("return {}", scalar_default(swift)))
        }
        Marshal::EnumScalar(_) => ("Int64".into(), "return 0".into()),
        Marshal::Str => ("UnsafeMutablePointer<CChar>?".into(), "return nil".into()),
        Marshal::Void => ("".into(), "return".into()),
        Marshal::Slice { .. } => unreachable!("slice return rejected earlier"),
    };

    let mut body = String::new();
    body.push_str("    cpClearError()\n");
    if !is_init && !is_static {
        body.push_str(&format!(
            "    guard let _self = cpObject(self_, as: {owner_box}.self) else {{ {fail_ret} }}\n"
        ));
    }
    // Unbox handle/string params and materialize slices up front.
    for p in &ps {
        match &p.marshal {
            Marshal::Str => body.push_str(&format!(
                "    guard let _{n} = cpStringFromBytes({n}, {n}_len) else {{ {fail_ret} }}\n",
                n = p.cname
            )),
            Marshal::Handle(t) => body.push_str(&format!(
                "    guard let _{n} = cpObject({n}, as: {b}.self) else {{ {fail_ret} }}\n",
                n = p.cname,
                b = box_name(t)
            )),
            Marshal::Slice { swift, .. } => body.push_str(&format!(
                "    let _{n} = {n} != nil ? Array(UnsafeBufferPointer(start: {n}, count: {n}_count)) : [{swift}]()\n",
                n = p.cname
            )),
            Marshal::EnumScalar(name) => body.push_str(&format!(
                "    guard let _{n} = {name}(rawValue: numericCast({n})) else {{ cpSetError(\"invalid {name} raw value\"); {fail_ret} }}\n",
                n = p.cname
            )),
            _ => {}
        }
    }

    // The Swift call expression with external labels.
    let mut call_args: Vec<String> = Vec::new();
    for p in &ps {
        let val = match &p.marshal {
            Marshal::Scalar { .. } => p.cname.clone(),
            Marshal::Str => format!("_{}", p.cname),
            Marshal::Handle(_) => format!("_{}.value", p.cname),
            Marshal::Slice { .. } => format!("_{}", p.cname),
            Marshal::EnumScalar(_) => format!("_{}", p.cname),
            Marshal::OptHandle(_) | Marshal::Void => continue,
        };
        if p.label == "_" {
            call_args.push(val);
        } else {
            call_args.push(format!("{}: {}", p.label, val));
        }
    }
    let args = call_args.join(", ");
    let callee = if is_init {
        format!("{owner_swift}({args})")
    } else if is_static {
        format!("{owner_swift}.{base}({args})")
    } else {
        format!("_self.value.{base}({args})")
    };
    let mut call = callee;
    if is_async {
        call = format!("try cpWaitForAsync {{ try await {call} }}");
    } else if throws {
        call = format!("try {call}");
    }

    let needs_try = is_async || throws;
    // Emit the result handling.
    let result_stmt = match &ret {
        Marshal::Handle(t) => format!("return cpRetained({}(_result))", box_name(t)),
        Marshal::OptHandle(t) => format!(
            "if let r = _result {{ return cpRetained({}(r)) }} else {{ return nil }}",
            box_name(t)
        ),
        Marshal::Scalar { .. } => "return _result".to_string(),
        Marshal::EnumScalar(_) => "return Int64(_result.rawValue)".to_string(),
        Marshal::Str => "return strdup(_result)".to_string(),
        Marshal::Void => "return".to_string(),
        Marshal::Slice { .. } => unreachable!("slice return rejected earlier"),
    };
    let is_void = matches!(ret, Marshal::Void);
    if needs_try && is_void {
        body.push_str(&format!(
            "    do {{\n        {call}\n        return\n    }} catch {{ cpSetError(\"{owner}.{base}: \\(error)\"); {fail_ret} }}\n"
        ));
    } else if needs_try {
        body.push_str(&format!(
            "    do {{\n        let _result = {call}\n        {result_stmt}\n    }} catch {{ cpSetError(\"{owner}.{base}: \\(error)\"); {fail_ret} }}\n"
        ));
    } else if is_void {
        body.push_str(&format!("    {call}\n    return\n"));
    } else {
        body.push_str(&format!("    let _result = {call}\n    {result_stmt}\n"));
    }

    let ret_clause = if swift_ret.is_empty() {
        String::new()
    } else {
        format!(" -> {swift_ret}")
    };
    let thunk = format!(
        "@_cdecl(\"{cname}\")\npublic func {cname}({}){ret_clause} {{\n{body}}}\n\n",
        swift_params.join(", ")
    );

    // --- Build the C+ extern + wrapper -------------------------------------
    let mut extern_params: Vec<String> = Vec::new();
    if !is_init && !is_static {
        extern_params.push("receiver: *u8".to_string());
    }
    for p in &ps {
        match &p.marshal {
            Marshal::Scalar { cplus, .. } => extern_params.push(format!("{}: {cplus}", p.cname)),
            Marshal::Str => {
                extern_params.push(format!("{}: *u8", p.cname));
                extern_params.push(format!("{}_len: usize", p.cname));
            }
            Marshal::Handle(_) => extern_params.push(format!("{}: *u8", p.cname)),
            Marshal::Slice { cplus, .. } => {
                extern_params.push(format!("{}: *{cplus}", p.cname));
                extern_params.push(format!("{}_count: usize", p.cname));
            }
            Marshal::EnumScalar(_) => extern_params.push(format!("{}: i64", p.cname)),
            Marshal::OptHandle(_) | Marshal::Void => {}
        }
    }
    let extern_ret = match &ret {
        Marshal::Handle(_) | Marshal::OptHandle(_) | Marshal::Str => " -> *u8".to_string(),
        Marshal::Scalar { cplus, .. } => format!(" -> {cplus}"),
        Marshal::EnumScalar(_) => " -> i64".to_string(),
        Marshal::Void => "".to_string(),
        Marshal::Slice { .. } => unreachable!("slice return rejected earlier"),
    };
    let extern_line = format!(
        "#[link_name = \"{cname}\"]\nextern fn {cname}({}){extern_ret};\n",
        extern_params.join(", ")
    );

    // Wrapper signature.
    let mut wrap_params: Vec<String> = Vec::new();
    if !is_init && !is_static {
        wrap_params.push("this".to_string());
    }
    for p in &ps {
        let ty = match &p.marshal {
            Marshal::Scalar { cplus, .. } => cplus.clone(),
            Marshal::Str => "str".to_string(),
            Marshal::Handle(t) => cident(t),
            Marshal::Slice { cplus, .. } => format!("{cplus}[]"),
            Marshal::EnumScalar(_) => "i64".to_string(),
            Marshal::OptHandle(_) | Marshal::Void => continue,
        };
        if p.label == "_" {
            wrap_params.push(format!("_ {}: {ty}", p.cname));
        } else {
            wrap_params.push(format!("{}: {ty}", p.cname));
        }
    }
    let wrap_name = if is_init {
        format!("new{suffix}")
    } else {
        format!("{}{suffix}", sanitize_ident(&base))
    };

    // Call arguments to the extern.
    let mut ecall: Vec<String> = Vec::new();
    if !is_init && !is_static {
        ecall.push("this._raw".to_string());
    }
    for p in &ps {
        match &p.marshal {
            Marshal::Scalar { .. } => ecall.push(p.cname.clone()),
            Marshal::Str => {
                ecall.push(format!("#str_ptr({})", p.cname));
                ecall.push(format!("#str_len({})", p.cname));
            }
            Marshal::Handle(_) => ecall.push(format!("{}._raw", p.cname)),
            Marshal::Slice { .. } => {
                ecall.push(format!("#slice_ptr({})", p.cname));
                ecall.push(format!("#slice_len({})", p.cname));
            }
            Marshal::EnumScalar(_) => ecall.push(p.cname.clone()),
            Marshal::OptHandle(_) | Marshal::Void => {}
        }
    }
    let ecall = ecall.join(", ");

    let wrapper = match &ret {
        // A non-optional handle is never nil from Swift, but the thunk still
        // returns nil on a null receiver — so both map to `Option[T]`.
        Marshal::Handle(t) | Marshal::OptHandle(t) => {
            let t = cident(t);
            format!(
                "    fn {wrap_name}({}) -> option::Option[{t}] {{\n        let raw: *u8 = {{ {cname}({ecall}) }};\n        if is_null(raw) {{ return option::Option[{t}]::None; }}\n        return option::some({t} {{ _raw: raw }});\n    }}\n",
                wrap_params.join(", ")
            )
        }
        Marshal::Scalar { cplus, .. } => format!(
            "    fn {wrap_name}({}) -> {cplus} {{\n        return {{ {cname}({ecall}) }};\n    }}\n",
            wrap_params.join(", ")
        ),
        Marshal::EnumScalar(_) => format!(
            "    fn {wrap_name}({}) -> i64 {{\n        return {{ {cname}({ecall}) }};\n    }}\n",
            wrap_params.join(", ")
        ),
        Marshal::Str => format!(
            "    fn {wrap_name}({}) -> option::Option[text::Text] {{\n{}    }}\n",
            wrap_params.join(", "),
            str_copyout_body(&format!("{cname}({ecall})"))
        ),
        Marshal::Void => format!(
            "    fn {wrap_name}({}) {{\n        {{ {cname}({ecall}); }}\n        return;\n    }}\n",
            wrap_params.join(", ")
        ),
        Marshal::Slice { .. } => unreachable!("slice return rejected earlier"),
    };

    Ok(MemberEmit {
        thunk,
        extern_line,
        wrapper,
        uses_text: matches!(ret, Marshal::Str),
    })
}

/// The framework-independent Swift plumbing (error channel, boxing, async), with
/// the package's C symbol prefix baked into the two exported `@_cdecl` helpers.
fn swift_plumbing(prefix: &str) -> String {
    format!(
        "// ── shared bridge runtime ──────────────────────────────────────────\n\
         private let _cpErrLock = NSLock()\n\
         private var _cpLastError = \"\"\n\
         private func cpSetError(_ m: String) {{ _cpErrLock.lock(); _cpLastError = m; _cpErrLock.unlock() }}\n\
         private func cpClearError() {{ cpSetError(\"\") }}\n\
         private func cpStringFromBytes(_ p: UnsafePointer<UInt8>?, _ n: Int) -> String? {{\n\
         \x20   guard let p else {{ cpSetError(\"null string pointer\"); return nil }}\n\
         \x20   return String(decoding: UnsafeBufferPointer(start: p, count: n), as: UTF8.self)\n\
         }}\n\
         private func cpRetained(_ o: AnyObject) -> UnsafeMutableRawPointer {{ Unmanaged.passRetained(o).toOpaque() }}\n\
         private func cpObject<T: AnyObject>(_ h: UnsafeMutableRawPointer?, as _: T.Type) -> T? {{\n\
         \x20   guard let h else {{ cpSetError(\"null handle\"); return nil }}\n\
         \x20   guard let v = Unmanaged<AnyObject>.fromOpaque(h).takeUnretainedValue() as? T else {{ cpSetError(\"handle has unexpected type\"); return nil }}\n\
         \x20   return v\n\
         }}\n\
         private func cpWaitForAsync<T>(_ body: @escaping () async throws -> T) throws -> T {{\n\
         \x20   let sem = DispatchSemaphore(value: 0)\n\
         \x20   var outcome: Result<T, Error>!\n\
         \x20   Task {{ do {{ outcome = .success(try await body()) }} catch {{ outcome = .failure(error) }}; sem.signal() }}\n\
         \x20   sem.wait()\n\
         \x20   return try outcome.get()\n\
         }}\n\
         @_cdecl(\"{prefix}_last_error\")\n\
         public func {prefix}_last_error(_ buf: UnsafeMutablePointer<UInt8>?, _ len: Int) -> Int32 {{\n\
         \x20   guard let buf, len > 0 else {{ return -1 }}\n\
         \x20   _cpErrLock.lock(); let bytes = Array(_cpLastError.utf8); _cpErrLock.unlock()\n\
         \x20   let n = min(bytes.count, len - 1)\n\
         \x20   for i in 0..<n {{ buf[i] = bytes[i] }}\n\
         \x20   buf[n] = 0\n\
         \x20   return Int32(n)\n\
         }}\n\
         @_cdecl(\"{prefix}_release\")\n\
         public func {prefix}_release(_ h: UnsafeMutableRawPointer?) {{\n\
         \x20   guard let h else {{ return }}\n\
         \x20   Unmanaged<AnyObject>.fromOpaque(h).release()\n\
         }}\n\n"
    )
}

/// Build a non-generic view of a generic member for one concrete element type:
/// drop `swiftGenerics`, rewrite a `some Sequence` parameter to a `[<elem>]`
/// slice, and substitute the generic parameter name(s) with `<elem>` in the decl
/// / params / return. A still-generic return (`View<elem>`) is left intact so
/// `emit_member` self-gates (it isn't a bound handle type).
fn instantiate_symbol(sym: &Value, gnames: &[String], elem: &str) -> Value {
    let subst = |sp: &str| -> String {
        let mut s = sp.to_string();
        for g in gnames {
            s = s.replace(g.as_str(), elem);
        }
        s
    };
    let mut s = sym.clone();
    if let Some(obj) = s.as_object_mut() {
        obj.remove("swiftGenerics");
        // Clean symbol-level decl so the blanket generic/`some` skips don't fire.
        let mut decl = frags(sym);
        if let Some(i) = decl.find(" where ") {
            decl.truncate(i);
        }
        for g in gnames {
            decl = decl.replace(&format!("<{g}>"), "");
        }
        decl = subst(&decl)
            .replace("some Sequence", &format!("[{elem}]"))
            .replace("some Collection", &format!("[{elem}]"));
        obj.insert(
            "declarationFragments".into(),
            serde_json::json!([{ "spelling": decl, "kind": "text" }]),
        );
        if let Some(fs) = obj.get_mut("functionSignature").and_then(|f| f.as_object_mut()) {
            if let Some(params) = fs.get_mut("parameters").and_then(|p| p.as_array_mut()) {
                for p in params.iter_mut() {
                    let pty = type_after_colon(
                        p.get("declarationFragments")
                            .and_then(|v| v.as_array())
                            .map(|a| a.as_slice())
                            .unwrap_or(&[]),
                    );
                    let name = p
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("arg")
                        .to_string();
                    if pty.starts_with("some ") {
                        if let Some(po) = p.as_object_mut() {
                            po.insert(
                                "declarationFragments".into(),
                                serde_json::json!([
                                    {"spelling": name, "kind": "identifier"},
                                    {"spelling": ": ", "kind": "text"},
                                    {"spelling": "[", "kind": "text"},
                                    {"spelling": elem, "kind": "typeIdentifier"},
                                    {"spelling": "]", "kind": "text"},
                                ]),
                            );
                        }
                    } else if let Some(frs) =
                        p.get_mut("declarationFragments").and_then(|v| v.as_array_mut())
                    {
                        for f in frs.iter_mut() {
                            if let Some(sp) = f.get("spelling").and_then(|x| x.as_str()) {
                                let ns = subst(sp);
                                if let Some(fo) = f.as_object_mut() {
                                    fo.insert("spelling".into(), Value::String(ns));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(rets) = fs.get_mut("returns").and_then(|r| r.as_array_mut()) {
                for f in rets.iter_mut() {
                    if let Some(sp) = f.get("spelling").and_then(|x| x.as_str()) {
                        let ns = subst(sp);
                        if let Some(fo) = f.as_object_mut() {
                            fo.insert("spelling".into(), Value::String(ns));
                        }
                    }
                }
            }
        }
    }
    s
}

/// Generate a full Swift-bridge package from a parsed symbol graph. `module` is
/// the symbol-graph module (drives C names and box types); `link` is the
/// importable framework when the module is a framework submodule (e.g. the
/// `CoreAIRuntime` graph is imported as `CoreAI`).
pub fn generate_bridge(
    graph: &Value,
    module: &str,
    link: Option<&str>,
    spec: &BridgeSpec,
) -> BridgeFiles {
    use std::collections::{BTreeMap, BTreeSet};
    let import_module = link.unwrap_or(module);
    let symbols: Vec<Value> = graph
        .get("symbols")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // memberOf: child precise → parent precise.
    let mut member_of: HashMap<String, String> = HashMap::new();
    if let Some(rels) = graph.get("relationships").and_then(|v| v.as_array()) {
        for r in rels {
            if r.get("kind").and_then(|v| v.as_str()) == Some("memberOf") {
                if let (Some(s), Some(t)) = (
                    r.get("source").and_then(|v| v.as_str()),
                    r.get("target").and_then(|v| v.as_str()),
                ) {
                    member_of.insert(s.to_string(), t.to_string());
                }
            }
        }
    }

    let is_pub = |s: &Value| access(s) == "public" || access(s) == "open";

    // Types that become opaque handles. A box stores `var value: T`, which
    // requires `T` to be Escapable — and `~Escapable` is not detectable from the
    // symbol graph. The safe, principled set: top-level types (the main API
    // surface, all Escapable) at any kind, plus enums at any depth (enums are
    // always Escapable). Nested *structs* are excluded — that is where the
    // `~Escapable` view/span types live (`NDArray.RawView`, …). Generic types
    // are skipped too: a box needs a concrete `T`, not `Foo<T>`.
    let mut type_names: BTreeSet<String> = BTreeSet::new();
    let mut type_precise: BTreeMap<String, String> = BTreeMap::new(); // dotted name → precise
    // Types safe to copy out of `self` for a property getter: classes (always
    // Copyable — a copy is a retain) plus whatever the spec vouches for.
    let mut copy_safe: BTreeSet<String> = spec.copyable.clone();
    for s in &symbols {
        let k = kind_of(s);
        if !matches!(k, "swift.class" | "swift.struct" | "swift.enum") || !is_pub(s) {
            continue;
        }
        if s.get("swiftGenerics").is_some() || frags(s).contains('<') {
            continue;
        }
        let top_level = s
            .get("pathComponents")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
            == 1;
        if !top_level && k != "swift.enum" {
            continue;
        }
        let name = path_of(s);
        // A spec-declared raw-value enum binds as an i64 scalar, not a handle —
        // keep it out of the boxed-handle set.
        if spec.raw_enums.contains(&name) {
            continue;
        }
        if k == "swift.class" {
            copy_safe.insert(name.clone());
        }
        type_names.insert(name.clone());
        type_precise.insert(name, precise(s).to_string());
    }

    // Group members by owning type precise.
    let precise_to_name: HashMap<String, String> = type_precise
        .iter()
        .map(|(n, p)| (p.clone(), n.clone()))
        .collect();
    let mut members_by_type: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut cases_by_enum: HashMap<String, Vec<&Value>> = HashMap::new();
    for s in &symbols {
        if kind_of(s) == "swift.enum.case" {
            if let Some(parent) = member_of.get(precise(s)) {
                cases_by_enum.entry(parent.clone()).or_default().push(s);
            }
            continue;
        }
        if let Some(parent) = member_of.get(precise(s)) {
            if let Some(name) = precise_to_name.get(parent) {
                if is_pub(s) {
                    members_by_type.entry(name.clone()).or_default().push(s);
                }
            }
        }
    }

    let lower = module.to_lowercase();
    let prefix = format!("cplus_{}", sanitize_ident(&lower));

    let mut thunks = String::new();
    let mut externs = String::new();
    let mut impls = String::new();
    let mut free = String::new();
    let mut handle_structs = String::new();
    let mut boxes = String::new();
    let mut uses_text = false;
    let mut emitted = 0usize;
    let mut skipped = 0usize;
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let skip = |reasons: &mut BTreeMap<String, usize>, n: &mut usize, body: &mut String, path: &str, reason: String| {
        let bucket = reason
            .split(|c| c == '(' || c == '`' || c == '—')
            .next()
            .unwrap_or(&reason)
            .trim()
            .to_string();
        *reasons.entry(bucket).or_insert(0) += 1;
        *n += 1;
        body.push_str(&format!("    // SKIPPED {path}: {reason}\n"));
    };

    // Emit one handle struct + box + impl block per type. `name` is the dotted
    // Swift spelling; `ident` is its flattened C+ identifier.
    for (name, _p) in &type_precise {
        let ident = cident(name);
        // `consuming` on the init param lets the box hold a noncopyable
        // (`~Copyable`) value too — noncopyability is not detectable from the
        // symbol graph, and copyable types accept a consuming param unchanged.
        // A spec-declared `~Copyable` owner uses OPTIONAL storage so a member
        // read can move the value out (`cpTakeOut`) exactly once.
        if spec.noncopyable_owners.contains(name) {
            boxes.push_str(&format!(
                "private final class {b} {{ var value: {name}?; init(_ v: consuming {name}) {{ value = consume v }} }}\n",
                b = box_name(name)
            ));
        } else {
            boxes.push_str(&format!(
                "private final class {b} {{ var value: {name}; init(_ v: consuming {name}) {{ value = v }} }}\n",
                b = box_name(name)
            ));
        }
        handle_structs.push_str(&format!("struct {ident} {{\n    _raw: *u8,\n}}\n\n"));

        let mut impl_body = String::new();
        impl_body.push_str(&format!(
            "    fn drop(ref this) {{\n        if !is_null(this._raw) {{ {{ {prefix}_release(this._raw); }} this._raw = null(); }}\n        return;\n    }}\n"
        ));
        let mut had_init = false;
        if let Some(members) = members_by_type.get(name) {
            for m in members {
                let mpath = path_of(m);
                // Generic instantiation: emit one binding per spec element type
                // (`<base>_<Type>`); the generic is otherwise skipped.
                if let Some(elems) = spec.instantiate.get(&mpath) {
                    let gnames: Vec<String> = m
                        .get("swiftGenerics")
                        .and_then(|g| g.get("parameters"))
                        .and_then(|p| p.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| {
                                    x.get("name").and_then(|n| n.as_str()).map(String::from)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    for elem in elems {
                        let syn = instantiate_symbol(m, &gnames, elem);
                        let suffix = format!("_{}", cident(elem));
                        match emit_member(&syn, &ident, name, &prefix, &type_names, &copy_safe, &spec.raw_enums, spec.noncopyable_owners.contains(name), &suffix) {
                            Ok(e) => {
                                thunks.push_str(&e.thunk);
                                externs.push_str(&e.extern_line);
                                impl_body.push_str(&e.wrapper);
                                uses_text = uses_text || e.uses_text;
                                emitted += 1;
                            }
                            Err(reason) => skip(&mut skip_reasons, &mut skipped, &mut impl_body, &format!("{mpath} [{elem}]"), reason),
                        }
                    }
                    continue;
                }
                // One `new` per type in M1.
                if kind_of(m) == "swift.init" && had_init {
                    skip(&mut skip_reasons, &mut skipped, &mut impl_body, &mpath, "extra init variant (one `new` per type in M1)".into());
                    continue;
                }
                match emit_member(m, &ident, name, &prefix, &type_names, &copy_safe, &spec.raw_enums, spec.noncopyable_owners.contains(name), "") {
                    Ok(e) => {
                        thunks.push_str(&e.thunk);
                        externs.push_str(&e.extern_line);
                        impl_body.push_str(&e.wrapper);
                        uses_text = uses_text || e.uses_text;
                        emitted += 1;
                        if kind_of(m) == "swift.init" {
                            had_init = true;
                        }
                    }
                    Err(reason) => skip(&mut skip_reasons, &mut skipped, &mut impl_body, &mpath, reason),
                }
            }
        }
        impls.push_str(&format!("impl {ident} {{\n{impl_body}}}\n\n"));
    }

    // Bonus: a raw-value enum whose integer values are present in the graph also
    // gets i64 constant accessors (on top of its opaque handle). When the values
    // are absent (the common symbolgraph case) the enum is just a handle — no
    // skip, since it is already bound above.
    for s in &symbols {
        if kind_of(s) != "swift.enum" || !is_pub(s) {
            continue;
        }
        let path = path_of(s);
        let cases = cases_by_enum.get(precise(s)).cloned().unwrap_or_default();
        if cases.is_empty() || cases.iter().any(|c| case_has_payload(c)) {
            continue;
        }
        let raws: Vec<(String, i64)> = cases
            .iter()
            .filter_map(|c| {
                let cn = c
                    .get("pathComponents")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.last())
                    .and_then(|x| x.as_str())?;
                case_raw_value(c).map(|v| (cn.to_string(), v))
            })
            .collect();
        if raws.len() != cases.len() {
            continue;
        }
        free.push_str(&format!("// Swift enum `{path}` — raw values as i64 accessors.\n"));
        for (cn, v) in raws {
            free.push_str(&format!(
                "fn {}_{}() -> i64 {{ return {} as i64; }}\n",
                cident(&path),
                sanitize_ident(&cn),
                v
            ));
            emitted += 1;
        }
    }

    // Per-case accessors for spec-declared raw-value enums: `<Enum>_<case>()`
    // returns the case's `.rawValue`, read at runtime — so the spec lists only
    // the enum, never the values (whatever their integer width).
    for s in &symbols {
        if kind_of(s) != "swift.enum" || !is_pub(s) {
            continue;
        }
        let name = path_of(s);
        if !spec.raw_enums.contains(&name) {
            continue;
        }
        let ident = cident(&name);
        let cases = cases_by_enum.get(precise(s)).cloned().unwrap_or_default();
        free.push_str(&format!("// Swift raw-value enum `{name}` — cases as i64.\n"));
        for c in cases {
            if case_has_payload(c) {
                continue;
            }
            let case = match c
                .get("pathComponents")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|x| x.as_str())
            {
                Some(c) => c,
                None => continue,
            };
            let cn = format!("{prefix}_{ident}_{}", sanitize_ident(case));
            thunks.push_str(&format!(
                "@_cdecl(\"{cn}\")\npublic func {cn}() -> Int64 {{ return Int64({name}.{case}.rawValue) }}\n\n"
            ));
            externs.push_str(&format!("#[link_name = \"{cn}\"]\nextern fn {cn}() -> i64;\n"));
            free.push_str(&format!(
                "fn {ident}_{c}() -> i64 {{ return {{ {cn}() }}; }}\n",
                c = sanitize_ident(case)
            ));
            emitted += 1;
        }
    }

    // Per-case constructors for spec-declared raw-value-less enums (kept as
    // handles): `<Enum>_<case>() -> Option[<Enum>]` boxes `EnumType.case`, so C+
    // can build a value to pass to a method that consumes the enum.
    for s in &symbols {
        if kind_of(s) != "swift.enum" || !is_pub(s) {
            continue;
        }
        let name = path_of(s);
        if !spec.enum_cases.contains(&name) || !type_names.contains(&name) {
            continue;
        }
        let ident = cident(&name);
        let cases = cases_by_enum.get(precise(s)).cloned().unwrap_or_default();
        free.push_str(&format!("// Swift enum `{name}` — case constructors.\n"));
        for c in cases {
            if case_has_payload(c) {
                continue;
            }
            let case = match c
                .get("pathComponents")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|x| x.as_str())
            {
                Some(c) => c,
                None => continue,
            };
            let cn = format!("{prefix}_{ident}_{}", sanitize_ident(case));
            thunks.push_str(&format!(
                "@_cdecl(\"{cn}\")\npublic func {cn}() -> UnsafeMutableRawPointer? {{ return cpRetained({b}({name}.{case})) }}\n\n",
                b = box_name(&name)
            ));
            externs.push_str(&format!("#[link_name = \"{cn}\"]\nextern fn {cn}() -> *u8;\n"));
            free.push_str(&format!(
                "fn {ident}_{c}() -> option::Option[{ident}] {{\n    let raw: *u8 = {{ {cn}() }};\n    if is_null(raw) {{ return option::Option[{ident}]::None; }}\n    return option::some({ident} {{ _raw: raw }});\n}}\n",
                c = sanitize_ident(case)
            ));
            emitted += 1;
        }
    }

    // `view_copy`: bulk data extraction over a `~Escapable` element view. For a
    // handle type with `view(as:)`, emit `<Type>_copy_<elem>` that takes the
    // view *inside* the thunk and memcpy's a contiguous run into a caller buffer
    // — the view never escapes, so it never has to be boxed. The owner must be a
    // bound, Copyable handle (the read binds a local copy of it); a non-scalar
    // element is skipped. (Write-back via `mutableView` is deferred — the mutable
    // view is lifetime-bound and can't be held across the size/copy steps.)
    for (owner, elems) in &spec.view_copy {
        if !type_names.contains(owner) || !copy_safe.contains(owner) {
            continue;
        }
        let ident = cident(owner);
        let bx = box_name(owner);
        let mut wrappers = String::new();
        for elem in elems {
            let c = match map_swift_type(elem) {
                Ok(c) if c != "()" && !c.starts_with('*') => c,
                _ => continue, // non-scalar element has no C representation
            };
            let suf = cident(&c);
            let copy = format!("{prefix}_{ident}_copy_{suf}");
            thunks.push_str(&format!(
                "@_cdecl(\"{copy}\")\npublic func {copy}(_ self_: UnsafeMutableRawPointer?, _ dest: UnsafeMutablePointer<{elem}>?, _ count: Int64) -> Int64 {{\n\
                 \x20   guard let _self = cpObject(self_, as: {bx}.self), let dest else {{ cpSetError(\"null handle/dest\"); return -1 }}\n\
                 \x20   let tmp = _self.value\n\
                 \x20   let view = tmp.view(as: {elem}.self)\n\
                 \x20   var n = 1; for i in 0..<view.shape.count {{ n *= view.shape[i] }}\n\
                 \x20   guard n <= Int(count) else {{ cpSetError(\"destination too small\"); return -1 }}\n\
                 \x20   guard view.isContiguous else {{ cpSetError(\"non-contiguous view unsupported\"); return -1 }}\n\
                 \x20   do {{ try view.withUnsafePointer {{ p, _, _ in for i in 0..<n {{ dest[i] = p[i] }} }} }} catch {{ cpSetError(\"\\(error)\"); return -1 }}\n\
                 \x20   return Int64(n)\n}}\n\n"
            ));
            externs.push_str(&format!(
                "#[link_name = \"{copy}\"]\nextern fn {copy}(receiver: *u8, dest: *{c}, count: i64) -> i64;\n"
            ));
            wrappers.push_str(&format!(
                "    fn copy_{suf}(this, dest: {c}[]) -> i64 {{\n        return {{ {copy}(this._raw, #slice_ptr(dest), #slice_len(dest) as i64) }};\n    }}\n"
            ));
            emitted += 1;
        }
        if !wrappers.is_empty() {
            impls.push_str(&format!("impl {ident} {{\n{wrappers}}}\n\n"));
        }
    }

    // Pre-existing @_cdecl / @convention(c) free functions: bind the C symbol
    // directly (no thunk needed — they already export a flat C ABI).
    for s in &symbols {
        if kind_of(s) != "swift.func" || !is_pub(s) {
            continue;
        }
        if member_of.contains_key(precise(s)) {
            continue; // members handled above
        }
        match classify_function(s) {
            FnVerdict::Emit(line) => {
                externs.push_str(&line);
                free.push_str(&format!(
                    "// (already C-callable: {})\n",
                    path_of(s)
                ));
                emitted += 1;
            }
            FnVerdict::Skip(reason) => {
                skip(&mut skip_reasons, &mut skipped, &mut free, &path_of(s), reason)
            }
        }
    }

    // ── Assemble the four files ────────────────────────────────────────────
    let mut swift = String::new();
    swift.push_str(&format!(
        "// {module}Bridge.swift — auto-generated by cpc-bindgen (--swift-bridge). DO NOT EDIT.\n\
         // @_cdecl bridge: owns the Swift values, exports a flat C ABI for C+.\n\n\
         import {import_module}\nimport Foundation\n\n"
    ));
    swift.push_str(&swift_plumbing(&prefix));
    if !spec.noncopyable_owners.is_empty() {
        // Move a `~Copyable` value out of an optional box slot, once.
        swift.push_str(
            "@inline(__always) private func cpTakeOut<T: ~Copyable>(_ slot: inout T?) -> T? {\n    var moved: T? = nil; swap(&moved, &slot); return moved\n}\n\n",
        );
    }
    if !boxes.is_empty() {
        swift.push_str("// ── boxes (stable opaque-handle identity for value/reference types) ──\n");
        swift.push_str(&boxes);
        swift.push('\n');
    }
    swift.push_str("// ── thunks ──────────────────────────────────────────────────────────\n");
    swift.push_str(&thunks);

    let mut cplus = String::new();
    cplus.push_str(&format!(
        "// {lower}.cplus — auto-generated by cpc-bindgen (--swift-bridge). DO NOT EDIT.\n\
         // C+ facade over the {module} Swift bridge: opaque handles + ergonomic wrappers.\n\n\
         import \"stdlib/option\" as option;\n"
    ));
    // String returns copy out of a malloc'd C string into an owned `Text`.
    if uses_text {
        cplus.push_str(
            "import \"stdlib/text\" as text;\n\
             extern fn strlen(s: *u8) -> usize;\n\
             extern fn free(p: *u8);\n",
        );
    }
    cplus.push_str(&format!(
        "\n#[link_name = \"{prefix}_last_error\"]\nextern fn {prefix}_last_error(buf: *u8, len: usize) -> i32;\n\
         #[link_name = \"{prefix}_release\"]\nextern fn {prefix}_release(handle: *u8);\n\n"
    ));
    cplus.push_str(&externs);
    cplus.push_str("\nfn null() -> *u8 { return { 0 as *u8 }; }\nfn is_null(p: *u8) -> bool { return p == null(); }\n\n");
    cplus.push_str("fn last_error_into(buf: u8[]) -> usize {\n    let n: usize = #slice_len(buf);\n    if n == (0 as usize) { return 0 as usize; }\n    let wrote: i32 = { ");
    cplus.push_str(&format!("{prefix}_last_error"));
    cplus.push_str("(#slice_ptr(buf), n) };\n    if wrote < 0 { return 0 as usize; }\n    return wrote as usize;\n}\n\n");
    cplus.push_str(&handle_structs);
    cplus.push_str(&impls);
    if !free.is_empty() {
        cplus.push_str(&free);
    }

    let header = format!(
        "#ifndef {guard}\n#define {guard}\n#include <stdint.h>\n#include <stddef.h>\n#ifdef __cplusplus\nextern \"C\" {{\n#endif\n\n\
         // C ABI exported by lib{lower}_bridge.dylib (see {module}Bridge.swift).\n\
         int32_t {prefix}_last_error(uint8_t *buf, size_t len);\n\
         void {prefix}_release(void *handle);\n\n\
         #ifdef __cplusplus\n}}\n#endif\n#endif\n",
        guard = format!("{}_BRIDGE_H", prefix.to_uppercase()),
    );

    let build_sh = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\ncd \"$(dirname \"$0\")\"\nmkdir -p bridge/build\nSDK=\"$(xcrun --sdk macosx --show-sdk-path)\"\nARCH=\"$(uname -m)\"\nxcrun swiftc \\\n  -sdk \"$SDK\" -target \"${{ARCH}}-apple-macos27.0\" \\\n  -parse-as-library -emit-library -emit-module \\\n  -module-name {module}Bridge \\\n  bridge/{module}Bridge.swift \\\n  -o bridge/build/lib{lower}_bridge.dylib\necho \"built bridge/build/lib{lower}_bridge.dylib\"\n"
    );

    BridgeFiles {
        swift,
        cplus,
        header,
        build_sh,
        emitted,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frag(spelling: &str, kind: &str) -> Value {
        json!({ "spelling": spelling, "kind": kind })
    }

    fn graph(symbols: Vec<Value>, rels: Vec<Value>) -> Value {
        json!({ "symbols": symbols, "relationships": rels })
    }

    #[test]
    fn raw_value_enum_emits_constants() {
        let e = json!({
            "kind": {"identifier": "swift.enum"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Color"],
            "accessLevel": "public",
            "declarationFragments": [frag("enum", "keyword"), frag(" Color", "text")],
        });
        let mk_case = |name: &str, raw: i64, id: &str| {
            json!({
                "kind": {"identifier": "swift.enum.case"},
                "identifier": {"precise": id},
                "pathComponents": ["Color", name],
                "accessLevel": "public",
                "names": {"title": format!("Color.{name}")},
                "declarationFragments": [frag(&format!("case {name} = {raw}"), "text")],
            })
        };
        let g = graph(
            vec![e, mk_case("red", 0, "c0"), mk_case("green", 7, "c1")],
            vec![
                json!({"kind":"memberOf","source":"c0","target":"E"}),
                json!({"kind":"memberOf","source":"c1","target":"E"}),
            ],
        );
        let out = generate(&g, "Demo");
        assert!(out.contains("fn red() -> i64 { return 0 as i64; }"), "{out}");
        assert!(out.contains("fn green() -> i64 { return 7 as i64; }"), "{out}");
        assert!(out.contains("2 emitted"), "{out}");
    }

    #[test]
    fn associated_value_enum_is_skipped() {
        let e = json!({
            "kind": {"identifier": "swift.enum"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Shape"],
            "accessLevel": "public",
            "declarationFragments": [frag("enum Shape", "text")],
        });
        let c = json!({
            "kind": {"identifier": "swift.enum.case"},
            "identifier": {"precise": "c0"},
            "pathComponents": ["Shape", "circle(_:)"],
            "accessLevel": "public",
            "names": {"title": "Shape.circle(_:)"},
            "declarationFragments": [frag("case circle(Double)", "text")],
        });
        let g = graph(
            vec![e, c],
            vec![json!({"kind":"memberOf","source":"c0","target":"E"})],
        );
        let out = generate(&g, "Demo");
        assert!(out.contains("SKIPPED Shape: enum with associated values"), "{out}");
    }

    #[test]
    fn plain_enum_without_raw_values_is_skipped() {
        let e = json!({
            "kind": {"identifier": "swift.enum"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Dir"],
            "accessLevel": "public",
            "declarationFragments": [frag("enum Dir", "text")],
        });
        let mk = |name: &str, id: &str| json!({
            "kind": {"identifier": "swift.enum.case"},
            "identifier": {"precise": id},
            "pathComponents": ["Dir", name],
            "accessLevel": "public",
            "names": {"title": format!("Dir.{name}")},
            "declarationFragments": [frag(&format!("case {name}"), "text")],
        });
        let g = graph(
            vec![e, mk("north","c0"), mk("south","c1")],
            vec![
                json!({"kind":"memberOf","source":"c0","target":"E"}),
                json!({"kind":"memberOf","source":"c1","target":"E"}),
            ],
        );
        let out = generate(&g, "Demo");
        assert!(out.contains("SKIPPED Dir: plain enum, no integer raw values"), "{out}");
    }

    #[test]
    fn async_method_is_skipped_for_the_right_reason() {
        let m = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "m"},
            "pathComponents": ["Engine", "run()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func run() async throws -> Int", "text")],
        });
        let out = generate(&graph(vec![m], vec![]), "Demo");
        assert!(out.contains("SKIPPED Engine.run(): async"), "{out}");
    }

    #[test]
    fn plain_method_is_skipped_no_c_symbol() {
        let m = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "m"},
            "pathComponents": ["Engine", "name()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func name() -> Int", "text")],
        });
        let out = generate(&graph(vec![m], vec![]), "Demo");
        assert!(out.contains("SKIPPED Engine.name(): instance/type method"), "{out}");
    }

    #[test]
    fn cdecl_function_emits_extern_fn() {
        let f = json!({
            "kind": {"identifier": "swift.func"},
            "identifier": {"precise": "f"},
            "pathComponents": ["c_add(_:_:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("@_cdecl func c_add(Int32, Int32) -> Int32", "text")],
            "functionSignature": {
                "parameters": [
                    {"name": "a", "declarationFragments": [frag("Int32", "typeIdentifier")]},
                    {"name": "b", "declarationFragments": [frag("Int32", "typeIdentifier")]}
                ],
                "returns": [frag("Int32", "typeIdentifier")]
            }
        });
        let out = generate(&graph(vec![f], vec![]), "Demo");
        assert!(out.contains("#[link_name = \"c_add\"]"), "{out}");
        assert!(out.contains("extern fn c_add(a: i32, b: i32) -> i32;"), "{out}");
    }

    #[test]
    fn private_symbols_are_ignored() {
        let s = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "s"},
            "pathComponents": ["Hidden"],
            "accessLevel": "internal",
            "declarationFragments": [frag("struct Hidden", "text")],
        });
        let out = generate(&graph(vec![s], vec![]), "Demo");
        assert!(!out.contains("Hidden"), "{out}");
    }

    #[test]
    fn scalar_type_mapping() {
        assert_eq!(map_swift_type("Int32").unwrap(), "i32");
        assert_eq!(map_swift_type("Double").unwrap(), "f64");
        assert_eq!(map_swift_type("Bool").unwrap(), "bool");
        assert_eq!(map_swift_type("UnsafeMutablePointer<Float>").unwrap(), "*f32");
        assert_eq!(map_swift_type("UnsafeRawPointer").unwrap(), "*u8");
        assert!(map_swift_type("String").is_err());
    }

    // ── bridge emitter ─────────────────────────────────────────────────────

    fn typed_param(name: &str, ty: &str) -> Value {
        json!({
            "name": name,
            "declarationFragments": [
                frag(name, "identifier"),
                frag(": ", "text"),
                frag(ty, "typeIdentifier"),
            ]
        })
    }

    /// A class `Engine` with an init, a throwing scalar method, an async method,
    /// a scalar property, and a generic method — exercising every M1 shape.
    fn engine_graph() -> Value {
        let class = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let init = json!({
            "kind": {"identifier": "swift.init"},
            "identifier": {"precise": "init"},
            "pathComponents": ["Engine", "init(seed:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("init(seed: Int32)", "text")],
            "functionSignature": {"parameters": [typed_param("seed", "Int32")], "returns": []},
        });
        let run = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "run"},
            "pathComponents": ["Engine", "run(input:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func run(input: Int32) throws -> Int32", "text")],
            "functionSignature": {"parameters": [typed_param("input", "Int32")], "returns": [frag("Int32", "typeIdentifier")]},
        });
        let warm = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "warm"},
            "pathComponents": ["Engine", "warm()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func warm() async -> Int32", "text")],
            "functionSignature": {"parameters": [], "returns": [frag("Int32", "typeIdentifier")]},
        });
        let level = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "level"},
            "pathComponents": ["Engine", "level"],
            "accessLevel": "public",
            "declarationFragments": [frag("var level", "text"), frag(": ", "text"), frag("Int32", "typeIdentifier")],
        });
        let transform = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "xf"},
            "pathComponents": ["Engine", "transform(value:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func transform<T>(value: T) -> Int32", "text")],
            "swiftGenerics": {"parameters": [{"name": "T", "index": 0, "depth": 0}]},
            "functionSignature": {"parameters": [typed_param("value", "T")], "returns": [frag("Int32", "typeIdentifier")]},
        });
        graph(
            vec![class, init, run, warm, level, transform],
            vec![
                json!({"kind":"memberOf","source":"init","target":"E"}),
                json!({"kind":"memberOf","source":"run","target":"E"}),
                json!({"kind":"memberOf","source":"warm","target":"E"}),
                json!({"kind":"memberOf","source":"level","target":"E"}),
                json!({"kind":"memberOf","source":"xf","target":"E"}),
            ],
        )
    }

    #[test]
    fn bridge_emits_plumbing_and_box() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(b.swift.contains("import Demo"), "{}", b.swift);
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_release\")"), "{}", b.swift);
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_last_error\")"), "{}", b.swift);
        assert!(
            b.swift.contains("private final class EngineBox { var value: Engine"),
            "{}",
            b.swift
        );
    }

    #[test]
    fn bridge_init_becomes_new_returning_handle() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_Engine_new\")"), "{}", b.swift);
        assert!(b.swift.contains("let _result = Engine(seed: seed)"), "{}", b.swift);
        assert!(b.swift.contains("return cpRetained(EngineBox(_result))"), "{}", b.swift);
        assert!(b.cplus.contains("extern fn cplus_demo_Engine_new(seed: i32) -> *u8;"), "{}", b.cplus);
        assert!(b.cplus.contains("fn new(seed: i32) -> option::Option[Engine]"), "{}", b.cplus);
    }

    #[test]
    fn bridge_throwing_method_wraps_in_do_catch() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_Engine_run\")"), "{}", b.swift);
        assert!(b.swift.contains("guard let _self = cpObject(self_, as: EngineBox.self)"), "{}", b.swift);
        assert!(b.swift.contains("try _self.value.run(input: input)"), "{}", b.swift);
        assert!(b.swift.contains("catch { cpSetError("), "{}", b.swift);
        assert!(b.cplus.contains("fn run(this, input: i32) -> i32"), "{}", b.cplus);
    }

    #[test]
    fn bridge_async_method_uses_wait_for_async() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(
            b.swift.contains("try cpWaitForAsync { try await _self.value.warm() }"),
            "{}",
            b.swift
        );
    }

    #[test]
    fn bridge_scalar_property_emits_getter() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_Engine_level\")"), "{}", b.swift);
        assert!(b.swift.contains("return _self.value.level"), "{}", b.swift);
        assert!(b.cplus.contains("fn level(this) -> i32"), "{}", b.cplus);
    }

    #[test]
    fn bridge_generic_method_is_skipped() {
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(b.cplus.contains("SKIPPED Engine.transform(value:): generic"), "{}", b.cplus);
        assert!(b.skipped >= 1);
        // Everything else was emitted (init, run, warm, level).
        assert!(b.emitted >= 4, "emitted={}", b.emitted);
    }

    #[test]
    fn bridge_box_init_is_consuming() {
        // `consuming` lets the box also hold a noncopyable (`~Copyable`) value.
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        assert!(
            b.swift.contains("init(_ v: consuming Engine) { value = v }"),
            "{}",
            b.swift
        );
    }

    #[test]
    fn bridge_optional_return_and_slice_param_and_dict_skip() {
        // M2: `T?` of a known handle → `Option[T]` (nil-unwrap in the thunk);
        // `[scalar]` param → `(ptr, count)`. A dictionary param still has no
        // bridge and must be skipped with its faithful spelling.
        let class = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let find = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "find"},
            "pathComponents": ["Engine", "find()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func find() -> Engine?", "text")],
            "functionSignature": {"parameters": [], "returns": [frag("Engine", "typeIdentifier"), frag("?", "text")]},
        });
        let tally = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "tally"},
            "pathComponents": ["Engine", "tally(values:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func tally(values: [Int32]) -> Int32", "text")],
            "functionSignature": {
                "parameters": [json!({
                    "name": "values",
                    "declarationFragments": [frag("values", "identifier"), frag(": ", "text"), frag("[", "text"), frag("Int32", "typeIdentifier"), frag("]", "text")]
                })],
                "returns": [frag("Int32", "typeIdentifier")]
            },
        });
        let lookup = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "lookup"},
            "pathComponents": ["Engine", "lookup(map:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func lookup(map: [String : Int]) -> Int", "text")],
            "functionSignature": {
                "parameters": [json!({
                    "name": "map",
                    "declarationFragments": [frag("map", "identifier"), frag(": ", "text"), frag("[", "text"), frag("String", "typeIdentifier"), frag(" : ", "text"), frag("Int", "typeIdentifier"), frag("]", "text")]
                })],
                "returns": [frag("Int", "typeIdentifier")]
            },
        });
        let g = graph(
            vec![class, find, tally, lookup],
            vec![
                json!({"kind":"memberOf","source":"find","target":"E"}),
                json!({"kind":"memberOf","source":"tally","target":"E"}),
                json!({"kind":"memberOf","source":"lookup","target":"E"}),
            ],
        );
        let b = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        // Optional handle return → Option[Engine] with nil-unwrap.
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_Engine_find\")"), "{}", b.swift);
        assert!(b.swift.contains("if let r = _result { return cpRetained(EngineBox(r)) } else { return nil }"), "{}", b.swift);
        assert!(b.cplus.contains("fn find(this) -> option::Option[Engine]"), "{}", b.cplus);
        // [Int32] param → (ptr, count) on the C ABI, `i32[]` slice in C+.
        assert!(b.swift.contains("Array(UnsafeBufferPointer(start: values, count: values_count))"), "{}", b.swift);
        assert!(b.cplus.contains("fn tally(this, values: i32[]) -> i32"), "{}", b.cplus);
        // Dictionary param: still no bridge.
        assert!(b.cplus.contains("SKIPPED Engine.lookup(map:): param `[String : Int]`"), "{}", b.cplus);
        assert!(!b.swift.contains("cplus_demo_Engine_lookup"), "{}", b.swift);
    }

    #[test]
    fn bridge_string_return_and_property_getset() {
        // String return → owned copy-out `Option[Text]`; a stored `var` String
        // property → getter + setter; `let` String → getter only.
        let class = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let greeting = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "greeting"},
            "pathComponents": ["Engine", "greeting()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func greeting() -> String", "text")],
            "functionSignature": {"parameters": [], "returns": [frag("String", "typeIdentifier")]},
        });
        let name = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "name"},
            "pathComponents": ["Engine", "name"],
            "accessLevel": "public",
            "declarationFragments": [frag("let", "keyword"), frag(" name", "text"), frag(": ", "text"), frag("String", "typeIdentifier")],
        });
        let note = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "note"},
            "pathComponents": ["Engine", "note"],
            "accessLevel": "public",
            "declarationFragments": [frag("var", "keyword"), frag(" note", "text"), frag(": ", "text"), frag("String", "typeIdentifier")],
        });
        let g = graph(
            vec![class, greeting, name, note],
            vec![
                json!({"kind":"memberOf","source":"greeting","target":"E"}),
                json!({"kind":"memberOf","source":"name","target":"E"}),
                json!({"kind":"memberOf","source":"note","target":"E"}),
            ],
        );
        let b = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        // String method return → strdup + Option[Text] copy-out.
        assert!(b.swift.contains("return strdup(_result)"), "{}", b.swift);
        assert!(b.cplus.contains("fn greeting(this) -> option::Option[text::Text]"), "{}", b.cplus);
        assert!(b.cplus.contains("import \"stdlib/text\" as text;"), "{}", b.cplus);
        assert!(b.cplus.contains("extern fn strlen(s: *u8) -> usize;"), "{}", b.cplus);
        // `let name` → getter only; `var note` → getter + setter.
        assert!(b.cplus.contains("fn name(this) -> option::Option[text::Text]"), "{}", b.cplus);
        assert!(!b.swift.contains("cplus_demo_Engine_set_name"), "{}", b.swift);
        assert!(b.cplus.contains("fn note(this) -> option::Option[text::Text]"), "{}", b.cplus);
        assert!(b.cplus.contains("fn set_note(this, value: str)"), "{}", b.cplus);
    }

    #[test]
    fn bridge_scalar_property_setter_when_var() {
        // A stored `var` scalar property emits a setter; the M1 `level` getter
        // here is `var`, so it now gains `set_level`.
        let b = generate_bridge(&engine_graph(), "Demo", None, &BridgeSpec::default());
        // engine_graph's `level` is declared `var level`.
        assert!(b.cplus.contains("fn level(this) -> i32"), "{}", b.cplus);
        assert!(b.cplus.contains("fn set_level(this, value: i32)"), "{}", b.cplus);
        assert!(b.swift.contains("_self.value.level = value"), "{}", b.swift);
    }

    #[test]
    fn bridge_failable_init_returns_optional() {
        // `init?` yields `Owner?` — must take the optional (nil-unwrap) path,
        // not box a value that might be nil.
        let class = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let init = json!({
            "kind": {"identifier": "swift.init"},
            "identifier": {"precise": "i"},
            "pathComponents": ["Engine", "init(x:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("init?(x: Int32)", "text")],
            "functionSignature": {"parameters": [typed_param("x", "Int32")], "returns": []},
        });
        let g = graph(vec![class, init], vec![json!({"kind":"memberOf","source":"i","target":"E"})]);
        let b = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        assert!(b.swift.contains("if let r = _result { return cpRetained(EngineBox(r)) } else { return nil }"), "{}", b.swift);
        assert!(b.cplus.contains("fn new(x: i32) -> option::Option[Engine]"), "{}", b.cplus);
    }

    #[test]
    fn bridge_computed_property_strips_accessor_block() {
        // `var count: Int { get }` — the `{ get }` accessor block must not be
        // swept into the type (which would make it unbridgeable). It is a
        // get-only computed property, so no setter.
        let class = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let count = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "c"},
            "pathComponents": ["Engine", "count"],
            "accessLevel": "public",
            "declarationFragments": [frag("var", "keyword"), frag(" count", "text"), frag(": ", "text"), frag("Int", "typeIdentifier"), frag(" { get }", "text")],
        });
        let g = graph(vec![class, count], vec![json!({"kind":"memberOf","source":"c","target":"E"})]);
        let b = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        assert!(b.cplus.contains("fn count(this) -> i64"), "{}", b.cplus);
        assert!(!b.cplus.contains("SKIPPED Engine.count"), "{}", b.cplus);
        assert!(!b.swift.contains("set_count"), "{}", b.swift);
    }

    #[test]
    fn bridge_nested_enum_is_handle_nested_struct_is_not() {
        // A nested enum is Escapable → boxable as a handle (dotted Swift type,
        // flattened C+ identifier). A nested struct may be `~Escapable` (a view)
        // and is not detectable, so it stays out of the handle set.
        let engine = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct Engine", "text")],
        });
        let mode = json!({
            "kind": {"identifier": "swift.enum"},
            "identifier": {"precise": "M"},
            "pathComponents": ["Engine", "Mode"],
            "accessLevel": "public",
            "declarationFragments": [frag("enum Mode", "text")],
        });
        let span = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "S"},
            "pathComponents": ["Engine", "Span"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct Span", "text")],
        });
        let mode_m = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "mm"},
            "pathComponents": ["Engine", "mode()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func mode() -> Engine.Mode", "text")],
            "functionSignature": {"parameters": [], "returns": [frag("Engine", "typeIdentifier"), frag(".", "text"), frag("Mode", "typeIdentifier")]},
        });
        let span_m = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "sm"},
            "pathComponents": ["Engine", "span()"],
            "accessLevel": "public",
            "declarationFragments": [frag("func span() -> Engine.Span", "text")],
            "functionSignature": {"parameters": [], "returns": [frag("Engine", "typeIdentifier"), frag(".", "text"), frag("Span", "typeIdentifier")]},
        });
        let g = graph(
            vec![engine, mode, span, mode_m, span_m],
            vec![
                json!({"kind":"memberOf","source":"mm","target":"E"}),
                json!({"kind":"memberOf","source":"sm","target":"E"}),
            ],
        );
        let b = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        // Nested enum Mode → handle, dotted Swift type, flattened identifier.
        assert!(b.swift.contains("class Engine_ModeBox { var value: Engine.Mode"), "{}", b.swift);
        assert!(b.cplus.contains("struct Engine_Mode {"), "{}", b.cplus);
        assert!(b.cplus.contains("fn mode(this) -> option::Option[Engine_Mode]"), "{}", b.cplus);
        // Nested struct Span → not a handle; the method that returns it is skipped.
        assert!(!b.cplus.contains("struct Engine_Span {"), "{}", b.cplus);
        assert!(b.cplus.contains("SKIPPED Engine.span(): return `Engine.Span`"), "{}", b.cplus);
    }

    #[test]
    fn bridge_handle_property_getter_gated_by_spec() {
        // A struct's handle-typed property getter copies the value out of `self`,
        // safe only when both owner and value are Copyable. Without a spec it is
        // skipped; declaring both in `copyable` unlocks it.
        let owner = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "O"},
            "pathComponents": ["Owner"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct Owner", "text")],
        });
        let part = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "P"},
            "pathComponents": ["Part"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct Part", "text")],
        });
        let prop = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "pp"},
            "pathComponents": ["Owner", "part"],
            "accessLevel": "public",
            "declarationFragments": [frag("var", "keyword"), frag(" part", "text"), frag(": ", "text"), frag("Part", "typeIdentifier"), frag(" { get }", "text")],
        });
        let g = graph(
            vec![owner, part, prop],
            vec![json!({"kind":"memberOf","source":"pp","target":"O"})],
        );
        // No spec → both Owner and Part are non-class, non-vouched → skipped.
        let b0 = generate_bridge(&g, "Demo", None, &BridgeSpec::default());
        assert!(b0.cplus.contains("SKIPPED Owner.part"), "{}", b0.cplus);
        assert!(!b0.cplus.contains("fn part(this)"), "{}", b0.cplus);
        // Spec vouches for both → handle getter emitted.
        let spec = BridgeSpec {
            copyable: ["Owner".to_string(), "Part".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let b1 = generate_bridge(&g, "Demo", None, &spec);
        assert!(b1.cplus.contains("fn part(this) -> option::Option[Part]"), "{}", b1.cplus);
        assert!(b1.swift.contains("cpRetained(PartBox(_v))"), "{}", b1.swift);
    }

    fn enum_with_case(enum_name: &str, eprecise: &str, case: &str, cprecise: &str) -> Vec<Value> {
        vec![
            json!({
                "kind": {"identifier": "swift.enum"},
                "identifier": {"precise": eprecise},
                "pathComponents": [enum_name],
                "accessLevel": "public",
                "declarationFragments": [frag(&format!("enum {enum_name}"), "text")],
            }),
            json!({
                "kind": {"identifier": "swift.enum.case"},
                "identifier": {"precise": cprecise},
                "pathComponents": [enum_name, case],
                "accessLevel": "public",
                "names": {"title": format!("{enum_name}.{case}")},
                "declarationFragments": [frag(&format!("case {case}"), "text")],
            }),
        ]
    }

    #[test]
    fn bridge_raw_enum_binds_as_i64_with_case_accessors() {
        // A spec-declared raw-value enum crosses as i64: the param init goes
        // through `rawValue`, and each case gets an `Enum_case() -> i64`.
        let engine = json!({
            "kind": {"identifier": "swift.class"},
            "identifier": {"precise": "E"},
            "pathComponents": ["Engine"],
            "accessLevel": "public",
            "declarationFragments": [frag("final class Engine", "text")],
        });
        let set_mode = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "sm"},
            "pathComponents": ["Engine", "setMode(m:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func setMode(m: Mode)", "text")],
            "functionSignature": {"parameters": [typed_param("m", "Mode")], "returns": []},
        });
        let mut syms = enum_with_case("Mode", "M", "a", "ca");
        syms.push(engine);
        syms.push(set_mode);
        let g = graph(
            syms,
            vec![
                json!({"kind":"memberOf","source":"sm","target":"E"}),
                json!({"kind":"memberOf","source":"ca","target":"M"}),
            ],
        );
        let spec = BridgeSpec {
            raw_enums: ["Mode".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let b = generate_bridge(&g, "Demo", None, &spec);
        assert!(b.cplus.contains("extern fn cplus_demo_Engine_setMode(receiver: *u8, m: i64)"), "{}", b.cplus);
        assert!(b.swift.contains("Mode(rawValue: numericCast(m))"), "{}", b.swift);
        assert!(b.cplus.contains("fn setMode(this, m: i64)"), "{}", b.cplus);
        // Case accessor reads the rawValue at runtime.
        assert!(b.swift.contains("Int64(Mode.a.rawValue)"), "{}", b.swift);
        assert!(b.cplus.contains("fn Mode_a() -> i64"), "{}", b.cplus);
        // Not also boxed as a handle.
        assert!(!b.cplus.contains("struct Mode {"), "{}", b.cplus);
    }

    #[test]
    fn bridge_enum_cases_emit_handle_constructors() {
        // A raw-value-less enum stays a handle; `enum_cases` adds a constructor
        // per case so C+ can build a value.
        let g = graph(
            enum_with_case("Kind", "K", "x", "cx"),
            vec![json!({"kind":"memberOf","source":"cx","target":"K"})],
        );
        let spec = BridgeSpec {
            enum_cases: ["Kind".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let b = generate_bridge(&g, "Demo", None, &spec);
        assert!(b.swift.contains("cpRetained(KindBox(Kind.x))"), "{}", b.swift);
        assert!(b.cplus.contains("fn Kind_x() -> option::Option[Kind]"), "{}", b.cplus);
        assert!(b.cplus.contains("struct Kind {"), "{}", b.cplus);
    }

    #[test]
    fn bridge_view_copy_emits_data_extraction() {
        // `view_copy` emits a contiguous bulk-copy accessor per scalar element,
        // gated on the owner being a bound, Copyable handle.
        let nd = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "ND"},
            "pathComponents": ["NDArray"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct NDArray", "text")],
        });
        let g = graph(vec![nd], vec![]);
        let mut view_copy = std::collections::BTreeMap::new();
        view_copy.insert("NDArray".to_string(), vec!["Float".to_string(), "Int32".to_string()]);
        let spec = BridgeSpec {
            copyable: ["NDArray".to_string()].into_iter().collect(),
            view_copy,
            ..Default::default()
        };
        let b = generate_bridge(&g, "Demo", None, &spec);
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_NDArray_copy_f32\")"), "{}", b.swift);
        assert!(b.swift.contains("tmp.view(as: Float.self)"), "{}", b.swift);
        assert!(b.swift.contains("view.withUnsafePointer"), "{}", b.swift);
        assert!(b.cplus.contains("fn copy_f32(this, dest: f32[]) -> i64"), "{}", b.cplus);
        assert!(b.cplus.contains("fn copy_i32(this, dest: i32[]) -> i64"), "{}", b.cplus);
        // Not vouched copyable -> no accessor.
        let b2 = generate_bridge(&g, "Demo", None, &BridgeSpec {
            view_copy: [("NDArray".to_string(), vec!["Float".to_string()])].into_iter().collect(),
            ..Default::default()
        });
        assert!(!b2.cplus.contains("fn copy_f32"), "{}", b2.cplus);
    }

    #[test]
    fn bridge_noncopyable_owner_uses_take() {
        // A `~Copyable` owner gets an optional box and a `take_<member>` that
        // moves the value out once; a borrow-read getter would not compile.
        let iv = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "IV"},
            "pathComponents": ["Holder"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct Holder", "text")],
        });
        let nd = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "ND"},
            "pathComponents": ["NDArray"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct NDArray", "text")],
        });
        let prop = json!({
            "kind": {"identifier": "swift.property"},
            "identifier": {"precise": "pp"},
            "pathComponents": ["Holder", "payload"],
            "accessLevel": "public",
            "declarationFragments": [frag("var", "keyword"), frag(" payload", "text"), frag(": ", "text"), frag("NDArray", "typeIdentifier"), frag(" { get }", "text")],
        });
        let g = graph(vec![iv, nd, prop], vec![json!({"kind":"memberOf","source":"pp","target":"IV"})]);
        let spec = BridgeSpec {
            noncopyable_owners: ["Holder".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let b = generate_bridge(&g, "Demo", None, &spec);
        assert!(b.swift.contains("class HolderBox { var value: Holder?; init(_ v: consuming Holder) { value = consume v }"), "{}", b.swift);
        assert!(b.swift.contains("private func cpTakeOut"), "{}", b.swift);
        assert!(b.swift.contains("@_cdecl(\"cplus_demo_Holder_take_payload\")"), "{}", b.swift);
        assert!(b.swift.contains("cpTakeOut(&_box.value)"), "{}", b.swift);
        assert!(b.cplus.contains("fn take_payload(this) -> option::Option[NDArray]"), "{}", b.cplus);
    }

    #[test]
    fn bridge_generic_instantiation_init_and_self_gating_view() {
        // A generic init with a `some Sequence` element param becomes one
        // binding per spec type; a generic returning a `<…>` view self-gates.
        let nd = json!({
            "kind": {"identifier": "swift.struct"},
            "identifier": {"precise": "ND"},
            "pathComponents": ["NDArray"],
            "accessLevel": "public",
            "declarationFragments": [frag("struct NDArray", "text")],
        });
        let init = json!({
            "kind": {"identifier": "swift.init"},
            "identifier": {"precise": "i"},
            "pathComponents": ["NDArray", "init(scalars:shape:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("init<Scalar>(scalars: some Sequence, shape: [Int]) where Scalar : BitwiseCopyable", "text")],
            "swiftGenerics": {"parameters": [{"name": "Scalar", "index": 0, "depth": 0}]},
            "functionSignature": {
                "parameters": [
                    json!({"name": "scalars", "declarationFragments": [frag("scalars", "identifier"), frag(": ", "text"), frag("some ", "text"), frag("Sequence", "typeIdentifier")]}),
                    json!({"name": "shape", "declarationFragments": [frag("shape", "identifier"), frag(": ", "text"), frag("[", "text"), frag("Int", "typeIdentifier"), frag("]", "text")]})
                ],
                "returns": []
            }
        });
        let viewm = json!({
            "kind": {"identifier": "swift.method"},
            "identifier": {"precise": "v"},
            "pathComponents": ["NDArray", "view(as:)"],
            "accessLevel": "public",
            "declarationFragments": [frag("func view<T>(as: T.Type) -> NDArray.View<T>", "text")],
            "swiftGenerics": {"parameters": [{"name": "T", "index": 0, "depth": 0}]},
            "functionSignature": {
                "parameters": [json!({"name": "as", "declarationFragments": [frag("as", "identifier"), frag(": ", "text"), frag("T", "typeIdentifier"), frag(".Type", "text")]})],
                "returns": [frag("NDArray.View", "typeIdentifier"), frag("<", "text"), frag("T", "typeIdentifier"), frag(">", "text")]
            }
        });
        let g = graph(
            vec![nd, init, viewm],
            vec![
                json!({"kind":"memberOf","source":"i","target":"ND"}),
                json!({"kind":"memberOf","source":"v","target":"ND"}),
            ],
        );
        let spec = BridgeSpec {
            instantiate: [
                ("NDArray.init(scalars:shape:)".to_string(), vec!["Float".to_string()]),
                ("NDArray.view(as:)".to_string(), vec!["Float".to_string()]),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let b = generate_bridge(&g, "Demo", None, &spec);
        // init instantiated, `some Sequence` -> [Float] slice, owner-handle return.
        assert!(b.cplus.contains("fn new_Float(scalars: f32[], shape: i64[]) -> option::Option[NDArray]"), "{}", b.cplus);
        assert!(b.swift.contains("NDArray(scalars: _scalars, shape: _shape)"), "{}", b.swift);
        // view self-gates: its return is still a non-handle `View<Float>`.
        assert!(!b.cplus.contains("fn view_Float"), "{}", b.cplus);
        assert!(b.cplus.contains("SKIPPED NDArray.view(as:) [Float]"), "{}", b.cplus);
    }
}
