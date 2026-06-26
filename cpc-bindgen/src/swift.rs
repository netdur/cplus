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
}
