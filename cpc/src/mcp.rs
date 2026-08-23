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
//! An index built once and never refreshed is stale from the first save, and an
//! editor's only recourse would be to respawn — which costs more than the
//! one-shot CLI the resident server replaced. So the session owns three things
//! a one-shot invocation cannot have:
//!
//! * **Overlays.** `did_change` hands over an unsaved buffer; every later answer
//!   is about that text. The caret is always in a buffer that differs from disk,
//!   so for completion this is not a refinement, it is the whole question.
//! * **Rebuilds on a worker.** A rebuild is ~2 s on a large project and the
//!   transport is one stdio pipe, so building inline would freeze every other
//!   query for the duration. Builds run on a thread; reads answer instantly from
//!   the newest finished graph. A caller who needs the *new* graph specifically
//!   asks to wait for it.
//! * **A last good graph.** A half-typed line does not parse. That is the normal
//!   state during completion, not an exception, so a failed rebuild keeps the
//!   previous graph answering and reports the error through `graph_status`
//!   rather than going blind.

use cplus_core::graph::{self, CodeGraph};
use cplus_core::resolver::LoadedProject;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// What a worker hands back when it finishes. The generation it built is
/// tracked on the session side instead, so a worker that dies without sending
/// anything is still accounted for.
struct BuildDone {
    ms: u128,
    outcome: Result<(LoadedProject, CodeGraph), String>,
}

/// The resident state: the project, the graph built over it, the unsaved
/// buffers that graph was built from, and whatever build is currently in
/// flight.
struct Session<L> {
    /// Re-resolves the project from disk with the given overlays applied.
    load: Arc<L>,
    loaded: LoadedProject,
    graph: CodeGraph,
    /// Canonical path → unsaved buffer text, for the files an editor has dirty.
    overlays: BTreeMap<PathBuf, String>,
    /// Bumped whenever the overlays change or a reload is demanded. The graph
    /// is current when `built_generation == generation`.
    generation: u64,
    built_generation: u64,
    /// The generation a worker is currently building, and the channel it will
    /// report on. The generation is kept here too so a worker that dies can
    /// still be accounted for.
    pending: Option<(u64, Receiver<BuildDone>)>,
    /// Why the last rebuild failed, if it did. The previous graph stays in
    /// `graph` and keeps answering.
    stale: Option<String>,
    last_build_ms: u128,
}

impl<L> Session<L>
where
    L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
{
    /// Start a worker for the current overlays, unless one is already running
    /// or the graph is already current.
    fn kick(&mut self) {
        if self.pending.is_some() || self.generation == self.built_generation {
            return;
        }
        let generation = self.generation;
        let load = Arc::clone(&self.load);
        let overlays = self.overlays.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let t = Instant::now();
            let outcome = (load)(&overlays).map(|loaded| {
                let g = CodeGraph::build(&loaded);
                (loaded, g)
            });
            let _ = tx.send(BuildDone {
                ms: t.elapsed().as_millis(),
                outcome,
            });
        });
        self.pending = Some((generation, rx));
    }

    /// Take any finished build and start the next one if the buffers moved on.
    ///
    /// With `wait`, keep going until the graph reflects the current buffers —
    /// that is what a caller who asked to wait is asking for, and it may mean
    /// waiting through one build that was already in flight for an older
    /// generation before the one they care about even starts.
    fn absorb(&mut self, wait: bool) {
        loop {
            if let Some((generation, rx)) = &self.pending {
                let generation = *generation;
                let got = if wait {
                    // A dead worker (`Err`) is a finished generation, not a
                    // reason to keep waiting: without this the retry below
                    // would spawn another worker to die the same way, forever.
                    rx.recv().map_err(|_| ())
                } else {
                    match rx.try_recv() {
                        Ok(d) => Ok(d),
                        Err(mpsc::TryRecvError::Empty) => return,
                        Err(mpsc::TryRecvError::Disconnected) => Err(()),
                    }
                };
                self.pending = None;
                self.built_generation = generation;
                match got {
                    Ok(done) => {
                        self.last_build_ms = done.ms;
                        match done.outcome {
                            Ok((loaded, graph)) => {
                                self.loaded = loaded;
                                self.graph = graph;
                                self.stale = None;
                            }
                            // A build that failed still counts as *attempted*
                            // for its generation: retrying the same unparseable
                            // buffer on every query would turn each one into a
                            // 2 s no-op.
                            Err(msg) => self.stale = Some(msg),
                        }
                    }
                    Err(()) => {
                        self.stale = Some(
                            "the rebuild worker died; still answering from the last good graph"
                                .to_string(),
                        )
                    }
                }
            }
            self.kick();
            if !wait || self.pending.is_none() {
                return;
            }
        }
    }

    /// Record a new buffer. Returns whether it actually differed.
    fn set_overlay(&mut self, path: PathBuf, text: &str) -> bool {
        if self.overlays.get(&path).map(|t| t == text).unwrap_or(false) {
            return false;
        }
        self.overlays.insert(path, text.to_string());
        self.generation += 1;
        true
    }

    fn clear_overlay(&mut self, path: &Path) -> bool {
        if self.overlays.remove(path).is_none() {
            return false;
        }
        self.generation += 1;
        true
    }

    /// Resolve a client-supplied file string to the canonical path the resolver
    /// keys overlays by. Accepts what the position tools accept — a path
    /// relative to the project root, an absolute path, or a file id
    /// (`src.services.cpc`) — and falls back to a root-relative path for a file
    /// that is not on disk yet.
    fn canonical_for(&self, file: &str) -> PathBuf {
        if let Some((_, (path, _))) = find_file(&self.loaded, file) {
            return path.clone();
        }
        let p = Path::new(file);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        };
        std::fs::canonicalize(&joined).unwrap_or(joined)
    }

    fn status_json(&self) -> Value {
        let mut overlays: Vec<String> = self
            .overlays
            .keys()
            .map(|p| p.display().to_string())
            .collect();
        overlays.sort();
        json!({
            "files": self.loaded.files.len(),
            "nodes": self.graph.nodes.len(),
            "edges": self.graph.edges.len(),
            "overlays": overlays,
            // A build is running right now.
            "building": self.pending.is_some(),
            // The graph does not yet reflect the buffers the client sent.
            "pending_rebuild": self.generation != self.built_generation,
            "stale": self.stale.is_some(),
            "error": self.stale,
            "last_build_ms": self.last_build_ms,
        })
    }
}

/// Run the server loop until stdin closes. Takes ownership of the initial
/// project so the session can replace it on reload; `load_ms` is what that
/// first load already cost the caller, reported as `last_build_ms` so the
/// startup and reload numbers mean the same thing.
pub fn serve<L>(loaded: LoadedProject, load_ms: u128, load: L) -> ExitCode
where
    L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
{
    let t = Instant::now();
    let graph = CodeGraph::build(&loaded);
    let mut session = Session {
        load: Arc::new(load),
        loaded,
        graph,
        overlays: BTreeMap::new(),
        generation: 0,
        built_generation: 0,
        pending: None,
        stale: None,
        last_build_ms: load_ms + t.elapsed().as_millis(),
    };
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

fn handle<L>(msg: &Value, session: &mut Session<L>) -> Option<String>
where
    L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
{
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
fn call_tool<L>(name: &str, args: &Value, session: &mut Session<L>) -> (String, bool)
where
    L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
{
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
            let mut st = session.status_json();
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
            let mut st = session.status_json();
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
            session.generation += 1;
            session.absorb(flag("wait").unwrap_or(true));
            let mut st = session.status_json();
            st["kind"] = json!("reload");
            let failed = session.stale.is_some();
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
            let mut st = session.status_json();
            st["kind"] = json!("graph-status");
            return (serde_json::to_string_pretty(&st).unwrap_or_default(), false);
        }
        _ => {}
    }

    // Reads never block: they answer from the newest finished graph, and pick
    // up a worker's result the moment it lands. A client that needs a specific
    // buffer reflected asks `did_change` to wait.
    session.absorb(false);
    let g = &session.graph;
    let loaded = &session.loaded;
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

/// Find the loaded file a client-supplied string names: a path suffix
/// (`src/services/cpc.cplus`), a full path, or the graph's own file id
/// (`src.services.cpc`).
fn find_file<'a>(
    loaded: &'a LoadedProject,
    file: &str,
) -> Option<(&'a String, &'a (std::path::PathBuf, String))> {
    loaded.files.iter().find(|(fid, (p, _))| {
        p.ends_with(file) || p.to_string_lossy() == file || fid.as_str() == file
    })
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
