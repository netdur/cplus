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
        setContentView(nativeCreateView(this));
    }

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
