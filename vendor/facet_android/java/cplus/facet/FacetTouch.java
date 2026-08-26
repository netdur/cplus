package cplus.facet;

// The RAW touch, for the two kinds that want the edges of one rather than the
// click it becomes: a button's `on_pressed` / `on_released`, and a canvas's
// five-verb interaction band.
//
// Returns FALSE always: this is an observer, not a consumer. A button that
// reported its press and then swallowed it would never click, and the ripple
// would never draw — both of those belong to the platform's own handling, which
// only runs if the event is left alone.
public final class FacetTouch implements android.view.View.OnTouchListener {
    private final long token;

    public FacetTouch(long token) { this.token = token; }

    @Override public boolean onTouch(android.view.View v, android.view.MotionEvent e) {
        nativeTouchEvent(token, e.getActionMasked(), e.getX(), e.getY());
        return false;
    }

    private static native void nativeTouchEvent(long token, int action, float x, float y);
}
