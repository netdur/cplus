# Tutorial

Getting a facet application onto the screen on macOS. How the backend works:
[guide.md](guide.md). What it fills and what it deviates on:
[ref.md](ref.md). Every verb's disposition: [../MANIFEST.md](../MANIFEST.md).

This package draws; it declares nothing. The vocabulary is `vendor/facet`, and
its own tutorial is the place to learn the tree.

## 1. Declare it

```toml
[dependencies]
stdlib      = "*"
facet       = "*"
flex_layout = "*"

[macos.dependencies]
facet_appkit = "*"
objc         = "*"
appkit       = "*"
quartzcore   = "*"
webkit       = "*"
```

The four alongside are what the backend binds against: the Objective-C
runtime, AppKit, Core Animation, and WebKit for `web` / `hybrid_web`.

Under `[macos.dependencies]` so the same package builds on a target with no
backend. There, facet's verbs become no-ops that say so once on stderr.

## 2. Install it

One call, before anything else:

```cplus
import "facet_appkit/facet_appkit" as backend;

fn main() -> i32 {
    backend::install();
    ...
}
```

`install` fills the five seam structs facet declares, sets the two theme
slots, arms the sync tick, and installs the async pump. It is idempotent: a
facade doing belt-and-braces registration will not double-arm anything.

Nothing else registers. If a hook is not in one of those structs, it is not
part of the seam.

## 3. Run a screen

The rest is facet's API, unchanged:

```cplus
var app: runtime::App = runtime::App::new("hello");
app.screen("main", main_screen);
match app.run("main") {
    status::Status::Ok => { return 0 as i32; }
    _other => { return 1 as i32; }
}
```

`App::run` opens the window, mounts the tree, and runs AppKit's loop.

## 4. Watch it update

A write marks a node dirty and asks for a sync. The backend coalesces those
onto a CFRunLoopObserver at before-waiting: the moment the loop has finished
everything it had and is about to sleep, which is the same point Core
Animation commits on.

So a hundred writes in one event cost one visual update, and a batch lands
together rather than tearing.

## 5. Add the agent surface

Useful in development, and the only way to drive the app without a person at
the keyboard:

```toml
[dependencies]
agent_core = "*"
agent_mcp  = "*"
json       = "*"

[macos.dependencies]
agent_appkit = "*"
```

```cplus
import "facet/agent" as agent;

backend::install();
agent::enable();

var app: runtime::App = runtime::App::new("hello");
app.agent_mcp("/tmp/hello.sock");
```

Then, with the app running:

```
printf '{"method":"describe_ui","params":{},"id":1}\n' | nc -U /tmp/hello.sock
```

`describe_ui` answers the live tree addressed by key. `click` drives a control
through the same path a mouse takes. `mode: "full"` describes the native view
tree instead, which is what to reach for when a control is on screen but wrong.

## 6. Window chrome

`Chrome` is read once when the window opens:

```cplus
screen::Chrome::new(title: "hello", width: 460.0f64, height: 420.0f64,
                    min_width: 320.0f64,
                    bar: screen::Bar::Blended,
                    zoomable: true, max_zoom: 4.0f64)
```

`bar` picks the titlebar shape. `Custom` hides the standard buttons so
`window_buttons()` can supply its own; pair it with `.window_drag()` on the
surface that should move the window, or the bar will not drag.

`zoomable` turns on pinch-to-zoom of the content: the picture scales as laid
out, without reflow, and a two-finger scroll pans while zoomed.

## 7. When something does not appear

In order of how often it is the answer:

The node has no size. A control sizes itself from its content; a container
sizes from its children. A `scroll` or a `canvas` is sized by its parent, so
`column { scroll() }` is zero-height until the scroll is given `.grow(1)`.

The node is out of flow. `display: none` removes it from layout. The non-view
kinds (`span`, `menu_item`, `context_menu`, `swipe_item`) are set that way on
purpose: they are read as nodes when their parent's view is built.

The key is not unique. Bare `find` takes the first match in DOM order across
every open window. Pass `within:` to scope it.

The write never reached the backend. `describe_ui` with `mode: "full"` shows
the native tree; if the value is right in facet and wrong on screen, the verb's
row in `MANIFEST.md` says what the backend does with it.

## 8. Where to go next

`guide.md` explains how the backend answers the contract: the five verbs, the
view/no-view rule, how input is delivered without gesture recognizers, and the
recycling seam.

`MANIFEST.md` is the per-verb record. Every verb is live, host-rendered,
derived, a modifier, create-only, or recorded as something AppKit cannot do,
and `python3 tools/verb_coverage.py --check` fails if any verb is in none of
those.
