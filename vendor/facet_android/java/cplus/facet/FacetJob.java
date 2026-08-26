package cplus.facet;

// A job to run ON THE UI THREAD, posted from somewhere else.
//
// The agent surface answers on a socket worker, and two things there are not
// that thread's to touch: a JNIEnv belongs to one thread, and facet's tree
// belongs to the UI thread (mount.cplus M6 asserts it on every write). So the
// worker posts this and waits, and the work happens where it is allowed to.
public final class FacetJob implements Runnable {
    @Override public void run() { nativeJob(); }
    private static native void nativeJob();
}
