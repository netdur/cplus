package cplus.gallery;

// The WHOLE JVM side. facet_android ships its own Java — the layout host, the
// Choreographer tick, and one listener adapter per event shape — inside the
// package as a DEX, so an app contributes this and nothing else.
public final class MainActivity extends android.app.Activity {
    static { System.loadLibrary("gallery"); }

    private static native android.view.View nativeCreateView(MainActivity self);

    @Override protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        setContentView(nativeCreateView(this));
    }
}
