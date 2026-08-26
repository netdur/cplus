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

    // THE TYPOGRAPHY, PER PICKER. A Spinner's rows are inflated by ArrayAdapter
    // from a PLATFORM layout — `simple_spinner_item` — so nothing about them is
    // ours to build. What is ours is what happens to the view afterwards: the
    // adapter hands each row back through `getView`, and every one of facet's
    // font verbs is a call on the TextView it returns.
    //
    // Kept per token rather than per adapter because the adapter is rebuilt
    // whenever the items change, and the style outlives that.
    private static final java.util.HashMap<Long, Style> STYLES = new java.util.HashMap<>();

    private static final class Style {
        float size; boolean scales;
        boolean bold, italic;
        String family;
        int color; boolean hasColor;
        int gravity;
        float letterSpacing;
    }

    // `size` is in facet's units and `scales` decides which unit that becomes:
    // SP follows the reader's font setting, DIP does not — the same pair every
    // text control here uses. `letterSpacing` arrives already in EMs, because
    // the conversion needs the font size and C+ has it.
    public static void style(long token, float size, boolean scales,
                             boolean bold, boolean italic, String family,
                             int color, boolean hasColor, int gravity,
                             float letterSpacing) {
        Style st = new Style();
        st.size = size; st.scales = scales;
        st.bold = bold; st.italic = italic; st.family = family;
        st.color = color; st.hasColor = hasColor;
        st.gravity = gravity; st.letterSpacing = letterSpacing;
        STYLES.put(token, st);
    }

    private static void dress(long token, android.view.View v) {
        if (!(v instanceof android.widget.TextView)) return;
        Style st = STYLES.get(token);
        if (st == null) return;
        android.widget.TextView t = (android.widget.TextView) v;
        if (st.size > 0f) {
            t.setTextSize(st.scales
                ? android.util.TypedValue.COMPLEX_UNIT_SP
                : android.util.TypedValue.COMPLEX_UNIT_DIP, st.size);
        }
        int face = android.graphics.Typeface.NORMAL;
        if (st.bold && st.italic) face = android.graphics.Typeface.BOLD_ITALIC;
        else if (st.bold) face = android.graphics.Typeface.BOLD;
        else if (st.italic) face = android.graphics.Typeface.ITALIC;
        if (st.family != null && st.family.length() > 0) {
            t.setTypeface(android.graphics.Typeface.create(st.family, face));
        } else {
            t.setTypeface(t.getTypeface(), face);
        }
        if (st.hasColor) t.setTextColor(st.color);
        if (st.gravity != 0) t.setGravity(st.gravity);
        t.setLetterSpacing(st.letterSpacing);
    }

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
        final long tok = token;
        android.widget.ArrayAdapter<String> a =
            new android.widget.ArrayAdapter<String>(s.getContext(),
                    android.R.layout.simple_spinner_item, labels) {
                @Override public boolean isEnabled(int position) {
                    if (position < 0 || position >= enabled.size()) return true;
                    return enabled.get(position).booleanValue();
                }
                @Override public boolean areAllItemsEnabled() { return false; }
                // THE FIELD and THE LIST are two views of the same row, and
                // both come through here. Styling only the first leaves a
                // picker whose closed state and open state disagree.
                @Override public android.view.View getView(int position,
                        android.view.View convertView, android.view.ViewGroup parent) {
                    android.view.View v = super.getView(position, convertView, parent);
                    dress(tok, v);
                    return v;
                }
                @Override public android.view.View getDropDownView(int position,
                        android.view.View convertView, android.view.ViewGroup parent) {
                    android.view.View v = super.getDropDownView(position, convertView, parent);
                    dress(tok, v);
                    return v;
                }
            };
        a.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        s.setAdapter(a);
        if (selected >= 0 && selected < labels.size()) s.setSelection(selected, false);
    }

    // NO MUTE. A Spinner POSTS its selection callback rather than making it, so
    // a flag raised around `setSelection` is already lowered by the time the
    // callback runs — measured, and it read `muted=false` every time. What
    // stops the loop is the equality guard on the native side: a pick that
    // matches the prop is not a change and is not reported.
    @Override public void onItemSelected(android.widget.AdapterView<?> parent,
                                         android.view.View v, int position, long id) {
        nativePicked(token, position);
    }

    @Override public void onNothingSelected(android.widget.AdapterView<?> parent) { }

    private static native void nativePicked(long token, int index);
}
