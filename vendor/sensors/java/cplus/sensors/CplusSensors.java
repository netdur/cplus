package cplus.sensors;

// The Java half of `vendor/sensors` on Android.
//
// It exists because `SensorEventListener` is an INTERFACE and JNI cannot
// implement one. Same arrangement as `CplusCamera` and `CplusLocation`.
//
// NO UNIT CONVERSION HAPPENS HERE, and that is worth stating because the Apple
// half does convert. Android already reports what the facade promises:
// m/s^2, rad/s, microtesla and hectopascal. Apple reports acceleration in G,
// so the conversion lives there and this file stays honest about doing none.

import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.SystemClock;

public final class CplusSensors implements SensorEventListener {

    private final long token;
    private SensorManager manager;
    private Sensor sensor;
    private boolean running;

    public CplusSensors(long token) { this.token = token; }

    private static int androidType(int kind) {
        if (kind == 1) { return Sensor.TYPE_GYROSCOPE; }
        if (kind == 2) { return Sensor.TYPE_MAGNETIC_FIELD; }
        if (kind == 3) { return Sensor.TYPE_PRESSURE; }
        return Sensor.TYPE_ACCELEROMETER;
    }

    public static boolean available(Context ctx, int kind) {
        try {
            SensorManager m = (SensorManager) ctx.getSystemService(Context.SENSOR_SERVICE);
            if (m == null) { return false; }
            return m.getDefaultSensor(androidType(kind)) != null;
        } catch (Throwable t) {
            return false;
        }
    }

    // 0 started, 1 unsupported, 2 unavailable, 4 failed.
    public int start(Context ctx, int kind, long intervalMs) {
        try {
            manager = (SensorManager) ctx.getSystemService(Context.SENSOR_SERVICE);
            if (manager == null) { return 1; }
            sensor = manager.getDefaultSensor(androidType(kind));
            if (sensor == null) { return 2; }

            // MICROSECONDS, and a HINT — the framework says so explicitly and
            // may deliver faster or slower. 0 asks for SENSOR_DELAY_NORMAL
            // rather than the fastest rate, which would pin a core.
            int period = (intervalMs > 0)
                ? (int) Math.min(intervalMs * 1000L, (long) Integer.MAX_VALUE)
                : SensorManager.SENSOR_DELAY_NORMAL;

            running = manager.registerListener(this, sensor, period);
            return running ? 0 : 4;
        } catch (Throwable t) {
            android.util.Log.w("CplusSensors", "start failed", t);
            return 4;
        }
    }

    public void stop() {
        running = false;
        try { if (manager != null) { manager.unregisterListener(this); } } catch (Throwable ignored) { }
    }

    public boolean isRunning() { return running; }

    @Override public void onSensorChanged(SensorEvent e) {
        if (!running) { return; }
        float[] v = e.values;
        double x = v.length > 0 ? v[0] : 0.0;
        double y = v.length > 1 ? v[1] : 0.0;
        double z = v.length > 2 ? v[2] : 0.0;
        nativeSample(token, x, y, z, unixMillis(e.timestamp));
    }

    // SensorEvent.timestamp is NANOSECONDS SINCE BOOT, not a wall clock — the
    // one thing about this API that silently produces nonsense if taken at face
    // value. A reading timestamped 4,000,000 would be January 1970.
    //
    // Converted against the same clock it came from, so the result stays
    // anchored to when the sample was TAKEN rather than when it was delivered.
    private static long unixMillis(long eventNanos) {
        long sinceBootNanos = SystemClock.elapsedRealtimeNanos();
        long ageMillis = (sinceBootNanos - eventNanos) / 1000000L;
        return System.currentTimeMillis() - ageMillis;
    }

    @Override public void onAccuracyChanged(Sensor s, int accuracy) { }

    private static native void nativeSample(long token, double x, double y,
                                            double z, long timeMs);
}
