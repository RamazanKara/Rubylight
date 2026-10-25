package com.limelight.console;

import android.content.Context;
import android.view.View;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import java.util.function.IntPredicate;

/**
 * The phone's Home grid, with one rule added to the D-pad: when no card lies above the focused one, Up leaves the grid
 * for the shell bar. A plain grid answers such a press with the section header above the first row, which cannot take
 * focus, so it scrolled to the header and kept focus on the card for every further press: a controller could not reach
 * the Home, PCs and Settings tabs from a long list.
 */
final class HomeGridLayoutManager extends GridLayoutManager {
    private final IntPredicate cardBefore;

    /** @param cardBefore whether some card (not a header) is listed before the adapter position */
    HomeGridLayoutManager(Context context, int spanCount, IntPredicate cardBefore) {
        super(context, spanCount);
        this.cardBefore = cardBefore;
    }

    @Override
    public View onFocusSearchFailed(View focused, int focusDirection, RecyclerView.Recycler recycler, RecyclerView.State state) {
        View found = super.onFocusSearchFailed(focused, focusDirection, recycler, state);
        if (found == null || found.hasFocusable() || focusDirection != View.FOCUS_UP) {
            return found;
        }
        // A header: worth scrolling to only while cards remain above it; else the search goes on outside the grid.
        return cardBefore.test(getPosition(found)) ? found : null;
    }
}
