# agent_gtk

The **GTK 4 backend for the agent surface** — the Linux/BSD counterpart to
`vendor/agent_appkit` (AppKit) and `vendor/agent_win32` (Win32). It lets an agent
*perceive and drive* a live native GTK GUI without screenshots, by walking the
`GtkWidget` hierarchy into the framework-neutral [`agent_core`](../agent_core)
identity model and gating every action through `agent_core`'s authorization
brain.

```
agent_core   (headless: identity / role / surface authorization / events)
    ▲
    │  reused unchanged, platform-neutral
    │
agent_gtk  ──walks──▶  a live GtkWidget tree (GTK 4 / GObject)
```

`agent_core` is shared with the AppKit and Win32 backends; only this thin bridge
is GTK-specific. Introspection is plain GTK/GObject `extern fn` calls
(`gtk_widget_get_first_child` / `g_type_name_from_instance` /
`gtk_editable_get_text` / …) plus `gtk/convert` for the `str`↔`char*` bridges, so
it can describe any widget tree built with [`vendor/gtk`](../gtk).

## Usage

```toml
[dependencies]
agent_gtk  = "*"
agent_core = "*"
```

```cplus
import "agent_gtk/agent_gtk" as agent;
import "agent_core/surface" as surface;

// 1. Curate the surface: tag the widgets the agent may see/act on. The id is a
//    stable NUL-terminated string literal.
agent::set_agent_id(button.raw(), #str_ptr("btn_login\0"));
agent::set_agent_id(entry.raw(),  #str_ptr("user_field\0"));

// 2. Snapshot the window (the READ path).
let surf: agent::Surface = agent::open(window.raw());
let nodes: vec::Vec[agent::UiNode] = surf.describe();
//   each UiNode = { id, role, class_name, frame, is_hidden, text, actionable, parent }

// 3. Act (the WRITE path) — each call is authorized by agent_core first.
let _ = surf.click("btn_login");                       // -> surface::Outcome
let v  = surf.text_version("user_field");
let _ = surf.set_text("user_field", "alice", v);       // optimistic concurrency
```

## What it does

- **Read — `describe()` → `Vec[UiNode]`.** A DFS over the GTK 4 child chain
  (`gtk_widget_get_first_child` / `gtk_widget_get_next_sibling`) classifies each
  widget by its GObject type into the curated `agent_core::Role` (Button / Input /
  Text / List / Group / Window), and reads its live frame
  (`gtk_widget_compute_bounds`), enabled (`gtk_widget_get_sensitive`), visibility
  (`gtk_widget_get_visible`) and text (`gtk_label_get_text` /
  `gtk_editable_get_text` / `gtk_button_get_label` / `gtk_window_get_title`). The
  flat list with `parent` indices reconstructs the tree.
- **Write — `click` / `set_text` / `focus`.** Each resolves the agent-id to a
  node and asks `agent_core::surface` first: `authorize_action` (click),
  `authorize_text_write` with a version stamp (set_text, optimistic
  concurrency), `authorize_read` (focus). Only on `Allowed` does the real I/O run
  — `gtk_widget_activate`, `gtk_editable_set_text`, `gtk_widget_grab_focus`. The
  result is an `Outcome` (`Allowed` / `NotFound` / `NotExposed` / `NotActionable`
  / `VersionConflict`).
- **Events — `emit`.** Translates a fired widget (app-installed GObject signal
  handler) into an `agent_core` event offered to a `Subscriber`.

## Exposure

Only widgets tagged with `set_agent_id` are **exposed** — part of the curated
surface the agent may act on. Untagged widgets are still walked (for tree
completeness) but are `NotExposed`, so actions on them are refused. The id is
held as GObject data (`g_object_set_data`, which does not copy), so pass a stable
string literal.

## Notes vs. the AppKit / Win32 backends

- **Subclass-aware classification.** Roles are decided with
  `g_type_check_instance_is_a`, which is ancestry-aware (a `GtkToggleButton`
  answers a `GtkButton` query, any `GtkEditable` is an Input), the GTK analogue of
  AppKit's `isKindOfClass:`.
- **No main-thread marshaling helper.** Like the Win32 backend (and unlike
  AppKit, which needs `performSelectorOnMainThread`), there is no thread hop here:
  GTK is single-threaded by contract, so an app that drives the surface off the
  GTK main thread must marshal itself — the same rule all GTK code lives under.
- **Float frames.** GTK lays out in floats (graphene), so `Rect` carries `f32`
  fields (faithful, no lossy cast), unlike Win32's integer `Rect`.
- **`agent_mcp`** currently targets the AppKit backend; serving this surface over
  MCP on Linux is a natural follow-up (a backend-neutral `agent_mcp`).
