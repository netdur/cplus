package cplus.facet;

// THE WINDOW'S INSETS CHANGED — and on an edge-to-edge window that is the only
// notice the keyboard gives.
//
// Left to itself Android RESIZES a window when the IME opens, and a scroll view
// inside it then scrolls the focused field into view by itself. Turning the
// system's fitting off — which is what makes `SafeArea::None` answerable — takes
// that away: the window keeps its full height, the keyboard covers the bottom
// third of it, and a field focused down there is typed into blind.
//
// So the insets become facet's to apply, the IME among them. This listener is
// how a change arrives: it does not consume the insets, it reports them.
public final class FacetInsets implements android.view.View.OnApplyWindowInsetsListener {
    @Override public android.view.WindowInsets onApplyWindowInsets(
            android.view.View v, android.view.WindowInsets insets) {
        // The pass runs INSIDE this call, synchronously, because the scroll
        // below needs the new geometry to already be in place — asking a scroll
        // view to reveal a rectangle before it has been given its smaller height
        // scrolls to where the field already was.
        nativeInsets();
        scrollFocusedIntoView(v);
        return insets;
    }

    // THE FOCUSED FIELD, BROUGHT BACK ABOVE THE KEYBOARD. `requestRectangleOnScreen`
    // walks up through the hosts — ViewGroup forwards it, offsetting as it goes —
    // and the first scroll view in the chain answers it. That is the same path
    // Android uses when it resizes the window itself, which is why a field in a
    // scroll behaves the way it does on every other app.
    private static void scrollFocusedIntoView(android.view.View v) {
        android.view.View focused = v.findFocus();
        if (focused == null) return;
        android.graphics.Rect r = new android.graphics.Rect(
            0, 0, focused.getWidth(), focused.getHeight());
        focused.requestRectangleOnScreen(r, true);
    }

    private static native void nativeInsets();
}
