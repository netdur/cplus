# C vs C+ vs C++

A comparison of three systems languages that target the same bare-metal niche —
no mandatory runtime, no garbage collector, direct control over memory and
layout — but make different trade-offs between control, expressiveness, and
compiler-enforced safety.

The aim here is a balanced description, including where each language is weak.

| | C | C++ | C+ |
|---|---|---|---|
| First released | 1972 | 1985 | 2024 (this repo) |
| Maturity | Decades-stable, ISO standard | Decades-stable, ISO standard | Pre-1.0, single implementation |
| Core idea | Portable assembler | Zero-overhead abstraction over C | Borrow-checked safety with a C ABI |
| Spec size | Small | Very large | Small |
| Ecosystem | Vast | Vast | Minimal |

---

## 1. Philosophy

**C** keeps a thin layer over the hardware. The language is small and the
compiler does little beyond translation; correctness is the programmer's
responsibility. This is both its strength (predictable, portable, ubiquitous)
and its weakness (entire bug classes are well-formed programs).

**C++** adds abstraction without giving up control: RAII, templates, classes,
operator overloading, the STL. The principle is "you don't pay for what you
don't use." The cost is size and complexity — the language is large, has many
overlapping ways to do most things, and retains all of C's footguns alongside
its own.

**C+** moves more correctness checks into the compiler: an ownership/borrow
system, no null, no implicit conversions, exhaustive pattern matching. It keeps
the language deliberately small (one obvious way to do things). The cost is
youth (unstable, tiny ecosystem), verbosity, and reduced flexibility (no
inheritance, no templates, no exceptions).

None of these is strictly "better" — they sit at different points on the
control/safety/maturity triangle.

---

## 2. Compilation & toolchain

| | C | C++ | C+ |
|---|---|---|---|
| Frontends | clang, gcc, msvc, many | clang, gcc, msvc, many | one (`cpc`) |
| Backend | LLVM / GCC / others | LLVM / GCC / others | emits LLVM IR, calls clang |
| Compilation unit | translation unit + `#include` | translation unit + (C++20) modules | module file with `import` |
| Generics | macros / `_Generic` | template instantiation | monomorphization |
| Build systems | Make, CMake, autotools, … | CMake, Bazel, Meson, … | `Cplus.toml` |
| Cross-compilation | mature, universal | mature, universal | early |

C and C++ have multiple independent, standardized implementations and decades
of tooling (debuggers, profilers, sanitizers, static analyzers, IDEs). C+ has a
single implementation and correspondingly thin tooling. For anything where
toolchain maturity matters, this is a real and large difference.

---

## 3. Memory & ownership

**C** — manual `malloc`/`free`, no ownership tracking. Use-after-free,
double-free, leaks, and overruns are all valid programs. Detection is a runtime
concern (ASan, Valgrind), not a compile-time guarantee.

**C++** — RAII makes destruction deterministic at scope exit, and
`unique_ptr`/`shared_ptr` encode ownership as library types. But raw
`new`/`delete`, raw pointers, and manual lifetimes remain available, the
compiler does not prevent dangling references, and `std::move` is a cast rather
than a checked transfer (using a moved-from object compiles). RAII is a major
ergonomic and safety improvement over C; it is not a guarantee.

**C+** — ownership and borrowing are compiler-checked. There are no reference
types; borrowing is expressed as a parameter marker:

| Marker | Meaning | Closest C++ analogue |
|---|---|---|
| `x: T` / `move x: T` | move (default for non-Copy) | by value after `std::move` |
| `mut x: T` | exclusive borrow, mutations propagate | `T&` |
| `borrow x: T` | shared borrow, caller retains | `const T&` |

Destruction is deterministic (`fn drop`, plus `defer`). The borrow checker
rejects use-after-move and overlapping mutable borrows at compile time. The
trade-off is the usual one for borrow checking: a learning curve, and some
correct programs are rejected and must be restructured. C+'s checker is also far
younger and less battle-tested than, say, Rust's.

```cplus
// C+ — using a moved value is a compile error
fn consume(move b: Buf) { /* ... */ }
let b = make_buf();
consume(b);
let n = b.len();   // error[E0335]: use of moved value `b`
```
```cpp
// C++ — the same shape compiles; b is in a valid-but-unspecified state
void consume(Buf b);
Buf b = make_buf();
consume(std::move(b));
auto n = b.len();
```

---

## 4. Type system

| Feature | C | C++ | C+ |
|---|---|---|---|
| Implicit numeric conversion | yes | yes | no (explicit `as`) |
| Null | `NULL` | `nullptr` | none (`Option[T]`) |
| Sum types | manual tagged union | `std::variant` | first-class `enum` + `match` |
| Generics | macros | templates (Turing-complete) | monomorphized `[T]` + interface bounds |
| Overloading | no | yes (fn + operator) | no |
| Inheritance / virtual | no | yes | no |
| Pattern matching | `switch` | `switch` / `if constexpr` | `match` / `guard let` |

C++ has by far the most expressive type system of the three — templates and
inheritance enable designs the others cannot express directly. C+ trades that
expressiveness for fewer silent failure modes (no implicit narrowing, no null,
exhaustive matches) and a smaller surface. The cost of C+'s choices is
verbosity: explicit casts everywhere, no overloading means more distinct names,
no inheritance means composition by hand.

---

## 5. Error handling

| | C | C++ | C+ |
|---|---|---|---|
| Idiom | return codes + `errno` | exceptions (+ `std::expected`, C++23) | tagged-union return values |
| Exceptions | none | yes | none |
| Propagation sugar | none | none / `try` blocks | none |
| Happy-path cost | branch | zero until thrown | branch |

C++ exceptions allow terse happy-path code and automatic propagation, at the
cost of non-local control flow and binary-size/unwinding overhead. C and C+ both
make errors explicit values, which keeps control flow visible but is more
verbose at every call site. C+ has no `?`-style propagation operator, so this
verbosity is more pronounced than in languages that do.

---

## 6. Metaprogramming

- **C** — the preprocessor (`#define`, `#include`, `#if`) and `_Generic`.
  Powerful, unhygienic, text-based.
- **C++** — the preprocessor plus templates plus `constexpr`/`consteval`.
  Extremely powerful (compile-time computation, generic libraries), and a
  frequent source of slow builds and hard-to-read diagnostics.
- **C+** — no preprocessor and no user macros. A fixed set of compiler
  intrinsics (`#size_of`, `#align_of`, `#include_bytes`, `#env`, etc.) covers
  the common compile-time needs. This keeps builds simple and code uniform, at
  the cost of not being able to express the kind of generic/compile-time
  libraries C++ templates allow.

---

## 7. Concurrency

| | C | C++ | C+ |
|---|---|---|---|
| Threads | pthreads / C11 threads | `std::thread` | `thread::spawn` |
| Async | none (libraries) | C++20 coroutines | `async`/`await` + reactor |
| Atomics | `<stdatomic.h>` | `std::atomic` | stdlib atomics + fences |
| Data-race prevention | none | none (UB on race) | borrow checker + `Send` bounds |

C and C++ leave data-race freedom to the programmer. C+ extends its ownership
checks across thread boundaries, turning some race conditions into compile
errors — with the same caveat as §3 that the checker is young.

---

## 8. ABI & interop

All three target the C ABI, which is the lingua franca for native linking.

- **C** is the C ABI. Everything links to it.
- **C++** can call C directly, and exposes C-callable surfaces via
  `extern "C"`. It is the only one of the three that can *consume* C++ libraries
  natively, because it speaks the C++ ABI (mangling, vtables, non-trivial type
  passing, exceptions).
- **C+** links against C symbols via `extern fn` declarations (a bindgen tool
  can generate these from headers), and exports C-callable object files. It does
  not compile C or C++ source, and cannot consume a C++ library directly.

Consuming a C++ library from anything other than C++ (C, C+, Rust, Go, Zig, …)
requires the same approach: a thin `extern "C"` wrapper in C++ that exposes a C
surface, which the other language then binds to. This is a property of the C++
ABI, not of any one of these languages.

C+'s interop is therefore comparable to C's: excellent for C, indirect for C++.
C++'s is strictly broader (it is the only one that needs no wrapper for C++).

---

## 9. What the compiler catches

This table is where the languages most visibly differ. "Compile-time" is
genuinely stronger than "runtime tool can find it," but note the trade: C+ buys
these guarantees with verbosity and borrow-checker friction, and C++ buys
flexibility C+ lacks.

| Bug class | C | C++ | C+ |
|---|---|---|---|
| Use-after-free | runtime tooling | runtime tooling | compile-time |
| Double-free | runtime | runtime | compile-time |
| Use of moved-from value | n/a | legal | compile-time |
| Null deref | runtime | runtime | eliminated (no null) |
| Implicit narrowing | silent | mostly silent | compile-time |
| Non-exhaustive cases | silent | silent | compile-time |
| Data race | runtime | UB | compile-time |
| Buffer overrun | silent | silent (raw) / checked (`.at()`) | bounds-checked outside `unsafe` |

C+ provides an `unsafe { }` escape hatch that recovers C-level capability
(raw deref, raw indexing, `extern` calls) where needed.

---

## 10. Feature matrix

| | C | C++ | C+ |
|---|---|---|---|
| Manual memory, no GC | ✓ | ✓ | ✓ |
| Deterministic destruction (RAII) | ✗ | ✓ | ✓ |
| Borrow checker | ✗ | ✗ | ✓ |
| Exceptions | ✗ | ✓ | ✗ |
| Templates / heavy metaprogramming | ✗ | ✓ | ✗ |
| Overloading | ✗ | ✓ | ✗ |
| Inheritance / virtual dispatch | ✗ | ✓ | ✗ |
| Sum types + exhaustive match | ✗ | partial (`variant`) | ✓ |
| Null in the language | ✓ | ✓ | ✗ |
| Implicit conversions | ✓ | ✓ | ✗ |
| Preprocessor / user macros | ✓ | ✓ | ✗ |
| Closures / lambdas | ✗ | ✓ | ✗ |
| Consume C directly | ✓ | ✓ | via `extern` decls |
| Consume C++ directly | ✗ | ✓ | ✗ |
| Mature multi-vendor toolchain | ✓ | ✓ | ✗ |
| Large ecosystem | ✓ | ✓ | ✗ |

---

## 11. Same program, three ways

A bounded string-to-integer parse.

```c
// C — out-param + sentinel return; caller must check
int parse_u32(const char *s, uint32_t *out) {
    uint64_t acc = 0;
    for (; *s; s++) {
        if (*s < '0' || *s > '9') return -1;
        acc = acc * 10 + (*s - '0');
        if (acc > UINT32_MAX) return -1;
    }
    *out = (uint32_t)acc;
    return 0;
}
```
```cpp
// C++ — optional (or throw); concise, uses the STL
std::optional<uint32_t> parse_u32(std::string_view s) {
    uint64_t acc = 0;
    for (char c : s) {
        if (c < '0' || c > '9') return std::nullopt;
        acc = acc * 10 + (c - '0');
        if (acc > std::numeric_limits<uint32_t>::max()) return std::nullopt;
    }
    return static_cast<uint32_t>(acc);
}
```
```cplus
// C+ — tagged enum; caller must match. Explicit casts, more verbose.
enum Parse { Ok(u32), BadDigit, Overflow }

fn parse_u32(s: str) -> Parse {
    let p: *u8 = str_ptr(s);
    let n: usize = str_len(s);
    let mut acc: u64 = 0 as u64;
    let mut i: usize = 0 as usize;
    while i < n {
        let b: u8 = unsafe { p[i] };
        if b < (48 as u8) { return Parse::BadDigit; }
        if b > (57 as u8) { return Parse::BadDigit; }
        acc = acc *% (10 as u64) +% ((b -% (48 as u8)) as u64);
        if acc > (4294967295 as u64) { return Parse::Overflow; }
        i = i +% (1 as usize);
    }
    return Parse::Ok(acc as u32);
}
```

The C++ version is the shortest and leans on the STL. The C+ version is the most
explicit — mandatory casts, an exhaustive enum the caller must handle, no null —
which is more verbose but harder to misuse silently. The C version is the most
portable and the easiest to call wrong.

---

## 12. Choosing

- **C** when portability, a tiny runtime, ABI ubiquity, or toolchain
  availability dominate, and you accept full responsibility for safety.
- **C++** when you need its expressiveness (templates, RAII libraries, the STL,
  inheritance), must interoperate with existing C++, or want a mature ecosystem
  — and can manage its size and footguns.
- **C+** when you want compiler-enforced memory/ownership safety with a C ABI
  and a small, uniform language, and can accept a pre-1.0 toolchain, a minimal
  ecosystem, more verbosity, and less flexibility than C++.

The honest summary: C is the smallest and most universal but least safe; C++ is
the most powerful and mature but the largest and most complex; C+ is the safest
by construction but the youngest and least proven, with the smallest ecosystem.
