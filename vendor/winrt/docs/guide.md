# Guide

See [tutorial.md](tutorial.md) for setup and [ref.md](ref.md) for signatures.

## Ownership

`Object` owns one COM reference and releases it on drop. `adopt` takes an
already-owned native +1 reference; `retain` adds a reference to a borrowed
pointer. `clone` and a successful `query` each produce another owner. Raw
pointers do not extend lifetimes. Do not adopt the same native reference
twice. Empty objects deliberately represent nullable ABI values; querying
an empty object fails, whereas `query_nullable` preserves native null.

`HString` owns a Windows HSTRING. Empty strings may have a zero handle;
this is valid. `new` converts UTF-8 strictly; `to_text` returns an independent
UTF-8 copy. Invalid Unicode conversion is reported as an HRESULT error.

`Status` is an HRESULT for mutating calls. `Result[T, Error]` carries a
getter's value or an HRESULT. Nonnegative HRESULTs are success, including
S_FALSE. The runtime does not translate restricted error information or
exception descriptions.

## Apartments, callbacks, and composition

Create `Apartment::sta()` on the UI thread. Release UI objects and event
subscriptions before dropping the apartment on that same thread. Atomic
delegate reference counting does not make controls or callbacks agile.
The delegate runtime claims IUnknown and its declared IID, not IInspectable.
By default it also rejects IAgileObject. An explicit `agile: true` constructor
argument opts in to that marker; it is a caller promise that transferring the
delegate and its context is safe under the receiving API's invocation rules.
It is not automatic marshaling or synchronization. DispatcherQueue requires
this option and invokes callbacks on its associated queue thread. Ordinary
control event handlers should retain the default.

Generated delegate constructors take a function and borrowed `*u8` context.
They own the delegate allocation, not its context. Remove subscriptions and
release all delegate references before disposing that context. C+ callbacks
must return an HRESULT-compatible `Status` instead of unwinding over COM.

`compose` is a low-level helper for a factory with exactly an outer-object
input and an inner-object output. The caller implementing an outer object
must implement controlling identity and keep the nondelegating inner reference
alive. `winui/application` provides that implementation for Application;
the standalone sample owns the outer apartment lifetime.

## Deployment and ABI

`load_runtime` performs a process-lifetime DLL load. It does not register
classes, initialize an apartment, install framework packages, or deploy
resources. The standalone WinUI sample stages an SDK runtime and embeds
activation registrations separately.

Pointer slot operations and delegate allocation assume Windows x64.
Generated aggregate calls explicitly follow the
[Windows x64 calling convention](https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention): small aggregates use integer arguments, and larger
ones use aligned temporary storage. This is not a promise of compiler-wide
aggregate ABI support on other targets.
