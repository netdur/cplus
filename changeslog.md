# C+ changelog

User-facing changes per release, newest first. The changelog starts at v0.0.14;
earlier history lives in each version's archived plan.

## v0.0.27 — 2026-08-14

> From v0.0.26 (~677 commits, 2026-07-02 → 2026-08-11). Three strands: the
> language and memory model close enough for real C ABI and safe views;
> facet grows into a full app surface (lifecycle, async, theme, ownership,
> layout); and the binding/tooling stack reaches Linux (GObject/GTK), layout
> (flex_layout), data (sqlite), and live tooling (inspector).

### Standard library
- **`stdlib/slice`** — checked sub-views over `T[]`: `sub` returns
  `Option[T[]]` (None on an invalid range), plus clamped
  `prefix`/`suffix`/`drop_first`/`drop_last`. The free-fn spelling ships
  the semantics; the `xs.sub(from:to:)` method form waits on generic
  slice impls.
- **`stdlib/flags`** — a `Flags` option-set over u64 bits, replacing the
  `(dirty & MASK) != (0 as u64)` texture with membership verbs:
  `contains` / `intersects` / `with` / `without` / `toggled` plus set
  algebra (`union_with` / `intersect_with` / `minus`). Bit values come
  from `const` masks (constant expressions fold) or repr-enum
  discriminants cast at the call site.

### Language
- **Derive through the empty impl**: `impl Point: Eq {}` — an empty impl
  block against `Eq` / `Ord` / `Hash` / `Clone` / `ToText` generates the
  memberwise implementation, extending the marker-impl idiom (`impl H:
  Send {}`) to code generation. Derived methods are synthesized as
  ordinary AST before sema, so they type-check, borrow-check and satisfy
  interface bounds exactly like hand-written ones — a struct with derived
  `Hash` + `Eq` is a valid `HashMap` key. Field rules: primitives and
  `str` direct (str orders via its blessed `compare`), nested structs
  recurse through their own methods, payload-free enums compare/hash by
  discriminant, generic targets carry their declared bounds
  (`impl Pair[T: Eq]: Eq {}`). Payload enums, arrays, slices and tuples
  are not derivable — **E0920** names the field and the manual fix.
  Deriving needs a struct target (E0916 otherwise); `Copy` remains
  structural and is never written.
- **Function contracts v0 — `#[requires(EXPR)]`**: machine-checked
  preconditions in the signature, on fns and methods (repeatable; full
  expressions over parameters, `this` fields, and consts — the attribute
  grammar takes real expressions now). Sema requires `bool` and PURITY
  (**E0924** — no calls or assignments in a contract); codegen emits
  each through the `assert` path at entry, so a violation traps (and
  test builds report instead of aborting). v0 checks in every build
  profile; `#[ensures]` and doc/graph surfacing are follow-ups.
- **Interface default method bodies** — an interface method may carry a
  body instead of a `;`, and an implementor may omit it. The body is COPIED
  into every impl block that left the method out, in `lower`, before
  anything else runs: the same mechanism derive uses, and the reason this
  cost no new machinery. Downstream sees a hand-written method — no new
  method-instantiation source, no new dispatch kind, no `dyn`, and `This`
  resolving through the impl block the copy now lives in. A default body
  that calls a method the implementor lacks is diagnosed against THAT type
  at its own impl block; an interface whose methods all have defaults takes
  an empty impl (`impl A: Greet {}`) without tripping E0916. The cost is
  code size: N implementors get N copies, the trade every generic in the
  language already makes.
- **Scoped threads (`thread::scope`)** — a set of threads that are all
  joined before the value goes away, which is what lets a worker BORROW a
  parent local instead of owning a copy of it. `Scope::lend(ref data, f)`
  hands `data` to a fresh thread; `Scope::drop` joins every worker it
  started, on every path out. No `Arc`, no copy back, no atomics for the
  common split-work case.
  What makes it safe is a borrow the checker can see: `lend` is
  `#[keeps(this)]` on a `ref` parameter, which now ties that argument to the
  receiver for the receiver's whole life (it used to tie view-typed
  arguments only — a `str` is a borrow written as a value, and a `ref`
  parameter is a borrow written as a borrow). Three mistakes became compile
  errors: the lent value dying while the scope lives (**E0514**), a write
  into it while a worker holds it (**E0381**), and lending the same place
  twice (**E0381**, two workers with exclusive access to one value).
  The last two closed real gaps in the borrow model that predate scopes: a
  mutating method call on a borrowed place was refused, but a plain field
  WRITE was not, and neither was passing the place as a `ref` argument. Both
  now are, for views as much as for scopes.
- **issue-10, the method-generic arm** — the body-instantiation pass had two
  arms, generic free fns and generic-struct impls. A method carrying its own
  type parameter (`impl Holder { fn lend[T](..) }`) is neither, so its body
  was never walked under a concrete `T`: a generic type it mentioned never
  got instantiated, monomorphize left it as a `TypeKind::Generic`, and
  codegen panicked with "monomorphize did not rewrite this site". Method
  instantiations now ride the same worklist, so what their bodies discover
  is chased exactly like a free fn's.
- **`#[ensures(EXPR)]`** — the other half of a contract. Same shape, same
  purity rule (**E0924**) and same emission as `#[requires]`, plus the
  binding `result`: the value being returned. It is checked at EVERY
  return, after the value exists and before the scope-exit drops, so a
  function with three returns is checked three times and no path out
  skips it. A postcondition on a function that returns nothing is still
  useful and still allowed — it speaks about `this` and `ref` parameters,
  which is the half a precondition cannot express — but naming `result`
  there is **E0928**, as is a parameter that collides with the binding.
  A postcondition sits between the call and the `ret`, which `musttail`
  forbids, so a contract turns a tail call into an ordinary one. Still
  open from the original item: `old()` state capture, which needs a
  snapshot rule against the ownership model.
- **`#[repr(C)] union`**: one storage, several typed views — the shape real
  C headers contain and bindgen could not describe without lying. Same
  declaration form as a struct (it rides `StructDecl` with `is_union`), C's
  layout rule (size = largest member, alignment = strictest, size rounded up
  so an array of unions strides correctly — verified field-for-field against
  clang), and field access at offset 0 with the load's type picking the
  member. Members must be `Copy`, the union non-generic and non-empty, and a
  literal names exactly one member — all **E0925**, and all consequences of a
  union having no tag: nothing can know which member is live, so no
  destructor could be run correctly. For an either/or VALUE in ordinary code
  the answer is still an enum with payloads, which has that tag.
- **Packed structs and bitfields** — the other half of describing a C
  header. `#[repr(C, packed)]` removes the padding between fields (and the
  struct's own alignment with it); `#[repr(C, packed = N)]` caps each
  field's alignment at N, C's `#pragma pack(N)`. `#[bits(N)]` gives an
  integer field a width and packs it beside its neighbours: a field that
  would cross its declared type's storage unit starts a new one, packing
  lets it straddle instead, a signed field sign-extends on read, and a
  write is a read-modify-write that preserves the neighbours sharing the
  unit. Every rule is C's and none of it is reasoned about — `cpc/tests/
  packed_layout_vs_clang.rs` builds nine shapes under both compilers and
  compares sizes, alignments, the struct's BYTES after per-field writes,
  and each field read back.
  One consequence has a diagnostic: neither a bitfield nor an under-aligned
  packed field has an address, so a `ref` parameter, a `ref this` receiver
  and `#addr_of` are refused (**E0927** / **E0926**). Passing a bitfield to
  a `ref u32` used to take the address of a temporary copy and discard the
  write. Packed fields must be `Copy` (a destructor is handed the address of
  what it tears down); a bitfield needs `#[repr(C)]`, an integer type, a
  width of 1..=its type's bits, and a non-generic, non-union struct.
- **`cpc-bindgen` emits both**: a bitfield becomes `#[bits(N)]` with its own
  signedness, a packed record becomes `#[repr(C, packed)]`, and C's
  alignment-forcing `unsigned :0;` becomes the padding width it stands for
  (computed with the compiler's own placement rule, which the generator now
  calls rather than re-deriving). What it emitted before was a `_packed0:
  u32` storage slot and read-only accessors that assumed every bitfield in
  the record lived in that first slot — wrong for a second run, for storage
  wider than 32 bits, and for every signed field, with no way to write one.
  A record under `#pragma pack(N)` is now REFUSED with a comment saying why:
  clang's JSON AST records that a maximum field alignment applies without
  saying what it is, and a guessed layout is the failure a binding generator
  exists to prevent.
- **FFI enums — explicit discriminants + `#[repr]`**: a payload-free
  enum takes C-style values (`NotFound = 404`, auto `prev + 1`
  otherwise; any constant expression folds) and an integer
  representation (`#[repr(u8)]` … `#[repr(i64)]`, `C` = i32), which is
  what it lowers to and crosses the C ABI as. Casts read the declared
  value; match switches on it. Wrong shapes — payload enums with values
  or an integer repr, out-of-range or duplicate values — are **E0923**.
  (The remaining FFI-completeness items — `#[repr(C)] union` and
  packed structs/bitfields — are tracked separately.)
- **Checked narrowing `as?`**: `n as? u8` evaluates to `Option[u8]` —
  `Some(converted)` when the value is representable, `None` otherwise.
  Integer source and target only (E0315 names the offending side); the
  infallible `as` keeps its truncating semantics for the cases where
  truncation is intended. Branch-free codegen: a 64-bit bounds check
  selecting the Option tag and payload.
- **Distinct newtypes**: `type UserId = distinct i64;` — a nominal
  integer alias. Same representation and ABI as the base; not
  interchangeable with it or with other brands (E0302 with a
  cast-pointing message). Conversion is explicit `as` in either
  direction. Same-brand `==`/`!=` and blessed `hash`/`eq` work, so a
  distinct type satisfies `Hash + Eq + Copy` bounds and keys a HashMap.
  Brands survive generics — `Vec[UserId]` is its own instantiation
  (mangled by the alias name) whose API takes and returns the brand —
  and erase to the base integer at the end of monomorphization, so
  codegen and the C ABI see plain integers. Base must be a plain integer
  type (**E0922**).
- **Const expressions**: a `const` (or scalar `static`) initializer may be
  any pure expression over literals and other consts — `const MASK: u64 =
  (1u64 << 40) - 1u64;`, `const CAP2: usize = CAP * 2;` — folded in lower
  before sema. Evaluation is TYPED at the declared width: overflow, bad
  shifts, division by zero, mixed types without a cast, and reference
  cycles are hard errors (**E0921**); the wrap spellings `+% -% *%` wrap
  exactly as at runtime. Cross-const references resolve in any order.
  Array lengths and fill counts take the same expressions inline —
  `[u8; CAP * 2]`, `[0u8; 1 << SHIFT]` — evaluated at `usize`. This
  retires the wrap-through-i32 mask-building dance.
- **Fixed: `[T; CONST]` in multi-file builds.** The resolver qualified
  const declarations but never rewrote the array-length lens, so a
  cross-module (or even same-module, in a binary target) `[T; CONST]`
  failed with E0912 "not a known const". The lens (and `[v; CONST]` fill
  counts) now resolve like every other item reference.
- **`==` / `!=` on payload-carrying enums is now E0302** ("match on the
  variants instead"). The shape previously escaped sema and died as
  invalid LLVM IR; payload-free enums still compare by discriminant.
- **`${p}` interpolation of a struct with `impl P: ToText` no longer ICEs.**
  Sema admitted the part but codegen's interp lowering only knew
  primitives / `str` / `Text` and hit its unreachable arm. Monomorphize
  now rewrites such a part into an explicit `p.to_text()` call, so
  `"${point}"` works — including derived `ToText` and nested structs.
- **`guard var` / `if var` / `while var`**: the pattern-binding statements
  take `var` in place of `let`, making the bound value(s) mutable — `guard
  var` in the enclosing scope, `if var` / `while var` inside the body
  (fresh per iteration for `while var`). Previously pattern bindings were
  always frozen; mutating one meant an extra `var x = v;` copy. The `let`
  spellings and all pattern-let diagnostics (E0347/E0348/E0349/E0350/E0351)
  are unchanged; a `guard var` complement binding (`else |Pat|`) stays
  immutable, since it is scoped to the diverging else block.
- **Value-turbofish**: `f::[T]` with no call following is a fn-pointer VALUE to
  the concrete instantiation. Bounds are enforced at the take site (E0502),
  arity at E0501; `::[T]` on a non-generic name is E0821. This makes the
  type-erasing trampoline pattern expressible from generic code — the basis of
  facet's lifecycle registry and service runner.
- **Named arguments resolve through type aliases.** `type Color =
  vocab::Color; Color::adaptive(light: a, dark: b)` used to die as E1002
  (and omitted defaults as a bogus arity error) while the fully qualified
  spelling worked — the resolver qualified the alias by its own name, so
  the named-argument matcher never found the target's signature. The
  2-segment path arm now hops the alias to its target, with the same
  cross-file visibility gates as the 3-segment arm.
- **Builder-DSL containers take constructor arguments** (DSL.5):
  `vstack(key: "body") { ... }` inside an `@` block is one element — the
  desugar passes the filled Builder first (`ctx::vstack(__b, key: "body")`),
  so every existing `fn column(take b: Builder, key: str = "")` finisher
  already satisfies the contract, and keys/config land on the element that
  has children. The `{` must follow `)` on the same line. A bare `{ ... }`
  in entry position — including `name(args)` with its block on the next
  line, which used to silently parse as two items — is now a parse error
  naming the fix.
- **Fn-pointer arguments infer generic fn-typed parameters**: a parameter
  `f: fn(take I) -> O` now unifies structurally with a fn-pointer argument,
  binding `I`/`O` (previously E0302 — a generic fn could not take a fn-typed
  argument mentioning its own params).
- **Generic calls under `await` monomorphize correctly.** Previously the
  awaited call site kept its template name and the build failed in codegen.
- Generic-fn uses inside a method-generic method on a concrete struct are now
  discovered by monomorphization (previously a codegen panic).

### Memory model — view-lifetime hardening
Nine ways a `str`/slice view of storage that dies at function return could
escape unchecked (safe-code use-after-free) are now rejected. A view is tracked
by its *shape*, not a method-name allowlist, through every form it can leak:
- **E0513 by view shape, not name.** Returning a view of a local (`return
  t.view()`, an alias, or a leaf inside a returned aggregate) is rejected for
  *any* view accessor — `Text::view`, `as_str`/`as_slice`, or a user method
  returning `str`/`T[]` from a borrowing receiver — not just two hard-coded
  names.
- **`take` parameters / `take this` are dying roots.** A returned view of a
  `take` parameter or `take this` receiver now fires E0513: the callee owns
  that storage and drops it at return (previously treated as a safe root).
- **Free-function views are traced.** `return head(local)` where `head(x) ->
  str` returns a view of its parameter is rejected — the view is rooted at
  `local`.
- **Views of temporaries.** `let s = mk().view();` (a view of a
  statement-scoped temporary) is rejected; passing such a view as a direct call
  argument stays legal.
- **Escapes into long-lived places.** Storing a view of a frame-dying owner
  into a `static` (or a static field) or a `ref` out-parameter is rejected —
  the owner outlives neither.
- **All the syntactic leak paths pin the owner (E0372/E0381).** A view written
  into a field/index place, produced by an `if`/block/`match` *expression*, or
  moved out by a struct destructure now pins its owner exactly like the direct
  `let s = t.view()` form.
- **Generic containers are tracked.** A slice view of a `Vec[T]` (or any
  generic Drop type) now pins it: moving it is E0372 and a mutating method such
  as `append` while a slice is live is E0381 (iterator invalidation). A
  read-only method alongside a live view stays legal — a shared read no longer
  spuriously conflicts with a shared borrow.
- **E0516 judges the write path, not the target's shape.** A view stored
  through a raw pointer is undeclared-flow at the raw seam, but the rule had
  matched a bare `*p = v` target only, so `(*sink).key = k` — the same store
  one field deeper — walked through and dangled at runtime. A raw-pointer
  deref anywhere in the projection chain (`(*p).f`, `(*p)[i]`, `(*p).a.b`) is
  now the same store, since the analysis knows nothing about what `p` points
  at either way. A store to a field of a plain local is unaffected.
  (`bugs/str-field-outliving-its-text-is-not-caught.md`)
- **A `*u8` return no longer erases a view.** Storing a view in a struct,
  boxing it, and returning `into_raw()`'s pointer used to compile and dangle:
  no rule fired once the type stopped naming the view. The body flow is now
  the summary — a raw-pointer-returning fn whose computed return flow carries
  a view-capable parameter gets that flow promoted to a return borrow, so the
  caller ties the pointer to the argument's owner and a dying owner is the
  same E0514 the unerased shape always was. Copying constructors
  (`text::from_str`) have an empty computed flow and tie nothing;
  self-referential carriers root at no parameter and are never promoted;
  `#[keeps(nothing)]` opts a declared boundary out. Closes the third and last
  route in `bugs/str-field-outliving-its-text-is-not-caught.md`.

### facet — component lifecycle
- `interface Lifecycle { fn on_attach(ref this); fn on_detach(ref this); }`.
  Hooks are fired FOR the component, never by it: `run_component` fires
  `on_attach` after the mount (the tree is live; initial routing belongs
  there) and `on_detach` when the loop stops, before teardown; teardown then
  drains the detach of every presented component. **Breaking:**
  `run_component` now requires `Component + Lifecycle` (empty hooks satisfy
  it).
- **`Handle.present(component)`** — the component-aware `set_child`: mounts
  the component's tree, fires its `on_attach`, and registers an erased detach
  so that whichever verb later removes or replaces that content (`present`,
  `set_child`, `remove_child`, `replace_child`, teardown) notifies the
  outgoing component first. Nested outlets included.
- **Liveness**: `facet::attached(this)` / `is_attached(cp)` answer for mounted
  components (staged components keep answering by attach state).
- **`Handle.switch_to(component)`** — the outlet verb for siblings the user
  RETURNS to (tabs, inspector panes, pagers). Each sibling is built once on
  its first visit; switching parks the outgoing view tree and re-attaches
  the incoming one, so view state (scroll offset, in-progress input)
  survives the switch. `on_attach` fires on every switch-in, `on_detach` as
  a sibling parks; switching to the shown sibling is a no-op. The outlet
  handles its own eviction (outlet leaves the tree → attached sibling
  detaches, parked trees drop) and theme changes (`set_theme` drops parked
  trees; the next switch re-stages against the new palette). Rule of thumb:
  returning siblings → `switch_to`; a new screen → `present`; don't mix the
  verbs on one outlet.
- `stage` / `attach` / `detach` remain the manual view-parking tier
  `switch_to` is built on. Staging now installs a frame-change observer on the
  parked root, so attached staged content re-lays its flex tree on ANY resize
  — window resize AND split-divider drag. (A staged subtree connects to its
  outlet natively, not in the parent flex tree, so the split-pane relayout
  walk never descended into it; before the observer a `switch_to`'d pane
  stopped reflowing on divider drags until re-attached.)
- Backend host contract: `run_window` splits into `open_window` (mounts,
  returns) and `run_loop` (blocks), so hosts can act between mount and loop.

### facet — services and async
- **`interface Service { produce; apply }` + `load_service`**: the threading
  contract as an interface — `produce` (the slow read) runs on a worker
  thread, `apply` and the optional `on_ready(ctx)` run back on the main
  thread. A service conforms once and gets the whole pipeline.
- **`run_on_main(work, ctx)`** — main-thread dispatch, backend-installed.
- **`spawn_ui(task)`** — hand an async task to the UI: it runs eagerly to its
  first suspension at the call site and resumes on the main thread as awaits
  complete. The AppKit run loop pumps the stdlib reactor through a dispatch
  source on its kqueue, so timers and async fd I/O wake tasks with the tree
  addressable. `block_on` in a handler remains wrong (it blocks the loop).
- **`on_worker(input, f)`** — awaitable blocking work: `f(input)` runs on a
  worker thread and the awaiting task resumes with the result.

### facet — App, screens, and navigation
- **`Screen`** — a component that also names its window: one conformance,
  `fn chrome(this) -> Chrome`, where `Chrome` is a plain record built with
  named parameters (title, size, minimums, titlebar flags, zoom).
  `runtime::run_screen(screen)` is `run_component` with the window read from
  the screen; it still returns the instance with its final field state.
- **`App`** — the process tier: named screen routes (`app.screen(name,
  factory)`, `factory: fn() -> ScreenBox`), an app-global menu-bar builder
  (default: the app name with Quit), `on_launch` / `on_quit` hooks, and
  `app.run(initial, arg?)` — one screen at a time as a blocking window; the
  sequential-window main loop as a type. `screen_box(s)` erases a screen to
  the heap so routes of different types share one registry.
- **`facet/nav`** — `go(route, arg?)` replaces the current screen, `push` /
  `pop` open and dismiss a secondary screen window alongside it, `quit()`
  ends the app, `arg()` reads the argument the showing verb carried.
  `go` / `quit` also unwind a `run_screen` / `run_component` window.
- **App context** — `runtime::app_running` / `app_name` / `has_screen` /
  `register_screen`: process-scoped reads from any handler, live while an
  App runs. `register_screen` adds a route at runtime (plugins); the first
  registration wins.
- **`app.agent_mcp(path)` + `facet/agent`** — opt-in serving of the agent
  surface over a Unix socket (describe_ui / click / set_text); each shown
  screen's window is re-walked into the surface. A separate module so the
  agent packages stay out of apps that never serve.
- `run_component` gains `custom_chrome` / `unified_toolbar` / `hide_title`.

### facet — theme
- **Two-tier color names.** Tier 1: the platform's semantic colors, extended
  (placeholder, link, selection backgrounds, `fill_secondary`, the full
  system palette). Tier 2: app-retintable THEME roles — `primary`/
  `on_primary`, `secondary`/`on_secondary`, `ink(a)` (the mark family, alpha
  on the color), `surface`/`raised`/`sunken`, `outline`,
  `success`/`warning`/`danger`, plus the extended surface tiers IDE-grade
  chrome needs (`content`/`toolbar`/`tabstrip`/`track`/`chip`/`recessed`) —
  each independently retintable at runtime. An unset role falls back to the
  nearest Tier-1 color, so a themeless app is native in both appearances.
  **Breaking:** `Color::primary()/secondary()/tertiary()` (the label tiers)
  are renamed `text()/text_secondary()/text_tertiary()`; `primary` now
  means the brand role.
- **`Theme` + `set_theme`.** A plain record built with named optional
  parameters, installed once. Calling `set_theme` again re-themes the LIVE
  app: the backend re-resolves recorded paint in place (same sweep as an
  appearance flip) — runtime theme switching with no extra machinery.
- **`Color::adaptive(light:, dark:)`** — a light/dark rgba pair in one
  Color, resolved by the current appearance at paint time and re-resolved
  live on a flip. Replaces per-color `is_dark()` branching and the
  rebuild-on-appearance-change dance.
- **Fix: styled containers paint.** A `column`/`row`/`zstack`/`grid` whose
  style operates on a view (background, gradient, border, corner, clip,
  opacity, hidden, shadow, transform, fade) now gets a backing view at
  mount and paints it; previously `.background()` on a container was a
  silent no-op unless something else supplied a view.

### Standard library
- `Box::into_raw` / `Box::from_raw` — surrender and reclaim a heap slot as a
  raw pointer across boundaries a `Box` cannot cross.
- Reactor: the kqueue fd is exported and a non-blocking poll variant added, so
  an external event loop can drive spawned futures.
- `thread::JoinHandle`'s refcount-shared ctx field is marked `opaque`,
  removing a spurious W0002 warning from every build that links `thread`.

### agent surface
- **describe_ui modes — the client chooses the view per request.**
  `{"params":{"mode":"full"}}` opts into the whole walked tree (auto-keyed
  structural nodes included); the DEFAULT is now the curated `exposed` view
  — only developer-exposed nodes, lean fields (id, role, name, actionable,
  re-parented to the nearest exposed ancestor) via
  `identity::Registry.describe()`. Small, high-signal context for agents;
  the `Backend` vtable gains `describe_exposed` (all three backends).
- **Exposed nodes carry a tool schema: name + intent description.** A
  node's name auto-derives from its widget's accessibility label, else its
  title/text; `.accessibility_label` overrides it (icon-only controls), and
  `.accessibility_hint` supplies the dev-authored intent ("opens the New
  Project wizard") — surfaced as `description` in the exposed describe JSON
  (omitted when empty). One annotation serves VoiceOver and agents alike.
  Registry gains set_description/description_of; NodeView and UiNode carry
  the field across all three backends.
- **The surface is lazy: it re-walks on request.** It was a one-time walk
  taken when the window opened — content mounted afterwards (`present()`-ed
  screens, added rows, @ui-native mutations: how real apps build) was
  invisible to describe_ui and unreachable by click/set_text. Every MCP
  operation now refreshes the surface from its window before acting
  (marshaled to the main thread, waitUntilDone), so describe/click/set_text
  always see the tree as it is NOW; a view not yet mounted is simply absent
  and the agent asks again later. No mutation hooks to keep complete.
  Verified live in iris: exposed describe grew from the 4-node frozen shell
  to every launcher and recents control.

### facet — the ownership hierarchy
- **process → app → window → node, in MAUI's own words**:
  `application.cplus` holds `Application` (theme, `Navigation`, `Window`s)
  — reached through `app::current()`, MAUI's `Application.Current` — with
  `open_window`/`close_window` mirroring the MAUI verbs. `runtime::App`
  embeds an `Application` by value; running makes it current, quitting
  releases it, so a second app in the same exe starts clean. Windows are
  plural and simultaneous — mount opens one, `sync` and the key-only `find`
  walk the current app's windows. A node's presentation lives on its own
  `Data` and dies with it; the mount-module registry and both lifecycle
  hooks are gone. Statics remain only for what every app shares: the
  Renderer, the platform hook slots, the UI thread, the one `current`
  door. The earlier `AppState` name was dropped deliberately: paired with
  `Component` it reads reactive, which facet is not — the record is
  ownership, and nothing re-renders.

### facet — the words facet owns (no MAUI row behind them)
- **`symbol`** (`symbol.cplus` + `icons.cplus`): an icon-font glyph as a
  control — `symbol(icons::GEAR, size:, fill:, color:)` — with 4,268
  constants generated from the bundled font by `tools/gen_icons.py`, plus
  the platform-set escape (`named:` resolves SF Symbols / freedesktop /
  Segoe on the backend). **`.gesture(…)`** (`gestures.cplus`): a behaviour
  that draws nothing is a modifier on `Node`, not a control — the 21
  gesture rows (taps, drag, hover, swipe, drop) live here, and handlers
  decline by returning `false` to pass the event up the native chain.
  **`tree`**, **`split`**, **`window_buttons`** + **`.window_drag()`**
  round out the set; facet-origin kind tags run from 1000 so regeneration
  can never collide.

### facet — the platform-free tier (Stage 3, first half)
- **The 23 tier symbols are back**, as six hand-written modules ported from
  the pre-regen facet: `nav` (route verbs, byte-for-byte), `services`
  (`run_on_main`, `after`/`Cancellable`, `spawn_ui`, `run_on_worker`,
  `Job`/`run_job`/`wait_for_jobs`), `component` (`Component`/`Lifecycle`,
  sender verbs, `component_at`), `screen` (`Chrome`, `Screen`, app menus,
  the type-erased `ScreenBox`), `theme` (roles, `set_theme`, resolution
  tables, appearance hooks), and `runtime` (the neutral facade: `Window`,
  `App` + routes, dialogs, agent hook slots — every entry refusing loudly
  with no backend). Three reseams, everything else copy-paste:
  `observe_size` takes `*Node`, the presentation registry keys on the outlet
  KEY instead of the retired `Handle`, and the Color token model (two tiers
  plus adaptive light/dark pairs) now lives in the generated vocabulary.
- **Renamed on the way in**, per the Stage 3 spec: `Service.produce` is
  `Job.run` (`run_job(job, then:)`, `wait_for_jobs` — two of iris's three
  uses produce data, the third scaffolds a project, so `Service` was a lie
  on one in three); `on_worker` is `run_on_worker`, pairing with
  `run_on_main`. `Chrome`'s six booleans collapsed to one `bar: Bar` enum
  (`Native | Blended | Hidden | Custom`) plus a separate
  `close_button_only` — 64 combinations for what was one decision. The
  spec's `Cancellable`-becomes-`events::Subscription` idea died on its own
  stated caveat: `Subscription` carries a `*Bus` its drop dereferences,
  which a timer handle must not want.
- **The mount seam is built** (`mount.cplus`): `Renderer` — the small
  vtable a backend fills (create/apply/insert/remove/schedule plus a
  `view_release` paired with every created view) — a top-down mount walk
  that inserts each view into the nearest ancestor host and lets
  view-less containers pass children through, and a sync walk that
  applies exactly the dirty nodes and clears their bits. `touch` now
  requests a sync when unbatched (a batch stays silent until its closing
  `C_FLUSH`), asserts tree writes are on the UI thread, and node teardown
  releases the native view through the pair the renderer set. The seven
  mount decisions (M1-M7) are answered in the module header; a
  fake-renderer conformance suite proves the pipeline headlessly.
- **And the rest of the page, closed in one pass**: eleven of the twelve
  deferred rows emit as fn+ctx pairs (shared-band on_focus/on_blur/
  on_attach/on_detach on CommonProps — the mount walk fires attach/detach
  post-walk; the seven continuous observers as ordinary event pairs, the
  events-package premise having died with the Subscription caveat), and
  observe_size stays a backend registration returning `Cancellable`; `set_content` (the
  verb formerly `present` — build, only-child, force-fill, typed detach
  registry, fire on_attach) and `switch_to` (flex `Display::None`
  parking — nothing removed, no cursor invalidated); `FontWeight` (a
  hand-authored scale) + `is_italic` split from MAUI's one-enum
  FontAttributes across ~10 controls and `TextSpan`; guard 5 (every
  ADOPT row reaches a control, a tier owner, or a recorded exception —
  its first run named 75 unclaimed rows, now owned); the raw-key band
  (`.gesture(on_key:)` + `key_code`/`key_named`/`consume_key` and
  friends); `find(key, within:)` defaulting to the mounted tree; and
  `MenuItem`'s default action agreeing with its own comment.

### facet — the DSL namespace
- **`elements.cplus`**: every element in one generated module, so the `@`
  builder DSL resolves bare names again — the regen had moved constructors
  into per-control modules and orphaned the DSL. `import "facet/elements"
  as ui;` then `@ui { vstack(key: "body") { button("Save", key: "save") } }`.
  Containers forward facet core's (Builder-first, so DSL.5 arguments
  compose); the 38 generated controls forward from the same signature
  authority the constructors are emitted from; the four facet-origin
  controls (`symbol`, `tree`, `split`, `window_buttons`) forward from
  guarded literals — `split(key: "sp") { pane pane }` takes its panes as
  DSL children.

### facet — addressing
- **`find(key).component()` / `component_at[C](key) -> Option[*C]`** — the
  instance-level analog of the view Handle: recover the component `present`
  registered behind a keyed slot, so a bare-fn callback (a native widget's
  fn-pointer) can invoke a component method without a module static. Valid
  while the presentation lasts; eviction clears it. iris's EditorTabs — the
  last module-static component — became an owned Workspace field.

### facet_appkit
- **Global `find` resolves a staged component's flex node.** A `switch_to`
  (staged) panel's root is a subview of its outlet, so its keyed views are
  reachable by `find_view_by_id` from the OUTER component's host too — and the
  global (cp=0) find was resolving the flex against that outer owner, whose
  tree does not contain them, yielding a null flex. Structural verbs
  (`set_child`, `add_child`, …) then silently no-op'd on a staged panel. Global
  find now picks the slot whose tree actually OWNS the view's flex, so the
  Handle carries the right owner and a live flex. This is what made a
  `facet::scroll` inside a staged router child (iris Saves timeline) never
  establish its documentView; the canonical pattern — a plain keyed slot,
  `set_child` a fresh `facet::scroll(content)` into it — now scrolls under the
  staging router exactly as it did under `present`.
- **`ui::list` follows the tail through a resize.** A narrower list re-wraps
  its rows TALLER, growing total height; the scroll view kept its point
  offset, so a list pinned at the bottom drifted UP on a narrowing (a split
  drag, a window shrink) — visible in iris chat as "narrowing the panel
  scrolls the transcript up." The debounced resize reload now records whether
  the viewport was at the bottom BEFORE it re-measures heights, and re-pins to
  the new end after if it was (widening already self-corrected via
  NSScrollView's clamp). Shared near-bottom / scroll-to-end helpers back both
  this and `list_set_count`.
- **Agent click on an expandable outline row toggles disclosure.** A folder
  row's click is the disclosure gesture, not a selection: expand (or
  collapse) it so nested rows materialize — each already carrying its
  row_id agent key — and become clickable. A file row still selects and
  fires on_select. Completes the agent file-open drive: files tab -> click
  src (expands) -> click src/main.cplus (opens in the editor).
- **Tree rows are agent-addressable and click-selectable.** `ui::tree` gains
  `row_id: fn(item, ctx) -> Text` (bound-method ready, like `row`): each
  MATERIALIZED cell is stamped with its dev key at build/recycle, so the
  agent surface exposes visible rows as children of the tree. And the agent's
  `click` on a row cell now SELECTS the row (selectRowIndexes: on the
  enclosing table, main-thread hopped) — the selection delegate fires exactly
  as a user click would, driving `on_select`. An agent can list the visible
  files and open one by its predictable id
  (`click file-tree:row:/abs/path`), verified end to end in iris: row click
  -> on_tree_select -> component_at -> EditorTabs.open -> the tab appears.
- **`ui::tree` / `ui::list` callbacks take component METHODS.** Their
  callbacks were bare fn pointers in (ctx, payload) order, so row builders
  and selection handlers had to live as top-level free fns. Both now follow
  the bound-method thunk order (payload first, ctx TRAILING) with each fn
  param's ctx adjacent (`on_select`+`ctx`, `row`+`row_ctx`) — so
  `on_select: this.open_file, row: this.file_row` binds directly, the
  receiver auto-riding the ctx. Bare fns + explicit ctx still work.
  **Breaking** for existing bare-fn callers: flip callback params to
  (item/index, ctx). iris's files_tree callbacks are FilesTree methods now.
- `image(path)` degrades a missing or unreadable file to a blank image view
  instead of aborting the app on AppKit's nil-image throw.
- **Screen windows**: `present_screen_window` (a lifecycle-aware secondary
  window — retained slot keyed by the screen instance, per-window delegate
  records, `on_closed` fires on any close path, never counted against the
  shell-window total) + `close_window(handle)`. The host pair under
  `nav::push` / `nav::pop`.
- `Application.stop` is nudged with a no-op app-defined event when the last
  shell window closes: a close driven from a callout (a `run_on_main` step,
  an agent's marshaled click) is not an event, so the stopped loop would
  otherwise idle until the next real one — an agent-driven quit hung.
- **Repaint registry**: views painted with dynamic colors (named tokens,
  adaptive pairs, theme roles) re-resolve their layer backgrounds, borders,
  gradients, and theme text colors IN PLACE on an appearance flip or
  `set_theme` — no rebuild, no relayout.

### Layout — `vendor/flex_layout`
- **Pure-C+ Flexbox + CSS Grid engine** (Yoga-parity): grow/shrink/basis,
  justify/align, min/max, percent sizes, absolute placement, measure
  callbacks. UI-kit-agnostic; 167 tests, ASan-clean. The Yoga C++ sources
  were dropped once the port stood alone.
- **`@flex` DSL** — declarative layout blocks (`row { box().grow(1.0) }`)
  as a contextual builder, with a worked app-shell demo.

### Bindings & new vendor packages
- **`cpc-bindgen --gobject`** — GIR-driven GObject Introspection path (the
  Linux analog of `--framework`): functions, enums, class graph, signals,
  boxed records, out-params, upcasts, whole-package mode, cross-namespace
  `--use`.
- **`cpc-bindgen --cpackage`** — pkg-config C package driver (Windows ABI
  hardening, dependency records, anonymous-union shims). Regenerated
  cblas/cuda and the Win32 path through it.
- **Generated GObject/GTK stack** replaces hand-made GTK/Adwaita: glib,
  gobject, gio, cairo, pango, gdk, gtk4, adwaita, and peers — all
  cross-linked via `--use`.
- **`vendor/quartzcore`** — auto-generated Core Animation binding so
  `NSView.layer()` is a real `CALayer`, not a methodless stub.
- **`vendor/sqlite` + `sqlite_ffi`** — idiomatic SQLite package over a raw
  bindgen base.
- **ObjC block signatures** — block argument and return types are part of
  the signature (no more “probably an 8-byte integer”); AppKit regen
  fixes 43 block callbacks.
- **`#[repr(C)] union`, packed structs, bitfields** land in the language
  so bindgen can describe C headers without lying (see Language above).

### facet — surface completion (post–Stage 3)
- **RTL** reaches the flex engine (not only the native view flip); shared
  band goes responsive.
- **Collection as grid** — `set_columns(n)` arranges items inside the
  collection, not only as a vertical list.
- **`text_button`** — text-only button (link-style decoration, toggle,
  border described separately from draw).
- **Live-tree verbs** that were never control-specific; band reaches
  flex constraints, absolute placement, bare `Node`, `mount::find`, nav
  params.
- **`mount::remove(key)`** — keyed delete matching create/update; owned
  collections namable at construction.
- **Splits** — panes report applied division; divider is one number flex
  owns; stylable; window buttons / traffic lights / window-drag fixed.
- **Nested scroll** — axis forwarding for nested scrolls; horizontal
  content can exceed its viewport.
- **One context per handler** across gestures, tree, pickers, list/
  collection (ctx last); typed index in the item slot.
- **Focus / blur / is_focused**; menu items can grey out, rename, find;
  shared-band background, shadow, clip; per-edge padding; adopt a native
  view as a node.
- **Appearance / theme paint** — light/dark, derived ink, and the
  appearance-flip repaint path actually run (hook was written onto the
  wrong Application record).
- **facet_gtk** freezes the contract as a second backend filling the
  same vtable; web/hybrid_web adopted onto webkit; titlebar
  leading/trailing slots.
- **Resources** — REST verbs (get/post/put/delete) over a shared store,
  async off the main thread, change notifications to watchers.

### facet_appkit — correctness pass
- Tree rows: measure after realise; place without stretch/animate/reload;
  grouped bind/height; Escape exits a text field.
- List: live drag keeps up without blue flash; tab order is document
  order; text area editor fills its box; predicted text is not rewritten.
- Drag sources begin sessions and keep payload across the pool; gesture
  views answer `performClick:`; “call super” skips classes carrying our
  own imp.
- `is_enabled` reaches every kind; collapsed panes restore; tree reports
  expansion; drag has ends; zero measurement is not a size.

### Agent surface & tooling
- **`vendor/inspector`** — live inspector for the mounted facet tree
  (declared vs computed props, structural edit, highlight overlay).
  Separate from the agent surface on purpose.
- **`agent_inapp`** — same verbs as MCP, in-process (no transport, no
  auth — the binary already trusts itself).
- Agent **click** splits permission from capability; control **parts**
  (e.g. search-field magnifier/cancel) are addressable; text writes and
  sheets reach the app.
- Compiler lints **W0824 / W0825** — declare that a handler takes (or
  refuses) a bound method / ctx slot at the declaration, not only at the
  call site.
- **`cpc test --asan`**, **`--release`**, **`--timings`**; many
  borrowck/sema/codegen soundness fixes (match consumes scrutinee when
  it binds a name; raw-pointer assignment drops old value; noalias and
  TBAA fixes under -O2/+).

### Completion — one composed answer, three front doors
- **`complete FILE:LINE:COL`** — the caret question, answered whole. The
  graph decides whether a caret follows a `.` (the receiver's fields and
  methods), a `::` (a module's items or a type's methods and variants), or
  neither (everything in scope), filters by the word already typed, and
  ranks nearest-binding-first. `scope-at` / `type-at` / `members` remain the
  primitives; deciding *which* of them a caret is asking is C+'s rules, so
  it lives in the compiler rather than in each caller. Available as
  `cpc query complete`, as the `complete_at` MCP tool, and as the LSP's
  `textDocument/completion` — all three call one function in core.
- **`cpc lsp` is resident.** It used to rebuild the whole project on *every*
  request (~2 s each on a large one), which is why it had no completion.
  The graph is now built once per project root and kept warm, with open
  buffers overlaid and rebuilds on a worker — the same session type
  `cpc mcp` uses, now shared from `cplus-core::session` rather than
  duplicated. Measured on a 185-module project: 3.7 s for the first
  answer, 3.7 ms for every one after it.
- **`cpc lsp` resolves dependencies.** It loaded projects without their
  declared `[dependencies]`, so every vendored import reported *E0401
  imported file not found*, the graph never built, and the editor silently
  fell back to single-file behaviour on any real project. It now resolves
  the way `cpc build` does, and diagnostics for such a project are the
  program's own instead of a fabricated missing import.
- **The graph is indexed.** `CodeGraph` carries `by_id` / `by_name` (symbol
  id and bare name to node indices), built once with it, so `def`,
  `members`, `symbols`, `callable_ids` and the completion walks stop
  scanning the whole node vector. Measured on a 43k-node project:
  `find_members` 1.63 → 0.29 ms, 2.67 → 0.43 ms on a 107-member type,
  `file_symbols` 0.12 → 0.07 ms. A symbol id can repeat (two `impl` blocks,
  one method name), so both maps hold a list — and answers stay in
  declaration order, so an indexed query answers identically to the scan it
  replaced. `cpc lsp` also stopped canonicalizing every file in the project
  on every request.

### agent — `wired` was false for a gesture, and `click` acted then denied it
- A regression from `wired` itself. It read appkit_ext's agent-click SLOT for
  every non-control — a slot facet never fills — so every gesture-bound
  container reported `wired=false`. That is the one CORRECT way to make a
  non-control clickable, and the very form the flag was added to tell apart
  from a dead button.
- A framework swaps in a class that receives input only for a node that HAS
  input: facet leaves a plain container as a `FlexFlippedView`, which does not
  answer `performClick:`, and arms it into a `FacetInput00`, which does. So
  answering the selector IS the answer. The exception is appkit_ext's own two
  classes, whose `performClick:` reads the slot and does nothing when it is
  empty — for those, and only those, the slot is the question.
- **`click` no longer acts and then reports `no_handler`.** The first version
  sent the click anyway on the reasoning that a report is not a refusal. That
  was worse than the bug it replaced: a verb that performs the action and says
  it did not cannot be trusted in either direction, and an agent retrying does
  the thing twice. It refuses without sending now, so a disagreement is inert.

### facet_runtime — a screen is not freed while its jobs are running
- `App::run` settles before dropping the primary screen. **Two of the three
  paths that free a screen did not.** `pushed_closed` calls `on_detach`, removes
  the entry and frees the box; `presented_closed` frees the tree — neither
  waited. A worker still inside that memory keeps writing, the next screen is
  allocated into it, and the fault surfaces in whatever was allocated next,
  which is why this family of crash never looked like the screen that caused it.
- Both settle now, and the wait lives at the FREE rather than at a caller:
  `close_all_pushed` runs before `App::run`'s settle, so a wait at the caller
  would still have been too late for pushed screens.
- **The bailout says so.** `spins > 100000` turns "teardown would wedge" into
  "teardown frees anyway", which is the same crash by another route. It is now
  a line on stderr rather than a silent `break`.

### facet — the dialogs answer the keyboard, and open on a value
- **Return and Escape.** A native alert gives a window a default button and a
  cancel button for free; a facet sheet is built out of facet controls — which
  is what makes it keyed and agent-drivable — and the cost is that everything
  the platform did for nothing has to be said. It was not said, so typing a
  name into a prompt and pressing Return did nothing, and the only way out of
  any sheet was the mouse. The primary button now carries `keyEquivalent
  "\r"` and the secondary `"\x1b"`, which also draws the default accented.
  Return fires while the text field has focus, because a window's default
  button answers regardless of first responder.
- **A `choose` sheet binds neither**, deliberately: its buttons are N options
  and none is "the obvious one", so binding Return would act on the user's
  behalf without being asked.
- **`prompt` opens on a value.** `initial:` — the ledger's `initialValue`,
  which is NOT the placeholder: one is the hint shown while the field is
  empty, the other is what the field starts with and what a person then
  edits. Only the hint had reached facet's signature, so Rename opened on
  nothing with the old name greyed out behind the caret. Appended last, on
  the neutral base and both facades.
- **Guard 8: an implementation CLAIM accounts for the row's parameters.**
  `METHOD_DROPS` says "facet says it as `runtime::prompt`" and that was
  checked at the METHOD level only — the verb exists, so the row is answered.
  A claim on a row with three or more parameters now has to say what each one
  is carried as, or that it is absent and why. It found two more the moment it
  ran: `DisplayActionSheet`'s `cancel` and `destruction` — a choose sheet has
  no cancel button and no way to mark an option destructive, and neither
  absence had been decided. Nine unaccounted parameters print on every regen.

### facet_uikit — a context menu, as the platform's own long press
- **`context_menu` is built on iOS**: `UIContextMenuInteraction` + `UIMenu` /
  `UIAction`, in `menus.cplus`. A right-click and a touch-and-hold are one
  affordance — Apple's own framing — so the ledger's `ContextFlyout` maps 1:1
  and facet's contract never says "right click"; each backend picks the
  gesture its platform has, the way `swipeable` is a menu on macOS and a pan
  on iOS.
- **The menu is built when it OPENS**, not when it is attached. A `UIMenu` is
  immutable and arrives from a provider block, where AppKit mutates an NSMenu
  built at attach time — so the provider re-reads the nodes each time and
  `enabled` and the titles are current with no refresh path at all.
- **A UIAction is the sender.** `UIActionHandler` is `void (^)(UIAction *)`,
  so one block type serves every item. `UIAccessibilityIdentification` is not
  adopted by `UIMenuElement`, so the key rides `UIAction.identifier` — the
  slot that means the same thing — and the payload rides an associated
  object. `component::key_of` / `item_of` answer off a menu action exactly as
  off a row, and reading a sender that is not a view answers empty instead of
  raising, the same guard AppKit needed.
- **A separator is an inline group.** UIKit has no separator element; the runs
  either side of a titleless item become `UIMenuOptionsDisplayInline`
  sub-menus, which is what draws the divider.
- **Inside a table the TABLE answers** — `tableView:contextMenuConfiguration
  ForRowAtIndexPath:point:` — because UITableView intercepts the long press to
  arbitrate it against scrolling. The same split that made a `context_menu` in
  an NSOutlineView row unreachable on AppKit. Both paths end in one function.
- The blocks are hand-built with a **static** descriptor rather than taken
  from the generated binding: `Block_copy` keeps the descriptor POINTER, and a
  UIAction is retained by UIKit for the life of the menu, so a descriptor on
  the frame that built the action is read after that frame is gone.
- Six checks in `selftest.cplus`, run on a real simulator by
  `tools/run_ios_tests.sh` — 78 passed, 0 failed. Disabling the two lines in
  `wants_view` / `create` turns two of them red, which is what makes them
  checks rather than decoration.

### facet — the payload half of a handler
- **`item:` at construction**, on the eight controls whose ledger type
  declares `CommandParameter` — button, checkbox, icon_button, refreshable,
  menu_item, context_menu_item, swipe_item, toolbar_item. It sits on the
  shared band beside `key`, and the pair is the point: **key is the ADDRESS a
  node answers to, item is the PAYLOAD its handler receives.**
- It had to be a *constructor parameter* rather than only `core::set_item`.
  A handler is `fn(sender, ctx)` and a bound method fills `ctx` with its
  RECEIVER (E0824) — the same pointer for every row — so the payload is the
  only thing that can say which one, and it has to be settable in the same
  expression that sets the handler. Reached only through a setter it forces a
  builder call into three statements, which is why an application that needed
  it encoded the payload into the KEY and parsed it back out instead.
- Appended LAST in each signature, so nothing a caller already passes
  positionally moves; every existing call site compiles unchanged.
- **`CommandParameter` stopped being dropped as MVVM.** It shared one regex
  and one reason string with `Command` — but `Command` is `ICommand`, which
  IS the view-model and is rightly dropped, while `CommandParameter` is the
  ARGUMENT the handler needs. facet had already rebuilt the concept from
  first principles as `core::set_item` / `item_of`, whose own comment reads
  "an item IS the payload half of a handler" — this row's definition. The map
  now points at where it lives instead of contradicting the code.

### facet — the contract closes over TYPES, not only rows
- **Guard 7**: a type the ledger renders must be decided. A row could not
  leave the ledger without a reason — an unmapped one fails the run — and a
  TYPE could. The existing type closure builds its work list from the rows a
  type declares, so a type whose surface is entirely INHERITED was never
  asked about, which is exactly what a marker type looks like:
  `MenuFlyoutSeparator` declares one member, its constructor.
- The floor is the ledger's own definition of "this renders": every type it
  puts on screen has an `I<Name>Handler`. Each must be extracted, aliased,
  dropped by family, or **UNBUILT** — a fourth outcome the other three could
  not express, because ALIAS claims "already covered, nothing refused".
- Three were claiming exactly that and were wrong: `MenuFlyoutSubItem`
  aliased to `MenuFlyout` (a submenu is not a flyout), `MenuBar` to
  `MenuBarItem` (the item is not the bar), `SwipeItemView` to `SwipeItem`.
  Six types now print each regen with the platform answer named:
  MenuBar, MenuFlyoutSeparator, MenuFlyoutSubItem, ShapeView,
  SwipeItemMenuItem, SwipeItemView.
- The check runs from `gen_contract`, not from `ledger_spec`, because that is
  where it RUNS: `ledger_spec.py` has not been able to regenerate the spec for
  some time (`check_type_closure` fails on `Matrix`, `NavigationProxy`,
  `Shape`, `TableModel` and the GIF decoder, none of which have a family
  rule), so `ledger-spec.json` is a frozen artifact and a floor living only
  there would never fire. Repairing that tool is its own job.

### facet — six gaps found building iris
- **A `context_menu` in a tree or list row can now open.** A table or
  outline view handles right-click itself and asks its OWN `menu`, which
  facet never set — so a menu nested in a row was built, attached to the
  row's view, and unreachable. Both row hosts are facet subclasses now
  (`FacetTableView` / `FacetOutlineView`) whose `menuForEvent:` answers with
  the clicked row's menu, after calling super so `clickedRow` still draws
  the ring. `swipeable`/`swipe_item` was unreachable the same way and is
  reachable for the same reason.
- **A `context_menu` under an unkeyed container is no longer dropped.** A
  node carrying one asks for a view exactly as one with a gesture or a
  background does — `wants_menu` joins `has_gestures` and `wants_painting`.
  Direct children only, so the rule agrees with `attach_context_menu`.
- **`run_job` answers whether it started, and refuses a second flight.**
  Two workers inside one job's `run()` assign the same fields; where those
  are `Text` or `Vec` the second frees what the first is writing. It shipped
  as a SIGSEGV. A 64-slot table of job addresses, claimed before the worker
  spawns and released when the apply is done. `job_in_flight` is the
  question five services were each answering with a private flag.
- **The agent surface tells a wired control from a dead one.** `describe_ui`
  reports `wired` beside `actionable` and `clickable`, and `click` answers
  `no_handler` — because an NSControl is driven by its target/action and
  ONLY that, so a gesture band on a button never fires. Made honest at the
  source: `controls::arm_control` installs an action when there is something
  on the other end and clears it when there is not, so lldb and
  Accessibility Inspector read the same fact.
- **The neutral runtime facade compiles, and keeps the facade's surface.**
  `runtime.cplus` promises app code type-checks on every target; it had
  stopped compiling on all of them (`Appearance::System` never existed) and
  was missing eight members the real facades carry. Build it with
  `cpc build --target android-arm64` — a platform-shadowed file is unbuilt
  by default on the platform you are standing on.
- **A `context_menu_item` handler can tell which item fired it.** A handler
  is `fn(sender, ctx)` and `ctx` is the component — the same pointer for
  every row — so identity is read off the sender, and a menu item was the
  one control kind carrying neither half, because both bindings live in
  `views::create` and a menu kind never gets a view. `bind_menu_identity`
  gives it the key as its `accessibilityIdentifier` and the item pointer as
  an associated object, so `component::key_of` / `item_of` answer from a
  menu action exactly as from a button. `swipe_item` gets it too.
- **A sender that is not a view no longer raises.** `key_of` and `item_of`
  walk `superview` when the first hop finds nothing, and an NSMenuItem does
  not answer it — so the natural line sent an unrecognised selector from
  inside a menu action. `input::superview_of` answers 0 for an object with
  no superview, which is what the walk's termination condition already
  means.
- **`context_menu` on iOS is an audible debt.** Being a non-view kind was
  answered first and stopped the question, so declaring one produced no
  menu and no warning. It warns once now, like every other not-yet-built
  kind. `facet_gtk`'s README carries the same decision in writing.

### Docs & process
- Vendor tutorial / guide / ref layout across packages; facet and
  facet_appkit docs closed Stage 4 / audit.
- Repo working notes (`CLAUDE.md`): local `cpc`, symbol graph over grep,
  generated-source map, AppKit “check the old tree first”.

## v0.0.26 — 2026-07-02

> A bindings release. `cpc-bindgen` grows from a C-header FFI generator into a
> framework binding generator: it reads Objective-C and Swift framework
> interfaces and emits typed, idiomatic C+ packages, bridging C object and
> collection types to standard-library types on both sides of a call. A set of
> generated vendor packages ships with it, and the standard library gains set
> and map primitives plus reference-counting uniqueness. The language changes
> are additive — named arguments, default parameter values, and a `fn(ref T)`
> pointer mode — together with soundness and codegen fixes.

### Framework binding generation
- `cpc-bindgen --framework` generates a whole C+ package from an Apple
  framework: its umbrella header, subframeworks, and transitively imported
  headers. C and Objective-C frameworks are both supported.
- An **Objective-C front-end** reads class, category, protocol, and enum
  declarations. Category methods merge into their class; `NS_ENUM` /
  `NS_OPTIONS` become C+ enums and constants.
- `--merge` emits a framework as a **single module** with full cross-type
  chaining, so a method returning one framework type resolves to that type's
  wrapper instead of an opaque handle.
- **Typed object wrappers**: object returns and arguments bind to the wrapping
  C+ type; nullable returns become `Option`; class factories return `Self`.
- **By-value structs**: geometry and descriptor structs (`MTLSize`, `NSRange`,
  `CGAffineTransform`, `CGVector`, `CATransform3D`, …) bind as C+ value structs
  with a proven struct-return ABI; typedef aliases and enum-typed fields are
  resolved.
- **Collection bridging**, both directions: `NSArray<NSString*>` ↔ `Vec[Text]`,
  `NSArray<id<P>>` ↔ `Vec[P]`, `NSArray<NSNumber*>` → `Vec[f64]`,
  `NSSet<NSString*>` ↔ `StringSet`, and `NSDictionary` ↔ `StringMap`.
- **Delegate synthesis**: a protocol becomes a C+ delegate whose callbacks carry
  scalar, struct, typedef, and collection arguments and non-void returns.
- **Blocks**: `usingBlock:` methods, completion-handler (typedef'd block)
  parameters, and value-returning blocks bind to C+ callbacks.
- `SEL` and `Class` bind as handles; `BOOL` ↔ `bool`; `**` out-parameters bind;
  Objective-C lightweight generics erase to `id`.

### Swift binding generation
- `cpc-bindgen --swift` reads a Swift module's symbol graph. `--swift-bridge`
  emits a compiled `@_cdecl` Swift bridge rather than a classification, and
  `--bridge-spec` supplies human-declared copyability the graph cannot express.

### SIMD and C vector types
- C SIMD vector types and canonical function-pointer types map to C+; integer
  and unsigned lane-vector wrappers are added to the `simd` package.

### Generated vendor packages
- New naming-guideline-aligned packages: `metal`, `appkit`, `uikit`,
  `accelerate`, `cblas`, `simd`, `win32`, `adwaita`, `android_view`, `jni`,
  `cuda`, `espidf`, `llama_cpp`, `coreai`, `log`, `uuid`, `arena`,
  `static-arena`, `rt`, and `rt_darwin`.

### Standard library
- **No-crash error model**: fallible operations return `Status` / `Option` /
  `Result` instead of trapping. The whole standard library is converted.
- **Named arguments and default parameter values** at call sites.
- New primitives: `HashSet[T]`, `StringSet` (a `Text`-keyed set), and
  `StringMap` (a `Text`-keyed owning map) with slot enumeration.
- **`Rc` / `Arc` uniqueness**: `is_unique`, `try_unwrap`, and a scoped
  `with_mut`; `MutexGuard` gains scoped `with` / `with_mut`.
- A Swift-style API pass across the standard library: value-centric method
  names, private (`_`) internals, and per-module tests. `json` migrates onto it.

### Language
- A function-pointer type can carry the `ref` mode (`fn(ref T)`): the callee
  borrows through the pointer, complementing the existing `fn(take T)`.

### Soundness & correctness
- Narrow-integer `extern` arguments and returns are zero/sign-extended on both
  the import and export sides, matching clang; `uint8_t` and other sub-word
  scalars now round-trip correctly across the C ABI.
- The `sret` return attribute is applied at a non-musttail `objc_msgSend` call
  site, so a struct-returning message no longer clobbers its receiver.
- A chained receiver (`a.b().c()`) no longer evaluates the inner receiver twice.
- Field privacy is resolved from a generic type's template origin, so an
  instantiation no longer loses its private-field marking.
- Same-named `extern` functions with conflicting signatures are rejected
  (E0357).

### Tooling
- `cpc fmt`: unary `+` / `-` hugs its operand; bitwise-or `|` is spaced.

## v0.0.25 — 2026-06-22

> A platforms-and-UI release. It also breaks the language feature freeze: the
> freeze held through the v0.0.23 hardening and the v0.0.24 vocabulary release,
> but building real cross-platform UI needed a new binding form — struct
> destructuring — so the freeze did not hold and that feature is added here.
> Otherwise: Linux (GTK 4 / libadwaita) and Windows (Win32) join macOS, the
> x86_64 System V C ABI is completed, and the `facet` UI framework and the agent
> surface span all three platforms.

### Platforms
- **Linux**: GTK 4 bindings (`vendor/gtk`) and libadwaita 1 (`vendor/adwaita`).
- **Windows** (`x86_64-pc-windows-msvc`): a native Win32 GUI binding
  (`vendor/win32`) and async socket I/O via a WSAPoll readiness reactor.
- The **x86_64 System V C ABI** is completed for fn-pointer and extern-import
  calls (struct args and returns), so non-macOS targets pass the C-ABI suite.

### UI & the agent surface
- **`facet`** + **`facet_appkit`**: a cross-platform native UI framework. UI is
  written declaratively in `@facet { ... }` builder blocks (`label` / `button` /
  `stack` + an `.on_click` modifier) and rendered to native AppKit widgets
  (FACET.1, AppKit-only so far).
- The **agent surface** gains Windows (`agent_win32`) and GTK (`agent_gtk`)
  backends alongside AppKit — describe-UI and authorized actions over a live
  native widget tree, reusing `agent_core` unchanged.
- **`agent_mcp` is backend-neutral**: one MCP (JSON-RPC) bridge drives any
  AppKit / Win32 / GTK surface through a fn-pointer vtable.

### Language
- **Struct destructuring** in `let` / `var`: `let TYPE { f1, f2 } = expr;` moves
  each named field into its own binding (mutable with `var`). Sound where a bare
  field move is rejected. The field list must be exhaustive, and a struct with
  an explicit `drop` cannot be destructured (E0509). This is the feature that
  broke the freeze.

### Soundness & correctness
- Indirect (fn-pointer) calls now apply the sret return ABI to non-Copy
  aggregate returns — a fn-pointer that returned a `drop`-carrying struct by
  value crashed (SIGBUS); it now returns correctly.
- A raw-pointer deref `*p` is a writable place for `ref` receivers and `ref`
  arguments, matching the existing `(*p).field = v` rule — `(*p).mut_method()`
  and `f(*p)` no longer raise a spurious E0328.
- The executor no longer re-enqueues an already-parked `spawn_local`'d task — a
  spawned task plus a nested `await`, both suspended on reactor sources, could
  resume a completed coroutine and segfault (#11).
- Pointer-typed `static` initializers emit as `null` / `inttoptr` instead of an
  invalid aggregate.

### Tooling
- `cpc init` / `cpc pm` / `cpc skill` — project scaffolding and package DX;
  `cpc init` pins the scaffolded stdlib dependency to the toolchain version.
- A WASM browser-playground backend runs C+ client-side (scalar core).

## v0.0.24 — 2026-06-20

> A vocabulary release: the keyword surface was revised for clarity and for
> accuracy when LLMs generate C+ code. This is a breaking change to syntax (an
> exception to the language feature freeze): most programs need mechanical
> renames. Semantics are unchanged except for the fixes under "Soundness &
> correctness" below.

### Vocabulary
- Receivers and the self-type are `this` / `This` (were `self` / `Self`).
- Parameter ownership is spelled `ref` (an exclusive, written-back borrow),
  `take` (consume / own), or a bare binding (a read-only borrow). The `mut`,
  `move`, and `borrow` markers are retired, as is the region-annotated
  `borrow REGION T` type.
- Locals: `let` is an immutable local, `var` a mutable local. `mut` is gone
  everywhere: no `let mut`, no `static mut` (every `static` is a mutable,
  addressable global), and parameters bind immutably.
- Interface impls connect with `:` (`impl Type: Interface`, was
  `impl Type for Interface`).
- Visibility is name-based: an item whose name starts with `_` is
  module-private, everything else is public. `pub` is retired; `export` marks
  the C-ABI / header / linker surface.
- `unsafe` is removed; raw-pointer and other low-level operations are written
  directly.
- A function-pointer type can carry the `take` ownership marker (`fn(take T)`),
  distinguishing a consuming callee from a borrowing one.
- Struct literals can infer their type from context: `{ field: value }`.
- `#addr(p)` returns a raw pointer's address as a `usize`.
- `Text` coerces to `str` at argument, binding, and return sites; the explicit
  `Text::as_str` is removed.

### Soundness & correctness
- Empty enums are rejected (E0361): a zero-variant enum is uninhabited yet was
  lowered as a plain `i32` tag.
- Integer literals are range-checked against their type (E0314): out-of-range
  values such as `let x: i8 = 300` are rejected instead of silently wrapping.
  Explicit `as` casts and signed minimums (`-128`, `-9223372036854775808`)
  stay valid.
- Same-scope shadowing is forbidden (E0363): redeclaring a name in one block
  silently swapped its type.
- Closed three arithmetic UB holes in codegen: oversized shift amounts,
  out-of-range float-to-int casts, and `INT_MIN / -1`.
- A Copy `ref` parameter writes back through a pointer, so `fn bump(ref n: i32)`
  updates the caller's value (verified C-callable in both directions); every
  `export fn` signature is checked for C-ABI representability (E0410).

### Docs
- SKILL, SPEC, MEMORY-MODEL, and the diagnostic catalog were rewritten to the
  v0.0.24 vocabulary; source comments were de-staled; the tutorial was retired.

## v0.0.23 — 2026-06-17

> First release under the **language feature freeze**: no new syntax or
> semantics. This is a soundness, correctness, and C-ABI hardening release.
> Code that compiled before still compiles; several programs that *wrongly*
> compiled (unsound) are now correctly rejected.

### Ownership & Drop soundness
- Closed multiple double-free / use-after-move holes: `match` now drops a
  consumed or wildcard-discarded owning scrutinee in every arm shape, and
  moving an owning payload out of a match on a place is rejected (E0337);
  array and tuple literal elements are consumed (closes the `[p, q]`
  double-free); whole-binding moves through value-transparent wrappers are
  consumed; constructing into an aggregate consumes the source; overwriting
  a Drop field or array element drops the old value first.
- A `move self` method call consumes its receiver and is rejected on a
  `borrow` receiver (E0337); turning a borrow into an owned value is
  rejected, while sound shared-region borrows are preserved.
- Fixed three Drop / ownership accounting holes (#6, #7, #8).
- stdlib made sound for non-Copy element types: `Box` / `Rc` / `Arc` /
  `Mutex` (set drops the old value, get is Copy-only),
  `HashMap[K: Copy, V: Copy]`, and the `MutexGuard` refcount (closes a
  guard-escape use-after-free).

### Generics, methods & dispatch
- Generic function *bodies* are now type-checked (closed the
  unchecked-generic-body hole).
- `Self` nested in a compound or generic type (`fn(Self)`, `*Self`,
  `Self[]`, `Pair[Self]`) is substituted in interface/impl matching.
- Method-level generics work on generic struct and enum impls
  (`impl Box[T] { fn f[U](...) }`); generic and enum associated functions
  work (`Box[i32]::make::[U]()`, `Maybe[i32]::make()`, including
  self-returning factories).
- Method-level generic bounds are enforced at the call site on generic
  struct/enum methods: `fn f[U: Copy]` rejects a non-Copy type (E0502),
  closing a double-free; impl-block generic bounds (`impl Box[T: Copy]`)
  likewise.
- `T::func()` path-form associated calls through a bound resolve; a generic
  function's type argument is inferred from a generic struct/enum argument;
  `Vec[Struct]::new()` no longer panics.

### unsafe & function pointers
- Taking a fn-pointer to an `unsafe fn` requires `unsafe` (a safe `fn(...)`
  pointer cannot carry the unsafe-ness), closing a laundering hole.
- Taking a fn-pointer to a `borrow`/`mut`-param function is rejected (ABI
  mismatch); indirect (fn-pointer) and generic-method calls consume their
  by-value arguments.

### C ABI
- Struct-by-value argument and return passing is unified onto the platform
  C ABI across native function definitions, direct calls, and fn-pointer
  calls, so a cpc function is C-callable and a fn-pointer is a real C
  function pointer (verified against clang in both directions).

### interface / impl
- Interface impls are written type-first: `impl TYPE for INTERFACE`.
- Interface/impl signature matching compares the `borrow` / `mut` / `move`
  receiver and parameter conventions, not only the types (E0505).

### Coroutines / codegen
- `for-in` early `break` no longer crashes (SIGTRAP) and drops in-scope
  coroutine locals (leak-on-cancel); await/reactor destroy edges route to
  cleanup instead of trapping.

### Text
- A string literal coerces to `Text` at assignment, matching `let` / `return`.

### Docs & tooling
- Added the normative language specification (`docs/SPEC.md`), a
  single-source diagnostic catalog (`errors.toml` + generator), and
  `MEMORY-MODEL.md`. CI runs all workflows on release tags only.

## v0.0.22 — 2026-06-13

> **Language feature freeze.** v0.0.22 is the last release to add language
> surface (syntax or semantics). From here on the language itself accepts
> **bug fixes only** — no new keywords, expressions, or type-system
> changes. New capability goes into **packages** (`vendor/`) and tooling,
> never the core language. The contextual builder-block DSL below is the
> final language feature to land.

### Contextual builder blocks (DSL.1–4: parser, lowering, lookup, containers + flow control)
- New expression syntax `@ctx { ... }`: the contextual builder block.
  `ctx` is any module path (`@view`, `@ui::view`); the body holds item
  expressions, leading-dot modifier lines that apply to the item above
  them (`.font = bigger`, `.on_click(f)`), `let` setup bindings, and
  nested `@` blocks. `@` was previously an invalid character; no
  existing source changes meaning.
- Modifier lines are line-oriented: a `.name` that starts a line attaches
  to the current item, while a same-line `.name` stays ordinary postfix
  access on the item expression. Inside call arguments, indexing,
  grouping parentheses, and nested blocks the rule is off, so wrapped
  subexpressions are unaffected.
- Parse-time rejections with builder-specific messages: a modifier with
  no current item (including after an interposed `let`), and `return` /
  `break` / `continue` / `yield` / `await` / loops / `defer` / `guard`
  inside a block.
- Blocks lower to the fixed builder protocol — ordinary package code,
  no macros: `ctx::Builder::new()`, one temporary per item with its
  modifiers applied (`__i.font = v`, `__i.method(args)`), `add(item)`
  per item, and `finish()` as the block's value. `let` entries splice
  through with ordinary block scoping; nested `@` blocks compose when
  `finish` returns the item type; the empty block is `new` + `finish`.
  Any package that ships `Builder` (`new`/`add`/`finish`), an item
  type, and constructor functions becomes a construction DSL.
- Synthesized nodes reuse the user's spans, so sema's ordinary
  diagnostics land on the DSL lines: a wrong item type reports at the
  item line, an unknown modifier field at the modifier line, a context
  module without `Builder` at the `@ctx` line.
- Contextual name lookup: inside `@view { ... }` a bare item name
  (`text(...)`) and a bare context member used as a modifier value
  resolve through the context as `view::text` without qualification.
  Precedence is locals → same-file top-level → contextual, so a `let`
  binding or a same-file function of the same name shadows the package
  member; a bare name that is no member at all falls through to the
  ordinary located "undefined" error. Item field/method names in
  modifiers (`.font`, `.boost(...)`) are never contextual. Because the
  rewrite produces real `view::text` references before the graph
  builds, code-graph/LSP navigation resolves them to the package
  symbols automatically.
- Container elements and item-control (DSL.4): a bare `name { ... }`
  inside a builder block is a *container element of the same context*
  (`vstack { ... }` builds `view::vstack`, its children resolve in
  `view`) — not a nested DSL. Containers take a filled `Builder`
  (`fn vstack(b: Builder) -> Item`), so the whole feature lowers to
  `Builder::new`/`add` plus a finisher (the root calls `.finish()`, a
  container calls `ctx::name(builder)`) — the compiler's output never
  names a collection type, so DSL packages work even on targets where
  `Vec` is gated. `if`/`else` and `for` are Flutter-style collection
  control flow: their items add into the same builder
  (`if logged_in { logout_button() }`, `for row in rows { item(row) }`),
  `if` needs no `else`. A nested *different* `@`-DSL block is rejected
  (write a same-context container without `@`); revisit if a real
  cross-DSL nesting use case appears.
- `cpc fmt` keeps `@ctx` glued and round-trips builder blocks —
  containers and `if`/`for` included — unchanged.

### Multi-backend consolidation
- New `--min-os VERSION` flag (after `--target`): overrides the OS floor
  in versioned target triples — 13.0 default for the iOS targets, API 24
  for android-arm64. Unversioned targets reject it.
- New `esp32c3-riscv32` target (RV32IMC, ilp32, ESP-IDF): 32-bit IR and
  RISC-V ELF objects through esp-clang, ABI pinned against an ilp32 probe
  (8-byte direct window, bare-pointer indirect, no byval). The object
  links into an ESP-IDF esp32c3 firmware. `TargetSpec` gains
  `extra_clang_args` (-march/-mabi for the C3).
- Windows/Linux CI now also run on main pushes, not only release tags.
- New recipe `docs/examples/recipes/android_hello`: C+ source to a signed
  APK (NDK link + script-assembled package), emulator-validated, with
  Gradle integration notes.

### Android UI (vendor/android_view + vendor/jni)
- `vendor/android_view` adopted and validated on the emulator: a C+-built
  View tree renders (nativeCreateView host contract), and Button taps
  reach C+ two ways — a host-shipped adapter class, or the self-contained
  `android_view/listener`, whose adapter ships in-package as a 976-byte
  pre-compiled DEX (`#include_bytes`), loaded with InMemoryDexClassLoader
  (API 26+) and bound via RegisterNatives; apps export one token-routed
  `cplus_on_click` hook.
- `vendor/jni` covers the full 233-slot JNI 1.6 function table (verified
  against the NDK's jni.h; object arrays, RegisterNatives, ExceptionCheck,
  NewDirectByteBuffer bound) and models `JNIEnv *` as the double pointer
  JNI requires (the bare table pointer trips an ART abort).

### Compiler
- File-aware spans: every span carries its source file (stamped at lex
  time), so cross-file diagnostics route themselves, monomorphization's
  call-site records cannot collide across files by construction (the
  v0.0.20 `(origin_file, span)` compound key is gone), and
  `#include_bytes`-style relative paths resolve against the call's own
  file. Internal-only; no language-visible change.
- String literals accept a bare `$` (previously an error; `$$` and
  `${...}` interpolation unchanged) — JNI descriptors for nested Java
  classes (`android/view/View$OnClickListener`) need it.
- Fixed an LLVM-redefinition error when a program both defines a C-ABI
  symbol (`pub extern fn`) and declares it as an extern import elsewhere
  (the app-provided-hook pattern): the import declare is now skipped for
  program-defined symbols.

## v0.0.21 — 2026-06-11

### esp32: heap types + the espidf package
- Embedded package profile: a target can exclude stdlib modules whose
  mechanism it lacks. On `esp32-xtensa`, importing the POSIX half of
  stdlib (`thread`, `mutex`, `channel`, `env`, `net`, `netsys`,
  `reactor`, `executor`, `time`, `fs`) fails at resolve time with E0866
  naming the target and pointing at `vendor/espidf` — instead of an IR
  verifier error after codegen. `async fn` on 32-bit targets is rejected
  at check time with E0867 (the coroutine runtime is 64-bit only). Heap
  modules (`vec`, `text`, `box`, ...) stay available; the host profile is
  unchanged.
- The 32-bit heap runtime: fat pointers (`{ ptr, usize }`), string/Text/Vec
  lengths, pointer-arithmetic GEP indices, and the libc size_t surface
  (`malloc` / `memcpy` / `memcmp` / `snprintf`) now follow the target's
  pointer width, lifting the heap-type restriction on `esp32-xtensa`.
  Verified by esp-clang's IR verifier (kept as a regression gate in e2e)
  and on hardware: a Text and a Vec[i32] built on the ESP32's newlib heap
  print correctly from the device. 64-bit targets are byte-identical.
- New `vendor/espidf` package: GPIO, esp_timer (`now_us`), task sleep
  (`delay_ms` via newlib `usleep`, tick-rate independent), and UART
  console printing. The gpio/timer externs are `#[no_alloc]`+`#[no_block]`
  leaves, so `#[realtime]` control loops can drive pins and read the
  clock under the contract. Entry convention: the app exports
  `cplus_app_main`; ESP-IDF's main component keeps a two-line
  `app_main` C shim. Validated on hardware with an all-C+ firmware
  (GPIO blink + `#[realtime]` PID + telemetry — no C beyond the shim).

### Multi-backend: esp32-xtensa (first 32-bit target) + #[realtime] on-device
- New `esp32-xtensa` target (rungs 3-4 collapsed: the local WROOM-32D spike
  proved esp-clang accepts cpc's IR, so 32-bit support and the Xtensa rung
  shipped together). `usize`/`isize`/pointers are 4 bytes: `llvm_ty`,
  `ty_bit_width`, `static_layout`, and `#size_of`/`#align_of` consult the
  target's pointer width (64-bit targets byte-identical). The Xtensa C ABI
  is pinned against an empirical esp-clang 20.1.1 probe: aggregate args
  ≤ 24 bytes coerce to arrays of align-sized chunks (`[3 x i32]`,
  `[2 x i64]`), larger pass indirect `byval`; returns > 16 bytes use sret
  (argument and return classification now split); no FP-register HFAs.
  Heap/fat-pointer types (Text, Vec, str) are not yet supported on 32-bit
  targets and fail loudly at IR verification rather than miscompile.
- esp-clang resolution: `$CPC_ESP_CLANG` > `$IDF_TOOLS_PATH` (set-but-wrong
  errors) > `~/.espressif`, newest `tools/esp-clang/` version, LLVM 19+
  enforced; missing installs get the `idf_tools.py install esp-clang` hint.
- Verified on hardware: a `#[realtime]` fixed-point PID (compile-time
  no-alloc / no-block / bounded-recursion contract) built as an
  `esp32-xtensa` staticlib, linked into an ESP-IDF firmware, runs closed
  loop on an ESP32-D0WDQ6 at ~1.84 µs (442 cycles) per step; the same
  contract rejects an allocating variant with E0901 at `cpc check`.
- Fixed a `musttail` miscompile-rejection (host-affecting): a
  `pub extern fn` wrapper tail-calling an internal fn returning the same
  ≤16-byte aggregate emitted `musttail` across mismatched IR return types
  (the export's return is ABI-coerced, the callee's is the bare struct);
  clang rejected the module. musttail is now skipped when either side's
  return is coerced.

### Multi-backend: target model + iOS + Android
- New `--target NAME` on `build` / `check` / `--emit-ll` / `--emit-ll-opt` /
  `--emit-asm` / `--emit-obj`. Named targets: `host` (the default),
  `ios-arm64`, `ios-arm64-simulator`, `android-arm64`. An unknown name fails
  with the supported list. Omitting `--target` reproduces the previous host
  behavior byte for byte.
- `android-arm64` (rung 2: the first non-host external toolchain): emits
  `aarch64-linux-android24` ELF objects and staticlibs through the Android
  NDK's clang, resolved from `$CPC_NDK_CLANG`, `$ANDROID_NDK_HOME` /
  `$ANDROID_NDK_ROOT` / `$ANDROID_NDK_LATEST_HOME`, or the SDK's default
  `ndk/` directory (newest version). The resolved clang must report LLVM 19+
  (NDK r28.2+); older NDKs and misconfigured variables fail with the setup
  hint. Staticlibs are archived with the NDK's `llvm-ar` (the host BSD `ar`
  cannot index ELF members). Verified end to end: a C+ staticlib linked by
  NDK clang ran on a Pixel 9 Pro XL emulator.
- A `TargetSpec` (triple, pointer width, endianness, object format, ABI and
  intrinsic selectors, handoff mode) now drives codegen's per-target decisions.
  The former compile-time `cfg!` gates (HFA classification, Microsoft x64 size
  buckets, SysV register pairs, `byval`, spin-loop hints, NEON `tbl1`, the
  Windows binary-mode ctor) resolve against the selected target, so a cross
  build emits the target's ABI and intrinsics, not the host's.
- External-builder handoff: the iOS targets stop at object emission — cpc
  never runs their final link (Xcode owns it). `cpc build` of a `[lib]`
  staticlib emits the object, archive, and C header into
  `target/<target-name>/<mode>/`; clang gets `-target <triple>` plus
  `-isysroot` from `xcrun` when available. `[[bin]]` builds, `cdylib`
  crate-types, `cpc test`, and single-file binaries are rejected for these
  targets with the supported flow named in the message.
- An explicit target pins `target triple = "<triple>"` in the emitted IR, so
  handed-off `.ll` artifacts carry their target. Host IR is unchanged.
- Bundled vendor artifacts resolve by the selected target's stable artifact
  triple (`vendor/<dep>/src/lib/arm64-apple-ios/...`); only the host target
  still consults `clang -print-target-triple`. E0862 now words the mismatch
  as a host or target triple accordingly.

### Bindings
- New `vendor/uikit` package: UIKit bindings mirroring `vendor/appkit`
  (ObjC-runtime FFI; `Window`, `ViewController`, `View`, `Label`, `Color`,
  `Screen`, app-delegate synthesis). Includes the `cplus_app_main` entry
  convention: a two-line C `main` shim in the Xcode target calls into the C+
  staticlib, which registers the delegate and enters `UIApplicationMain`.
  Verified on the iOS simulator: a C+-driven screen (white window, centered
  label) renders on an iPhone 16 Pro simulator.
- `vendor/uikit` expanded to the full binding surface (18 modules):
  controls (Button, Slider, Switch, SegmentedControl, ProgressView,
  ActivityIndicator, PageControl, DatePicker), text (TextField,
  SecureTextField, TextView, SearchBar), containers, data (TableView,
  CollectionView, PickerView), graphics (ImageView, Image, Font,
  BezierPath), dialogs (AlertController), toolbar/navigation/tab bars,
  pasteboard, Auto Layout anchors, events, notifications, navigation /
  tab / split / page controllers, custom-view synthesis (`drawRect:`),
  and ownership rules (owned wrappers release in `drop`). The umbrella
  module re-exports the set; the whole surface sema-checks for the iOS
  targets and links against the simulator SDK in e2e.

## v0.0.20 — 2026-06-11

### Agent surface (Theme B)
- New `agent_consent` recipe: a reference consent middleware over `agent_core`'s
  `AuthGate`. `decide(rules_dir, mode, agent_id, prompt)` resolves an agent in
  three steps — a remembered per-agent rule (persisted to disk), a standing Mode
  (allow-all / deny-all), else prompt the user and remember the answer — then
  maps the result onto a real `AuthGate`. Closes the "ask-user + persisted
  per-agent rules" residual; the gate itself stays a pure predicate.
- `agent_appkit` actions (click / set_text / scroll_to) now marshal to the main
  thread when called off it, so an MCP bridge driven on a background connection
  can't message AppKit off-main. Closure-free (`performSelectorOnMainThread:` +
  an `[NSThread isMainThread]` fast path; scroll_to's NSRect rides a once-
  registered `cplusScrollSelfVisible:` NSView method). `on_main_thread()` is
  public.
- New `Surface::layout_diagnostics`: per-node Auto Layout health
  (`uses_autolayout`, `has_ambiguous_layout` via `-[NSView hasAmbiguousLayout]`),
  so an agent can check a generated UI's layout without a screenshot. The tree
  walk guards the NSView-only selectors so the NSWindow root node is safe.

### Compiler
- Fixed a `musttail` miscompile on arm64: a tail call returning a by-value
  aggregate wider than 16 bytes (returned indirectly by AAPCS64) was marked
  `musttail`, which LLVM's arm64 backend rejects ("failed to perform tail call
  elimination on a call site marked musttail"). The >16-byte eligibility guard
  was x86-64-only; it now applies on all targets. Surfaced building the
  llama.cpp bindings (the 72-byte `llama_model_params` FFI return).
- Closed the inferred-call half of the v0.0.19 monomorphization fix: an
  inferred (no-turbofish) generic call resolved its concrete type-args through
  `call_monos`, keyed by a file-less span, so two such calls at the same byte
  offset in different files could select the wrong instantiation. `call_monos`
  is now keyed by `(origin_file, span)`. (Turbofish calls were already
  collision-free.)

### Bindings
- `llama_cpp` verified end to end: the `llama_cpp_smoke` recipe links against a
  current llama.cpp via `${LLAMA_CPP_LIB}` and runs real text generation on the
  Metal GPU (gemma-4-E2B). Closes the loop with the env-var portability change
  and the arm64 `musttail` fix above.

### Build / manifest
- New W0003 warning: a `[[bin]]` package's own `[link] libs`/`frameworks` are
  ignored when building the binary (those are read only when the package is a
  *dependency*). The warning points to `[[bin]] libs`/`frameworks`, where a
  binary's own libraries belong. The build still succeeds.
- `[link].search-paths` and `[link].extra-objects` now expand `${VAR}` and
  `${VAR:-default}` against the environment, so a binding can point at an
  external SDK without baking an absolute path into the manifest. An unset
  `${VAR}` with no fallback fails at parse time with E0865 naming the variable,
  rather than an opaque linker error. `vendor/llama_cpp` reads `${LLAMA_CPP_LIB}`;
  `vendor/cuda` reads `${CUDA_LIB:-/usr/local/cuda/lib64}`.

## v0.0.19 — 2026-06-09

The agent surface reaches the GUI: a macOS app can expose itself to an external
agent — described, driven, and observed — over a consent-gated JSON-RPC bridge.
Also the breaking intrinsic and string-method renames, a monomorphization fix,
and bindings for llama.cpp.

### Language / compiler (breaking)
- Intrinsics use the `#name(...)` sigil; the legacy `__cplus_*()` call spelling
  is removed.
- `.to_string()` / `ToString` are now `.to_text()` / `ToText`.
- Naming an owned string via `.to_text()` or interpolation requires
  `import "stdlib/text"` (E0613); borrowed views (`str`) need no import.

### Compiler
- Fixed a monomorphization miscompile: a turbofish generic call now mangles its
  callee from its own type-args instead of the file-keyed `call_monos`, so two
  same-offset turbofish calls in different files no longer resolve to the same
  wrong instantiation.
- Multi-file diagnostics render against the right file (GAP 3); static-init
  narrowing casts; clearer E0303 (suggests `Text`) and E0502 (names the real
  type) messages.

### Agent surface — GUI side (Theme B)
- `vendor/agent_appkit`: `open(window)` walks the live NSView tree into a
  `Surface`. describe_ui snapshot (`Vec[UiNode]`); authorized `click` /
  `set_text` / `scroll_to` through the agent_core authorization brain (exposure
  via `set_agent_id`, optimistic-concurrency text edits); notification-to-verb
  event translation.
- `vendor/agent_mcp`: the MCP bridge. JSON-RPC 2.0 (describe_ui / actions /
  events) over Unix-domain sockets (`serve_uds` / `serve_fd`), every request
  gated by an agent_core consent `AuthGate`.
- New `appkit_agent` recipe showing the whole flow.

### vendor/appkit
- Ownership `into_raw` / `from_raw` for parented view wrappers (GAP 2); SF
  Symbols, a layer-backed `RoundedView`, toolbar and text coverage (GAP 4/5);
  the correct NSImage symbol-configuration selector (GAP 6).

### vendor/llama_cpp (new)
- C+ bindings for llama.cpp's C API: raw FFI generated from the upstream headers
  with cpc-bindgen (`build.sh`), plus a hand-written safe facade (`Session`:
  load / generate / tokenize / decode / sample). Links `libllama` / `libmtmd`;
  the `[link]` search-path points at a local llama.cpp build. A `llama_cpp_smoke`
  recipe shows greedy generation.

### vendor/coreai (new)
- Swift bridge for Apple's CoreAI, adapted to the real API (Xcode 27 / macOS 27).

### Tooling
- cpc-bindgen emits safe `pub fn` wrappers over `#[link_name]` externs, `pub`
  records/fields, and `pub type` typedef aliases (the bindings llama_cpp needs).

## v0.0.18 — 2026-06-08

The owned string is now `Text` — a single, fully-stdlib string type — and the
compiler-blessed `string` is gone. One owned-string concept, with most of its
API living in the standard library instead of the compiler.

### Language — `Text` replaces `string` (breaking)
- **`string` is removed.** Source-level `string` (and `string::new` /
  `string::with_capacity`) now error with E0303. The owned, growable string is
  `Text`, implemented entirely in `vendor/stdlib/src/text.cplus` and recognised
  by one compiler lang-item (`#[lang("string")]`). `str` (the borrowed view) is
  unchanged.
- **Import-required.** A file that names an owned string or uses interpolation
  must `import "stdlib/text"`. Single-file programs that only need views use
  `str`. (`.to_string()` / interpolation still work without the import via type
  inference, producing an un-nameable owned value; to *name* the type, import
  `Text`.)
- **`Text` API** — all in stdlib, extensible without touching the compiler:
  `new` / `with_capacity` / `from_str`, `push_str` / `clear` / `truncate` /
  `clone`, `len` / `capacity` / `is_empty`, `find` / `rfind` / `contains` /
  `starts_with` / `ends_with`, `slice` / `trim*` / `split -> Vec[Text]`, the
  `unsafe as_str` borrow escape hatch, and `c_str -> Option[CString]` for the C
  ABI. `Text` is `Send + Sync` (usable as a `thread::spawn` payload and in
  `Arc[Text]`).
- **Multi-line string literals** `"""..."""` — verbatim: no indentation
  stripping, no escape processing; the bytes between the quotes are the value.
- String interpolation and `.to_string()` now produce an owned `Text`.

### Language — `unsafe fn`
- Functions can be declared `unsafe fn`; calling one outside an `unsafe { }`
  block is rejected (E0801). The grep-able escape hatch for operations whose
  safety the compiler can't verify (e.g. `Text::as_str`, raw FFI returns).

### stdlib + vendor
- Migrated off `string` to `Text`: stdlib `cow` / `fs`; vendor `json`, `appkit`
  (the Objective-C string bridge), `uuid`, and `agent_core`. The owned `Text`
  made the JSON deep-clone paths safe (`Text::clone()` instead of an
  `as_str().to_string()` round-trip), removing `unsafe` from them.

## v0.0.17 — 2026-06-07

Foundations: an ownership-safe `Vec`, a compiler soundness fix behind it, the
framework-agnostic core of the agent surface, and a scoped-down package manager.

### Compiler
- **`string` value-param soundness fix:** a `string` (or other owning value)
  passed by value and then *stored or forwarded* (e.g. `self.v.push(s)`,
  `self.field = s`) instead of returned is no longer double-freed. `effective_move`
  now covers `Ty::String` alongside `Ty::Struct`/`Ty::Enum`. Repro in
  `bugs/string-param-store-double-free/`. (Requires a `cpc` reinstall from source.)

### stdlib — `Vec` rewrite (breaking)
- `Vec` is now ownership-safe: overflow-checked allocation sizing, null-checked
  malloc/realloc, and **no silent out-of-bounds reads**.
- API changes: `get` is a bounds-checked `vec::get::[T](v, i) -> Option[T]`
  (Copy elements); `at_copy(i) -> T` asserts in-bounds; `at(i) -> Option[*T]`
  reads a non-Copy element in place; `pop` returns `Option[T]`; added `set`,
  `swap_remove`, `truncate`, `shrink_to_fit`, `is_empty`. `iter` stays a gen
  method. All in-tree callers (json, clap, agent_core) migrated.

### Package manager (new: `cplus-pm`)
- A standalone tool to **manage packages in a project's `vendor/`**:
  `install` / `remove` / `update`, with git-tag versioning, `pubgrub`
  resolution, SHA-256 content addressing, a shared cache, and a lockfile. No
  dependency on the compiler.

### Agent surface core (new: `vendor/agent_core`, groundwork)
- The framework-agnostic core for agent-controllable apps: the build-time-stable
  agent-id tree, curated `describe`, the all-or-none auth gate + exposure +
  affordance ceiling, bubbling events with `{node,verb,role}` subscriptions, and
  action/text-op authorization with optimistic-concurrency versioning. Headless
  and fully tested; the AppKit backend (GUI wiring) and MCP bridge are next.

## v0.0.16 — 2026-06-07

The AppKit surface: full binding coverage, a leak-free ownership model, and
event-driven drag-and-drop — plus a P0 calling-convention fix behind all macOS
geometry, and a loop Drop/move soundness fix.

### Language
- **`#` sigil for compiler builtins:** the FFI/raw and byte-swap builtins
  (`str_ptr`, `slice_ptr`, `slice_len`, `str_from_raw_parts`, `bswap32`,
  `htons`, …) and `println` now require the `#name(...)` form, like the existing
  `#size_of`/`#addr_of`. A bare call is a fix-it error. This makes a
  compiler-known builtin self-evident at the call site (the library `io::println`
  is unchanged).
- **Infinite `loop` diverges:** a function whose body ends in an infinite `loop`
  (no `break` can exit it) no longer needs a dead trailing `return`.
- **`let _ = expr;`** is now a discard binding (evaluates and drops the value).

### AppKit (vendor/appkit)
- **Event-driven drag-and-drop:** a drag *source* can now start a drag from a
  `mouseDragged:` gesture (`create_drag_source_view` + `begin_string_drag` /
  `DraggingItem` / `begin_dragging_session`), alongside the existing drop
  destination. See the `appkit_drag_drop` recipe.
- **Leak-free ownership:** every `alloc/init` widget wrapper now follows the
  "+1 normal form" (owns its object, releases once in `drop`) — controls, text,
  containers, toolbar items, panels, controllers, data views, and the base
  views. Factory/shared/top-level objects (windows, the app, status bar,
  shared panels, colors/fonts) correctly stay non-owned.
- **Full module coverage:** every vendor/appkit module now has tests.
- **`TextField::new_label`** is a real static label (non-editable, non-bezeled);
  it no longer behaves like an input field or accepts dropped text.

### Fixes
- **Struct-by-value ABI (P0):** `NSPoint`/`NSSize`/`NSRect` and other
  homogeneous float aggregates passed by value to `objc_msgSend` now go in FP
  registers per AAPCS64. Previously they were integer-coerced / passed
  indirectly, so every geometry argument (`setFrame:`, `initWithContentRect:`,
  `moveToPoint:`, …) silently received garbage coordinates on Apple Silicon.
- **Loop-body Drop:** an owned value created inside a `while`/`for`/`loop` body
  is now dropped at the end of each iteration (and on `break`/`continue`).
  Previously it leaked every iteration.
- **Move across loop iterations:** `let y = x;` on a non-Copy value now moves
  the source, and re-moving a binding declared outside a loop on each iteration
  is rejected (E0335) — previously an un-tracked move that, with the loop-Drop
  fix, would double-free. Re-initializing the binding before the move stays
  valid.
- **Negative float literals** no longer emit invalid IR (`double -5`).
- **`Slider`** value get/set used the wrong (float vs double) ABI; fixed to
  `doubleValue`/`setDoubleValue:`.

### Infra
- **macOS CI:** a `cargo test --workspace` job now runs on Apple Silicon
  (push-to-main + PRs), alongside the tag-triggered Linux and Windows CI.

## v0.0.15 — 2026-06-05

Language hardening, a P0 ownership fix, the first Linux and Windows ports, and
GPU/CPU BLAS bindings.

### Language
- **Module-level global asm:** `#asm("...")` at item scope lowers to LLVM
  `module asm`, for raw module-level symbols or directives. The function-body
  `#asm(...)` inline-asm form is unchanged.
- **`#[no_alloc]` drop glue:** the check now also rejects owned drop-carrying
  parameters (`move x`, a move-by-default non-Copy struct, `move self`) and
  discarded drop-carrying temporaries, not just `let` locals.
- **`if`/`else` value typing:** an if-expression sizes its result from the type
  codegen actually produces, so any value-producing arm shape (including a
  method call) is accepted; the previous hand-kept type predictor is removed.

### Graph / LSP
- **value-refs precise scoping:** uses resolve to the innermost in-scope
  definition (shadowing is handled correctly); `match`-arm payload bindings and
  `for` loop variables are first-class definitions; and a binding returned from
  a function records the caller-side bindings its value flows into.

### Fixes
- **Ownership (P0):** a heap-owning enum passed by value as a call or method
  argument (e.g. `vec.push(v)` where `v` owns a nested `Vec`) is now moved
  rather than borrow-copied. Previously the caller's scope-exit drop could free
  memory the callee had already stored, a use-after-free (surfaced by a
  `vendor/json` parse + stringify round-trip).
- **Borrow checker:** a bare non-Copy concrete struct/enum argument used while
  its place is borrowed now reports E0372 (move while borrowed) instead of
  E0383 (read while borrowed), matching the move semantics.
- **Codegen:** string interpolation frees its per-segment conversion buffers
  (previously leaked).
- **Use-after-move on generic-payload types:** an enum or struct whose
  Copy-ness depends on a generic payload/field (e.g. `enum W { A(Vec[i32]) }`,
  a recursive `Node { Branch(Vec[Node]) }`, the `vendor/json`
  `Value::Array(Vec[Value])` shape) is now correctly treated as non-Copy, so a
  use-after-move on it is reported (E0335). The move check is also now
  flow-sensitive: a move that happens only on a branch that `return`s/`break`s/
  `continue`s no longer falsely poisons the value on the path where that branch
  is not taken.

### Numerics / GPU
- **`vendor/cuda`:** CUDA Runtime + cuBLAS bindings (NVIDIA GPU) — device
  management, `DeviceBuffer` (Drop = `cudaFree`), a cuBLAS `Handle`
  (Drop = `cublasDestroy`) with `sgemm`/`sgemv` (column-major). Plain C FFI, no
  kernel language; C+ stays a consumer of GPU SDKs.
- **`vendor/cblas`:** reference CBLAS bindings (OpenBLAS / Netlib / MKL) — the
  cross-platform CPU path. Level 1/2/3 (`sdot`/`saxpy`/`sscal`/`snrm2`/`sasum`,
  `sgemv`, `sgemm`, plus d-variants).
- **`[link] search-paths`:** a manifest `[link]` table may now list library
  search directories; each becomes both `-L<dir>` (link time) and
  `-Wl,-rpath,<dir>` (run time), so a library outside the default path
  (e.g. CUDA's `lib64`) resolves without `LD_LIBRARY_PATH`. Relative entries
  resolve against the manifest directory.

### Platform
- **Linux/x86-64:** first Linux bring-up of the toolchain (requires
  clang/LLVM 19+). `cpc` discovers a clang ≥ 19 on its own, links via GNU ld
  with `-lm`, selects `*_linux.cplus` stdlib overrides (epoll reactor), and
  ships a `.deb`. All changes are platform-conditional; macOS output is
  unchanged.
- **Windows/x86-64 (MSVC):** the toolchain builds, tests, and runs on
  `x86_64-pc-windows-msvc`. `cpc` selects `llvm-ar`, links math from the UCRT
  (no `m.lib`), pulls f16 helpers from `compiler-rt`, applies the Microsoft x64
  struct ABI (indirect for non-1/2/4/8 aggregates), sets stdout/stderr to
  binary mode so `\n` stays a single LF (not `\r\n`), and provides a Win32
  `reactor_windows` async backend (timers + cooperative scheduling; socket/file
  IOCP is a follow-up). All changes are platform-conditional.
- **Coroutine codegen portability:** `llvm.coro.end` is emitted in the
  return-type form the target clang expects (`i1` on older LLVM / Apple
  clang 21, `void` on LLVM 22+), probed at build time. Previously a fixed form
  failed to verify on the other toolchain.

### Tooling
- Linux and Windows CI run `cargo test --workspace` on release tags and attach
  the prebuilt binaries (`.deb`; Windows `.zip`) to the GitHub Release,
  alongside the macOS tarball from the release workflow.
- CI actions bumped to `actions/checkout@v5` and `upload-artifact@v5`.

## v0.0.14 — 2026-06-05

Language track. The headline themes are the completed ownership/Drop model, inline
assembly, and code-knowledge-graph value depth.

### Ownership & Drop
- **`unsafe impl Send for T {}` / `unsafe impl Sync for T {}`** — a manual marker
  override. A nominal type that transitively hides a raw pointer is now `!Send`
  and `!Sync` by default (moving or sharing it across a `Send`/`Sync` bound is
  rejected, E0502); a bare `*T` used directly stays Send. The override re-enables
  a type you vouch for. `Send`/`Sync` impls must carry `unsafe` (E0860); `unsafe`
  applies only to those markers (E0861). Conditional generic form carries the
  condition as bounds: `unsafe impl Send for Arc[T: Send + Sync] {}`. `Arc`,
  `Mutex`, and `Channel` carry the right conditional impls.
- **`#[no_alloc]` drop-glue** — a `#[no_alloc]` function now also rejects implicit
  destructors run at scope exit that would allocate/free (a `string`/`Vec`/`Box`
  local, or a type whose `drop` is not itself `#[no_alloc]`), reaching through
  fields, enum payloads, and array elements (E0901).
- **Container element drop** — dropping a `Vec[T]` (and Box/Arc/Rc/HashMap) runs
  each element's `drop` exactly once before freeing the buffer.
- **Consumed-enum payload** — matching an owned enum and binding a payload that is
  not moved out now drops it at arm exit (no leak), while every move-out shape
  still disarms the drop (no double-free).

### Inline assembly
- **Tier 2 — operands + clobbers.** Rust-style named operands:
  `#asm("add {s}, {a}, {b}", s = out(reg) sum, a = in(reg) a, b = in(reg) b,
  clobber("cc"))`. `in`/`out`/`inout` set direction; `reg` lets the compiler pick
  a register (then `{name}` must appear in the template) or `"x0"` pins one.
  `out`/`inout` targets must be `mut` variables; operands are register-sized
  scalars.
- **Tier 3 — `#[naked]` functions.** No prologue/epilogue; the body is inline asm
  that handles the ABI and returns itself (E0909 if the body is not asm-only).
  For trampolines, entry stubs, custom-ABI shims.

### Code knowledge graph
- **`type-at` on inferred expressions.** `cpc query type-at FILE:LINE:COL` (and
  LSP hover) now answer call results, field/index reads, arithmetic, and
  `match`/`if` values, not just annotated positions, rendered with concrete names
  (`Result[Value, ParseError]`, `Vec[i32]`).
- **`value-refs`.** `cpc query value-refs FILE:LINE:COL` returns a binding's
  value-flow: its definition plus every use classified as read / call / construct
  (re-wrap) / return / match / assign.
- **LSP dirty-buffer overlay.** Hover, type-at, value-refs, goto-definition,
  references, and document-symbols reflect unsaved editor edits before save.

### Fixes
- **Codegen:** a `match` arm (or other value position) whose body is an `if`
  building a payload-carrying enum constructor no longer discards the value
  (previously a silent miscompile; surfaced by the json package migration).

### Other
- `vendor/json` parser migrated to a match-consumable result enum with recursive
  auto-Drop; accessors borrow and deep-clone.

Deferred to v0.0.15 (additive): module-level global asm, the if-result predictor
refactor, value-refs precise scoping (shadowing), and the package side
(AppKit → agent).
