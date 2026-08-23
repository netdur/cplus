package cplus.facet;

// Java interfaces cannot be implemented from native code, so every click rides
// this adapter. The token is the facet node's address, so one hook routes any
// number of controls without a side table.
public final class FacetClick implements android.view.View.OnClickListener {
    private final long token;
    public FacetClick(long token) { this.token = token; }
    @Override public void onClick(android.view.View v) { nativeClick(token); }
    private static native void nativeClick(long token);
}
