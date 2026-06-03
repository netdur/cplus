# C+ Raw-Pointer Accountability Plan (`opaque` marker + unaccounted-pointer error)

Status: **design complete, implementation deferred.** Polarity (§2), keyword
(§4), scope (§5), and the drop-body check (§6, "local direct release") are all
decided. What remains is error wording and the build itself (§8, §10). This
document records the design and its reasoning so it survives until a focused
implementation cycle.

This plan diverges from [plan.own.md](plan.own.md): it inverts that document's
default (§3) and replaces the `own` marker with `opaque`. Where the two
conflict, this document is the newer thinking; the owning-C+-type auto-drop
half of `plan.own.md` (§2.1 there) is orthogonal and unchanged.

## 1. Problem

The compiler can free a value only when it knows the value owns memory. For
owning C+ types (`string`, `Vec[T]`, `Box[T]`) the type carries that knowledge,
so auto-drop is sound. For a raw pointer (`*T`) the compiler is blind: a `*u8`
is an address, and the address does not say whether it is:

- **owned**: this struct allocated it and must release it, or
- **borrowed**: another party owns it and this struct must not touch it.

Both mistakes are real bugs in opposite directions: releasing a borrowed
pointer is a double-free, failing to release an owned one is a leak.

The earlier design ([plan.own.md](plan.own.md)) chose to assume *borrowed* for
an unmarked raw pointer and stay silent, with an opt-in `own` marker that only
warned (W0003) when an owned pointer had no `drop`. That never produces a false
nag, but it leaves the dangerous case silent: a struct that owns a raw pointer
and has no `drop` leaks, and nothing reports it. The compiler assumes; it does
not verify.

## 2. The decision

Invert the default. A raw pointer in a struct must be *accounted for*. If it is
not, the program does not compile.

```cplus
struct CameraFrame {
    buf: *u8,        // ERROR: unaccounted raw pointer
    width: i32,
    height: i32,
}
```

There are exactly three accounted shapes:

| Field shape | Result | Meaning |
| :--- | :--- | :--- |
| `buf: *u8` (no marker, no `drop`) | **error** | who frees this? unstated, possible leak |
| `buf: *u8` + a `drop` that releases it | compiles | this struct owns it and frees it, with the correct releaser |
| `opaque buf: *u8` | compiles | not this struct's responsibility; managed elsewhere |

The result: there is no silent-leak case for raw pointers. The compiler never
assumes; it forces the author to state ownership, then it checks the statement
to the depth chosen in §6.

## 3. Why guilty-by-default, against `plan.own.md`

`plan.own.md` chose innocent-by-default (unmarked raw is assumed borrowed). The
evidence collected since favors the inversion:

- A survey of struct raw-pointer fields across `vendor/` and `stdlib/` found
  about 109 such fields. The genuinely owned heap pointers among them
  (`Vec.ptr`, `Box.p`, `HashMap` storage, `Arc`/`Rc`/`Mutex` control blocks,
  arena pointers) **already have `drop` impls**. The "forgot to free" bug the
  warning was meant to catch barely occurs in current code.
- The large remainder (about 90) are FFI handles: ObjC `obj: *u8`, Metal
  `raw: *u8`, JNI reserved slots. These are not freed with libc `free`; their
  lifetimes are managed by a foreign runtime or another object. Under
  innocent-by-default they are silent (correct), but the model can never *nudge*
  about one that should have been released.
- The clarifying case is a borrowed buffer the struct reads but does not own:
  a live Android camera frame passed in for processing. The struct reads the
  pixels and must not free the buffer. This is the `opaque` case, and it must
  cost no annotation friction beyond one marker.

Guilty-by-default makes the silent-leak case (an owned raw pointer with no
`drop`) impossible, at the cost of requiring one marker on each genuinely
borrowed pointer. Given the field survey, that cost is the ~90 FFI fields
gaining `opaque`, which is a one-time mechanical migration.

`own` is no longer needed as a separate marker. Ownership is now expressed by
the presence of a `drop` (the shape that knows the correct releaser); a marker
that only *claims* ownership without releasing it adds nothing the default error
does not already force.

## 4. The keyword: `opaque`

The marker means "this raw pointer is not the struct's responsibility to
release; treat it as opaque to the ownership system." Rejected alternatives, and
why each reads as a lie at the use site:

- **`own`**: in the camera case the struct does not own the buffer, so `own buf`
  asserts the opposite of the truth. It is also the antonym of this marker's
  meaning, and collides with `plan.own.md`'s opposite use of the same word.
- **`borrow`**: accurate in meaning, but `borrow` already names the parameter
  marker (shared, read-only, caller keeps ownership). Reusing it on fields
  blurs that single clear meaning.
- **`free`**: reads as "release this," the opposite of "do not touch it." On a
  borrowed camera buffer it is the most dangerous possible reading, and if it
  were load-bearing it would free memory the struct does not own. As an
  auto-release marker for the *owned* case it fails differently: it hardcodes
  libc `free` as the releaser, which is wrong for the ObjC / JNI / `close` /
  custom-allocator majority. The compiler cannot know the releaser, which is the
  same reason `plan.own.md` made its marker check-only.

`opaque` describes inspectability rather than ownership, which is an imperfect
fit for a readable data buffer, but it does not assert a falsehood, and most
target fields (FFI handles) are genuinely opaque. Programming keywords commonly
take meaning from use rather than dictionary sense (`static`, `volatile`,
`extern`); the requirement is that the word not read as a lie, which `opaque`
satisfies and `own` / `free` do not.

## 5. Scope

- The rule applies to raw pointer (`*T`) struct fields only.
- Owning C+ types (`string`, `Vec[T]`, `Box[T]`, Drop structs) are unaffected:
  the compiler already knows they own heap and auto-drops them. They never take
  a marker. The `plan.own.md` §2.1 auto-field-drop for these types is
  complementary to this plan and proceeds independently.
- `opaque` on a non-raw field is redundant and need not be diagnosed.

## 6. Drop-body check: local direct release (decision)

The §2 error forces an owned raw pointer to *have* a `drop`. It does not by
itself prove the `drop` *releases* that pointer. The remaining honest mistake is
a `drop` that omits a field:

```cplus
struct Frame { buf: *u8, extra: *u8 }
impl Frame {
    fn drop(mut self) {
        unsafe { free(self.buf); }   // self.extra never released: leak
    }
}
```

### Why not "analyze the drop body"

The obvious approach is to *prove* each field is released. Proving "released on
every control-flow path" is **undecidable in general** (it can depend on runtime
values), and the two shapes that force that hard analysis are a release wrapped
in an arbitrary condition (`if cond { free(self.b) }`) and a release delegated to
another function (`self.cleanup()`). Catching those needs path-sensitive,
interprocedural reasoning.

### The C+ move: constrain the shape, don't analyze it

Those two shapes are exactly the ones that violate **local clarity**, the
principle the rest of the language is built on: you cannot tell, reading the
`drop` body, whether a conditionally-guarded or delegated free actually runs. So
the language *forbids* them, and the checker no longer proves anything across
paths or functions — it checks that a **direct, local release** is present. The
undecidable proof becomes a decidable structural check, at the cheap end of the
cost scale, with no missed-path case left because hidden paths are illegal.

The clarity violation is not "a condition"; it is "a condition that hides whether
the free runs." A null-check on the field being freed does not hide it (the free
runs exactly when there is something to free), and some releasers require it
(`CFRelease(NULL)` crashes). So a null-guard is admitted as a second legal shape,
not an exception.

### The rule: two accepted shapes

Each owned (non-`opaque`) raw-pointer field must reach a release through exactly
one of:

```cplus
// Shape 1 — unconditional direct release
fn drop(mut self) { unsafe { free(self.b); } }

// Shape 2 — release directly guarded by a null-check on the same field
fn drop(mut self) {
    if self.b.is_not_null() { unsafe { free(self.b); } }   // or `if !self.b.is_null()`
}
```

Rejected with **E0510**:

```cplus
if self.ready { unsafe { free(self.b); } }   // arbitrary condition: hides whether it runs
for ... { unsafe { free(self.b); } }          // nested in a loop
self.cleanup();                              // delegated to another function
// ...and a field with no release of any kind
```

The check is a fixed two-shape pattern match: the field appears as an argument to
a direct call that is either a statement at the top level of the `drop` body, or
the sole content of a single `if` whose condition is a null-test on that same
field. No dataflow, no interprocedural walk.

### What counts as "a release"

A release is recognized **structurally**: the field is passed as an argument to a
direct call (in one of the two shapes). The compiler does **not** keep a registry
of releaser functions (`free` / `close` / `objc_release` / `CFRelease` / custom
allocator frees), for the same reason `opaque` is check-only — it cannot know
every releaser. The consequence: a deliberate non-release that still passes the
field to a direct call, e.g. `unsafe { printf("%p", self.b); }`, satisfies the
check without freeing. This is accepted as author responsibility, the same family
as a false `opaque` or any `unsafe`: it is **local and visible**, so a reader or
auditor sees `printf(self.b)` right there. The rule guarantees the release is
*present and clear*, not that it is semantically a free.

A releaser registry (a function-level marker stating "this parameter is released
by this call" — metadata, never a generated call, so it stays on the right side
of the attributes-are-pure-metadata principle) could upgrade the check to catch
the `printf` case later. It is deliberately **out of the first cut**.

### Result

- **Decidable + cheap**: two syntactic shapes, a pattern match.
- **Complete for legal code**: no hidden-path case remains, because hidden paths
  are rejected.
- **Clear**: the release and its only-legal guard are both in the `drop` body;
  nothing to trace.
- **Honest wording**: the rule is "a direct, optionally null-guarded release per
  owned field." The tutorial currently says the compiler "traces each owned field
  … on every path", which describes the rejected analysis approach; reword it to
  this rule when the feature lands.

## 7. Remaining leak taxonomy

With the §2 error and the §6 local-direct-release rule in place, the leak cases
are:

| Case | Example | Status |
| :--- | :--- | :--- |
| silent did-nothing leak | `buf: *u8`, no `drop`, no marker | **eliminated** (E0510) |
| `drop` omits a field | frees `buf`, not `extra` | **caught** (no direct release → E0510) |
| release hidden in a condition / helper | `if cond { free(self.b) }`, `self.cleanup()` | **rejected** (not a legal shape → E0510) |
| `drop` passes the field to a non-releaser | `printf("%p", self.b)` | author responsibility (visible; a releaser registry could catch it later) |
| false `opaque` | marks an owned pointer `opaque` | author responsibility (escape hatch) |
| mid-life overwrite | `self.buf = a; self.buf = b;` first lost | not modeled (runtime logic) |
| raw pointer local, not a field | `let p = malloc(); ...` no `free` | out of scope (function-local, `unsafe`) |

The model's guarantee is precise: it eliminates the silent did-nothing leak *and*
the omitted-field and hidden-release leaks (the last two by forbidding the unclear
shapes rather than analyzing them). What remains is the author passing a field to
something that is visibly not a release, which is local and auditable — the same
trust boundary as `unsafe`.

## 8. Implementation surface

Localized, mirroring the marker machinery for `restrict` / `move` / `borrow`.

1. **Lexer**: add `opaque` as a keyword (reserve like `restrict`).
2. **AST**: add a per-field `is_opaque` flag to `StructField`.
3. **Parser**: accept optional `opaque` in the field-prefix position
   (attrs, `pub`, then `opaque`, then name).
4. **Sema**: the §2/§6 check. For each struct, for each raw-pointer field that is
   not `opaque`, require that the `drop` body contains a direct release of that
   field in one of the two §6 shapes (unconditional, or guarded by a null-test on
   the same field); otherwise emit **E0510**. The check is a structural pattern
   match over the `drop` body — no dataflow, no interprocedural walk.
5. **Diagnostics**: register **E0510**.
6. **Tests**: positive (unconditional release; null-guarded release; `opaque`
   present), negative (bare raw pointer with no release; release hidden in an
   arbitrary condition; release delegated to a helper; omitted field), per the
   project's test discipline.

Two `StructField` construction sites need the new field (parser, monomorphize).
No existing source identifier is named `opaque`, so reserving it as a keyword is
safe.

## 9. Migration

About 90 raw-pointer FFI-handle fields (ObjC, Metal, JNI) currently have neither
a marker nor a `drop`. Each gains `opaque` (or, where the struct should release
the handle, a `drop` calling the correct foreign releaser). One-time and
mechanical. The genuinely owned heap structs already have `drop` impls and are
unaffected.

## 10. Decisions and remaining questions

Resolved:

- **Polarity**: guilty-by-default — an unaccounted raw pointer is a compile error
  (§2).
- **Keyword**: `opaque` (§4).
- **Drop-body check**: local direct release, two accepted shapes (§6). This
  replaces the earlier "how deep to analyze (Level 1–4)" framing — the answer was
  to constrain the `drop` body rather than analyze it.
- **Releaser recognition**: structural (field passed to a direct call); no
  releaser registry in the first cut (§6).

Remaining:

- **Error wording.** The unaccounted-pointer case, the omitted/missing-release
  case, and the hidden-release (illegal shape) case all surface as **E0510**;
  finalize whether they share one message with variants or split into sibling
  codes.
- **Null-guard spelling.** Accept `if self.f.is_not_null()` and
  `if !self.f.is_null()`; confirm no third spelling needs recognizing.
- **Releaser registry** remains a possible later upgrade to catch the
  "passed to a non-releaser" case (§6, §7), deliberately out of the first cut.
