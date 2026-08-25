#!/bin/sh
# Regenerate src/widgets.cplus from the Android SDK.
#
# Hand additions do NOT go in that file — they go in src/android_view_ext.cplus,
# which survives this script. Same split as vendor/appkit (appkit.cplus is
# generated, appkit_ext.cplus is hand-written); see the repo CLAUDE.md.
#
# The class list is CURATED, not exhaustive. facet owns geometry on Android
# (plans/plan.android.md), so the layout containers a backend would use to place
# FACET's tree — RelativeLayout, ConstraintLayout, GridLayout — are deliberately
# absent: a backend that positions children itself never calls them, and binding
# them would multiply this file for nothing.
#
# FrameLayout and LinearLayout ARE here, and the distinction is worth stating:
# facet owns the layout of facet's TREE, not of a control's INTERNALS. ScrollView
# extends FrameLayout and cannot be reached without it, and a stepper is three
# Android views inside one facet node — nothing in facet's tree, so nothing for
# flex to place. A layout container reaching a facet child would still be wrong.
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
    android.widget.FrameLayout \
    android.widget.LinearLayout \
    android.widget.ScrollView \
    android.widget.HorizontalScrollView \
    android.widget.ImageButton \
    android.graphics.Bitmap \
    android.graphics.BitmapFactory \
    android.graphics.drawable.Drawable \
    android.graphics.drawable.GradientDrawable \
    > src/widgets.cplus

wc -l src/widgets.cplus
tail -1 src/widgets.cplus
