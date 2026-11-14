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

    /**
     * Back up to the first row, the whole top header shows: a plain grid stopped once the card was on screen and left the
     * section's title above the list's edge.
     */
    @Override
    public boolean requestChildRectangleOnScreen(RecyclerView parent, View child, android.graphics.Rect rect, boolean immediate,
            boolean focusedChildVisible) {
        int position = getPosition(child);
        if (position > 0 && !cardBefore.test(rowStart(position))) {
            View header = findViewByPosition(0);
            if (header != null && header.getTop() < parent.getPaddingTop()) {
                int dy = header.getTop() - parent.getPaddingTop();
                if (immediate) parent.scrollBy(0, dy);
                else parent.smoothScrollBy(0, dy);
                return true;
            }
        }
        return super.requestChildRectangleOnScreen(parent, child, rect, immediate, focusedChildVisible);
    }

    /** The first position on the grid row of {@code position}. */
    private int rowStart(int position) {
        SpanSizeLookup spans = getSpanSizeLookup();
        int row = spans.getSpanGroupIndex(position, getSpanCount());
        int start = position;
        while (start > 0 && spans.getSpanGroupIndex(start - 1, getSpanCount()) == row) start--;
        return start;
    }
}
