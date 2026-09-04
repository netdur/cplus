# permissions

Ask the platform for access, and know the answer without asking.

```toml
[dependencies]
permissions = "*"

[macos.dependencies]
objc = "*"      # the Apple half's msgSend; brings -framework Foundation

[ios.dependencies]
objc = "*"

[android.dependencies]
jni          = "*"   # checkSelfPermission / requestPermissions, reflectively
android_view = "*"
facet        = "*"   # app_events: the Activity and the JavaVM
flex_layout  = "*"   # facet's closure, restated
events       = "*"
```

Name only the platforms you build for. The resolver validates every import
against ONE flat set taken from **your** manifest — it does not read a
dependency's own — so a package's transitive deps are named again here. Miss one
and the link says which symbol.

```cplus
import "permissions/permissions" as permissions;
```

## Common case

Read a state, ask when it can be asked, answer through a callback.

```cplus
fn answered(name: str, s: permissions::State, ctx: *u8) {
    if s == permissions::State::Granted { start_capture(); }
    return;
}

match permissions::state(of: permissions::CAMERA) {
    permissions::State::Granted => { start_capture(); }
    permissions::State::Blocked => { let _ = permissions::open_settings(pane: permissions::CAMERA); }
    _ => { let _s = permissions::request(permissions::CAMERA, on_answer: answered); }
}
```

A permission is a **name**, not an enum: the constants exist so a typo is a
compile error, and a bare string is the escape hatch for anything this package
has never heard of (`permissions::state(of: "android.permission.NFC")`).

## Two things that will bite you

- **The app's own manifest is a hard prerequisite and nothing enforces it.** On
  Apple, a missing `Info.plist` usage-description key leaves `state` working
  normally and makes `request` **kill the process** — asynchronously, after the
  call has already returned. On Android a missing `<uses-permission>` makes the
  read answer denied forever with no dialog. See
  [guide.md](docs/guide.md#the-manifest-is-yours).
- **`Denied` and `Blocked` are different.** `Denied` means asking again shows a
  dialog; `Blocked` means it does nothing and the only road left is
  `open_settings`. Collapsing them produces a button that does nothing.

## Docs

| Need | File |
|---|---|
| Use it in minutes | [docs/tutorial.md](docs/tutorial.md) |
| How / why / gotchas | [docs/guide.md](docs/guide.md) |
| Exact signatures | [docs/ref.md](docs/ref.md) |

## Platforms

| Platform | State | Diverges by |
|---|---|---|
| macOS, iOS | full | one file; notifications need a signed bundle, so macOS answers `Unsupported` for it |
| Android | full, including location | one dialog per batch, a persisted "have asked" bit, an API 33 fork for notifications |
| Linux | `Unsupported` for everything | `xdg-desktop-portal` is parked, and it has no check-without-asking |

## Tests

Unit tests are `#[test]` fns beside the code, with `src/test_main.cplus` as the
discovery root:

```sh
cd vendor/permissions && cpc test          # 183 checks, macOS host
```

`tests/` holds the iOS runner — a package rather than flat files, because the
checks need UIKit and a bundle, so they have to be an app. It is driven by
`tools/run_ios_tests.sh`, which also grants and revokes through
`xcrun simctl privacy` and asserts the reads follow. `playground/permprobe` is
the Android probe for the one thing no harness can do: tap Deny twice.
