package com.limelight.console;

import android.content.Context;
import android.view.View;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.ui.AppTheme;
import com.limelight.ui.GlassForm;

import java.util.function.IntPredicate;

/**
 * The phone's Home grid, with one rule added to the D-pad: when no card lies above the focused one, Up leaves the grid
 * for the shell bar. A plain grid answers such a press with the section header above the first row, which cannot take
 * focus, so it scrolled to the header and kept focus on the card for every further press: a controller could not reach
 * the Home, PCs and Settings tabs from a long list.
 */
final class HomeGridLayoutManager extends GridLayoutManager {
    private final IntPredicate cardBefore;
    private final IntPredicate heading;
    /** The keyline a focused card is brought to is the TV grid's: a phone's first row sits under the glass top row. */
    private final boolean television;
    /** How much a focused TV card grows, and the gap and ring drawn around it outside its bounds. */
    private final float focusScale;
    private final int ring;

    /**
     * @param cardBefore whether some card (not a header) is listed before the adapter position
     * @param heading whether the adapter position is a section's heading
     */
    HomeGridLayoutManager(Context context, int spanCount, IntPredicate cardBefore, IntPredicate heading) {
        super(context, spanCount);
        this.cardBefore = cardBefore;
        this.heading = heading;
        this.television = GlassForm.of(context) == GlassForm.TELEVISION;
        android.content.res.Resources res = context.getResources();
        focusScale = androidx.core.content.res.ResourcesCompat.getFloat(res, R.dimen.nj_focus_scale_cover_tv);
        ring = res.getDimensionPixelSize(R.dimen.glass_ring_gap) + res.getDimensionPixelSize(
                AppTheme.isHighContrast(context) ? R.dimen.nj_hc_focus_ring_width : R.dimen.glass_ring_width);
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
     * On a TV a focused card comes to rest at one keyline under the list's top edge, in both directions. The list's tall
     * foot padding leaves a padded view only a sliver high, so the platform's scroll put every card's top exactly on that
     * edge: the card's lift and focus ring, drawn past its bounds, were cut by the list's clip and dimmed by its fade, and
     * the section's title went above the edge (nightjar#7). A card in its section's first row brings its title in above
     * it; any other card stands its ring's reach below the fade.
     */
    @Override
    public boolean requestChildRectangleOnScreen(RecyclerView parent, View child, android.graphics.Rect rect, boolean immediate,
            boolean focusedChildVisible) {
        if (!television) {
            return super.requestChildRectangleOnScreen(parent, child, rect, immediate, focusedChildVisible);
        }
        int dy = child.getTop() - keyline(parent, child);
        if (dy == 0) return false;
        if (immediate) parent.scrollBy(0, dy);
        else parent.smoothScrollBy(0, dy);
        return true;
    }

    /**
     * A TV keeps a screen's worth of rows above the list's edge laid out: the title over a row that focus comes back up
     * to is measured where it has to stand, and the top fade has content to dissolve.
     */
    @Override
    protected void calculateExtraLayoutSpace(RecyclerView.State state, int[] extraLayoutSpace) {
        super.calculateExtraLayoutSpace(state, extraLayoutSpace);
        if (television) extraLayoutSpace[0] = Math.max(extraLayoutSpace[0], getHeight());
    }

    /** Where the top of the focused card {@code child} rests on a TV. */
    private int keyline(RecyclerView parent, View child) {
        int top = parent.getPaddingTop();
        int start = rowStart(getPosition(child));
        if (start > 0 && heading.test(start - 1)) {
            View head = findViewByPosition(start - 1);
            int title = head != null ? child.getTop() - head.getTop() : headingHeight();
            if (title > 0) return top + title;
        }
        int fade = parent.isVerticalFadingEdgeEnabled() ? parent.getVerticalFadingEdgeLength() : 0;
        return top + fade + focusReach(child.getHeight(), focusScale, ring);
    }

    /** The height a laid-out section heading takes, or 0 when none is laid out. */
    private int headingHeight() {
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            int position = getPosition(child);
            if (position >= 0 && heading.test(position)) return getDecoratedMeasuredHeight(child);
        }
        return 0;
    }

    /** How far past its top a card {@code height} tall reaches when focused: half its growth, then the gap and ring. */
    static int focusReach(int height, float scale, int ring) {
        return Math.round(height * Math.max(0f, scale - 1f) / 2f + ring * Math.max(1f, scale));
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
