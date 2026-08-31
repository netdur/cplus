# applinks

A link that opens this app, routed to whoever asked for it — deep links
(`myapp://record/42`) and universal / app links (`https://example.com/record/42`)
through one door, cold start and warm alike.

```toml
[dependencies]
stdlib      = "*"
facet       = "*"
flex_layout = "*"
events      = "*"
applinks    = "*"
```

```cplus
import "applinks/applinks" as applinks;
import "stdlib/url" as url;

fn opened(u: str, ctx: *u8) {
    match url::parse(u) {
        option::Option[url::Url]::Some(p) => {
            // myapp://record/42  ->  "record", "42"
            let route: str = p.segment(0 as usize);
            let id: str    = p.segment(1 as usize);
        }
        option::Option::None => { }
    }
    return;
}

let _o: applinks::Outcome = applinks::on_link(opened, scheme: "myapp");
```

Register the handler wherever is convenient — a link that launched the process
is replayed to one that turns up afterwards.

**Code alone receives nothing.** The scheme has to be registered in
`Info.plist` or `AndroidManifest.xml`, and a universal link needs an
entitlement and a file on your web server as well. `cpc init` writes the first
two, commented out; [docs/guide.md](docs/guide.md) has all four and the several
ways each one silently does nothing.

- [docs/tutorial.md](docs/tutorial.md) — depend, register, open a URL
- [docs/guide.md](docs/guide.md) — the two kinds of link, filters, cold start,
  why `host:` refuses a custom scheme, and what to check when links do not arrive
- [docs/ref.md](docs/ref.md) — every signature

No platform half: this package names no backend. The link arrives through
whichever facet backend the app already uses, as `app_events::E_OPEN_URL`.

## Tests

```
cd vendor/applinks && cpc test
```

Unit tests are `#[test]` blocks in `src/applinks.cplus`. What they cannot reach
— a backend actually firing the event — is covered per platform; see the
testing section of the guide.
