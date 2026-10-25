package com.limelight.preferences;

import android.content.Context;
import android.text.TextUtils;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewParent;
import android.view.ViewTreeObserver;
import android.widget.AdapterView;
import androidx.appcompat.widget.AppCompatTextView;

/**
 * A settings row's summary that is capped to its layout's maxLines until the row is the one the D-pad has
 * selected: then it shows all of its text, so a cap that keeps the list compact on a TV never hides a word
 * from the person reading the row. Without a cap (the phone layouts) it is a plain TextView.
 */
public class ExpandingSummaryView extends AppCompatTextView implements ViewTreeObserver.OnGlobalFocusChangeListener {
    private final int collapsedLines;
    private boolean listening;

    public ExpandingSummaryView(Context context, AttributeSet attrs) {
        super(context, attrs);
        collapsedLines = getMaxLines();
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        if (collapsedLines != Integer.MAX_VALUE && !listening) {
            getViewTreeObserver().addOnGlobalFocusChangeListener(this);
            listening = true;
        }
        update();
    }

    @Override
    protected void onDetachedFromWindow() {
        if (listening) {
            getViewTreeObserver().removeOnGlobalFocusChangeListener(this);
            listening = false;
        }
        super.onDetachedFromWindow();
    }

    @Override
    public void setSelected(boolean selected) {
        super.setSelected(selected);
        update();
    }

    @Override
    public void onGlobalFocusChanged(View oldFocus, View newFocus) {
        update();
    }

    /** Whether this summary sits in the selected row of a list that holds focus. */
    private boolean inFocusedRow() {
        View row = this;
        for (ViewParent parent = getParent(); parent != null; parent = row.getParent()) {
            if (parent instanceof AdapterView) return row.isSelected() && ((View) parent).hasFocus();
            if (!(parent instanceof View)) return false;
            row = (View) parent;
        }
        return false;
    }

    private void update() {
        if (collapsedLines == Integer.MAX_VALUE) return;
        boolean expanded = inFocusedRow();
        int lines = expanded ? Integer.MAX_VALUE : collapsedLines;
        if (getMaxLines() == lines) return;
        setMaxLines(lines);
        setEllipsize(expanded ? null : TextUtils.TruncateAt.END);
        // The row grows below the selection: the list only keeps the selected row in view when it moves.
        post(this::showRow);
    }

    private void showRow() {
        View row = this;
        for (ViewParent parent = getParent(); parent instanceof View; parent = row.getParent()) {
            if (parent instanceof AdapterView) {
                if (row.isSelected() && ((View) parent).hasFocus() && row.getHeight() > 0) {
                    row.requestRectangleOnScreen(new android.graphics.Rect(0, 0, row.getWidth(), row.getHeight()));
                }
                return;
            }
            row = (View) parent;
        }
    }
}
