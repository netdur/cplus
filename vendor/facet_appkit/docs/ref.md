# Reference

What this package exposes, and what it does with each part of facet's
contract. How it works: [guide.md](guide.md). Fast start:
[tutorial.md](tutorial.md). Per-verb dispositions:
[../MANIFEST.md](../MANIFEST.md).

Most of this package is not called by an application. It is called by facet,
through the seam. An application calls `install` and then writes facet.

## facet_appkit/facet_appkit

`install()` fills the seam and arms the loop. Idempotent.

It fills `Renderer`, `Scheduler`, `KeyReader`, `SenderReaders` and the two
theme slots, installs the sync tick, and installs the async pump that lets
`spawn_ui` tasks resume.

`installed() -> bool`, `uninstall()` for tests.

`theme_changed()` repaints every open window; facet calls it when `set_theme`
runs.

The agent surface is NOT installed here. An application opts in with
`facet::agent::enable()` and names a socket with `App::agent_mcp`.

## The seam this package fills

| Struct | Filled by |
|---|---|
| `Renderer` | `views::create` `apply` `insert` `remove` `view_release`, `scheduler::schedule` |
| `Scheduler` | `scheduler::run_on_main` `after` `cancel_after` `observe_size` `cancel_observe_frame` |
| `KeyReader` | `input::key_reader()` |
| `SenderReaders` | `input::sender_readers()` |
| `AgentHooks` | `facet::agent::enable()`, not `install` |
| theme slots | `window::is_dark`, `theme_changed` |

## Modules

Internal, listed so a reader knows where a behaviour lives.

| | |
|---|---|
| `views` | the five Renderer verbs, the view/no-view rule, intrinsic size |
| `controls` | per-kind create and apply for all 42 kinds |
| `paint` | colour, font, and the shared band |
| `geometry` | the frame walk, z ordering |
| `scheduler` | the sync tick, timers, the async pump |
| `input` | the armed class, gestures, drag and drop |
| `text_input` | the three text controls and their write-back |
| `window` | Chrome to NSWindow, the menu bar, titlebar accessories |
| `recycler` | the recycling list and outline |
| `compose` | controls built from more than one native thing |
| `drawing` | the canvas display-list replay |
| `zoom` | pinch zoom and pan-while-zoomed |
| `dialogs` | the alert sheet and the file pickers |
| `scrolling` | scroll observation and the nested-axis rule |
| `web` | WKWebView for `web` and `hybrid_web` |

## Coverage

```
python3 tools/verb_coverage.py          # the summary
python3 tools/verb_coverage.py --list   # every verb, by bucket
python3 tools/verb_coverage.py --check  # the gate
```

The gate fails on a verb that is neither implemented nor recorded, on a verb
whose bit is named in a mask but whose field is never read, on a dead handler,
and on a ledger row naming a verb that does not exist.

Two limits are documented in the tool and worth knowing:

It asks whether SOME body reaching a control's struct reads the field, not
whether the body that gates the bit does. A reader that exists but is never
called still counts.

Its `NOT_CONTROLS` list exempts `gestures`, `theme`, `screen`, `application`,
`nav`, `services` and `component`, and the shared `C_*` bits with them. Those
were audited by hand; nothing yet stops them drifting.

## Create-only verbs

Read when the object is built; a later write does not reach the screen. Each
is here because the object that would have to be rebuilt is not the control's
own view.

| | |
|---|---|
| `list.row_height` `tree.row_height` | read when the data source is built |
| `toolbar_item.placement` `priority` `is_destructive` | an NSToolbarItem is built once, with the window |
| `menu.priority` | an NSMenu is built once, with the window |
| `window_chrome.spacing` | the traffic lights are laid out once, by the window |

`text_field.is_secure` is NOT one of these. NSSecureTextField is a different
class, and the flip builds the other one and puts it in the slot the old one
held, carrying focus and every prop across. Anything holding the raw view
across that write is holding a dead one.

## Host-rendered kinds

No view of their own; their host re-applies them.

`span` is a run inside its label's attributed string. `menu` / `menu_item` /
`context_menu` / `context_menu_item` are NSMenu rows. `toolbar_item` is an
NSToolbarItem. `swipe_item` is a row in the menu its swipeable builds.

They are `display: none` so they take no space in the layout, and still in the
tree so their host can read them.

## Decided

The `cannot-ledger` block in `MANIFEST.md` is empty: every declared CONTROL
verb has an implementation.

Two gesture parameters are decided and recorded in prose rather than that
block, because the block is machine-checked against control verbs and these
are on `gestures`, which the tool does not census:

`touch_points` asks for a finger count. `magnifyWithEvent:` does not carry one,
and the API that does is answered by a trackpad and not a mouse.

`swipe_threshold` asks for a distance. `swipeWithEvent:` delivers a discrete
±1 per axis, already recognised and quantised.

## Platform notes

Every backing view is flipped, so all geometry is top-left.

A background needs a layer, so `background_color`, `background`,
`background_image`, `shadow` and `clip` set `wantsLayer`. A node that names
none of them does not pay for one.

`blur` resigns first responder to the WINDOW. AppKit has no null first
responder, and passing nil makes the window refuse the change rather than
clear it.

Focus is reported from two places: the armed class for a plain view, and the
text delegate for the three text controls, because focusing an NSTextField
makes the window's field editor the first responder rather than the field.
Both are edge-triggered, so one user-visible focus change calls the
application once.

`z_index` orders siblings in the frame walk. AppKit paints subviews in array
order, so ordering is the implementation.

Pinch zoom is a BOUNDS change on the content view. A view's bounds are its own
coordinate space, so shrinking them magnifies everything drawn in them with no
layout pass and no view touched but the host.

## Testing

```
cd vendor/facet_appkit && cpc test
```

The suite drives real AppKit objects: it opens windows, makes views first
responder, synthesises events, and reads back what the platform did. A test
that asserts facet's own state without asking the platform is not testing this
package.

Feel cannot be asserted. Drag, pinch and scroll behaviour end with running an
example and looking, which is why `examples/hello_appkit` carries an agent
surface as well as a window.
