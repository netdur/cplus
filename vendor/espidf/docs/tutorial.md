# Tutorial

Blink-shaped loop and firmware entry. Details: [guide.md](guide.md).
Signatures: [ref.md](ref.md).

## Setup

```toml
[dependencies]
espidf = "*"
stdlib = "*"
```

Target **`esp32-xtensa`**. Consume as a `[lib]` staticlib; IDF links the `.a`.

```cplus
import "espidf/gpio" as gpio;
import "espidf/timer" as timer;
import "espidf/task" as task;
import "espidf/log" as log;
import "stdlib/status" as status;
```

Or the umbrella re-exports for clock/sleep:

```cplus
import "espidf/espidf" as idf;
// idf::now_us(), idf::delay_ms(ms)
```

## GPIO

```cplus
let pin: i32 = 2;
let _r: status::Status = gpio::reset(pin);
let _d: status::Status = gpio::set_direction(pin, to: gpio::Mode::Output);
let _h: status::Status = gpio::set_level(pin, to: gpio::Level::High);

match gpio::level(pin) {
    option::Option[gpio::Level]::Some(lv) => {
        if lv.is_high() { /* ... */ }
    }
    option::Option[gpio::Level]::None => { /* bad pin */ }
}
```

## Clock, sleep, log

```cplus
let t0: i64 = timer::now_us();
task::delay_ms(100);
task::delay_us(500);
log::print_line("hello");
log::print_i32("n: ", 42);
log::print_i64("us: ", timer::now_us() -% t0);
```

## Firmware entry

ESP-IDF calls `void app_main(void)`. Export `cplus_app_main` from C+ and
shim in the main component:

```c
extern void cplus_app_main(void);
void app_main(void) { cplus_app_main(); }
```

```cmake
idf_component_register(SRCS "main.c" INCLUDE_DIRS ".")
target_link_libraries(${COMPONENT_LIB} PRIVATE "<path>/libapp.a")
```

## Day-one rules

- Pin numbers are plain `i32` (`gpio_num_t`).
- Mutators return **`Status`** (`Ok` / error); reads use **`Option`**, not -1.
- Log takes **`str`** (length-aware `printf`) — no CString.
- GPIO/timer leaves are **`#[no_alloc]`**; usable from realtime-marked code.
