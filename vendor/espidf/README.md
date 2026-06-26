# espidf

C+ bindings for ESP-IDF on the `esp32-xtensa` target: GPIO, the esp_timer
microsecond clock, task sleep, and console output.

## Building

```
cpc build --target esp32-xtensa
```

The consuming package declares `espidf = "*"` under `[dependencies]` and
builds as a `[lib]` staticlib. cpc stops at the archive; the ESP-IDF/CMake
build links the firmware (the bound symbols live in IDF's `driver`,
`esp_timer`, and newlib components).

## Entry convention

ESP-IDF calls `void app_main(void)` after boot. The C+ app exports
`cplus_app_main` and the IDF main component keeps a two-line C shim:

```c
extern void cplus_app_main(void);
void app_main(void) { cplus_app_main(); }
```

The main component's `CMakeLists.txt` links the staticlib:

```cmake
idf_component_register(SRCS "main.c" INCLUDE_DIRS ".")
target_link_libraries(${COMPONENT_LIB} PRIVATE "<path>/libapp.a")
```

## Modules

- `gpio`: `reset`, `set_direction(pin, to: Mode)`, `set_level(pin, to: Level)`,
  `level(pin) -> Option[Level]`. `Mode` (`Disable`/`Input`/`Output`/
  `InputOutput`) and `Level` (`Low`/`High`) are Copy enums; the mutators
  return `Status` (`Ok` on `ESP_OK`).
- `timer`: `now_us` (esp_timer, i64 microseconds since boot)
- `task`: `delay_ms` / `delay_us` (newlib `usleep`, tick-rate independent)
- `log`: `print_line(text: str)` / `print_i32(label: str, value)` /
  `print_i64(label: str, value)` via UART stdout

## API style

The public surface follows `naming_guideline.md`:

- Pin direction and level are role-named Copy enums (`Mode`, `Level`), not
  bare `gpio_mode_t` / 0-or-1 integers.
- GPIO mutators return `Status`; the level read returns `Option[Level]`, so a
  bad-pin fault is a value (`None`), never a -1 sentinel.
- `log` helpers take `str`, not raw NUL-terminated `*u8`; the C bridging
  (`printf("%.*s", len, ptr)`) is hidden inside the helper, so no
  NUL terminator or `CString` allocation is required.

## Notes

- These typed wrappers stay heap-free. `Status`, `Mode`, and `Level` are
  payload-free Copy enums, and the `str` log helpers use the length-aware
  `printf` form — so every wrapper remains a `#[no_alloc]` leaf, usable from
  `#[realtime]` code. Heap types (Text, Vec) still work on the 32-bit target
  (newlib heap) for app code that wants them.
- `#[realtime]` / `#[no_alloc]` code composes with these bindings: the
  gpio/timer externs are `#[no_alloc]` + `#[no_block]` leaves, so a
  contract-checked control loop can drive pins and read the clock.
