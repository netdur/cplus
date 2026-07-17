# Tutorial

Quick path: depend, generate a v4 UUID, format it, parse one back. Deeper
notes in [guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
uuid = "*"
stdlib = "*"
```

```cplus
import "uuid/uuid" as uuid;
import "stdlib/option" as option;
import "stdlib/text" as text;
```

## Generate (v4)

```cplus
guard let option::Option[uuid::Uuid]::Some(id) = uuid::Uuid::new_v4() else {
    // /dev/urandom failed — rare on macOS/Linux
    return;
};
let s: text::Text = id.to_text();
```

`new_v4` returns `Option` because entropy can fail. Always handle `None`.

## Parse

Canonical form only: 36 characters, hyphens at 8 / 13 / 18 / 23, hex digits
(upper or lower):

```cplus
guard let option::Option[uuid::Uuid]::Some(id) =
    uuid::Uuid::parse("12345678-9abc-def0-1234-56789abcdef0") else {
    return;
};
```

## Nil and bytes

```cplus
let z: uuid::Uuid = uuid::Uuid::nil();
assert z.is_nil();

let raw: uuid::Uuid = uuid::Uuid::from_bytes([
    0x00u8, 0x11u8, 0x22u8, 0x33u8,
    0x44u8, 0x55u8, 0x66u8, 0x77u8,
    0x88u8, 0x99u8, 0xaau8, 0xbbu8,
    0xccu8, 0xddu8, 0xeeu8, 0xffu8,
]);
let b: u8 = raw.byte(at: 0usize);
let all: [u8; 16] = raw.bytes();
```

## Day-one rules

- Format is always lowercase hex with hyphens (`to_text`).
- Parse rejects wrong length, bad dashes, or non-hex — returns `None`.
- `new_v4` is Unix-oriented (`/dev/urandom`); not a Windows path today.
- `byte(at:)` expects index in `0..16` — no bounds check.
