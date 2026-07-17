# Guide

How the package is meant to be used, what it does and does not implement, and
the footguns. Fast start: [tutorial.md](tutorial.md). Signatures:
[ref.md](ref.md).

## What this package is

A small **RFC 4122** helper around a 16-byte value:

| Capability | Support |
|---|---|
| Generate | **v4 only** (random), via `/dev/urandom` |
| Parse | Canonical 8-4-4-4-12 hex string (36 chars) |
| Format | Same canonical form, **lowercase** hex |
| Nil | All-zero UUID (`nil` / `is_nil`) |
| Raw bytes | `from_bytes` / `bytes` / `byte(at:)` |

It is **not** a full UUID library: no v1/v3/v5/v6/v7, no URN prefix
(`urn:uuid:`), no brace form `{...}`, no Microsoft mixed-endian layout, no
Windows `BCryptGenRandom` path.

## When to use which constructor

| Need | Call |
|---|---|
| New random id | `Uuid::new_v4()` → handle `None` |
| Known string from wire/config | `Uuid::parse(s)` → handle `None` |
| Known 16 bytes (DB blob, peer format) | `Uuid::from_bytes([...])` |
| Placeholder / “unset” | `Uuid::nil()` and `is_nil()` |

### `from_bytes` does not fix version bits

`from_bytes` stores the array **as given**. It does not force version 4 or
the RFC variant. Use it for round-trips of already-valid ids, not as a
second random generator.

`new_v4` **does** mask bits per RFC 4122 §4.4:

- byte 6 high nibble → `4` (version)
- byte 8 high two bits → `10` (variant)

So a successful `new_v4` is never the nil UUID (those bits alone prevent
all zeros).

## Parsing rules

Accepted:

- length exactly **36**
- `-` at indices **8, 13, 18, 23**
- hex digits `0-9` `a-f` `A-F` in all other positions

Rejected → `None` (no partial parse, no error detail):

- wrong length
- dash in the wrong place (or missing)
- any non-hex nibble

Not accepted (by design today):

- no hyphens (`0123456789abcdef...` 32 hex)
- uppercase-only is fine; mixed is fine; braces / URN are not
- surrounding whitespace

`to_text` always emits lowercase. Round-trip holds for any successfully
parsed id: `parse(s).to_text()` matches the canonical lowercase form of the
same bytes (if `s` was already lowercase canonical, string equality holds).

## Formatting and ownership

`to_text` builds a 36-character canonical string and returns an owned
`text::Text` (heap). The intermediate format buffer is stack-only; the
caller owns the result and drops it as usual for `Text`.

There is no `to_str` that returns a borrowed view of internal storage —
the UUID holds raw bytes only, not a cached string.

## Entropy and platforms

`new_v4` opens `/dev/urandom`, reads 16 bytes (looping on short reads),
then applies version/variant masks.

| Outcome | Meaning |
|---|---|
| `Some(u)` | 16 random bytes + RFC masks applied |
| `None` | open failed, or read returned ≤ 0 before 16 bytes |

This is portable across **macOS and Linux**. Other hosts without
`/dev/urandom` get `None` until a platform backend is added. Treat `None`
as a real failure in production paths (log, abort start-up, or fall back
explicitly — do not silently use `nil` as a “random” id).

## Equality and identity

There is no `==` helper on `Uuid` in this package. Compare with:

- `is_nil()` for the zero id, or
- byte-wise: loop `byte(at: i)` / compare `bytes()` arrays yourself.

Two values with the same 16 bytes are the same UUID regardless of how they
were constructed (`parse` vs `from_bytes` vs `new_v4`).

## Gotchas

### `byte(at:)` is unchecked

`at` must be in `0..16`. Out-of-range is undefined (raw array index). Prefer
`bytes()` when you need the whole array.

### Parse is strict about shape, not about version

`parse` does **not** require version nibble 4 or a particular variant. A
string for a v1 UUID in canonical form parses fine. Only `new_v4` enforces
v4 bits.

### Do not use nil as a stand-in for “failed to generate”

```cplus
// bad: hides entropy failure
let id: uuid::Uuid = match uuid::Uuid::new_v4() {
    option::Option[uuid::Uuid]::Some(u) => u,
    option::Option[uuid::Uuid]::None => uuid::Uuid::nil(),
};
```

Callers that treat nil as “missing” will collide with “RNG failed.” Propagate
`Option` or fail loudly.

### Case on format vs parse

Parse accepts upper and lower hex; format always lowercases. Do not compare
`to_text()` to an uppercase source string without normalizing.

## Typical app patterns

**Id for a new record**

```cplus
guard let option::Option[uuid::Uuid]::Some(id) = uuid::Uuid::new_v4() else {
    // surface error
    return;
};
// store id.bytes() in DB, or id.to_text() as TEXT
```

**Id from config / URL**

```cplus
guard let option::Option[uuid::Uuid]::Some(id) = uuid::Uuid::parse(input) else {
    // 400 bad request
    return;
};
```

**Optional foreign key**

```cplus
// unset
var parent: uuid::Uuid = uuid::Uuid::nil();
// once known
parent = parsed;
if parent.is_nil() { /* no parent */ }
```
