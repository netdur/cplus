#!/bin/zsh
set -e
cd "$(dirname "$0")"
DEV=${1:?usage: ./run_sim.sh <booted simulator udid>}
CPC=../../target/release/cpc
# Build against the repo's packages, not the released store.
[ -e vendor ] || ln -s ../../vendor vendor
$CPC build --target ios-arm64-simulator
A=$PWD/out/GlassMusic.app; mkdir -p $A; cp ios/Info.plist $A/
xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 -I target/ios-arm64-simulator/debug ios/main.m target/ios-arm64-simulator/debug/libglass_music_ios.a $($CPC build --target ios-arm64-simulator --print-link-args) -framework UIKit -framework QuartzCore -framework Foundation -framework CoreGraphics -framework WebKit -lobjc -o $A/Glass_music_ios 2>&1 | grep -v duplicate || true
xcrun simctl terminate $DEV dev.cplus.glassmusic 2>/dev/null || true
xcrun simctl install $DEV $A
xcrun simctl launch $DEV dev.cplus.glassmusic
