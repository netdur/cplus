package cplus.facet;

// The web view's JVM half: the two navigation edges and the crash.
//
// `WebViewClient` is a CLASS, not an interface — it cannot be implemented from
// native code, which is the same wall `BaseAdapter` is and the reason FacetRows
// exists. This is the smallest subclass that reports what facet asks for.
//
// `onRenderProcessGone` returning TRUE is what keeps the app alive when the
// renderer dies: false means "the app should be killed too", and facet's
// `on_process_terminated` exists precisely so the application can decide.
public final class FacetWeb extends android.webkit.WebViewClient {
    private final long token;

    public FacetWeb(long token) { this.token = token; }

    @Override public void onPageStarted(android.webkit.WebView v, String url,
                                        android.graphics.Bitmap favicon) {
        nativeNavigating(token);
    }

    @Override public void onPageFinished(android.webkit.WebView v, String url) {
        nativeNavigated(token, v.canGoBack(), v.canGoForward());
    }

    @Override public boolean onRenderProcessGone(android.webkit.WebView v,
                                                 android.webkit.RenderProcessGoneDetail d) {
        nativeTerminated(token);
        return true;
    }

    // EVERY REQUEST THE PAGE MAKES, which is what facet's
    // `on_web_resource_requested` names. Returning null means "load it the
    // normal way" — this is a REPORT, not an interception, and a backend that
    // started answering resources here would be deciding what a page may load
    // on the application's behalf.
    //
    // It runs on a background thread, so like the bridge it hops to the UI
    // thread before anything touches facet's tree.
    @Override public android.webkit.WebResourceResponse shouldInterceptRequest(
            final android.webkit.WebView v,
            android.webkit.WebResourceRequest request) {
        if (request != null) {
            final String url = request.getUrl().toString();
            v.post(new Runnable() {
                @Override public void run() { nativeResourceRequested(token, url); }
            });
        }
        return null;
    }

    private static native void nativeResourceRequested(long token, String url);
    private static native void nativeNavigating(long token);
    private static native void nativeNavigated(long token, boolean back, boolean forward);
    private static native void nativeTerminated(long token);
}
