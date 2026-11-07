package com.limelight.ui;

import android.content.Context;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewParent;

/**
 * A cover in a {@link android.widget.GridView}: the grid holds focus and moves a selection, so this card shows focus while it has
 * the selection and the grid has focus (never in touch mode, where the grid selects nothing).
 */
public class GridCover extends CoverCard {
    private boolean shown;

    public GridCover(Context context) {
        super(context);
    }

    public GridCover(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected boolean showsFocus() {
        return shown || isFocused();
    }

    @Override
    public void setSelected(boolean selected) {
        super.setSelected(selected);
        update();
    }

    /** The selection or the grid's focus changed. */
    public void update() {
        ViewParent parent = getParent();
        boolean now = isSelected() && parent instanceof View && ((View) parent).hasFocus();
        if (now == shown) return;
        shown = now;
        onFocusShown(now);
    }
}
