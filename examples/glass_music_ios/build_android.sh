#!/bin/zsh
# cpc -> NDK link -> dex -> apk -> the running emulator or device. No Gradle.
set -e
cd "$(dirname "$0")"
[ -e vendor ] || ln -s ../../vendor vendor

SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
BT="$SDK/build-tools/36.0.0"
AJ="$SDK/platforms/android-36/android.jar"
NDK="$SDK/ndk/29.0.13846066"
CC="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang"
CPC=../../target/release/cpc
V=../../vendor

rm -rf out && mkdir -p out/lib/arm64-v8a out/assets
$CPC build --target android-arm64
$CC -target aarch64-linux-android24 -shared \
    -Wl,--whole-archive target/android-arm64/debug/libglass_music_ios.a -Wl,--no-whole-archive \
    $($CPC build --target android-arm64 --print-link-args) -llog -lm -Wl,--no-undefined \
    -o out/lib/arm64-v8a/libglass_music_ios.so
$BT/d8 --release --lib $AJ --output out $V/facet_android/facet_android.dex
cp $V/facet/assets/MaterialSymbolsOutlined.ttf out/assets/
$BT/aapt2 link -o out/base.apk --manifest AndroidManifest.xml -I $AJ -A out/assets \
    --min-sdk-version 26 --target-sdk-version 34
(cd out && zip -q base.apk classes.dex lib/arm64-v8a/libglass_music_ios.so)
$BT/zipalign -f -p 4 out/base.apk out/aligned.apk
$BT/apksigner sign --ks ~/.android/debug.keystore --ks-pass pass:android \
    --ks-key-alias androiddebugkey --key-pass pass:android --out out/app.apk out/aligned.apk
$SDK/platform-tools/adb install -r -t out/app.apk
$SDK/platform-tools/adb shell am start -n dev.cplus.glassmusic/cplus.facet.FacetActivity
