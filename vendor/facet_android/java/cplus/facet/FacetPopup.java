package cplus.facet;

// The picker's JVM half: a `Spinner`, its adapter, and the selection coming
// back.
//
// THE ITEMS ARE ACCUMULATED HERE because they arrive one at a time. An
// `ArrayAdapter` takes a List and `vendor/jni` types no array slots, so C+
// calls `begin`, then `add` per item, then `commit` — the same shape a builder
// has, with the list living on this side for the length of it.
//
// `isEnabled` is overridden rather than dropped: facet's `item_enabled` answers
// false for a header, and a header that can be PICKED is a different control.
public final class FacetPopup implements android.widget.AdapterView.OnItemSelectedListener {
    private static final java.util.HashMap<Long, java.util.ArrayList<String>> LABELS =
        new java.util.HashMap<>();
    private static final java.util.HashMap<Long, java.util.ArrayList<Boolean>> ENABLED =
        new java.util.HashMap<>();

    private final long token;

    public FacetPopup(long token) { this.token = token; }

    // NO MUTE, and that is a MEASURED decision rather than an omission. This
    // class had one — the guard FacetText carries for a keystroke — and a log
    // of the real sequence showed it never closed: a Spinner POSTS
    // `onItemSelected`, so the callback for the backend's own `setSelection`
    // arrives after the commit has already unmuted, `muted=false` every time.
    //
    // What actually suppresses the echo is on the C+ side and always was:
    // `fire_picked` returns early when the index it is handed is the one the
    // props already hold. A guard that works beats a guard that reads well.

    public static void begin(long token) {
        LABELS.put(token, new java.util.ArrayList<String>());
        ENABLED.put(token, new java.util.ArrayList<Boolean>());
    }

    public static void add(long token, String label, boolean enabled) {
        java.util.ArrayList<String> l = LABELS.get(token);
        if (l == null) return;
        l.add(label);
        ENABLED.get(token).add(Boolean.valueOf(enabled));
    }

    public static void commit(android.widget.Spinner s, long token, int selected) {
        final java.util.ArrayList<String> labels = LABELS.get(token);
        final java.util.ArrayList<Boolean> enabled = ENABLED.get(token);
        if (labels == null) return;
        android.widget.ArrayAdapter<String> a =
            new android.widget.ArrayAdapter<String>(s.getContext(),
                    android.R.layout.simple_spinner_item, labels) {
                @Override public boolean isEnabled(int position) {
                    if (position < 0 || position >= enabled.size()) return true;
                    return enabled.get(position).booleanValue();
                }
                @Override public boolean areAllItemsEnabled() { return false; }
            };
        a.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        s.setAdapter(a);
        if (selected >= 0 && selected < labels.size()) s.setSelection(selected, false);
    }

    @Override public void onItemSelected(android.widget.AdapterView<?> parent,
                                         android.view.View v, int position, long id) {
        if (!muted) nativePicked(token, position);
    }

    @Override public void onNothingSelected(android.widget.AdapterView<?> parent) { }

    private static native void nativePicked(long token, int index);
}
