//! A resident code graph: the project, the graph over it, the unsaved buffers
//! that graph was built from, and the worker rebuilding it.
//!
//! This lives in core rather than in one front end because there are two of
//! them — `cpc mcp` (the agent surface) and `cpc lsp` (the editor's) — and they
//! ask the graph the same questions. When the residency lived in `cpc mcp`
//! only, the LSP rebuilt the whole project on **every request**, which on a
//! large project is ~2 s per keystroke-adjacent query; the two front doors had
//! diverged into "resident" and "not", and an editor that spoke the wrong one
//! paid for it. One session type, two callers.
//!
//! Three properties make a graph usable while someone is typing over it:
//!
//! * **Overlays.** An unsaved buffer stands in for the file on disk. The caret
//!   is always in a buffer that differs from disk, so for anything asked at a
//!   caret this is not a refinement, it is the whole question.
//! * **Rebuilds on a worker.** A rebuild is ~2 s on a large project, so
//!   building inline would freeze every other query for the duration. Builds
//!   run on a thread; reads answer instantly from the newest finished graph.
//!   A caller who needs the *new* graph specifically asks to wait for it.
//! * **A last good graph.** A half-typed line does not parse. That is the
//!   normal state during completion, not an exception, so a failed rebuild
//!   keeps the previous graph answering and reports the error through the
//!   status rather than going blind.

use crate::graph::CodeGraph;
use crate::resolver::LoadedProject;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

/// Re-resolves the project from disk with the given overlays applied. Owned by
/// the caller because *how* a project is found differs per front end (a
/// manifest in the cwd for `cpc mcp`, the manifest above the open document for
/// the LSP).
pub type Loader =
    dyn Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static;

/// What a worker hands back when it finishes. The generation it built is
/// tracked on the session side instead, so a worker that dies without sending
/// anything is still accounted for.
struct BuildDone {
    ms: u128,
    outcome: Result<(LoadedProject, CodeGraph), String>,
}

/// What the session is currently holding — the answer to "is what you just told
/// me reflected yet, and did the last build work".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphStatus {
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    /// The buffers standing in for files on disk, as display paths.
    pub overlays: Vec<String>,
    /// A build is running right now.
    pub building: bool,
    /// The graph does not yet reflect the buffers the client sent.
    pub pending_rebuild: bool,
    /// The last rebuild failed; the previous graph is what is answering.
    pub stale: bool,
    pub error: Option<String>,
    pub last_build_ms: u128,
}

pub struct GraphSession {
    load: Arc<Loader>,
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

impl GraphSession {
    /// Take ownership of an already-loaded project and build the first graph.
    /// `load_ms` is what that load already cost the caller, folded into the
    /// reported build time so the startup and reload numbers mean the same
    /// thing.
    pub fn new<L>(loaded: LoadedProject, load_ms: u128, load: L) -> GraphSession
    where
        L: Fn(&BTreeMap<PathBuf, String>) -> Result<LoadedProject, String> + Send + Sync + 'static,
    {
        let t = Instant::now();
        let graph = CodeGraph::build(&loaded);
        GraphSession {
            load: Arc::new(load),
            loaded,
            graph,
            overlays: BTreeMap::new(),
            generation: 0,
            built_generation: 0,
            pending: None,
            stale: None,
            last_build_ms: load_ms + t.elapsed().as_millis(),
        }
    }

    /// Record the overlays the initial project was *already* loaded with,
    /// without marking the graph out of date. The LSP needs this: it collects
    /// every open buffer, loads the project through them, and hands both over —
    /// re-registering them as changes would schedule a rebuild for a graph that
    /// already reflects them.
    pub fn seed_overlays(&mut self, overlays: BTreeMap<PathBuf, String>) {
        self.overlays = overlays;
    }

    pub fn graph(&self) -> &CodeGraph {
        &self.graph
    }

    pub fn loaded(&self) -> &LoadedProject {
        &self.loaded
    }

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
    pub fn absorb(&mut self, wait: bool) {
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

    /// Record a new buffer. Returns whether it actually differed — re-sending
    /// identical text costs no build.
    pub fn set_overlay(&mut self, path: PathBuf, text: &str) -> bool {
        if self.overlays.get(&path).map(|t| t == text).unwrap_or(false) {
            return false;
        }
        self.overlays.insert(path, text.to_string());
        self.generation += 1;
        true
    }

    pub fn clear_overlay(&mut self, path: &Path) -> bool {
        if self.overlays.remove(path).is_none() {
            return false;
        }
        self.generation += 1;
        true
    }

    /// Demand a rebuild even though nothing was marked dirty — "the files under
    /// you moved", which the session cannot see for itself.
    pub fn mark_dirty(&mut self) {
        self.generation += 1;
    }

    /// The unsaved buffer for a file, if one was handed over. This is the text
    /// a caret query should classify against, since it is what the user is
    /// looking at.
    pub fn overlay_text(&self, path: &Path) -> Option<&str> {
        self.overlays.get(path).map(|s| s.as_str())
    }

    /// Resolve a client-supplied file string to the canonical path overlays are
    /// keyed by. Accepts what the position queries accept — a path relative to
    /// the project root, an absolute path, or a file id (`src.services.cpc`) —
    /// and falls back to a root-relative path for a file not on disk yet.
    pub fn canonical_for(&self, file: &str) -> PathBuf {
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

    pub fn status(&self) -> GraphStatus {
        let mut overlays: Vec<String> = self
            .overlays
            .keys()
            .map(|p| p.display().to_string())
            .collect();
        overlays.sort();
        GraphStatus {
            files: self.loaded.files.len(),
            nodes: self.graph.nodes.len(),
            edges: self.graph.edges.len(),
            overlays,
            building: self.pending.is_some(),
            pending_rebuild: self.generation != self.built_generation,
            stale: self.stale.is_some(),
            error: self.stale.clone(),
            last_build_ms: self.last_build_ms,
        }
    }
}

/// Find the loaded file a caller-supplied string names: a path suffix
/// (`src/services/cpc.cplus`), a full path, or the graph's own file id
/// (`src.services.cpc`).
pub fn find_file<'a>(
    loaded: &'a LoadedProject,
    file: &str,
) -> Option<(&'a String, &'a (PathBuf, String))> {
    loaded.files.iter().find(|(fid, (p, _))| {
        p.ends_with(file) || p.to_string_lossy() == file || fid.as_str() == file
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project loaded straight from an in-memory source map, so the session
    /// can be exercised without touching disk.
    fn project_from(src: &str) -> LoadedProject {
        let toks = crate::lexer::tokenize(src).expect("lex");
        let mut program = crate::parser::parse(toks).expect("parse");
        for item in &mut program.items {
            item.origin_file = Some("src".to_string());
        }
        let mut files = BTreeMap::new();
        files.insert(
            "src".to_string(),
            (PathBuf::from("src/main.cplus"), src.to_string()),
        );
        LoadedProject {
            program,
            entry_file_id: "src".to_string(),
            files,
            imports: BTreeMap::new(),
            import_aliases: BTreeMap::new(),
        }
    }

    fn session_over(first: &str, then: &'static str) -> GraphSession {
        GraphSession::new(project_from(first), 0, move |_overlays| {
            Ok(project_from(then))
        })
    }

    #[test]
    fn the_first_graph_is_ready_before_any_build_is_asked_for() {
        let s = session_over("fn a() -> i32 { return 1; }", "fn a() -> i32 { return 1; }");
        assert!(!s.graph().def("a").is_empty());
        assert!(!s.status().pending_rebuild);
        assert!(!s.status().stale);
    }

    #[test]
    fn an_overlay_is_reflected_after_a_wait() {
        let mut s = session_over("fn a() -> i32 { return 1; }", "fn b() -> i32 { return 2; }");
        assert!(s.set_overlay(PathBuf::from("src/main.cplus"), "fn b() -> i32 { return 2; }"));
        assert!(s.status().pending_rebuild, "the graph is behind the buffer");
        s.absorb(true);
        assert!(!s.graph().def("b").is_empty(), "the new name is in the graph");
        assert!(!s.status().pending_rebuild);
        assert_eq!(s.status().overlays.len(), 1);
    }

    #[test]
    fn re_sending_identical_text_costs_no_build() {
        let mut s = session_over("fn a() -> i32 { return 1; }", "fn a() -> i32 { return 1; }");
        let text = "fn a() -> i32 { return 1; }";
        assert!(s.set_overlay(PathBuf::from("src/main.cplus"), text));
        s.absorb(true);
        assert!(
            !s.set_overlay(PathBuf::from("src/main.cplus"), text),
            "the same text again is not a change"
        );
        assert!(!s.status().pending_rebuild);
    }

    #[test]
    fn a_failed_rebuild_keeps_the_last_good_graph_and_reports_why() {
        let mut s = GraphSession::new(project_from("fn a() -> i32 { return 1; }"), 0, |_| {
            Err("expected `}`".to_string())
        });
        s.mark_dirty();
        s.absorb(true);
        let st = s.status();
        assert!(st.stale, "the failure is reported");
        assert_eq!(st.error.as_deref(), Some("expected `}`"));
        assert!(
            !s.graph().def("a").is_empty(),
            "the previous graph still answers"
        );
        assert!(
            !st.pending_rebuild,
            "a failed build still counts as attempted — it must not retry forever"
        );
    }

    #[test]
    fn clearing_an_overlay_that_was_never_set_is_not_a_change() {
        let mut s = session_over("fn a() -> i32 { return 1; }", "fn a() -> i32 { return 1; }");
        assert!(!s.clear_overlay(Path::new("src/nope.cplus")));
        assert!(!s.status().pending_rebuild);
    }

    #[test]
    fn a_file_resolves_by_path_suffix_or_by_id() {
        let p = project_from("fn a() -> i32 { return 1; }");
        assert!(find_file(&p, "src/main.cplus").is_some());
        assert!(find_file(&p, "main.cplus").is_some());
        assert!(find_file(&p, "src").is_some(), "the file id");
        assert!(find_file(&p, "nope.cplus").is_none());
    }
}
