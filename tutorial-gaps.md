Yes. I did not “know” those bugs existed. I inferred likely stress points from places where the tutorial’s rules implied hidden machinery that was not explicitly specified.

Here is each point and the reasoning path.

### 1. `self` on non-Copy vs plain parameter

What I noticed:

```cplus
fn f(x: Buf)
fn read(self)
fn write(mut self)
fn into_raw(move self)
```

The tutorial says non-Copy parameters move by default:

```cplus
fn f(x: string) -> string
```

But it also says bare `self` is read-only method access, while `move self` consumes.

So `x: T` and `self` look syntactically similar but behave differently for non-Copy types.

How I figured it:

The language has a deliberate asymmetry:

```text
plain non-Copy param = move
plain non-Copy self = borrow/read
```

That is not necessarily wrong, but it is a classic documentation/spec sharp edge. Any place where the same surface form has different ownership meaning needs a formal matrix and tests.

So I predicted “doc sharp edge,” not necessarily compiler bug.

---

### 2. Returning `borrow`

What I noticed:

```cplus
fn longest(a: borrow A string, b: borrow A string) -> borrow A string
```

But C+ has no `&T` type. The tutorial says borrowing is a parameter marker, not a type.

How I figured it:

A return value is not a parameter. So if `borrow` appears in return position, the compiler needs some way to represent “this returned value is tied to region A.”

That implies hidden lifetime/provenance tracking.

Then the question becomes:

```cplus
let r = longest(a, b);
holder.field = r;
global = r;
return r;
```

What prevents `r` from escaping?

In Rust, the type carries the lifetime. In C+, if the type is still just `string`, then the borrow information must exist in the checker as metadata. The tutorial did not define that metadata or its storage/escape rules.

So I suspected either:

```text
borrow returns are not fully implemented
```

or:

```text
the docs omitted a major part of the type system
```

Codex confirmed the first.

---

### 3. `str` and `T[]` borrow-shaped values

What I noticed:

The tutorial says:

```text
No &T / &mut T reference types
```

but also says:

```text
str = pointer + length, borrowed
T[] = fat-pointer view
```

So even though C+ has no reference syntax, it still has reference-shaped values.

How I figured it:

This example is the obvious lifetime trap:

```cplus
fn bad() -> str {
    let s: string = "hello".to_string();
    return s.as_str();
}
```

If `str` is just a Copy pair of pointer and length, this compiles and dangles.

If `str` is memory-safe, the compiler must track that the returned view is derived from local owned storage.

The tutorial mentioned one special restriction:

```text
str params are rejected in async fn
```

That showed the compiler knows `str` is borrow-shaped in at least one context. But it did not state general escape rules for returning or storing `str`.

So I flagged it as a likely partial implementation: handled for async, maybe not globally.

That is exactly what Codex found.

---

### 4. `mut x: Copy` vs `mut x: NonCopy`

What I noticed:

The parameter table says:

```text
mut x: T on non-Copy = exclusive borrow, mutations propagate back
mut x: T on Copy = pass by value, locally mutable
```

Same syntax, different mutation propagation depending on `Copy`.

How I figured it:

This is coherent, but easy to misread:

```cplus
fn bump(mut x: i32) {
    x = x +% 1;
}
```

does not affect caller.

But:

```cplus
fn bump(mut c: Counter) {
    c.value = c.value +% 1;
}
```

does affect caller if `Counter` is non-Copy.

That is not unsound. It is a teaching/diagnostic edge. An LLM may handle it because the rule is mechanical, but a human might expect `mut` to always mean caller mutation or always mean local mutation.

So I expected “real doc sharp edge, probably acceptable.”

---

### 5. Field move from a type with `Drop`

What I noticed:

The tutorial says:

```cplus
impl Buf {
    fn drop(mut self) { unsafe { free(self.ptr); } }
}
```

and:

```text
Defining drop makes the type non-Copy.
Return values always move.
Non-Copy values move by default.
```

But it did not describe partial moves.

The obvious test is:

```cplus
struct Pair {
    a: string,
    b: string,
}

impl Pair {
    fn drop(mut self) {
        // may read or drop a and b
    }
}

fn take_a(p: Pair) -> string {
    return p.a;
}
```

How I figured it:

Moving `p.a` creates a partially moved `p`.

At scope exit, what happens?

Possible implementations:

```text
1. reject field move from Drop type
2. use drop flags
3. allow and accidentally drop/read moved field
```

Rust has complicated rules here because the problem is genuinely sharp. Since the tutorial did not mention partial-move rules, drop flags, `ManuallyDrop`, destructuring constraints, or “field moves from Drop types are rejected,” this looked like the highest-risk unsoundness.

Codex confirmed option 3.

This was the most predictable concrete compiler bug.

---

### 6. Raw pointer escape from borrowed data

What I noticed:

The tutorial says raw pointer operations require `unsafe`, but raw pointers are `Copy` opaque addresses.

So this is possible:

```cplus
fn leak_ptr(borrow s: string) -> *u8 {
    return s.as_ptr();
}
```

How I figured it:

A raw pointer is intentionally outside the borrow checker in many systems languages. Returning it is usually allowed. Dereferencing it later is where `unsafe` enters.

So I did not call this a language bug. I called it a design edge that must be documented.

The reasoning:

```text
borrow checker protects safe references/views
raw pointers are escape hatches
unsafe deref carries validity obligation
```

Codex confirmed it compiles and UAF occurs if misused, which is acceptable only if the docs say this explicitly.

---

The pattern I used was basically:

Whenever the tutorial says “X is safe / checked / borrow-shaped,” I ask:

```text
Where is that information stored?
How does it escape?
What happens at scope exit?
What happens when moved?
What happens when stored?
What happens when returned?
```

The two biggest red flags were:

```text
borrow exists in return position, but borrow is supposedly not a type
Drop exists, but partial moves are not specified
```

Those almost always hide bugs in a young ownership language.
