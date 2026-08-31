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

## Current state (2026-08-31)

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
