package cplus.facet;

// The three drawing calls that cannot be made from C+, and nothing else.
//
// `vendor/jni` types no array slots — the same "define only what we call" gap
// the object-field rung closed for fields — so a call whose argument is a
// float[] has no door. Two of these are that: `Matrix.setValues` and
// `DashPathEffect`. The third is text LAYOUT, which is not a gap but a
// judgement: StaticLayout is Android's own line breaker, and hand-rolling wrap,
// alignment and ellipsis in C+ would be a second, worse answer to a question
// the platform already answers.
//
// Everything else about a facet `canvas` — every shape, every state change,
// every transform — is replayed from C+ in drawing.cplus.
public final class FacetDraw {
    private FacetDraw() {}

    // An affine transform, in the order facet records it: a b c d tx ty, which
    // is [a c tx / b d ty] as a Matrix's nine.
    public static void concat(android.graphics.Canvas canvas,
                              float a, float b, float c, float d, float tx, float ty) {
        android.graphics.Matrix m = new android.graphics.Matrix();
        m.setValues(new float[] { a, c, tx, b, d, ty, 0f, 0f, 1f });
        canvas.concat(m);
    }

    // A dash pattern of up to eight runs. `count` of zero is SOLID, which is a
    // null PathEffect and not a zero-length array — DashPathEffect refuses
    // fewer than two entries and refuses an odd count.
    public static void dash(android.graphics.Paint p, int count, float phase,
                            float r0, float r1, float r2, float r3,
                            float r4, float r5, float r6, float r7) {
        if (count <= 0) { p.setPathEffect(null); return; }
        if (count > 8) count = 8;
        if ((count & 1) != 0) count -= 1;
        if (count < 2) { p.setPathEffect(null); return; }
        float[] runs = new float[] { r0, r1, r2, r3, r4, r5, r6, r7 };
        float[] used = new float[count];
        System.arraycopy(runs, 0, used, 0, count);
        p.setPathEffect(new android.graphics.DashPathEffect(used, phase));
    }

    // A two-stop gradient BACKGROUND. `GradientDrawable` takes its colours as
    // an int[] and its direction as an eight-value enum, and neither has a door
    // from C+ — the array for the reason above, the enum because C+ can reach
    // the constants but not name the ctor's parameter type without one.
    //
    // facet's angle is degrees CLOCKWISE FROM UP, so 0 points up and 90 points
    // right. Android has eight directions and nothing between them, so the
    // angle SNAPS to the nearest 45 — an approximation, and named as one in
    // MANIFEST section 2.
    public static void gradientBackground(android.view.View v, int start, int end,
                                          float angleDeg, float radius) {
        android.graphics.drawable.GradientDrawable d =
            new android.graphics.drawable.GradientDrawable(orientationOf(angleDeg),
                                                           new int[] { start, end });
        if (radius > 0f) d.setCornerRadius(radius);
        v.setBackground(d);
    }

    private static android.graphics.drawable.GradientDrawable.Orientation
    orientationOf(float angleDeg) {
        float a = angleDeg % 360f;
        if (a < 0f) a += 360f;
        int step = Math.round(a / 45f) % 8;
        switch (step) {
            case 1: return android.graphics.drawable.GradientDrawable.Orientation.BL_TR;
            case 2: return android.graphics.drawable.GradientDrawable.Orientation.LEFT_RIGHT;
            case 3: return android.graphics.drawable.GradientDrawable.Orientation.TL_BR;
            case 4: return android.graphics.drawable.GradientDrawable.Orientation.TOP_BOTTOM;
            case 5: return android.graphics.drawable.GradientDrawable.Orientation.TR_BL;
            case 6: return android.graphics.drawable.GradientDrawable.Orientation.RIGHT_LEFT;
            case 7: return android.graphics.drawable.GradientDrawable.Orientation.BR_TL;
            default: return android.graphics.drawable.GradientDrawable.Orientation.BOTTOM_TOP;
        }
    }

    // A CONTROL'S BACKGROUND, which is a background AND a touch.
    //
    // The shape is C+'s — a fill, a corner radius and a stroke, the same three
    // the band builds — but a control that replaces its platform background
    // loses the platform's PRESSED STATE with it, and a button that does not
    // answer a finger reads as broken however right it looks. So the shape goes
    // inside a RippleDrawable, and the ripple colour is the theme's own
    // `colorControlHighlight` rather than a number invented here.
    //
    // The mask is a second shape with the same radius: without one the ripple
    // is unbounded and paints square corners over a rounded button.
    public static void controlBackground(android.view.View v, int fill, boolean hasFill,
                                         int stroke, int strokeWidth, float radius) {
        android.graphics.drawable.GradientDrawable content =
            new android.graphics.drawable.GradientDrawable();
        content.setColor(hasFill ? fill : 0);
        if (radius > 0f) content.setCornerRadius(radius);
        if (strokeWidth > 0) content.setStroke(strokeWidth, stroke);

        android.graphics.drawable.GradientDrawable mask =
            new android.graphics.drawable.GradientDrawable();
        mask.setColor(0xFFFFFFFF);
        if (radius > 0f) mask.setCornerRadius(radius);

        v.setBackground(new android.graphics.drawable.RippleDrawable(
            android.content.res.ColorStateList.valueOf(highlight(v)), content, mask));
        // A view only shows a pressed state if it can be pressed.
        v.setClickable(true);
    }

    // A TAP HAS TO SHOW. This is the FOREGROUND — a ripple with no content of
    // its own — so it composes with whatever the background already is: the
    // platform's own drawable, one facet built, or nothing at all.
    //
    // It is not optional decoration. A plain `Button` under this theme has a
    // background with no pressed layer, so a finger on it changed not one
    // pixel: the control worked and looked broken, which is the worst pair.
    public static void tapFeedback(android.view.View v, float radius) {
        android.graphics.drawable.GradientDrawable mask =
            new android.graphics.drawable.GradientDrawable();
        mask.setColor(0xFFFFFFFF);
        if (radius > 0f) mask.setCornerRadius(radius);
        v.setForeground(new android.graphics.drawable.RippleDrawable(
            android.content.res.ColorStateList.valueOf(highlight(v)), null, mask));
    }

    // A GLYPH BUTTON'S PRESS IS A CIRCLE, and Android already has one:
    // `selectableItemBackgroundBorderless` is the borderless ripple every icon
    // button on the platform wears — unbounded, centred on the finger, round.
    //
    // Taken from the THEME rather than built here, because a circle built from
    // a mask is an ELLIPSE the moment the view is wider than it is tall, which
    // a glyph button stretched by a row always is.
    public static void tapFeedbackBorderless(android.view.View v) {
        android.content.res.TypedArray a = v.getContext().obtainStyledAttributes(
            new int[] { android.R.attr.selectableItemBackgroundBorderless });
        android.graphics.drawable.Drawable d = a.getDrawable(0);
        a.recycle();
        if (d == null) { tapFeedback(v, 0f); return; }
        v.setForeground(d);
        if (!(d instanceof android.graphics.drawable.RippleDrawable)) return;
        final android.graphics.drawable.RippleDrawable r =
            (android.graphics.drawable.RippleDrawable) d;
        // AN UNBOUNDED RIPPLE TAKES ITS RADIUS FROM ITS HOTSPOT BOUNDS, and
        // those default to the whole view — so a glyph button stretched across a
        // row rippled in a circle the width of the row, spilling over the card
        // it sat in. A centred SQUARE is what a glyph button's box actually is;
        // the circle then fits the control instead of the layout.
        //
        // Re-set on every layout, because facet lays this view out itself and
        // the size it gives can change between passes.
        v.addOnLayoutChangeListener(new android.view.View.OnLayoutChangeListener() {
            @Override public void onLayoutChange(android.view.View view, int l, int t,
                                                 int rr, int b, int ol, int ot,
                                                 int orr, int ob) {
                int w = rr - l, h = b - t;
                int side = Math.min(w, h);
                if (side <= 0) return;
                int cx = w / 2, cy = h / 2, half = side / 2;
                r.setHotspotBounds(cx - half, cy - half, cx + half, cy + half);
            }
        });
    }

    // THE SOFT KEYBOARD, which is what `focus` means to a text field. A view
    // that has focus and no keyboard is focused in name only on a phone.
    // WHETHER A VIEW TAKES A TOUCH, and why remembering is the whole job.
    //
    // facet's `input_transparent` is a two-state prop and Android has no single
    // bit behind it: a view is passed over when it is neither clickable nor
    // focusable, and CLEARING those is easy. Setting them back is the hard half
    // — the answer is not "clickable and focusable", it is whatever THIS widget
    // was born with. A Button is clickable; a TextView is not; a ListView is
    // both. Writing the same three trues onto all of them made every label in
    // the tree focusable, and a row with a focusable child is a row an
    // AdapterView refuses to click: `hasFocusable()` is checked before the item
    // click is delivered, so every list and every grid stopped reporting taps.
    //
    // Same shape as the remembered background one file over, and for the same
    // reason: "facet declares nothing" has to restore the PLATFORM'S value.
    // 0x7F0F000D. THE TAG IDS ARE ONE NUMBER SPACE across this backend and they
    // live in whichever file needs them — controls.cplus holds most, and this
    // was written as ...000B, which is the refreshable's spinner. A collision
    // there is a ProgressBar read as an Integer, or worse the other way round.
    // Taken, at the time of writing: 0001-0007 and 000A-000C in controls.cplus,
    // paint.cplus and swipe.cplus. Grep 0x7F0F before taking one.
    private static final int INPUT_TAG = 0x7F0F000D;

    public static void rememberInput(android.view.View v) {
        if (v.getTag(INPUT_TAG) != null) return;
        int bits = (v.isClickable() ? 1 : 0)
                 | (v.isFocusable() ? 2 : 0)
                 | (v.isLongClickable() ? 4 : 0);
        v.setTag(INPUT_TAG, Integer.valueOf(bits));
    }

    public static void inputTransparent(android.view.View v, boolean through) {
        if (through) {
            rememberInput(v);
            v.setClickable(false);
            v.setFocusable(false);
            v.setLongClickable(false);
            return;
        }
        Object t = v.getTag(INPUT_TAG);
        if (!(t instanceof Integer)) return;      // never made transparent; leave it alone
        int bits = (Integer) t;
        v.setClickable((bits & 1) != 0);
        v.setFocusable((bits & 2) != 0);
        v.setLongClickable((bits & 4) != 0);
    }

    // RICH TEXT, BUILT ONE RUN AT A TIME.
    //
    // Two facet verbs land here and they differ in ONE thing: a label's
    // `formatted_text` carries the text INSIDE each run, so writing it replaces
    // the content; a text area's `style_runs` styles text that is already there
    // and must not touch it — re-setting an editor's text moves the caret and
    // breaks the undo stack. `spansBegin` takes the whole string for the first
    // and the view's current text for the second, and everything after is the
    // same.
    //
    // The builder is parked on the view rather than passed back across, because
    // a SpannableStringBuilder is a Java object with no C+ shape: the calls
    // between begin and commit name the view and nothing else.
    private static final int SPAN_TAG = 0x7F0F000E;

    public static void spansBegin(android.widget.TextView v, String text) {
        v.setTag(SPAN_TAG, new android.text.SpannableStringBuilder(text));
    }

    public static void spansBeginFromView(android.widget.TextView v) {
        spansBegin(v, v.getText().toString());
    }

    // One run. `start` and `length` are in UTF-16 code units, which is what a
    // Java string is indexed in — the C+ side converts from bytes.
    //
    // Every attribute is optional and says so with its own flag rather than a
    // sentinel: a colour of zero is transparent black, which is a colour, and a
    // weight of zero is not "unset".
    public static void spansAdd(android.widget.TextView v, int start, int length,
                                int color, boolean hasColor,
                                int background, boolean hasBackground,
                                boolean bold, boolean italic,
                                int decoration, int sizePx, String link) {
        Object t = v.getTag(SPAN_TAG);
        if (!(t instanceof android.text.SpannableStringBuilder)) return;
        android.text.SpannableStringBuilder b = (android.text.SpannableStringBuilder) t;
        int end = start + length;
        if (start < 0 || end > b.length() || length <= 0) return;
        int flag = android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE;
        if (hasColor) b.setSpan(new android.text.style.ForegroundColorSpan(color), start, end, flag);
        if (hasBackground) b.setSpan(new android.text.style.BackgroundColorSpan(background), start, end, flag);
        if (bold && italic) b.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD_ITALIC), start, end, flag);
        else if (bold) b.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), start, end, flag);
        else if (italic) b.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.ITALIC), start, end, flag);
        if (decoration == 1) b.setSpan(new android.text.style.StrikethroughSpan(), start, end, flag);
        else if (decoration == 2) b.setSpan(new android.text.style.UnderlineSpan(), start, end, flag);
        if (sizePx > 0) b.setSpan(new android.text.style.AbsoluteSizeSpan(sizePx), start, end, flag);
        if (link != null && link.length() > 0) {
            b.setSpan(new android.text.style.URLSpan(link), start, end, flag);
        }
    }

    // A LINK NEEDS A MOVEMENT METHOD, or a URLSpan draws as a link and does
    // nothing when tapped. Set only when there is one: it makes the view
    // clickable, and a label that quietly took touches would be the input
    // trap this backend already learned once.
    public static void spansCommit(android.widget.TextView v, boolean hasLinks) {
        Object t = v.getTag(SPAN_TAG);
        if (!(t instanceof android.text.SpannableStringBuilder)) return;
        v.setText((android.text.SpannableStringBuilder) t);
        if (hasLinks) {
            v.setMovementMethod(android.text.method.LinkMovementMethod.getInstance());
        }
        v.setTag(SPAN_TAG, null);
    }

    public static void showKeyboard(android.view.View v) {
        v.requestFocus();
        android.view.inputmethod.InputMethodManager m =
            (android.view.inputmethod.InputMethodManager)
                v.getContext().getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
        if (m != null) m.showSoftInput(v, 0);
    }

    public static void hideKeyboard(android.view.View v) {
        android.view.inputmethod.InputMethodManager m =
            (android.view.inputmethod.InputMethodManager)
                v.getContext().getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
        if (m != null) m.hideSoftInputFromWindow(v.getWindowToken(), 0);
        v.clearFocus();
    }

    // A HEADING, which is an accessibility ROLE and not a description —
    // `setAccessibilityHeading` is API 28 and this backend's floor is 26, so
    // the version is asked rather than assumed. A reader on 26 hears the label
    // and not the level, which is the honest degradation.
    public static void setHeading(android.view.View v, boolean heading) {
        if (android.os.Build.VERSION.SDK_INT >= 28) v.setAccessibilityHeading(heading);
    }

    // A TWO-STATE COLOUR, which is what a switch is: one colour when it is on
    // and another when it is off. `ColorStateList.valueOf` gives one colour for
    // every state, and the constructor that takes the pair takes an `int[][]` —
    // no door from C+, so it is one here.
    public static android.content.res.ColorStateList checkedCsl(int on, int off) {
        int[][] states = new int[][] {
            new int[] { android.R.attr.state_checked },
            new int[] {},
        };
        return new android.content.res.ColorStateList(states, new int[] { on, off });
    }

    // A LENGTH CAP, which Android spells as an InputFilter ARRAY — no door from
    // C+, so it is one here. Zero is no cap, which is the reading every numeric
    // zero gets in facet's contract, and it clears the filters rather than
    // setting a cap of nothing.
    public static void maxLength(android.widget.TextView v, int max) {
        if (max <= 0) {
            v.setFilters(new android.text.InputFilter[0]);
            return;
        }
        v.setFilters(new android.text.InputFilter[] {
            new android.text.InputFilter.LengthFilter(max) });
    }

    // HTML, which is what `text_format: Html` asks for. `Html.fromHtml` is
    // Android's own parser and the FROM_HTML_MODE_LEGACY flag is the one that
    // matches every other platform's block handling.
    public static CharSequence html(String source) {
        return android.text.Html.fromHtml(source, android.text.Html.FROM_HTML_MODE_LEGACY);
    }

    public static void setHtml(android.widget.TextView v, String source) {
        v.setText(html(source));
    }

    // The theme's own press colour. Asked for rather than picked: a highlight
    // that does not come from the theme is wrong in one of the two modes.
    //
    // Through `obtainStyledAttributes`, NOT `resolveAttribute`. The first shape
    // of this read `TypedValue.data` after resolving the attribute, and
    // `colorControlHighlight` resolves to a COLOR STATE LIST here — so `data`
    // was a resource id read as a colour, which came out invisible. Every press
    // in the app changed exactly nothing, and the mechanism looked broken when
    // the number was.
    private static int highlight(android.view.View v) {
        android.content.res.TypedArray a = v.getContext().obtainStyledAttributes(
            new int[] { android.R.attr.colorControlHighlight });
        int c = a.getColor(0, 0);
        a.recycle();
        // A theme that answers nothing still has to give a finger something to
        // see: white at 20% over dark, black at 20% over light.
        if (android.graphics.Color.alpha(c) == 0) {
            int bg = 0;
            android.content.res.TypedArray b = v.getContext().obtainStyledAttributes(
                new int[] { android.R.attr.colorBackground });
            bg = b.getColor(0, 0xFF000000);
            b.recycle();
            boolean dark = (android.graphics.Color.red(bg) + android.graphics.Color.green(bg)
                            + android.graphics.Color.blue(bg)) < 384;
            c = dark ? 0x33FFFFFF : 0x33000000;
        }
        return c;
    }

    // FACET'S OWN ICON FONT, from the APK's assets.
    //
    // `Typeface.createFromAsset` is the only door that takes an AssetManager,
    // and the FILL axis needs `Typeface.Builder` — which takes an
    // AssetManager too, and is API 26, this backend's floor exactly. So the
    // font is loaded once per fill value and kept: a Typeface is immutable and
    // an icon strip would otherwise rebuild it per glyph.
    //
    // MaterialSymbolsOutlined is a VARIABLE font with the FILL axis kept (0
    // outline .. 1 filled). `setFontVariationSettings` is how that axis is
    // asked for, and it is why this cannot be `createFromAsset`.
    private static final java.util.HashMap<String, android.graphics.Typeface> FONTS =
        new java.util.HashMap<>();

    public static android.graphics.Typeface iconFont(android.content.Context c,
                                                     String path, int fill) {
        String key = path + "#" + fill;
        android.graphics.Typeface t = FONTS.get(key);
        if (t != null) return t;
        try {
            android.graphics.Typeface.Builder b =
                new android.graphics.Typeface.Builder(c.getAssets(), path);
            if (fill > 0) b.setFontVariationSettings("'FILL' 1");
            t = b.build();
        } catch (Exception e) {
            t = null;
        }
        if (t == null) return null;
        FONTS.put(key, t);
        return t;
    }

    // A block of text in a box: wrapped, aligned both ways, and clipped or not.
    // `align` 0 start / 1 center / 2 end, `valign` 0 top / 1 middle / 2 bottom.
    public static void textBlock(android.graphics.Canvas canvas, android.graphics.Paint p,
                                 String s, float x, float y, float w, float h,
                                 int align, int valign, boolean clip, float lineSpacing) {
        if (s == null || s.length() == 0 || w <= 0f) return;
        android.text.TextPaint tp = new android.text.TextPaint(p);
        android.text.Layout.Alignment a = android.text.Layout.Alignment.ALIGN_NORMAL;
        if (align == 1) a = android.text.Layout.Alignment.ALIGN_CENTER;
        else if (align == 2) a = android.text.Layout.Alignment.ALIGN_OPPOSITE;
        android.text.StaticLayout l = new android.text.StaticLayout(
            s, tp, (int) Math.ceil(w), a, 1f, lineSpacing, false);
        float dy = 0f;
        if (valign == 1) dy = (h - l.getHeight()) / 2f;
        else if (valign == 2) dy = h - l.getHeight();
        int save = canvas.save();
        if (clip) canvas.clipRect(x, y, x + w, y + h);
        canvas.translate(x, y + dy);
        l.draw(canvas);
        canvas.restoreToCount(save);
    }
}
