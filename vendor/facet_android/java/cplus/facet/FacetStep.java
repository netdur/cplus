package cplus.facet;

// A stepper is three Android views inside ONE facet node — minus, value, plus —
// so the two buttons share a token and are told apart by their delta. Every
// other adapter here carries only the node address, because every other control
// has one place a tap can come from.
public final class FacetStep implements android.view.View.OnClickListener {
    private final long token;
    private final int delta;
    public FacetStep(long token, int delta) { this.token = token; this.delta = delta; }
    @Override public void onClick(android.view.View v) { nativeStep(token, delta); }
    private static native void nativeStep(long token, int delta);
}
