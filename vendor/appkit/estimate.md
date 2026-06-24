# AppKit binding — work estimation and the auto-generation question

Companion to `gap.md`. Estimates the effort to close the gaps by hand, then
weighs that against building a binding generator.

## Baseline

The current `vendor/appkit` is 86 wrapper structs, 712 functions, ~471 bound
selectors, 4,820 lines across 21 modules. `gap.md` describes this as a practical
core, not complete coverage. For scale: AppKit is ~250 classes and several
thousand methods; Foundation is a comparable second surface. The naming
alignment (done) reshaped the existing 712 functions; the gaps below are mostly
new selector bindings, plus delegate and lifecycle subsystems.

Pace assumption for new, tested bindings: ~20-40 mechanical method wrappers per
engineer-day; delegate families and block-based callbacks are slower (a few days
each) because each needs class synthesis or a callback trampoline plus tests.

## Manual estimate (closing gap.md by hand)

| Area (gap.md section) | Scope | Est. (eng-days) |
|---|---|---|
| **P1** Delegates / data sources | 8-10 families (text, outline, collection, rich table, split/tab/browser/popover/toolbar/menu/panel); each = class synthesis + shims + typed helper + tests | 25 |
| **P1** App & window lifecycle | launch/activate/terminate delegates, first-responder, window events (resize/move/focus/fullscreen/close), sheets + modal completion, minimize/zoom/tabgroups/restoration/titlebar accessories/screen queries, NSDocumentController | 18 |
| **P1** Events & interaction | local/global monitors, responder callbacks (key/mouse/scroll/magnify/rotate/pressure), gestures/tracking-areas/cursor, full drag sessions (pasteboard items/promised files/previews/validation) | 15 |
| **P2** Foundation layer | NSURL/NSDate/NSError/NSBundle/process, NSUserDefaults/NSFileManager, dict/set/ordered-set/attributed-string/mutable-data, timers/run-loop/undo/operation-queues | 30 |
| **P2** Text system | NSTextStorage/NSLayoutManager/NSTextContainer, attributed text/ranges/attachments/find-replace/spelling/grammar/completion | 22 |
| **P2** Dialogs / panels / sharing | async sheet variants + completion blocks, accessory views, content types, NSSharingServicePicker, color/font/help panels | 10 |
| **P2** Menus / commands / toolbars | item actions + validation, contextual menus/delegates/alternate items/key-equivs, modern toolbar providers/customization/search-group, Touch Bar | 10 |
| **P3** Views & Auto Layout | frame/bounds getters, subview removal/reorder, alpha/tooltips/appearance/safe-area, bulk constraint activation/identifiers/multipliers/VFL/hugging-compression/guide anchors, constraint Result | 12 |
| **P3** Graphics & animation | NSGraphicsContext/gradients/shadows/transforms/compositing/clipping/image-reps/PDF, CALayer/transactions/animations, fuller NSBezierPath/NSImage/symbol config | 22 |
| **P3** Controls & containers | editable combo, segmented config, level indicators, date/path/search/color-well, popovers, scrubbers, symbol buttons | 10 |
| **X** Ownership audit completion | every owned wrapper drop-correct, child→parent transfers, delegate/observer lifetimes as owned values | 7 |
| **X** Accessibility / localization / appearance | roles/labels/values/actions/notifications, RTL, theme/contrast/reduced-motion observation | 12 |
| **X** Facade consistency | re-export or document layout/events/drag/convert | 1 |
| **Total** | | **~194 (range 150-240)** |

That is roughly **7-12 person-months**, and the result is still a curated
subset (~25-40% of AppKit+Foundation), permanently chasing each new SDK. Manual
binding effort is O(API surface) and never converges.

## Auto-generation: feasibility

The bindings are mechanical: a wrapper struct + per-method `objc_msgSend` shim +
type mapping. Apple ships machine-readable metadata that drives exactly this:

- **Framework headers** (parseable with libclang): class/method/property
  signatures and types.
- **Nullability** (`_Nullable` / `_Nonnull`): drives `Option` returns
  automatically — the rule we applied by hand.
- **ARC ownership** (alloc/new/copy/init families + `NS_RETURNS_RETAINED`):
  drives the +1/`drop` model we maintain by hand.
- **`NS_ENUM` / `NS_OPTIONS`**: become C+ enums / bit-mask constants.
- **`NS_SWIFT_NAME` + apinotes**: Apple has *already curated* Swift-style names
  for most of the API — most of the naming-guideline renames come for free.
- **Value structs** (NSRect/NSPoint/NSSize): map to `#[repr(C)]` structs.

Precedent: Rust's `objc2` generates all of AppKit/Foundation/UIKit from headers
via a libclang `header-translator`; Swift's own Clang importer plus overlays does
the same. Hand-writing ObjC bindings at this scale is the outlier, not the norm.

### What a generator handles well
Breadth: exhaustive raw msgSend shims + wrappers; type mapping; `Option` from
nullability; enums/masks; +1/`drop` from ARC families; Swift names from apinotes.

### What still needs hand work
- **Delegate / data-source synthesis** — the closure-free class-synthesis
  pattern is C+-specific; doable as templated codegen per protocol, but custom.
- **Blocks** (completion handlers for sheets/animations) — C+ has fn-pointers
  under a closure-free constraint, not ObjC blocks; needs a trampoline shim.
  This is the genuinely hard 20%.
- **Ergonomic top** — default values, content-taking inits, collapsing overload
  families: judgment calls, though apinotes accelerate them.
- **C+ language limits** — no generic method dispatch; generated code must avoid
  it and emit the `str`/`Text` bridge calls explicitly.

### Effort to build the generator
A libclang-based header parser → C+ emitter, MVP covering Foundation + AppKit
core (raw layer + Option + ownership + enums + value structs): **~30-40
eng-days**. Templated delegate synthesis adds ~10. Blocks/trampolines can be
deferred. After that, regenerating for new classes or SDKs is near-free.

## Recommendation: two-tier, generator-backed

1. **Generated raw layer.** Build the libclang→C+ generator. It emits the
   exhaustive ObjC-shaped layer for Foundation + AppKit, with `Option`,
   ownership, enums, and Swift names derived from metadata.
2. **Hand-curated swifty overlay.** Keep the naming-guideline layer thin, over
   the high-traffic ~20% apps actually use, built on top of the generated layer.
   This is where defaults, content-taking inits, and `Status`/`Result` choices
   live.
3. **Templated delegate synthesis** in the generator; **defer blocks** (or add a
   trampoline) until a real consumer needs completion handlers.

Net: this turns gap.md from "~150-240 days of manual work that never finishes"
into "~40-50 days to a generator + amortized curation," and the coverage is
broad instead of a perpetual subset.

### Cross-platform multiplier
The same approach pays off beyond AppKit. GTK/Adwaita ship **GObject-
Introspection `.gir`** files — fully machine-readable, the canonical source that
gtk-rs and PyGObject already generate from. Win32 is C/COM (different mechanism);
Android is JNI. A binding-generator investment is therefore strategic for the
whole `vendor/` surface (and for `facet`, which sits on top of these bindings),
not a one-platform tactic.

### Suggested sequencing
- Hand-build the 2-3 highest-value delegate families now (table, text-edit,
  window) — any real app needs them, and they define the delegate-template shape
  the generator will reuse.
- Then build the generator MVP (Foundation + AppKit raw layer) rather than
  continuing the manual gap.md treadmill.
- Curate the swifty overlay on demand, driven by what real apps/`facet` need.

### Risks / unknowns
- Blocks + the closure-free constraint are the hard part; scope the MVP to
  defer them.
- libclang must parse the macOS SDK headers cleanly (objc2 proves this is
  workable, but it is real integration effort).
- The generator is compiler-adjacent tooling; it needs an owner and tests of its
  own, like any codegen.
