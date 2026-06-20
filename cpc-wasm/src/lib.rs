//! WASM front end for the C+ web playground (cplus-lang.dev).
//!
//! [`cplus_compile`] runs the whole front end — lex → parse → attrs → lower →
//! sema → borrowck → monomorphize → LLVM IR codegen — in the browser, with no
//! `clang` and no filesystem. It returns diagnostics and, when the program is
//! error-free, the pre-optimization LLVM IR.
//!
//! It does NOT run the program. `cpc` produces LLVM IR and shells out to
//! `clang` to make a native binary; `clang` can't run in a browser, so
//! *executing* a C+ program from the web needs a server-side runner (or a
//! wasm LLVM toolchain). This crate is the client-side "does it compile, and
//! what does it lower to" half of the playground.

use cplus_core::ast::{ItemKind, Program};
use cplus_core::codegen::{self, BuildMode};
use cplus_core::diagnostics::{self, Diagnostic, LineMap, Severity};
use cplus_core::{attrs, borrowck, lexer, lower, monomorphize, parser, sema, wasm_emit};
use std::collections::BTreeMap;
use std::path::PathBuf;
use wasm_bindgen::prelude::*;

/// Virtual file name the playground compiles under (diagnostics report it).
const FILE: &str = "playground.cplus";

/// Compile a single C+ source string.
///
/// Returns a JSON string:
/// ```json
/// { "ok": bool, "diagnostics": [Diagnostic, ...], "ir": string | null }
/// ```
/// `ok` is true iff there are no error-severity diagnostics. `ir` is the
/// pre-optimization LLVM IR when `ok`, otherwise `null`. Each `Diagnostic`
/// carries `severity`, `code`, `message`, and a `primary` span
/// (`{ file, start: {line, col}, end: {line, col} }`).
#[wasm_bindgen]
pub fn cplus_compile(source: &str) -> String {
    let doc = run(source);
    serde_json::to_string(&doc).unwrap_or_else(|_| {
        r#"{"ok":false,"diagnostics":[{"severity":"error","code":"EWASM","message":"result serialization failed","primary":{"file":"playground.cplus","start":{"line":1,"col":1},"end":{"line":1,"col":1}}}],"ir":null}"#.to_string()
    })
}

/// The C+ toolchain version this playground front end was built from.
#[wasm_bindgen]
pub fn cplus_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Compile a single C+ source string for the *run* path (wasm playground slice,
/// `plans/plan.wasm-playground.md`).
///
/// Same front end as [`cplus_compile`], but the clean-program tail emits
/// WebAssembly (via `cplus_core::wasm_emit`) instead of LLVM IR, and assembles
/// it to a runnable module in-process. Returns JSON:
/// ```json
/// { "ok": bool, "diagnostics": [Diagnostic, ...],
///   "wat": string | null, "wasm": [u8, ...] | null }
/// ```
/// When `ok`, `wat` is the WebAssembly text (for display) and `wasm` is the
/// assembled bytes — instantiate them directly with an `env.println_i32(i32)`
/// import; no `wat2wasm` download needed. The runnable subset is the i32 core
/// (arithmetic, control flow, `#println`); richer programs report `E1900`.
#[wasm_bindgen]
pub fn cplus_run(source: &str) -> String {
    let doc = run_wat(source);
    serde_json::to_string(&doc).unwrap_or_else(|_| {
        r#"{"ok":false,"diagnostics":[{"severity":"error","code":"EWASM","message":"result serialization failed","primary":{"file":"playground.cplus","start":{"line":1,"col":1},"end":{"line":1,"col":1}}}],"wat":null,"wasm":null}"#.to_string()
    })
}

fn run_wat(source: &str) -> serde_json::Value {
    let path = PathBuf::from(FILE);

    if source.contains("\nimport ") || source.starts_with("import ") {
        return finish_wat(
            &[single_diag(
                "the web playground is single-file — `import` (modules, stdlib, vendor packages) isn't available in the browser",
            )],
            None,
            None,
        );
    }

    let toks = match lexer::tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            let lm = LineMap::new(source);
            return finish_wat(&[diagnostics::from_lex(&e, &path, &lm, source)], None, None);
        }
    };
    let mut prog: Program = match parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = LineMap::new(source);
            return finish_wat(&[diagnostics::from_parse(&e, &path, &lm, source)], None, None);
        }
    };

    let mut diags: Vec<Diagnostic> = Vec::new();
    diags.extend(attrs::check(&prog, path.clone(), source));
    if has_error(&diags) {
        return finish_wat(&diags, None, None);
    }
    diags.extend(lower::lower(&mut prog, &path, source));
    if has_error(&diags) {
        return finish_wat(&diags, None, None);
    }
    // Record per-expression types (`check_multi_with_value_types`) so the wasm
    // emitter can resolve literal types without re-inferring.
    let (sema_diags, mono) =
        sema::check_multi_with_value_types(&prog, path.clone(), source, BTreeMap::new());
    diags.extend(sema_diags);
    if has_error(&diags) {
        return finish_wat(&diags, None, None);
    }
    diags.extend(borrowck::check(&prog, &path, source));
    if has_error(&diags) {
        return finish_wat(&diags, None, None);
    }

    // Clean front end. The wasm backend runs the scalar core with no generics,
    // so monomorphize is a structural no-op — emit straight from the checked
    // program. Out-of-subset constructs surface as an `E1900` diagnostic.
    match wasm_emit::generate_wat(&prog, &path, source, &mono.value_types) {
        Ok(wat) => {
            // Assemble in-process so the page gets runnable bytes directly.
            let wasm = wat::parse_str(&wat).ok();
            finish_wat(&diags, Some(wat), wasm)
        }
        Err(d) => {
            diags.push(d);
            finish_wat(&diags, None, None)
        }
    }
}

fn finish_wat(diags: &[Diagnostic], wat: Option<String>, wasm: Option<Vec<u8>>) -> serde_json::Value {
    serde_json::json!({
        "ok": !has_error(diags),
        "diagnostics": diags,
        "wat": wat,
        "wasm": wasm,
    })
}

/// A synthetic playground-level error diagnostic (E0000), anchored at 1:1 —
/// the shared shape both the IR and wasm paths use for single-file refusals.
fn single_diag(message: &str) -> Diagnostic {
    use cplus_core::diagnostics::{DiagCode, Position, SourceSpan};
    Diagnostic {
        severity: Severity::Error,
        code: DiagCode("E0000"),
        message: message.to_string(),
        primary: SourceSpan {
            file: PathBuf::from(FILE),
            start: Position { line: 1, col: 1, byte: 0 },
            end: Position { line: 1, col: 1, byte: 0 },
        },
        labels: Vec::new(),
        notes: Vec::new(),
        suggestions: Vec::new(),
    }
}

fn run(source: &str) -> serde_json::Value {
    let path = PathBuf::from(FILE);

    // The playground is single-file: `import` needs the resolver + filesystem,
    // neither of which exists in the browser. Reject up front with a clear note
    // rather than letting the loader fail obscurely.
    if source.contains("\nimport ") || source.starts_with("import ") {
        return single_error(
            "the web playground is single-file — `import` (modules, stdlib, vendor packages) isn't available in the browser",
        );
    }

    let toks = match lexer::tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            let lm = LineMap::new(source);
            return finish(&[diagnostics::from_lex(&e, &path, &lm, source)], None);
        }
    };
    let mut prog: Program = match parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = LineMap::new(source);
            return finish(&[diagnostics::from_parse(&e, &path, &lm, source)], None);
        }
    };

    // attrs → lower → sema → borrowck each accumulate diagnostics. We stop at
    // the first stage that errors (mirrors `cpc build`): downstream passes
    // assume their predecessor succeeded, so continuing past an error risks
    // cascade noise or a panic on malformed-but-parsed input.
    let mut diags: Vec<Diagnostic> = Vec::new();

    diags.extend(attrs::check(&prog, path.clone(), source));
    if has_error(&diags) {
        return finish(&diags, None);
    }

    diags.extend(lower::lower(&mut prog, &path, source));
    if has_error(&diags) {
        return finish(&diags, None);
    }

    let (sema_diags, mono) =
        sema::check_multi_with_mono(&prog, path.clone(), source, BTreeMap::new());
    diags.extend(sema_diags);
    if has_error(&diags) {
        return finish(&diags, None);
    }

    diags.extend(borrowck::check(&prog, &path, source));
    if has_error(&diags) {
        return finish(&diags, None);
    }

    // Clean front end — monomorphize and emit IR (debug mode, fp-contract on,
    // no debug-info, no sanitizers, not a library).
    let post = run_monomorphize(prog, &mono);
    let ir = codegen::generate_with_mono(&post, BuildMode::Debug, true, None, &[], false, &mono);
    finish(&diags, Some(ir))
}

fn has_error(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| matches!(d.severity, Severity::Error))
}

fn finish(diags: &[Diagnostic], ir: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "ok": !has_error(diags),
        "diagnostics": diags,
        "ir": ir,
    })
}

fn single_error(message: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "diagnostics": [{
            "severity": "error",
            "code": "E0000",
            "message": message,
            "primary": {
                "file": FILE,
                "start": { "line": 1, "col": 1, "byte": 0 },
                "end": { "line": 1, "col": 1, "byte": 0 },
            },
        }],
        "ir": serde_json::Value::Null,
    })
}

/// Build the type-name closure and monomorphize. Mirrors `run_monomorphize`
/// in `cpc/src/main.rs` (the single-file driver path), minus the unused
/// per-file source map.
fn run_monomorphize(program: Program, mono: &sema::MonoInfo) -> Program {
    let mut struct_names: Vec<String> = Vec::new();
    let mut enum_names: Vec<String> = Vec::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Struct(s) if s.generic_params.is_empty() => {
                struct_names.push(s.name.name.clone())
            }
            ItemKind::Enum(e) if e.generic_params.is_empty() => {
                enum_names.push(e.name.name.clone())
            }
            _ => {}
        }
    }
    // Generic instantiations live past the non-generic tables; slot each at its
    // real id so `name_of(Ty::Struct(id))` returns the mangled name.
    for info in mono.struct_instantiations.values() {
        let slot = info.id as usize;
        if struct_names.len() <= slot {
            struct_names.resize(slot + 1, String::from("?"));
        }
        struct_names[slot] = info.mangled_name.clone();
    }
    for info in mono.enum_instantiations.values() {
        let slot = info.id as usize;
        if enum_names.len() <= slot {
            enum_names.resize(slot + 1, String::from("?"));
        }
        enum_names[slot] = info.mangled_name.clone();
    }
    let name_of = move |ty: &sema::Ty| -> String {
        match ty {
            sema::Ty::Struct(id) => struct_names
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_else(|| "?".into()),
            sema::Ty::Enum(id) => enum_names
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_else(|| "?".into()),
            other => other.name().to_string(),
        }
    };
    monomorphize::monomorphize(program, mono, &name_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> serde_json::Value {
        serde_json::from_str(&cplus_compile(src)).expect("output is valid JSON")
    }

    fn codes(v: &serde_json::Value) -> Vec<String> {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn clean_program_emits_ir() {
        let v = compile("fn main() -> i32 {\n    return 0;\n}\n");
        assert_eq!(v["ok"], true, "diagnostics: {:?}", v["diagnostics"]);
        let ir = v["ir"].as_str().expect("ir present when ok");
        assert!(ir.contains("define"), "expected LLVM IR, got: {ir}");
        assert!(ir.contains("main"));
    }

    #[test]
    fn type_error_reports_and_no_ir() {
        // returning a string where i32 is declared must fail sema, not panic.
        let v = compile("fn main() -> i32 {\n    return \"nope\";\n}\n");
        assert_eq!(v["ok"], false);
        assert!(v["ir"].is_null());
        assert!(
            !v["diagnostics"].as_array().unwrap().is_empty(),
            "a type error should surface at least one diagnostic"
        );
    }

    #[test]
    fn parse_error_reports_and_no_ir() {
        let v = compile("fn main( {\n");
        assert_eq!(v["ok"], false);
        assert!(v["ir"].is_null());
        let cs = codes(&v);
        assert!(!cs.is_empty(), "parse error should produce a diagnostic");
    }

    #[test]
    fn lex_error_reports_and_no_ir() {
        // an unterminated string literal is a lexer-level failure.
        let v = compile("fn main() -> i32 {\n    let s = \"unterminated\n}\n");
        assert_eq!(v["ok"], false);
        assert!(v["ir"].is_null());
        assert!(!v["diagnostics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn imports_rejected_in_playground() {
        let v = compile("import \"stdlib/io\" as io;\nfn main() -> i32 {\n    return 0;\n}\n");
        assert_eq!(v["ok"], false);
        assert!(v["ir"].is_null());
        assert_eq!(codes(&v), vec!["E0000"]);
        let msg = v["diagnostics"][0]["message"].as_str().unwrap();
        assert!(msg.contains("single-file"), "got: {msg}");
    }

    #[test]
    fn diagnostic_shape_is_stable() {
        // The website renders one shape for both synthetic and stage diagnostics.
        let v = compile("import \"x\" as x;\nfn main() -> i32 { return 0; }\n");
        let d = &v["diagnostics"][0];
        assert!(d["severity"].is_string());
        assert!(d["code"].is_string());
        assert!(d["message"].is_string());
        assert_eq!(d["primary"]["file"], FILE);
        assert!(d["primary"]["start"]["line"].is_number());
        assert!(d["primary"]["start"]["col"].is_number());
    }

    #[test]
    fn version_matches_crate() {
        assert_eq!(cplus_version(), env!("CARGO_PKG_VERSION"));
    }

    fn run(src: &str) -> serde_json::Value {
        serde_json::from_str(&cplus_run(src)).expect("output is valid JSON")
    }

    #[test]
    fn run_emits_wat_and_assembled_wasm() {
        let v = run("fn main() -> i32 {\n    #println(7);\n    return 0;\n}\n");
        assert_eq!(v["ok"], true, "diagnostics: {:?}", v["diagnostics"]);
        let wat = v["wat"].as_str().expect("wat present when ok");
        assert!(wat.contains("call $println_i32"), "got: {wat}");
        // The bytes are assembled in-process: a valid wasm module starts with
        // the `\0asm` magic (0x00 0x61 0x73 0x6d).
        let wasm = v["wasm"].as_array().expect("wasm bytes present when ok");
        assert!(wasm.len() > 8, "wasm module too small");
        let magic: Vec<u64> = wasm.iter().take(4).map(|b| b.as_u64().unwrap()).collect();
        assert_eq!(magic, vec![0x00, 0x61, 0x73, 0x6d], "missing wasm magic header");
    }

    #[test]
    fn run_reports_out_of_subset_without_wasm() {
        // A struct is valid C+ but out of the current wasm subset (Phase 2):
        // ok=false, E1900, no wat / wasm — and no panic. (Floats now RUN — that
        // path is covered by `run_emits_wat_and_assembled_wasm`-style cases.)
        let v = run("struct P {\n    x: i32,\n}\nfn main() -> i32 {\n    let p: P = { x: 1 };\n    return p.x;\n}\n");
        assert_eq!(v["ok"], false);
        assert!(v["wat"].is_null());
        assert!(v["wasm"].is_null());
        assert_eq!(codes(&v), vec!["E1900"]);
    }

    #[test]
    fn run_rejects_import_single_file() {
        let v = run("import \"stdlib/io\" as io;\nfn main() -> i32 { return 0; }\n");
        assert_eq!(v["ok"], false);
        assert!(v["wasm"].is_null());
        assert_eq!(codes(&v), vec!["E0000"]);
    }
}
