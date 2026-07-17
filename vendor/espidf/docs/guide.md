# Guide

ESP-IDF bindings for C+ on ESP32 (Xtensa). Fast start: [tutorial.md](tutorial.md).
API: [ref.md](ref.md).

## Scope

| Module | Role |
|---|---|
| `espidf/gpio` | Pin reset, direction, drive, read |
| `espidf/timer` | `esp_timer` monotonic µs since boot |
| `espidf/task` | Sleep via newlib `usleep` |
| `espidf/log` | UART stdout helpers (`printf`) |
| `espidf/espidf` | Umbrella: re-exports `now_us`, `delay_ms` + imports modules |

This is a **small, heap-free** surface for blink / control loops — not a full
ESP-IDF SDK. Wi‑Fi, NVS, peripherals beyond GPIO, FreeRTOS tasks, etc. are
out of scope for this package today.

## Build model

cpc produces a **static library** for `--target esp32-xtensa`. There is no
`[link]` in `Cplus.toml`: symbols come from IDF components (`driver`,
`esp_timer`, newlib). The IDF/CMake tree owns the final firmware link.

The app package is typically a `[lib]`; the main component’s `main.c` only
forwards `app_main` → `cplus_app_main`.

## Realtime / no-alloc

Wrappers avoid heap (`Status`, `Mode`, `Level` are payload-free; log uses
`%.*s` with explicit length). GPIO and timer externs are marked
`#[no_alloc]` + `#[no_block]` where applicable so contract-checked control
loops can call them.

`task::delay_*` **blocks** (sleep) — fine for ordinary tasks, not for
hard realtime ISRs.

## GPIO design

| C | C+ |
|---|---|
| `gpio_mode_t` int | `Mode` enum + `.raw()` |
| 0/1 level | `Level` + `is_high` / `is_low` |
| `esp_err_t` | `Status` (`Ok` or `InvalidInput` for non-zero) |
| `gpio_get_level` -1 | `Option[Level]::None` |

Mutators: `reset`, `set_direction(pin, to:)`, `set_level(pin, to:)`.  
Read: `level(pin) -> Option[Level]`.

Non-zero `esp_err_t` collapses to `Status::InvalidInput` — you lose the
detailed IDF error code in the typed surface (recover via raw externs if
needed).

## Timer and task

- `timer::now_us()` → `esp_timer_get_time()` (i64 µs since boot).
- `task::delay_ms` / `delay_us` → `usleep` (tick-rate independent vs
  `pdMS_TO_TICKS` macros, which cannot be bound).

## Logging

`ESP_LOGx` macros are not bindable. Helpers:

- `print_line(text: str)`
- `print_i32(label: str, value: i32)`
- `print_i64(label: str, value: i64)`

Stdout goes to the serial monitor via IDF/newlib UART. No log levels or
tags.

## Gotchas

### Host `cpc test` vs device

Unit tests only exercise pure enum/status mapping. GPIO/timer/log calls
need the board + full IDF link.

### Pin validity

Invalid pin → `set_*` may return error Status; `level` returns `None`.

### Sleep is not no_block

Do not call `delay_ms` from a path that must never block.

### Umbrella is thin

`espidf/espidf` does not re-export every gpio symbol — import `gpio` /
`log` for those.

### Platform

Documented for **esp32-xtensa** under ESP-IDF. Other chips may need pin
and driver review.

## Typical loop

```cplus
fn cplus_app_main() {
    let pin: i32 = 2;
    let _r: status::Status = gpio::reset(pin);
    let _d: status::Status = gpio::set_direction(pin, to: gpio::Mode::Output);
    loop {
        let _h: status::Status = gpio::set_level(pin, to: gpio::Level::High);
        task::delay_ms(500);
        let _l: status::Status = gpio::set_level(pin, to: gpio::Level::Low);
        task::delay_ms(500);
    }
}
```
