# arena

Heap bump allocator: append-only allocations, free everything on `reset` / Drop.

```toml
[dependencies]
arena = "*"
```

```cplus
import "arena/arena" as arena;

var a: arena::Arena = arena::Arena::new(4096usize);
let p: *u8 = a.alloc_bytes(64usize);
let n: *i32 = a.alloc::[i32](42);
```

Fixed-size, no-malloc sibling: **`static-arena`**.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests live in `src/arena.cplus`.

```
cd vendor/arena && cpc test
```
