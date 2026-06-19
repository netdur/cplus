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
use cplus_core::{attrs, borrowck, lexer, lower, monomorphize, parser, sema};
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
}
