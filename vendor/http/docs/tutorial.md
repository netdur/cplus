# Tutorial

Quick path: depend, GET a URL, read the answer. Gotchas in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
http = "*"
objc = "*"
```

`objc` is http's own dependency, and it has to be named here too: the resolver
validates every import in a build against one flat set taken from **your**
manifest, not from a dependency's. Leaving it out is `E0852: first segment
'objc' is not a declared dependency`, pointing at a file you did not write.

```cplus
import "stdlib/result" as result;
import "http/http" as http;
```

macOS and iOS only. `Foundation` links automatically — depending on `http` is
enough.

## GET something

```cplus
match http::get("https://example.com/feed.json") {
    result::Result[http::Response, http::Error]::Ok(r)  => { /* r.status, r.body */ }
    result::Result[http::Response, http::Error]::Err(e) => { /* e.code, e.message */ }
}
```

`Ok` means the server answered. It does **not** mean it liked the request — a
404 is an `Ok` carrying `status == 404`. `Err` is for exchanges that never
produced a response at all: offline, DNS failure, TLS failure, timeout.

## Read the response

```cplus
r.status                      // i32
r.is_success()                // 200..=299
r.body                        // vec::Vec[u8], the raw bytes
r.body.as_byte_view()         // str view of those bytes, no copy
r.body_text()                 // owned Text copy
r.header("content-type")      // Option[Text], case-insensitive
r.header_count()              // usize
```

## Build a fuller request

```cplus
var req: http::Request = http::Request::new("https://example.com/v1/items",
                                            method: http::Method::Post,
                                            timeout_seconds: 10.0);
let _a: status::Status = req.set_header("Content-Type", to: "application/json");
let _b: status::Status = req.set_header("Authorization", to: "Bearer ${token}");
let _c: status::Status = req.set_body_text("{\"name\":\"widget\"}");

match http::send(req) { ... }
```

Only the URL is required — `method:` defaults to `Get` and `timeout_seconds:` to
60, and both labels may appear in either order. `send` borrows the request, so
the same one can be sent again.

Binary bodies take a slice:

```cplus
let _b: status::Status = req.set_body(bytes.as_slice());
```

## Off the main thread

Every verb blocks. Do the request on a worker and deliver the result on the UI
thread:

```cplus
fn fetch(ctx: *u8) {
    match http::get("https://example.com/feed.json") {
        result::Result[http::Response, http::Error]::Ok(r)  => { stash(r); }
        result::Result[http::Response, http::Error]::Err(e) => { stash_error(e); }
    }
    services::run_on_main(apply, ctx);
}
```

## Day-one rules

- **Non-2xx is `Ok`.** Check `is_success()` or `status` yourself.
- **It blocks.** Never on the main thread of a UI app.
- **`Result` has no combinators** — `match` or `guard let`, like everywhere
  else in C+.
- **Set `Content-Type` yourself.** `set_body_text` sets bytes, not a type.
- Redirects are followed, cookies are stored, and compression is negotiated —
  all by NSURLSession, not by this package.
