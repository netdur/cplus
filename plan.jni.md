# Java Native Interface (JNI) Interop for C+

This plan outlines how C+ can natively interoperate with Java via the Java Native Interface (JNI), enabling the creation of Android NDK libraries or JVM extensions entirely in C+.

## Status (adopted 2026-06-01, `51e9171`)

**Shipped: `vendor/jni`.** The minimal binding is adopted and consumable. The
ZERO-compiler-features claim below is **confirmed** — `vendor/jni` compiles and
runs with no `cpc` change, and is now the smallest proof-point that cpc handles
function-pointer struct fields plus a type that references itself through a
pointer (`JNIEnv = *JNINativeInterface`, used inside the struct's own fields).

Two things the adoption taught (both fixed in the package):
- **Directory must match the package name.** Dependency resolution keys on
  `vendor/<depname>/`, and a dep name can't contain a hyphen (`E0857`), so the
  original `jni-min` dir was unresolvable. Shipped as `vendor/jni`.
- **The table fields must be `pub`.** Reading a fn-ptr out of the table
  (`(*env).GetVersion`) is the whole point; without `pub` every access is
  `E0403`. The `Call*Method`-truncated table is `pub` through its defined
  fields; `reserved*` stay private (layout is unaffected by visibility).

A layout `#[test]` pins the table sizes (344 / 64 bytes on 64-bit) so a dropped
or mistyped field — which would silently shift every later offset — fails the
build.

## Open Questions

- **Package Structure**: ~~Should we create a `jni-min` package?~~ **Resolved** —
  shipped as `vendor/jni` (see Status).
- **C-Strings**: ~~JNI heavily relies on null-terminated strings; the package
  expects the `"string\0"` workaround at call sites.~~ **Resolved** — `c"..."`
  C-string literals shipped (a bare `*u8` to a NUL-terminated `.rodata` blob),
  so call sites can write `FindClass(env, c"java/lang/String")` instead of the
  `"...\0"` workaround.
- **Round-trip verification**: the layout `#[test]` covers offsets, but the full
  `.so` + JVM `System.loadLibrary` round-trip (Verification Plan below) is **not
  yet run** — it needs a JVM/Java toolchain in the test environment.

## Proposed Changes

We do not need to modify the `cpc` compiler. Instead, we can build a `jni` wrapper package in C+.

### 1. Types and Primitives
Map JNI types directly to C+ primitives.
```cplus
type jint = i32;
type jlong = i64;
type jboolean = u8;
type jobject = *u8;
type jclass = *u8;
type jstring = *u8;
```

### 2. The JNINativeInterface Struct
JNI is fundamentally a large C struct filled with function pointers. We can define this in C+ using the newly added `fn(...) -> T` type and `#[repr(C)]`.

```cplus
#[repr(C)]
pub struct JNINativeInterface {
    reserved0: *u8,
    reserved1: *u8,
    reserved2: *u8,
    reserved3: *u8,
    GetVersion: fn(*u8) -> jint,
    DefineClass: fn(*u8, *u8, jobject, *u8, jint) -> jclass,
    FindClass: fn(*u8, *u8) -> jclass,
    // ... (map the rest of the JNIEnv function pointers)
}

// JNIEnv is a pointer to the interface pointer
type JNIEnv = *JNINativeInterface;
```

### 3. Calling JVM Functions
To call back into Java, C+ will dereference the `JNIEnv` pointer to access the function pointer, and invoke it.

```cplus
fn print_class_name(env_ptr: JNIEnv, this_obj: jobject) {
    // 1. Deref the environment to get the struct of function pointers
    let env: JNINativeInterface = unsafe { *env_ptr };
    
    // 2. Call the function pointer (passing the env_ptr as the first arg)
    let cls: jclass = env.FindClass(env_ptr, "java/lang/String\0");
}
```

### 4. Exporting Native Methods to Java
Java discovers native functions by looking for unmangled symbols in the `.so` library matching the pattern `Java_PackageName_ClassName_MethodName`.

Since `cpc` exports `pub fn` unmangled when building a library (as proven by the `lib_target_exposes_pub_symbols_unmangled` test), exporting a JNI function is as simple as defining a `pub fn`:

```cplus
// Corresponds to: package com.example; class App { native int add(int a, int b); }
pub fn Java_com_example_App_add(env: JNIEnv, this_obj: jobject, a: jint, b: jint) -> jint {
    return a +% b;
}
```

## Verification Plan

### Automated Tests
- Create a `vendor/jni-min` directory with the `JNINativeInterface` definition.
- Build a `.so` (or `.dylib`) library using C+.
- Write a tiny Java application that uses `System.loadLibrary` to load the C+ library and call a native `add` method to verify round-trip execution.
