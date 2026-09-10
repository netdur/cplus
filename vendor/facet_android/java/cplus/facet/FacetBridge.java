package cplus.facet;

// THE PAGE TALKING BACK. `window.facet.postMessage("...")` from JavaScript
// arrives here and goes straight across.
//
// `addJavascriptInterface` is the only channel a WebView has in this direction,
// and `@JavascriptInterface` is not decoration: without the annotation the
// method is invisible to the page on every API level this backend targets, and
// the call fails silently in the JavaScript console where nobody looks.
//
// The method runs on the WebView's own JavaScript thread, NOT the UI thread —
// which is why it posts rather than calling native directly. facet's tree is
// the UI thread's, and a handler that ran here would be writing props from the
// wrong one.
public final class FacetBridge {
    private final long token;
    private final android.view.View host;

    public FacetBridge(long token, android.view.View host) {
        this.token = token;
        this.host = host;
    }

    @android.webkit.JavascriptInterface
    public void postMessage(final String body) {
        host.post(new Runnable() {
            @Override public void run() { nativeRawMessage(token, body); }
        });
    }

    private static native void nativeRawMessage(long token, String body);
}
