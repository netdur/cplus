#!/bin/sh
# Regenerate location.dex from java/. Needs a JDK and the Android SDK.
#
# THE DEX IS A COMMITTED BUILD ARTIFACT. Editing java/cplus/location/*.java
# changes NOTHING until this script runs, because the dex is checked in and
# `#include_bytes`'d with a hard-coded length. The symptom is a Java method that
# is plainly there and never called — the same trap vendor/facet_android
# documents, and it has already cost this project real time once.
#
# Consumers never run this.
set -e
cd "$(dirname "$0")/.."
SDK="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
AJ="$SDK/platforms/android-36/android.jar"
BT="$SDK/build-tools/36.0.0"

rm -rf classes && mkdir -p classes
# NOT `2>/dev/null`. A compile error would otherwise look like nothing at all:
# `set -e` stops the script with no output, the COMMITTED dex stays in place,
# and the backend goes on calling a method the running dex does not have.
javac -source 8 -target 8 -nowarn -classpath "$AJ" -d classes java/cplus/location/*.java
"$BT/d8" --release --lib "$AJ" --min-api 26 --output . classes/cplus/location/*.class
mv classes.dex location.dex
rm -rf classes

# The length lives in the backend, not in a dex.cplus of its own — this
# package has one class where facet_android has twenty-seven.
SRC=src/location_backend_android.cplus
SIZE=$(stat -f%z location.dex 2>/dev/null || stat -c%s location.dex)
echo "location.dex: $SIZE bytes"
# The #include_bytes type carries the length, so keep the source in step.
sed -i '' "s/const _DEX_LEN: usize = [0-9]*/const _DEX_LEN: usize = $SIZE/" $SRC
sed -i '' "s/\*\[u8; [0-9]*\] = #include_bytes/*[u8; $SIZE] = #include_bytes/" $SRC
grep -n "_DEX_LEN\|include_bytes" $SRC
