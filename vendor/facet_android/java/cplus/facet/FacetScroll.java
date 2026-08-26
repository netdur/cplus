package cplus.facet;

// WHERE A SCROLL IS, which facet asks for on four kinds and this backend
// answered on none.
//
// Two interfaces on one class because they are the same question asked of two
// widgets: a `ScrollView` reports PIXELS through `OnScrollChangeListener`, and
// an `AbsListView` reports ROWS through `OnScrollListener` — the first visible
// item, how many are visible and how many there are. The row form is what
// `on_item_appearing`, `on_item_disappearing` and the remaining-items threshold
// are all computed from, so it crosses whole rather than as a pixel offset the
// C+ side would have to turn back into rows.
public final class FacetScroll implements android.view.View.OnScrollChangeListener,
                                          android.widget.AbsListView.OnScrollListener {
    private final long token;

    public FacetScroll(long token) { this.token = token; }

    @Override public void onScrollChange(android.view.View v, int x, int y, int ox, int oy) {
        nativeScrolled(token, x, y);
    }

    @Override public void onScrollStateChanged(android.widget.AbsListView v, int state) { }

    @Override public void onScroll(android.widget.AbsListView v, int first, int visible,
                                   int total) {
        nativeRows(token, first, visible, total);
    }

    private static native void nativeScrolled(long token, int x, int y);
    private static native void nativeRows(long token, int first, int visible, int total);
}
