package cplus.facet;

// facet's `schedule` verb. A write marks a dirty bit and asks for a tick; the
// tick applies everything at once. postOnAnimation puts this on the
// Choreographer's animation queue — the same "just before the frame commits"
// slot facet_appkit's CFRunLoop observer occupies, and for the same reason.
public final class FacetTick implements Runnable {
    @Override public void run() { nativeTick(); }
    private static native void nativeTick();
}
