# Reference

```cplus
import "http/http" as http;
```

Everything below is in module `http`. macOS and iOS only.

---

## Free functions

### `get`

```cplus
fn get(url: str) -> result::Result[Response, Error]
```

GET `url`, blocking until the server answers or the request times out.
Equivalent to `send(Request::new(url))`.

### `send`

```cplus
fn send(req: Request) -> result::Result[Response, Error]
```

Perform `req`, blocking. `req` is borrowed — only read — so the same request can
be sent again.

`Ok` means an HTTP response arrived, whatever its status. `Err` means none did.

### `method_name`

```cplus
fn method_name(m: Method) -> str
```

The wire spelling: `"GET"`, `"HEAD"`, `"POST"`, `"PUT"`, `"PATCH"`, `"DELETE"`.

---

## `Method`

```cplus
enum Method { Get, Head, Post, Put, Patch, Delete }
```

---

## `Header`

```cplus
struct Header {
    name: text::Text,
    value: text::Text,
}
```

One header field. You rarely build these by hand — `Request::set_header` and the
response reader do it for you.

---

## `Request`

```cplus
struct Request {
    method: Method,
    url: text::Text,
    headers: vec::Vec[Header],
    body: vec::Vec[u8],
    timeout_seconds: f64,
}
```

All fields are public and readable. `timeout_seconds` becomes
`NSURLRequest.timeoutInterval` — Foundation's idle timeout between packets, not
a deadline on the whole transfer.

### `Request::new`

```cplus
fn new(url: str, method: Method = Method::Get, timeout_seconds: f64 = 60.0) -> Request
```

A request with no headers and no body. The URL is the content; the rest defaults
— 60s matches Foundation's own interval.

```cplus
Request::new("https://example.com/feed.json")                                  // a GET
Request::new("https://example.com/v1/items", method: Method::Post)
Request::new(feed_url, timeout_seconds: 4.0)
```

### `set_header`

```cplus
fn set_header(ref this, name: str, to: str) -> status::Status
```

Set `name` to a value, **replacing** any field already under that name
(case-insensitively). Same semantics as `-setValue:forHTTPHeaderField:`.

```cplus
req.set_header("Accept", to: "application/json")
```

### `header`

```cplus
fn header(this, name: str) -> option::Option[text::Text]
```

The value under `name`, or `None`. Case-insensitive. Returns an owned copy.

### `set_body`

```cplus
fn set_body(ref this, bytes: u8[]) -> status::Status
```

Replace the body with a copy of `bytes`.

### `set_body_text`

```cplus
fn set_body_text(ref this, body: str) -> status::Status
```

Replace the body with a copy of `body`'s bytes. Sets no content type — say what
the bytes are with `set_header("Content-Type", to: ...)`.

### `set_timeout`

```cplus
fn set_timeout(ref this, seconds: f64)
```

---

## `Response`

```cplus
struct Response {
    status: i32,
    body: vec::Vec[u8],
    // headers are private; read them with `header`
}
```

A non-2xx `status` is data, not an error.

### `is_success`

```cplus
fn is_success(this) -> bool
```

`200 <= status <= 299`.

### `header`

```cplus
fn header(this, name: str) -> option::Option[text::Text]
```

The value of response header `name`, or `None`. Case-insensitive, owned copy.

### `header_count`

```cplus
fn header_count(this) -> usize
```

### `body_text`

```cplus
fn body_text(this) -> text::Text
```

The body bytes as an owned `Text`. **Not** validated as UTF-8.

For a no-copy read use `r.body.as_byte_view()`, which is a `str` over the same
bytes.

---

## `Error`

```cplus
struct Error {
    code: i64,
    message: text::Text,
}
```

Transport failures only — an exchange that produced no HTTP response.

`code` is Foundation's NSError code (always **negative**) when the request
reached the session, or one of the positive constants below when it never left
the process. `message` is the NSError's `localizedDescription`.

Common Foundation codes:

| code | means |
|------|-------|
| -1001 | timed out |
| -1002 | unsupported URL |
| -1003 | host not found |
| -1004 | could not connect to host |
| -1009 | not connected to the internet |
| -1200 | TLS handshake failed |

---

## Constants

| Name | Value | Meaning |
|---|---|---|
| `ERROR_INVALID_URL` | 1 | `+[NSURL URLWithString:]` returned nil. Rare — Foundation percent-escapes most malformed input and the session reports -1002 instead. |
| `ERROR_TASK_REFUSED` | 2 | No semaphore, or NSURLSession returned no task. |
| `ERROR_NO_RESPONSE` | 3 | The exchange finished with neither an NSError nor an NSHTTPURLResponse (a non-HTTP URL scheme, for instance). |

---

## Dependencies

```toml
[dependencies]
http = "*"
objc = "*"
```

`objc` must be named in the consuming manifest as well: the resolver validates
every import in a build against one flat set taken from that manifest, not from
a dependency's own. Omitting it is `E0852`.

`Foundation` links automatically through this package's `[link]` table.
