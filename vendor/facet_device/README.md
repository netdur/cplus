# facet_device

What the device can do, and whether the user has allowed it.

```cplus
import "facet_device/permissions" as perms;

if perms::status(perms::Capability::Camera) == perms::Permission::Undetermined {
    let got: perms::Permission = perms::request(perms::Capability::Camera, parent: window_handle);
}
```

An optional tier, like `facet_agent`: an application that never imports it links
none of it — which matters here because the stacks a capability drags in are
large and platform-specific.

`status` never prompts. `request` may, and blocks until the user answers or the
bound expires. Four answers, not two: `Undetermined` is not `Denied` (nobody has
been asked) and `Unavailable` is not `Denied` either (there is nothing to ask,
or nothing to ask with).

Linux is the XDG Desktop Portal over D-Bus. `MANIFEST.md` is the honest status —
what is live, what is a stated debt, and what the platform genuinely cannot do.
