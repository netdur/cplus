# http

Blocking HTTP(S) over the OS's own client. Darwin only: **NSURLSession**, which
is Foundation, so one backend serves both macOS and iOS. No TLS, DNS or HTTP
framing of our own — the OS already ships all three.

```toml
[dependencies]
http = "*"
objc = "*"   # http's own dep; the resolver validates imports against THIS manifest
```

```cplus
import "http/http" as http;
```

## Common case

```cplus
match http::get("https://example.com/feed.json") {
    result::Result[http::Response, http::Error]::Ok(r) => {
        // A non-2xx status is DATA, not an error. 404 is an answer.
        if r.is_success() { parse(r.body.as_byte_view()); }
    }
    result::Result[http::Response, http::Error]::Err(e) => {
        // Transport only: offline, DNS failure, TLS failure, timeout.
        report(e.code, e.message);
    }
}
```

With a method, headers and a body:

```cplus
var req: http::Request = http::Request::new("https://example.com/v1/items",
                                            method: http::Method::Post,
                                            timeout_seconds: 10.0);
let _a: status::Status = req.set_header("Content-Type", to: "application/json");
let _b: status::Status = req.set_body_text("{\"name\":\"widget\"}");
match http::send(req) { ... }
```

## Blocking by design

Every verb blocks the calling thread. **Call it off-main.** In a facet app that
means a service: produce off the UI thread, hop the result home with
`services::run_on_main` before touching a view. There is no async tier in v1.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Roadmap

Other platforms are not in scope yet, and each is the OS's own client bound the
same way: linux → libcurl, windows → WinHTTP, android → JNI to
`HttpURLConnection`, esp32 → `esp_http_client`.

## Tests

Unit tests live in `src/http.cplus`.

```
cd vendor/http && cpc test
```

The tests named `net_*` make real requests and need the internet; everything
else runs offline, including the transport-error path (a malformed URL is
rejected by NSURLSession locally, before any DNS).
