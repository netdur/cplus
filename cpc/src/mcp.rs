//! `cpc mcp` — a resident, stdio MCP server over the code knowledge graph.
//!
//! The graph is built once at startup and kept warm in memory; each request is
//! answered from that index, so an agent's query is a memory lookup rather than
//! a re-parse (the load-bearing decision in plan.graph.md §3). The transport is
//! MCP stdio: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! The tool names and descriptions are written *for the model* (§7): they read
//! as the obvious first reach, and each says plainly "use this instead of grep
//! — it is resolved and typed, grep is neither."
//!
//! ## The graph is live
//!
//! The residency itself — overlays, the rebuild worker, the last good graph —
//! lives in [`cplus_core::session`], because `cpc lsp` needs exactly the same
//! thing and used to rebuild the whole project per request instead. This file
//! is the MCP shape over it: tool names and descriptions written for the model,
//! and the JSON each verb answers with.
use cplus_core::graph::{self, CodeGraph};
use cplus_core::resolver::LoadedProject;
use cplus_core::session::{find_file, GraphSession};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// The status JSON every session verb answers with, as a mutable object the
/// caller stamps its own `kind` (and extras) onto.
fn status_json(session: &GraphSession) -> Value {
    serde_json::to_value(session.status()).unwrap_or_else(|_| json!({}))
}

/// The text a caret question should be classified against: the unsaved buffer
/// if the client handed one over, else what is on disk. The overlay wins even
/// while its rebuild is in flight — the user is looking at that text, and the
/// classification half of completion is pure lexing over it, so it is correct
/// before the semantic half catches up.
fn source_for<'a>(session: &'a GraphSession, file: &str) -> Option<(String, &'a str)> {
    let (fid, (path, disk)) = find_file(session.loaded(), file)?;
    let text = session.overlay_text(path).unwrap_or(disk.as_str());
    Some((fid.clone(), text))
}

/// Run the server loop until stdin closes. Takes ownership of the initial
/// project so the session can replace it on reload; `load_ms` is what that
/// first load already cost the caller, reported as `last_build_ms` so the
/// startup and reload numbers mean the same thing.
pub fn serve<L>(loaded: LoadedProject, load_ms: u128, load: L) -> ExitCode
where
    L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
{
    let mut session = GraphSession::new(loaded, load_ms, load);
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                // Can't recover an id from an unparseable message; reply with null id.
                let _ = writeln!(out, "{}", error(Value::Null, -32700, "parse error"));
                let _ = out.flush();
                continue;
            }
        };
        if let Some(resp) = handle(&msg, &mut session) {
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}

fn handle(msg: &Value, session: &mut GraphSession) -> Option<String> {
    // A message with no `method` is a response we don't track; ignore it.
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id").cloned();
    match method {
        "initialize" => {
            // Echo the client's protocol version when it offers one.
            let pv = msg
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION)
                .to_string();
            Some(result(
                id?,
                json!({
                    "protocolVersion": pv,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "cpc-graph", "version": env!("CARGO_PKG_VERSION") },
                }),
            ))
        }
        "tools/list" => Some(result(id?, json!({ "tools": tool_defs() }))),
        "tools/call" => {
            let params = msg.get("params")?;
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let empty = json!({});
            let args = params.get("arguments").unwrap_or(&empty);
            let (text, is_error) = call_tool(name, args, session);
            Some(result(
                id?,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            ))
        }
        "ping" => Some(result(id?, json!({}))),
        // Notifications (initialized, cancelled, …) get no reply.
        m if m.starts_with("notifications/") => None,
        _ => id.map(|i| error(i, -32601, &format!("method not found: {method}"))),
    }
}

/// Dispatch a tool call to a graph query. Returns the result text and whether
/// it is an error (a missing argument or an unknown symbol).
fn call_tool(name: &str, args: &Value, session: &mut GraphSession) -> (String, bool) {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str());

    // The session verbs mutate state and decide their own waiting; they do not
    // go through the read path below.
    let flag = |k: &str| args.get(k).and_then(|v| v.as_bool());
    match name {
        "did_change" => {
            let (Some(file), Some(text)) = (arg("file"), arg("text")) else {
                return missing("file/text");
            };
            let path = session.canonical_for(file);
            let changed = session.set_overlay(path.clone(), text);
            // Default: start the rebuild and return immediately, so an editor
            // can warm the graph while the user keeps typing. `wait: true` for
            // the call right before a question whose answer must reflect this
            // exact text.
            session.absorb(flag("wait").unwrap_or(false));
            let mut st = status_json(session);
            st["kind"] = json!("did-change");
            st["file"] = json!(path.display().to_string());
            st["bytes"] = json!(text.len());
            st["changed"] = json!(changed);
            return (serde_json::to_string_pretty(&st).unwrap_or_default(), false);
        }
        "did_close" => {
            let Some(file) = arg("file") else {
                return missing("file");
            };
            let path = session.canonical_for(file);
            let dropped = session.clear_overlay(&path);
            session.absorb(flag("wait").unwrap_or(false));
            let mut st = status_json(session);
            st["kind"] = json!("did-close");
            st["file"] = json!(path.display().to_string());
            st["dropped"] = json!(dropped);
            return (serde_json::to_string_pretty(&st).unwrap_or_default(), false);
        }
        "reload" => {
            // Force a build even when nothing was marked dirty: `reload` means
            // "the files under you moved", which the server cannot see. It
            // waits by default — a caller asking for a reload wants the new
            // graph, not a promise of one.
            session.mark_dirty();
            session.absorb(flag("wait").unwrap_or(true));
            let mut st = status_json(session);
            st["kind"] = json!("reload");
            let failed = session.status().stale;
            return (
                serde_json::to_string_pretty(&st).unwrap_or_default(),
                failed,
            );
        }
        "graph_status" => {
            // Observes without building — but does collect a worker that has
            // already finished, so a client polling this sees the new graph the
            // moment it is ready.
            session.absorb(false);
            let mut st = status_json(session);
            st["kind"] = json!("graph-status");
            return (serde_json::to_string_pretty(&st).unwrap_or_default(), false);
        }
        _ => {}
    }

    // Reads never block: they answer from the newest finished graph, and pick
    // up a worker's result the moment it lands. A client that needs a specific
    // buffer reflected asks `did_change` to wait.
    session.absorb(false);
    if name == "complete_at" {
        return complete_at(args, session);
    }
    let g = session.graph();
    let loaded = session.loaded();
    match name {
        "find_definition" => match arg("symbol") {
            Some(s) => (CodeGraph::nodes_to_json(&g.def(s)), false),
            None => missing("symbol"),
        },
        "find_members" => match arg("type") {
            Some(t) => (CodeGraph::nodes_to_json(&g.members(t)), false),
            None => missing("type"),
        },
        "file_symbols" => (CodeGraph::nodes_to_json(&g.symbols(arg("file"))), false),
        "find_references" => opt(arg("symbol"), "symbol", |s| g.refs_json(s)),
        "find_callers" => opt(arg("function"), "function", |f| g.callers_json(f)),
        "find_callees" => opt(arg("function"), "function", |f| g.callees_json(f)),
        "code_context" => opt(arg("function"), "function", |f| g.context_json(f)),
        "call_hierarchy" => match arg("function") {
            Some(f) => {
                let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
                match g.call_hierarchy_json(f, depth) {
                    Some(j) => (j, false),
                    None => not_found(f),
                }
            }
            None => missing("function"),
        },
        "type_at" => type_at(args, g, loaded),
        "scope_at" => scope_at(args, g, loaded),
        other => (format!("unknown tool: `{other}`"), true),
    }
}

/// Resolve `file` + 1-based `line`/`col` to a (file id, byte offset) pair.
fn position(args: &Value, loaded: &LoadedProject) -> Result<(String, u32), (String, bool)> {
    let (Some(file), Some(line), Some(col)) = (
        args.get("file").and_then(|v| v.as_str()),
        args.get("line").and_then(|v| v.as_u64()),
        args.get("col").and_then(|v| v.as_u64()),
    ) else {
        return Err(missing("file/line/col"));
    };
    let Some((fid, (_, src))) = find_file(loaded, file) else {
        return Err((format!("no source file matching `{file}`"), true));
    };
    let Some(byte) = graph::byte_offset(src, line as u32, col as u32) else {
        return Err((format!("position {line}:{col} is out of range"), true));
    };
    Ok((fid.clone(), byte))
}

fn scope_at(args: &Value, g: &CodeGraph, loaded: &LoadedProject) -> (String, bool) {
    match position(args, loaded) {
        Ok((fid, byte)) => (g.scope_at_json(&fid, byte), false),
        Err(e) => e,
    }
}

/// `complete_at` — the composed answer at a caret.
///
/// Two halves with different freshness requirements, which is why this is a
/// verb and not a client-side chain: the *classification* (is this a `.`, a
/// `::`, or a bare word) reads the live buffer and is always exact, while the
/// *candidates* come from the last finished graph. A client that wants both
/// current sends `did_change` first; one that just wants the shape of the
/// answer does not have to.
fn complete_at(args: &Value, session: &GraphSession) -> (String, bool) {
    let (Some(file), Some(line), Some(col)) = (
        args.get("file").and_then(|v| v.as_str()),
        args.get("line").and_then(|v| v.as_u64()),
        args.get("col").and_then(|v| v.as_u64()),
    ) else {
        return missing("file/line/col");
    };
    let Some((fid, held)) = source_for(session, file) else {
        return (format!("no source file matching `{file}`"), true);
    };
    // An explicit `text` is for the caller who has not sent `did_change` and
    // wants one answer about a buffer the server has never seen.
    let src = args
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or(held);
    let Some(byte) = graph::byte_offset(src, line as u32, col as u32) else {
        return (format!("position {line}:{col} is out of range"), true);
    };
    (
        session.graph().complete_at_json(&fid, src, byte),
        false,
    )
}

fn type_at(args: &Value, g: &CodeGraph, loaded: &LoadedProject) -> (String, bool) {
    let (fid, byte) = match position(args, loaded) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match g.type_at_json(&fid, byte) {
        Some(j) => (j, false),
        None => ("no locally-typed node at that position".to_string(), true),
    }
}

fn opt(a: Option<&str>, field: &str, f: impl Fn(&str) -> Option<String>) -> (String, bool) {
    match a {
        Some(s) => match f(s) {
            Some(j) => (j, false),
            None => not_found(s),
        },
        None => missing(field),
    }
}

fn missing(field: &str) -> (String, bool) {
    (format!("missing required argument: `{field}`"), true)
}

fn not_found(name: &str) -> (String, bool) {
    (format!("`{name}` is not a known symbol"), true)
}

fn result(id: Value, value: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string()
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// The agent-facing tool surface. Names and descriptions are deliberately
/// written so a model reaches for these before `grep`.
fn tool_defs() -> Value {
    let sym = |req: &str, desc: &str| {
        json!({
            "type": "object",
            "properties": { req: { "type": "string", "description": desc } },
            "required": [req],
        })
    };
    json!([
        {
            "name": "find_definition",
            "description": "Find where a C+ symbol is defined (function, method, type, field, const). Resolved and typed — use this instead of grep, which can't tell a type from a same-named local. Arg: `symbol` (bare name or qualified id).",
            "inputSchema": sym("symbol", "Symbol name, e.g. `Point` or `src.geo::Point::area`."),
        },
        {
            "name": "find_references",
            "description": "Find every use site of a symbol (call sites and named-type uses) with precise file:line:col. The resolved replacement for grepping a name. The result's `scope` says what coverage it has.",
            "inputSchema": sym("symbol", "Symbol to find uses of."),
        },
        {
            "name": "find_callers",
            "description": "Find the functions/methods that call a given function. Resolved call edges — beats grepping `name(` which also matches the definition, comments, and unrelated names.",
            "inputSchema": sym("function", "Function or method name."),
        },
        {
            "name": "find_callees",
            "description": "Find what a given function calls (one hop). Carries an `unresolved` count for call sites whose target couldn't be resolved statically.",
            "inputSchema": sym("function", "Function or method name."),
        },
        {
            "name": "call_hierarchy",
            "description": "Transitive callees of a function to a given depth. Use to understand blast radius before changing a function.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "function": { "type": "string", "description": "Function or method name." },
                    "depth": { "type": "integer", "description": "Max hops (default 3)." },
                },
                "required": ["function"],
            }),
        },
        {
            "name": "find_members",
            "description": "List the fields and methods of a struct or enum.",
            "inputSchema": sym("type", "Struct or enum name."),
        },
        {
            "name": "file_symbols",
            "description": "Outline the symbols of a file (or the whole project if `file` is omitted). A `#[test]` function carries `is_test: true` — keep them for an outline, drop them when completing a name.",
            "inputSchema": json!({
                "type": "object",
                "properties": { "file": { "type": "string", "description": "Optional file id, e.g. `src.main`." } },
            }),
        },
        {
            "name": "code_context",
            "description": "The one-shot edit pack for a function: its signature, callers, callees, and the types it touches. Prefer this over several separate lookups when about to change a function.",
            "inputSchema": sym("function", "Function or method name."),
        },
        {
            "name": "type_at",
            "description": "The type at a position — resolves a parameter, field, typed local, `self`, or a use of one, and inferred expressions (call results, field/index reads, match/if values).",
            "inputSchema": pos_schema(),
        },
        {
            "name": "scope_at",
            "description": "Every name you can type at a position: the locals and parameters in scope (with their types where known), `this`, the file's import aliases and what module each resolves to, and the file's own module-level items. Shadowed names are already removed, and so are `#[test]` functions — nobody completes one. This is the \"what can I call here\" question — pair it with `find_members` after a `.` and `file_symbols` after an alias `::`.",
            "inputSchema": pos_schema(),
        },
        {
            "name": "complete_at",
            "description": "What to type at a caret, already composed: after a `.` the receiver's fields and methods, after a `::` the module's or type's qualified names, and otherwise everything in scope — filtered by the word already typed and ranked with the nearest bindings first. This is `scope_at` + `type_at` + `find_members` with the \"which question is this\" step done for you, and that step is the one a caller cannot do without re-deriving C+'s own rules. The classification always reads the current buffer, so it is right even mid-edit; the candidates come from the last finished graph, so pair it with `did_change` when the names themselves are what just changed. `receiver_type` absent on a member answer means the receiver's type is not locally known — an empty list, never a guess.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Source file path, or the file id (`src.services.cpc`)." },
                    "line": { "type": "integer", "description": "1-based line of the caret." },
                    "col": { "type": "integer", "description": "1-based column of the caret." },
                    "text": { "type": "string", "description": "Optional: the buffer to classify against, for a caller who has not sent `did_change`. Defaults to the overlay for this file, else what is on disk. Note the caret is still resolved against the graph's copy, so if your edits have moved the lines *above* the caret, send `did_change` instead — that is the path that rebuilds." },
                },
                "required": ["file", "line", "col"],
            }),
        },
        {
            "name": "did_change",
            "description": "Hand the server an unsaved buffer. Every later answer is about this text instead of the file on disk — which for anything at the caret is the only correct answer, since the caret is always in a buffer that differs from disk. The rebuild runs on a worker and other queries keep being answered from the previous graph while it does; pass `wait: true` on the call right before a question whose answer must reflect this exact text. A burst of edits costs one build, and re-sending identical text costs none. If the buffer does not parse — which a half-typed line usually does not — the last good graph keeps answering and `graph_status` carries the error; an editor completing after a `.` should send the buffer with the incomplete access trimmed and then ask `type_at` about the receiver.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Source file path, or the file id (`src.services.cpc`)." },
                    "text": { "type": "string", "description": "The buffer's full current contents." },
                    "wait": { "type": "boolean", "description": "Block until the graph reflects this text (default false: rebuild in the background)." },
                },
                "required": ["file", "text"],
            }),
        },
        {
            "name": "did_close",
            "description": "Drop the unsaved buffer for a file, so answers go back to what is on disk. Send this after a save, or when the editor closes the file.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Source file path, or the file id." },
                    "wait": { "type": "boolean", "description": "Block until the graph reflects the drop (default false)." },
                },
                "required": ["file"],
            }),
        },
        {
            "name": "reload",
            "description": "Rebuild the graph from disk, keeping any buffers `did_change` handed over. Use after files changed outside the editor — a branch switch, a generator, a dependency update — which the server cannot see. Waits for the new graph by default. Returns the new node/file counts and the build time; on a parse failure the previous graph is kept and the error is reported.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "wait": { "type": "boolean", "description": "Block until the rebuild finishes (default true)." },
                },
            }),
        },
        {
            "name": "graph_status",
            "description": "What the resident server is currently holding: file and node counts, which buffers are overlaid, whether a rebuild is pending, and whether the last rebuild failed (with the error). Observes only — it does not itself rebuild, so `pending_rebuild: true` means the next real query will. Ask this when an answer looks wrong before concluding the graph is broken.",
            "inputSchema": json!({ "type": "object", "properties": {} }),
        },
    ])
}

/// The `file` / `line` / `col` argument shape the position queries share.
fn pos_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "file": { "type": "string", "description": "Source file path, or the file id (`src.services.cpc`)." },
            "line": { "type": "integer", "description": "1-based line." },
            "col": { "type": "integer", "description": "1-based column." },
        },
        "required": ["file", "line", "col"],
    })
}
