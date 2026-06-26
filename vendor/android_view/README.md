# android_view

Android View bindings for C+, layered on `vendor/jni`. Validated end to end:
the example below renders on a Pixel emulator (API 36) — the staticlib from
`cpc build --target android-arm64`, linked into `libapp.so` by the NDK's
clang, loaded by the two-method `MainActivity` host below.

This mirrors the AppKit/UIKit package shape:

- `runtime`: JNI environment helpers, method calls, UTF strings, global refs.
- `activity`: borrowed `Activity` wrapper and `setContentView`.
- `view`: `View` and `LinearLayout`.
- `controls`: `TextView` and `Button` (text + `set_on_click`).
- `listener`: self-contained click handling (package-shipped DEX adapter);
  imported separately, not via the umbrella, because it obligates the app
  to export the `cplus_on_click` hook.
- `android_view`: umbrella module.

## Host Contract

Android still needs a JVM-side entry component. The intended shape is a tiny
`Activity` that loads the native library and calls into C+:

```java
public final class MainActivity extends android.app.Activity {
    static { System.loadLibrary("app"); }

    private static native android.view.View nativeCreateView(MainActivity self);

    @Override protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        setContentView(nativeCreateView(this));
    }
}
```

The native entrypoint receives `JNIEnv *` and the `Activity`. In C+ it should
convert the native env pointer with `android_view::from_native(envp)`, build a
View tree, and return the root object. The root should be returned as a raw
`jobject`; the JVM parent will retain it.

## Example Shape

```cplus
import "android_view/android_view" as av;
import "jni/jni" as jni;

// `nativeCreateView` is a *static* native method, so JNI passes
// (env, class, args...): the second parameter is the jclass, the third
// is the MainActivity argument.
pub extern fn Java_com_example_MainActivity_nativeCreateView(
    envp: *jni::JNIEnv,
    cls: jni::jobject,
    activity_obj: jni::jobject,
) -> jni::jobject {
    let env: av::Env = av::from_native(envp);
    let act: av::Activity = av::Activity::from_borrowed(env, activity_obj);

    var root: av::LinearLayout = av::LinearLayout::new(env, act.as_context());
    root.set_orientation(av::orientation_vertical());

    let title: av::TextView = av::TextView::new(env, act.as_context());
    title.set_text(#str_ptr("Hello from C+\0"));
    root.add_view(title.as_view_obj());

    return root.into_raw();
}
```

## Building and packaging

cpc stops at the staticlib; the NDK's clang links the shared library the
JVM loads (`--whole-archive` keeps the JNI exports no Java code references
at link time):

```sh
cpc build --target android-arm64
$NDK/toolchains/llvm/prebuilt/<host>/bin/clang -target aarch64-linux-android24 \
    -shared -Wl,--whole-archive target/android-arm64/debug/libapp.a \
    -Wl,--no-whole-archive -o lib/arm64-v8a/libapp.so
```

`libapp.so` goes into the APK at `lib/arm64-v8a/` (a Gradle project places
it via `jniLibs`). The listener module needs minSdk 26+
(InMemoryDexClassLoader); everything else runs on 24+.

## Click handling

Java interfaces cannot be implemented from native code, so clicks ride an
adapter class. Two paths:

### Self-contained (recommended, API 26+)

The adapter ships *inside this package* as a pre-compiled DEX
(`adapter/adapter.dex`, embedded via `#include_bytes`); on first use
`android_view/listener` loads it with `dalvik.system.InMemoryDexClassLoader`
and binds its native method with `RegisterNatives`. The host app ships no
Java beyond MainActivity. Importing the module obligates the app to export
the hook (bionic binds eagerly at `System.loadLibrary`, so a missing hook
fails at library load):

```cplus
import "android_view/listener" as listener;

// wire any control:
listener::set_on_click(env, button.as_view_obj(), 1 as i64);

// every adapter click lands here; `token` routes controls:
pub extern fn cplus_on_click(envp: *jni::JNIEnv, token: i64, view: jni::jobject) { ... }
```

Validated on the emulator: taps reach the hook and a `setText` from C+
updates the screen, with only MainActivity in the APK's dex.
`adapter/build.sh` regenerates the dex (committed; consumers never run it).

### Host-shipped adapter (works on any API level)

Alternatively the host app ships the tiny class next to MainActivity:

```java
package com.example.app;

public final class NativeClickListener implements android.view.View.OnClickListener {
    private final long token;
    public NativeClickListener(long token) { this.token = token; }
    private static native void nativeOnClick(long token, android.view.View v);
    @Override public void onClick(android.view.View v) { nativeOnClick(token, v); }
}
```

The C+ side wires it with `button.set_on_click(#str_ptr("com/example/app/NativeClickListener\0"), token)`
and exports the matching `Java_..._nativeOnClick` handler:

```cplus
pub extern fn Java_com_example_app_NativeClickListener_nativeOnClick(
    envp: *jni::JNIEnv,
    cls: jni::jobject,
    token: i64,
    view: jni::jobject,
) { ... }
```

`token` routes multiple controls through one adapter class. Validated on the
emulator: taps on a Button reach the C+ handler and a `setText` from C+
updates the screen.

## Ownership

Wrappers own a JNI global reference and delete it in `drop`. Methods that pass a
child to a parent use raw `jobject` handles, matching `appkit`'s `addSubview:`
style. For a root object returned to Java, call `into_raw()` to transfer the
global reference out of the wrapper.

## Gaps

This is a first slice, not a complete Android toolkit:

- `JValue` currently supports object/int/boolean/long slots only
  (floats need real bit casts).
- Layout params, colors, density conversion, resources, and UI-thread dispatch
  are still missing.

Two former gaps are fixed: C+ string literals accept a bare `$` (v0.0.22
lexer), so nested-class descriptors like `android/view/View$OnClickListener`
work directly; and `vendor/jni` models `JNIEnv *` as the double pointer JNI
requires — `Env` stores the handle a native method receives and passes it to
every table call (ART aborts if handed the bare table pointer).
