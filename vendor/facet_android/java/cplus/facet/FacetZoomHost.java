package cplus.facet;

// Pinch-to-magnify for the whole tree.
//
// `Chrome.zoomable` is a WINDOW property, not a control: it magnifies what is
// already laid out and reflows nothing, which is what a magnifier does and what
// the iOS backend gets from a UIScrollView's zoom. Android has no scroll view
// that zooms an arbitrary child, so this is the same idea spelled out: one
// ScaleGestureDetector, a scale and a translation, written onto the child.
//
// SCALE ONLY, NEVER A RELAYOUT. facet's frames are computed once at the window
// size and the magnification is a transform on top of them, so text stays as
// crisp as the platform's own scaling makes it and nothing in the tree learns
// about the zoom. A pinch that reflowed would be a different feature.
//
// TOUCHES BELONG TO THE CONTENT until there are two of them. Android maps a
// touch through a child's scale and translation on the way down, so a button
// under a magnified tree is still hit where it is drawn.
public class FacetZoomHost extends android.view.ViewGroup {
    private final float min, max;
    private float scale = 1f, tx = 0f, ty = 0f;
    private float lastFocusX, lastFocusY;
    private final android.view.ScaleGestureDetector detector;

    public FacetZoomHost(android.content.Context c, float min, float max) {
        super(c);
        this.min = min <= 0f ? 1f : min;
        this.max = max <= this.min ? this.min : max;
        setClipChildren(true);
        detector = new android.view.ScaleGestureDetector(c,
            new android.view.ScaleGestureDetector.SimpleOnScaleGestureListener() {
                @Override public boolean onScaleBegin(android.view.ScaleGestureDetector d) {
                    lastFocusX = d.getFocusX();
                    lastFocusY = d.getFocusY();
                    return true;
                }
                @Override public boolean onScale(android.view.ScaleGestureDetector d) {
                    // The focus MOVES, and that movement is a two-finger pan —
                    // applied before the zoom so the pinch is about where the
                    // fingers are now.
                    tx += d.getFocusX() - lastFocusX;
                    ty += d.getFocusY() - lastFocusY;
                    lastFocusX = d.getFocusX();
                    lastFocusY = d.getFocusY();
                    zoomTo(clamp(scale * d.getScaleFactor(), min, max),
                           d.getFocusX(), d.getFocusY());
                    return true;
                }
            });
    }

    // The point under the fingers stays under the fingers: with the pivot at
    // the origin a child draws at `x * s + t`, so holding `f` fixed across a
    // scale change is one line and no pivot juggling.
    private void zoomTo(float next, float fx, float fy) {
        if (scale <= 0f) return;
        tx = fx - (fx - tx) * next / scale;
        ty = fy - (fy - ty) * next / scale;
        scale = next;
        settle();
    }

    // No empty margin: the content is never smaller than the window and never
    // dragged past its own edge.
    private void settle() {
        float w = getWidth(), h = getHeight();
        tx = clamp(tx, Math.min(0f, w - w * scale), 0f);
        ty = clamp(ty, Math.min(0f, h - h * scale), 0f);
        for (int i = 0; i < getChildCount(); i++) {
            android.view.View c = getChildAt(i);
            c.setPivotX(0f);
            c.setPivotY(0f);
            c.setScaleX(scale);
            c.setScaleY(scale);
            c.setTranslationX(tx);
            c.setTranslationY(ty);
        }
    }

    private static float clamp(float v, float lo, float hi) {
        if (v < lo) return lo;
        if (v > hi) return hi;
        return v;
    }

    @Override protected void onMeasure(int wSpec, int hSpec) {
        int w = android.view.View.MeasureSpec.getSize(wSpec);
        int h = android.view.View.MeasureSpec.getSize(hSpec);
        for (int i = 0; i < getChildCount(); i++) {
            getChildAt(i).measure(
                android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
        }
        setMeasuredDimension(w, h);
    }

    // The child FILLS this, always. Unlike a FacetHost — whose children are
    // facet's and placed by facet — this host has exactly one child and it is
    // the window.
    @Override protected void onLayout(boolean changed, int l, int t, int r, int b) {
        for (int i = 0; i < getChildCount(); i++) {
            getChildAt(i).layout(0, 0, r - l, b - t);
        }
        settle();
    }

    @Override public boolean onInterceptTouchEvent(android.view.MotionEvent e) {
        detector.onTouchEvent(e);
        return e.getPointerCount() > 1;
    }

    @Override public boolean onTouchEvent(android.view.MotionEvent e) {
        detector.onTouchEvent(e);
        return true;
    }
}
