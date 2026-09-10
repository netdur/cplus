package cplus.facet;

// The time picker's JVM half, which is the date picker's one field over: a
// dialog, opened from a field, because Android's embedded `TimePicker` is a
// full clock face and the compact posture an app asks for is a chip.
//
// The formatting is here for the same reason it is in FacetDate: a time's TEXT
// is a locale question, and `SimpleDateFormat` reads the same LDML patterns
// `format` is written in.
public final class FacetTime implements android.app.TimePickerDialog.OnTimeSetListener,
                                        android.content.DialogInterface.OnDismissListener {
    private final long token;

    private FacetTime(long token) { this.token = token; }

    public static void show(android.content.Context c, long token, int hour, int minute) {
        FacetTime f = new FacetTime(token);
        android.app.TimePickerDialog dlg = new android.app.TimePickerDialog(
            c, f, hour, minute, android.text.format.DateFormat.is24HourFormat(c));
        dlg.setOnDismissListener(f);
        dlg.show();
        nativeOpened(token);
    }

    public static String format(String pattern, int hour, int minute) {
        java.util.Calendar c = java.util.Calendar.getInstance();
        c.clear();
        c.set(java.util.Calendar.HOUR_OF_DAY, hour);
        c.set(java.util.Calendar.MINUTE, minute);
        java.text.DateFormat f;
        if (pattern == null || pattern.length() == 0) {
            f = java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT);
        } else {
            try {
                f = new java.text.SimpleDateFormat(pattern, java.util.Locale.getDefault());
            } catch (IllegalArgumentException e) {
                f = java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT);
            }
        }
        return f.format(c.getTime());
    }

    @Override public void onTimeSet(android.widget.TimePicker v, int hour, int minute) {
        nativeTimeSet(token, hour, minute);
    }

    @Override public void onDismiss(android.content.DialogInterface di) {
        nativeTimeDismissed(token);
    }

    private static native void nativeTimeSet(long token, int hour, int minute);
    private static native void nativeOpened(long token);
    private static native void nativeTimeDismissed(long token);
}
