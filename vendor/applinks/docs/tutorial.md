# Tutorial

Quick path: register a scheme, register a handler, open a URL. Deeper rationale
and gotchas live in [guide.md](guide.md); signatures in [ref.md](ref.md).

## Depend

```toml
[dependencies]
stdlib      = "*"
facet       = "*"
flex_layout = "*"
events      = "*"
applinks    = "*"
```

No platform section: this package names no backend. The link arrives through
whichever facet backend your app already uses.

```cplus
import "applinks/applinks" as applinks;
import "stdlib/url" as url;
```

## Receive a link

```cplus
fn opened(u: str, ctx: *u8) {
    // `u` is the whole URL, exactly as the system spelled it.
    return;
}

let _o: applinks::Outcome = applinks::on_link(opened, scheme: "myapp");
```

Register it wherever the rest of your app registers things — `on_attach` is
fine. A link that launched the process is replayed to a handler that turns up
afterwards, so late registration still receives it.

## Register the scheme

Code alone receives nothing. The platform has to know the scheme belongs to
you.

**iOS / macOS** — in `Info.plist`:

```xml
<key>CFBundleURLTypes</key>
<array>
    <dict>
        <key>CFBundleURLName</key><string>com.example.myapp</string>
        <key>CFBundleURLSchemes</key>
        <array><string>myapp</string></array>
    </dict>
</array>
```

**Android** — inside the `<activity>` in `AndroidManifest.xml`:

```xml
<intent-filter>
    <action android:name="android.intent.action.VIEW" />
    <category android:name="android.intent.category.DEFAULT" />
    <category android:name="android.intent.category.BROWSABLE" />
    <data android:scheme="myapp" />
</intent-filter>
```

`cpc init` writes both, commented out, with the scheme as a placeholder.

## Try it

```
macOS      open "myapp://record/42"
iOS sim    xcrun simctl openurl booted "myapp://record/42"
Android    adb shell am start -a android.intent.action.VIEW -d "myapp://record/42"
```

All three work whether the app is running or not.

## Take the URL apart

```cplus
match url::parse(u) {
    option::Option[url::Url]::Some(p) => {
        let route: str = p.segment(0 as usize);   // "record"
        let id: str    = p.segment(1 as usize);   // "42"
    }
    option::Option::None => { }
}
```

**`segment`, not `path`.** In `myapp://record/42` the word `record` is the URL's
HOST — `path` is `/42`. `segment(0)` reads the host when there is one and the
first path component when there is not, so all three spellings of a deep link
route the same way. This is the mistake to know about before you write the
handler, not after.

## Day-one rules

- The `str` is borrowed for the call. Keep it with `text::from_str`.
- Handlers run on the UI thread, so navigating from one is safe.
- `scheme:` is how two subscribers stay out of each other's traffic — the same
  channel carries a sign-in SDK's OAuth callback.
- `host:` matches http/https only. That is deliberate; see the guide.
- A universal link (`https://`) needs more than a plist key. See the guide.

## Tests

```
cd vendor/applinks && cpc test
```

Unit tests are `#[test]` blocks in `src/applinks.cplus`.
