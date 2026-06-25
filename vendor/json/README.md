# json

Typed JSON parsing and serialization for C+.

## Usage

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

## API

- Parse with `Value::parse(source:)`. Parse failures return `ParseError`, whose
  `offset` is the byte position where parsing stopped.
- Build values with `Value::null`, `boolean(value:)`, `number(value:)`,
  `text(value:)`, `array(values:)`, and `object(members:)`. Build object
  members with `Member::new(key:, value:)`.
- Inspect variants with `is_null`, `is_boolean`, `is_number`, `is_text`,
  `is_array`, and `is_object`.
- Read potentially absent values with `as_boolean`, `as_number`, `as_text`,
  `item(at:)`, `value(for_key:)`, `key(at:)`, and `object_value(at:)`. These
  return `Option` rather than a sentinel value or null pointer.
- Serialize compact JSON with `value.to_text()`.
