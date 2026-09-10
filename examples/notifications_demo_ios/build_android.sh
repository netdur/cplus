#!/bin/sh
# cpc -> NDK link -> dex -> apk -> device, by hand. No Gradle.
#
# Same shape as examples/facet_gallery_android/build.sh; see that file for why
# the link closure is named here rather than discovered.
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

# facet and facet_android are source-mode, so they are already inside this app's
# archive; the prebuilt ones are named here. `permissions` is PREBUILT — the
# default since 2026-08-16 — so its object code lives in its own slice and
# leaving it out fails at link naming `permissions.src.permissions.state`.
DEPS="$V/applinks/lib/$T/libapplinks.a \
      $V/permissions/lib/$T/libpermissions.a \
      $V/notifications/lib/$T/libnotifications.a \
      $V/facet_runtime/lib/$T/libfacet_runtime.a \
      $V/android_view/lib/$T/libandroid_view.a \
      $V/jni/lib/$T/libjni.a \
      $V/flex_layout/lib/$T/libflex_layout.a \
      $V/events/lib/$T/libevents.a \
      $V/stdlib/lib/$T/libstdlib.a"

"$CC" -target aarch64-linux-android24 -shared \
    -Wl,--whole-archive target/android-arm64/debug/libnotifications_demo_ios.a \
    -Wl,--no-whole-archive $DEPS -llog -lm \
    -Wl,--no-undefined \
    -o out/lib/arm64-v8a/libnotificationsdemo.so

# NO JAVA IN THIS APP. facet_android's dex carries FacetActivity — including the
# `onRequestPermissionsResult` override this probe exists to exercise — and `d8`
# takes a .dex as an input, so the merge IS the build step.
"$BT/d8" --release --lib "$AJ" --output out $V/facet_android/facet_android.dex

# POST_NOTIFICATIONS is an API 33 permission, so the target level decides
# whether the app is asked at all.
"$BT/aapt2" link -o out/base.apk --manifest AndroidManifest.xml -I "$AJ" \
    --min-sdk-version 26 --target-sdk-version 34
(cd out && zip -q base.apk classes.dex lib/arm64-v8a/libnotificationsdemo.so)
"$BT/zipalign" -f -p 4 out/base.apk out/aligned.apk
"$BT/apksigner" sign --ks ~/.android/debug.keystore \
    --ks-pass pass:android --ks-key-alias androiddebugkey --key-pass pass:android \
    --out out/app.apk out/aligned.apk

"$SDK/platform-tools/adb" install -r -t out/app.apk
"$SDK/platform-tools/adb" shell am start -n cplus.notificationsdemo/cplus.facet.FacetActivity
