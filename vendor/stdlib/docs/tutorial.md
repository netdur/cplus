# Tutorial

Quick path through the modules you need on day one. Module map and deeper
rules: [guide.md](guide.md). Signatures: [ref.md](ref.md).

## Setup

```toml
[dependencies]
stdlib = "*"
```

Import only what you use:

```cplus
import "stdlib/option" as option;
import "stdlib/result" as result;
import "stdlib/status" as status;
import "stdlib/vec" as vec;
import "stdlib/text" as text;
import "stdlib/io" as io;
```

## Option and Result

```cplus
let o: option::Option[i32] = option::some(42);
match o {
    option::Option[i32]::Some(n) => { /* n */ }
    option::Option[i32]::None => {}
}

let r: result::Result[i32, result::IoError] = result::io_ok(7);
match r {
    result::Result[i32, result::IoError]::Ok(v) => { /* v */ }
    result::Result[i32, result::IoError]::Err(e) => { /* e */ }
}
```

Mutators that can fail without producing a value use `status::Status`
(`Ok` / `OutOfMemory` / …) — see the guide.

## Vec

```cplus
var v: vec::Vec[i32] = vec::new::[i32]();
let _ok: status::Status = v.append(1);
let _ok2: status::Status = v.append(2);
match v.at(0usize) {
    option::Option[i32]::Some(x) => { /* x == 1 when T is Copy */ }
    option::Option[i32]::None => {}
}
let n: usize = v.count();
```

For non-`Copy` elements, prefer `at_ptr` / `each_ref` / `fold_ref` so you do
not move out of the vector.

## Text and str

```cplus
var t: text::Text = text::from_str("hello");
let _s: status::Status = t.append(" world");
let view: str = t.view();          // borrow; do not keep past mutation/drop
io::println(view);
```

`str` is a borrowed view; `Text` is the owned growable string. Prefer `str`
parameters at APIs; own with `Text` when you need to store or build.

`str` has methods of its own (importing `stdlib/text` brings them in;
without text, write `import "stdlib/str" as _;` — the discard alias, for
imports that exist only to bring a method set into the build). It spells length `count()`, like every stdlib
container; there is no `len()`:

```cplus
let s = "  a,b,c  ";
let t = s.trim();                          // "a,b,c" — a view, nothing copied
t.count();                                 // 5 (bytes)
t.contains("b");                           // true
t.has_prefix("a");  t.find(",");           // bool / Option[usize]
let parts = t.split(separator: ",");       // Vec[str] of views
"42".to_i64();  "2.5".to_f64();            // Option — strict, trim() first
"file.txt".removing_suffix(".txt");        // "file" — view, unchanged if absent
```

Sub-view results (`trim`, `slice`, `prefix`, `split` pieces, …) borrow the
same bytes as the receiver, so they follow the same lifetime rule as
`view()`. Operations that must allocate — uppercasing, replacing, padding,
building — live on `Text`; convert with `s.to_text()`. Slices and arrays
have the same two core reads: `xs.count()`, `xs.is_empty()`.

## Print

```cplus
io::print("hi");
io::println("line");
io::eprintln("diag");              // stderr
```

## Files (sketch)

```cplus
import "stdlib/fs" as fs;

guard let result::Result[fs::File, result::IoError]::Ok(f) = fs::open_read("a.txt") else {
    return;
};
// File Drop closes the fd
```

## Day-one rules

- Import **`stdlib/<module>`**, not a single mega-module.
- Fallible work returns `Result`, `Option`, or `Status` — libraries do not trap.
- `Text` / `Vec` own heap data; dropping them frees children recursively.
- Platform-specific files (`netsys_linux`, `reactor_windows`, …) are selected
  by the resolver; you still import `stdlib/net`, `stdlib/reactor`, etc.
