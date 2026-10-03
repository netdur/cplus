# Reference

Import `"winrt/runtime" as rt`. See [tutorial.md](tutorial.md) and
[guide.md](guide.md) for usage and lifetime rules. All results below use
`stdlib/result::Result`; `Text` is `stdlib/text::Text`.

## Values and owners

| API | Behavior |
|---|---|
| `Guid { a: u32, b: u16, c: u16, d: [u8; 8] }` | C layout, 16 bytes, alignment 4. |
| `Error { code: i32 }` | HRESULT failure. |
| `Status { code: i32 }` | HRESULT, including successful informational values. |
| `Status.is_ok() -> bool`, `.is_err() -> bool` | Sign-based HRESULT checks. |
| `Object::adopt(pointer: *u8) -> Object` | Takes ownership of one +1 reference; accepts zero. |
| `Object::retain(pointer: *u8) -> Object` | Adds one reference; accepts zero. |
| `Object::empty() -> Object` | Native null owner. |
| `Object.raw() -> *u8`, `.is_null() -> bool` | Borrowed pointer and null check. |
| `Object.clone() -> Object` | Adds one reference. |
| `Object.query(iid: Guid) -> Result[Object, Error]` | QueryInterface; rejects a null source or successful null output. |
| `Object.query_nullable(iid: Guid) -> Result[Object, Error]` | Preserves a null source; otherwise QueryInterface. |
| `HString::new(value: str) -> Result[HString, Error]` | Strict UTF-8 to owned UTF-16 conversion. |
| `HString::adopt(value: usize) -> HString` | Takes ownership of an HSTRING handle. |
| `HString.raw() -> usize` | Borrowed handle. |
| `HString.to_text() -> Result[Text, Error]` | Strict UTF-16 to owned UTF-8 conversion. |
| `Apartment::sta() -> Result[Apartment, Error]` | RoInitialize on Windows x64; balances success with RoUninitialize on drop. |
| `Composition.object() -> Object`, `.inner() -> Object` | Additional owned references to the composed instance and nondelegating inner. |

`Object`, `HString`, and `Apartment` have automatic destructors; do not
manually release their owned handles. `Composition` owns both returned objects.

## Activation

```cplus
fn activation_factory(name: str, iid: Guid) -> Result[Object, Error];
fn activate(name: str) -> Result[Object, Error];
fn compose(name: str, iid: Guid, method: usize, outer: *u8) -> Result[Composition, Error];
fn load_runtime(path: str) -> Status;
```

Factories use RoGetActivationFactory; activation uses RoActivateInstance.
`compose` uses the specified factory slot with outer/inner ABI parameters.
`load_runtime` loads a DLL for the process lifetime; it never unloads it.
Deployment/registration is the caller's responsibility.

## Generator support / raw ABI

These functions require valid native pointers and the correct ABI:

```cplus
fn read_pointer(object: *u8, offset: usize) -> *u8;
fn write_pointer(object: *u8, offset: usize, value: *u8);
fn slot(object: *u8, index: usize) -> *u8;
fn add_ref(object: *u8);
fn release(object: *u8);
fn delegate(iid: Guid, invoke: *u8, callback: *u8, context: *u8, agile: bool=false) -> Result[Object, Error];
fn delegate_callback(object: *u8) -> *u8;
fn delegate_context(object: *u8) -> *u8;
fn delegate_query(self: *u8, id: *Guid, out: *usize) -> i32;
fn delegate_add_ref(self: *u8) -> u32;
fn delegate_release(self: *u8) -> u32;
fn box_interface(value: Object, reference_iid: Guid) -> Result[Object, Error];
```

Offsets are bytes; slots are pointer indices. AddRef/Release accept zero.
Delegate allocation owns a four-slot IUnknown/Invoke table; callback/context
pointers are borrowed. `agile: true` opts into IAgileObject and requires the
caller to satisfy callback/context threading rules. It adds no synchronization.
Delegate support functions apply only to allocations
created by `delegate`. Prefer a generated typed delegate constructor.

`box_interface` owns a reference to an interface/delegate value through an
`IInspectable` implementing `IReference<T>`. The caller must supply the
matching parameterized IID and interface pointer; generated delegate
`boxed()` methods do this automatically. The box's `Value` getter returns
an owned reference. It does not implement `IAgileObject` or expose the
delegate interface directly through QueryInterface.

`unknown_iid() -> Guid`, `inspectable_iid() -> Guid`, `agile_iid() -> Guid`, and
`guid_equal(a: Guid, b: Guid) -> bool` expose identity constants/comparison.
`no_interface()`, `pointer_error()`, and `out_of_memory()` return `i32`
HRESULT constants. `error_from_win32() -> Error` converts GetLastError
for a failed Win32 call and must be used immediately after that failure.

Dependencies: `stdlib`; import libraries: `runtimeobject`, `ole32`.
Tests: `cpc test --filter winrt_` from the package root on Windows x64.
