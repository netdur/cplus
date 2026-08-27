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
//
// NOT final, and for exactly one reason: FacetSwipeHost is this host plus a
// touch intercept. Every facet host measures and lays out the same way, so a
// swipeable inheriting a second copy of that would be the drift this file's
// whole comment is about.
public class FacetHost extends android.view.ViewGroup {
    public FacetHost(android.content.Context c) {
        super(c);
        setClipChildren(false);
    }

    // The size facet last gave this host, remembered for the ONE case where
    // Android asks rather than tells: a ScrollView measures its child with an
    // UNSPECIFIED height so the child can be taller than the viewport, and
    // `MeasureSpec.getSize` of an UNSPECIFIED spec is ZERO. A document that
    // measures zero gives the ScrollView a scroll range of zero, so the page
    // renders in full and does not move — which looks exactly like a scroll
    // that works on content that happens to fit.
    private int wantW, wantH;

    public void setWanted(int w, int h) { wantW = w; wantH = h; }

    @Override protected void onMeasure(int wSpec, int hSpec) {
        setMeasuredDimension(resolve(wSpec, wantW), resolve(hSpec, wantH));
    }

    // EXACTLY still wins: when Android pins a size, that is the size. The
    // remembered one only fills in what a loose spec leaves open.
    private static int resolve(int spec, int want) {
        int mode = android.view.View.MeasureSpec.getMode(spec);
        int size = android.view.View.MeasureSpec.getSize(spec);
        if (mode == android.view.View.MeasureSpec.EXACTLY) return size;
        if (want <= 0) return size;
        if (mode == android.view.View.MeasureSpec.AT_MOST) return Math.min(want, size);
        return want;
    }

    @Override protected void onLayout(boolean changed, int l, int t, int r, int b) {
        // Deliberately empty. See above.
    }

    // A WIDGET ASKING FOR A LAYOUT HAS TO REACH FACET, or it never gets one.
    //
    // `onLayout` being empty is the design, and it has a consequence: a child
    // that changes its own size or content calls `requestLayout()`, the request
    // walks up to this host, and here it dies. Android would have re-laid the
    // subtree out; facet places views only when its OWN geometry changes, and a
    // widget's internal state is not facet's geometry.
    //
    // Measured, and it is not theoretical: a Spinner reports its new selection
    // from `checkSelectionChanged`, which runs inside the Spinner's own LAYOUT.
    // Picking an item therefore did nothing at all — no callback, no handler, no
    // redraw — until some unrelated change made facet run a pass, at which point
    // the pick arrived late. "I select 2, nothing happens; I toggle the switch
    // and the picker suddenly changes."
    //
    // So the request is forwarded: facet schedules a pass, the pass places every
    // view, the widget lays out, and whatever it wanted to report it reports.
    // The native side ignores requests raised INSIDE a pass — every `setText` in
    // an apply raises one — so this cannot feed itself.
    @Override public void requestLayout() {
        super.requestLayout();
        nativeLayoutRequested();
    }

    private static native void nativeLayoutRequested();

    // ONLY THE ROOT REPORTS. Every box, and every scroll document, is a
    // FacetHost too — and each of them is resized by facet's own layout pass.
    // A host that reported its own size would hand that back as THE WINDOW
    // SIZE and the next pass would lay the tree out against it: a document
    // sized to its 2979px content told facet the window was 2979 tall, so the
    // scroll's viewport became as tall as its content and there was nothing
    // left to scroll.
    //
    // Latent for as long as the root was the only host in the tree, which it
    // was until a `box` or a `scroll` appeared.
    private boolean reportsSize;

    public void setReportsSize(boolean r) { reportsSize = r; }

    // The window changed size (rotation, split screen, IME). facet has to
    // re-run layout, and only the host knows when.
    @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
        super.onSizeChanged(w, h, ow, oh);
        if (reportsSize) nativeSizeChanged(w, h);
    }

    private static native void nativeSizeChanged(int w, int h);
}
