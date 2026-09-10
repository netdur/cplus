# Tutorial

Quick path: depend, read a state, ask for one, handle the answer. Deeper
rationale and gotchas live in [guide.md](guide.md); signatures in
[ref.md](ref.md).

## Setup

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

No `[link]` frameworks and no Java or dex: the Apple half `dlopen`s what it
needs and the Android half reaches the platform reflectively.

## Read a state

`state` never prompts, so it is safe while building a screen.

```cplus
let s: permissions::State = permissions::state(of: permissions::CAMERA);
```

Six answers: `Unknown`, `Granted`, `Limited`, `Denied`, `Blocked`,
`Unsupported`.

## Ask for one

```cplus
fn answered(name: str, s: permissions::State, ctx: *u8) {
    if s == permissions::State::Granted { start_capture(); }
    return;
}

let _st: status::Status = permissions::request(permissions::CAMERA,
                                               on_answer: answered);
```

The callback always runs, exactly once, including for a name this build has
never heard of.

## Ask for several

One dialog on Android, a sequence on Apple. The callback fires once per name.

```cplus
var want: vec::Vec[str] = vec::new::[str]();
let _a: status::Status = want.append(permissions::CAMERA);
let _b: status::Status = want.append(permissions::MICROPHONE);
let _r: status::Status = permissions::request_many(want.as_slice(),
                                                   on_answer: answered);
```

## Send them to Settings

When the answer is `Blocked`, asking again does nothing and this is the only
road left.

```cplus
if permissions::state(of: permissions::CAMERA) == permissions::State::Blocked {
    let _ok: bool = permissions::open_settings(pane: permissions::CAMERA);
}
```

## The names

```cplus
permissions::CAMERA          permissions::CONTACTS
permissions::MICROPHONE      permissions::CALENDAR
permissions::PHOTOS_READ     permissions::NOTIFICATIONS
permissions::PHOTOS_ADD      permissions::LOCATION_WHEN_IN_USE
                             permissions::LOCATION_ALWAYS
```

Any other string is passed through to the platform where it can be
(`"android.permission.NFC"`) and answers `Unsupported` where it cannot.

## Day-one rules

- **Put the keys in your app's manifest.** On Apple a missing `Info.plist`
  usage-description key does not break `state` — it kills the process on
  `request`, a moment after the call returns. On Android a missing
  `<uses-permission>` denies forever with no dialog. Nothing checks this for
  you — [guide.md](guide.md#the-manifest-is-yours) lists the pairs.
- **`Denied` is not `Blocked`.** Ask again on `Denied`; offer Settings on
  `Blocked`.
- **The callback may run before `request` returns.** When no dialog is needed —
  already granted, already blocked, an unknown name — it fires on the calling
  thread, synchronously. Only a real prompt is asynchronous, and that lands on
  the main thread.
- **Ask for background location second.** `LOCATION_ALWAYS` is refused until
  `LOCATION_WHEN_IN_USE` is held.
