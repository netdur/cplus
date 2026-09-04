package cplus.biometrics;

// The Java half of `vendor/biometrics` on Android.
//
// `BiometricPrompt.AuthenticationCallback` is an ABSTRACT CLASS, which JNI can
// no more subclass than it can implement an interface — same reason
// `CplusCamera` and `CplusLocation` exist.
//
// FRAMEWORK BiometricPrompt, not androidx.biometric. The AndroidX one is
// better — it back-ports to API 23 and draws its own dialog on old versions —
// and it arrives as an AAR with a transitive closure the AAR measurement priced
// in megabytes of dex. The framework class is API 28; below that this package
// answers Unsupported, which is a smaller lie than shipping a fragment library
// to authenticate a fingerprint.

import android.app.Activity;
import android.content.Context;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.Build;
import android.os.CancellationSignal;

public final class CplusBiometrics {

    private final long token;
    private CancellationSignal cancel;

    public CplusBiometrics(long token) { this.token = token; }

    // 0 none, 1 fingerprint, 2 face, 3 iris.
    //
    // ANDROID WILL NOT SAY WHICH. `BiometricManager` answers whether SOMETHING
    // strong is enrolled and never what it is — there is no equivalent of
    // Apple's `biometryType`. Reporting "fingerprint" would be a guess that is
    // wrong on every face-unlock phone, so this answers 1 as "something", and
    // the guide says so.
    public static int kind(Context ctx) {
        if (Build.VERSION.SDK_INT < 29) { return 0; }
        try {
            BiometricManager m = ctx.getSystemService(BiometricManager.class);
            if (m == null) { return 0; }
            if (m.canAuthenticate() == BiometricManager.BIOMETRIC_SUCCESS) { return 1; }
        } catch (Throwable ignored) { }
        return 0;
    }

    // 0 ok, 1 unsupported, 2 unavailable, 3 not enrolled.
    public static int status(Context ctx) {
        if (Build.VERSION.SDK_INT < 29) { return 1; }
        try {
            BiometricManager m = ctx.getSystemService(BiometricManager.class);
            if (m == null) { return 1; }
            int r = m.canAuthenticate();
            if (r == BiometricManager.BIOMETRIC_SUCCESS) { return 0; }
            if (r == BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED) { return 3; }
            return 2;
        } catch (Throwable ignored) {
            return 2;
        }
    }

    // 0 prompt raised, 1 unsupported, 2 unavailable, 7 failed.
    public int authenticate(Activity activity, String reason, boolean allowPasscode) {
        if (Build.VERSION.SDK_INT < 28) { return 1; }
        if (activity == null) { return 2; }
        try {
            BiometricPrompt.Builder b = new BiometricPrompt.Builder(activity)
                .setTitle(reason);

            // A NEGATIVE BUTTON IS MANDATORY unless device credentials are
            // allowed — the Builder throws without one. The two are mutually
            // exclusive, which is why this is an if/else rather than both.
            if (allowPasscode && Build.VERSION.SDK_INT >= 30) {
                b.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG
                                         | BiometricManager.Authenticators.DEVICE_CREDENTIAL);
            } else {
                b.setNegativeButton("Cancel", activity.getMainExecutor(),
                    (dialog, which) -> nativeResult(token, 5));
            }

            cancel = new CancellationSignal();
            b.build().authenticate(cancel, activity.getMainExecutor(),
                new BiometricPrompt.AuthenticationCallback() {
                    @Override public void onAuthenticationSucceeded(
                            BiometricPrompt.AuthenticationResult r) {
                        nativeResult(token, 0);
                    }
                    // FAILED is one wrong finger, not the end of the attempt —
                    // the prompt stays up and the person tries again. Reporting
                    // it would fire the handler several times for one ask.
                    @Override public void onAuthenticationFailed() { }
                    @Override public void onAuthenticationError(int code, CharSequence msg) {
                        nativeResult(token, mapError(code));
                    }
                });
            return 0;
        } catch (Throwable t) {
            android.util.Log.w("CplusBiometrics", "authenticate failed", t);
            return 7;
        }
    }

    private static int mapError(int code) {
        switch (code) {
            case BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED:
            case BiometricPrompt.BIOMETRIC_ERROR_CANCELED:
                // The negative button is NOT here: that constant is androidx's,
                // not the framework's, and the button's own listener already
                // reports cancellation directly.
                return 5;   // Cancelled
            case BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT:
            case BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT_PERMANENT:
                return 6;   // LockedOut
            case BiometricPrompt.BIOMETRIC_ERROR_NO_BIOMETRICS:
                return 3;   // NotEnrolled
            case BiometricPrompt.BIOMETRIC_ERROR_HW_NOT_PRESENT:
            case BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE:
                return 2;   // Unavailable
            default:
                return 7;   // Failed
        }
    }

    public void cancel() {
        try { if (cancel != null) { cancel.cancel(); } } catch (Throwable ignored) { }
    }

    private static native void nativeResult(long token, int outcome);
}
