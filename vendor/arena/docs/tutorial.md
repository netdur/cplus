# Tutorial

Quick path: create a heap arena, allocate, reset. Gotchas in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
arena = "*"
```

```cplus
import "arena/arena" as arena;
import "stdlib/option" as option;
```

## Allocate

```cplus
var a: arena::Arena = arena::Arena::new(4096usize);

// raw bytes (default 8-byte align)
let p: *u8 = a.alloc_bytes(64usize);
// check OOM: null pointer
if { #addr(p) } == 0usize { return; }

// Option form
guard let option::Option[*u8]::Some(q) = a.alloc_bytes_opt(16usize, zeroed: true) else {
    return;
};

// typed Copy value
let n: *i32 = a.alloc::[i32](123);
assert { *n } == 123;

// string copy → str view into the arena
guard let option::Option[str]::Some(s) = a.alloc_str("hello") else {
    return;
};
```

## Reset and free

```cplus
a.reset();   // free all chunks; arena reusable
// Drop also frees everything
```

## Day-one rules

- Check **null** from `alloc_bytes` / `alloc` (or use `alloc_bytes_opt` / `alloc_str`).
- Pointers and `str` views are valid only until **`reset`** or Drop.
- `alloc[T]` is for **Copy** `T` only — no per-slot Drop on free.
- Prefer **`static-arena`** when the budget is fixed and heap is forbidden.
