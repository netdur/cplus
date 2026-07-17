# log

Leveled logging to stderr: timestamp, fixed-width level tag, optional ANSI color.

```toml
[dependencies]
log = "*"
```

```cplus
import "log/log" as log;

log::set_max_level(log::Level::Info);   // default is already Info
log::info("server listening");
log::warn("retrying connection");
log::error("bind failed");
```

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

```
cpc test
```
