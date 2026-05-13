// Minimal VS Code extension for C+.
//
// Wires the editor's LSP client to the `cpc lsp` subcommand. The
// language server itself lives in the `cpc-lsp` binary (Phase 4 slice
// 4E.1 / 4E.2); this extension is just the editor-side glue —
// declaring the `cplus` language, spawning the server, forwarding
// document sync / format / code-action requests.
//
// Run in dev mode:
//   1. `cd editors/vscode && npm install`
//   2. Open this folder in VS Code, hit F5 → opens an Extension
//      Development Host window with the extension loaded.
//   3. In the dev host, open any `.cplus` file. Diagnostics light up.
//
// Package + install permanently:
//   1. `npm install -g @vscode/vsce`
//   2. `cd editors/vscode && vsce package` → produces `cplus-vscode-*.vsix`
//   3. In VS Code: Extensions panel → "..." menu → "Install from VSIX".

const { workspace, window } = require("vscode");
const {
  LanguageClient,
  TransportKind,
} = require("vscode-languageclient/node");

let client = null;

function activate(context) {
  const config = workspace.getConfiguration("cplus");
  const cpcPath = config.get("cpcPath", "cpc");

  // The server is spawned as `<cpc> lsp` — `cpc` finds `cpc-lsp`
  // next to itself or on PATH, then forwards stdio.
  const serverOptions = {
    command: cpcPath,
    args: ["lsp"],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [
      { scheme: "file", language: "cplus" },
    ],
    synchronize: {
      // Re-send the manifest when it changes so the server can refresh
      // its project-root cache on the next check.
      fileEvents: workspace.createFileSystemWatcher("**/Cplus.toml"),
    },
    outputChannel: window.createOutputChannel("C+ Language Server"),
  };

  client = new LanguageClient(
    "cplus",
    "C+ Language Server",
    serverOptions,
    clientOptions,
  );

  client.start().catch((err) => {
    window.showErrorMessage(
      `C+: failed to start language server (${cpcPath} lsp). ` +
        `Set cplus.cpcPath if the binary is not on PATH. (${err})`,
    );
  });

  context.subscriptions.push({
    dispose: () => {
      if (client) {
        return client.stop();
      }
    },
  });
}

function deactivate() {
  if (!client) return undefined;
  return client.stop();
}

module.exports = { activate, deactivate };
