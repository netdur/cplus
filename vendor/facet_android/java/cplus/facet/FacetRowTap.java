package cplus.facet;

// A row was tapped. `AdapterView.OnItemClickListener` is an interface, so it
// needs an object like every other event shape here.
public final class FacetRowTap implements android.widget.AdapterView.OnItemClickListener {
    private final long token;
    public FacetRowTap(long token) { this.token = token; }
    @Override public void onItemClick(android.widget.AdapterView<?> parent, android.view.View v,
                                      int position, long id) {
        nativeRowTapped(token, position);
    }
    private static native void nativeRowTapped(long token, int position);
}
