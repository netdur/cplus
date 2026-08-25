#!/bin/sh
# Regenerate src/widgets.cplus from the Android SDK.
#
# Hand additions do NOT go in that file — they go in src/android_view_ext.cplus,
# which survives this script. Same split as vendor/appkit (appkit.cplus is
# generated, appkit_ext.cplus is hand-written); see the repo CLAUDE.md.
#
# The class list is CURATED, not exhaustive. facet owns geometry on Android
# (plans/plan.android.md), so the layout containers — LinearLayout,
# RelativeLayout, ConstraintLayout, GridLayout and their LayoutParams — are
# deliberately absent: a backend that positions children itself never calls
# them, and binding them would multiply this file for nothing.
set -e
cd "$(dirname "$0")/.."

SDK="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
AJ="$SDK/platforms/android-36/android.jar"
BINDGEN="${BINDGEN:-../../target/release/cpc-bindgen}"

"$BINDGEN" --java --java-classpath "$AJ" --java-runtime "./runtime" \
    android.view.View \
    android.view.ViewGroup \
    android.widget.TextView \
    android.widget.Button \
    android.widget.EditText \
    android.widget.ImageView \
    android.widget.ProgressBar \
    android.widget.CompoundButton \
    android.widget.CheckBox \
    android.widget.Switch \
    android.widget.RadioButton \
    android.widget.AbsSeekBar \
    android.widget.SeekBar \
    > src/widgets.cplus

wc -l src/widgets.cplus
tail -1 src/widgets.cplus
