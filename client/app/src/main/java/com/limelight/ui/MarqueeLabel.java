package com.limelight.ui;

import android.content.Context;
import android.text.TextUtils;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewParent;
import android.view.ViewTreeObserver;
import android.widget.AdapterView;

import androidx.appcompat.widget.AppCompatTextView;

/**
 * A tile's one-line label: cut short with an ellipsis, and scrolling sideways while the tile it is on has focus, so a
 * long name or role can be read without the label growing over the art. It stays where the layout put it. With Reduce
 * motion it keeps the ellipsis.
 */
public class MarqueeLabel extends AppCompatTextView implements ViewTreeObserver.OnGlobalFocusChangeListener {
    private ViewTreeObserver observer;

    public MarqueeLabel(Context context) {
        this(context, null);
    }

    public MarqueeLabel(Context context, AttributeSet attrs) {
        this(context, attrs, android.R.attr.textViewStyle);
    }

    public MarqueeLabel(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
        setSingleLine(true);
        setEllipsize(TextUtils.TruncateAt.END);
        setMarqueeRepeatLimit(-1);
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        observer = getViewTreeObserver();
        observer.addOnGlobalFocusChangeListener(this);
        follow(getRootView().findFocus());
    }

    @Override
    protected void onDetachedFromWindow() {
        if (observer != null && observer.isAlive()) observer.removeOnGlobalFocusChangeListener(this);
        observer = null;
        follow(null);
        super.onDetachedFromWindow();
    }

    @Override
    public void onGlobalFocusChanged(View oldFocus, View newFocus) {
        follow(newFocus);
    }

    private void follow(View focused) {
        boolean scroll = within(this, focused) && !Motion.reduced(getContext());
        // A selected label scrolls without focus of its own; the tile keeps the focus.
        boolean selected = scroll && (!(focused instanceof AdapterView) || tileOf(this, focused).isSelected());
        TextUtils.TruncateAt wanted = selected ? TextUtils.TruncateAt.MARQUEE : TextUtils.TruncateAt.END;
        if (getEllipsize() != wanted) setEllipsize(wanted);
        if (isSelected() != selected) setSelected(selected);
    }

    /**
     * A label set to scroll is cut off without an ellipsis, so only the one scrolling is. A grid moves the selected
     * state here as the D-pad moves, with no focus change to follow, and before it attaches the tile again, so the focus
     * can't be traced from here; a grid only selects a tile while it has the focus or the tile is pressed.
     */
    @Override
    public void setSelected(boolean selected) {
        if (selected && !Motion.reduced(getContext())) setEllipsize(TextUtils.TruncateAt.MARQUEE);
        super.setSelected(selected);
        if (!selected) setEllipsize(TextUtils.TruncateAt.END);
    }

    /**
     * True when {@code focused} is the label or holds it. A grid holds the focus for all its tiles and only selects one, so
     * there the selected state decides (nightjar#7: every tile in view scrolled).
     */
    static boolean within(View label, View focused) {
        if (focused == null) return false;
        if (focused == label) return true;
        for (ViewParent parent = label.getParent(); parent != null; parent = parent.getParent()) {
            if (parent == focused) return true;
        }
        return false;
    }

    /** The child of {@code holder}, which holds the label, that the label is in. */
    private static View tileOf(View label, View holder) {
        View tile = label;
        while (tile.getParent() != holder) tile = (View) tile.getParent();
        return tile;
    }
}
