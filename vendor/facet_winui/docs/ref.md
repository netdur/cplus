# Reference

Import `"facet_winui/facet_winui" as host`.
[Tutorial](tutorial.md) · [Guide](guide.md) · [Coverage](../MANIFEST.md)

```cplus
fn run(take tree: core::Node, title: str = "Facet / WinUI") -> rt::Status;
```

Consumes a retained Facet tree, initializes the runtime/apartment, creates one
native window, installs the renderer, and runs until window close. Callback
owners must outlive the call. Releases the tree's native records and handlers
before returning. Initial unsupported kinds/adopted views/leaf children return
E_NOTIMPL; a second run attempt returns E_UNEXPECTED. Deployment prerequisites
are described in the sample README. Internal renderer HRESULT failures are
currently fatal.

```cplus
fn live_views() -> i64;
fn live_subscriptions() -> i64;
```

UI-thread diagnostic counts for owned native view records and control click
subscriptions. Both must be zero after `run` returns. These are not counts of
all internal allocations or COM references in the Windows App SDK.

The sibling `views` and `native` modules are implementation support, not a
second application API. Native pointers exposed to handlers are borrowed View
records; use `component::key_of` / `item_of`, never cast them to HWND.
