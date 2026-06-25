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
appkit, and the rest). The declarative, SwiftUI-style UI layer is `facet` and is
out of scope here; adopting Swift's guidelines for imperative APIs is a separate
concern from `facet`.

## Current state (2026-06-24)

- stdlib: Swift-style naming and labeled parameters adopted. Consistency pass
  done (`Vec::set(value, at:)` matching `insert`; `swap_remove(at:)`;
  `truncate(to:)` on both `Vec` and `Text`; `env::arg(index:)`). First default
  values in use: `Text::drop_first(count: usize = 1)` / `drop_last(... = 1)`.
  Behavior-adding defaults (`find(from:)`, `split(max_splits:)`,
  `replacing(max:)`) not yet applied.
- json: partially aligned.
- appkit: thin ObjC binding; labeled parameters, default values, role-based
  type names, `Option` returns, and `Text` parameters are not yet applied.
