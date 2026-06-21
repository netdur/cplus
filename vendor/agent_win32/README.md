# agent_win32

The **Win32 backend for the agent surface** — the Windows counterpart to
`vendor/agent_appkit` (AppKit). It lets an agent *perceive and drive* a live
native Windows GUI without screenshots, by walking the HWND hierarchy into the
framework-neutral [`agent_core`](../agent_core) identity model and gating every
action through `agent_core`'s authorization brain.

```
agent_core   (headless: identity / role / surface authorization / events)
    ▲
    │  reused unchanged, platform-neutral
    │
agent_win32  ──walks──▶  a live HWND tree (user32)
```

`agent_core` is shared with the AppKit backend; only this thin bridge is
Windows-specific. Introspection is plain `user32` `extern fn` calls
(`GetWindow` / `GetClassNameA` / `GetWindowTextA` / …), so **no GUI-toolkit
package is required** — it can describe any HWND tree, including one built with
[`vendor/win32`](../win32).

## Usage

```toml
[dependencies]
agent_win32 = "*"
agent_core  = "*"
```

```cplus
import "agent_win32/agent_win32" as agent;
import "agent_core/surface" as surface;

// 1. Curate the surface: tag the controls the agent may see/act on. The id is a
//    stable NUL-terminated string literal.
agent::set_agent_id(button_hwnd, #str_ptr("btn_login\0"));
agent::set_agent_id(field_hwnd,  #str_ptr("user_field\0"));

// 2. Snapshot the window (the READ path).
let surf: agent::Surface = agent::open(window_hwnd);
let nodes: vec::Vec[agent::UiNode] = surf.describe();
//   each UiNode = { id, role, class_name, frame, is_hidden, text, actionable, parent }

// 3. Act (the WRITE path) — each call is authorized by agent_core first.
let _ = surf.click("btn_login");                       // -> surface::Outcome
let v  = surf.text_version("user_field");
let _ = surf.set_text("user_field", "alice", v);       // optimistic concurrency
```

## What it does

- **Read — `describe()` → `Vec[UiNode]`.** A DFS over `GetWindow(GW_CHILD /
  GW_HWNDNEXT)` classifies each window by class name (+ style bits) into the
  curated `agent_core::Role` (Button / Input / Text / List / Group / Window),
  and reads its live frame (`GetWindowRect`), enabled (`IsWindowEnabled`),
  visibility (`IsWindowVisible`) and caption (`GetWindowTextA`). The flat list
  with `parent` indices reconstructs the tree.
- **Write — `click` / `set_text` / `focus`.** Each resolves the agent-id to a
  node and asks `agent_core::surface` first: `authorize_action` (click),
  `authorize_text_write` with a version stamp (set_text, optimistic
  concurrency), `authorize_read` (focus). Only on `Allowed` does the real I/O
  run — `SendMessage(BM_CLICK)`, `SetWindowTextA`, `SetFocus`. The result is an
  `Outcome` (`Allowed` / `NotFound` / `NotExposed` / `NotActionable` /
  `VersionConflict`).
- **Events — `emit`.** Translates a fired control (app-installed callback) into
  an `agent_core` event offered to a `Subscriber`.

## Exposure

Only windows tagged with `set_agent_id` are **exposed** — part of the curated
surface the agent may act on. Untagged windows are still walked (for tree
completeness) but are `NotExposed`, so actions on them are refused. The id is
held as a window property (`SetPropA`), so pass a stable string literal.

## Notes vs. the AppKit backend

- **No main-thread marshaling helper.** AppKit needs `performSelectorOnMainThread`
  to mutate UI off-thread; on Win32 a cross-thread `SendMessage` is delivered on
  the window's owning thread by the OS, so the gated actions are direct sends.
- **Flatter tree + style-bit classification.** Win32 controls are direct
  children of the window, and the single `Button` class covers push/checkbox/
  radio (→ Button) and group boxes (→ Group), split by the window style.
- **`agent_mcp`** currently targets the AppKit backend; serving this surface
  over MCP on Windows is a natural follow-up (a backend-neutral `agent_mcp`).

Validated by `cpc/tests/e2e.rs::agent_win32_describe_and_gated_actions`.
