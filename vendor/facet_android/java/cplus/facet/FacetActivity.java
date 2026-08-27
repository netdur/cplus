package cplus.facet;

// THE APP'S ACTIVITY, so an app does not have to write one.
//
// Everything else in this package is loaded at RUNTIME from an in-memory DEX,
// which cannot supply this class: the system instantiates the launch Activity
// before any of our code runs. So this one is merged into the APK's own
// classes.dex at build time (`d8` takes a .dex as an input), and the manifest
// names it directly.
//
// What is left for an app to write is an AndroidManifest.xml and C+. No Java.
//
// The library name comes from manifest meta-data, the way NativeActivity takes
// `android.app.lib_name` — the class has to be generic across apps and cannot
// know what the .so is called.
public class FacetActivity extends android.app.Activity {

    private static final String META_LIB = "cplus.facet.lib";
    private static final String DEFAULT_LIB = "app";

    @Override protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        System.loadLibrary(libraryName());
        // EDGE TO EDGE, so facet decides where the safe area is.
        //
        // Left alone, the SYSTEM insets the window and every app is safe-area'd
        // whether it asked or not — which makes `SafeArea::None` unanswerable,
        // and a full-bleed photo is a real thing to ask for. Taking the whole
        // window means the insets become a value facet can read and apply per
        // node, the way the other two backends do.
        getWindow().setDecorFitsSystemWindows(false);
        setContentView(nativeCreateView(this));
    }

    // The system bars and the display cutout, in pixels, packed into one long
    // as left/top/right/bottom, 16 bits each.
    //
    // PACKED rather than an int[] because the array is the expensive half: four
    // numbers that each fit in a screen's worth of pixels cross as one primitive
    // return, and the JNI side needs no array handling at all.
    //
    // `getRootWindowInsets` answers null before the view is attached, and zero
    // is the right answer then — the first layout pass runs before the window
    // has insets to report, and the second one has them.
    public static long windowInsets(android.view.View v) {
        if (v == null) return 0L;
        android.view.WindowInsets w = v.getRootWindowInsets();
        if (w == null) return 0L;
        android.graphics.Insets i = w.getInsets(
            android.view.WindowInsets.Type.systemBars()
                | android.view.WindowInsets.Type.displayCutout());
        return ((long) (i.left & 0xffff) << 48) | ((long) (i.top & 0xffff) << 32)
             | ((long) (i.right & 0xffff) << 16) | (long) (i.bottom & 0xffff);
    }

    // THE SYSTEM BACK IS THE APP'S TO ANSWER, and closing was the wrong default.
    //
    // A back gesture that ends the process is what an app with no navigation
    // gets for free, and this backend gave it to every app — a demo screen deep
    // in the gallery, and back quit. facet has a navigation tier
    // (`nav::push` / `nav::pop`) whose hooks the running app registers, so the
    // press goes there first and only falls through to the platform when facet
    // says there is nowhere to go back TO. That fallthrough is the right
    // behaviour at the top level: on Android, back from the first screen leaves
    // the app.
    //
    // `onBackPressed` rather than an OnBackInvokedCallback: the callback API is
    // API 33 and opt-in per manifest, and the deprecated override still runs on
    // every level this backend targets, including where the new dispatcher is
    // on. One path, no version fork.
    @Override public void onBackPressed() {
        if (nativeBack()) return;
        super.onBackPressed();
    }

    private static native boolean nativeBack();

    private String libraryName() {
        try {
            android.content.pm.ActivityInfo info = getPackageManager().getActivityInfo(
                getComponentName(), android.content.pm.PackageManager.GET_META_DATA);
            if (info.metaData != null) {
                String name = info.metaData.getString(META_LIB);
                if (name != null && name.length() > 0) return name;
            }
        } catch (Throwable t) {
            // Fall through to the default. A missing meta-data line is an app
            // that took the default name, not an error.
        }
        return DEFAULT_LIB;
    }

    // Resolved by SYMBOL, not RegisterNatives: this class is loaded by the app's
    // own classloader, which has a native-library namespace — the very thing an
    // in-memory loader lacks and the reason every other native here is bound
    // explicitly.
    private static native android.view.View nativeCreateView(FacetActivity self);
}
