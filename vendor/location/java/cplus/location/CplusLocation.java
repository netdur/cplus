package cplus.location;

// The Java half of `vendor/location` on Android.
//
// It exists because `LocationListener` is an INTERFACE, and JNI cannot
// implement one without either a java.lang.reflect.Proxy or a real class. A
// real class is cheaper to reason about and is what `CplusCamera` already does
// for camera2's abstract callbacks.
//
// FRAMEWORK LocationManager, not Play Services' FusedLocationProvider. Fused
// gives better fixes indoors, and it arrives as a Play Services AAR — a
// dependency the AAR measurement priced at megabytes of dex for a package
// whose whole job is a latitude and a longitude. The framework provider ships
// with the OS and needs nothing.

import android.content.Context;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;

public final class CplusLocation implements LocationListener {

    private final long token;
    private LocationManager manager;
    private boolean once;
    private boolean running;
    private int lastError;

    // Kept so `stop` can remove exactly this listener, and so the timeout can
    // be cancelled — the framework has no per-request handle.
    private Handler handler;
    private Runnable timeout;

    public CplusLocation(long token) { this.token = token; }

    public int lastError() { return lastError; }

    // Which provider answers a tier: GPS for the fine ones, NETWORK for coarse.
    //
    // FUSED IS DELIBERATELY NOT USED, though API 31+ has it. Two reasons, and
    // the second is the one that bites. It is a BLEND, so it does not map onto
    // the accuracy tiers the facade publishes — asking it for "Fine" and being
    // handed a network fix would make `Updates::accuracy()` a lie. And the
    // EMULATOR never feeds it: `adb emu geo fix` reaches the GPS provider
    // only, so a package preferring FUSED reports every provider healthy, the
    // stream running, and delivers nothing — measured 2026-09-02, every
    // provider's `last location=null` while `geo fix` answered OK.
    private String providerFor(int accuracy) {
        if (accuracy <= 0) { return LocationManager.NETWORK_PROVIDER; }
        try {
            if (manager.isProviderEnabled(LocationManager.GPS_PROVIDER)) {
                return LocationManager.GPS_PROVIDER;
            }
        } catch (Throwable ignored) { }
        return LocationManager.NETWORK_PROVIDER;
    }

    // 0 = started. 1 = denied, 2 = unsupported, 3 = services off, 5 = failed.
    //
    // THE PERMISSION IS NOT CHECKED HERE. `requestLocationUpdates` throws
    // SecurityException when it is missing, which is the only signal Android
    // gives and is caught below — the same shape as camera2's `openCamera`.
    public int start(Context ctx, int accuracy, float distanceFilterM,
                     long intervalMs, long timeoutMs, boolean onceOnly) {
        lastError = 0;
        try {
            manager = (LocationManager) ctx.getSystemService(Context.LOCATION_SERVICE);
            if (manager == null) { lastError = 2; return 2; }
            if (!manager.isLocationEnabled()) { lastError = 3; return 3; }

            String provider = providerFor(accuracy);
            once = onceOnly;
            running = true;
            handler = new Handler(Looper.getMainLooper());

            long minTime = intervalMs > 0 ? intervalMs : 0;
            manager.requestLocationUpdates(provider, minTime, distanceFilterM, this,
                                           Looper.getMainLooper());

            // A ONE-SHOT NEEDS ITS OWN CLOCK. The framework has no
            // "requestSingleUpdate with a deadline" that is not deprecated, so
            // the give-up is a posted Runnable — without it an app indoors
            // waits forever, which is the exact failure the Apple half reports
            // through its own ten-second timer.
            if (once) {
                final long ms = timeoutMs > 0 ? timeoutMs : 15000L;
                timeout = new Runnable() {
                    @Override public void run() {
                        if (!running) { return; }
                        stop();
                        // An invalid fix, matching the facade's contract:
                        // negative accuracy means "no position", never (0,0).
                        nativeFix(token, 0.0, 0.0, -1.0, 0.0, -1.0, -1.0f, -1.0f, 0L);
                    }
                };
                handler.postDelayed(timeout, ms);
            }
            return 0;
        } catch (SecurityException se) {
            // ACCESS_FINE/COARSE_LOCATION is not granted. Android throws rather
            // than prompting; asking is the caller's job, before this.
            lastError = 1;
            running = false;
            return 1;
        } catch (Throwable t) {
            android.util.Log.w("CplusLocation", "start failed", t);
            lastError = 5;
            running = false;
            return 5;
        }
    }

    public void stop() {
        running = false;
        if (handler != null && timeout != null) {
            try { handler.removeCallbacks(timeout); } catch (Throwable ignored) { }
        }
        timeout = null;
        try { if (manager != null) { manager.removeUpdates(this); } } catch (Throwable ignored) { }
    }

    public boolean isRunning() { return running; }

    // What the grant ACTUALLY allows: 0 coarse, 2 fine.
    //
    // Android 12+ lets a person grant approximate location while a fine
    // request succeeds, so this is read from the permission rather than echoed
    // back from what was asked.
    public int grantedAccuracy(Context ctx) {
        try {
            if (ctx.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION)
                    == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                return 2;
            }
        } catch (Throwable ignored) { }
        return 0;
    }

    public boolean lastKnown(Context ctx, int accuracy) {
        try {
            LocationManager m = (LocationManager) ctx.getSystemService(Context.LOCATION_SERVICE);
            if (m == null) { return false; }
            Location l = m.getLastKnownLocation(providerForStatic(m, accuracy));
            if (l == null) { return false; }
            deliver(l);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    private static String providerForStatic(LocationManager m, int accuracy) {
        if (accuracy <= 0) { return LocationManager.NETWORK_PROVIDER; }
        try {
            if (m.isProviderEnabled(LocationManager.GPS_PROVIDER)) {
                return LocationManager.GPS_PROVIDER;
            }
        } catch (Throwable ignored) { }
        return LocationManager.NETWORK_PROVIDER;
    }

    @Override public void onLocationChanged(Location l) {
        if (!running) { return; }
        if (once) { stop(); }
        deliver(l);
    }

    // THE UNKNOWNS ARE NEGATIVE, matching the facade and CoreLocation. Android
    // answers `hasSpeed()`/`hasBearing()` instead of a sentinel, so the
    // translation happens here rather than leaking two conventions across the
    // seam.
    private void deliver(Location l) {
        double altAcc = -1.0;
        if (android.os.Build.VERSION.SDK_INT >= 26 && l.hasVerticalAccuracy()) {
            altAcc = l.getVerticalAccuracyMeters();
        }
        nativeFix(token,
                  l.getLatitude(), l.getLongitude(),
                  l.hasAccuracy() ? l.getAccuracy() : -1.0f,
                  l.hasAltitude() ? l.getAltitude() : 0.0,
                  altAcc,
                  l.hasSpeed() ? l.getSpeed() : -1.0f,
                  l.hasBearing() ? l.getBearing() : -1.0f,
                  l.getTime());
    }

    // Deprecated on API 29+ and still abstract on older levels; overriding it
    // costs nothing and keeps this class loadable everywhere.
    @Override public void onStatusChanged(String p, int s, Bundle e) { }
    @Override public void onProviderEnabled(String p) { }
    @Override public void onProviderDisabled(String p) { }

    private static native void nativeFix(long token, double lat, double lon,
                                         double accuracy, double altitude,
                                         double altitudeAccuracy,
                                         float speed, float course, long timeMs);
}
