# facet_agent — a declarative facet UI an agent can see and drive

The facet counterpart of the `appkit_agent` recipe. An MVU counter app is
built with the `@facet` DSL; the widgets an agent may use carry an
`.agent_id("...")` right in the description:

```cplus
fn view() -> facet::Node {
    let t: text::Text = "count ${COUNT}";
    return @facet {
        vstack {
            label(t.view(), size: 20.0f64).agent_id("count-label")
            button("+1").on_click(on_inc).agent_id("inc-btn")
        }
    };
}
```

## Why the tag lives in the description

facet's app runtime re-renders after every event handler by replacing the
widget views wholesale. A tag pinned on a view (the imperative
`set_agent_id` channel) would die with the replaced view. A tag in the
description is re-applied by the backend at every mount — facet_appkit writes
it to the created view's `accessibilityIdentifier` — so the agent surface
stays addressable across re-renders by construction.

The agent side (`agent_appkit`) honors both channels when it walks a window:
an explicitly pinned id wins, otherwise a non-empty accessibilityIdentifier
counts as the tag.

## Staleness, not use-after-free

A `Surface` snapshot is tied to the views that existed at `open` time. The
surface retains them, so after a re-render the old surface is safe but
useless — acting through it reports `Stale`, the agent's cue to re-open:

```text
--- describe: 5 nodes ---
  app/window#0 text='facet agent demo' actionable=false
  app/window#0/group#0 text='' actionable=false
  app/window#0/group#0/group#0 text='' actionable=false
  count-label text='count 0' actionable=true
  inc-btn text='+1' actionable=true
--- click inc-btn -> Allowed, COUNT=1 ---
--- click again through the OLD surface -> Stale, COUNT=1 ---
--- re-open, click -> Allowed, COUNT=2 ---
```

## Build + run

The recipe relies on its dependencies being symlinked into `vendor/` (the
same model as every other recipe):

```bash
mkdir -p vendor
for p in stdlib objc quartzcore appkit flex_layout facet facet_appkit agent_core agent_appkit; do
  ln -s "$(git rev-parse --show-toplevel)/vendor/$p" "vendor/$p"
done
cpc build
./target/debug/facet_agent
```

## From here to a real app

Swap the print statements for `fak::run_app(view)` and serve the surface on a
background connection (`agent_mcp::serve_uds`) — see the `appkit_agent`
recipe for the JSON-RPC + consent-gate side, which is identical here.
