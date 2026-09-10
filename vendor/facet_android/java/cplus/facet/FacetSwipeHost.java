package cplus.facet;

// A facet host that can STEAL a horizontal drag.
//
// A swipeable's content is ordinary facet children — a box, a label, sometimes
// a button — and a swipe has to work over all of them. An OnTouchListener on
// the host only ever sees what no child consumed, so a swipeable with a button
// in it would be dead exactly where the button is. `onInterceptTouchEvent` is
// Android's answer to that and it is the whole reason this class exists.
//
// The decision is C+'s, both times: this class carries no slop, no axis test
// and no state. It asks, and does what it is told.
public class FacetSwipeHost extends FacetHost {
    private final long token;

    public FacetSwipeHost(android.content.Context c, long token) {
        super(c);
        this.token = token;
        // The content slides out of the box, and what leaves the box is not
        // drawn. A facet host does not clip; this one has to.
        setClipChildren(true);
    }

    @Override public boolean onInterceptTouchEvent(android.view.MotionEvent e) {
        return nativeIntercept(token, e.getActionMasked(), e.getX(), e.getY());
    }

    @Override public boolean onTouchEvent(android.view.MotionEvent e) {
        return nativeTouch(token, e.getActionMasked(), e.getX(), e.getY());
    }

    private static native boolean nativeIntercept(long token, int action, float x, float y);
    private static native boolean nativeTouch(long token, int action, float x, float y);
}
