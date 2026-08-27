# Guide

How the package works, why it is shaped this way, and the things that will
surprise you.

## What it binds, and what it refuses to write

`http` is a binding, not an implementation. Everything hard about an HTTPS
request — certificate chain validation, DNS, connection reuse, HTTP/2 and
HTTP/3, `Accept-Encoding` negotiation, proxies, VPN routing, the user's privacy
switches — is already in the platform's own client, already tested, and already
patched on a schedule this repo does not keep. Re-implementing any of it here
would be worse code shipping later.

That decision is also what makes the package small. There are two verbs.

## One package, three transports

`http/http` is the module you import everywhere. Underneath it,
`transport.cplus` is swapped per platform by the resolver's `_<platform>` file
override — the same mechanism `stdlib` uses for `reactor.cplus` /
`reactor_linux.cplus`:

| Platform | File | Client |
|---|---|---|
| macOS, iOS | `transport.cplus` | `NSURLSession` |
| Android | `transport_android.cplus` | `java.net.HttpURLConnection` |
| Linux | `transport_linux.cplus` | none — refuses with `-3001` |

NSURLSession lives in **Foundation**, not AppKit, which is what lets one file
serve macOS and iOS both. The Android transport is reflective JNI against
classes already on every app's boot classpath, so it compiles to a `.so` and
ships **no Java and no dex**.

The two halves meet at a stable symbol (`http_transport_perform_v1`) rather
than an import: the transport must import `http` for `Request` and `Response`,
and an import back would be a cycle, which C+ rejects (E0404). Same shape
`stdlib/executor` uses to reach the reactor.

### A socket client was the other option

One implementation instead of three, and it is not what this package is. It
would be **plaintext only** — TLS is the part nobody can write portably, and
Android's own `libssl` sits in a linker namespace an app may not reach
(measured: `dlopen` and `dlsym` both refuse). An `http` package that could not
fetch an `https` URL would not be worth the name.

### Android: two things to know

`android.permission.INTERNET` is required — Android gates `socket()` on it and
loopback is not an exception.

Requests from the UI thread **fail**, they do not merely stutter: the platform
throws `NetworkOnMainThreadException`, which arrives as `Error` code `-2004`.
That is the same "call it off-main" rule as everywhere else, enforced by the
platform instead of by convention.

## Why blocking

An async HTTP API needs a story for cancellation, for progress, for what thread
a continuation resumes on, and for how a future interacts with the UI thread.
Each of those is a design decision worth making once, carefully, and none of
them is needed to fetch a JSON document. So v1 blocks, and a caller that needs
concurrency gets it the way C+ gets concurrency everywhere else: a thread.

Blocking the main thread does **not** deadlock — NSURLSession delivers its
completion on a private serial queue, never on the main queue, and the Android
transport is synchronous all the way down — it just freezes the UI for a network
round trip. Both are bugs. Call it off-main. On Android the platform refuses
outright; see above.

The facet shape is a service: produce off the UI thread, apply on it.

```cplus
// worker
fn fetch(ctx: *u8) {
    let box: *Pending = { ctx as *Pending };
    match http::get(feed_url) {
        result::Result[http::Response, http::Error]::Ok(r)  => { { (*box).body = r.body_text() }; }
        result::Result[http::Response, http::Error]::Err(e) => { { (*box).failed = true }; }
    }
    services::run_on_main(apply, ctx);      // the hop home
}

// main thread
fn apply(ctx: *u8) { /* now it is safe to touch views */ }
```

## Non-2xx is data

`Err` means there was no HTTP response. A 404, a 500, a 401 — all of those are
`Ok`, carrying the status the server sent. A caller that treats "the server said
no" and "there is no network" as the same event will write the wrong error
message, and there is no way to recover the distinction once it is lost.

```cplus
guard let result::Result[http::Response, http::Error]::Ok(r) = http::get(url) else {
    return;                       // no answer at all
};
if !r.is_success() { /* the server answered, and the answer was no */ }
```

## Errors

`Error` is `{ code: i64, message: Text }`.

`code` is the platform's own code whenever the request reached the
session, and every NSURLErrorDomain code is **negative**:

| code  | means |
|-------|-------|
| -1001 | timed out |
| -1002 | unsupported URL (this is what a malformed URL looks like) |
| -1003 | host not found |
| -1004 | could not connect |
| -1009 | not connected to the internet |
| -1200 | TLS handshake failed |

A **positive** code means the request never left the process:
`ERROR_INVALID_URL` (1), `ERROR_TASK_REFUSED` (2), `ERROR_NO_RESPONSE` (3). All
three are rare, and the first is rarer than it looks — modern Foundation
percent-escapes almost any input rather than refusing it, so `http::get("not a
url")` comes back as a real -1002 from the session, not as a local rejection.

`message` is the NSError's `localizedDescription`: the sentence Apple would
have shown a user, in the user's language.

## Headers

Header names are case-insensitive on the wire, so they are case-insensitive
here — `Request::set_header`, `Request::header` and `Response::header` all
compare that way. `set_header` *replaces* an existing field rather than adding a
second one, which is the same rule Foundation's `-setValue:forHTTPHeaderField:`
follows; the two therefore never disagree about how many of a field exist.

Response headers are copied out of `-allHeaderFields` into owned C+ values, so a
`Response` outlives every Objective-C object it came from. Foundation has
already merged repeated fields and canonicalised the names.

Lookup is a linear scan. A response has a handful of headers; a map would cost
more to build than it saves.

## Bodies

`Response.body` is a `Vec[u8]` — raw bytes, because a response is not
necessarily text.

- `r.body.as_byte_view()` is a `str` view over those bytes, no copy. Use it for
  searching and for handing to a parser.
- `r.body_text()` is an owned `Text` copy.

Neither validates UTF-8. A `Text` holds whatever bytes it is given, and only the
caller knows whether this response was supposed to be text at all — check
`Content-Type` first when it is not your own server.

On the request side, `set_body_text` and `set_body` both *replace* the body, and
both are one memcpy.

## How the escaping block works

This is the mechanically interesting part, and it is worth reading before
changing it.

`-dataTaskWithRequest:completionHandler:` takes an **escaping** block: the
session `Block_copy`s it during the msgSend and calls it much later, on a
background queue. C+ has no closures, so the block is a hand-built `#[repr(C)]`
struct in the layout libclosure expects — isa from `rt::stack_block_isa()`,
flags, reserved, invoke, descriptor, then the captures. The same shape has been
in `appkit_ext::dynamic_color` (which hands a provider block to NSColor) since
the theme work; this is that recipe applied to a second, bigger escape.

`flags` is **0**, which means the block declares no copy/dispose helpers. So
`Block_copy` is a flat memcpy of the 48 bytes, nothing walks the captures, and
`#addr_of(blk)` on a stack local is safe: what escapes is the session's copy,
not this frame. The two captures are POD pointers for the same reason — a
capture the copy would have to retain or release needs helpers, and helpers need
flags.

The descriptor is shared by every copy and read after the copy, so it is a
`static`, fully initialised at its declaration. That last detail is what makes
concurrent `send`s safe: `block_desc` only ever reads it. (The
malloc-one-on-first-use idiom next door in `appkit_ext` would race here and leak
the loser's copy.) Its `size` field is a literal 48 because a `static`
initializer takes no `#size_of` (E0911) and no `const` may fold one (E0921) —
`completion_block_is_the_flags_zero_stack_shape` is the guard that fails the
moment the struct stops being 48 bytes.

## The threading contract, in full

One `dispatch_semaphore_t`, created at 0. The calling thread resumes the task
and waits; the completion handler runs on a session queue and signals.

The handler:

1. copies the NSData bytes out with `malloc` + `memcpy` — **now**, because the
   NSData is autoreleased into a pool this code does not own and that pool
   drains the moment the handler returns;
2. retains the NSHTTPURLResponse and the NSError (immutable objects, so a +1
   reference is cheaper than transcribing them);
3. writes all of that into a `Slot` on the waiting frame;
4. signals **last**.

`dispatch_semaphore_signal` is a release barrier and `dispatch_semaphore_wait`
an acquire barrier, so write-before-signal / read-after-wait is the entire
synchronisation story. Nothing else is shared between the two threads: the
waiting thread does every allocation, every `Text`, and every release. The suite
passes under `--tsan`.

The wait is **unbounded**, and that is a decision, not an oversight. The
request's own `timeoutInterval` guarantees the handler eventually fires, so
there is nothing to race. Racing a semaphore timeout against it would abandon
the slot while a live background queue still holds a pointer into a frame that
is about to return — a use-after-free with a network-shaped trigger.

The semaphore is released right after the wait. That is safe for a related
reason: once `dispatch_semaphore_signal` has woken a waiter it touches no more
of the semaphore's memory.

## Autorelease pools

`+[NSURL URLWithString:]`, `+[NSMutableURLRequest requestWithURL:]` and the
bridge's NSStrings are all autoreleased, and a plain C thread has no pool. Every
exchange therefore runs inside `objc_autoreleasePoolPush` /
`objc_autoreleasePoolPop`, popped by a `defer` so the early returns cannot skip
it. Without it the objects leak and the runtime says so on stderr.

Verified: the probe app under `playground/http_leak` reports **0 leaks** from
`leaks` after seven exchanges including failures.

## What the OS decides, not this package

`+sharedSession` is the process-wide session — the system cookie store, cache,
credential store, and connection reuse across every caller in the process. So:
redirects are followed, cookies are stored and resent, `Accept-Encoding` is
negotiated and the response transparently decompressed, and system proxy and VPN
configuration applies.

None of that is configurable here, on purpose. A package-private
`NSURLSessionConfiguration` would be a policy decision made in the wrong place;
when a caller genuinely needs one, that is a new verb taking a session, not a
hidden default.

## Not in v1

Async, download-to-file, streaming/incremental bodies, upload progress,
per-request cookie or cache policy, authentication challenges beyond what the
system credential store answers on its own, and cancellation. Each is a real
feature; none of them is needed to fetch a document, and adding them
speculatively would fix their shape before there is a caller to shape them.

## Other platforms

Not in scope. Each is the OS's own client, bound the same way this one is:

| platform | client |
|---|---|
| linux | libcurl, easy interface, blocking `curl_easy_perform` |
| windows | WinHTTP, `WinHttpSendRequest` / `WinHttpReceiveResponse` |
| android | JNI to `java.net.HttpURLConnection` (see `vendor/jni`) |
| esp32 | `esp_http_client` from ESP-IDF (see `vendor/espidf`) |

When the second backend lands, the split follows the language's own mechanism:
the platform-free half (Method / Header / Request / Response / Error) stays in
`http.cplus` and the transport moves to `transport.cplus`, shadowed per platform
by `transport_linux.cplus` and friends. Doing that split now, with one backend,
would be guessing at a seam.

## Testing

```
cd vendor/http && cpc test
```

Everything except the `net_*` tests runs offline — including the transport-error
path, because NSURLSession rejects a malformed URL locally with -1002 before any
DNS lookup. Request building and error mapping are tested against **real**
Foundation objects (a real `NSMutableURLRequest` read back through
`-HTTPMethod` / `-valueForHTTPHeaderField:` / `-HTTPBody`, a real `NSError` from
`+errorWithDomain:code:userInfo:`), not against stand-ins.

The `net_*` tests need the internet and are the end-to-end gate: a live 200 with
a JSON body, a live 404 that must arrive as `Ok`, and a POST whose method,
headers and body are echoed back by the server.
