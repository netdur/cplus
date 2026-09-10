# Reference

Every entry point. How and why live in [guide.md](guide.md); the quick path is
[tutorial.md](tutorial.md).

```cplus
import "applinks/applinks" as applinks;
```

## `Outcome`

```cplus
enum Outcome { Ok, InvalidInput, TooMany }
```

| variant | meaning |
|---|---|
| `Ok` | Registered. **Not** a promise that links will arrive — that depends on the platform registration this package does not own. |
| `InvalidInput` | A null handler. |
| `TooMany` | All eight slots are taken. |

## `on_link`

```cplus
fn on_link(f: fn(str, *u8), ctx: *u8 = 0 as *u8,
           scheme: str = "", host: str = "") -> Outcome
```

Register `f(url, ctx)` for links this app receives.

- `url` is the whole URL as the system spelled it — `absoluteString` on Apple,
  `Intent.getData().toString()` on Android. Not decoded, not trimmed.
- The `str` is **borrowed for the call**. Copy it with `text::from_str` to keep
  it.
- Called on the UI thread.

**Filters**, both defaulting to everything:

| argument | matches |
|---|---|
| `scheme: "myapp"` | URLs with that scheme, ignoring case |
| `host: "example.com"` | **http/https only**, on that host, ignoring case |

Giving both narrows — both must match. A URL that does not parse reaches only
rows with no filter.

`host:` refusing a custom scheme is deliberate and not configurable; see
[the guide](guide.md#gotcha-host-refuses-a-custom-scheme-on-purpose).

**Replay.** A link that arrived before this call — including the one that
launched the process — is delivered to the new row immediately, subject to its
filters. This is what makes registering in `on_attach` safe.

## `cancel`

```cplus
fn cancel(f: fn(str, *u8)) -> Outcome
```

Stop delivering to `f`. Matching is by **function**, not by filter: a package
that registered two rows with the same function removes both, so an `on_detach`
can undo an `on_attach` without recording what it passed.

`InvalidInput` for a null handler. `Ok` otherwise, including when `f` was never
registered.

## `SLOT_COUNT`

```cplus
const SLOT_COUNT: usize = 8 as usize;
```

How many handlers can be live at once. A cancelled slot is reused.

## Taking a URL apart

Not part of this package — `stdlib/url` is the parser, and `http` uses it too.

```cplus
import "stdlib/url" as url;

fn parse(s: str) -> option::Option[Url]
fn decode(s: str) -> text::Text        // percent only: the PATH rule
fn decode_form(s: str) -> text::Text   // percent + `+` as space: the QUERY rule

struct Url { scheme: str, host: str, port: u16, path: str, query: str, fragment: str }

impl Url {
    fn is_scheme(this, name: str) -> bool          // case-insensitive
    fn is_web(this) -> bool                        // http or https
    fn segment(this, index: usize) -> str          // host first; "" past the end
    fn segment_count(this) -> usize
    fn query_value(this, key: str) -> option::Option[text::Text]   // decoded
    fn has_query(this, key: str) -> bool
}
```

Every field of `Url` **views** the string that was parsed, so the source must
outlive it and needs a named owner:

```cplus
let s: str = incoming.view();     // named, not a temporary
match url::parse(s) { ... }
```

`segment(0)` is the host when there is one and the first path component when
there is not, so `myapp://record/42`, `myapp:///record/42` and
`myapp:record/42` all answer `"record"`.

## Platform support

| platform | delivers | via |
|---|---|---|
| macOS | custom scheme | `application:openURLs:` |
| iOS | custom scheme, universal link | scene delegate, warm and cold |
| Android | custom scheme, app link | `getData()` in `onCreate` / `onNewIntent` |
| Linux, Windows | nothing yet | no backend fires `E_OPEN_URL` |

On a platform with no delivery, `on_link` still answers `Ok` and no link ever
arrives. This package cannot tell the difference between that and a missing
registration, and does not pretend to.
