# static-arena

Fixed-size bump allocator in the struct itself — **no malloc**. Stack or
`static` storage; `Option` on overflow.

```toml
[dependencies]
static-arena = "*"
```

```cplus
import "static-arena/static-arena" as sa;

var a: sa::StaticArena16K = sa::StaticArena16K::new();
guard let option::Option[*u8]::Some(p) = a.alloc_bytes(64usize) else {
    return;
};
```

Growing heap sibling: **`arena`**.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests live in `src/static-arena.cplus`.

```
cd vendor/static-arena && cpc test
```
