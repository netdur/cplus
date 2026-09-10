# Tutorial

Quick path: stack arena, allocate, reset. Gotchas in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
static-arena = "*"
stdlib = "*"
```

```cplus
import "static-arena/static-arena" as sa;
import "stdlib/option" as option;
```

## 16 KiB or 64 KiB

```cplus
var a: sa::StaticArena16K = sa::StaticArena16K::new();
// or: StaticArena64K::new()
```

## Allocate

```cplus
guard let option::Option[*u8]::Some(p) = a.alloc_bytes(64usize) else {
    return;   // does not fit
};

guard let option::Option[*i32]::Some(n) = a.alloc::[i32](7) else {
    return;
};

guard let option::Option[str]::Some(s) = a.alloc_str("hi") else {
    return;
};

let left: usize = a.remaining();
a.reset();    // full capacity again; old pointers invalid
```

## Day-one rules

- Overflow → **`None`** (cursor unchanged on failed `alloc_bytes`).
- No heap — good for `#[no_alloc]` / fixed budgets.
- `alloc[T]` is **Copy-only**; reset does not Drop slots.
- Pointers/`str` die at **`reset`** or when the arena goes out of scope.
- Prefer **`static`** storage for large arenas; stack `new()` of huge
  buffers can overflow (see guide). Need more than 64 KiB or growth →
  package **`arena`**.
