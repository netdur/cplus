# json

Typed JSON parse and serialize for C+.

```toml
[dependencies]
json = "*"
```

```cplus
import "json/json" as json;
import "stdlib/option" as option;
import "stdlib/result" as result;
import "stdlib/text" as text;

let parsed: result::Result[json::Value, json::ParseError] =
    json::Value::parse(source: "{\"name\":\"Ada\"}");

guard let result::Result[json::Value, json::ParseError]::Ok(value) = parsed else {
    return 1;
};
guard let option::Option[json::Value]::Some(name_value) = value.value(for_key: "name") else {
    return 1;
};
guard let option::Option[text::Text]::Some(name) = name_value.as_text() else {
    return 1;
};
```

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests live in `src/json.cplus`.

```
cd vendor/json && cpc test
```
