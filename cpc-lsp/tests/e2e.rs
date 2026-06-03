//! E2E tests for `cpc-lsp`. Spawn the binary, drive it with framed
//! JSON-RPC messages over stdin, parse responses from stdout, assert
//! shape. Slice 4E.1 covers initialize + didOpen-triggers-diagnostics
//! + clean shutdown.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CPC_LSP: &str = env!("CARGO_BIN_EXE_cpc-lsp");

fn frame(payload: &serde_json::Value) -> Vec<u8> {
    let body = payload.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn init_request() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "capabilities": {} }
    })
}

fn initialized_notif() -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} })
}

fn shutdown_request(id: i64) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown" })
}

fn exit_notif() -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": "exit" })
}

fn did_open_notif(uri: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "cplus",
                "version": 1,
                "text": text,
            }
        }
    })
}

/// Drive the LSP through a fixed message sequence; capture all framed
/// stdout messages until the process exits. Returns parsed messages
/// (as `serde_json::Value`) in order, plus stderr text for assertion
/// fallback.
struct LspRun {
    messages: Vec<serde_json::Value>,
    stderr: String,
    exit_code: i32,
}

fn drive(msgs: &[serde_json::Value], timeout: Duration) -> LspRun {
    let mut child = Command::new(CPC_LSP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cpc-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");

    // Write the whole sequence in one go, then drop stdin (closes the
    // pipe so the server stops reading once it's done with what we sent).
    for m in msgs {
        stdin.write_all(&frame(m)).expect("write");
    }
    stdin.flush().expect("flush");
    drop(stdin);

    // Read framed messages until EOF.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let start = Instant::now();
    loop {
        match stdout.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            break;
        }
    }
    let mut stderr_buf = String::new();
    let _ = stderr.read_to_string(&mut stderr_buf);
    let exit_code = child.wait().expect("wait").code().unwrap_or(-1);
    LspRun {
        messages: parse_framed(&buf),
        stderr: stderr_buf,
        exit_code,
    }
}

/// Cheap LSP frame parser: walk `Content-Length: N\r\n\r\n<N bytes>`
/// blocks until exhausted. Header-only `Content-Type` etc. are not
/// emitted by `lsp-server` so we don't handle them.
fn parse_framed(buf: &[u8]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let Some(header_end) = find_subseq(&buf[i..], b"\r\n\r\n") else { break; };
        let header = std::str::from_utf8(&buf[i..i + header_end]).unwrap_or("");
        let len: usize = header.lines().find_map(|l| {
            let (k, v) = l.split_once(": ")?;
            (k.eq_ignore_ascii_case("Content-Length")).then_some(v.trim().parse().ok())?
        }).unwrap_or(0);
        let body_start = i + header_end + 4;
        let body_end = body_start + len;
        if body_end > buf.len() { break; }
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[body_start..body_end]) {
            out.push(v);
        }
        i = body_end;
    }
    out
}

fn find_subseq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---- the actual tests ----

#[test]
fn initialize_responds_with_capabilities() {
    let run = drive(
        &[init_request(), initialized_notif(), shutdown_request(2), exit_notif()],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let init_resp = run.messages.iter().find(|m| m["id"] == 1)
        .expect("initialize response present");
    let caps = &init_resp["result"]["capabilities"];
    assert!(caps["textDocumentSync"].is_object(), "expected textDocumentSync, got: {init_resp}");
    let shutdown_resp = run.messages.iter().find(|m| m["id"] == 2)
        .expect("shutdown response present");
    assert!(shutdown_resp["result"].is_null(), "shutdown response: {shutdown_resp}");
}

/// Opening a file with a sema error should publish a diagnostic for it.
#[test]
fn did_open_publishes_diagnostics_on_bad_source() {
    // A `let` that reads an undeclared name → E0306 (undefined name).
    let dir = tempdir();
    let file = dir.join("bug.cplus");
    std::fs::write(&file, "fn main() -> i32 { return zzz; }\n").unwrap();
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, "fn main() -> i32 { return zzz; }\n"),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);

    // Find the publishDiagnostics notification for our file.
    let diags_msg = run.messages.iter().find(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"].as_str() == Some(uri.as_str())
    }).expect("expected publishDiagnostics for our file");

    let diags = diags_msg["params"]["diagnostics"].as_array()
        .expect("diagnostics array");
    assert!(!diags.is_empty(), "expected at least one diagnostic, got: {diags_msg}");
    // The undefined-name error is E0306 (sema's name-resolution code).
    let has_undef = diags.iter().any(|d|
        matches!(d["code"].as_str(), Some(c) if c.starts_with("E03"))
    );
    assert!(has_undef, "expected an E03xx sema diagnostic, got: {diags:?}");

    // Severity should be "error" (1).
    let sev = diags[0]["severity"].as_i64().expect("severity");
    assert_eq!(sev, 1, "expected severity=error, got: {diags:?}");
}

/// Opening a clean file should publish an empty diagnostic list (to
/// clear any prior squiggles).
#[test]
fn did_open_publishes_empty_diagnostics_on_clean_source() {
    let dir = tempdir();
    let file = dir.join("ok.cplus");
    let src = "fn main() -> i32 { return 0; }\n";
    std::fs::write(&file, src).unwrap();
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);

    let diags_msg = run.messages.iter().find(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"].as_str() == Some(uri.as_str())
    }).expect("expected publishDiagnostics for our file");
    let diags = diags_msg["params"]["diagnostics"].as_array().expect("array");
    assert!(diags.is_empty(), "expected no diagnostics for clean source; got: {diags:?}");
}

/// Unadvertised request methods reply with MethodNotFound instead of
/// hanging the editor. `textDocument/completion` isn't served (no
/// completion provider advertised), so it's a stable target for this
/// assertion. (hover/references/documentSymbol ARE served as of v0.0.13.)
#[test]
fn unsupported_request_returns_method_not_found() {
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            serde_json::json!({
                "jsonrpc": "2.0", "id": 99, "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": "file:///nope.cplus" },
                    "position": { "line": 0, "character": 0 }
                }
            }),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0);
    let resp = run.messages.iter().find(|m| m["id"] == 99)
        .expect("completion response present");
    assert!(resp["error"].is_object(), "expected error response, got: {resp}");
    assert_eq!(resp["error"]["code"].as_i64(), Some(-32601), "MethodNotFound");
}

// ---- slice 4E.2: formatting + code-actions ----

fn formatting_request(id: i64, uri: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/formatting",
        "params": {
            "textDocument": { "uri": uri },
            "options": { "tabSize": 4, "insertSpaces": true }
        }
    })
}

fn code_action_request(id: i64, uri: &str, range: serde_json::Value, diags: &[serde_json::Value]) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": { "uri": uri },
            "range": range,
            "context": { "diagnostics": diags }
        }
    })
}

/// `textDocument/formatting` on an ugly buffer returns a single
/// `TextEdit` replacing the whole document with the formatted version.
#[test]
fn formatting_returns_text_edit() {
    let dir = tempdir();
    let file = dir.join("ugly.cplus");
    let ugly = "fn  main()->i32{return 0;}\n";
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, ugly),
            formatting_request(99, &uri),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("formatting response");
    let edits = resp["result"].as_array().expect("edits array");
    assert_eq!(edits.len(), 1, "expected one TextEdit, got: {edits:?}");
    // lsp-types serializes TextEdit fields as camelCase.
    let new_text = edits[0]["newText"].as_str().expect("newText");
    assert_eq!(new_text, "fn main() -> i32 { return 0; }\n");
}

/// `textDocument/formatting` on a buffer that's already canonical
/// returns an empty edit list (no spurious edits).
#[test]
fn formatting_returns_no_edits_for_canonical_source() {
    let dir = tempdir();
    let file = dir.join("clean.cplus");
    let clean = "fn main() -> i32 { return 0; }\n";
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, clean),
            formatting_request(99, &uri),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("formatting response");
    let edits = resp["result"].as_array().expect("edits array");
    assert!(edits.is_empty(), "expected no edits, got: {edits:?}");
}

/// `textDocument/codeAction` over a diagnostic that carries a
/// machine-applicable suggestion returns a Quick Fix code action whose
/// WorkspaceEdit applies the suggestion's `(span, replacement)`.
#[test]
fn code_action_offers_quickfix_for_manifest_edition_error() {
    // Use a manifest with a bad edition — E0406 carries a MaybeIncorrect
    // suggestion replacing `edition = "..."` with `edition = "2026"`.
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"\nedition=\"2018\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let entry = dir.join("src/main.cplus");
    std::fs::write(&entry, "fn main() -> i32 { return 0; }\n").unwrap();
    let entry_uri = format!("file://{}", entry.display());

    // Open the entry. The manifest error fires; its primary span is on
    // the manifest file. The code-action handler only emits quick-fixes
    // for diagnostics whose suggestions land in the *currently-asked*
    // URI. To pick up E0406's suggestion via this path we ask for
    // code-actions on the manifest URI directly.
    let manifest_uri = format!("file://{}/Cplus.toml", dir.display());
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&entry_uri, "fn main() -> i32 { return 0; }\n"),
            // Manifest range is (1,1)-(1,1) per ManifestError::to_diagnostic.
            code_action_request(
                99,
                &manifest_uri,
                serde_json::json!({
                    "start": { "line": 0, "character": 0 },
                    "end":   { "line": 0, "character": 0 }
                }),
                &[],
            ),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("code-action response");
    let actions = resp["result"].as_array().expect("actions array");
    assert!(!actions.is_empty(), "expected at least one Quick Fix, got: {actions:?}");
    // The first action should be a CodeAction kind=quickfix with an edit.
    let a = &actions[0];
    assert_eq!(a["kind"].as_str(), Some("quickfix"));
    assert!(a["edit"]["changes"].is_object(),
        "expected WorkspaceEdit.changes, got: {a:?}");
}

/// Code-action on a range with no overlapping diagnostics → empty list.
#[test]
fn code_action_empty_when_no_diagnostics_overlap() {
    let dir = tempdir();
    let file = dir.join("clean.cplus");
    let uri = format!("file://{}", file.display());
    std::fs::write(&file, "fn main() -> i32 { return 0; }\n").unwrap();

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, "fn main() -> i32 { return 0; }\n"),
            code_action_request(
                99,
                &uri,
                serde_json::json!({
                    "start": { "line": 0, "character": 0 },
                    "end":   { "line": 0, "character": 0 }
                }),
                &[],
            ),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("code-action response");
    let actions = resp["result"].as_array().expect("actions array");
    assert!(actions.is_empty(), "expected no actions on clean buffer, got: {actions:?}");
}

// ---- slice 4E.3: goto-definition ----

fn definition_request(id: i64, uri: &str, line: u32, character: u32) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/definition",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }
    })
}

/// Single-file mode: click on a function call's name, jump to its definition.
#[test]
fn definition_single_file_jumps_to_fn_declaration() {
    let dir = tempdir();
    let file = dir.join("main.cplus");
    // Layout:
    //   line 0: fn helper() -> i32 { return 7; }     <- decl, col 3..9 = "helper"
    //   line 1: fn main() -> i32 { return helper(); }<- call site, col 26..32 = "helper"
    let src = "fn helper() -> i32 { return 7; }\nfn main() -> i32 { return helper(); }\n";
    std::fs::write(&file, src).unwrap();
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            // Cursor at the `helper` in the call site (line 1, char 26).
            definition_request(99, &uri, 1, 26),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("definition response");
    let result = &resp["result"];
    // Either a single Location or an array — both shapes are valid per LSP.
    let target = if result.is_object() {
        result.clone()
    } else if result.is_array() && !result.as_array().unwrap().is_empty() {
        result[0].clone()
    } else {
        panic!("expected at least one location, got: {result}");
    };
    assert_eq!(target["uri"].as_str(), Some(uri.as_str()));
    // The decl `helper` sits on line 0, columns 3..9.
    let r = &target["range"];
    assert_eq!(r["start"]["line"].as_u64(), Some(0));
    assert_eq!(r["start"]["character"].as_u64(), Some(3));
    assert_eq!(r["end"]["line"].as_u64(), Some(0));
    assert_eq!(r["end"]["character"].as_u64(), Some(9));
}

/// Project mode: click on `square` in `math::square(...)` in main.cplus,
/// jump to the `pub fn square` definition in math.cplus.
#[test]
fn definition_project_mode_jumps_across_files() {
    let cpc = env!("CARGO_BIN_EXE_cpc-lsp");
    let _ = cpc; // unused; we don't spawn cpc-lsp from cpc here
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let math = dir.join("src/math.cplus");
    let main_p = dir.join("src/main.cplus");
    // math.cplus: `pub fn square(n: i32) -> i32 { return n * n; }`
    //                     ^^^^^^ name at col 7..13
    std::fs::write(&math, "pub fn square(n: i32) -> i32 { return n * n; }\n").unwrap();
    let main_src = "import \"math.cplus\" as math;\nfn main() -> i32 { return math::square(7); }\n";
    std::fs::write(&main_p, main_src).unwrap();
    let main_uri = format!("file://{}", main_p.display());
    let math_uri = format!("file://{}", math.display());

    // Cursor at `square` in `math::square` on line 1. After "return math::"
    // (line 1, characters 0..32), `square` starts at character 32.
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&main_uri, main_src),
            definition_request(99, &main_uri, 1, 32),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("definition response");
    let result = &resp["result"];
    let target = if result.is_object() {
        result.clone()
    } else if result.is_array() && !result.as_array().unwrap().is_empty() {
        result[0].clone()
    } else {
        panic!("expected at least one location for cross-file jump, got: {result}");
    };
    // macOS canonicalizes `/var/folders` to `/private/var/folders`; the
    // resolver uses the canonical form. Compare by the trailing
    // `math.cplus` instead of the absolute path.
    let target_uri = target["uri"].as_str().expect("uri");
    assert!(target_uri.ends_with("math.cplus"),
        "expected jump to a `math.cplus`, got: {target_uri} (compare to {math_uri})");
    let r = &target["range"];
    assert_eq!(r["start"]["line"].as_u64(), Some(0));
    assert_eq!(r["start"]["character"].as_u64(), Some(7), "expected col 7 for `square`");
}

/// Clicking on whitespace / a keyword returns no definition (null /
/// empty array).
#[test]
fn definition_on_keyword_returns_empty() {
    let dir = tempdir();
    let file = dir.join("main.cplus");
    let src = "fn main() -> i32 { return 0; }\n";
    std::fs::write(&file, src).unwrap();
    let uri = format!("file://{}", file.display());

    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            // Cursor on the `fn` keyword.
            definition_request(99, &uri, 0, 0),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("response");
    // Either an empty array or null — both indicate "no definition."
    let result = &resp["result"];
    let empty = result.is_null()
        || (result.is_array() && result.as_array().unwrap().is_empty());
    assert!(empty, "expected empty / null result, got: {result}");
}

// ---- v0.0.13: graph fold-in (references / hover / documentSymbol) ----

fn references_request(id: i64, uri: &str, line: u32, character: u32, incl_decl: bool) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "textDocument/references",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": incl_decl }
        }
    })
}

fn hover_request(id: i64, uri: &str, line: u32, character: u32) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }
    })
}

fn document_symbol_request(id: i64, uri: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "textDocument/documentSymbol",
        "params": { "textDocument": { "uri": uri } }
    })
}

/// A minimal project: `Cplus.toml` + `src/main.cplus` (default bin entry).
/// Returns (dir, entry path, entry `file://` uri).
fn mini_project(main_src: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let dir = tempdir();
    std::fs::write(dir.join("Cplus.toml"), "[package]\nname=\"x\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let entry = dir.join("src/main.cplus");
    std::fs::write(&entry, main_src).unwrap();
    let uri = format!("file://{}", entry.display());
    (dir, entry, uri)
}

/// `textDocument/references` returns the resolved use sites of the symbol
/// under the cursor, from the graph's reference index. `Point` is used in two
/// type positions (a return type and a struct literal).
#[test]
fn references_finds_type_use_sites() {
    let src = "struct Point { x: i32 }\n\
               fn mk() -> Point { return Point { x: 1 }; }\n\
               fn main() -> i32 { return mk().x; }\n";
    let (_dir, _entry, uri) = mini_project(src);
    // Cursor on the `Point` declaration name (line 0): `struct Point` → P at col 7.
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            references_request(99, &uri, 0, 7, false),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("references response");
    let locs = resp["result"].as_array().expect("locations array");
    assert_eq!(locs.len(), 2, "expected two type-use sites of `Point`, got: {locs:?}");
    for l in locs {
        assert!(l["uri"].as_str().unwrap().ends_with("main.cplus"));
    }
}

/// `textDocument/hover` reports the type of a parameter under the cursor,
/// from the graph's type-at index.
#[test]
fn hover_shows_parameter_type() {
    // `n: i32` param; hover on its use inside the body.
    let src = "fn sq(n: i32) -> i32 { return n *% n; }\nfn main() -> i32 { return sq(6); }\n";
    let (_dir, _entry, uri) = mini_project(src);
    // `fn sq(n: i32) -> i32 { return ` is 30 chars (0-based), so the first `n`
    // of `return n *% n` sits at character 30.
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            hover_request(99, &uri, 0, 30),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("hover response");
    let value = resp["result"]["contents"]["value"].as_str().unwrap_or("");
    assert!(value.contains("i32"), "hover should mention the type i32, got: {value:?}");
}

/// `textDocument/documentSymbol` returns the file's top-level outline.
#[test]
fn document_symbol_lists_top_level_items() {
    let src = "struct Point { x: i32, y: i32 }\n\
               fn helper() -> i32 { return 7; }\n\
               fn main() -> i32 { return helper(); }\n";
    let (_dir, _entry, uri) = mini_project(src);
    let run = drive(
        &[
            init_request(),
            initialized_notif(),
            did_open_notif(&uri, src),
            document_symbol_request(99, &uri),
            shutdown_request(2),
            exit_notif(),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(run.exit_code, 0, "non-zero exit; stderr:\n{}", run.stderr);
    let resp = run.messages.iter().find(|m| m["id"] == 99).expect("documentSymbol response");
    let syms = resp["result"].as_array().expect("symbols array");
    let names: Vec<&str> = syms.iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(names.contains(&"Point"), "expected `Point` in outline, got: {names:?}");
    assert!(names.contains(&"helper"), "expected `helper` in outline, got: {names:?}");
    assert!(names.contains(&"main"), "expected `main` in outline, got: {names:?}");
}

// ---- helpers ----

/// v0.0.3 Phase 2 (CWE-377 hardening): use `tempfile::TempDir` for secure
/// random paths instead of the predictable PID-based shape. See the
/// matching helper in `cpc/tests/e2e.rs` for the leak rationale.
fn tempdir() -> std::path::PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("cpc-lsp-test-")
        .tempdir()
        .expect("tempdir creation");
    let leaked: &'static tempfile::TempDir = Box::leak(Box::new(dir));
    leaked.path().to_path_buf()
}
