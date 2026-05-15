# Research: Concurrency in C+ (Threads and Async/Await)

This document outlines the architectural requirements, compiler changes, and standard library additions needed to support threads and `async/await` in C+, based on its current LLVM-backed, borrow-checked design.

---

## Part 1: OS Threads (`std::thread`)

To support native OS threads, C+ must bridge its borrow checker and memory ownership model with POSIX/Windows threading APIs.

### 1. Language & Compiler Requirements
- **Closures or Function + Context**: `pthread_create` takes a function pointer and a `void*` context. Since C+ currently lacks closures, spawning a thread would initially require passing a function pointer and a heap-allocated context struct. Eventually, adding closures (which capture environment) would make the API ergonomic.
- **Thread Safety Bounds (Send/Sync equivalents)**: The borrow checker needs a mechanism to prevent data races. If you send a pointer to a thread, the compiler must guarantee the data outlives the thread, or is exclusively owned by the thread. Without traits (like Rust's `Send`), the compiler might need built-in rules:
  - `move` semantics naturally transfer ownership to a thread.
  - Passing `&mut` or `&` across threads would require scoped threads (where the compiler proves the thread joins before the scope ends).
- **Atomic Intrinsics**: Codegen must support LLVM's atomic operations (`atomicrmw`, `cmpxchg`, `load atomic`, `store atomic`) along with memory ordering (`seq_cst`, `acquire`, `release`, etc.).

### 2. Standard Library (`vendor/stdlib`)
- **`std::thread`**: A wrapper around `pthread_create` (macOS/Linux).
- **Synchronization Primitives**: `Mutex[T]`, `RwLock[T]`, and `Condvar`. `Mutex[T]` would wrap `pthread_mutex_t` and use the borrow checker to ensure the inner data `T` is only accessed when locked.
- **Atomic Types**: `AtomicI32`, `AtomicBool`, etc., wrapping the LLVM intrinsics.

---

## Part 2: `async/await`

Implementing `async fn getResult()` and `await fetch()` is significantly more complex than threads, as it requires pausing and resuming execution. 

### 1. Compilation Strategy: Frontend vs. LLVM Coroutines
C+ compiles to LLVM IR. There are two primary ways to implement `async`:

**Option A: Frontend State Machines (The Rust Way)**
- The `cpc` compiler transforms the AST of an `async fn` into a state machine `enum` and a `poll` method.
- **Pros**: Full control over memory layout, precise borrow checking across `await` points, easily compiled to plain C-like IR.
- **Cons**: Extremely complex to implement in the compiler. Requires massive changes to `sema` and `codegen`.

**Option B: LLVM Coroutines (The C++20 / Swift Way)**
- `cpc` emits relatively normal LLVM IR but inserts `llvm.coro.begin`, `llvm.coro.suspend`, and `llvm.coro.end` intrinsics at `await` points.
- The LLVM middle-end optimizer (CoroEarly, CoroSplit passes) automatically chops the function into a state machine and builds the continuation frame.
- **Pros**: Much less work in `cpc`. LLVM does the heavy lifting of saving/restoring registers and locals.
- **Cons**: Requires allocating the coroutine frame (usually on the heap, requiring an allocator). Debugging LLVM coroutine passes can be notoriously difficult.

*Given C+'s reliance on LLVM for optimizations (Phase 1), Option B (LLVM Coroutines) is likely the most architecturally honest and efficient path.*

### 2. Language & Syntax Changes
- **Keywords**: Add `async` (function modifier) and `await` (expression).
- **Return Types**: An `async fn() -> T` doesn't return `T` immediately. It returns a `Future[T]` (or similar interface). Since C+ doesn't have traits yet, `Future` might need to be a built-in compiler-known type or interface that exposes a `poll` method.
- **Borrow Checking across `await`**: The borrow checker must verify that references held across an `await` point remain valid. If the future is moved between threads, those references must satisfy thread-safety rules.

### 3. The Runtime & Executor
`async` functions do nothing until they are polled. C+ will need an executor (event loop) to drive them.
- **Waker/Context System**: When `fetch()` performs non-blocking I/O and gets `EWOULDBLOCK`, it needs a way to tell the executor "wake me up when the socket is ready". This requires a `Waker` mechanism.
- **I/O Reactor**: The stdlib (or a third-party `cpc` package) needs to implement an event loop using `kqueue` (macOS) or `epoll` (Linux).
- **Usage Example**:
  ```cplus
  // Execution starts by spawning the root future into an executor
  pub fn main() {
      executor::block_on(getResult());
  }
  ```

---

## Part 3: Go-Style Concurrency (Goroutines & Channels)

Go's concurrency model uses **M:N scheduling** (M lightweight goroutines multiplexed onto N OS threads) and built-in channels for communication.

### 1. The Runtime Shift (M:N Scheduling)
- **Heavyweight Runtime**: Unlike C+'s current philosophy (zero-cost abstractions, minimal runtime, C ABI compatibility), Go requires a significant runtime. This includes a work-stealing scheduler and an integrated network poller.
- **Stack Management**: Goroutines start with a small stack (e.g., 2KB) that grows dynamically. C+ would need to implement stack copying or segmented stacks. LLVM supports segmented stacks (`-fsplit-stack`), but it adds overhead to every function call and complicates FFI with C.
- **Context Switching**: The compiler and runtime must coordinate to yield execution (either cooperatively at function calls or preemptively via signals). Rust initially had M:N green threads but removed them before 1.0 specifically because the runtime overhead and FFI complexity violated systems-programming principles.

### 2. Channels
- Channels are synchronized message queues.
- **Good News**: This is purely a data structure. C+ can easily implement channels (e.g., `Channel[T]`) as a library backed by a `Mutex`/`Condvar` or an atomic lock-free queue. You do **not** need a Go-style runtime to have Go-style channels; they work perfectly fine with 1:1 OS threads or async tasks.

### 3. I/O Interception
- Go makes blocking I/O calls appear synchronous, but under the hood, the runtime intercepts them, puts the goroutine to sleep, and schedules another one via `epoll`/`kqueue`.
- To achieve this in C+, the `vendor/stdlib` would have to mandate that all I/O goes through a custom runtime reactor that understands how to park and unpark the lightweight threads.

*Conclusion on Go-style Concurrency: Implementing M:N goroutines would fundamentally change the architecture of C+, shifting it from a low-level systems language (like C/Rust) to a runtime-heavy language (like Go). However, providing Channels as a thread-safe data structure is highly viable and recommended regardless of the underlying execution model.*

---

## Summary of Implementation Phases

If C+ were to implement concurrency, the roadmap would look like:

1. **Phase A: Primitives**
   - Implement `llvm.atomic.*` intrinsics in `cpc`.
   - Build a basic `pthread` wrapper in `vendor/stdlib` to test value movement across OS threads.
2. **Phase B: Thread Safety Checks & Channels**
   - Introduce compiler rules (or traits) to prevent `unsafe` data sharing.
   - Implement `Mutex[T]` and `Channel[T]`.
3. **Phase C: LLVM Coroutines (Async/Await)**
   - Add `async`/`await` keywords.
   - Wire `cpc` codegen to emit `llvm.coro.*` intrinsics.
   - Implement a basic `Future` interface.
4. **Phase D: Async I/O Runtime**
   - Build a `kqueue`/`epoll` reactor in `vendor/stdlib/net`.
   - Implement async wrappers like `TcpStream::read_async`.

---

## Part 4: The `println` Situation

Currently, there is an architectural conflict regarding how `println` works in C+:

1. **The Magic Intrinsic**: Historically, the compiler implemented `println` as a magic intrinsic (e.g., `println(i: i32)` and `println(s: str)`). This was necessary early on so that test programs could output values before the language had a real standard library or string interpolation.
2. **The Stdlib Definition**: In Phase 3C (Stdlib bootstrap), `vendor/stdlib/src/io.cplus` defines `pub fn println(s: str)`.
3. **The Conflict**: C+ explicitly avoids function overloading. If `stdlib/io` provides `println(s: str)`, the magic `println(n: i32)` intrinsic breaks the "no magic / honest FFI" rules. 

### What You Should Do About It

Because Phase 8 successfully shipped **String Interpolation** (`"${expr}"`) and the `ToString` interface, you no longer need the compiler to magically understand how to print integers.

**The Action Plan:**
1. **Implement `stdlib/io::println`**: Replace the `TODO` in `vendor/stdlib/src/io.cplus` with an actual `extern fn printf` or `write` call that takes the `str` slice.
2. **Remove the Compiler Intrinsics**: Delete the magic `println` and `print` special cases from the `cpc` compiler's `sema.rs` and `codegen.rs`.
3. **Migrate the Examples**: Update all existing `.cplus` examples (like `fizzbuzz.cplus`). 
   - Add `import "stdlib/io" as io;`
   - Change calls like `println(i)` to `io::println("${i}")`. 

This completes the transition of I/O out of the compiler and into the library space, honoring C+'s architectural goals.
