# Reference

Manual for the `json` package. Signatures and behavior only.

```cplus
import "json/json" as json;
```

Public surface: `Value`, `Member`, `ParseError`. Internal parser/encoder
types (`Parser`, `Buf`, …) are not part of the API.

Depends on `stdlib` (`vec`, `result`, `option`, `text`).

---

## `Value`

```cplus
enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(text::Text),
    Array(vec::Vec[Value]),
    Object(vec::Vec[Member]),
}
```

Recursive JSON value. Owns nested heap data; dropped recursively.

---

### Constructors

#### `Value::null`

```cplus
fn null() -> Value
```

JSON `null`.

#### `Value::boolean`

```cplus
fn boolean(value: bool) -> Value
```

JSON boolean.

#### `Value::number`

```cplus
fn number(value: f64) -> Value
```

JSON number stored as `f64`.

#### `Value::text`

```cplus
fn text(value: str) -> Value
```

JSON string. Copies `value` into an owned `text::Text`.

#### `Value::array`

```cplus
fn array(take values: vec::Vec[Value]) -> Value
```

JSON array. Takes ownership of `values`.

#### `Value::object`

```cplus
fn object(take members: vec::Vec[Member]) -> Value
```

JSON object. Takes ownership of `members`. Order is insertion order.

---

### Parse / serialize

#### `Value::parse`

```cplus
fn parse(source: str) -> result::Result[Value, ParseError]
```

Parse one JSON value from `source`. On success, only trailing whitespace may
remain. On failure, `Err(ParseError { offset })` where `offset` is the byte
index at which parsing stopped.

Accepts standard JSON: objects, arrays, strings (with escapes and `\u`
including surrogate pairs), numbers, `true` / `false` / `null`. Rejects
trailing commas and non-JSON extensions.

#### `to_text`

```cplus
fn to_text(this) -> text::Text
```

Serialize to compact JSON (no insignificant whitespace). Numbers use a
shortest round-trip style when possible. Returns an owned `text::Text`.

---

### Predicates

Each returns whether `this` is exactly that variant.

```cplus
fn is_null(this) -> bool
fn is_boolean(this) -> bool
fn is_number(this) -> bool
fn is_text(this) -> bool
fn is_array(this) -> bool
fn is_object(this) -> bool
```

---

### Accessors

All borrow `this`. Absence or type mismatch yields `None`. Nested `Value` /
`Text` results are **clones** (caller owns them; parent unchanged).

#### `item_count`

```cplus
fn item_count(this) -> usize
```

Array length, object member count, or `0` for scalars / null.

#### `as_boolean`

```cplus
fn as_boolean(this) -> option::Option[bool]
```

`Some` if `Bool`, else `None`.

#### `as_number`

```cplus
fn as_number(this) -> option::Option[f64]
```

`Some` if `Number`, else `None`.

#### `as_text`

```cplus
fn as_text(this) -> option::Option[text::Text]
```

Cloned string if `Text`, else `None`.

#### `item`

```cplus
fn item(this, at: usize) -> option::Option[Value]
```

Cloned array element at `at`, or `None` if not an array or out of range.

#### `value`

```cplus
fn value(this, for_key: str) -> option::Option[Value]
```

Cloned object value for the first member whose key equals `for_key`, or
`None` if not an object or key missing. A present JSON `null` is
`Some(Null)`, not `None`.

#### `key`

```cplus
fn key(this, at: usize) -> option::Option[text::Text]
```

Cloned key of the `at`-th object member (insertion order), or `None`.

#### `object_value`

```cplus
fn object_value(this, at: usize) -> option::Option[Value]
```

Cloned value of the `at`-th object member, or `None`.

---

## `Member`

```cplus
struct Member {
    key: text::Text,
    value: Value,
}
```

One object field.

#### `Member::new`

```cplus
fn new(key: str, take value: Value) -> Member
```

Copies `key` into owned `Text` and takes `value`.

---

## `ParseError`

```cplus
struct ParseError {
    offset: usize,
}
```

Parse failure. `offset` is the byte index into the source string where the
parser stopped (not a line/column pair).

---

## Package

| | |
|---|---|
| Package name | `json` |
| Module path | `json/json` |
| Dependencies | `stdlib` (`vec`, `result`, `option`, `text`) |
| Tests | `cpc test` (in `src/json.cplus`) |
