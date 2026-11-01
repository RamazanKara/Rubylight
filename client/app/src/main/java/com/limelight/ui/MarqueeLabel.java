package com.limelight.ui;

import android.content.Context;
import android.text.TextUtils;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewParent;
import android.view.ViewTreeObserver;

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
        TextUtils.TruncateAt wanted = scroll ? TextUtils.TruncateAt.MARQUEE : TextUtils.TruncateAt.END;
        if (getEllipsize() != wanted) setEllipsize(wanted);
        // A selected label scrolls without focus of its own; the tile keeps the focus.
        if (isSelected() != scroll) setSelected(scroll);
    }

    /** True when {@code focused} is the label or holds it. */
    static boolean within(View label, View focused) {
        if (focused == null) return false;
        if (focused == label) return true;
        for (ViewParent parent = label.getParent(); parent != null; parent = parent.getParent()) {
            if (parent == focused) return true;
        }
        return false;
    }
}
