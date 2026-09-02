package cplus.camera;

import android.content.Context;
import android.graphics.SurfaceTexture;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.Image;
import android.media.ImageReader;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.util.Size;
import android.view.Surface;
import android.view.TextureView;

import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.List;

// The camera2 dance, in Java, because camera2 cannot be driven from JNI.
//
// `CameraDevice.StateCallback`, `CameraCaptureSession.StateCallback` and
// `CameraCaptureSession.CaptureCallback` are ABSTRACT CLASSES, not interfaces.
// `java.lang.reflect.Proxy` implements interfaces only, so there is no way to
// answer them from native code — they need real subclasses, and real subclasses
// need a dex. That is the whole reason this file exists rather than a hundred
// more JNI calls.
//
// Given that a dex was unavoidable, the seam is drawn where it is cheapest:
// this class owns the entire pipeline and native code sees five methods. The
// alternative — exposing camera2 verb by verb — would have been the same dex
// plus a JNI call per step.
//
// ONE BACKGROUND THREAD per instance. camera2 delivers on the Handler you give
// it, and giving it the main looper means an open or a capture can stall the UI
// thread on hardware that is slow to respond.
public final class CplusCamera {

    private final long token;

    private HandlerThread thread;
    private Handler handler;

    private CameraDevice device;
    private CameraCaptureSession session;
    private ImageReader reader;
    private ImageReader frames;
    private boolean streaming;
    private int wantW, wantH;
    private int frameW = 0, frameH = 0;
    private int actualFacing = 0;
    private TextureView view;
    // The sensor's mounting angle, degrees clockwise from the device's natural
    // orientation. Read from CameraCharacteristics, NOT assumed: the usual
    // value is 90 for a back lens and 270 for a front one, but it is a
    // per-device fact and assuming it is how preview transforms go wrong on
    // exactly the handsets nobody tested on.
    private int sensorOrientation = 90;
    private Surface previewSurface;
    private boolean wantsPreview;
    private boolean closed;
    // Why the last `open` failed, in `camera::Outcome` codes. Without this the
    // native side sees only "false" and has to GUESS — and it guessed `Busy`
    // for a permission denial, which tells the user another app has the camera
    // when what they need is to grant access.
    private int lastError = 0;

    public CplusCamera(long token) { this.token = token; }

    // ---- enumeration, which needs no permission ----------------------------

    public static int count(Context ctx) {
        try {
            CameraManager m = (CameraManager) ctx.getSystemService(Context.CAMERA_SERVICE);
            return m.getCameraIdList().length;
        } catch (Throwable t) {
            return 0;
        }
    }

    public static boolean has(Context ctx, boolean front) {
        return idFor(ctx, front) != null;
    }

    // The first camera facing this way.
    //
    // Falls back to the FIRST camera when nothing reports the wanted facing —
    // the same judgement the Apple half makes for a desktop camera that reports
    // no position. An external USB camera on Android reports EXTERNAL, and
    // refusing to open it because it is neither front nor back would make the
    // package useless on exactly the devices that have one.
    private static String idFor(Context ctx, boolean front) {
        try {
            CameraManager m = (CameraManager) ctx.getSystemService(Context.CAMERA_SERVICE);
            String[] ids = m.getCameraIdList();
            int want = front ? CameraCharacteristics.LENS_FACING_FRONT
                             : CameraCharacteristics.LENS_FACING_BACK;
            for (String id : ids) {
                Integer f = m.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING);
                if (f != null && f == want) { return id; }
            }
            return ids.length > 0 ? ids[0] : null;
        } catch (Throwable t) {
            return null;
        }
    }

    // ---- open / close ------------------------------------------------------

    // Returns whether the OPEN WAS REQUESTED, not whether the camera is ready:
    // camera2 opens asynchronously and there is nothing useful to block on here.
    // A capture before the device is ready answers false rather than waiting.
    public int lastError() { return lastError; }

    public int actualFacing() { return actualFacing; }
    public int frameWidth() { return frameW; }
    public int frameHeight() { return frameH; }

    public boolean open(Context ctx, boolean front, int wantWidth, int wantHeight) {
        lastError = 0;
        // CLEAR THE LATCH `close()` SET, and this object is reopened far more
        // often than it looks. `closed` was write-once: nothing ever set it
        // back, so the SECOND open on the same object requested the device
        // successfully, got `onOpened`, and then `tryStart` returned at its
        // first line because the flag still said closed. A camera that opened
        // with no preview and no frames, reporting success.
        //
        // Android backgrounds and foregrounds an app many times over one run
        // and revokes the camera each time, so "open, close, open again on the
        // same object" is the NORMAL path on this platform, not a corner.
        closed = false;
        String id = idFor(ctx, front);
        // 2 = Unsupported: no lens faces that way and there is no fallback.
        if (id == null) { lastError = 2; return false; }
        wantW = wantWidth;
        wantH = wantHeight;
        // WHAT WE ACTUALLY GOT. `idFor` falls back to the first camera when
        // nothing faces the requested way, so this is not the request echoed
        // back — an EXTERNAL camera reports neither front nor back and lands
        // here as back, which is the more useful lie of the two.
        try {
            CameraManager m0 = (CameraManager) ctx.getSystemService(Context.CAMERA_SERVICE);
            CameraCharacteristics ch0 = m0.getCameraCharacteristics(id);
            Integer f = ch0.get(CameraCharacteristics.LENS_FACING);
            actualFacing = (f != null && f == CameraCharacteristics.LENS_FACING_FRONT) ? 1 : 0;
            Integer so0 = ch0.get(CameraCharacteristics.SENSOR_ORIENTATION);
            if (so0 != null) { sensorOrientation = so0.intValue(); }
            pickFrameSize(m0, id);
        } catch (Throwable ignored) { }
        try {
            thread = new HandlerThread("cplus-camera");
            thread.start();
            handler = new Handler(thread.getLooper());

            // 1920x1080 JPEG, and maxImages 2 rather than 1: the reader must be
            // able to hold the next frame while the previous Image is still
            // being copied out, or a capture during delivery is dropped.
            reader = ImageReader.newInstance(1920, 1080, android.graphics.ImageFormat.JPEG, 2);
            reader.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
                @Override public void onImageAvailable(ImageReader r) { deliver(r); }
            }, handler);

            CameraManager m = (CameraManager) ctx.getSystemService(Context.CAMERA_SERVICE);
            m.openCamera(id, new CameraDevice.StateCallback() {
                @Override public void onOpened(CameraDevice d) { device = d; tryStart(); }
                @Override public void onDisconnected(CameraDevice d) { d.close(); device = null; }
                @Override public void onError(CameraDevice d, int e) { d.close(); device = null; }
            }, handler);
            return true;
        } catch (SecurityException se) {
            // CAMERA is not granted. This is the ONLY signal Android gives —
            // `openCamera` throws rather than prompting, so a caller that has
            // not asked for the permission first arrives here.
            //
            // 1 = Denied. Reporting it is the whole point of `lastError`.
            lastError = 1;
            close();
            return false;
        } catch (Throwable t) {
            // 4 = Failed: anything else.
            //
            // LOGGED, not just counted. `lastError` gives the C+ side a code it
            // can act on, but a code cannot say WHICH throwable — and the one
            // that actually turns up here (the device is still shutting down
            // from the previous close) is indistinguishable from a dozen others
            // until the message is in the log.
            android.util.Log.w("CplusCamera", "open failed", t);
            lastError = 4;
            close();
            return false;
        }
    }

    // SWITCH THE LENS WITHOUT REPLACING THE OBJECT.
    //
    // camera2 cannot re-point a `CameraDevice`: the only way to the other lens
    // is to close this one and open the other id. But the native side holds a
    // handle to THIS object, and the facet preview node captured that handle
    // as its factory context — so tearing the object down would leave a mounted
    // preview pointing at a dead session.
    //
    // So everything except the device and the capture session SURVIVES: the
    // handler thread, the readers, the TextureView and its SurfaceTexture.
    // `tryStart` then rebuilds the session against those same surfaces, and the
    // preview never notices.
    public boolean switchTo(Context ctx, boolean front) {
        if (closed) { return false; }
        String id = idFor(ctx, front);
        if (id == null) { return false; }
        try {
            CameraManager m = (CameraManager) ctx.getSystemService(Context.CAMERA_SERVICE);

            // Down: session first, then device. The other order leaves the
            // session holding a closed device.
            try { if (session != null) { session.close(); } } catch (Throwable ignored) { }
            session = null;
            try { if (device != null) { device.close(); } } catch (Throwable ignored) { }
            device = null;

            CameraCharacteristics ch = m.getCameraCharacteristics(id);
            Integer f = ch.get(CameraCharacteristics.LENS_FACING);
            actualFacing = (f != null && f == CameraCharacteristics.LENS_FACING_FRONT) ? 1 : 0;
            Integer so = ch.get(CameraCharacteristics.SENSOR_ORIENTATION);
            if (so != null) { sensorOrientation = so.intValue(); }

            // The new lens may not publish the size the old one did. Only then
            // is the reader rebuilt — an ImageReader is a surface the session
            // is configured against, so replacing it needlessly costs a second
            // session rebuild.
            int oldW = frameW, oldH = frameH;
            pickFrameSize(m, id);
            if (frames != null && (frameW != oldW || frameH != oldH)) {
                try { frames.close(); } catch (Throwable ignored) { }
                frames = null;
            }
            if (streaming && frames == null) {
                frames = ImageReader.newInstance(frameW, frameH,
                    android.graphics.ImageFormat.YUV_420_888, 2);
                frames.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
                    @Override public void onImageAvailable(ImageReader r) { deliverFrame(r); }
                }, handler);
            }

            m.openCamera(id, new CameraDevice.StateCallback() {
                @Override public void onOpened(CameraDevice d) { device = d; tryStart(); }
                @Override public void onDisconnected(CameraDevice d) { d.close(); device = null; }
                @Override public void onError(CameraDevice d, int e) { d.close(); device = null; }
            }, handler);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    public void close() {
        closed = true;
        try { if (session != null) { session.close(); } } catch (Throwable ignored) { }
        session = null;
        try { if (device != null) { device.close(); } } catch (Throwable ignored) { }
        device = null;
        try { if (reader != null) { reader.close(); } } catch (Throwable ignored) { }
        reader = null;
        streaming = false;
        try { if (frames != null) { frames.close(); } } catch (Throwable ignored) { }
        frames = null;
        previewSurface = null;
        view = null;
        // QUIT SAFELY, not quit(): a capture already queued on this looper is
        // holding an Image, and dropping it on the floor leaks the buffer for
        // the lifetime of the process.
        try { if (thread != null) { thread.quitSafely(); } } catch (Throwable ignored) { }
        thread = null;
        handler = null;
    }

    // ---- preview -----------------------------------------------------------

    // A TextureView rather than a SurfaceView: it is an ordinary view in the
    // hierarchy, so facet's layout applies to it like anything else. A
    // SurfaceView punches a hole through the window and does not compose.
    public android.view.View preview(Context ctx) {
        wantsPreview = true;
        view = new TextureView(ctx);
        view.setSurfaceTextureListener(new TextureView.SurfaceTextureListener() {
            @Override public void onSurfaceTextureAvailable(SurfaceTexture st, int w, int h) {
                st.setDefaultBufferSize(PREVIEW_W, PREVIEW_H);
                previewSurface = new Surface(st);
                configureTransform(w, h);
                tryStart();
            }
            // WHERE ROTATION LANDS. The activity declares configChanges for
            // orientation and screenSize, so a rotation does not recreate it —
            // the tree is re-laid-out and this fires with the new size. It was
            // EMPTY, which is the whole bug: the view turned and the buffer
            // did not.
            @Override public void onSurfaceTextureSizeChanged(SurfaceTexture st, int w, int h) {
                configureTransform(w, h);
            }
            @Override public boolean onSurfaceTextureDestroyed(SurfaceTexture st) {
                previewSurface = null;
                return true;
            }
            @Override public void onSurfaceTextureUpdated(SurfaceTexture st) { }
        });
        return view;
    }

    // The preview buffer's own size, and the reason the transform needs it: the
    // camera fills this, the view is whatever facet's layout says, and the
    // matrix maps one onto the other.
    private static final int PREVIEW_W = 1920;
    private static final int PREVIEW_H = 1080;

    // The display's rotation as a Surface.ROTATION_* constant.
    //
    // Through the VIEW's display, not the Activity's WindowManager: on a
    // foldable the view can be on either screen, and `getDefaultDisplay` is
    // both deprecated and wrong there — it answers for the default display
    // whichever one this view is actually on.
    private int displayRotation() {
        try {
            android.view.Display d = (view != null) ? view.getDisplay() : null;
            if (d != null) { return d.getRotation(); }
        } catch (Throwable ignored) { }
        return Surface.ROTATION_0;
    }

    // MAP THE SENSOR-ORIENTED BUFFER ONTO A VIEW THAT HAS ROTATED.
    //
    // camera2 delivers into the SurfaceTexture in the sensor's own
    // orientation and a TextureView does not correct it — the caller is
    // required to. Nothing did, so the facet tree rotated (it is an ordinary
    // view hierarchy) while the image inside it did not, leaving the preview
    // 90 degrees out and stretched in landscape. Measured on a Fold,
    // 2026-09-02.
    //
    // At ROTATION_0 the identity matrix is correct, which is exactly why the
    // bug was invisible until someone turned the phone.
    //
    // The two-step for the quarter turns: FIT the view rect onto the swapped
    // buffer rect, scale back up so the rotated image still covers the view,
    // then rotate. Doing it in the other order scales along the wrong axis.
    private void configureTransform(int viewW, int viewH) {
        if (view == null || viewW == 0 || viewH == 0) { return; }
        try {
            int rotation = displayRotation();
            android.graphics.Matrix matrix = new android.graphics.Matrix();
            android.graphics.RectF viewRect = new android.graphics.RectF(0, 0, viewW, viewH);
            float cx = viewRect.centerX();
            float cy = viewRect.centerY();

            if (rotation == Surface.ROTATION_90 || rotation == Surface.ROTATION_270) {
                android.graphics.RectF bufferRect =
                    new android.graphics.RectF(0, 0, PREVIEW_H, PREVIEW_W);
                bufferRect.offset(cx - bufferRect.centerX(), cy - bufferRect.centerY());
                matrix.setRectToRect(viewRect, bufferRect,
                                     android.graphics.Matrix.ScaleToFit.FILL);
                float scale = Math.max((float) viewH / PREVIEW_H, (float) viewW / PREVIEW_W);
                matrix.postScale(scale, scale, cx, cy);
                matrix.postRotate(90 * (rotation - 2), cx, cy);
            } else if (rotation == Surface.ROTATION_180) {
                matrix.postRotate(180, cx, cy);
            }
            view.setTransform(matrix);

            // LOGGED because this is a per-device fact being applied to a
            // per-device buffer, and the only way to tell a right transform
            // from a wrong one is to see both numbers next to what the screen
            // actually shows.
            android.util.Log.i("CplusCamera", "transform: view=" + viewW + "x" + viewH
                + " rotation=" + (90 * rotation) + " sensor=" + sensorOrientation
                + " facing=" + (actualFacing == 1 ? "front" : "back"));
        } catch (Throwable t) {
            android.util.Log.w("CplusCamera", "configureTransform failed", t);
        }
    }

    // Build the capture session once we have a device, and rebuild it when the
    // preview surface arrives later. Both orders happen: `open` then `preview`
    // is the ordinary one, but a view can be mounted before the camera opens.
    private void tryStart() {
        if (closed || device == null || reader == null) { return; }
        if (wantsPreview && previewSurface == null) { return; }
        try {
            List<Surface> targets = new ArrayList<Surface>();
            if (previewSurface != null) { targets.add(previewSurface); }
            targets.add(reader.getSurface());
            if (frames != null) { targets.add(frames.getSurface()); }

            device.createCaptureSession(targets, new CameraCaptureSession.StateCallback() {
                @Override public void onConfigured(CameraCaptureSession s) {
                    session = s;
                    repeat();
                }
                @Override public void onConfigureFailed(CameraCaptureSession s) { session = null; }
            }, handler);
        } catch (Throwable ignored) { }
    }

    // The repeating request drives BOTH the preview and the frame stream, so it
    // is rebuilt whenever either changes. Splitting them into two repeating
    // requests is not an option: a session has one, and the last one set wins.
    private void repeat() {
        if (session == null || device == null) { return; }
        if (previewSurface == null && !streaming) { return; }
        try {
            CaptureRequest.Builder b =
                device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
            if (previewSurface != null) { b.addTarget(previewSurface); }
            if (streaming && frames != null) { b.addTarget(frames.getSurface()); }
            session.setRepeatingRequest(b.build(), null, handler);
        } catch (Throwable ignored) { }
    }

    // NEGOTIATE the frame size against what the device actually publishes.
    //
    // camera2 does NOT accept an arbitrary size: an ImageReader created at one
    // the device does not support is rejected when the session is configured,
    // and the failure arrives asynchronously in `onConfigureFailed` with no
    // reason attached. So the request is matched to the nearest supported size
    // by AREA — 640x480 and 800x600 are closer to each other than an
    // aspect-ratio match would suggest, and a caller asking for a size is
    // really saying how much work it wants to do per frame.
    //
    // With no request, the smallest published size at or above 640x480 — a
    // frame handler runs per pixel, and the sensor's native size is the wrong
    // default for that.
    private void pickFrameSize(CameraManager m, String id) {
        frameW = 640; frameH = 480;
        try {
            StreamConfigurationMap map = m.getCameraCharacteristics(id)
                .get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
            if (map == null) { return; }
            Size[] sizes = map.getOutputSizes(android.graphics.ImageFormat.YUV_420_888);
            if (sizes == null || sizes.length == 0) { return; }

            long want = (wantW > 0 && wantH > 0)
                ? (long) wantW * (long) wantH
                : 640L * 480L;
            Size best = null;
            long bestDiff = Long.MAX_VALUE;
            for (Size s : sizes) {
                long area = (long) s.getWidth() * (long) s.getHeight();
                long diff = Math.abs(area - want);
                if (diff < bestDiff) { bestDiff = diff; best = s; }
            }
            if (best != null) { frameW = best.getWidth(); frameH = best.getHeight(); }
        } catch (Throwable ignored) { }
    }

    // ---- frames ------------------------------------------------------------

    // YUV_420_888 because it is the one format camera2 guarantees on every
    // device. Plane 0 is luma with one byte per pixel, which is the same layout
    // AVFoundation's biplanar 420 gives on the other side — so the C+ surface
    // needs no conversion and no per-platform branch.
    //
    // 640x480 rather than the preview's size: a frame handler is doing work per
    // pixel, and 1080p at 30Hz is two million pixels a frame. A caller that
    // wants more can ask for it when this grows a parameter.
    // A STANDING REQUEST, not an immediate one. camera2 opens asynchronously, so
    // a caller that armed frames right after `open` would be refused for a
    // reason it cannot see and cannot wait on. The reader is built now (it needs
    // no device), the flag is set, and `tryStart` picks both up whenever the
    // device actually arrives.
    public boolean startFrames() {
        if (closed) { return false; }
        try {
            if (frames == null) {
                frames = ImageReader.newInstance(frameW, frameH, android.graphics.ImageFormat.YUV_420_888, 2);
                frames.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
                    @Override public void onImageAvailable(ImageReader r) { deliverFrame(r); }
                }, handler);
                streaming = true;
                // The reader is a new session target, so the session has to be
                // rebuilt — `tryStart` picks the new surface up, and no-ops
                // until there is a device to rebuild against.
                tryStart();
                return true;
            }
            streaming = true;
            repeat();
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    public boolean stopFrames() {
        streaming = false;
        repeat();
        return true;
    }

    // On the background handler, and it STAYS there — a frame handler that hops
    // to the main thread at 30Hz spends the UI thread on work the UI is not
    // waiting for. This is the opposite of the still-capture path, deliberately.
    private void deliverFrame(ImageReader r) {
        Image img = null;
        try {
            img = r.acquireLatestImage();
            if (img == null) { return; }
            Image.Plane y = img.getPlanes()[0];
            nativeFrame(token, y.getBuffer(), img.getWidth(), img.getHeight(), y.getRowStride());
        } catch (Throwable ignored) {
        } finally {
            // CLOSE IT. An ImageReader holding maxImages stops delivering,
            // silently — the stream simply stops after two frames.
            if (img != null) { try { img.close(); } catch (Throwable ignored2) { } }
        }
    }

    // ---- capture -----------------------------------------------------------

    public boolean capture() {
        if (closed || session == null || device == null || reader == null) { return false; }
        try {
            CaptureRequest.Builder b = device.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE);
            b.addTarget(reader.getSurface());
            session.capture(b.build(), null, handler);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    // On the background handler. ALWAYS calls back, with an empty array when
    // there is no image: a caller waiting for a shutter needs to know it is not
    // coming, and a callback that sometimes never fires is the worst shape this
    // seam could have.
    //
    // HOPS TO THE MAIN LOOPER before crossing into native, so the package's
    // contract — "the photo arrives on the main thread" — holds on both
    // platforms. Doing it here rather than in C+ costs three lines instead of
    // a second dex class and a second native binding.
    private void deliver(ImageReader r) {
        Image img = null;
        byte[] out = new byte[0];
        try {
            img = r.acquireNextImage();
            if (img != null) {
                ByteBuffer buf = img.getPlanes()[0].getBuffer();
                out = new byte[buf.remaining()];
                buf.get(out);
            }
        } catch (Throwable ignored) {
        } finally {
            // CLOSE THE IMAGE. An ImageReader with maxImages unreleased stops
            // delivering, silently — the second capture simply never arrives.
            if (img != null) { try { img.close(); } catch (Throwable ignored2) { } }
        }
        final byte[] payload = out;
        new Handler(Looper.getMainLooper()).post(new Runnable() {
            @Override public void run() { nativePhoto(token, payload); }
        });
    }

    private static native void nativePhoto(long token, byte[] jpeg);

    // A DIRECT ByteBuffer, so native code reads the plane in place —
    // `GetDirectBufferAddress` on the other side. Copying it here would double
    // the per-frame cost for nothing.
    private static native void nativeFrame(long token, java.nio.ByteBuffer luma,
                                           int width, int height, int rowStride);
}
