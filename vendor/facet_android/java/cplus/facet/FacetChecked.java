package cplus.facet;

// The read half of every compound button — checkbox, toggle, radio. Same shape
// and same reason as FacetClick: a Java interface cannot be implemented from
// native code, so the listener has to be an object of a class that does, and
// the token is the facet node's address so one hook routes the whole tree.
//
// The new state is PASSED rather than read back. `isChecked()` would be a
// second JNI crossing to ask the control what it just told us, and the value is
// already in hand here.
public final class FacetChecked
        implements android.widget.CompoundButton.OnCheckedChangeListener {
    private final long token;
    public FacetChecked(long token) { this.token = token; }
    @Override public void onCheckedChanged(android.widget.CompoundButton v, boolean on) {
        nativeChecked(token, on);
    }
    private static native void nativeChecked(long token, boolean on);
}
