package cplus.facet;

// A FILE AN APP HANDS TO ANOTHER APP (spendwise S54). Since API 24 a file://
// URI throws FileUriExposedException, and androidx's FileProvider is an AAR
// facet does not ship. This is the part of it facet needs: read-only access to
// files the app copied into <cacheDir>/cplus-share/, under the authority
// <package>.cplus.files, and nothing else.
//
// The app declares it, as it declares FacetActivity:
//   <provider android:name="cplus.facet.FacetFileProvider"
//             android:authorities="<package>.cplus.files"
//             android:exported="false" android:grantUriPermissions="true"/>
public final class FacetFileProvider extends android.content.ContentProvider {
    static final String DIR = "cplus-share";

    static String authority(android.content.Context ctx) {
        return ctx.getPackageName() + ".cplus.files";
    }

    @Override public boolean onCreate() { return true; }

    // The file a URI names, or null when it names anything outside the share
    // folder: one path segment, no separators, no "..".
    private java.io.File fileFor(android.net.Uri uri) {
        android.content.Context ctx = getContext();
        if (ctx == null) return null;
        java.util.List<String> segs = uri.getPathSegments();
        if (segs == null || segs.size() != 1) return null;
        String name = segs.get(0);
        if (name.isEmpty() || name.equals(".") || name.equals("..") || name.contains("/")) return null;
        java.io.File dir = new java.io.File(ctx.getCacheDir(), DIR);
        java.io.File f = new java.io.File(dir, name);
        try {
            if (!f.getCanonicalFile().getParentFile().equals(dir.getCanonicalFile())) return null;
        } catch (java.io.IOException e) { return null; }
        return f;
    }

    @Override
    public android.database.Cursor query(android.net.Uri uri, String[] projection, String selection,
                                         String[] args, String sort) {
        java.io.File f = fileFor(uri);
        if (f == null || !f.isFile()) return null;
        String[] cols = projection != null ? projection
            : new String[] { android.provider.OpenableColumns.DISPLAY_NAME,
                             android.provider.OpenableColumns.SIZE };
        android.database.MatrixCursor c = new android.database.MatrixCursor(cols, 1);
        Object[] row = new Object[cols.length];
        for (int i = 0; i < cols.length; i++) {
            if (android.provider.OpenableColumns.DISPLAY_NAME.equals(cols[i])) row[i] = f.getName();
            else if (android.provider.OpenableColumns.SIZE.equals(cols[i])) row[i] = f.length();
        }
        c.addRow(row);
        return c;
    }

    @Override
    public String getType(android.net.Uri uri) {
        java.io.File f = fileFor(uri);
        return f == null ? null : mimeOf(f.getName());
    }

    static String mimeOf(String name) {
        int dot = name.lastIndexOf('.');
        if (dot >= 0) {
            String ext = name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT);
            String t = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext);
            if (t != null) return t;
        }
        return "application/octet-stream";
    }

    @Override
    public android.os.ParcelFileDescriptor openFile(android.net.Uri uri, String mode)
            throws java.io.FileNotFoundException {
        if (mode == null || !mode.equals("r")) throw new java.io.FileNotFoundException("read-only");
        java.io.File f = fileFor(uri);
        if (f == null || !f.isFile()) throw new java.io.FileNotFoundException(uri.toString());
        return android.os.ParcelFileDescriptor.open(f, android.os.ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override public android.net.Uri insert(android.net.Uri uri, android.content.ContentValues v) { return null; }
    @Override public int delete(android.net.Uri uri, String s, String[] a) { return 0; }
    @Override public int update(android.net.Uri uri, android.content.ContentValues v, String s, String[] a) { return 0; }
}
