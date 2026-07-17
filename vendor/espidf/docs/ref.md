# Reference

Manual for the `espidf` package.

```cplus
import "espidf/gpio" as gpio;
import "espidf/timer" as timer;
import "espidf/task" as task;
import "espidf/log" as log;
// optional umbrella:
import "espidf/espidf" as idf;
```

---

## Module `gpio`

### `Mode`

```cplus
enum Mode { Disable, Input, Output, InputOutput }

fn raw(this) -> i32   // gpio_mode_t: 0..3
```

### `Level`

```cplus
enum Level { Low, High }

fn raw(this) -> u32   // 0 or 1
fn is_high(this) -> bool
fn is_low(this) -> bool
```

### Functions

```cplus
fn reset(pin: i32) -> status::Status
fn set_direction(pin: i32, to: Mode) -> status::Status
fn set_level(pin: i32, to: Level) -> status::Status
fn level(pin: i32) -> option::Option[Level]
```

Non-zero `esp_err_t` → `Status::InvalidInput`.  
`gpio_get_level` not 0/1 → `None`.

Marked `#[no_alloc]` (and externs `#[no_block]`).

---

## Module `timer`

```cplus
fn now_us() -> i64
```

`esp_timer_get_time()` — microseconds since boot. `#[no_alloc]`.

---

## Module `task`

```cplus
fn delay_ms(ms: u32)
fn delay_us(us: u32)
```

`usleep` (newlib / FreeRTOS under IDF). **Blocking.**

---

## Module `log`

```cplus
fn print_line(text: str)
fn print_i32(label: str, value: i32)
fn print_i64(label: str, value: i64)
```

UART stdout via `printf("%.*s", …)`. `#[no_alloc]`.

---

## Module `espidf` (umbrella)

```cplus
fn now_us() -> i64      // timer::now_us
fn delay_ms(ms: u32)    // task::delay_ms
```

Also imports `gpio`, `timer`, `task`, `log` for `cpc test` discovery when
used as the package entry.

---

## Package

| | |
|---|---|
| Package name | `espidf` |
| Target | `esp32-xtensa` (firmware via ESP-IDF) |
| Dependencies | `stdlib` (`status`, `option`) |
| Link | none in cpc — IDF components provide symbols |
| Tests | `cpc test` (enum/status mapping on host) |
