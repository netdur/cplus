# C+ for VS Code

Minimal VS Code extension that wires the editor's LSP client to the
`cpc lsp` subcommand. Lights up:

- Live diagnostics on every error code C+ knows about (E0001–E0410)
- Format-on-save / `Format Document` via `cpc fmt`
- Quick Fixes for diagnostics that carry machine-applicable suggestions

The language server lives in the `cpc-lsp` binary — this extension is
just the editor-side glue.

## Run in dev mode (fastest path to squiggles)

1. Make sure `cpc` and `cpc-lsp` are built. From the repo root:

   ```
   cargo build --release
   export PATH="$(pwd)/target/release:$PATH"
   ```

   (Or use `target/debug/` if you prefer.)

2. Install the extension's runtime deps:

   ```
   cd editors/vscode
   npm install
   ```

3. Open `editors/vscode/` as a folder in VS Code, then press **F5**.
   That opens an *Extension Development Host* window with the
   extension loaded.

4. In the dev host, open any `.cplus` file from `docs/examples/`.
   Diagnostics appear in the gutter; saving re-checks.

## Install permanently (so you don't need to keep the dev host open)

1. Install the VS Code extension packager:

   ```
   npm install -g @vscode/vsce
   ```

2. Build a `.vsix`:

   ```
   cd editors/vscode
   vsce package
   ```

3. In VS Code, **Extensions** panel → `...` menu → **Install from VSIX...**.

## Settings

- `cplus.cpcPath` — path to `cpc`. Default: `cpc` (uses PATH). Set this
  to an absolute path if `cpc` isn't on PATH or you want to pin a
  specific build.
- `cplus.trace.server` — `"off"` (default), `"messages"`, or
  `"verbose"`. Logs the LSP JSON-RPC traffic to the "C+ Language
  Server" output channel; useful when reporting bugs.

## What works in this slice

Phase 4 slices 4E.1 and 4E.2:

- Diagnostics push on `didOpen` and `didSave` (not per-keystroke — the
  AI-iteration loop fits save-triggered better, and Phase 4 has no
  incremental parser yet).
- `textDocument/formatting` wraps `cpc fmt`.
- `textDocument/codeAction` lifts every diagnostic's
  `(span, replacement)` suggestion into a Quick Fix; the `Preferred`
  flag is set for `MachineApplicable` suggestions so they bind to the
  default keystroke (`⌘.` / `Ctrl+.`).

Goto-definition lands in slice 4E.3. Completions / hover /
find-references / rename / inlay-hints land in Phase 5.

## Status

Slice 4E.2. Test coverage in [`cpc-lsp/tests/e2e.rs`](../../cpc-lsp/tests/e2e.rs)
drives the binary via framed JSON-RPC and asserts the responses; the
VS Code side is verified by hand for now.
