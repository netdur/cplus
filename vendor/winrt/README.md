# winrt

Shared C+ ownership and ABI support for Windows Runtime projections. It has
no WinUI or Facet dependency. The current implementation targets Windows x64.

```toml
[dependencies]
stdlib = "*"
winrt = "*"
```

```cplus
import "winrt/runtime" as rt;
// HString::new converts UTF-8 and returns Result[HString, Error].
// Owned HString and Object values release their handles on drop.
```

[Tutorial](docs/tutorial.md) · [Guide](docs/guide.md) · [Reference](docs/ref.md)

Run `cpc test --filter winrt_` from this package on Windows. Unit tests are
in `src/runtime.cplus`; `src/winrt.cplus` is the test discovery root.
The generated [WinUI sample](../../examples/winui_standalone) covers native
activation, composition, delegates, and shutdown end to end.

Generated delegates expose `boxed()` for APIs such as WinUI `AddHandler`
that accept an inspectable object. The runtime owns the delegate through a
typed `IReference<T>` box; ordinary typed event subscriptions still use
`object()`. Keep the box for `RemoveHandler`, and keep the borrowed callback
context alive until removal. Boxing does not make a delegate agile.
