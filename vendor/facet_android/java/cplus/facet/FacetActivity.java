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
        // API 30, and the fallback is not a lesser mode — it is the system
        // doing the fitting, which is what every app got before this call
        // existed. On those levels `windowInsets` answers zero and facet insets
        // nothing, because the window it is given is already inside the bars.
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
        }
        setContentView(nativeCreateView(this));
        // AFTER setContentView, and that ordering is the whole cold-start
        // story: `nativeCreateView` runs the app's entry, so by the time this
        // line executes a package has had its chance to subscribe. The latch in
        // facet's app_events makes it safe even if it has not.
        deliverPayload(getIntent());
        deliverLink(getIntent());
        // A button pressed while this process was dead parked its payload; the
        // native exists now.
        drainPendingPayload();
    }

    // A NOTIFICATION TAPPED WHILE THE APP IS RUNNING lands here rather than in
    // onCreate. `setIntent` so a later `getIntent()` sees the new one instead of
    // the launch intent, which is the default and would replay a stale payload.
    @Override protected void onNewIntent(android.content.Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        deliverPayload(intent);
        deliverLink(intent);
    }

    // The payload a notification carried, if this intent is one. Opaque here:
    // facet does not know what it means, only who to hand it to.
    private void deliverPayload(android.content.Intent i) {
        if (i == null) return;
        String p = i.getStringExtra(PAYLOAD_EXTRA);
        if (p == null || p.length() == 0) return;
        // ONCE. A rotation re-runs onCreate against the same intent, and a
        // payload delivered twice is a deep link followed twice.
        i.removeExtra(PAYLOAD_EXTRA);
        nativeNotificationTap(p);
    }

    // The extra a notification's PendingIntent carries. Shared with
    // vendor/notifications, which writes it; the string is the contract.
    public static final String PAYLOAD_EXTRA = "cplus.payload";

    static native void nativeNotificationTap(String payload);

    // A DEEP LINK. The same two doors as the payload above and for the same
    // reason — cold in onCreate, warm in onNewIntent — but a different field:
    // a VIEW intent carries its URL as the intent's DATA, not as an extra.
    //
    // ONE INTENT-FILTER SERVES BOTH KINDS OF LINK. A custom scheme and a
    // verified https App Link arrive here identically; what differs is the
    // filter in the manifest and, for https, whether the system verified the
    // domain before deciding to send it here at all.
    //
    // `onNewIntent` ONLY FIRES FOR A singleTop ACTIVITY. The default launch
    // mode is `standard`, which builds a SECOND instance of this Activity for
    // an incoming link — a second facet mount stacked on the first. Notifications
    // never hit that because they build their own intent and set
    // FLAG_ACTIVITY_SINGLE_TOP; a browser does not do that for us. So the
    // manifest `cpc new` writes declares android:launchMode="singleTop", and an
    // app that hand-writes its manifest must do the same.
    private void deliverLink(android.content.Intent i) {
        if (i == null) return;
        android.net.Uri u = i.getData();
        if (u == null) return;
        String s = u.toString();
        if (s == null || s.length() == 0) return;
        // ONCE, the same discipline as the payload: a rotation re-runs onCreate
        // against the same intent, and a link followed twice is a route
        // followed twice.
        i.setData(null);
        nativeOpenUrl(s);
    }

    static native void nativeOpenUrl(String url);

    // A PAYLOAD THAT ARRIVED WITH NO NATIVE TO GIVE IT TO.
    //
    // `FacetNotificationReceiver` runs in this app's process, but a broadcast
    // can start that process from cold — and the .so is loaded in onCreate
    // below, so the receiver may fire before any native exists. It parks the
    // payload here and this drains it once the library is up.
    static String pendingPayload = null;

    private void drainPendingPayload() {
        String p = pendingPayload;
        if (p == null) return;
        pendingPayload = null;
        nativeNotificationTap(p);
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
        // The system fitted the window itself below API 30 — see onCreate — so
        // there is nothing left for facet to take off.
        if (android.os.Build.VERSION.SDK_INT < 30) return 0L;
        android.view.WindowInsets w = v.getRootWindowInsets();
        if (w == null) return 0L;
        // THE KEYBOARD IS AN INSET like any other. On a window the system no
        // longer fits, the IME is the one that moves during use — and a field
        // under it is a field typed into blind.
        android.graphics.Insets i = w.getInsets(
            android.view.WindowInsets.Type.systemBars()
                | android.view.WindowInsets.Type.displayCutout()
                | android.view.WindowInsets.Type.ime());
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

    // THE PERMISSION ANSWER HAS NO OTHER DOOR.
    //
    // `requestPermissions` is an Activity method and so is its result, so a
    // services package that wants to ask for the camera has nowhere to receive
    // the answer unless this class forwards it. facet's `app_events` is the
    // fan-out that forwards it — see E_PERMISSION_RESULT.
    //
    // ONE NATIVE CALL PER PERMISSION, rather than passing the two arrays down.
    // The loop is three lines here and the alternative is jobjectArray and
    // jintArray handling on the C+ side for a batch that is almost always one
    // or two entries. It also matches the payload `AppEvent` already has —
    // `text` for the permission, `result` for the answer — with no new struct.
    //
    // `onRequestPermissionsResult` is deprecated in favour of the Activity
    // Result API, and the deprecated override is still right here for the same
    // reason `onBackPressed` above is: the replacement is AndroidX and opt-in
    // per manifest, this one runs on every level this backend targets, and one
    // path beats a version fork.
    @Override public void onRequestPermissionsResult(int requestCode, String[] permissions,
                                                     int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (permissions == null || grantResults == null) return;
        int n = Math.min(permissions.length, grantResults.length);
        for (int i = 0; i < n; i++) {
            nativePermissionResult(requestCode, permissions[i], grantResults[i]);
        }
    }

    private static native void nativePermissionResult(int requestCode, String permission,
                                                      int result);

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
