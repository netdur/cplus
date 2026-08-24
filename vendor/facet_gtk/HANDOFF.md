# facet_gtk — handoff

Rewritten 2026-08-24, the third time. The first version was written at 8% with
no handlers; the second at 48% with the read half done and the gallery still
out of reach. `git log --oneline vendor/facet_gtk/` has what landed; this file
is the part that is not in the diff.

**Where it stands: the gallery runs, and it has already earned its keep.**
`examples/facet_gallery` — the same application macOS runs, unchanged — builds
and comes up on Linux through `facet_runtime/runtime_linux.cplus` over this
backend. That was the target the first two handoffs both named and neither
reached.

It has found five bugs so far, which is the argument for it:

1. **Nothing was mounted.** The runtime facade opens every screen through
   `window::open_window`, and mine did not call `mount::mount` — AppKit's does.
   Every node had a frame, a position and no WIDGET. The window came up the
   right size with the layout correct and completely empty.
2. **`split` had no body**, so the gallery's own frame — a sidebar and a main
   pane — was two equal flex children whose content then overflowed the window
   by 564 points. `position: 240` was ignored entirely.
3. **`tree` had no body**, so the catalog sidebar was empty and the gallery
   could not be navigated at all: one demo, and no way to reach another.
4. **A scroll host was sized by its content.** `flex_layout`'s default shrink
   is 0, not CSS's 1, so the catalog laid out at 1148 points inside a 619-point
   pane and ran off the bottom with a scrollbar that had nothing to scroll.
5. **`max_lines: 0` reached GTK as a limit of no lines.** facet says 0 for "no
   limit"; GTK says −1. Six `Gtk-WARNING` lines a frame, and a label measuring
   a natural width smaller than its own minimum.
6. **`flex-wrap` will not make a grid.** Six 100-point items in a 300-point
   container came out two to a line when asked for three, so `collection`'s
   columns are explicit line nodes instead.

```
347 / 360 prop bits    96%     (appkit 318/88%, uikit 311/86%)
```

Both halves exist. Every kind facet declares now has a body, every non-view kind
is read by whatever owns it, the small named debts are paid, and **every
scrolling kind recycles** — `list`, `collection` and `tree` are all one
GtkListView with two slot maps over it. A 20000-row list opens by building 205
rows instead of 20000, and the number does not move when the model grows.
Nothing in this package is materialised any more. What is left in
`MANIFEST.md` §2 is a handful of rows that are one paragraph each.

---

## 1. Read these first, in this order

1. `vendor/facet_appkit/MANIFEST.md` — **the doctrine**, and the only thing
   here that is not negotiable. For each verb facet declares, a backend either
   implements it or **states plainly that the platform cannot**. AppKit's
   cannot-ledger is EMPTY at 324/324.
2. `vendor/facet_uikit/MANIFEST.md` §"TWO NUMBERS" — props are WRITES,
   handlers are READS. This package's MANIFEST states both, because only one
   of them has a tool.
3. This package's `MANIFEST.md`, then `python3 tools/parity.py`.

The bar that matters most, from appkit's manifest: **a control does not have to
be one native widget.** Sixty-odd rows left its cannot-ledger once the answer
was allowed to be built from two classes instead of one. Here, `button` is a
GtkButton over a GtkBox over a GtkImage and a GtkLabel, `page_dots` is a GtkBox
of styled labels, and a window's subtitle is two labels in a GtkHeaderBar —
each of those was a candidate "cannot" row that turned out not to be one. This
package's ledger has eleven rows and every one names what was looked for —
including four that are narrowings rather than absences.

## 2. The thing I did not do, and it is the next thing

**The RECYCLER LANDED, and it took the tree with it.** `list`, `collection` and
`tree` are all a GtkListView over a model that is a LENGTH — a GtkStringList of
N empty strings, because facet's data is behind `row(i)` and this package never
sees it — with a GtkSignalListItemFactory building a row's subtree through
`mount::realise` as it scrolls in.

TWO SLOT MAPS, ONE MACHINE. That division is the thing to keep. A sequence's map
is arithmetic over a count (with groups and grid lines folded in); a tree's is a
cached pre-order walk of the expanded model. `tree.cplus` says why they cannot
be one protocol — "a list is count plus index and a tree is parent → children
plus expansion plus stable identity". Everything BELOW the maps is shared: the
factory, the deferred fill, the per-row layout, the pooling rules. Every one of
those is a bug this package has already paid for once, and a second copy is
where they would come back.

Both maps are PURE, which is why the migration was safe: every property the two
materialisers were tested for is a question about a map, answerable with no
widget and no display, and the tests moved across rather than being dropped.

`collection`'s four recycler-shaped verbs landed with it — `item_sizing`,
`scroll_anchor`, `remaining_threshold` and drag-to-reorder were all "named but
inert" rows in §2 and are now answered, because each of them is a question about
rows that do not exist yet and there was nowhere to ask it before.

Those four went, and so did the four small rows beside them: menu accelerators
are BOUND now (a GtkShortcutController per window) rather than only drawn,
`is_enabled` and `title_of` are re-asked once per sync, a drag carries a live
paintable of the node being dragged, and a stated `scroll.content_size` wins
over the measured one.

The fine-grained verbs went too: `invalidate_rows` and `insert_rows` /
`remove_rows` are now ONE `items-changed` over the range they name, so appending
three rows to a 5000-row list rebinds NOTHING — measured, 206 rows built before
the append and 206 after — instead of replacing the model and rebuilding every
visible cell.

**§2 is down to one item**, and it is the measured number below.

**The one thing left, and it is a measured number.** GtkListView keeps 205 cells for a
viewport that holds seven. It does not grow with the model — 500, 5000 and 20000
rows all give 205 — and it did not move for the viewport height, a row-height
estimate at setup, or dropping the per-cell width request. What DID move it was
getting the model fill out of the size-allocate. Someone who knows
`gtk_list_item_manager` could probably halve the working set in an afternoon;
guessing at it from outside did not.

---

**SINCE THAT WAS WRITTEN** (2026-08-24), the read half closed and four more
ledgered gaps did:

  * `on_web_resource_requested` and `on_raw_message_received` — the last two
    unfired handlers. **68 of 68 now**, level with facet_appkit. The message
    channel needs a SECOND library (`jsc_value_to_string` is JavaScriptCore's,
    not WebKit's) and the two engine generations hand the signal different
    things; §3 has the details.
  * the canvas draws its IMAGES and casts its SHADOWS. cairo has no blur, so
    there is one now — three box passes over a scratch surface the path is
    copied into. Both were "recorded commands that draw nothing".
  * `observe_size` is answered from the LAYOUT WALK rather than from a GTK
    signal that does not exist, and `after` holds more than one timer.
  * RTL: the widget direction, and the logical-to-physical corner swap.
  * the two background verbs nobody had answered — the brush and the image.

And one BUG that had been there the whole time and that the suite was
protecting: `css_rgba` wrote an opaque alpha as `0.1000`, which GTK parses as
one tenth. Every explicitly-coloured thing this package drew was at a tenth of
its opacity. Three tests asserted the broken string. **If a colour ever looks
wrong here, print what GTK actually parsed** — `gdk_rgba_parse` in a five-line
python-gi script settled this in a minute after the string had looked right to
two readings.

What is left in §2 is now mostly DECIDED rather than undone — design limits
(an observation is per-window, a reorder is a drop on a row), perf that nothing
has needed (the tree's flat walk, `relayout_all`'s missing prune), and the one
measured number above.

## 3. What the gallery is now for

It is the conformance target, it is running, and it is the queue: open a demo,
see what is wrong, fix that. That is better than any list in this file, and it
is why the previous two handoffs both put it above breadth.

```bash
cd examples/facet_gallery && ../../target/release/cpc run
FACET_GTK_FRAMES=1 ./target/debug/facet_gallery 2>&1 | head -40
```

**Use the second line.** One node per line — key, kind, the frame flex
computed, and whether it has a widget — and it is the only diagnostic that
works when the window is up and wrong. `w=0` on a kind with a body is the
zero-height-strip bug; `view=-` on a backed kind is a node this package never
gave a widget to. Both of this round's bugs were a glance at that output.

Two things it will show and neither is a mystery:

- **A toolbar has no frame.** Its items are read out of the tree when the window
  opens and packed into the header bar, so they trace as `0,0 0x0 view=-` — the
  same shape a `menu_item` traces as, and correct. A blank strip in the column
  where they were declared would be the bug.

## 4. How to check it

```bash
cargo build --release                                  # then ALWAYS ./target/release/cpc
cd vendor/facet_gtk && ../../target/release/cpc test   # 188 tests, no display needed
python3 vendor/facet_gtk/tools/parity.py --check       # from the repo root
cd examples/facet_gtk_probe && ../../target/release/cpc run
cd examples/facet_gallery   && ../../target/release/cpc run
```

`facet_runtime`'s own suite is part of the gate now and was not before: its
`test_main` imports `./runtime`, so the platform override compiles in this
package's build — which means **a facet_gtk that does not build turns
facet_runtime red on Linux**, exactly as it turns it red on a Mac.

`examples/facet_gtk_probe` is the hands test: one window, one control of every
kind that has a body, and a status label that **every handler writes into**. A
control that does something visible while the label stays still is a broken
read half for that kind, which is the failure the write-side coverage number
cannot see. Its root is a `scroll` and it holds two draggable cards and a drop
zone, all three on purpose — and a CANVAS, which is the one part of facet a
test cannot reach at all: a drawing is either right on screen or it is not. The
canvas draws every shape family, a path, a gradient and all THREE text
commands, each with the failure it would show written beside it.

An example resolves its dependencies from a `vendor/` symlink into the repo's
own tree (`.gitignore` names the convention: `/examples/*/vendor`). If one will
not resolve, that symlink is missing:

```bash
ln -sfn ../../vendor examples/facet_gallery/vendor
```

Do NOT `cpc pm install` there — it fetches the PUBLISHED packages from the
registry and you will be testing 0.0.27 rather than your working tree,
silently.

To see what facet actually computed rather than squinting at a window, walk the
tree printing `(*n).frame()` and `facet::view_of(n)` per node. That is how the
zero-height-strip bug was found, and it is faster than any screenshot.

## 5. Traps I hit, so you do not

The first two handoffs' entries are still true and are not repeated here;
`git show` those revisions if you want them. These are the ones this round
added.

- **THE BINDING WAS ONE ARGUMENT SHORT, and the fix is in the generator.** GIR
  puts a callable's `GError **` on the callable as `throws="1"` and NOT in its
  `<parameters>`, and `cpc-bindgen`'s `--gobject` path only walked the parameter
  list — so every throwing method in the whole GObject stack was bound one
  argument short. That is not "a binding that cannot report errors": it is
  ABI-WRONG, and the callee writes an error pointer through whatever register
  the missing slot landed on. `gdk_pixbuf_animation_new_from_file` segfaulted
  INSIDE libgdk_pixbuf with a backtrace naming the binding, which is exactly the
  shape of failure that gets blamed on the caller.

  879 callables across the four GIRs this repo binds carry the attribute; the fix
  is `throws_of()` in `cpc-bindgen/src/gir.rs`, and regenerating moved 932 lines
  across seven vendor packages. `gir::tests::a_throwing_callable_gets_the_trailing_gerror_slot`
  is the gate. **Read this before writing another cannot-row against a GObject
  binding**: a call that crashes or answers nonsense may be the binding, not the
  library, and `nm -D` plus the GIR attribute is a five-minute check.
- **`create` raises EVERY bit, including the command bits.** So a verb that MEANS
  "do this now" — `animate_progress` is the one here — runs at create for every
  widget of that kind, whether or not the application ever asked. It was a
  one-step tween landing on the right value, so nothing looked wrong; what was
  wrong is that the tween's state is one static pair, so a bar being CREATED
  would have taken the animation away from a bar mid-flight. A zero duration is
  now a set, which is also what the AppKit backend does. Found by printing the
  tween's own steps in the probe, not by reading the code.
- **DO NOT MUTATE A LIST MODEL FROM INSIDE A SIZE-ALLOCATE.** This cost the most
  of anything this round. The recycler learns its width from the scrolled
  window's own adjustment, whose bounds are set during allocation — so filling
  the model there re-enters the allocation that is running. The symptoms were
  loud and pointed nowhere useful: 205 cells created for a viewport that holds
  seven, every one of them allocated at its padding's negative
  (`gtk_widget_size_allocate(): width -4 and height -5`), and a count that did
  not move when the viewport, the row height or the width request changed —
  because none of those were what drove it. `g_idle_add` halved the cell count
  and removed every warning. When a GTK number refuses to respond to the input
  that should control it, ask what phase you are in.
- **A row's HOST is not always the cell.** `geometry::reposition_children(node,
  host, frame)` wants the host that `node`'s CHILDREN were inserted into, which
  is the node's OWN widget when it has one — and every recycled row has one,
  because the row carries a click gesture. Passing the cell's Fixed instead
  asked GTK to reparent widgets that already had a parent: 61463
  `gtk_fixed_put` criticals in a four-second run. `window::host_for` answers the
  same question for a window root and is the thing to read.
- **At TEARDOWN the cell's child is already gone.** A GtkListItem being finalised
  has dropped its child, so a pointer to it kept from `setup` is dangling and
  writing through it is `g_object_set_data: assertion 'G_IS_OBJECT (object)'
  failed`, once per cell, from inside `g_object_unref`. Teardown may not touch
  the host; the rebind path may.
- **AN INTERNED STRING IS NOT NUL-TERMINATED.** `text::intern` mallocs a
  24-byte header plus exactly `n` bytes and copies `n` — there is no terminator.
  `window.cplus` kept menu titles as the interned POINTER in a `[usize; N]` and
  rebuilt the `str` with `strlen`, which reads into whatever malloc put next: a
  menu item titled "Save" came back as `Save` followed by the tail of an
  unrelated CSS declaration. **Latent for three rounds**, because the byte after
  a small allocation is usually zero — it surfaced only when a test asked about
  the second of three titles. The tables hold `str` now.
  And `text::intern` is called AT EVERY STORE rather than through a one-line
  helper: the borrow checker recognises it BY NAME as the process-lifetime
  escape (E0514 and E0515 say so in as many words) and a wrapper is opaque to
  that check — which is the compiler telling you the same thing this trap is
  about.
- **A verb can be UNANSWERABLE UNTIL SOMETHING ELSE EXISTS.** Four of
  `collection`'s were: "measure the first item only", "keep the scroll offset
  across an insert", "tell me when the reader is near the end", "let a row be
  dragged". Every one of them is a question about rows that have not been built
  yet, and a materialiser builds them all at once — so there was no moment to
  ask any of them in, and they sat in §2 as named-but-inert for three rounds.
  The recycler did not make them easier; it made them EXPRESSIBLE. Worth
  remembering when a cannot-row looks like laziness.
- **EMPTYING A MODEL SENDS THE READER TO THE TOP.** Filling in two steps — clear,
  then refill — is two `items-changed`, and the first of them says the model has
  no rows: the scrolled window's adjustment collapses to zero and the scroll
  position is gone. On a tree that is every disclosure click. One splice
  replacing the whole model says the same thing in one notification and keeps
  the anchor.
- **A GtkStringList emits `items-changed` PER `append`.** Filling a 5000-row
  model an item at a time is five thousand notifications, each of which the list
  view acts on. `gtk_string_list_splice` is one — and the binding skips it
  (`const char * const *`), so it is hand-bound in `recycler.cplus`.
- **A cannot-row belongs to the KIND, not to the word.** `is_destructive` is
  unanswerable on a `menu_item` (a GMenu is a model and carries no appearance)
  and perfectly ordinary on a `toolbar_item` and a `swipe_item`, because in this
  backend both of those are GtkButtons and GTK ships `destructive-action` as a
  style class. The manifest now carries the split explicitly. Before writing a
  row, check every kind that carries the verb rather than the first one you met.
- **A GtkHeaderBar's own children are not walkable.** The widgets you pack go
  into internal boxes under a GtkWindowHandle, so "take out what this package
  put there" has no answer at the header level and a rebuilt toolbar would
  append to the old one forever. One GtkBox per side, packed once and refilled,
  has an answer: its children ARE its children. It also disposes of the
  ordering subtlety, since `pack_end` packs inward from the edge and would
  otherwise need the trailing items in reverse.
- **Three placements, two slots, and a header bar does not overflow.**
  `toolbar_item.placement` has three values and GTK has `pack_start` and
  `pack_end`; `priority` does two jobs on AppKit (the sort AND which item
  survives into the overflow menu) and only the first exists here, because a
  GtkHeaderBar clips rather than overflowing. Both are manifest rows now. A verb
  that maps ONTO LESS is still worth stating even when nothing is missing.
- **A GtkFixed does not clip.** That is why it is the host under an external
  layout engine — and it means any kind that moves a child outside its own
  frame draws over its neighbours. `swipeable` is the only one that does, and it
  sets `GTK_OVERFLOW_HIDDEN` on itself alone rather than on every Fixed.
- **`gdk_toplevel_begin_move` wants SURFACE coordinates.** A widget nested in a
  tree is not at the surface origin, and GTK keeps a further offset for
  client-side shadows — so the press point goes through
  `gtk_widget_translate_coordinates` to the native's widget space and then
  through `gtk_native_get_surface_transform`. Skip either and the window jumps
  by the width of its own drop shadow on the first pixel of every drag.
- **A dependency is a choice, and `g_module_open` is the third option.** Linking
  WebKitGTK would make every facet application on Linux depend on a browser and
  fail to build without one; refusing the kind would have been a false
  cannot-row. Opening the engine at runtime is neither, and it makes the failure
  legible: one line naming the library it looked for, instead of a linker error
  about a symbol nobody recognises. Reach for it when a kind needs something
  large that most applications will not use.
- **`tools/parity.py` counted bits named in COMMENTS.** A comment saying "NOT
  `menu_item::P_IS_DESTRUCTIVE`, which this backend does not answer" named the
  bit as loudly as an implementation would — so the honest thing to write was
  also the thing that inflated the number. The tool strips line comments now.
  Neither other backend's number moved, so nobody had been leaning on it.
- **A `Vec` behind a raw pointer does not take a swap written through it.** An
  in-place insertion sort over `*vec::Vec[usize]` silently did nothing, and the
  test that caught it was the one asserting the ORDER rather than the contents.
  The menu sort builds a fresh vector instead.
- **A viewless child's change arrives as `touch_all` on its HOST.** `mount`
  does that because a `span` cannot say which of its label's verbs it affected
  — so a span edit reaches `apply_label` with every bit set and needs no dirty
  bit of its own. That is the mechanism every remaining non-view kind will use.
- **Pango's markup is XML.** A span run containing `&` or `<` ends the layout at
  that character or fails to parse outright, and a caption is exactly where an
  ampersand turns up. The escape walks BYTES, which is right for UTF-8: every
  byte of a multi-byte sequence is >= 0x80 and none of the five entities is.
- **A Taylor series only works NEAR ZERO.** The gradient axis needs a sine, and
  the first version normalised the angle to one turn and then evaluated seven
  terms about zero — which gives −0.075 for sin(π), a value that is 0. The fold
  to the FIRST QUADRANT with the sign carried separately is what makes it
  usable, and it is what the comment claimed before the code did it.
- **cairo's arc names read mathematically and draw on SCREEN.** Its y grows
  downward, so `cairo_arc` sweeps clockwise on screen while its name suggests
  otherwise. Taking the name at face value mirrors every arc and nothing else
  looks wrong — which is why the probe draws a quarter arc with the direction
  written in its comment.
- **A gesture that must not fight the scroll listens for a PRECONDITION.**
  Pull-to-refresh works because it only fires while the scroll is already at its
  top; arming it in the capture phase, or claiming the drag, would have broken
  scrolling to add a refresh. If a gesture has to share a widget with GTK's own,
  look for the state that tells them apart rather than for a phase.
- **Which widget GTK exposes decides what a verb costs.** `is_open` is one
  `notify::active` on a GtkMenuButton and unreachable on a GtkDropDown, because
  the first keeps its popover as a property and the second keeps it as a private
  child. Before writing a cannot-row, check whether a DIFFERENT GTK widget
  answers the same verb — the pickers and `popup` are the same three verbs with
  opposite answers.
- **GTK counts months from ZERO** and facet from one. The kind of difference
  that ships as "every date is a month early".
- **`flex-wrap` is not a grid.** Six items 100 points wide in a 300-point
  container wrap after TWO, not three. Whatever the reason, a column count that
  comes out one short is not a column count — `collection` emits one row node
  per line instead, which also aligns the lines, which wrap does not.
- **A third of 300 is not 100.** `100.0 / 3.0` percent of 300 comes back as
  100.000000000000014, so a test asserting `== 100.0` is asserting about binary
  floating point rather than about the layout. Give layout assertions a point of
  tolerance.
- **Headless, every label measures 0x0.** No backend means no measure callback,
  so a test that leans on text measurement is asserting about the absence of a
  window. State the sizes a layout test depends on.
- **`flex_layout`'s default `flex_shrink` is 0, not CSS's 1.** Nothing gives
  way unless it is told to, which is right for an ordinary box and exactly
  backwards for a scroll host — and it is the single fact behind two of this
  round's five bugs. If a container overflows and you expected it to squash,
  this is why.
- **A short label with `line_break: TailTruncation` makes GTK warn.** An
  ellipsizing GtkLabel reports its MINIMUM width as the ellipsis and its
  NATURAL width as the text, so a glyph narrower than "…" measures natural <
  minimum and GTK says so once per label per frame. A disclosure triangle has
  nothing to truncate; saying `NoWrap` is the fix, and the warning is worth
  remembering because it names the label and not the caller.
- **`window::open_window` MOUNTS, and that is not optional.** `mount::mount` is
  what calls `app::open_window`, so the app's record and the native window have
  to be established together or `app::newest_window_root()` answers about a
  window that is not there — which every window-scoped verb in the facade
  resolves through. Mine did not, and the gallery came up empty with a
  perfectly correct layout. If you add a second window entry, mount in it.
- **A split's panes are sized by writing STYLE, not by placing them.** Setting
  the pane rects at placement time would leave their CONTENTS laid out for a
  different width — flex has to know the position, so the position is written
  as the leading pane's extent and both minimums as the panes' own. And the
  LEADING pane is the one that shrinks: `min_trailing` cannot be checked when
  the position is written, because the container's width is unknown until
  layout.
- **A widget this package adds for itself is not one of facet's children.** The
  split's divider lives in the split's own GtkFixed, so `views::place_at_slot`
  counts only widgets that carry a node — otherwise every slot after the
  divider shifts by one.
- **`runtime.cplus` — the NEUTRAL base — did not compile.** It returned
  `vocab::Appearance::System`, and there is no such variant. That file is what
  every platform without an override lands on, so `facet_runtime` was red on
  Linux the whole time and the failure looked like a missing facade rather than
  a typo. It is the file a new platform builds against; check it FIRST when
  porting, before writing anything.
- **Copy the facade, do not write one.** The base says so and it is right:
  `runtime_linux.cplus` is `runtime_macos.cplus` with three regions changed —
  the imports, the seven observers, and the quit seam. Everything else in
  ~1100 lines is about facet's own tiers and is not platform-shaped at all.
- **There is no ⌘Q on GTK, and that is a simplification.** The macOS facade
  intercepts `terminate:` because it never returns to the run loop. Here every
  quit is a window closing, so `on_should_quit` is asked through the primary
  window's `should_close` and the ordinary unwind is the only unwind.
- **A menu bar changes the LAYOUT SIZE.** The window's child is a vertical
  GtkBox holding the bar and the content GtkFixed, so `relayout_all` measures
  the FIXED and never the window. Measuring the window lays every tree a menu
  bar's height too tall and hides the bottom row of every screen.
- **GtkFileDialog is async only.** GTK 4.10 replaced the modal chooser and left
  no blocking form, and facet's verb answers a path. The wait is a nested
  GMainLoop quit from the completion callback — which is also what makes the
  blocking `alert` work, and the rest of the app keeps drawing through both.
- **A GMenu item names an ACTION by string.** There is no "menu item with a
  callback" in GTK 4: the item carries `app.facet.item7`, a GSimpleActionGroup
  holds the action, and the group is inserted on the window. The handler and
  its context are kept in a fixed table here rather than on the MenuItem,
  because the MenuItem owns three Texts the caller's AppMenu still holds.
- **`GSimpleAction::activate` and `GtkDropTarget::drop` are both missing from
  the binding**, for the same reason: a parameter the GIR does not present (a
  GVariant, a GValue). Both are hand-bound as typed aliases of
  `g_signal_connect_data`, which is what `gobject/signal.cplus` describes.
  Assume nothing about a class being fully bound because most of it is.
- **The tests must not name a `P_*` bit outside `test_main.cplus`.**
  `tools/parity.py` counts references and skips only that file, so an inline
  test asserting on `label::P_TEXT` would inflate the coverage number. That is
  why this package has a test root and not inline tests.

## 6. The one-line summary

The stack is wired end to end — contract, backend, facade, application — and
the gallery is both the proof and the queue. What is left is breadth (330 of
360 prop bits, and three non-view families unread) and the recycler; the gallery
is the thing that says which of them to do first, and it has now said it six
times.
