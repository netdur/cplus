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

use cplus_core::graph::{self, CodeGraph};
use cplus_core::resolver::LoadedProject;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the server loop until stdin closes. The graph and project are borrowed
/// for the whole session (resident).
pub fn serve(g: &CodeGraph, loaded: &LoadedProject) -> ExitCode {
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
        if let Some(resp) = handle(&msg, g, loaded) {
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}

fn handle(msg: &Value, g: &CodeGraph, loaded: &LoadedProject) -> Option<String> {
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
            let (text, is_error) = call_tool(name, args, g, loaded);
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
fn call_tool(name: &str, args: &Value, g: &CodeGraph, loaded: &LoadedProject) -> (String, bool) {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str());
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
        other => (format!("unknown tool: `{other}`"), true),
    }
}

fn type_at(args: &Value, g: &CodeGraph, loaded: &LoadedProject) -> (String, bool) {
    let (Some(file), Some(line), Some(col)) = (
        args.get("file").and_then(|v| v.as_str()),
        args.get("line").and_then(|v| v.as_u64()),
        args.get("col").and_then(|v| v.as_u64()),
    ) else {
        return missing("file/line/col");
    };
    let Some((fid, (_, src))) = loaded
        .files
        .iter()
        .find(|(_, (p, _))| p.ends_with(file) || p.to_string_lossy() == file)
    else {
        return (format!("no source file matching `{file}`"), true);
    };
    let Some(byte) = graph::byte_offset(src, line as u32, col as u32) else {
        return (format!("position {line}:{col} is out of range"), true);
    };
    match g.type_at_json(fid, byte) {
        Some(j) => (j, false),
        None => (
            format!("no locally-typed node at {file}:{line}:{col}"),
            true,
        ),
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
            "description": "Outline the symbols of a file (or the whole project if `file` is omitted).",
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
            "description": "The type at a position — resolves a parameter, field, typed local, `self`, or a use of one. Inferred expressions return an error (not yet typed).",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Source file path." },
                    "line": { "type": "integer", "description": "1-based line." },
                    "col": { "type": "integer", "description": "1-based column." },
                },
                "required": ["file", "line", "col"],
            }),
        },
    ])
}
