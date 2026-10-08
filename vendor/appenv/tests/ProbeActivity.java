package dev.cplus.appenvprobe;

public final class ProbeActivity extends android.app.Activity {
    static { System.loadLibrary("appenvprobe"); }
    private static native int run(android.app.Activity activity);
    @Override public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        android.util.Log.i("AppEnvProbe", "result=" + run(this));
        finish();
    }
}
