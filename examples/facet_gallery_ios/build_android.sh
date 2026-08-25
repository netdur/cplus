#!/bin/sh
# cpc -> NDK link -> dex -> apk -> device, by hand. No Gradle.
#
# THE LINK CLOSURE IS THE APP'S JOB on Android. cpc emits one archive per
# PREBUILT package and compiles the source-mode ones (facet, facet_android) into
# the app's own archive. Miss one and it fails at dlopen, not at link, with a
# mangled C+ symbol name — see plans/plan.android.md finding 3.
set -e
cd "$(dirname "$0")"

SDK="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
BT="$SDK/build-tools/36.0.0"
AJ="$SDK/platforms/android-36/android.jar"
NDK="$SDK/ndk/29.0.13846066"
CC="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang"
CPC="../../target/release/cpc"
V="../../vendor"
T="aarch64-linux-android"

rm -rf out && mkdir -p out/lib/arm64-v8a

"$CPC" build --target android-arm64

# facet and facet_android are source-mode, so they are already inside the app's
# archive; the prebuilt ones are named here.
DEPS="$V/facet_runtime/lib/$T/libfacet_runtime.a \
      $V/facet_agent/lib/$T/libfacet_agent.a \
      $V/agent_android/lib/$T/libagent_android.a \
      $V/agent_core/lib/$T/libagent_core.a \
      $V/agent_mcp/lib/$T/libagent_mcp.a \
      $V/agent_inapp/lib/$T/libagent_inapp.a \
      $V/json/lib/$T/libjson.a \
      $V/android_view/lib/$T/libandroid_view.a \
      $V/jni/lib/$T/libjni.a \
      $V/flex_layout/lib/$T/libflex_layout.a \
      $V/events/lib/$T/libevents.a \
      $V/stdlib/lib/$T/libstdlib.a"

# -llog: facet_android reports unbuilt kinds through liblog, because an app's
# stderr goes to /dev/null on Android.
#
# --no-undefined turns the DEFAULT failure mode inside out, and that is the
# point. A shared library may legally leave symbols undefined, so a missing one
# surfaces at `System.loadLibrary` as a dlopen error naming ONE symbol — you fix
# it, rebuild, install, launch, and get the next one. With this flag the linker
# names them all at once, at build time. Two rounds of that (`_NSGetExecutablePath`,
# then `CC_SHA256`) is what prompted it.
"$CC" -target aarch64-linux-android24 -shared \
    -Wl,--whole-archive target/android-arm64/debug/libfacet_gallery_ios.a \
    -Wl,--no-whole-archive $DEPS -llog \
    -Wl,--no-undefined \
    -o out/lib/arm64-v8a/libiosgallery.so

# NO JAVA IN THIS APP. facet_android's own dex carries the Activity the manifest
# names, and `d8` takes a .dex as an input — so the merge IS the build step that
# would otherwise compile the app's MainActivity.
"$BT/d8" --release --lib "$AJ" --output out $V/facet_android/facet_android.dex

"$BT/aapt2" link -o out/base.apk --manifest AndroidManifest.xml -I "$AJ" \
    --min-sdk-version 26 --target-sdk-version 34
(cd out && zip -q base.apk classes.dex lib/arm64-v8a/libiosgallery.so)
"$BT/zipalign" -f -p 4 out/base.apk out/aligned.apk
"$BT/apksigner" sign --ks ~/.android/debug.keystore \
    --ks-pass pass:android --ks-key-alias androiddebugkey --key-pass pass:android \
    --out out/app.apk out/aligned.apk

"$SDK/platform-tools/adb" install -r -t out/app.apk
"$SDK/platform-tools/adb" shell am start -n cplus.iosgallery/cplus.facet.FacetActivity
