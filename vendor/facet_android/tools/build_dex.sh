#!/bin/sh
# Regenerate facet_android.dex from java/. Needs a JDK and the Android SDK.
#
# The dex is COMMITTED and consumers never run this. It is embedded in the
# binary with #include_bytes and loaded at runtime with InMemoryDexClassLoader
# (API 26+), its natives bound with RegisterNatives — so an app that uses this
# backend ships no Java beyond its own Activity.
set -e
cd "$(dirname "$0")/.."
SDK="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
AJ="$SDK/platforms/android-36/android.jar"
BT="$SDK/build-tools/36.0.0"

rm -rf classes && mkdir -p classes
# NOT `2>/dev/null`. It was, and a compile error then looked like nothing at all:
# `set -e` stopped the script with no output, the COMMITTED dex stayed in place,
# and the backend went on calling a method the running dex did not have — which
# surfaces as a NoSuchMethodError abort at runtime, one build later, in whatever
# code happened to call it next. Warnings are the only thing worth hiding here.
javac -source 8 -target 8 -nowarn -classpath "$AJ" -d classes java/cplus/facet/*.java
"$BT/d8" --release --lib "$AJ" --output . classes/cplus/facet/*.class
mv classes.dex facet_android.dex
rm -rf classes

SIZE=$(stat -f%z facet_android.dex 2>/dev/null || stat -c%s facet_android.dex)
echo "facet_android.dex: $SIZE bytes"
# The #include_bytes type carries the length, so keep the source in step.
sed -i '' "s/const DEX_LEN: usize = [0-9]*/const DEX_LEN: usize = $SIZE/" src/dex.cplus
sed -i '' "s/\*\[u8; [0-9]*\] = #include_bytes/*[u8; $SIZE] = #include_bytes/" src/dex.cplus
grep -n "DEX_LEN\|include_bytes" src/dex.cplus
