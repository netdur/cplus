package cplus.facet;

// The date picker's JVM half: a DIALOG, not an embedded widget.
//
// Android's `DatePicker` is a full calendar — hundreds of points tall, with a
// mode fixed by an XML attribute there is no setter for. UIDatePicker's compact
// posture, which is what an app asking for a 44-point picker means, is a FIELD
// that opens a picker; on Android that field is a Button and the picker is
// DatePickerDialog. So the control is a button and this is the dialog.
//
// The formatting is here too, because a date's TEXT is a locale question and
// `SimpleDateFormat` reads the same LDML patterns `format` is written in.
public final class FacetDate implements android.app.DatePickerDialog.OnDateSetListener,
                                        android.content.DialogInterface.OnDismissListener {
    private final long token;

    private FacetDate(long token) { this.token = token; }

    // A zero year in the bounds means "not asked for", which is the reading
    // every numeric zero gets in facet's contract.
    public static void show(android.content.Context c, long token, int y, int m, int d,
                            int minY, int minM, int minD, int maxY, int maxM, int maxD) {
        FacetDate f = new FacetDate(token);
        android.app.DatePickerDialog dlg =
            new android.app.DatePickerDialog(c, f, y, m - 1, d);
        if (minY > 0) dlg.getDatePicker().setMinDate(millis(minY, minM, minD));
        if (maxY > 0) dlg.getDatePicker().setMaxDate(millis(maxY, maxM, maxD));
        dlg.setOnDismissListener(f);
        dlg.show();
        nativeOpened(token);
    }

    public static String format(String pattern, int y, int m, int d) {
        java.util.Calendar c = java.util.Calendar.getInstance();
        c.clear();
        c.set(y, m - 1, d);
        java.text.DateFormat f;
        if (pattern == null || pattern.length() == 0) {
            f = java.text.DateFormat.getDateInstance(java.text.DateFormat.MEDIUM);
        } else {
            try {
                f = new java.text.SimpleDateFormat(pattern, java.util.Locale.getDefault());
            } catch (IllegalArgumentException e) {
                f = java.text.DateFormat.getDateInstance(java.text.DateFormat.MEDIUM);
            }
        }
        return f.format(c.getTime());
    }

    private static long millis(int y, int m, int d) {
        java.util.Calendar c = java.util.Calendar.getInstance();
        c.clear();
        c.set(y, m - 1, d);
        return c.getTimeInMillis();
    }

    // A MONTH IS ONE-BASED on the way out, because it is one-based in facet's
    // `Date` and zero-based in java.util.Calendar. Converting at the boundary
    // is the whole of it; converting anywhere else is a bug that shows up in
    // January.
    @Override public void onDateSet(android.widget.DatePicker v, int y, int m, int d) {
        nativeDateSet(token, y, m + 1, d);
    }

    @Override public void onDismiss(android.content.DialogInterface di) {
        nativeDismissed(token);
    }

    private static native void nativeDateSet(long token, int y, int m, int d);
    private static native void nativeOpened(long token);
    private static native void nativeDismissed(long token);
}
