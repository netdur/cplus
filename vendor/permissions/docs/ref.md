# Reference

Manual for the `permissions` package. Signatures and behavior only — no
tutorials. Import:

```cplus
import "permissions/permissions" as permissions;
```

---

## Conventions

Used by every entry point below.

| Item | Definition |
|---|---|
| Name | a `str`. The constants are portable names; any other string is passed to the platform where it can be and answers `Unsupported` where it cannot |
| Handler | `fn(name: str, state: State, ctx: *u8)` |
| Default `ctx` | `0 as *u8` when omitted |
| `name` in a handler | borrowed from this package's own copy for the dispatch; copy it (`text::from_str`) to keep it |
| Callback timing | before `request` returns when no dialog is needed; on the main thread when a prompt was shown |
| Callback count | exactly once per name, always, including for unknown names |

---

## Names

```cplus
const CAMERA: str                 // "camera"
const MICROPHONE: str             // "microphone"
const PHOTOS_READ: str            // "photos.read"
const PHOTOS_ADD: str             // "photos.add"
const CONTACTS: str               // "contacts"
const CALENDAR: str               // "calendar"
const NOTIFICATIONS: str          // "notifications"
const LOCATION_WHEN_IN_USE: str   // "location.when_in_use"
const LOCATION_ALWAYS: str        // "location.always"
```

Platform coverage:

| Name | macOS | iOS | Android |
|---|---|---|---|
| `CAMERA`, `MICROPHONE` | yes | yes | yes |
| `PHOTOS_READ` | yes | yes, `Limited` possible | yes (API 33 media permission) |
| `PHOTOS_ADD` | yes | yes | always `Granted` — MediaStore needs no permission |
| `CONTACTS`, `CALENDAR` | yes | yes | yes |
| `NOTIFICATIONS` | `Unsupported` — needs a signed bundle | yes, cached read | yes; below API 33 reads a setting, not a grant |
| `LOCATION_WHEN_IN_USE` | not in this pass | not in this pass | yes; `Limited` when coarse-only |
| `LOCATION_ALWAYS` | not in this pass | not in this pass | yes; `Denied` until foreground is held |

---

## `State`

```cplus
enum State { Unknown, Granted, Limited, Denied, Blocked, Unsupported }
```

| Arm | Meaning |
|---|---|
| `Unknown` | Never asked, or the platform will not say. `request` will prompt |
| `Granted` | Full access |
| `Limited` | Partial access — selected photos, write-only calendar, provisional notifications, coarse-only location |
| `Denied` | Refused; asking again shows a dialog |
| `Blocked` | Refused; asking again does nothing. Parental controls, an MDM profile, a second refusal, or a platform that prompts once |
| `Unsupported` | This build cannot ask: an unregistered name, or a platform with no such concept. Never a refusal |

### `to_code`

```cplus
fn to_code(this) -> u32
```

The wire form the seam speaks: `Unknown` 0, `Granted` 1, `Limited` 2, `Denied`
3, `Blocked` 4, `Unsupported` 5. Stable — a probe or a test may compare against
the numbers.

### `from_code`

```cplus
fn State::from_code(c: u32) -> State
```

Inverse of `to_code`. An unrecognised code answers `Unsupported` rather than
trapping: the seam is a version boundary and a newer backend must degrade.

---

## Verbs

### `state`

```cplus
fn state(of: str) -> State
```

What the platform says right now. **Never prompts**, so it is safe to call while
building a screen and safe to call repeatedly. An empty name answers
`Unsupported` without reaching the platform.

For `NOTIFICATIONS` on Apple this reads a cache rather than the platform: the
first call of a process answers `Unknown` and starts an asynchronous refresh.

### `can_prompt`

```cplus
fn can_prompt(name: str) -> bool
```

True for `Unknown` and `Denied` — the two states where `request` puts a dialog
on screen. False for `Granted`, `Limited`, `Blocked` and `Unsupported`. Gating a
"grant access" button on this hides it once access is held.

Unlabelled first argument: `for:` is the natural label and `for` is a keyword.

### `request`

```cplus
fn request(name: str, on_answer: fn(str, State, *u8),
           on_answer_ctx: *u8 = 0 as *u8) -> status::Status
```

Ask. `Ok`, or `InvalidInput` for a null handler. `Ok` means the request was
accepted, not that access was granted.

The handler receives the name it was asked about, the answer, and `on_answer_ctx`.
It runs exactly once. When the platform needs no dialog it runs **before this
function returns**, on the calling thread; a real prompt answers later, on the
main thread.

On Android, `LOCATION_ALWAYS` is refused with `Denied` — and nothing is asked or
recorded — unless `LOCATION_WHEN_IN_USE` is already held.

### `request_many`

```cplus
fn request_many(names: str[], on_answer: fn(str, State, *u8),
                on_answer_ctx: *u8 = 0 as *u8) -> status::Status
```

Ask for several. `Ok`, or `InvalidInput` for a null handler or an empty slice.
The handler fires **once per name**, the same shape `request` uses.

On Android this is one `requestPermissions` array and therefore **one dialog**;
names that need no prompt are answered immediately and left out of it. Apple has
no batch API, so prompts arrive in sequence.

### `open_settings`

```cplus
fn open_settings(pane: str = "") -> bool
```

Open the settings page where a person can change their mind. True when a page
was opened.

`pane` is a permission name. macOS deep-links per domain
(`x-apple.systempreferences:...?Privacy_Camera`); iOS and Android have one
app-level page and ignore it. `""` means the app's own page and is right
everywhere.

Returns `bool` rather than `Status` deliberately: the only failure is "this
platform has no such page", and `stdlib/status` has no arm that means it —
`InvalidInput` would blame the caller for a fact about the platform.

---

## `permissions_backend` — the Apple half

```cplus
import "permissions/permissions_backend" as permissions_backend;
```

Apple platforms only. Everything in this module except the items below is
module-private.

### `register_apple`

```cplus
fn register_apple(name: str,
                  class_name: str,
                  status_sel: str,
                  shape: u32,
                  request_sel: str,
                  request_shape: u32,
                  plist_key: str,
                  framework: str = "",
                  argument: str = "",
                  value: i64 = 0 as i64,
                  settings_anchor: str = "") -> status::Status
```

Add a domain this package does not ship. `Ok`, or `InvalidInput` for an empty
`name`, an empty `plist_key`, an empty `class_name`, or an unrecognised `shape`.

Registering an existing name **shadows** the built-in row.

| Parameter | Meaning |
|---|---|
| `name` | the name callers will pass to `state` / `request` |
| `class_name` | the ObjC class answering both selectors |
| `status_sel` | class-method selector returning the authorization status |
| `shape` | which argument `status_sel` takes — `S_MEDIA`, `S_ENTITY`, `S_LEVEL` |
| `request_sel` | the selector that prompts |
| `request_shape` | `R_CLASS_MEDIA`, `R_CLASS_LEVEL`, `R_INST_ENTITY`, `R_INST_PLAIN` |
| `plist_key` | the `Info.plist` usage-description key. Required — a row without one is a process kill waiting for its first user |
| `framework` | absolute path, `dlopen`ed on first use. Empty when the class is already in the process |
| `argument` | the NSString argument for `S_MEDIA` |
| `value` | the integer argument for `S_ENTITY` and `S_LEVEL` |
| `settings_anchor` | the macOS System Settings anchor, e.g. `"Privacy_Camera"` |

A registered row is always read with the standard status enum and always
constructs its own instance. It cannot express location, whose answer arrives on
a delegate rather than from a class method.

### Status shapes

```cplus
const S_MEDIA: u32     // +authorizationStatusForMediaType:   (NSString)
const S_ENTITY: u32    // +authorizationStatusForEntityType:  (integer)
const S_LEVEL: u32     // +authorizationStatusForAccessLevel: (integer)
```

### Request shapes

```cplus
const R_CLASS_MEDIA: u32   // +requestAccessForMediaType:completionHandler:   block(BOOL)
const R_CLASS_LEVEL: u32   // +requestAuthorizationForAccessLevel:handler:    block(NSInteger)
const R_INST_ENTITY: u32   // -requestAccessForEntityType:completionHandler:  block(BOOL, NSError*)
const R_INST_PLAIN: u32    // -requestFullAccessToEventsWithCompletion:       block(BOOL, NSError*)
```

`R_INST_PLAIN`'s selector is macOS 14 / iOS 17. An application below that floor
registers its own row.

---

## Package metadata

| Item | Value |
|---|---|
| Import | `permissions/permissions` |
| Apple extension surface | `permissions/permissions_backend` |
| Dependencies | `stdlib`; `objc` on macOS and iOS; `jni`, `android_view`, `facet`, `flex_layout`, `events` on Android. A consumer restates these in its own manifest — the resolver validates against one flat set taken from the consuming manifest |
| `[link]` frameworks | none — the Apple half `dlopen`s what it needs |
| Unit tests | `#[test]` beside the code, root `src/test_main.cplus`; `cd vendor/permissions && cpc test` |
| iOS checks | `tools/run_ios_tests.sh` (simulator, plus a `simctl privacy` round-trip) |
| Android probe | `playground/permprobe` |
