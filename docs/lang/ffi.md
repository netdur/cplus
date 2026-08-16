# FFI — the C boundary

The decision this page settles: **what crossing into or out of C costs, and
which shape to use at the boundary.** Signatures in [ref.md](ref.md);
ownership fundamentals in [ownership.md](ownership.md).

## 1. The declaration is the marker

There is no `unsafe` block. Every operation that can invoke undefined
behavior is already loud at the point of use: `*p` / `p[i]` are the only
derefs, `x as *T` is the only pointer constructor, `#addr(p)` is the loud
pointer→integer read, and a foreign call cannot appear without an
`extern fn` declaration in scope. The declaration is the audit point; the
call site stays bare.

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);

let p: *u8 = malloc(64 as usize);
p[0] = 65 as u8;                    // deref-write: visible
let q: *u8 = p + 1;                 // pointer arithmetic strides by sizeof(T)
free(p);
```

Null is `0 as *T` on the way in, and `p.is_null()` / `p.is_not_null()` on
the way out — blessed on every raw pointer and fn-pointer, a single compare,
no memory access. Pointer ↔ integer conversion goes through `usize`, never
directly to a narrower type (E0315).

## 2. The three traps that outrank everything else

1. **Variadics must be declared variadic.** On AArch64-Darwin, named
   arguments ride registers and varargs ride the stack — a fixed-arity
   declaration of `fcntl` compiles, calls, and passes garbage with no
   diagnostic. `extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;` — the
   `...` is load-bearing.
2. **`str` does not cross.** It is a fat pointer with no C counterpart —
   rejected in `export` signatures (E0410). Cross as `*u8` + `usize` and
   rebuild the view on the other side (`#str_from_raw_parts`), or hand C a
   NUL-terminated `c"..."` literal / `#str_ptr` of a NUL-terminated string.
3. **Only `export` is callable from outside.** Everything else has
   module-mangled names and an internal calling convention — an lldb
   `call`, a C declaration, or a test harness reaching an internal symbol
   passes garbage *silently*. Need to drive an internal path from outside?
   Write the two-line `export` wrapper. That is what it is for.

## 3. Layout: making a struct C-true

```cplus
#[repr(C)] struct NSPoint { x: f64, y: f64 }            // C field order + padding
#[repr(C, packed)] struct Wire { kind: u8, len: u32 }   // no padding: size 5, align 1
#[repr(C, packed = 2)] struct Legacy { a: u8, b: u32 }  // #pragma pack(2)

#[repr(C)] struct Flags {
    #[bits(3)] kind: u32,        // bitfields, C's exact unit/straddle rules
    #[bits(5)] level: u32,
    #[bits(4)] delta: i32,       // signed: sign-extends on read
}

#[repr(C)] union FloatBits { f: f32, bits: u32 }        // one storage, typed views
```

Rules that follow from C's rules: a bitfield or under-aligned packed field
has **no address** — no `ref`, no `#addr_of` (E0926/E0927); copy to a local
first. Union members must be Copy (no tag means no destructor can run).
Unions are for *binding headers*; an either/or value in ordinary code is an
enum, which has a tag.

Payload-free enums cross with a pinned width:

```cplus
#[repr(u8)] enum Mode { Off = 0, Slow = 10, Fast = 200 }   // uint8_t on the wire
```

Discriminants take constant expressions and C's prev+1 rule. Tagged enums
(payload-carrying) never cross the boundary — convert at the edge.

## 4. Ownership at the boundary

The borrow checker's writ ends at `extern fn`. What replaces it is a
convention you write down:

- **A struct field holding a C pointer must declare its owner** (E0510):
  a `drop` that frees it, or `opaque` meaning "C frees this, not me"
  ([ownership.md](ownership.md) §7).
- **A fn-pointer parameter follows the same grammar as everything else**:
  `fn(R)` borrows its argument, `fn(take R)` consumes it (E0312 keeps the
  two apart). Registering a C callback that will *keep* the payload means
  declaring the pointer type with `take`.
- **By-value parameters borrow; only `take` consumes** — including in
  callbacks that C invokes. The over-release bugs all come from assuming
  the Rust rule; C+ is explicit instead.

The sanitizers cover what the checker can't: `cpc build --asan` (and
`--ubsan`, `--tsan`, `--msan`) instrument cpc-emitted code exactly as clang
instruments C, and they are the tool for the raw tier.

## 5. Calling out: symbols and linking

```cplus
#[link_name = "objc_msgSend"] extern fn msg_ptr(r: *u8, s: *u8) -> *u8;
```

`extern fn` names are module-scoped in C+ — `#[link_name]` pins the actual
linker symbol, which is also how one symbol gets several typed C+ faces
(the `objc_msgSend` pattern). The libraries themselves come from the
manifest's `[link]` table — `frameworks`, `libs`, `search-paths`,
`extra-objects` — and a dependency's `[link]` travels with it
([packages.md](packages.md) §5).

For Objective-C, the language carries a typed tier so bindings don't hand-
roll casts: `#selector("name")` (cached SEL) and
`#msg_send(recv, "sel", args...) -> RetTy`. The `objc` / `appkit` / `uikit`
vendor packages are generated on top of exactly this.

## 6. Calling in: `export`

```cplus
export fn probe_emit(name_ptr: *u8, name_len: usize) {
    let name: str = { #str_from_raw_parts(name_ptr, name_len) };
    events::emit(name);
    return;
}

export extern fn app_main() -> i32 {   // an app entry the platform shell calls
    runtime::run_component(App::new());
    return 0;
}
```

`export` gives a function a stable, unmangled name, the C calling
convention, and a line in the generated header (`target/<…>/<name>.h`, or
`cpc --emit-header`). Signatures must be C-representable (E0410): integers,
floats, raw pointers, `#[repr(C)]` aggregates, repr'd enums — no `str`, no
tagged enums, no SIMD vectors (round-trip SIMD via `[f32; N]`).

`export extern fn` is the app-entry flavor: on external-builder platforms
(iOS, Android) the platform shell — `main.m`, JNI — calls it, and the build
system links your archive under it ([packages.md](packages.md) §3).

## 7. Embedding data instead of loading it

`#include_bytes("path")` compiles a file into the binary as `*[u8; N]`;
`#compile_shader("k.metal", "msl")` runs the platform shader compiler at
build time and embeds the result. Both remove a filesystem dependency from
the shipped artifact — the FFI-adjacent trick that most often replaces a C
asset pipeline.
