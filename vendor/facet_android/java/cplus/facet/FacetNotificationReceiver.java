package cplus.facet;

// A NOTIFICATION'S ACTION BUTTON, ANSWERED WITHOUT OPENING THE APP.
//
// The notification body's tap starts the Activity, which is what a person
// expects: tapping a message opens it. A BUTTON is the opposite — pressing Play
// on a player, or Archive on a mail notification, should do the thing and leave
// the shade where it is. An Activity PendingIntent cannot do that; it always
// brings the app forward and collapses the shade.
//
// So buttons route through here instead. A broadcast runs in the app's process
// without starting an Activity, so the shade stays open and the notification
// stays put.
//
// LIKE FacetActivity, THIS CLASS IS MERGED INTO THE APP'S classes.dex at build
// time and named in its AndroidManifest.xml:
//
//     <receiver android:name="cplus.facet.FacetNotificationReceiver"
//               android:exported="false" />
//
// An app that omits the line loses its action buttons — the PendingIntent
// resolves to nothing — and keeps everything else. That is a better failure
// than a crash, and it is why this is a separate class rather than another
// method on the Activity.
public class FacetNotificationReceiver extends android.content.BroadcastReceiver {

    @Override public void onReceive(android.content.Context context,
                                    android.content.Intent intent) {
        if (intent == null) return;
        String p = intent.getStringExtra(FacetActivity.PAYLOAD_EXTRA);
        if (p == null || p.length() == 0) return;
        try {
            FacetActivity.nativeNotificationTap(p);
        } catch (Throwable t) {
            // THE PROCESS WAS COLD. A broadcast can start it, and the .so is
            // loaded by FacetActivity.onCreate — which has not run. Park the
            // payload; the Activity drains it when it next starts. Dropping it
            // would make a button pressed from a dead app do nothing at all.
            FacetActivity.pendingPayload = p;
        }
    }
}
