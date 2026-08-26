package cplus.facet;

// The one View in this package that DRAWS.
//
// Everything else here is a widget Android already owns; a facet `canvas` is a
// recorded display list that has to be replayed, and `onDraw` is the only place
// a `android.graphics.Canvas` exists. So the class is the thinnest possible
// bridge: it carries the facet node's address and hands the Canvas straight
// back across, exactly as FacetRows hands back its questions.
//
// `setWillNotDraw(false)` because a plain View is assumed not to draw and its
// onDraw is skipped — a View subclass that draws has to say so.
public final class FacetCanvas extends android.view.View {
    private final long token;

    public FacetCanvas(android.content.Context c, long token) {
        super(c);
        this.token = token;
        setWillNotDraw(false);
    }

    @Override protected void onDraw(android.graphics.Canvas canvas) {
        nativeDraw(token, canvas);
    }

    private static native void nativeDraw(long token, android.graphics.Canvas canvas);
}
