# espidf

C+ bindings for **ESP-IDF** on `esp32-xtensa`: GPIO, microsecond clock, sleep,
UART console. Heap-free wrappers suitable for `#[no_alloc]` / realtime loops.

```toml
[dependencies]
espidf = "*"
```

```cplus
import "espidf/gpio" as gpio;
import "espidf/timer" as timer;
import "espidf/task" as task;
import "espidf/log" as log;

gpio::set_direction(2, to: gpio::Mode::Output);
gpio::set_level(2, to: gpio::Level::High);
let t: i64 = timer::now_us();
log::print_i64("us: ", t);
task::delay_ms(500);
```

Build as a staticlib for the ESP-IDF/CMake firmware link:

```
cpc build --target esp32-xtensa
```

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path + firmware handoff
- [docs/guide.md](docs/guide.md) — modules, realtime contracts, gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Host-side unit tests cover enum/ABI mapping (no board required):

```
cd vendor/espidf && cpc test
```
