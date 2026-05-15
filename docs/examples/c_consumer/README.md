# C consumer reference

Phase 5 reference example. Shows how to build a C+ library and call into it
from C, exercising every C-ABI class the compiler supports today.

## Layout

```
c_consumer/
├── mathlib/                 ← the C+ library
│   ├── Cplus.toml           ← [lib] crate-type = "both"
│   └── src/lib.cplus        ← one pub extern fn per ABI class
└── c_user/                  ← the C consumer
    ├── c_user.c             ← calls every export, asserts results
    └── Makefile             ← drives `cpc build` then clang
```

## Run it

```bash
$ cd c_user
$ make check                 # builds everything, runs c_user, prints "OK"
```

The `make check` target is also what the CI test
`c_consumer_reference_example_runs_clean` invokes — keep this script
working and you keep the user-facing slice working.

## What each ABI class exercises

| Export | Class | C signature |
|---|---|---|
| `add(i32, i32) -> i32` | Direct scalar | `int32_t add(int32_t a, int32_t b);` |
| `square(Point) -> i32` | ≤8B aggregate → `i64` coerce | `int32_t square(Point p);` |
| `make_point(...) -> Point` | ≤8B return coerce | `Point make_point(int32_t x, int32_t y);` |
| `sum_pair(Pair) -> i64` | 16B aggregate → `[2 x i64]` | `int64_t sum_pair(Pair p);` |
| `make_pair(...) -> Pair` | 16B return coerce | `Pair make_pair(int64_t a, int64_t b);` |
| `sum_triple(Triple)` | >16B → indirect ptr | `int64_t sum_triple(Triple t);` |
| `make_triple(...) -> Triple` | >16B return → sret | `Triple make_triple(int64_t, int64_t, int64_t);` |
| `color_index(Color)` | plain enum → `i32` | `int32_t color_index(Color c);` |
| `fill_with(*i32, i32)` | raw pointer | `void fill_with(int32_t *out, int32_t value);` |
| `apply(fn(i32)->i32, i32)` | fn-pointer arg | `int32_t apply(int32_t (*f)(int32_t), int32_t x);` |

## Design note

[docs/design/phase5-c-abi-export.md](../../design/phase5-c-abi-export.md)
has the rationale, the locked decisions, and the worked ABI examples
showing what cpc emits at the LLVM level for each class.
