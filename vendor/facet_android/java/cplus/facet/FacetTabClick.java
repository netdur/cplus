package cplus.facet;

// A TAB WAS PICKED. Its own listener because a tab click carries TWO things —
// the node whose `selected_index` to write, and which tab was hit — and a
// listener is free to hold both.
//
// The first attempt packed them into the one long `FacetClick` already carries:
// a node address shifted left with the index in the low byte, on the reasoning
// that an ordinary address shifted back right could not be a node. It cannot be
// a VALID node, which is not the same thing — it is a garbage POINTER, and the
// kind check dereferenced it. Every button in the application crashed on its
// first click. Two fields cost less than that arithmetic did.
public final class FacetTabClick implements android.view.View.OnClickListener {
    private final long token;
    private final int index;

    public FacetTabClick(long token, int index) { this.token = token; this.index = index; }

    @Override public void onClick(android.view.View v) { nativeTab(token, index); }

    private static native void nativeTab(long token, int index);
}
