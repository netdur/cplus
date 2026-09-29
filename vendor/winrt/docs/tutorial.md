# Tutorial

For ownership rules see [guide.md](guide.md); signatures are in [ref.md](ref.md).

Add `winrt = "*"` and `stdlib = "*"` under `[dependencies]`, then import
`"winrt/runtime"` and `"stdlib/result"`.

```cplus
let greeting: rt::HString = match rt::HString::new("Hello, 日本語") {
    result::Result::Ok(value) => { value },
    result::Result::Err(error) => { return error.code; }
};
```

`greeting` owns the UTF-16 HSTRING; its destructor releases it. Generated
WinUI string setters accept `str` directly and do this conversion for you.

Before activating WinRT objects, create an apartment:

```cplus
let apartment: rt::Apartment = match rt::Apartment::sta() {
    result::Result::Ok(value) => { value },
    result::Result::Err(error) => { return error.code; }
};
```

Keep it alive until all objects created in the apartment have been released.
Use generated constructors and `Interface::query(object)` for normal calls.
`Object::clone()` creates an additional owned COM reference. A generated
getter returning `Object` can represent native null; check `is_null()`.

For a runnable UI example, use
[winui_standalone](../../../examples/winui_standalone/README.md).
