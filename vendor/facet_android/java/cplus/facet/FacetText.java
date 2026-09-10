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
// AND THE CARET, which is the same object watching a different thing.
//
// A selection change is not a text change and no listener reports it: Android
// keeps the caret as two SPANS on the Editable — `Selection.SELECTION_START`
// and `SELECTION_END` — and moving it is a span move. A SpanWatcher hears
// exactly that, and it is attached the only way a SpanWatcher can be: by being
// set as a span itself, over the whole text.
//
// The alternative is subclassing EditText for `onSelectionChanged`, which would
// mean a facet EditText that is not the platform's EditText everywhere else in
// this backend. One object, two interfaces, no subclass.
public final class FacetText implements android.text.TextWatcher,
                                        android.text.SpanWatcher {
    private final long token;
    private boolean muted;
    private int lastStart = -1, lastEnd = -1;
    private boolean watching;
    public FacetText(long token) { this.token = token; }
    public void mute(boolean m) { this.muted = m; }

    // Attach as a span over the whole text, INCLUSIVE at both ends so it
    // survives typing at either edge. Re-attached whenever the text is replaced,
    // because a new Editable carries none of the old one's spans.
    public static void watchSelection(android.widget.EditText v, FacetText w) {
        w.watching = true;
        w.attach(v.getText());
    }

    private void attach(android.text.Editable e) {
        if (e == null) return;
        e.removeSpan(this);
        e.setSpan(this, 0, e.length(), android.text.Spanned.SPAN_INCLUSIVE_INCLUSIVE);
    }

    @Override public void onSpanChanged(android.text.Spannable text, Object what,
                                        int ostart, int oend, int nstart, int nend) {
        if (what != android.text.Selection.SELECTION_START
                && what != android.text.Selection.SELECTION_END) {
            return;
        }
        int start = android.text.Selection.getSelectionStart(text);
        int end = android.text.Selection.getSelectionEnd(text);
        if (start > end) { int t = start; start = end; end = t; }
        // BOTH SPANS MOVE FOR ONE CARET MOVE, so the pair is compared before
        // reporting: otherwise every arrow key is two callbacks saying the same
        // thing.
        if (start == lastStart && end == lastEnd) return;
        lastStart = start; lastEnd = end;
        if (!muted) nativeSelection(token, start, end - start);
    }

    @Override public void onSpanAdded(android.text.Spannable t, Object w, int s, int e) { }
    @Override public void onSpanRemoved(android.text.Spannable t, Object w, int s, int e) { }
    @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) { }
    @Override public void onTextChanged(CharSequence s, int a, int b, int c) { }
    @Override public void afterTextChanged(android.text.Editable s) {
        // RE-ARMED HERE, and it has to be. `setText` on an EditText hands the
        // view a NEW Editable, and spans do not survive that — so the caret
        // watch is dropped by the very first apply that writes the text, which
        // is every one of them. The TextWatcher registration DOES survive
        // (it lives on the view), so this is the one callback guaranteed to run
        // afterwards.
        if (watching && s.getSpanStart(this) < 0) attach(s);
        if (!muted) nativeText(token, s.toString());
    }
    private static native void nativeText(long token, String text);
    private static native void nativeSelection(long token, int start, int length);
}
