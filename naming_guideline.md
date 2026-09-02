# C+ API Naming and Design Guideline

## Why this exists

C+ accumulated functions and methods named without a single convention. Rather
than invent a bespoke house style, the project adopts the Swift API Design
Guidelines as its reference standard. The goal is not to resemble Swift. It is to
have one principled, well-tested convention so every package reads consistently.

C+ implemented named parameters (free order) and default values specifically to
make this style expressible. Use them.

## The core principle

Clarity at the point of use. The call site should read as a phrase that
describes intent. Optimize for the reader of calling code, not the writer of the
API.

    vec.insert(value, at: index)        // "insert value at index"
    text.slice(from: start, to: end)    // "slice from start to end"
    let label = Label::new("Inbox")

## Rules

### Name for role and meaning; omit needless words

- Name a type for what it is, not the class it wraps. A configured `NSTextField`
  used as a label is `Label`, not `TextField`.
- Drop words the type or context already implies. `index_of_selected_item`
  becomes `selected_index`; `set_string_value` becomes `set_text`.
- Do not encode the type in the name. `array_of_strings` is `strings`.

### Read as a grammatical phrase

- A method and its argument labels should form an English clause.
- The first argument is unlabeled (`_`) when the method name already implies its
  role and the call still reads well, as in `insert(value, at: index)`. Label it
  when omitting the label would be unclear.
- Booleans read as assertions: `is_editable`, `has_prefix`, `is_empty`. A boolean
  parameter is named so the call reads naturally: `set_editable(false)`.

### Use named parameters, with free order

Labeled parameters are part of the API, not decoration. They make the call
self-documenting and independent of argument order.

    fn slice(from: usize, to: usize) -> Text
    text.slice(to: 10, from: 0)         // free order is allowed

### Use default values

Collapse families of overloads and constructors into one signature with
defaults. This is the least-used feature today and should be applied across the
APIs.

    fn new(text: str, editable: bool = true, font: Font = Font::system()) -> Label
    Label::new("Hello")                 // defaults apply
    Label::new("Hello", editable: false)

### Constructors take their content

Prefer an initializer that takes the essential content over a bare constructor
followed by setters.

    Label::new("Hello")                 // not new_label(frame) then set_string_value
    Window::titled("Inbox", content: frame)

### Return types express absence and failure

- A value that can be absent returns `Option[T]`, never a sentinel or null.
- Fallible operations follow the error model: a mutator returns `Status`, a read
  returns `Option`, a value-plus-reason returns `Result`.

### Strings

Public APIs take and return `Text` or `str`, not raw `*u8` C strings. Internal
bridging (for example to an `NSString`) is hidden inside the method, not exposed
as a parallel `_ns` variant.

## What the language provides

- Named parameters with free order and default values.
- `_` for an unlabeled first argument.
- `Option`, `Result`, `Status` for the return-type rules.
- `_field` for privacy, so a clean public surface can hide raw handles.

## Scope

This guideline governs the imperative API surface of every package (stdlib, json,
appkit, and the rest), `facet` included. That last part is a correction: this
section used to exempt `facet` as a declarative, SwiftUI-style layer. facet is
not one. A control is a constructor plus a typed cursor, and every verb on that
cursor is an imperative API in the sense this page means, so the guideline
applies to it in full.

## Current state (2026-09-01)

- sensors (pass done 2026-09-02, **written against this page**, and the
  cheapest pass yet — one rename). Findings:
  - **`available(kind)` became `has(kind)`.** The call site is a question, and
    `sens::has(Kind::Barometer)` reads as one where
    `sens::available(Kind::Barometer)` reads as a noun with an argument stuck
    on. `camera::has(Facing::Back)` is the same shape and already had the right
    word; `location::available()` takes no argument and keeps its own name
    honestly. The rule is "read as a grammatical phrase", and the test is
    whether the call is a sentence.
  - A **deliberate deviation, recorded rather than hidden**: the stream handle
    is `Readings` where `location`'s is `Updates`. Cross-package uniformity
    would say pick one word, and this page says clarity at the point of use
    wins — a stream of sensor readings is not a stream of updates, and
    `READINGS.stop()` says more than `UPDATES.stop()` would.
  - `Request::defaults()` is the SAME deviation location recorded, for the same
    reason: `Request::new()` cannot be a default-argument expression (E0308).
    Two packages in, this is a language limitation worth a line in the Rules
    section rather than a repeated note in the changelog.
  - The facade declares a private `#[link_name = "sqrt"] extern fn _sqrt`
    because stdlib has no math module. That is allowed rather than a leak: the
    rule governs the SURFACE, and `Sample::magnitude()` hides it completely —
    the same test `securestore::get`'s pointer failed and this passes.

- location (pass done 2026-09-02, **written against this page and then
  audited** — the cheaper order, and it showed: four small corrections, no
  restructuring). Findings:
  - **`to_code` without `from_code`, for the third time.** `Outcome` could be
    written to the seam and not read back from it. camera's pass already
    recorded this twice in one package and suggested the pairing belonged in
    the Rules section rather than the changelog; three packages in, that is now
    plainly true — **a code a type can produce, it must be able to consume**.
  - **`enabled()` answered a question nobody asked.** At the call site
    `location::enabled()` reads as "is the location package enabled", which is
    not what it means — it is the DEVICE's system-wide switch. Now
    `services_enabled()`. The rule this failed is "name for role and meaning":
    the short name was shorter and wronger.
  - **The JNI callback was `export`ed**, verbatim the mistake camera's pass
    found: `RegisterNatives` takes a function POINTER, not a linker symbol. Two
    packages, same reflex. Worth noting that removing `export` from an
    `extern fn` does NOT leave a valid definition — a plain `extern fn` is a
    declaration and must end in `;` — so the correct form is a plain `fn`,
    which is what camera has.
  - A **deliberate deviation, recorded rather than hidden**: `Request` has two
    constructors where this page says to collapse them into one with defaults.
    `Request::defaults()` exists because `Request::new()` **cannot be a
    default-argument expression** even though every one of its parameters has a
    default — E0308 counts the parameters before filling them. camera's
    `open(request: Request = Request::new())` compiles, so the restriction is
    positional rather than absolute; not chased further.
  - Also not naming, and the same shape as camera's `println` finding: the
    macOS probe used `io::println`, whose stdout is **fully buffered when
    redirected**, so a probe killed with `pkill` produced an empty log that
    read exactly like "the events never fired". `io::eprintln` is the one to
    use, and this project's own memory already said so.

- camera (pass done 2026-09-01, **retrofitted** — written to work and then
  brought to this page, the expensive order permissions warned about, and it
  cost more than securestore's pass did). Findings:
  - **`ref this` on read-only methods was the whole readability problem.** A
    `match` binding is FROZEN, so `open() -> Result[Camera, Outcome]` forced
    every caller to write `var live: Camera = c;` inside the arm before it could
    call anything. A by-value `this` BORROWS — only `take` consumes — so
    `is_open`, `preview` and `capture` take `this` and only `close`/`drop` ask
    for `ref`. The ugliness read as a flaw in returning `Result`; it was a
    reflex in the receiver list.
  - **The docs taught the long match arm.** Every example wrote
    `result::Result[camera::Camera, camera::Outcome]::Ok(c)`. The type arguments
    are INFERRED — `result::Result::Ok(c)` compiles — so the noise was invented
    by the documentation, not required by the language. Measured with a throwaway
    package rather than assumed.
  - **`outcome_from_code` and `facing_code` were loose functions.** Now
    `Outcome::from_code` and `Facing::to_code`, paired with the `to_code` that
    already existed. This is permissions' `State::from_code` correction verbatim,
    twice in one package, which suggests the pairing rule belongs in the Rules
    section above rather than only in the changelog.
  - **The photo handler took a pointer and a length.** Now `fn(u8[], *u8)`: a
    slice carries both as one value, so a length that does not belong to the
    buffer is unrepresentable, and `#slice_ptr` / `#slice_len` are the FFI tier
    when the bytes go on to C. The `_c_*` seam still splits them, the way
    securestore's `str` arguments do — a fat pointer through a C-ABI declaration
    is a promise this repo does not make — so the facade bridges with a fixed
    four-slot table keyed by session handle.
  - **`count()` returned `i64`**, which was the seam's type leaking into the
    surface. `usize`, matching `Vec::count` and `str::count`.
  - **The Android backend exposed five transport identifiers** — `DEX_LEN`,
    `LOADER`, `C_CAMERA`, `NativeLB`, `camera_android_photo` — now all
    `_`-prefixed, http's rule. `camera_android_photo` was also `export`ed for no
    reason: `RegisterNatives` takes a function POINTER, not a linker symbol.
    Removing it was re-verified on the emulator, because "it still compiles" is
    not evidence for a JNI binding. The Apple backend needed nothing.
  - A **deliberate deviation, recorded rather than hidden**: `capture` returns
    `Outcome` where `Ok` means the request was ACCEPTED, not that a photo
    exists. The verb starts an asynchronous operation and the photo arrives on a
    handler; there is no return value that could carry the result, and `Status`
    has no arm for "in flight".
  - **A required `_ctx` slot cannot receive a bound method**, which is worth
    recording because facet's own comment on `adopt_native_with` says the
    opposite. Measured: the compiler fills a handler's context slot with the
    receiver only when that slot has a DEFAULT — a required one is rejected by
    the arity check first (E0308), even though E0824 shows the fill was
    intended. `facet::services::run_on_main(work, ctx)` was the only one of its
    three neighbours without a default (`after` and `observe_size` both have
    one), so `run_on_main(this.publish)` did not compile. Defaulted.
  - Not naming, but found by the same pass and worth the same warning: the
    probes were full of `var m: text::Text = "..."; io::println(m.view());`.
    An interpolated literal passed DIRECTLY to `io::print`/`println`/`eprintln`
    is a SINK — zero heap, no `Text` materialised — and the reference says so.
    The ceremony was both slower and uglier. A named binding is right only when
    the message is used twice.

- securestore (pass done 2026-09-01, **written against this page and then
  audited** — the cheaper order permissions recommends, and it showed: both
  backends needed nothing). Findings:
  - **`set(key, value)` was two adjacent same-typed positional strings**, which
    is the third time this page has recorded that exact mistake — http's
    `set_header(name, value)` and permissions' `register_apple`. Swappable at
    the call site, silent when reversed, and a stored secret under the wrong key
    is the worst possible version of it. Now `set(key, to: value)`, the shape
    `set_header(name, to:)` and `Vec::set(value, at:)` already use.
  - **The out-parameter was unlabelled**: `get(key, #addr_of(t))` said nothing
    about which way the data flowed. Now `get(key, into: #addr_of(t))`.
  - **`DEFAULT_SERVICE` was public and no verb took it.** That is the test this
    page states — permissions' `S_*` constants stay public *because*
    `register_apple` takes them — and this one failed it: it existed only as a
    default value nobody types. Deleted; the default is `""` inline, and the
    namespace rule is documented once above the verbs rather than encoded in a
    constant.
  - **The five `C_*` seam codes were public.** They are the integers the two
    halves meet at, meaningless to a caller, and now `_C_*`.
  - `clear(service)` took its one argument positionally while the other four
    labelled the same argument. Labelled, so `service:` reads the same
    everywhere.
  - A **deliberate deviation, recorded rather than hidden**: `get` writes
    through an out-parameter instead of returning `Option[Text]`. A caller must
    tell "absent" from "refused" from "broken", and `Option` collapses the last
    two into the first — this is a value plus five reasons, which the error
    model calls a `Result` and which the out-parameter plus `Outcome` spells
    without a generic.
  - Both backends passed the audit with nothing to change, which is the argument
    for writing to this page first: `permissions` was retrofitted and had about
    twenty identifiers to hide.

## Earlier state (2026-08-31)

- permissions (pass done 2026-08-31, retrofitted — the package was written to
  work and then brought to this page, which is the more expensive order and
  worth not repeating). Five findings:
  - **The surface was the whole backend.** `permissions_backend::` exposed
    about twenty identifiers — `Domain`, `find`, `class_of`, `add`, `domains`,
    `map_status`, `intern_c`, `open_pane`, `settings_url` — of which exactly one
    (`register_apple`) was meant to be public. All the rest are `_`-prefixed
    now, http's rule applied verbatim. The `S_*` / `R_*` / `M_*` constants stay
    public because `register_apple` takes them, which is the test for whether a
    constant belongs to the surface.
  - **`register_apple` took eleven positional arguments**, three of them
    adjacent strings. Now `name` positional and every other argument labelled,
    with the four that most rows do not need defaulted (`framework`,
    `argument`, `value`, `settings_anchor`). Same finding http recorded about
    two positional strings, four times over.
  - **`for:` is not available** — `for` is a keyword. `state(of: name)` reads
    and is used; `can_prompt(name)` stays unlabelled because every alternative
    read worse than none and the verb already implies its one argument;
    `open_settings(pane:)` names what the argument actually is rather than
    reaching for a preposition.
  - **The handler pair is spelled facet's way**: `request(name, on_answer:,
    on_answer_ctx:)`, not `cb` / `ctx`. One spelling for a handler pair across
    the tree beats a second one invented per package.
  - **`state_of_code` became `State::from_code`**, paired with the existing
    `to_code` — one round trip should read as one pair, not as a method and a
    loose function.
  - A **deliberate deviation, recorded rather than hidden**: `open_settings`
    returns `bool`, not `Status`. The only thing that can go wrong is "this
    platform has no such page", and `stdlib/status` has no arm that means it —
    `InvalidInput` would blame the caller for a fact about the platform.
  - The pass paid for itself immediately: making the backend private broke the
    iOS runner, which had been reaching into `find` and `class_of`. Rewriting it
    against the public surface worked unchanged, which is the evidence the
    surface was sufficient all along — a test that can only be written against
    internals is usually testing the wrong thing.

## Earlier state (2026-08-22)

- http (pass done 2026-08-22, written against this page rather than retrofitted):
  the two audit findings were a leaked surface and two bare positional pairs.
  Every transport identifier — the block struct, the slot, `perform`, `harvest`,
  `build_ns_request`, the descriptor static — is `_`-prefixed, so `http::` offers
  the documented surface and nothing else (verified: a consumer calling
  `http::perform` now gets E0405). `extern fn` declarations needed no prefix —
  they are already module-private. The constructor takes its content with the
  rest defaulted, `Request::new(url, method: = Get, timeout_seconds: = 60.0)`,
  which collapses what would have been three constructors and makes `get(url)`
  literally `send(Request::new(url))`. `set_header(name, value)` became
  `set_header(name, to: value)` — "set header Accept to application/json", the
  `Vec::set(value, at:)` shape — because two same-typed positional strings are
  swappable at the call site and a label is the fix the language already has.

- facet (pass done 2026-08-03): the contract's 362 verbs are generated against
  this page rather than transcribed from MAUI. The rename table is
  `tools/maui_map.py`'s `RENAME` (48 rows) and the MAP is the naming authority
  — the generator only transcribes it. Three rules are enforced by a lint in
  `tools/gen_contract.py` that fails the run: a boolean setter drops the
  assertion prefix (`set_enabled(false)` / `is_enabled()`), a setter is at most
  26 characters, and a trailing noun that names the type rather than the role
  (`_mode` `_type` `_strategy` `_visibility` `_source` `_enabled`) is rejected.
  Exceptions live in `ALLOWED` with a reason each; there are two.
- facet's drawing vocabulary (added by hand 2026-08-04, from
  `Microsoft.Maui.Graphics.ICanvas` and `PathF`): the lint does not reach it,
  because the lint runs over the MAP and these rows were never in a manifest
  the extraction read. Named against this page directly, and the four places
  it bit are worth keeping:
  - MAUI's two `DrawString` overloads are not overloads in C+, and collapsing
    them with defaults would have been wrong — a point and a box are different
    things. They are `draw_text(at:)` and `draw_text_block(box:)`, named for
    the difference. `Rotate`'s two overloads DO collapse, into
    `rotate(degrees:, around: Point = zero)`, because there the second form is
    the first with a default.
  - `StrokeDashPattern` + `StrokeDashOffset` became one
    `set_stroke_dash(_, offset:)`: an offset without a pattern means nothing,
    so two verbs let a caller write half a state.
  - Trailing type-nouns went, as the lint would have required: `WindingMode` is
    `Winding`, `BlendMode` is `Blend`, `set_blend(v:)`.
  - Words facet already owns won over MAUI's: `StrokeSize` is
    `set_stroke_width` and `Alpha` is `set_opacity`, because `bordered` and the
    shared band already say it that way and one word should mean one thing.

## Earlier state (2026-06-24)

- stdlib: Swift-style naming and labeled parameters adopted. Consistency pass
  done (`Vec::set(value, at:)` matching `insert`; `swap_remove(at:)`;
  `truncate(to:)` on both `Vec` and `Text`; `env::arg(index:)`). First default
  values in use: `Text::drop_first(count: usize = 1)` / `drop_last(... = 1)`.
  Behavior-adding defaults (`find(from:)`, `split(max_splits:)`,
  `replacing(max:)`) not yet applied.
- json: partially aligned.
- appkit: thin ObjC binding; labeled parameters, default values, role-based
  type names, `Option` returns, and `Text` parameters are not yet applied.
- flex_layout (pass done 2026-07-31): `Layout` renamed `Frame` (named for what
  it is) and the scalar `layout_*` getters collapsed into `frame()` /
  `child_frame(at:)`; `calculate_layout(width:, height:, direction:)` fully
  defaulted (omitted axis = unconstrained, so the NaN sentinel stays internal);
  the owned payload renamed `attach` / `attachment() -> Option[*u8]` /
  `detach`; grid line naming folded into `add_grid_column(track, line:)` with
  `column_line(named:) -> Option[i32]`; `set_gap_length` merged into one
  `set_gap(gutter, length: StyleLength)`; `undef`/`is_undef`/`_pct`
  abbreviations spelled out; responsive `add_breakpoint(name, up_to:)`,
  `breakpoint_width() -> Option[f64]` (replaces the `breakpoint()` +
  `matched_breakpoint()` pair), `is_same_class`. `Node::new()` deliberately
  stays a bare constructor — ~30 style properties make a content-taking
  initializer worse, and the `@flex` DSL is the phrase-reading layer.
