# Guide

How links reach an app, which of the two kinds you are actually building, and
the several ways each one silently does nothing. Signatures are in
[ref.md](ref.md); the five-minute path is [tutorial.md](tutorial.md).

## The two kinds of link are not one feature

They arrive at the same handler and share nothing else.

| | custom scheme (`myapp://`) | universal / app link (`https://`) |
|---|---|---|
| Apple registration | `CFBundleURLTypes` in Info.plist | `com.apple.developer.associated-domains` **entitlement** |
| Apple server side | none | `https://d/.well-known/apple-app-site-association` |
| Apple delivery | `openURLs:` / `openURLContexts:` | `NSUserActivity` with a `webpageURL` |
| Android registration | `<data android:scheme="myapp">` | `<data android:scheme="https">` + `android:autoVerify="true"` |
| Android server side | none | `https://d/.well-known/assetlinks.json` |
| Android delivery | `getData()` | `getData()` — same door |
| anyone can claim it | **yes** | no, the OS checks the domain |
| the user sees | **an "Open in …?" confirmation** | nothing — it just opens |
| when it fails | nothing happens | **the browser opens instead, silently** |

The "anyone can claim it" row is why `host:` behaves the way it does — see
below. The row under it is the same fact from the user's side: iOS interrupts a
custom-scheme open with a confirmation, because nothing verified that the app
should get the link. A universal link is checked against your domain, so it
opens straight through. If a link is something people follow often, that
dialog is a real reason to do the universal-link work.

macOS does not prompt for a custom scheme: `open "myapp://x"` reaches the app
directly, cold or warm.

## Choosing a filter

```cplus
applinks::on_link(f)                            // everything
applinks::on_link(f, scheme: "myapp")           // a deep link
applinks::on_link(f, host: "example.com")       // a universal link
applinks::on_link(f, scheme: "https", host: "example.com")   // both must match
```

Use `scheme:` for anything you registered yourself. Use `host:` for a domain
you own and have verified.

### Gotcha: `host:` refuses a custom scheme on purpose

`on_link(f, host: "example.com")` does **not** fire for
`myapp://example.com/record/42`, even though the host matches.

A verified universal link is trustworthy *because* the operating system checked
the domain association before delivering it. A custom scheme carries no such
proof — any app on the device may register `myapp://`, and any web page may
link to it. If `host:` matched both, a handler written for the verified road
would run on the unverified one, and the check that makes the feature safe
would be bypassed by spelling.

So `host:` implies http/https, and that is not configurable. To match a custom
scheme's first component, filter on the scheme and read the segment:

```cplus
applinks::on_link(f, scheme: "myapp");
// then inside f: url::parse(u) ... p.segment(0 as usize) == "record"
```

### Gotcha: an unparseable URL reaches only the unfiltered rows

A filter cannot be applied to something with no scheme to read, and guessing
would be worse than not delivering. If you want to see everything the system
hands over, including malformed input, register with no filter.

## Cold start, and why late registration works

A link that launches a dead process is delivered before any of your code has
subscribed. Three things make that survivable:

1. The backend fires `E_OPEN_URL` from whichever delegate method received it.
2. `facet/app_events` **latches** that kind and replays it to a new subscriber.
3. This package latches it again for its own rows.

Step 3 is not redundant. `app_events` replays to a new *subscriber*, and this
package subscribes exactly once — so without a second latch the replay would
land on whichever row registered first and every later one would miss the link
that started the app.

The consequence for you: **register wherever is convenient.** `on_attach` is
fine. There is no race to win.

### Gotcha: the latch replays the most recent link, not only a cold one

It behaves exactly like the channel underneath: a row registered after any link
gets that link. If you register a second handler ten minutes into a session, it
is handed whatever arrived last. Register once, early, if that matters.

## Registration is not code, and it is where the time goes

Almost every "deep links don't work" report is one of these.

### The scheme is not registered

`on_link` returns `Ok` and nothing ever arrives. `Ok` means *registered*, not
*links will come*. Check the plist or the manifest first.

**macOS also caches it.** LaunchServices records a bundle's URL claims when it
first sees the bundle, so adding a scheme to a plist that has already been
registered changes nothing until you re-register:

```
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f MyApp.app
```

### Android: the intent-filter is missing a category

`BROWSABLE` is what lets a link from a browser or a message reach the app;
`DEFAULT` is what lets an implicit intent match at all. With one of them
missing, the filter is present and nothing is ever delivered.

### Android: the activity is not `singleTop`

The default launch mode is `standard`, and a `standard` activity handed a new
intent gets a **second instance** — a second facet mount stacked on the first —
rather than `onNewIntent`. Notifications never hit this because they build their
own intent and set `FLAG_ACTIVITY_SINGLE_TOP`; a browser does not do that for
you.

```xml
<activity android:name="cplus.facet.FacetActivity"
          android:launchMode="singleTop" ...>
```

`cpc init` writes this. An app with a hand-written manifest must add it.

### The universal link is not verified

This is the one that fails silently and looks like a bug in your code: the
link opens **the browser** instead of the app, with no error anywhere.

**Android** tells you the truth:

```
adb shell pm get-app-links <your.package>
```

A domain in state `verified` works. `none` or `legacy_failure` means Android
could not fetch or match `assetlinks.json`. From API 31 the user can also turn
link handling off per app, and that overrides verification.

**Apple** has no equivalent query. Check the file by hand: it must be at
`https://yourdomain/.well-known/apple-app-site-association`, served over HTTPS
with **no redirects**, with `Content-Type: application/json`, no `.json`
extension, and the app ID inside it must be `TEAMID.bundle.identifier`.

Both files are yours to serve, from a domain you control. Neither can be
produced by this repo, and neither can be tested from a simulator.

## Routing: this package hands you a string and stops

There is no route table here, and that is a decision rather than an omission.
`facet_runtime` already has one — `register_screen` / `has_screen` /
`push_screen` — and a second registry mapping URL patterns to screens would be
a parallel routing authority whose disagreements with the real one look like a
link opening the wrong screen.

The recipe is three lines:

```cplus
fn opened(u: str, ctx: *u8) {
    match url::parse(u) {
        option::Option[url::Url]::Some(p) => {
            let route: str = p.segment(0 as usize);
            if runtime::has_screen(route) { nav::go(route, arg: p.segment(1 as usize)); }
        }
        option::Option::None => { }
    }
    return;
}
```

It also keeps this package usable from something with no facet runtime at all.

## Threading

Handlers run on the UI thread, always. `facet/app_events` guarantees it, and a
backend whose platform callback arrives elsewhere hops it before firing. So
navigating or writing the tree from a handler is safe with no further
ceremony.

## What is not here

**Opening a URL.** Handing a URL *to* the system needs no registration and is
a different feature in the other direction.

**Push notifications.** A tapped notification is `vendor/notifications`, and its
payload is not a URL. The two ride the same channel under different kinds, so
neither sees the other's traffic.

**The scheme's registration.** See above — it is four files and none of them
are code.

## Testing, and the one step an agent cannot take

Everything about routing, filtering and the latch is covered by
`cd vendor/applinks && cpc test`, driven through the real fan-out.

Whether a *backend* fires the event is covered per platform:

| platform | how | covered |
|---|---|---|
| macOS | `open "myapp://x"`, warm and cold | yes, end to end |
| iOS | `xcrun simctl openurl booted "myapp://x"`, plus `facet_uikit`'s simulator suite driving the delivery verbs with real `NSURL` / `NSUserActivity` objects | yes, end to end — one tap, see below |
| Android | `adb shell am start -a android.intent.action.VIEW -d …`, warm and cold | yes, end to end |

`xcrun simctl openurl` raises the same "Open in …?" confirmation a real user
gets, and nothing here can dismiss it — there is no tap command in `simctl`,
and `osascript` needs assistive access. So the iOS run needs one tap from a
person; it is not a simulator artifact, it is the dialog in the table above.
Both halves have been driven through by hand and land correctly.

Verified https links cannot be tested anywhere in this repo, on any platform,
for the reasons in the table at the top.
