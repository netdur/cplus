# share

Hand something to the rest of the system — the platform's own sheet.

```toml
[dependencies]
share = "*"
```

```cplus
import "share/share" as sh;

sh::text("look at this");
sh::url("https://example.com/thing");
sh::file("/path/to/report.pdf");
```

## The return value is "the sheet opened"

Never "the person shared". No platform reports reliably which app was chosen or
whether the send completed — iOS's completion handler reports cancellation but
lies about success for several targets, and Android's chooser reports nothing
at all. An app that needs to know something was sent has to observe the result
itself.

## Three verbs, not one with a kind

A URL gets a preview card, a different app list and "Copy Link" instead of
"Copy". A file goes through a provider a string never touches. A caller who has
a URL should say so.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| `text` | ✅ | ✅ | ✅ |
| `url` | ✅ real `NSURL` | ✅ real `NSURL` | ✅ as text — Android has no link intent |
| `file` | ✅ | ✅ | ❌ needs an app-level `FileProvider` |
| `subject` | dropped | dropped | ✅ `EXTRA_SUBJECT` |

- [tutorial](docs/tutorial.md) · [guide](docs/guide.md) · [ref](docs/ref.md)

## Tests

    cd vendor/share && cpc test

A sheet cannot be asserted — it needs eyes.
