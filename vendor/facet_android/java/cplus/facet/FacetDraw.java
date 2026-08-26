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

    // The theme's own press colour. Asked for rather than picked: a highlight
    // that does not come from the theme is wrong in one of the two modes.
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
