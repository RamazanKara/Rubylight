package com.limelight.console;

import android.content.Context;
import android.view.View;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.ui.AppTheme;
import com.limelight.ui.GlassForm;

import java.util.function.IntFunction;
import java.util.function.IntPredicate;
import java.util.function.ToIntFunction;

/**
 * Home's grid and list, with two rules added to the D-pad. When no card lies above the focused one, Up leaves the grid
 * for the shell bar. A plain grid answers such a press with the section header above the first row, which cannot take
 * focus, so it scrolled to the header and kept focus on the card for every further press: a controller could not reach
 * the Home, PCs and Settings tabs from a long list. And Up from the card a Down press landed on goes back to the card it
 * came from.
 */
final class HomeGridLayoutManager extends GridLayoutManager {
    private final IntPredicate cardBefore;
    private final IntPredicate heading;
    private final IntFunction<String> keyAt;
    private final ToIntFunction<String> positionOfKey;
    /** The card the last Down left and the card it landed on, by stable key; null when focus has moved on since. */
    private String downFrom;
    private String downTo;
    /** The direction and starting card of the focus search under way, until a card takes the focus. */
    private int searchDirection;
    private String searchFrom;
    /** The keyline a focused card is brought to is the TV grid's: a phone's first row sits under the glass top row. */
    private final boolean television;
    /** How much a focused TV card grows, and the gap and ring drawn around it outside its bounds. */
    private final float focusScale;
    private final int ring;

    /**
     * @param cardBefore whether some card (not a header) is listed before the adapter position
     * @param heading whether the adapter position is a section's heading
     * @param keyAt the stable key of the card at the adapter position, null for a heading
     * @param positionOfKey the adapter position of the card with the key, or -1
     */
    HomeGridLayoutManager(Context context, int spanCount, IntPredicate cardBefore, IntPredicate heading,
            IntFunction<String> keyAt, ToIntFunction<String> positionOfKey) {
        super(context, spanCount);
        this.cardBefore = cardBefore;
        this.heading = heading;
        this.keyAt = keyAt;
        this.positionOfKey = positionOfKey;
        this.television = GlassForm.of(context) == GlassForm.TELEVISION;
        android.content.res.Resources res = context.getResources();
        focusScale = androidx.core.content.res.ResourcesCompat.getFloat(res, R.dimen.nj_focus_scale_cover_tv);
        ring = res.getDimensionPixelSize(R.dimen.glass_ring_gap) + res.getDimensionPixelSize(
                AppTheme.isHighContrast(context) ? R.dimen.nj_hc_focus_ring_width : R.dimen.glass_ring_width);
    }

    /**
     * Up from the card a Down press landed on goes back to the card it came from (nightjar#7). The platform's search
     * takes the card nearest the middle of the one focused: from a List line as wide as the screen that was the second
     * card of the row above, not the first one focus had come down from.
     */
    @Override
    public View onInterceptFocusSearch(View focused, int direction) {
        View card = findContainingItemView(focused);
        int position = card == null ? RecyclerView.NO_POSITION : getPosition(card);
        searchDirection = direction;
        searchFrom = position == RecyclerView.NO_POSITION ? null : keyAt.apply(position);
        if (direction == View.FOCUS_UP && searchFrom != null && searchFrom.equals(downTo)) {
            int back = positionOfKey.applyAsInt(downFrom);
            View view = back >= 0 && row(back) < row(position) ? findViewByPosition(back) : null;
            if (view != null && view.isFocusable()) return view;
        }
        return super.onInterceptFocusSearch(focused, direction);
    }

    @Override
    public boolean onRequestChildFocus(RecyclerView parent, RecyclerView.State state, View child, View focused) {
        int position = getPosition(child);
        String key = position == RecyclerView.NO_POSITION ? null : keyAt.apply(position);
        // The search is this move's only while focus still sits on the card it started from: a probe that moved nothing,
        // a press that left the list, focus coming in from the tabs or after a refresh leave it behind.
        View before = parent.getFocusedChild();
        int beforeAt = before == null ? RecyclerView.NO_POSITION : getPosition(before);
        String beforeKey = beforeAt == RecyclerView.NO_POSITION ? null : keyAt.apply(beforeAt);
        if (searchFrom != null && !searchFrom.equals(beforeKey)) {
            searchDirection = 0;
            searchFrom = null;
        }
        int from = searchFrom == null ? -1 : positionOfKey.applyAsInt(searchFrom);
        if (searchDirection == View.FOCUS_DOWN && key != null && from >= 0 && row(position) > row(from)) {
            downFrom = searchFrom;
            downTo = key;
        } else if (key == null || !key.equals(downTo)) {
            downFrom = null;
            downTo = null;
        }
        searchDirection = 0;
        searchFrom = null;
        return super.onRequestChildFocus(parent, state, child, focused);
    }

    /** The grid row of the adapter position. */
    private int row(int position) {
        return getSpanSizeLookup().getSpanGroupIndex(position, getSpanCount());
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
