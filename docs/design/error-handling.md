# Error handling in C+

C+ libraries **do not crash on recoverable failures**. There is no `panic`, no
abort, no exception unwinding, and no trap (`assert` lowers to a bare
`llvm.trap` — SIGILL with no message — and is therefore banned from library
code). A failure that the program could reasonably continue past is reported as
a **value**, and the operation **recovers** to a valid state. Checking the value
is the caller's choice; ignoring it is always safe.

This is the same stance as the no-`null` rule: a missing or failed result is a
value you can branch on, never a hole that detonates later.

## The three return shapes

| Shape | Use for | On failure |
|---|---|---|
| `Status` | a **mutation** that can fail | an error code; the receiver is left valid; **ignorable** |
| `Option[T]` | a **read / lookup** where the value may be absent | `None` |
| `Result[T, E]` | a **value producer** where the *reason* for failure matters | `Err(reason)` |

`Status` (`stdlib/status.cplus`) is the "void result": a payload-free enum
(`Ok | OutOfMemory | OutOfBounds | InvalidInput`), so it is `Copy`, compares with
`==` / `!=`, and answers `is_ok()` / `is_err()`. Ignoring a returned `Status`
leaks nothing. Domain modules may return their own status-style enum when they
need other codes (e.g. a channel returning `Closed`); new shared codes are added
to `Status` as modules need them.

## Authoring rules

1. **Mutators return `Status` and recover.** Do not mutate the receiver until
   every fallible step has succeeded, so an error path leaves it exactly as it
   was. An owned input that cannot be stored is dropped (a `take` parameter drops
   at scope exit on the error path — no leak).

   ```cplus
   fn push(ref this, take value: T) -> status::Status {
       var required: usize = 0 as usize;
       match required_len(this.len, 1 as usize) {
           option::Option[usize]::Some(r) => { required = r; }
           option::Option[usize]::None    => { return status::Status::OutOfMemory; }
       }
       if required > this.cap {
           let st: status::Status = this.grow_to(next_capacity(this.cap, required));
           if st != status::Status::Ok {
               return st;                  // value drops here; vec UNCHANGED
           }
       }
       let slot: *T = { this.ptr + this.len };
       { *slot = value; }                  // mutate only after capacity secured
       this.len = required;
       return status::Status::Ok;
   }
   ```

2. **Constructors recover to a valid-empty object** (no `Status`) when there is a
   valid empty state. On allocation failure they return the empty form; a later
   mutating call retries and reports the failure as a `Status`.

   ```cplus
   fn with_capacity[T](n: usize) -> Vec[T] {
       if n == (0 as usize) { return new::[T](); }
       let raw: *u8 = { malloc(/* checked bytes */) };
       if raw.is_not_null() { return /* Vec over raw */; }
       return new::[T]();                  // OOM -> empty but valid
   }
   ```

   A constructor with **no** valid-empty state (a `Box`/`Arc`/`Rc` is always a
   live heap slot) returns `Option[Self]` instead — `None` on allocation
   failure.

3. **Reads return `Option[T]`.** Out-of-range or missing is `None`, never a trap
   or an out-of-bounds access.

   ```cplus
   fn byte_at(this, i: usize) -> option::Option[u8] {
       if i >= this.len { return option::Option[u8]::None; }
       return option::Option[u8]::Some(this._byte_at_raw(i));
   }
   ```

4. **Value-with-reason returns `Result[T, E]`.** Use when the caller needs the
   produced value on success and a meaningful reason on failure — I/O and parsing,
   not mere absence. `fs`/`net` use `Result[T, IoError]`.

5. **Every `malloc`/`realloc` is null-checked.** On null, free anything already
   allocated in this call, leave the receiver untouched, and report
   `OutOfMemory` (or `None`). Never write through an unchecked allocation.

6. **No `assert` in library code.** A genuine internal invariant that truly
   cannot be violated is restructured so it need not be asserted; a condition the
   caller can trigger is reported, not trapped.

## Consuming results

```cplus
// Ignore — fine for Status; the operation already recovered.
v.push(x);

// Check — the `error != OK` shape.
var e: status::Status = v.push(x);
if e != status::Status::Ok { io::println("push failed"); }   // or e.is_err()

// Match — Option / Result must handle both arms.
match m.get(key) {
    option::Option[i32]::Some(val) => { use_it(val); }
    option::Option[i32]::None      => { use_default(); }
}

// Guard let — bail early (there is no `?` operator).
guard let option::Option[i32]::Some(val) = m.get(key) else { return; };
use_it(val);

// Propagate a Status up (manual; no `?`).
let s1: status::Status = a.push(x);
if s1 != status::Status::Ok { return s1; }
return b.push(y);
```
