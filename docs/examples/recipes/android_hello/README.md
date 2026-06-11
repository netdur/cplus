# android_hello — Android app via vendor/android_view

A LinearLayout with two TextViews and a Button, built entirely in C+ and
handed to Android through the `nativeCreateView` host contract. Button taps
land in a C+ handler (the click adapter ships inside `vendor/android_view`
as a pre-compiled DEX — no host-side Java beyond `MainActivity`), increment
a counter, and write the new text back into the status TextView.

Validated on a Pixel emulator (API 36): the screen renders and programmatic
taps drive the counter.

## How it fits together

1. `cpc build --target android-arm64` produces a static library; cpc stops
   there (the external-builder handoff).
2. The NDK's clang links `libapp.so` from the archive. `--whole-archive`
   matters: the JNI entry points are referenced by the JVM at runtime, not
   by any object in the link, so without it the linker would drop them.
3. `host/MainActivity.java` is the entire Java surface: it loads the
   library and sets the C+-built View tree as its content view.
4. Clicks: `listener::set_on_click(env, view, token)` instantiates the
   package's DEX-shipped adapter (loaded via `InMemoryDexClassLoader`,
   bound with `RegisterNatives`); every tap reaches the app's exported
   `cplus_on_click(envp, token, view)` hook, with `token` routing controls.

## Build + run (script path)

```bash
./build_apk.sh                       # SDK paths overridable via env; see header
adb install -r apk/app.apk
adb shell am start -n com.cplus.hello/.MainActivity
```

The recipe relies on `vendor/android_view` and `vendor/jni` being symlinked
into the project's `vendor/` directory — the same model as every other
recipe.

## Gradle integration

In an Android Studio project, steps 3-4 of the script are Gradle's job;
only the native library needs producing. Add a task (or CI step) that runs:

```bash
cpc build --target android-arm64
$NDK/toolchains/llvm/prebuilt/<host>/bin/clang -target aarch64-linux-android24 \
    -shared -Wl,--whole-archive target/android-arm64/debug/libandroid_hello.a \
    -Wl,--no-whole-archive -o app/src/main/jniLibs/arm64-v8a/libapp.so
```

and copy `host/MainActivity.java` into your application package (adjusting
the package name in both the Java file and the C+ export
`Java_<package>_MainActivity_nativeCreateView`). Gradle picks up
`jniLibs/` automatically; `minSdk 26` covers the DEX click path
(`minSdk 24` if you only render).

## File map

```
android_hello/
├── Cplus.toml              package metadata ([lib] staticlib)
├── src/lib.cplus           View tree + click hook, all C+
├── host/MainActivity.java  the entire Java surface (2 methods)
├── host/AndroidManifest.xml
└── build_apk.sh            staticlib -> libapp.so -> signed APK
```
