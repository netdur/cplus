//! C+ Language Server — slices 4E.1 (skeleton + live diagnostics) and
//! 4E.2 (formatting + code-action quick-fixes).
//!
//! Transport: stdio JSON-RPC via `lsp-server`. Synchronous dispatch loop;
//! no async runtime. Every language operation routes through
//! `cplus-core` — the same library `cpc build` / `cpc fmt` use. See
//! `docs/design/phase4-lsp.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cplus_core::ast::ItemKind;
use cplus_core::{attrs, borrowck, fmt as cpfmt, graph, lexer, lower, manifest, parser, resolver, sema};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::Notification as _;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, HoverProviderCapability, Location, MarkupContent, MarkupKind, NumberOrString,
    OneOf, Position, PublishDiagnosticsParams, Range, ReferenceParams, SaveOptions,
    ServerCapabilities, SymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkspaceEdit,
};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    // Slice 4E.1 only accepts `--log PATH` (writes a per-server trace to
    // a file in addition to stderr). No other flags.
    let mut log_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--log" => log_path = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                eprintln!("cpc-lsp — C+ Language Server (stdio JSON-RPC)");
                eprintln!("flags: --log PATH    write trace events to PATH in addition to stderr");
                return Ok(());
            }
            other => {
                eprintln!("cpc-lsp: unknown argument: {other:?}");
                std::process::exit(2);
            }
        }
    }
    let _ = log_path; // logging-to-file is a 4E.2 polish item; honor parsing today.

    eprintln!("cpc-lsp: starting (stdio)");
    let (connection, io_threads) = Connection::stdio();

    let server_caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            // We want save notifications without re-sending the whole text
            // — we already have it from didChange.
            save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                include_text: Some(false),
            })),
            will_save: None,
            will_save_wait_until: None,
        })),
        // Slice 4E.3 — goto-definition. v0.0.13: served from the code graph
        // (resolved) in project mode, with the name-based single-file fallback.
        definition_provider: Some(OneOf::Left(true)),
        // v0.0.13 (graph fold-in): references, hover (type-at), and the
        // document outline all read the same `CodeGraph` index that
        // `cpc query` / `cpc mcp` use — editor and agent share one graph.
        references_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        // Slice 4E.2 — formatting wraps `fmt::format_source`; code-actions
        // lift `MachineApplicable` / `MaybeIncorrect` suggestions from
        // already-published diagnostics.
        document_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        // Diagnostics — slice 4E.1's load-bearing feature. We push on
        // didOpen / didSave; pull diagnostics (LSP 3.17) not yet advertised.
        ..Default::default()
    };

    let server_caps_json = serde_json::to_value(&server_caps)?;
    let init_params: Value = match connection.initialize(server_caps_json) {
        Ok(v) => v,
        Err(e) => {
            // Initialization failed before the handshake completed — most
            // likely the client closed before we got a chance to reply.
            eprintln!("cpc-lsp: initialize failed: {e}");
            return Err(Box::new(e));
        }
    };
    eprintln!("cpc-lsp: initialized");
    let _ = init_params;

    let mut state = ServerState::default();
    main_loop(connection, &mut state)?;
    // Note: `connection` is moved into `main_loop` and dropped on return.
    // That drops its inbound/outbound channels, which lets the io
    // threads exit; otherwise `join()` deadlocks (writer thread waits
    // forever for more outgoing messages).
    io_threads.join()?;
    eprintln!("cpc-lsp: shutting down cleanly");
    Ok(())
}

// ---------------- state ----------------

#[derive(Default)]
struct ServerState {
    /// Per-document buffer. BTreeMap for deterministic iteration order
    /// (§5.3).
    docs: BTreeMap<Url, DocSnapshot>,
    /// Slice 4E.2: cache the most recently computed cplus-core diagnostics
    /// per file URI. The pushed LSP shape strips suggestion data; we need
    /// the originals here so the code-action handler can lift their
    /// `(span, replacement)` pairs into editor quick-fixes.
    last_diagnostics: BTreeMap<Url, Vec<cplus_core::diagnostics::Diagnostic>>,
}

#[derive(Debug, Clone)]
struct DocSnapshot {
    /// Last-known version from `didOpen` / `didChange`.
    version: i32,
    text: String,
}

// ---------------- main loop ----------------

fn main_loop(
    conn: Connection,
    state: &mut ServerState,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    // Iterate by reference; the `Connection` is moved in so it drops on
    // return (and the channel sender drops with it, unblocking the io
    // threads' write loop).
    while let Ok(msg) = conn.receiver.recv() {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(&conn, state, req);
            }
            Message::Notification(not) => {
                handle_notification(&conn, state, not);
            }
            Message::Response(_) => {
                // Server-initiated requests aren't issued in 4E.1; drop.
            }
        }
    }
    Ok(())
}

// ---------------- request dispatch ----------------

fn handle_request(conn: &Connection, state: &mut ServerState, req: Request) {
    let method = req.method.clone();
    let id = req.id.clone();
    match method.as_str() {
        "textDocument/formatting" => {
            let resp = match serde_json::from_value::<DocumentFormattingParams>(req.params) {
                Ok(p) => handle_formatting(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        "textDocument/codeAction" => {
            let resp = match serde_json::from_value::<CodeActionParams>(req.params) {
                Ok(p) => handle_code_action(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        "textDocument/definition" => {
            let resp = match serde_json::from_value::<GotoDefinitionParams>(req.params) {
                Ok(p) => handle_definition(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        "textDocument/references" => {
            let resp = match serde_json::from_value::<ReferenceParams>(req.params) {
                Ok(p) => handle_references(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        "textDocument/hover" => {
            let resp = match serde_json::from_value::<HoverParams>(req.params) {
                Ok(p) => handle_hover(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        "textDocument/documentSymbol" => {
            let resp = match serde_json::from_value::<DocumentSymbolParams>(req.params) {
                Ok(p) => handle_document_symbol(state, &p),
                Err(e) => bad_params(id.clone(), &method, &e.to_string()),
            };
            let _ = conn.sender.send(Message::Response(resp_with_id(id, resp)));
        }
        _ => {
            // Unknown / unadvertised method. Reply with MethodNotFound so
            // the client doesn't hang waiting for a response.
            let resp = Response {
                id,
                result: None,
                error: Some(lsp_server::ResponseError {
                    code: lsp_server::ErrorCode::MethodNotFound as i32,
                    message: format!("unsupported method: {method}"),
                    data: None,
                }),
            };
            let _ = conn.sender.send(Message::Response(resp));
        }
    }
}

/// Wrap a handler's result `serde_json::Value` (or an error response) in
/// a fresh `Response` carrying the original request id.
fn resp_with_id(id: lsp_server::RequestId, body: HandlerResult) -> Response {
    match body {
        HandlerResult::Ok(v) => Response { id, result: Some(v), error: None },
        HandlerResult::Err(err) => Response { id, result: None, error: Some(err) },
    }
}

enum HandlerResult {
    Ok(Value),
    Err(lsp_server::ResponseError),
}

fn bad_params(_id: lsp_server::RequestId, method: &str, msg: &str) -> HandlerResult {
    HandlerResult::Err(lsp_server::ResponseError {
        code: lsp_server::ErrorCode::InvalidParams as i32,
        message: format!("{method}: bad params: {msg}"),
        data: None,
    })
}

// ---------------- formatting (slice 4E.2) ----------------

fn handle_formatting(state: &ServerState, params: &DocumentFormattingParams) -> HandlerResult {
    let uri = &params.text_document.uri;
    let Some(snap) = state.docs.get(uri) else {
        // No buffer for this URI — return an empty edit list rather than
        // an error; clients sometimes ask for formatting on close-adjacent
        // events and racing with `didClose` shouldn't surface as an error.
        return HandlerResult::Ok(serde_json::Value::Array(vec![]));
    };
    let edits = match cpfmt::format_source(&snap.text) {
        Ok(formatted) if formatted != snap.text => {
            // Whole-document replacement is cheaper for the client to apply
            // than computing a minimal diff and is well-supported by every
            // LSP client we care about. Slice 4E.2.
            vec![TextEdit {
                range: whole_document_range(&snap.text),
                new_text: formatted,
            }]
        }
        Ok(_) => vec![],
        Err(_) => {
            // Lex error in the buffer. The diagnostic pipeline will have
            // already reported it; don't surface a second error here —
            // return no edits.
            vec![]
        }
    };
    HandlerResult::Ok(serde_json::to_value(edits).expect("Vec<TextEdit> serializes"))
}

fn whole_document_range(text: &str) -> Range {
    // LSP positions are 0-based line, 0-based character. The range that
    // covers the whole buffer is (0,0) → end of last line.
    let mut line: u32 = 0;
    let mut last_line_len: u32 = 0;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            last_line_len = 0;
        } else {
            last_line_len += 1;
        }
    }
    Range {
        start: Position { line: 0, character: 0 },
        end: Position { line, character: last_line_len },
    }
}

// ---------------- code actions (slice 4E.2) ----------------

fn handle_code_action(state: &ServerState, params: &CodeActionParams) -> HandlerResult {
    let uri = &params.text_document.uri;
    let asked_range = params.range;

    let Some(diags) = state.last_diagnostics.get(uri) else {
        return HandlerResult::Ok(serde_json::Value::Array(vec![]));
    };

    // Walk every cached diagnostic for this URI; if its primary span
    // overlaps the requested range AND it carries at least one
    // suggestion, emit one code-action per suggestion.
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for d in diags {
        if d.suggestions.is_empty() { continue; }
        let lsp_d = map_diagnostic(d);
        if !ranges_overlap(asked_range, lsp_d.range) { continue; }
        for sugg in &d.suggestions {
            // The suggestion's span tells us which file to edit. For
            // 4E.2 we only emit edits in the requesting URI's file —
            // a cross-file fix is more involved (we'd need a
            // WorkspaceEdit that touches another file's buffer).
            let Ok(target_uri) = Url::from_file_path(&sugg.span.file) else { continue; };
            if target_uri != *uri { continue; }
            let edit_range = source_span_to_range(&sugg.span);
            let edit = TextEdit {
                range: edit_range,
                new_text: sugg.replacement.clone(),
            };
            let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
                std::collections::HashMap::new();
            changes.insert(target_uri, vec![edit]);
            let workspace_edit = WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            };
            let is_machine_applicable = matches!(
                sugg.applicability,
                cplus_core::diagnostics::Applicability::MachineApplicable
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: sugg.description.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![lsp_d.clone()]),
                edit: Some(workspace_edit),
                command: None,
                is_preferred: Some(is_machine_applicable),
                disabled: None,
                data: None,
            }));
        }
    }
    let resp: CodeActionResponse = actions;
    HandlerResult::Ok(serde_json::to_value(resp).expect("CodeActionResponse serializes"))
}

fn ranges_overlap(a: Range, b: Range) -> bool {
    // Inclusive overlap. The client sends a range; we accept any
    // diagnostic whose range touches it. Empty ranges (zero-width) count
    // when they sit inside or on the boundary of the other range.
    let a_after_b_end = a.start.line > b.end.line
        || (a.start.line == b.end.line && a.start.character > b.end.character);
    let b_after_a_end = b.start.line > a.end.line
        || (b.start.line == a.end.line && b.start.character > a.end.character);
    !(a_after_b_end || b_after_a_end)
}

fn source_span_to_range(span: &cplus_core::diagnostics::SourceSpan) -> Range {
    Range {
        start: Position {
            line: span.start.line.saturating_sub(1),
            character: span.start.col.saturating_sub(1),
        },
        end: Position {
            line: span.end.line.saturating_sub(1),
            character: span.end.col.saturating_sub(1),
        },
    }
}

// ---------------- goto-definition (slice 4E.3) ----------------

fn handle_definition(state: &ServerState, params: &GotoDefinitionParams) -> HandlerResult {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(snap) = state.docs.get(uri) else {
        return HandlerResult::Ok(serde_json::Value::Null);
    };
    let Ok(open_path) = uri.to_file_path() else {
        return HandlerResult::Ok(serde_json::Value::Null);
    };
    // Re-lex the buffer at the cursor to find the identifier under it.
    // Per the design note (§7): we look up just the bare identifier,
    // accepting that clicking on `prefix` of `prefix::Item` won't jump.
    let Some(ident_name) = identifier_at_position(&snap.text, pos) else {
        return HandlerResult::Ok(serde_json::Value::Null);
    };

    // v0.0.13 (graph fold-in): in project mode, resolve via the code graph —
    // the same resolved index `cpc query` uses. Definition nodes carry a
    // precise `file:line:col`. Single-file mode keeps the name-based fallback.
    let locations = match build_project_graph(state, &open_path) {
        Some((g, _loaded)) => g
            .def(&ident_name)
            .iter()
            .filter_map(|n| n.location.as_ref().map(|loc| (loc, n.name.as_str())))
            .filter_map(|(loc, name)| graph_loc_to_lsp(loc, name.chars().count() as u32))
            .collect(),
        None => find_decls_in_single_file(&ident_name, &open_path, &snap.text),
    };

    let resp = if locations.is_empty() {
        // Returning `Null` (rather than an empty array) is the LSP-idiomatic
        // "no definition found" — some clients treat the two differently.
        GotoDefinitionResponse::Array(Vec::new())
    } else if locations.len() == 1 {
        GotoDefinitionResponse::Scalar(locations.into_iter().next().unwrap())
    } else {
        GotoDefinitionResponse::Array(locations)
    };
    HandlerResult::Ok(serde_json::to_value(resp).expect("GotoDefinitionResponse serializes"))
}

// ---------------- references / hover / outline (v0.0.13 graph fold-in) ----------------

/// `textDocument/references`: the resolved use sites of the symbol under the
/// cursor, from the code graph's reference index. Honors
/// `context.include_declaration`. Project mode only (the graph needs a
/// resolved project); single-file returns no results.
fn handle_references(state: &ServerState, params: &ReferenceParams) -> HandlerResult {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let null = || HandlerResult::Ok(serde_json::Value::Null);
    let Some(snap) = state.docs.get(uri) else { return null(); };
    let Ok(open_path) = uri.to_file_path() else { return null(); };
    let Some(ident) = identifier_at_position(&snap.text, pos) else { return null(); };
    let Some((g, _loaded)) = build_project_graph(state, &open_path) else { return null(); };

    let mut locs: Vec<Location> = Vec::new();
    if params.context.include_declaration {
        for n in g.def(&ident) {
            if let Some(loc) = &n.location {
                if let Some(l) = graph_loc_to_lsp(loc, n.name.chars().count() as u32) {
                    locs.push(l);
                }
            }
        }
    }
    let ident_len = ident.chars().count() as u32;
    for r in g.refs(&ident) {
        if let Some(l) = graph_loc_to_lsp(&r.location, ident_len) {
            locs.push(l);
        }
    }
    HandlerResult::Ok(serde_json::to_value(locs).expect("Vec<Location> serializes"))
}

/// `textDocument/hover`: the locally-known type at the cursor, from the
/// graph's `type-at` index (parameters, fields, typed locals, and their
/// identifier uses). Project mode only.
fn handle_hover(state: &ServerState, params: &HoverParams) -> HandlerResult {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let null = || HandlerResult::Ok(serde_json::Value::Null);
    let Some(_snap) = state.docs.get(uri) else { return null(); };
    let Ok(open_path) = uri.to_file_path() else { return null(); };
    let Some((g, loaded)) = build_project_graph(state, &open_path) else { return null(); };
    let Some(fid) = fid_for_path(&loaded, &open_path) else { return null(); };
    let Some((_, src)) = loaded.files.get(&fid) else { return null(); };
    // The graph's spans are over the on-disk source; map the cursor (1-based)
    // through that source so the byte aligns with the index.
    let Some(byte) = graph::byte_offset(src, pos.line + 1, pos.character + 1) else {
        return null();
    };
    let Some(spot) = g.type_at(&fid, byte) else { return null(); };
    let value = format!("```cplus\n{}: {}\n```", spot.what, spot.ty);
    // Highlight the spot's own span (ASCII identifiers ⇒ bytes == chars).
    let width = spot.span.end.saturating_sub(spot.span.start);
    let start = Position {
        line: spot.location.line.saturating_sub(1),
        character: spot.location.col.saturating_sub(1),
    };
    let hover = Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(Range {
            start,
            end: Position { line: start.line, character: start.character + width },
        }),
    };
    HandlerResult::Ok(serde_json::to_value(hover).expect("Hover serializes"))
}

/// `textDocument/documentSymbol`: the file's outline from the graph's
/// `symbols` query (top-level items defined in this file). Project mode only.
fn handle_document_symbol(state: &ServerState, params: &DocumentSymbolParams) -> HandlerResult {
    let uri = &params.text_document.uri;
    let null = || HandlerResult::Ok(serde_json::Value::Null);
    let Ok(open_path) = uri.to_file_path() else { return null(); };
    let Some((g, loaded)) = build_project_graph(state, &open_path) else { return null(); };
    let Some(fid) = fid_for_path(&loaded, &open_path) else { return null(); };

    #[allow(deprecated)] // `DocumentSymbol.deprecated` field — required by the struct.
    let syms: Vec<DocumentSymbol> = g
        .symbols(Some(&fid))
        .iter()
        .filter_map(|n| {
            let loc = n.location.as_ref()?;
            let range = Range {
                start: Position {
                    line: loc.line.saturating_sub(1),
                    character: loc.col.saturating_sub(1),
                },
                end: Position {
                    line: loc.line.saturating_sub(1),
                    character: loc.col.saturating_sub(1) + n.name.chars().count() as u32,
                },
            };
            Some(DocumentSymbol {
                name: n.name.clone(),
                detail: n.signature.clone(),
                kind: node_kind_to_symbol_kind(n.kind),
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            })
        })
        .collect();
    HandlerResult::Ok(
        serde_json::to_value(DocumentSymbolResponse::Nested(syms))
            .expect("DocumentSymbolResponse serializes"),
    )
}

/// Load + resolve the enclosing project and build the code graph. `None` in
/// single-file mode (no reachable `Cplus.toml` with a real bin entry) or when
/// the project fails to resolve. v0.0.14: open editor buffers are overlaid onto
/// their on-disk files (keyed by canonical path), so hover/type-at/value-refs/
/// goto-def reflect unsaved edits.
fn build_project_graph(
    state: &ServerState,
    open_path: &Path,
) -> Option<(graph::CodeGraph, resolver::LoadedProject)> {
    let overlays: BTreeMap<PathBuf, String> = state
        .docs
        .iter()
        .filter_map(|(uri, snap)| {
            let p = uri.to_file_path().ok()?;
            let canon = std::fs::canonicalize(&p).unwrap_or(p);
            Some((canon, snap.text.clone()))
        })
        .collect();
    match find_manifest(open_path) {
        ManifestProbe::Loaded { manifest, .. } if manifest.bins[0].path.is_file() => {
            let loaded =
                resolver::load_project_with_overlays(&manifest.bins[0].path, &manifest.root, overlays)
                    .ok()?;
            let g = graph::CodeGraph::build(&loaded);
            Some((g, loaded))
        }
        _ => None,
    }
}

/// Find the resolver file id whose path is the open document.
fn fid_for_path(loaded: &resolver::LoadedProject, open_path: &Path) -> Option<String> {
    let target = std::fs::canonicalize(open_path).ok();
    loaded
        .files
        .iter()
        .find(|(_, (p, _))| std::fs::canonicalize(p).ok() == target)
        .map(|(fid, _)| fid.clone())
}

/// Convert a graph `Location` (1-based line/col, path string) to an LSP
/// `Location`, highlighting `name_len` characters from the start.
fn graph_loc_to_lsp(loc: &graph::Location, name_len: u32) -> Option<Location> {
    let uri = Url::from_file_path(Path::new(&loc.file)).ok()?;
    Some(Location {
        uri,
        range: one_line_range(loc.line, loc.col, name_len),
    })
}

/// A one-line LSP `Range` from a 1-based (line, col) start spanning `width`
/// characters. Names don't cross line boundaries, so a single line is fine.
fn one_line_range(line: u32, col: u32, width: u32) -> Range {
    let start = Position {
        line: line.saturating_sub(1),
        character: col.saturating_sub(1),
    };
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + width,
        },
    }
}

fn node_kind_to_symbol_kind(k: graph::NodeKind) -> SymbolKind {
    match k {
        graph::NodeKind::Module => SymbolKind::MODULE,
        graph::NodeKind::Function | graph::NodeKind::ExternFn => SymbolKind::FUNCTION,
        graph::NodeKind::Method => SymbolKind::METHOD,
        graph::NodeKind::Struct => SymbolKind::STRUCT,
        graph::NodeKind::Enum => SymbolKind::ENUM,
        graph::NodeKind::Variant => SymbolKind::ENUM_MEMBER,
        graph::NodeKind::Field => SymbolKind::FIELD,
        graph::NodeKind::Const => SymbolKind::CONSTANT,
        graph::NodeKind::Static => SymbolKind::VARIABLE,
        graph::NodeKind::TypeAlias => SymbolKind::CLASS,
        graph::NodeKind::Interface => SymbolKind::INTERFACE,
    }
}

/// Convert an LSP `Position` (0-based line, 0-based character) to a byte
/// offset in `text`. Uses chars-per-line semantics — close enough for
/// editing one-byte-per-char source (which `.cplus` files are, since the
/// lexer is byte-oriented and identifiers are ASCII).
fn position_to_byte(text: &str, pos: Position) -> Option<usize> {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for (i, ch) in text.char_indices() {
        if line == pos.line && col == pos.character {
            return Some(i);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    if line == pos.line && col == pos.character {
        Some(text.len())
    } else {
        None
    }
}

/// Tokenize `text` and find the `Ident` token whose span contains the
/// byte offset for `pos`. Returns the ident's name string. `None` when
/// the cursor sits in whitespace, a comment, a keyword, or a non-ident
/// token.
fn identifier_at_position(text: &str, pos: Position) -> Option<String> {
    let byte = position_to_byte(text, pos)? as u32;
    let toks = lexer::tokenize(text).ok()?;
    for t in toks {
        if t.span.start <= byte && byte <= t.span.end {
            if let lexer::TokenKind::Ident(name) = t.kind {
                return Some(name);
            }
        }
    }
    None
}

/// Scan the resolver's merged program for items whose source-level name
/// matches `target`. Item names in the merged program are qualified
/// (`src.math.square`); we match either the full qualified name or the
/// last `.`-segment so a click on `square` finds `src.math.square`.
fn find_decls_in_project(
    target: &str,
    program: &cplus_core::ast::Program,
    files: &std::collections::BTreeMap<String, (PathBuf, String)>,
) -> Vec<Location> {
    let mut out = Vec::new();
    for item in &program.items {
        let Some((name, name_span)) = item_name_and_span(item) else { continue; };
        if !name_matches(name, target) { continue; }
        let Some(file_id) = item.origin_file.as_ref() else { continue; };
        let Some((path, src)) = files.get(file_id) else { continue; };
        let lm = cplus_core::diagnostics::LineMap::new(src);
        let source_span = lm.span(path, name_span, src);
        let Ok(uri) = Url::from_file_path(path) else { continue; };
        out.push(Location { uri, range: source_span_to_range(&source_span) });
    }
    out
}

/// Single-file fallback: parse the open buffer directly and find
/// matching top-level items.
fn find_decls_in_single_file(
    target: &str,
    open_path: &Path,
    text: &str,
) -> Vec<Location> {
    let Ok(toks) = lexer::tokenize(text) else { return Vec::new(); };
    let Ok(prog) = parser::parse(toks) else { return Vec::new(); };
    let lm = cplus_core::diagnostics::LineMap::new(text);
    let Ok(uri) = Url::from_file_path(open_path) else { return Vec::new(); };
    let mut out = Vec::new();
    for item in &prog.items {
        let Some((name, name_span)) = item_name_and_span(item) else { continue; };
        if !name_matches(name, target) { continue; }
        let source_span = lm.span(&open_path.to_path_buf(), name_span, text);
        out.push(Location {
            uri: uri.clone(),
            range: source_span_to_range(&source_span),
        });
    }
    out
}

fn item_name_and_span(item: &cplus_core::ast::Item) -> Option<(&str, cplus_core::lexer::Span)> {
    match &item.kind {
        ItemKind::Function(f) => Some((f.name.name.as_str(), f.name.span)),
        ItemKind::Struct(s) => Some((s.name.name.as_str(), s.name.span)),
        ItemKind::Enum(e) => Some((e.name.name.as_str(), e.name.span)),
        // Slice 7GEN.3: interface declarations are named items —
        // goto-definition on an interface name jumps here.
        ItemKind::Interface(i) => Some((i.name.name.as_str(), i.name.span)),
        // `impl` blocks don't define a name themselves — methods inside
        // are accessed via the type. Skip; 4E.3 doesn't index methods.
        ItemKind::Impl(_) => None,
        ItemKind::TypeAlias(a) => Some((a.name.name.as_str(), a.name.span)),
        // v0.0.9 Phase 4: const/static items expose a name + span so
        // goto-definition jumps to the declaration.
        ItemKind::Const(c) => Some((c.name.name.as_str(), c.name.span)),
        ItemKind::Static(s) => Some((s.name.name.as_str(), s.name.span)),
        // v0.0.15: module-scope `#asm("...")` declares no symbol name.
        ItemKind::ModuleAsm(_) => None,
    }
}

/// `target` matches `qualified` iff either the whole qualified name is
/// equal, OR the last `.`-segment is equal. So a click on `square`
/// matches both single-file `square` and resolver-qualified
/// `src.math.square`. The entry binary's `fn main` stays bare-`main`,
/// so single-file logic catches it.
fn name_matches(qualified: &str, target: &str) -> bool {
    qualified == target || qualified.rsplit('.').next() == Some(target)
}

// ---------------- notification dispatch ----------------

fn handle_notification(conn: &Connection, state: &mut ServerState, not: Notification) {
    let method = not.method.clone();
    match method.as_str() {
        m if m == lsp_types::notification::DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = match cast_notif(&not) {
                Ok(p) => p,
                Err(e) => { eprintln!("cpc-lsp: bad didOpen params: {e}"); return; }
            };
            let uri = params.text_document.uri.clone();
            state.docs.insert(
                uri.clone(),
                DocSnapshot {
                    version: params.text_document.version,
                    text: params.text_document.text.clone(),
                },
            );
            publish_diagnostics_for(conn, state, &uri);
        }
        m if m == lsp_types::notification::DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = match cast_notif(&not) {
                Ok(p) => p,
                Err(e) => { eprintln!("cpc-lsp: bad didChange params: {e}"); return; }
            };
            // Full sync (we advertised TextDocumentSyncKind::FULL), so
            // the single content change carries the new buffer.
            let Some(snap) = state.docs.get_mut(&params.text_document.uri) else { return; };
            snap.version = params.text_document.version;
            if let Some(change) = params.content_changes.into_iter().next() {
                snap.text = change.text;
            }
            // No diagnostic recompute on per-keystroke changes — see
            // design note §5.1 (push on save only in 4E.1).
        }
        m if m == lsp_types::notification::DidSaveTextDocument::METHOD => {
            let params: DidSaveTextDocumentParams = match cast_notif(&not) {
                Ok(p) => p,
                Err(e) => { eprintln!("cpc-lsp: bad didSave params: {e}"); return; }
            };
            publish_diagnostics_for(conn, state, &params.text_document.uri);
        }
        m if m == lsp_types::notification::DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = match cast_notif(&not) {
                Ok(p) => p,
                Err(e) => { eprintln!("cpc-lsp: bad didClose params: {e}"); return; }
            };
            let uri = params.text_document.uri;
            state.docs.remove(&uri);
            // Clear diagnostics for the file the editor just closed.
            publish_empty_diagnostics(conn, &uri);
        }
        _ => {
            // Quietly ignore unknown notifications — editors send a lot
            // of stuff we haven't advertised support for, and that's fine.
        }
    }
}

fn cast_notif<T: serde::de::DeserializeOwned>(not: &Notification) -> Result<T, serde_json::Error> {
    serde_json::from_value(not.params.clone())
}

// ---------------- diagnostics ----------------

/// Compute and push diagnostics for `uri`. Path: look up the buffer,
/// pick single-file vs project mode, run the pipeline, map every C+
/// `Diagnostic` to the LSP shape, send `textDocument/publishDiagnostics`.
/// Also caches the *original* cplus-core diagnostics so the
/// code-action handler can resurface their suggestions later.
fn publish_diagnostics_for(conn: &Connection, state: &mut ServerState, uri: &Url) {
    let Some(snap) = state.docs.get(uri).cloned() else { return; };
    let path = match uri.to_file_path() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("cpc-lsp: non-file URI {uri}; skipping");
            return;
        }
    };

    let by_file = compute_diagnostics(&path, &snap.text);

    // Cache the originals + push the mapped LSP shape. We clear any
    // entries from `last_diagnostics` that the current run didn't refresh
    // for the open file's URI — otherwise stale suggestions would persist
    // in code-action queries after the user fixes the issue.
    let mut pushed_open = false;
    for (file_path, raw_diags) in &by_file {
        let file_uri = match Url::from_file_path(file_path) {
            Ok(u) => u,
            Err(_) => continue,
        };
        if file_uri == *uri { pushed_open = true; }
        let lsp_diags: Vec<Diagnostic> = raw_diags.iter().map(map_diagnostic).collect();
        state.last_diagnostics.insert(file_uri.clone(), raw_diags.clone());
        push_diagnostics(conn, &file_uri, &lsp_diags, snap.version);
    }
    if !pushed_open {
        // Clear stale diagnostics on the open file.
        state.last_diagnostics.insert(uri.clone(), Vec::new());
        push_diagnostics(conn, uri, &[], snap.version);
    }
}

fn publish_empty_diagnostics(conn: &Connection, uri: &Url) {
    push_diagnostics(conn, uri, &[], 0);
}

fn push_diagnostics(conn: &Connection, uri: &Url, diags: &[Diagnostic], version: i32) {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: diags.to_vec(),
        version: Some(version),
    };
    let notif = Notification {
        method: lsp_types::notification::PublishDiagnostics::METHOD.into(),
        params: serde_json::to_value(params).expect("PublishDiagnosticsParams serializes"),
    };
    let _ = conn.sender.send(Message::Notification(notif));
}

/// Run the C+ pipeline against the open file's buffer (and any imported
/// files if a `Cplus.toml` is reachable). Returns the *raw* cplus-core
/// diagnostics grouped by their originating file path — the LSP layer
/// maps them to the LSP shape before pushing, and keeps the originals
/// for the code-action handler (slice 4E.2).
fn compute_diagnostics(
    open_path: &Path,
    open_text: &str,
) -> BTreeMap<PathBuf, Vec<cplus_core::diagnostics::Diagnostic>> {
    let mut by_file: BTreeMap<PathBuf, Vec<cplus_core::diagnostics::Diagnostic>> = BTreeMap::new();

    match find_manifest(open_path) {
        ManifestProbe::None => {
            // No Cplus.toml in any ancestor — fall through to single-file
            // mode below.
        }
        ManifestProbe::Error(d) => {
            // Manifest exists but failed to parse / had unsupported
            // edition / etc. Surface the error as a diagnostic on the
            // manifest file itself; don't fall back to single-file mode
            // (that would hide the real problem).
            push_into(&mut by_file, d);
            by_file.entry(open_path.to_path_buf()).or_default();
            return by_file;
        }
        ManifestProbe::Loaded { manifest, .. } => {
            if !manifest.bins[0].path.is_file() {
                // E0407 — manifest's binary entry doesn't exist on disk.
                push_into(&mut by_file, manifest_entry_missing_diagnostic(&manifest));
                by_file.entry(open_path.to_path_buf()).or_default();
                return by_file;
            }
            match resolver::load_project(&manifest.bins[0].path, &manifest.root) {
                Ok(mut loaded) => {
                    // Phase 5 slice 5ATTR.1: attribute validation runs first.
                    let attr_diags = attrs::check_multi(
                        &loaded.program,
                        manifest.bins[0].path.clone(),
                        open_text,
                        loaded.files.clone(),
                    );
                    for d in attr_diags { push_into(&mut by_file, d); }
                    let lower_diags = lower::lower(&mut loaded.program, &manifest.bins[0].path, open_text);
                    for d in lower_diags { push_into(&mut by_file, d); }
                    let diags = sema::check_multi(
                        &loaded.program,
                        manifest.bins[0].path.clone(),
                        open_text,
                        loaded.files.clone(),
                    );
                    for d in diags { push_into(&mut by_file, d); }
                    // Phase 5 borrow checker (slice 5BC.2a).
                    let bc_diags = borrowck::check(&loaded.program, &manifest.bins[0].path, open_text);
                    for d in bc_diags { push_into(&mut by_file, d); }
                }
                Err(failure) => {
                    push_into(&mut by_file, failure.to_diagnostic());
                }
            }
            by_file.entry(open_path.to_path_buf()).or_default();
            return by_file;
        }
    }

    let toks = match lexer::tokenize(open_text) {
        Ok(t) => t,
        Err(e) => {
            let lm = cplus_core::diagnostics::LineMap::new(open_text);
            let d = cplus_core::diagnostics::from_lex(&e, &open_path.to_path_buf(), &lm, open_text);
            push_into(&mut by_file, d);
            return by_file;
        }
    };
    let mut prog = match parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = cplus_core::diagnostics::LineMap::new(open_text);
            let d = cplus_core::diagnostics::from_parse(&e, &open_path.to_path_buf(), &lm, open_text);
            push_into(&mut by_file, d);
            return by_file;
        }
    };
    // Phase 5 slice 5ATTR.1: attribute validation runs first.
    let attr_diags = attrs::check(&prog, open_path.to_path_buf(), open_text);
    for d in attr_diags { push_into(&mut by_file, d); }
    let lower_diags = lower::lower(&mut prog, &open_path.to_path_buf(), open_text);
    for d in lower_diags { push_into(&mut by_file, d); }
    let diags = sema::check(&prog, open_path.to_path_buf(), open_text);
    for d in diags { push_into(&mut by_file, d); }
    // Phase 5 borrow checker (slice 5BC.2a).
    let bc_diags = borrowck::check(&prog, &open_path.to_path_buf(), open_text);
    for d in bc_diags { push_into(&mut by_file, d); }

    by_file.entry(open_path.to_path_buf()).or_default();
    by_file
}

fn push_into(
    map: &mut BTreeMap<PathBuf, Vec<cplus_core::diagnostics::Diagnostic>>,
    d: cplus_core::diagnostics::Diagnostic,
) {
    let file = d.primary.file.clone();
    map.entry(file).or_default().push(d);
}

/// Result of walking up from the open file looking for `Cplus.toml`.
/// Three outcomes:
///   - `None`: no manifest in any ancestor → single-file mode.
///   - `Error`: manifest exists but failed to parse / validate → emit
///     the error as a diagnostic.
///   - `Loaded`: manifest is well-formed → run project mode.
enum ManifestProbe {
    None,
    Error(cplus_core::diagnostics::Diagnostic),
    Loaded {
        #[allow(dead_code)]
        manifest_path: PathBuf,
        manifest: manifest::Manifest,
    },
}

fn find_manifest(open_path: &Path) -> ManifestProbe {
    let Some(mut dir) = open_path.parent() else { return ManifestProbe::None; };
    loop {
        let candidate = dir.join("Cplus.toml");
        if candidate.is_file() {
            return match manifest::load(&candidate) {
                Ok(m) if !m.bins.is_empty() => ManifestProbe::Loaded {
                    manifest_path: candidate,
                    manifest: m,
                },
                Ok(_) => ManifestProbe::None, // empty bin list: behave as single-file
                Err(e) => ManifestProbe::Error(e.to_diagnostic()),
            };
        }
        let Some(parent) = dir.parent() else { return ManifestProbe::None; };
        dir = parent;
    }
}

/// Build the E0407 "binary entry missing" diagnostic.
fn manifest_entry_missing_diagnostic(
    m: &manifest::Manifest,
) -> cplus_core::diagnostics::Diagnostic {
    use cplus_core::diagnostics::{DiagCode, Position as P, Severity, SourceSpan};
    cplus_core::diagnostics::Diagnostic {
        severity: Severity::Error,
        code: DiagCode("E0407"),
        message: format!(
            "binary entry `{}` does not exist",
            m.bins[0].path.display()
        ),
        primary: SourceSpan {
            file: m.bins[0].path.clone(),
            start: P { line: 1, col: 1, byte: 0 },
            end: P { line: 1, col: 1, byte: 0 },
        },
        labels: Vec::new(),
        notes: Vec::new(),
        suggestions: Vec::new(),
    }
}

/// Map a cplus-core `Diagnostic` to LSP's `Diagnostic`. Mechanical.
fn map_diagnostic(d: &cplus_core::diagnostics::Diagnostic) -> Diagnostic {
    let severity = Some(match d.severity {
        cplus_core::diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
        cplus_core::diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
        cplus_core::diagnostics::Severity::Note => DiagnosticSeverity::INFORMATION,
    });
    let range = Range {
        start: Position {
            // LSP expects 0-based line/col; cplus-core stores 1-based.
            line: d.primary.start.line.saturating_sub(1),
            character: d.primary.start.col.saturating_sub(1),
        },
        end: Position {
            line: d.primary.end.line.saturating_sub(1),
            character: d.primary.end.col.saturating_sub(1),
        },
    };
    Diagnostic {
        range,
        severity,
        code: Some(NumberOrString::String(d.code.0.to_string())),
        code_description: None,
        source: Some("cpc".to_string()),
        message: d.message.clone(),
        related_information: if d.notes.is_empty() {
            None
        } else {
            // Notes don't carry their own span in cplus-core today — render
            // them as a newline-joined trailer on the message so editors
            // surface them inline.
            None
        },
        tags: None,
        data: None,
    }
    // Quick-fix code-actions (lifted from `d.suggestions`) land in slice 4E.2.
}

