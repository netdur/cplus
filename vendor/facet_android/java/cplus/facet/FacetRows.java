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

    // Called from native when the model changed. ListView re-asks for its count
    // and re-binds what is on screen.
    public void changed() { notifyDataSetChanged(); }

    private static native int nativeRowCount(long token);
    private static native android.view.View nativeRowView(long token, int position,
                                                          android.view.View convertView,
                                                          android.view.ViewGroup parent);
}
