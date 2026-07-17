# uuid

RFC 4122 UUID: generate v4, parse, format, and inspect 16-byte ids.

```toml
[dependencies]
uuid = "*"
```

```cplus
import "uuid/uuid" as uuid;
```

## Common case

```cplus
guard let option::Option[uuid::Uuid]::Some(id) = uuid::Uuid::new_v4() else {
    return;
};
let s: text::Text = id.to_text();   // "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"
```

Parse a canonical string:

```cplus
guard let option::Option[uuid::Uuid]::Some(id) =
    uuid::Uuid::parse("12345678-9abc-def0-1234-56789abcdef0") else {
    return;
};
```

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

```
cpc test
```
