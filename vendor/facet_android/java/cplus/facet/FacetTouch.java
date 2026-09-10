package cplus.facet;

// The RAW touch, for the two kinds that want the edges of one rather than the
// click it becomes: a button's `on_pressed` / `on_released`, and a canvas's
// five-verb interaction band.
//
// Returns FALSE always: this is an observer, not a consumer. A button that
// reported its press and then swallowed it would never click, and the ripple
// would never draw — both of those belong to the platform's own handling, which
// only runs if the event is left alone.
// AND THE HOVER, which is the same report from a different stream: hover events
// never reach `onTouch` — they are their own dispatch, delivered to a mouse or a
// stylus and to nothing else — so a control that wants both has to be both
// listeners. One class rather than two, because the payload is identical and
// the action code already says which stream it came from (ENTER 9, MOVE 7,
// EXIT 10, against DOWN 0 / UP 1 / MOVE 2 / CANCEL 3).
public final class FacetTouch implements android.view.View.OnTouchListener,
                                         android.view.View.OnHoverListener {
    private final long token;

    public FacetTouch(long token) { this.token = token; }

    @Override public boolean onTouch(android.view.View v, android.view.MotionEvent e) {
        nativeTouchEvent(token, e.getActionMasked(), e.getX(), e.getY());
        return false;
    }

    // FALSE for the same reason `onTouch` is: a hover that is swallowed here is
    // a hover the platform cannot use for its own pointer feedback.
    @Override public boolean onHover(android.view.View v, android.view.MotionEvent e) {
        nativeTouchEvent(token, e.getActionMasked(), e.getX(), e.getY());
        return false;
    }

    private static native void nativeTouchEvent(long token, int action, float x, float y);
}
