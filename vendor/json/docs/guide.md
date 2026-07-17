# Guide

How to use the typed JSON tree, why it is shaped this way, and the footguns.
Fast start: [tutorial.md](tutorial.md). Signatures: [ref.md](ref.md).

## Model

`Value` is a recursive tagged enum:

| Variant | JSON | Payload |
|---|---|---|
| `Null` | `null` | — |
| `Bool` | `true` / `false` | `bool` |
| `Number` | number | `f64` |
| `Text` | string | owned `text::Text` |
| `Array` | `[...]` | owned `Vec[Value]` |
| `Object` | `{...}` | owned `Vec[Member]` |

`Member` is `{ key: Text, value: Value }`. Objects keep **insertion order**.
Lookup is **linear** over members (`value(for_key:)`), not a hash map —
`Value` is non-`Copy` (it owns heap data), and this shape is fine for normal
payloads. Hundreds of keys on one object is the scale where a different
layout would matter.

Drop is recursive: when a `Value` goes out of scope, nested arrays/objects
and strings free with it. No manual free API.

## Parse contract

```cplus
Value::parse(source: str) -> Result[Value, ParseError]
```

- One complete JSON value, then only optional whitespace to end of `source`.
- Extra tokens after the value → `Err` with `offset` at the leftover byte.
- Empty input → error.
- Trailing commas in arrays/objects → error (strict JSON).
- `ParseError.offset` is a **byte** index into `source` where parsing stopped.

Whitespace skipped between tokens: space, tab, LF, CR (standard JSON set).

### Strings

- Escapes: `\"` `\\` `\/` `\b` `\f` `\n` `\r` `\t` and `\uXXXX`.
- UTF-16 surrogate pairs (`\uD83D\uDE00`) combine into one Unicode scalar
  and are stored as UTF-8 (including 4-byte sequences).
- Lone surrogates are kept as code unit bits (3-byte UTF-8), so data can
  still round-trip rather than being rejected.

### Numbers

Stored as `f64`. Parse:

- Integer-shaped tokens with ≤15 digits use a fast path (exact in `f64`).
- Everything else goes through `strtod` (fractions, exponents, big ints).
- Leading `+`, bare `.`, and similar non-JSON shapes fail or fall through
  the scanner rules — stick to standard JSON number grammar.

Serialize (`to_text`) aims for **shortest round-trip**:

- Integers print as integers (`42`, not `42.0` or `4.2e1`).
- Simple decimals prefer fixed form (`0.1`, not noisy `%.17g`).
- Extreme magnitudes fall back to scientific / high precision so
  `parse(to_text(v))` preserves the `f64` bits when possible.

JSON has no integer type separate from number — if you need exact big
integers, this package is the wrong layer.

## Build contract

| Constructor | Notes |
|---|---|
| `null()` / `boolean` / `number` / `text` | scalars; `text` copies `str` into owned `Text` |
| `array(take values)` | takes ownership of the `Vec` |
| `object(take members)` | takes ownership of the `Vec[Member]` |
| `Member::new(key, take value)` | key copied to `Text`; value moved in |

There is no in-place “set field on object” mutator on `Value`. To grow an
object or array, build a `Vec`, `append`, then wrap with `object` / `array`
(or match out the vec, mutate, wrap again — see package tests).

## Accessors and ownership

All read APIs **borrow** the receiver and return:

- `Option` for absence or type mismatch, never a trap;
- **owned clones** for nested `Value` / `Text` so the parent stays usable.

That means:

```cplus
let a: Option[Value] = root.value(for_key: "a");
let b: Option[Value] = root.value(for_key: "b"); // root still valid
```

Cost: deep clone of the subtree you pull out. Prefer pulling once and
reusing the clone when walking large trees.

### Null vs missing

| Situation | Result of `value(for_key:)` |
|---|---|
| Key absent | `None` |
| Key present, JSON `null` | `Some(Value::Null)` — `is_null()` is true |
| Not an object | `None` |

Same idea for arrays: out of range → `None`; an element that is JSON
`null` → `Some(Null)`.

### Type predicates vs `as_*`

- `is_number()` / `is_text()` / … — cheap tag checks.
- `as_number()` / `as_text()` / `as_boolean()` — payload or `None` on
  mismatch. Wrong type is `None`, not a conversion (`"1"` is not a number).

`item_count()` is array length, object member count, or `0` for scalars.

### Object order APIs

`key(at:)` and `object_value(at:)` walk members in **insertion / parse
order**. Use them to iterate without knowing keys in advance. Duplicate
keys: `value(for_key:)` returns the **first** match.

## Serialize

`to_text() -> Text` emits compact JSON (no spaces, no pretty-print).

- Object keys and string values are escaped (`"`, `\`, controls as `\u00XX`).
- Member order is preserved.
- No streaming writer — the whole document is built in memory.

## What this package is not

- Schema validation / serde-style derive
- JSON5 / comments / trailing commas
- Partial parse or multi-value streams (NDJSON)
- Pretty-printer or configurable indent
- Integer or decimal types beyond `f64`
- Path queries (`$.a.b[0]`) — walk with `value` / `item` yourself

## Gotchas

### Deep clones on every accessor

Pulling a large array element clones the element. In hot loops, parse once
and match on the structure if you need zero-copy walks (today’s public API
is clone-on-read).

### Linear key lookup

`value(for_key:)` is O(n) in member count. Fine for configs and API
payloads; not a document database.

### Numbers are binary floats

`0.1 + 0.2` style issues apply. Round-trip formatting is careful; exact
decimal money is not this package’s job.

### Whole-source parse

`parse` does not stop after the first value and ignore the rest — leftover
non-whitespace is an error. Good for files and request bodies; for
concatenated values, split externally first.

### Building from match

`array` / `object` **take** the vec. After `Value::array(v)`, `v` is moved.
To append later, match the `Array` / `Object` variant out, mutate, reconstruct.

## Typical patterns

**Config field**

```cplus
guard let Result[Value, ParseError]::Ok(root) = Value::parse(source: body) else {
    return;
};
guard let Option[Value]::Some(port_v) = root.value(for_key: "port") else {
    return;
};
guard let Option[f64]::Some(port) = port_v.as_number() else {
    return;
};
```

**Emit a response object**

```cplus
var m: vec::Vec[Member] = vec::new::[Member]();
m.append(Member::new("status", Value::text("ok")));
m.append(Member::new("id", Value::number(id as f64)));
let body: text::Text = Value::object(m).to_text();
```
