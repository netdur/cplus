#!/bin/sh
# Regenerate adapter.dex from NativeClickListener.java. Needs a JDK and the
# Android SDK (android.jar + build-tools d8). The dex is committed so package
# consumers never run this; rerun only when the adapter source changes, and
# update the #include_bytes length in ../src/listener.cplus to the new size.
set -e
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
AJ="$SDK/platforms/android-36/android.jar"
BT="$SDK/build-tools/36.0.0"
javac -classpath "$AJ" -d classes NativeClickListener.java
"$BT/d8" --release --lib "$AJ" --output . classes/cplus/androidview/*.class
mv classes.dex adapter.dex
rm -rf classes
ls -la adapter.dex
