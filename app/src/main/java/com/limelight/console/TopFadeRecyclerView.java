package com.limelight.console;

import android.content.Context;
import android.util.AttributeSet;

import androidx.recyclerview.widget.RecyclerView;

/**
 * A list whose rows fade out only at its top, under a heading. The platform fades the bottom too, and ends that fade at
 * the bottom padding's edge: with clipToPadding off, rows below it stood bright under a hard line.
 */
public class TopFadeRecyclerView extends RecyclerView {
    public TopFadeRecyclerView(Context context) {
        super(context);
    }

    public TopFadeRecyclerView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public TopFadeRecyclerView(Context context, AttributeSet attrs, int defStyle) {
        super(context, attrs, defStyle);
    }

    @Override
    protected float getBottomFadingEdgeStrength() {
        return 0f;
    }
}
