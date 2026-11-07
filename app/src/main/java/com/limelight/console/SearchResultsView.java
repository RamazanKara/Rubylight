package com.limelight.console;

import android.content.Context;
import android.util.AttributeSet;

import androidx.recyclerview.widget.RecyclerView;

/**
 * Search's covers, fading out under the field. A fading edge stands at the padding's edge: the top has none for that reason,
 * and the foot, whose padding keeps clear of the keyboard, has no fade, or it would lie as a band across the covers.
 */
public class SearchResultsView extends RecyclerView {
    public SearchResultsView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected float getBottomFadingEdgeStrength() {
        return 0f;
    }
}
