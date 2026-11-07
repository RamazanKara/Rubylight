package com.limelight.console;

import android.content.Context;
import android.util.AttributeSet;
import android.view.MotionEvent;
import android.widget.FrameLayout;

/**
 * All games' frame around the poster grid. On a TV it stands into the focus line and filters above it, room for a focused
 * poster's glow: a touch in that strip is for what lies under it, not the grid.
 */
public class GridFrame extends FrameLayout {
    private int passAbove;

    public GridFrame(Context context) {
        super(context);
    }

    public GridFrame(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public GridFrame(Context context, AttributeSet attrs, int defStyle) {
        super(context, attrs, defStyle);
    }

    /** Touches that start in the top {@code px} go to the views under the frame. */
    void passTouchesAbove(int px) {
        passAbove = px;
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        // Declining the down leaves the whole gesture to the views below.
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN && event.getY() < passAbove) return false;
        return super.dispatchTouchEvent(event);
    }
}
