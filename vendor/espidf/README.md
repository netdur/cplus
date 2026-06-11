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

- `gpio`: `reset`, `set_direction`, `set_level`, `get_level`, mode values
- `timer`: `now_us` (esp_timer, i64 microseconds since boot)
- `task`: `delay_ms` / `delay_us` (newlib `usleep`, tick-rate independent)
- `log`: `print_line` / `print_i32` / `print_i64` via UART stdout

## Notes

- Heap types (Text, Vec, str) work on the 32-bit target (newlib heap);
  the bindings themselves stick to `i32`/`u32`/`*u8` and `#str_ptr`
  literals so they stay usable from `#[no_alloc]` code.
- `#[realtime]` / `#[no_alloc]` code composes with these bindings: the
  gpio/timer externs are `#[no_alloc]` + `#[no_block]` leaves, so a
  contract-checked control loop can drive pins and read the clock.
