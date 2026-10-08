package cplus.facet;

// share::file on Android (spendwise S54): copy the file into the provider's
// folder, then ACTION_SEND it as a content:// URI with a read grant, through a
// chooser. 0 sheet shown, 1 unsupported (the app's manifest does not declare
// FacetFileProvider; logged once with the line to add), 2 bad input (no such
// file), 3 failed.
public final class FacetFiles {
    private static boolean providerWarned = false;

    public static int shareFile(android.app.Activity activity, String path) {
        if (activity == null || path == null) return 3;
        String authority = FacetFileProvider.authority(activity);
        if (activity.getPackageManager().resolveContentProvider(authority, 0) == null) {
            if (!providerWarned) {
                providerWarned = true;
                android.util.Log.w("facet", "share::file needs <provider android:name=\"cplus.facet.FacetFileProvider\" "
                    + "android:authorities=\"" + authority + "\" android:exported=\"false\" "
                    + "android:grantUriPermissions=\"true\"/> inside <application> in AndroidManifest.xml");
            }
            return 1;
        }
        java.io.File src = new java.io.File(path);
        if (!src.isFile()) return 2;
        try {
            java.io.File dir = new java.io.File(activity.getCacheDir(), FacetFileProvider.DIR);
            if (!dir.isDirectory() && !dir.mkdirs()) return 3;
            java.io.File dst = new java.io.File(dir, src.getName());
            if (!src.getCanonicalPath().equals(dst.getCanonicalPath())) {
                try (java.io.InputStream in = new java.io.FileInputStream(src);
                     java.io.OutputStream out = new java.io.FileOutputStream(dst)) {
                    byte[] buf = new byte[8192];
                    int n;
                    while ((n = in.read(buf)) > 0) out.write(buf, 0, n);
                }
            }
            android.net.Uri uri = new android.net.Uri.Builder().scheme("content")
                .authority(authority).appendPath(dst.getName()).build();
            android.content.Intent send = new android.content.Intent(android.content.Intent.ACTION_SEND);
            send.setType(FacetFileProvider.mimeOf(dst.getName()));
            send.putExtra(android.content.Intent.EXTRA_STREAM, uri);
            // The grant has to travel through the chooser to the app picked,
            // which is what the ClipData is for.
            send.setClipData(android.content.ClipData.newRawUri(dst.getName(), uri));
            send.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
            android.content.Intent chooser = android.content.Intent.createChooser(send, null);
            chooser.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
            activity.startActivity(chooser);
            return 0;
        } catch (Throwable t) {
            android.util.Log.w("facet", "share::file failed", t);
            return 3;
        }
    }
}
