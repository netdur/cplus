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
# ListView and its chain are here for the RECYCLER, and they are the exception
# that proves the rule about containers: facet owns the layout of facet's tree,
# and a recycled row is a subtree the PLATFORM owns — created, pooled and
# destroyed on its schedule, not ours. `mount::realise` / `unrealise` /
# `sync_from` exist precisely because a row is not in the window walk.
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
    android.widget.AdapterView \
    android.widget.AbsListView \
    android.widget.ListView \
    android.graphics.Bitmap \
    android.graphics.BitmapFactory \
    android.graphics.drawable.Drawable \
    android.graphics.drawable.GradientDrawable \
    android.graphics.Canvas \
    android.graphics.Paint \
    'android.graphics.Paint$Style' \
    'android.graphics.Paint$Cap' \
    'android.graphics.Paint$Join' \
    'android.graphics.Paint$Align' \
    android.graphics.Path \
    'android.graphics.Path$Direction' \
    'android.graphics.Path$FillType' \
    android.graphics.RectF \
    android.graphics.Matrix \
    android.graphics.Typeface \
    android.graphics.PathEffect \
    android.graphics.DashPathEffect \
    android.graphics.Shader \
    'android.graphics.Shader$TileMode' \
    android.graphics.LinearGradient \
    android.graphics.PorterDuff \
    'android.graphics.PorterDuff$Mode' \
    android.graphics.PorterDuffXfermode \
    android.graphics.Xfermode \
    > src/widgets.cplus

wc -l src/widgets.cplus
tail -1 src/widgets.cplus
