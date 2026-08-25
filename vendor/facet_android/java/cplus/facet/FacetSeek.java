package cplus.facet;

// The read half of a slider. SeekBar reports three things and facet declares
// all three — `on_value_changed`, `on_drag_started`, `on_drag_completed` — so
// all three cross rather than only the one that moves the number.
//
// `fromUser` is DROPPED on purpose: `setProgress` from an apply pass calls this
// with false, and the C+ side already guards its own writes. Passing it would
// invite a backend to grow a second rule about which changes are real.
public final class FacetSeek implements android.widget.SeekBar.OnSeekBarChangeListener {
    private final long token;
    public FacetSeek(long token) { this.token = token; }
    @Override public void onProgressChanged(android.widget.SeekBar b, int p, boolean fromUser) {
        if (fromUser) nativeProgress(token, p);
    }
    @Override public void onStartTrackingTouch(android.widget.SeekBar b) { nativeSeekStart(token); }
    @Override public void onStopTrackingTouch(android.widget.SeekBar b) { nativeSeekStop(token); }
    private static native void nativeProgress(long token, int progress);
    private static native void nativeSeekStart(long token);
    private static native void nativeSeekStop(long token);
}
