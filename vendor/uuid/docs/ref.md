# Reference

Manual for the `uuid` package. Signatures and behavior only.

```cplus
import "uuid/uuid" as uuid;
```

RFC 4122-oriented UUID value: 16 bytes, v4 generation, canonical
parse/format. Depends on `stdlib` (`option`, `text`).

---

## `Uuid`

```cplus
struct Uuid {
    _bytes: [u8; 16],   // private
}
```

Opaque 16-byte id. Construct with the associated functions below; read with
`byte` / `bytes` / `is_nil` / `to_text`.

---

### `Uuid::nil`

```cplus
fn nil() -> Uuid
```

The nil UUID: all sixteen bytes zero (RFC 4122 §4.1.7).

---

### `Uuid::from_bytes`

```cplus
fn from_bytes(bytes: [u8; 16]) -> Uuid
```

Build a UUID from raw bytes as given. Does **not** set version or variant
bits.

---

### `Uuid::new_v4`

```cplus
fn new_v4() -> option::Option[Uuid]
```

Generate a random RFC 4122 version-4 UUID.

- Reads 16 bytes from `/dev/urandom` (short reads retried until full or
  failure).
- Sets version nibble on byte 6 to `4`.
- Sets variant bits on byte 8 to `10xx`.
- Returns `None` if open or read fails.

---

### `Uuid::parse`

```cplus
fn parse(s: str) -> option::Option[Uuid]
```

Parse a canonical UUID string into a `Uuid`.

**Requires:**

- `#str_len(s) == 36`
- `s[8]`, `s[13]`, `s[18]`, `s[23]` are `'-'`
- remaining characters are hex (`0-9`, `a-f`, `A-F`)

Returns `None` on any violation. Does not validate version/variant nibbles.
Does not accept unhyphenated, braced, or URN forms.

---

### `is_nil`

```cplus
fn is_nil(this) -> bool
```

`true` when every byte is zero (the nil UUID).

---

### `byte`

```cplus
fn byte(this, at: usize) -> u8
```

The byte at index `at`. Valid range is `0..16` (unchecked).

---

### `bytes`

```cplus
fn bytes(this) -> [u8; 16]
```

A copy of all sixteen bytes, in network/RFC order (byte 0 is the first
octet of the UUID string’s first group).

---

### `to_text`

```cplus
fn to_text(this) -> text::Text
```

Format as the canonical 36-character string:
`8 hex - 4 hex - 4 hex - 4 hex - 12 hex`, **lowercase** hex digits.
Returns an owned `text::Text`. Infallible for any `Uuid` value.

---

## Package

| | |
|---|---|
| Package name | `uuid` |
| Module path | `uuid/uuid` |
| Dependencies | `stdlib` (`option`, `text`) |
| Tests | `cpc test` (in `src/uuid.cplus`) |
| Platforms (v4) | macOS / Linux via `/dev/urandom` |
