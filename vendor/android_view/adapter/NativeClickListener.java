package cplus.androidview;

/**
 * The package-shipped click adapter. Compiled to adapter.dex (see build.sh)
 * and embedded in the C+ binary via #include_bytes; android_view/listener
 * loads it at runtime with InMemoryDexClassLoader and binds nativeOnClick
 * with RegisterNatives. Host apps ship no Java for click handling.
 */
public final class NativeClickListener implements android.view.View.OnClickListener {
    private final long token;

    public NativeClickListener(long token) { this.token = token; }

    private static native void nativeOnClick(long token, android.view.View v);

    @Override public void onClick(android.view.View v) { nativeOnClick(token, v); }
}
