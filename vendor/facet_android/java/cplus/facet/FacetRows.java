package cplus.facet;

// The recycler's JVM half.
//
// `BaseAdapter` is ABSTRACT, so it cannot be implemented from native code — the
// same wall `View.OnClickListener` is, one size up. This is the smallest class
// that satisfies it: every question goes straight back across, and the token is
// the facet node's address as everywhere else here.
//
// WHY ListView AND NOT RecyclerView: RecyclerView is AndroidX, an .aar with its
// own dex and dependency graph, and this project ships no Gradle. ListView is in
// android.jar and recycles through `convertView`, which is the whole mechanism
// we need — facet owns layout, so RecyclerView's LayoutManagers would go unused.
public final class FacetRows extends android.widget.BaseAdapter {
    private final long token;

    public FacetRows(long token) { this.token = token; }

    @Override public int getCount() { return nativeRowCount(token); }
    @Override public Object getItem(int position) { return null; }
    @Override public long getItemId(int position) { return position; }

    // `convertView` is the recycled cell, or null the first time a row of this
    // shape is needed. Handing it back across is what makes scrolling a few
    // property writes instead of a subtree per row.
    @Override public android.view.View getView(int position, android.view.View convertView,
                                               android.view.ViewGroup parent) {
        return nativeRowView(token, position, convertView, parent);
    }

    // A ListView recycles by VIEW TYPE — `convertView` is only ever handed back
    // for a position of the same type — so without these every row went into
    // one pool and a section header could be handed to a body. `getViewTypeCount`
    // must be answered BEFORE any row is built and must not change, which is why
    // the native side caps the application's kinds rather than counting them.
    @Override public int getViewTypeCount() { return nativeViewTypeCount(token); }
    @Override public int getItemViewType(int position) {
        return nativeItemViewType(token, position);
    }

    // A HEADER IS NOT SELECTABLE. `areAllItemsEnabled` false plus this is what
    // stops a section title highlighting under a finger and reporting a tap as
    // a row — type 0 is the header pool.
    @Override public boolean areAllItemsEnabled() { return false; }
    @Override public boolean isEnabled(int position) {
        return nativeItemViewType(token, position) != 0;
    }

    // Called from native when the model changed. ListView re-asks for its count
    // and re-binds what is on screen.
    public void changed() { notifyDataSetChanged(); }

    private static native int nativeRowCount(long token);
    private static native int nativeViewTypeCount(long token);
    private static native int nativeItemViewType(long token, int position);
    private static native android.view.View nativeRowView(long token, int position,
                                                          android.view.View convertView,
                                                          android.view.ViewGroup parent);
}
