# Phase 4 Slice 4E — `cpc-lsp` foundations

> Status: design note, not yet implemented.
> Scope: a Language Server for C+ that serves diagnostics, formatting, and goto-definition in editors (VS Code, Helix, Neovim, Zed, Emacs) over the standard LSP stdio transport. Capability set is intentionally minimal for Phase 4; completions / hover / find-references / refactorings land in Phase 5+ alongside richer sema.
> Out of scope: incremental parsing, virtual workspaces, multi-root workspaces, semantic tokens, code actions / quickfixes (deferred to slice 4E.2), inlay hints, document symbols / outline (deferred), real-time linting beyond the diagnostics surface that `cpc build` already produces.

---

## 1. Problem

§5.1 commits the compiler to a library-first architecture so any tool can build on `cplus-core`. The LSP is the obvious second consumer after the CLI. §5.4 lists `cpc lsp` as a built-in subcommand and §5.9 names "AI recovery" as the loss function — live diagnostics in the editor cut the AI-iteration loop from "save → terminal → re-read errors → fix" to "save → see squiggle → fix." Same for the formatter: `textDocument/formatting` over LSP wraps `cpc fmt` so format-on-save works in any editor.

What we need from the editor's perspective:

1. **Live diagnostics.** Every error code C+ knows about (E0001–E0410) renders as a structured `Diagnostic` in the editor's gutter / problem list, with the same line/col precision and same suggestions that `cpc build --diagnostics=json` produces.
2. **Format on save.** `textDocument/formatting` runs `cpc fmt`'s in-memory entry point and returns a single `TextEdit` replacing the buffer.
3. **Goto-definition** for simple cases — `Ident` references and qualified paths. Skips macros, generics, and anything that requires symbol-table reverse mapping (Phase 5 work).

That's the Phase-4 minimum. Everything else is a Phase-5 polish slice.

---

## 2. CLI surface

```
cpc lsp                          start the LSP server on stdin/stdout
cpc lsp --log /path/to/log       diagnostic logging (stderr by default)
```

Editors discover the binary via the standard mechanism — `cpc lsp` is named in the editor's LSP-config file with no project-level setup needed. Multi-file resolution uses the project's `Cplus.toml` if present in any ancestor directory; otherwise the LSP runs in single-file mode (still useful for editing standalone `.cplus` files).

Transport: stdio JSON-RPC, the LSP standard. No TCP, no Unix socket — keep one config working everywhere.

---

## 3. Capability set (Phase 4 minimum)

The server advertises this capabilities object in its `initialize` response:

```jsonc
{
  "textDocumentSync": {
    "openClose": true,
    "change": 1,         // Full — re-send the whole buffer on edit
    "save": { "includeText": false }
  },
  "documentFormattingProvider": true,
  "definitionProvider": true,
  "diagnosticProvider": {
    "interFileDependencies": true,
    "workspaceDiagnostics": false
  }
}
```

Notes on each:

- **`change: 1`** — full document sync. Incremental sync is cheap to wire up *if* we have an incremental parser; we don't (Phase 4 reparses the whole file on every keystroke). Full sync is dumber and works correctly.
- **`workspaceDiagnostics: false`** — diagnostics are computed per-file, on demand from the editor's "pull" request. Push diagnostics on `didSave` only (no per-keystroke recompute in 4E.1; that's a 4E polish item once the latency profile is real). The "pull" model (LSP 3.17+) plus `didSave` push gives editors the right experience with minimal server-side recomputation.
- **`documentFormattingProvider: true`** — wraps `cpc fmt` via `fmt::format_source`. Returns one `TextEdit` replacing the whole buffer (cheaper for the client to apply than computing a diff).
- **`definitionProvider: true`** — see §5.

Not advertised in 4E.1: hover, completion, code actions, signature help, references, document symbols, semantic tokens, rename, inlay hints, code lens. Each one of these needs its own design slice; advertising a capability we serve poorly is worse than not advertising it (editors gray-out unsupported features cleanly).

---

## 4. Architecture

### 4.1 Binary

A new crate, `cpc-lsp/`, alongside `cpc/`. Both link `cplus-core`. Per §5.1 this is the third tool in the compiler-as-library family.

```
cpc/                 binary — CLI driver (build, fmt, check, lsp dispatch)
cpc-lsp/             binary — LSP server (this slice)
cplus-core/          library — all language logic
```

`cpc lsp` (the subcommand in the CLI binary) `exec`s the `cpc-lsp` binary if it's on PATH; otherwise prints a clear error. This split keeps the CLI binary lean — the LSP brings in `lsp-server` + `lsp-types` deps that compile-time matter for users who just want `cpc build`.

### 4.2 Dependencies

Two new crates, both from rust-analyzer's family — minimal, well-maintained, sync (no tokio):

- **`lsp-server`** — JSON-RPC framing, stdio transport, request dispatch loop. ~3k LOC.
- **`lsp-types`** — LSP protocol types. ~10k LOC of generated bindings.

Why not `tower-lsp`? It pulls tokio and the full async ecosystem. For C+, which has stayed dep-lean, `lsp-server` is a better fit. rust-analyzer itself uses it.

### 4.3 Server lifecycle

```
1. main()                  — parse args, init logging
2. Connection::stdio()     — set up JSON-RPC framing on stdin/stdout
3. initialize handshake    — receive client capabilities, send ours
4. main loop               — read message, dispatch to handler, send reply
   - if request:           call handler, send Response or Error
   - if notification:      call handler, send nothing
5. shutdown / exit         — clean termination
```

Per-document state is held in a `State` struct shared across requests:

```rust
struct State {
    documents: BTreeMap<Url, DocSnapshot>,
    project_cache: Option<ProjectCache>,   // resolver result if Cplus.toml present
}

struct DocSnapshot {
    version: i32,
    text: String,
    // Last diagnostic batch we sent the client. Used to compute
    // pull-diagnostics "unchanged" responses cheaply.
    last_diagnostics: Vec<Diagnostic>,
}
```

`BTreeMap` not `HashMap` per §5.3 determinism.

### 4.4 Request dispatch

The standard `lsp-server` dispatch loop:

```rust
for msg in conn.receiver {
    match msg {
        Message::Request(req) if req.method == "shutdown" => { ... }
        Message::Request(req) if req.method == "textDocument/formatting" => handle_formatting(...),
        Message::Request(req) if req.method == "textDocument/definition" => handle_definition(...),
        Message::Request(req) if req.method == "textDocument/diagnostic" => handle_diagnostic_pull(...),
        Message::Notification(n) if n.method == "textDocument/didOpen" => on_did_open(...),
        Message::Notification(n) if n.method == "textDocument/didChange" => on_did_change(...),
        Message::Notification(n) if n.method == "textDocument/didSave" => on_did_save(...),
        Message::Notification(n) if n.method == "textDocument/didClose" => on_did_close(...),
        _ => {}   // ignore unknown — editors send features we haven't advertised; that's fine
    }
}
```

Each handler is a thin wrapper around `cplus-core` calls. Same library, no separate reimplementation.

---

## 5. Diagnostics

This is the load-bearing feature. Phase 4 produces structured diagnostics already; the LSP just maps them onto LSP's `Diagnostic` type.

### 5.1 Trigger model

Three triggers, in priority order:

1. **`textDocument/didOpen`**: full check on open. Editor gets immediate squiggles when the file appears.
2. **`textDocument/didSave`**: full check on save. The "real" feedback loop — user saves, sees errors.
3. **`textDocument/diagnostic` (pull)**: client asks; we return our cached batch or recompute if dirty.

4E.1 does NOT recompute on `didChange` (per-keystroke). Reasons: (a) Phase 4 has no incremental parser; reparsing the file on every keystroke spikes CPU; (b) most LSP clients can pull on idle anyway; (c) save-triggered checks are the natural fit for an AI workflow (the agent saves a candidate fix, sees the result, iterates).

This decision is revisitable once we have latency data — if save-triggered feels laggy on big projects, push diagnostics on a debounced `didChange`. The compiler-as-library shape makes that a behavior change in the LSP, not a re-architecture.

### 5.2 Multi-file context

If `Cplus.toml` is found in an ancestor of the active file:
- Run the resolver to load the project.
- The diagnostic batch sent for *each* file includes only the diagnostics whose `primary.file` equals that file.
- Editor sees per-file diagnostics as expected.

If no manifest:
- Single-file mode. `sema::check` directly on the buffer. Same shape as `cpc FILE` does today.

The decision is per-file: an open buffer at `/project/src/foo.cplus` runs in project mode if `/project/Cplus.toml` exists. A standalone `/tmp/scratch.cplus` runs in single-file mode.

### 5.3 Mapping cplus-core `Diagnostic` to LSP `Diagnostic`

Already structurally compatible — every field has a counterpart:

| cplus-core            | LSP                                       |
|-----------------------|-------------------------------------------|
| `severity`            | `severity` (Error=1, Warning=2, Note=4)   |
| `code` ("E0341")      | `code` (string variant)                   |
| `message`             | `message`                                 |
| `primary.{file,start,end}` | `range`                              |
| `notes[]`             | rendered into `relatedInformation`        |
| `suggestions[]`       | (4E.2 — `CodeAction` provider)            |

The mapping is mechanical and lives in `cpc-lsp/src/protocol.rs` as a single function.

---

## 6. Formatting

Trivial wrapper around `cplus-core/fmt::format_source`:

```rust
fn handle_formatting(state: &State, params: DocumentFormattingParams) -> Vec<TextEdit> {
    let doc = state.documents.get(&params.text_document.uri)?;
    match cplus_core::fmt::format_source(&doc.text) {
        Ok(formatted) if formatted != doc.text => {
            vec![TextEdit {
                range: whole_doc_range(&doc.text),
                new_text: formatted,
            }]
        }
        _ => vec![],   // already formatted or lex error → no edit
    }
}
```

The client applies the single edit by replacing the whole buffer. Cheap on the client side.

Range formatting (`textDocument/rangeFormatting`) is not advertised in 4E.1 — formatting individual ranges is a 4E.2 / 4D.3 item that requires the formatter to operate on partial AST.

---

## 7. Goto-definition

Phase 4 sema retains enough info to support goto-def for two cases:

1. **Item references**: `Ident("foo")` → location of the `fn foo` / `struct foo` / `enum foo` declaration.
2. **Qualified paths**: `math::square` → location of `pub fn square` in `math.cplus`.

What we do NOT support in 4E.1:
- Field references (`p.x` → field `x` of struct): needs sema to retain expression-type info per span.
- Method calls (`p.translate(1,1)` → `fn translate(...)` declaration in `impl`).
- Variable references (`let x = ...; ... x ...`): needs scope-aware position lookup.

Implementation strategy: when the resolver / sema runs, it records `(file_id, span) → declaration_location` for each top-level item resolution. The LSP queries this table on `textDocument/definition`. Storage cost is small (one entry per cross-reference).

For 4E.1, a simple fallback works: do a *re-lex* of the target file at the cursor's byte position, find the identifier token, then do a project-wide search for any `fn`/`struct`/`enum` declaration whose name matches. Linear in number of items; the project is small. Replace with a proper index in 4E.2 once we have larger projects.

---

## 8. Implementation plan

Three sub-slices.

### 8.1 Slice 4E.1 — Skeleton + diagnostics

- New `cpc-lsp/` crate. `lsp-server` + `lsp-types` deps.
- `Connection::stdio()` setup, initialize handshake.
- `didOpen` / `didSave` triggers full pipeline (lexer + parser + lower + resolver + sema) on the buffer text.
- Diagnostic batches sent via `textDocument/publishDiagnostics`.
- Pull-diagnostics handler (3.17+).
- `cpc lsp` subcommand in the `cpc` binary that `exec`s `cpc-lsp` from PATH.
- Tests: golden tests verify the LSP responds to a synthetic `initialize` / `didOpen` / pull-diagnostic sequence.

### 8.2 Slice 4E.2 — Formatting

- `textDocument/formatting` wired to `fmt::format_source`.
- `CodeAction` provider for diagnostics carrying `MachineApplicable` suggestions — auto-applicable "Quick Fix" for E0401 did-you-mean, etc.
- Tests: formatting request returns expected `TextEdit`s; code-action request returns expected fix.

### 8.3 Slice 4E.3 — Goto-definition

- Index of `(file_id, span) → location` built during resolver/sema.
- `textDocument/definition` handler.
- Tests: a click on `math::square` in `main.cplus` jumps to `math.cplus:1:8` etc.

After 4E.3 the LSP is editor-usable for the diagnostic/format/jump triad. Hover, completions, find-refs, refactorings land in Phase 5.

---

## 9. Interactions

### 9.1 Compiler-as-library (§5.1)

The LSP uses the same `cplus-core` library that `cpc build` and `cpc fmt` use. No reimplementation of parsing, sema, or formatting. Validates the architecture.

### 9.2 Determinism (§5.3)

LSP message ordering matters for editor UX. The server processes messages strictly in receive order (the `lsp-server` loop is single-threaded by default). Diagnostic batches are sorted by file path then by byte offset of the primary span — `BTreeMap` keyed appropriately. Editor renders deterministic gutter ordering across runs.

### 9.3 Structured diagnostics (§5.2)

The cplus-core `Diagnostic` shape was designed for this. Mapping is mechanical (§5.3 table). The `code` field is the same `E0xxx` string the CLI prints; users / agents Google "C+ E0345" and find the same docs.

### 9.4 Project mode vs single-file

Detection: walk up from the open file's directory looking for `Cplus.toml`. First match wins. The resolved project's binary entry needn't include the open file — the LSP runs the project pipeline either way; the open file may simply be unreachable from the entry (in which case it won't be type-checked, which is correct: it's not part of the build).

Future polish: `[[lib]]` entries in the manifest, so library files are part of the project even without a binary that imports them.

### 9.5 Performance

In-process latency budget (per `didSave`):

- Lex + parse + sema for a single-file program: target < 50 ms on a small file. Phase 1 numbers from `bench.md` suggest this is comfortably achievable for ~1k-line files.
- Lex + parse + sema across a 10-file project: target < 200 ms.

These are aspirational, not SLOs. Real numbers come from in-editor use.

---

## 10. Resolved decisions (locked in 2026-05-11)

- **Push + pull diagnostics: both.** Push on `didOpen` / `didSave`; respond to `textDocument/diagnostic` pulls. Advertise both capabilities; let the client pick.
- **Project root discovery: walk-up on `didSave`.** Walk up from the open file's directory looking for `Cplus.toml` whenever a buffer is saved; cache the result by directory. No filesystem watcher in 4E.1 — if the user creates `Cplus.toml` mid-session, the LSP picks it up on the next save. Defer the watcher to 4E polish if it bites.
- **Parse-error policy: not applicable in 4E.1.** Diagnostics fire only on `didOpen` / `didSave` and pulls; an in-flight `didChange` doesn't re-check. When per-keystroke checks land in a follow-up slice, the rule becomes: keep the last successfully-parsed AST and re-emit its diagnostics during parse-error states.
- **Multi-binary projects: union the bins.** When `Cplus.toml` lists more than one `[[bin]]`, walk each bin's import graph and merge the resulting diagnostics. If the open file isn't reachable from any bin, fall back to single-file mode. (Manifest only allows one `[[bin]]` today, so this is a forward-compatibility choice; the dispatch code is written that way from the start.)
- **Logging: stderr default + optional `--log PATH`.** Conventional pattern; some editors swallow stderr, so we provide an opt-in persistent trace.

---

## 11. Non-goals

- No incremental parsing in 4E. Full reparse per save is fine for now.
- No completions / hover / signature-help / find-refs / rename / inlay-hints / code-lens / semantic-tokens. Each gets its own design slice in Phase 5+.
- No DAP (debugger protocol). Separate concern; lands with the Phase 9 debugger story.
- No multi-root / workspace folders. One project, one root. Multi-root LSP is a complication editors handle inconsistently anyway.

---

## 12. Summary

A lean LSP server: stdio transport, `lsp-server` + `lsp-types` deps, single-threaded synchronous dispatch loop, three capabilities — diagnostics, formatting, goto-definition. Built on `cplus-core` so no reimplementation. Three sub-slices: skeleton + diagnostics (4E.1), formatting + code-actions (4E.2), goto-def (4E.3). Phase-5 polish brings the richer capabilities.

The compiler-as-library architecture (§5.1) means each slice is a thin LSP-to-library binding. Most of the work is protocol plumbing, which `lsp-server` already does.
