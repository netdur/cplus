package cplus.facet;

// The one ViewGroup facet_android ever creates.
//
// onLayout is EMPTY on purpose. facet owns every frame — flex computes them and
// the backend pushes them with View.layout() — so Android's layout pass is got
// out of the way rather than negotiated with. Children keep the rects C+ gave
// them, and an Android-initiated relayout cannot move them.
//
// Measured before this backend existed: it works, and it is what lets the whole
// LinearLayout/RelativeLayout/ConstraintLayout family go unbound.
public final class FacetHost extends android.view.ViewGroup {
    public FacetHost(android.content.Context c) {
        super(c);
        setClipChildren(false);
    }

    @Override protected void onMeasure(int wSpec, int hSpec) {
        setMeasuredDimension(
            android.view.View.MeasureSpec.getSize(wSpec),
            android.view.View.MeasureSpec.getSize(hSpec));
    }

    @Override protected void onLayout(boolean changed, int l, int t, int r, int b) {
        // Deliberately empty. See above.
    }

    // The window changed size (rotation, split screen, IME). facet has to
    // re-run layout, and only the host knows when.
    @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
        super.onSizeChanged(w, h, ow, oh);
        nativeSizeChanged(w, h);
    }

    private static native void nativeSizeChanged(int w, int h);
}
