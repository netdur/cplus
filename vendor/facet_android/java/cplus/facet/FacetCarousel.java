package cplus.facet;

// A CAROUSEL, which on Android is a paging HorizontalScrollView.
//
// Not a ViewPager (AndroidX, an .aar with its own dex, and this project ships
// no Gradle) and not a Gallery: a carousel's pages are the CHILDREN facet put
// under the node, already built and already mounted, so an adapter would have
// nothing to answer. What is missing from a plain HorizontalScrollView is only
// the paging — a drag that ends between two pages has to finish on one, and a
// fling has to move exactly one page rather than however far the velocity
// carries it.
//
// The page width comes from facet, because facet is what decided it: a
// carousel's `columns` and `peek_insets` are what make a page narrower than the
// viewport, and both are laid out on the C+ side.
public final class FacetCarousel extends android.widget.HorizontalScrollView {
    private final long token;
    private int pageWidth = 1;
    private int pages;
    private boolean swipeable = true;
    private boolean animated = true;
    private int current = -1;

    public FacetCarousel(android.content.Context c, long token) {
        super(c);
        this.token = token;
        setHorizontalScrollBarEnabled(false);
    }

    public void setPage(int width, int count) {
        pageWidth = Math.max(1, width);
        pages = Math.max(0, count);
    }

    public void setSwipeable(boolean s) { swipeable = s; }
    public void setAnimated(boolean a) { animated = a; }

    // WHERE FACET SAYS TO BE. Also the one place `current` is set without
    // reporting first, so `position` written by an application does not come
    // straight back as a change it did not make.
    public void goTo(int index, boolean report) {
        int page = clamp(index);
        if (animated) smoothScrollTo(page * pageWidth, 0);
        else scrollTo(page * pageWidth, 0);
        if (report) report(page);
        else current = page;
    }

    // A VIEWPORT CLIPS, and here it has to do it itself.
    //
    // `clipChildren` bounds each child to the CHILD'S own rectangle, not to the
    // parent's — a child laid out larger than its parent still draws in full,
    // and what would cut it off is the GRANDPARENT clipping this view. Every
    // FacetHost turns that off in its constructor, on purpose: facet places by
    // frame and a shadow or an overshoot must not be sliced. So the document —
    // four pages wide inside a one-page window — drew the pages either side of
    // the current one straight over whatever the carousel sat in.
    //
    // Clipping to the scrolled rectangle is the whole fix, and it belongs here
    // rather than in a flag: this is the one view in the tree that is
    // deliberately smaller than its content.
    @Override protected void dispatchDraw(android.graphics.Canvas canvas) {
        int save = canvas.save();
        canvas.clipRect(getScrollX(), getScrollY(),
                        getScrollX() + getWidth(), getScrollY() + getHeight());
        super.dispatchDraw(canvas);
        canvas.restoreToCount(save);
    }

    @Override public boolean onInterceptTouchEvent(android.view.MotionEvent e) {
        if (!swipeable) return false;
        return super.onInterceptTouchEvent(e);
    }

    // A DRAG THAT ENDS BETWEEN PAGES FINISHES ON ONE. `super` first: the scroll
    // has to have moved before there is anything to snap.
    @Override public boolean onTouchEvent(android.view.MotionEvent e) {
        if (!swipeable) return false;
        boolean handled = super.onTouchEvent(e);
        int action = e.getActionMasked();
        if (action == android.view.MotionEvent.ACTION_UP
                || action == android.view.MotionEvent.ACTION_CANCEL) {
            goTo(Math.round(getScrollX() / (float) pageWidth), true);
        }
        return handled;
    }

    // ONE PAGE PER FLING, which is what makes this a pager rather than a scroll
    // that happens to stop on a boundary. HorizontalScrollView calls this with
    // the NEGATED finger velocity — a finger moving right (towards the previous
    // page) arrives here negative — so the sign reads the way scrollX moves.
    @Override public void fling(int velocityX) {
        int base = getScrollX() / pageWidth;
        int into = getScrollX() % pageWidth;
        int target;
        if (velocityX > 400) target = base + 1;
        else if (velocityX < -400) target = (into == 0) ? base - 1 : base;
        else target = Math.round(getScrollX() / (float) pageWidth);
        goTo(target, true);
    }

    private int clamp(int index) {
        if (index < 0) return 0;
        if (pages > 0 && index > pages - 1) return pages - 1;
        return index;
    }

    private void report(int index) {
        if (index == current) return;
        current = index;
        nativeCarouselPage(token, index);
    }

    private static native void nativeCarouselPage(long token, int index);
}
