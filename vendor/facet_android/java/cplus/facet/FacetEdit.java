package cplus.facet;

// The two reads a text control has beyond its keystrokes: the RETURN KEY and
// FOCUS.
//
// Both are listeners rather than overrides, so they hang on any EditText this
// backend made without a subclass — the same shape FacetText takes for the
// keystroke itself.
public final class FacetEdit implements android.widget.TextView.OnEditorActionListener,
                                        android.view.View.OnFocusChangeListener {
    private final long token;

    public FacetEdit(long token) { this.token = token; }

    // The action key was pressed. IME_ACTION_UNSPECIFIED arrives when the
    // keyboard has no action of its own and the ENTER key was hit, which is
    // still a submit — a field that answers Done but not Enter is a field that
    // ignores a hardware keyboard.
    @Override public boolean onEditorAction(android.widget.TextView v, int actionId,
                                            android.view.KeyEvent e) {
        if (e != null && e.getAction() != android.view.KeyEvent.ACTION_DOWN) return false;
        nativeSubmit(token);
        // FALSE, so the platform still does what the action says — a `Next`
        // still moves focus, a `Search` still closes the keyboard. facet's
        // handler is told; it does not take the key over.
        return false;
    }

    @Override public void onFocusChange(android.view.View v, boolean has) {
        nativeFocus(token, has);
    }

    private static native void nativeSubmit(long token);
    private static native void nativeFocus(long token, boolean focused);
}
