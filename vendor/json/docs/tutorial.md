# Tutorial

Quick path: parse JSON, read fields, build a value, serialize. Gotchas in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
json = "*"
stdlib = "*"
```

```cplus
import "json/json" as json;
import "stdlib/option" as option;
import "stdlib/result" as result;
import "stdlib/text" as text;
import "stdlib/vec" as vec;
```

## Parse

```cplus
let r: result::Result[json::Value, json::ParseError] =
    json::Value::parse(source: "{\"ok\":true,\"n\":1}");

guard let result::Result[json::Value, json::ParseError]::Ok(root) = r else {
    // err.offset is the byte where parsing stopped
    return;
};
```

## Read

Missing key or wrong type → `None`. Present JSON `null` → `Some(Null)`:

```cplus
guard let option::Option[json::Value]::Some(ok_v) = root.value(for_key: "ok") else {
    return;
};
guard let option::Option[bool]::Some(ok) = ok_v.as_boolean() else {
    return;
};

guard let option::Option[json::Value]::Some(n_v) = root.value(for_key: "n") else {
    return;
};
guard let option::Option[f64]::Some(n) = n_v.as_number() else {
    return;
};
```

Arrays:

```cplus
// root is [10, 20]
let count: usize = root.item_count();
guard let option::Option[json::Value]::Some(first) = root.item(at: 0usize) else {
    return;
};
```

## Build and serialize

```cplus
var members: vec::Vec[json::Member] = vec::new::[json::Member]();
members.append(json::Member::new("enabled", json::Value::boolean(true)));
members.append(json::Member::new("count", json::Value::number(3.0f64)));
let obj: json::Value = json::Value::object(members);

let out: text::Text = obj.to_text();   // compact: {"enabled":true,"count":3}
```

Constructors: `null()`, `boolean(value:)`, `number(value:)`, `text(value:)`,
`array(values:)`, `object(members:)`.

## Day-one rules

- Accessors return **owned clones** — the parent `Value` stays valid.
- `null` in JSON ≠ missing key (`Some(Null)` vs `None`).
- Serialize is **compact** only (no pretty-print).
- Trailing commas and junk after the value are parse errors.
