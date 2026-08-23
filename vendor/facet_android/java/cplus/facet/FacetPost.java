package cplus.facet;

// `services::after` — a delayed callback. Holds the token facet handed out so a
// pending timer can be cancelled by identity.
public final class FacetPost implements Runnable {
    private final long token;
    public FacetPost(long token) { this.token = token; }
    @Override public void run() { nativeAfter(token); }
    private static native void nativeAfter(long token);
}
