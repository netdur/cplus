#!/bin/sh
# android_hello: C+ staticlib -> NDK-linked libapp.so -> installable APK.
# The validated no-Gradle pipeline; see README.md for Gradle integration.
#
# Needs: the Android SDK (build-tools + a platforms/android-NN jar + an NDK
# r28.2+) and a JDK. Override defaults via environment:
#   ANDROID_SDK_ROOT (default ~/Library/Android/sdk)
#   ANDROID_API      (platform jar version, default 36)
#   BUILD_TOOLS      (default 36.0.0)
set -e
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
API="${ANDROID_API:-36}"
BT="$SDK/build-tools/${BUILD_TOOLS:-36.0.0}"
AJ="$SDK/platforms/android-$API/android.jar"
NDK_CLANG=$(ls -d "$SDK"/ndk/*/toolchains/llvm/prebuilt/*/bin/clang | sort | tail -1)

# 1. C+ -> staticlib (cpc stops here; the NDK owns the link).
cpc build --target android-arm64

# 2. NDK clang links the shared library the JVM loads. --whole-archive
#    keeps the JNI exports no Java code references at link time.
mkdir -p apk/lib/arm64-v8a
"$NDK_CLANG" -target aarch64-linux-android24 -shared \
    -Wl,--whole-archive target/android-arm64/debug/libandroid_hello.a \
    -Wl,--no-whole-archive -o apk/lib/arm64-v8a/libapp.so

# 3. Host Java (MainActivity only — the click adapter ships inside
#    vendor/android_view as a dex) -> classes.dex.
mkdir -p apk/java/com/cplus/hello
cp host/MainActivity.java apk/java/com/cplus/hello/
cd apk
javac -classpath "$AJ" -d classes java/com/cplus/hello/MainActivity.java
"$BT/d8" --release --lib "$AJ" --output . classes/com/cplus/hello/*.class

# 4. Assemble + align + debug-sign.
"$BT/aapt2" link --manifest ../host/AndroidManifest.xml -I "$AJ" -o base.apk
zip -q base.apk classes.dex lib/arm64-v8a/libapp.so
"$BT/zipalign" -f 4 base.apk app.apk
"$BT/apksigner" sign --ks ~/.android/debug.keystore --ks-pass pass:android app.apk
echo "built: apk/app.apk  (adb install -r apk/app.apk)"
