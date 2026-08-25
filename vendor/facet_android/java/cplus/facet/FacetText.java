package cplus.facet;

// The read half of a text field. A TextWatcher fires per keystroke, which is
// the granularity facet's `on_text_changed` declares.
//
// The text CROSSES as a String rather than being read back through
// `getText().toString()` on the native side: that would be two more JNI
// crossings per keystroke to fetch something already sitting in `s`.
//
// A guard flag suppresses the callback while the backend is writing — an apply
// pass calling `setText` would otherwise report the application's own write
// back to it as if a finger had typed it, and a handler that writes on change
// would loop.
public final class FacetText implements android.text.TextWatcher {
    private final long token;
    private boolean muted;
    public FacetText(long token) { this.token = token; }
    public void mute(boolean m) { this.muted = m; }
    @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) { }
    @Override public void onTextChanged(CharSequence s, int a, int b, int c) { }
    @Override public void afterTextChanged(android.text.Editable s) {
        if (!muted) nativeText(token, s.toString());
    }
    private static native void nativeText(long token, String text);
}
