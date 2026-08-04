# facet_appkit — the manifest

INTENT.md's rule: for each thing facet declares, this package either implements
it with what AppKit offers, or **states plainly that AppKit cannot**. This file
is where every "cannot" is written down, so anyone can answer "how does that
happen on a Mac?" by reading it.

A row here is a commitment, not a note. Nothing is left implicit: a verb that
is neither implemented nor listed below is a gap, and the gap is a bug.

Status: **Stage 4 item 1 complete**, item 2 begun (five kinds of 42). Items are in
progress; this file grows a row each time something is decided either way.

---

## AppKit cannot

Nothing is recorded here yet. The rows Stage 4 expects to land in this section
are the mobile-only handful the tier ledger deferred — soft-input policy and
the nav-bar back button — which have no desktop equivalent and will be recorded
as "AppKit cannot (mobile concept)" when item 5 reaches them.

The doctrine is the same one an UNSUPPORTED control verb follows: the contract
still declares it, because the contract is readable as a whole and a backend's
gaps are not the vocabulary's business.

## Implemented, with a deviation worth knowing

### Every backing view is flipped

facet's tree is top-left origin, because flex is. AppKit's NSView is not, by
default. Rather than carry a flip through the frame walk — the pre-regen
backend threaded a `parent_flipped` flag through every level and computed
`parent.height - (child.top - parent.top) - child.height` for the bottom-up
case — every view this backend creates has `isFlipped` true, and the window's
content view is flipped too.

Consequence for an application that drops beneath facet: a native view you
add yourself lands in a **flipped** superview, so its frame is read top-left.
That is the opposite of a hand-built AppKit window and it is deliberate.

### `blur` resigns first responder to the window

There is no null first responder on AppKit: passing nil makes the window
refuse the change rather than clear it. `blur` therefore makes the WINDOW the
first responder, which is what "nothing in the tree is focused" means on this
platform.

### The sync tick is a CFRunLoopObserver at before-waiting

facet has no loop of its own; M5 says the backend coalesces sync requests on
its run loop. This uses `kCFRunLoopBeforeWaiting` — the moment the loop has
finished everything it had and is about to sleep, which is the same flush
point Core Animation commits on. A hundred writes in one event cost one tick.

### A background needs a layer

`background_color` on a plain view has no AppKit equivalent without a backing
layer, so setting one turns `wantsLayer` on for that view. A node that sets no
background stays layer-free, so a tree that themes nothing pays for no layers.

## Not yet reached

These are unimplemented because their stage item has not landed, NOT because
AppKit cannot. They are listed so the difference is never ambiguous.

| What | Stage 4 item |
|---|---|
| The 42 per-kind `create`/`apply` bodies | 2 |
| `KeyReader` — key code / chars / modifiers / named | 4 |
| The gesture band and the decline chain | 4 |
| `SenderReaders` — raise / key_of / item_of / drop | 4 |
| The app menu, toolbar tier, titlebar content | 5 |
| Window sizing policy, density, modal stack | 5 |
| The agent surface | 6 |

Until item 2 lands, a control kind with no body renders as an empty backing
view **and says so on stderr**, once per kind. A silent wrong view is the
failure this package exists to avoid.
