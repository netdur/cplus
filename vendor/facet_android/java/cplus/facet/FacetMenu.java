package cplus.facet;

// A CONTEXT MENU, which on Android is a LONG PRESS and a PopupMenu.
//
// The kind is the first non-view one this backend answers: a `context_menu`
// node decorates the node it sits under, so it has no place in the view tree
// and no frame — the menu is built from the NODES and hung on the PARENT'S
// view, which is what `setOnLongClickListener` plus a PopupMenu amounts to
// here. facet_appkit hangs an NSMenu on `NSView.menu` for the same reason and
// calls the right-click its trigger; the gesture differs, the shape does not.
//
// `registerForContextMenu` is the other Android answer and is NOT used: it
// needs the Activity to override `onCreateContextMenu`, and this backend's
// Activity is the one class an app cannot replace.
//
// The items are accumulated the way a Spinner's are — `begin`, then `add` per
// item, then `attach` — because they arrive one at a time from C+ and this
// project types no array slots.
public final class FacetMenu implements android.view.View.OnLongClickListener,
                                        android.widget.PopupMenu.OnMenuItemClickListener {
    private static final java.util.HashMap<Long, java.util.ArrayList<String>> LABELS =
        new java.util.HashMap<>();
    private static final java.util.HashMap<Long, java.util.ArrayList<Boolean>> DESTRUCTIVE =
        new java.util.HashMap<>();

    private final long token;
    private final android.view.View anchor;

    private FacetMenu(long token, android.view.View anchor) {
        this.token = token;
        this.anchor = anchor;
    }

    public static void begin(long token) {
        LABELS.put(token, new java.util.ArrayList<String>());
        DESTRUCTIVE.put(token, new java.util.ArrayList<Boolean>());
    }

    public static void add(long token, String label, boolean destructive) {
        java.util.ArrayList<String> l = LABELS.get(token);
        if (l == null) return;
        l.add(label);
        DESTRUCTIVE.get(token).add(Boolean.valueOf(destructive));
    }

    // The parent's view is the anchor AND the trigger. Long-clickable is set
    // explicitly: a View that does not handle long presses is not sent them,
    // the same rule that made the split's divider need `setClickable`.
    public static void attach(android.view.View v, long token) {
        v.setLongClickable(true);
        v.setOnLongClickListener(new FacetMenu(token, v));
    }

    @Override public boolean onLongClick(android.view.View v) {
        java.util.ArrayList<String> labels = LABELS.get(token);
        if (labels == null || labels.isEmpty()) return false;
        java.util.ArrayList<Boolean> destructive = DESTRUCTIVE.get(token);
        android.widget.PopupMenu menu = new android.widget.PopupMenu(v.getContext(), anchor);
        for (int i = 0; i < labels.size(); i++) {
            CharSequence title = labels.get(i);
            // DESTRUCTIVE IS A COLOUR, because Android's menu API has no flag
            // for it and every app that marks one marks it red. facet_appkit
            // reached the same conclusion about NSMenuItem: a contract verb the
            // backend silently drops is worse than one it answers plainly.
            if (destructive != null && i < destructive.size() && destructive.get(i)) {
                android.text.SpannableString s = new android.text.SpannableString(title);
                s.setSpan(new android.text.style.ForegroundColorSpan(0xFFCC3333), 0,
                          s.length(), android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                title = s;
            }
            // GROUP 0, ID = the index, ORDER = the index: the id is what comes
            // back on a click and the order is what keeps the list in the order
            // facet declared it.
            menu.getMenu().add(0, i, i, title);
        }
        menu.setOnMenuItemClickListener(this);
        menu.show();
        return true;
    }

    @Override public boolean onMenuItemClick(android.view.MenuItem item) {
        nativeMenuItem(token, item.getItemId());
        return true;
    }

    private static native void nativeMenuItem(long token, int index);
}
