package com.cplus.hello;

public final class MainActivity extends android.app.Activity {
    static { System.loadLibrary("app"); }

    private static native android.view.View nativeCreateView(MainActivity self);

    @Override protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        setContentView(nativeCreateView(this));
    }
}
