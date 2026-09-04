# Tutorial

## 1. Depend on it

```toml
[dependencies]
share = "*"
```

## 2. Share

```cplus
import "share/share" as sh;

fn share_tapped(sender: *u8, ctx: *u8) {
    let _o: sh::Outcome = sh::text("look at this");
}
```

## 3. Use the right verb

```cplus
sh::url("https://example.com/thing");   // preview card, "Copy Link"
sh::file("/tmp/report.pdf");            // through a file provider
sh::text("plain words", subject: "Re: the thing");
```

`subject` is carried on Android and dropped on Apple — see the
[guide](guide.md#subject).

## 4. Do not wait for an answer

```cplus
// WRONG: `Ok` means the sheet opened, not that anything was sent.
if sh::text("x").to_code() == sh::Outcome::Ok.to_code() { mark_as_shared(); }
```

There is no reliable "they shared it" on any platform. If your app needs to
know, observe the thing itself.

## 5. Hide a button that would do nothing

```cplus
if sh::available() { show_share_button(); }
```

False in a headless process, and on macOS before any window exists.
