package com.limelight.console;

import android.content.Context;
import android.graphics.Rect;
import android.view.View;
import android.view.ViewParent;
import android.view.animation.Interpolator;

import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;

import java.util.function.BooleanSupplier;

/**
 * The layout manager of Home's shelves, both directions of it.
 *
 * <p>A <b>row</b> manager (vertical) holds the shelves. When a card inside one gets focus it snaps that shelf's top to the
 * list's top edge, over {@link #ROW_MS}, and an UP or DOWN press lands on the card the shelf remembers, or else on the card
 * nearest the one that was focused ({@link RowFocus}). A <b>card</b> manager (horizontal) holds one shelf's cards. It keeps
 * the focused card one card short of the end, so the next card always peeks in, and scrolls over {@link #CARD_MS}. Both
 * scroll at once while {@code instant} says so (a held key, Reduce motion).
 */
final class ShelfLayoutManager extends LinearLayoutManager {
    static final int ROW_MS = 240;
    static final int CARD_MS = 200;

    /** What a vertical move into a shelf lands on; implemented by the adapter that knows each shelf's memory. */
    interface RowFocus {
        /** The card of the shelf {@code row} to focus when focus comes from {@code from}, or null to leave it to the search. */
        View cardIn(RecyclerView rows, View row, View from);
    }

    private final boolean cards;
    private final BooleanSupplier instant;
    private final Interpolator interpolator;
    private RowFocus rowFocus;
    private RecyclerView attached;

    /** A manager for one shelf's cards when {@code orientation} is horizontal, for the shelves when it is vertical. */
    ShelfLayoutManager(Context context, int orientation, BooleanSupplier instant) {
        super(context, orientation, false);
        this.cards = orientation == HORIZONTAL;
        this.instant = instant;
        this.interpolator = android.view.animation.AnimationUtils.loadInterpolator(context, R.interpolator.tv_std);
        if (cards) setInitialPrefetchItemCount(7);
    }

    void setRowFocus(RowFocus rowFocus) {
        this.rowFocus = rowFocus;
    }

    @Override
    public void onAttachedToWindow(RecyclerView view) {
        super.onAttachedToWindow(view);
        attached = view;
    }

    @Override
    public void onDetachedFromWindow(RecyclerView view, RecyclerView.Recycler recycler) {
        super.onDetachedFromWindow(view, recycler);
        attached = null;
    }

    /** Cards are laid out a little past the edge, so the card a key moves to is always there. */
    @Override
    protected void calculateExtraLayoutSpace(RecyclerView.State state, int[] extraLayoutSpace) {
        if (cards) {
            int card = getChildCount() > 0 ? getChildAt(0).getWidth() : 0;
            extraLayoutSpace[0] = 0;
            extraLayoutSpace[1] = 2 * card;
        } else {
            // A screen's worth of shelves past the end: a key to the shelf below always finds it laid out.
            extraLayoutSpace[0] = 0;
            extraLayoutSpace[1] = getHeight();
        }
    }

    /** How far the list scrolls for a card that has just taken focus: the keyline of a shelf, the snap of a list of shelves. */
    int shiftFor(View child, int start, int end) {
        return cards ? cardShift(child.getLeft(), child.getRight(), start, end) : child.getTop() - start;
    }

    /**
     * The scroll that puts a card at {@code left..right} where it belongs in a strip from {@code start} to {@code end}: fully
     * in view, and short of the end by one card, which is the peek of the next one. Positive scrolls toward the end.
     */
    static int cardShift(int left, int right, int start, int end) {
        int card = right - left;
        if (left < start) return left - start;
        int limit = end - card;
        return right > limit ? right - limit : 0;
    }

    @Override
    public boolean requestChildRectangleOnScreen(RecyclerView parent, View child, Rect rect, boolean immediate,
                                                 boolean focusedChildVisible) {
        int shift = cards
                ? shiftFor(child, getPaddingLeft(), getWidth() - getPaddingRight())
                : shiftFor(child, getPaddingTop(), 0);
        if (shift == 0) return false;
        int dx = cards ? shift : 0;
        int dy = cards ? 0 : shift;
        if (immediate || instant.getAsBoolean()) {
            parent.scrollBy(dx, dy);
        } else {
            parent.smoothScrollBy(dx, dy, interpolator, cards ? CARD_MS : ROW_MS);
        }
        return true;
    }

    /** The shelf's direct child that holds {@code view}, or null. */
    private View rowOf(View view) {
        if (attached == null) return null;
        View child = view;
        for (ViewParent parent = child.getParent(); parent != null && parent != attached; parent = child.getParent()) {
            if (!(parent instanceof View)) return null;
            child = (View) parent;
        }
        return child.getParent() == attached ? child : null;
    }

    @Override
    public View onInterceptFocusSearch(View focused, int direction) {
        if (attached == null) return super.onInterceptFocusSearch(focused, direction);
        if (cards) {
            // Sideways stays on the shelf: the search must not jump to a card of another one that lies in that direction.
            boolean forward = direction == View.FOCUS_RIGHT || direction == View.FOCUS_FORWARD;
            boolean back = direction == View.FOCUS_LEFT || direction == View.FOCUS_BACKWARD;
            if (!forward && !back) return super.onInterceptFocusSearch(focused, direction);
            View card = rowOf(focused);
            if (card == null) return super.onInterceptFocusSearch(focused, direction);
            boolean rtl = getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
            boolean toEnd = forward != (rtl && (direction == View.FOCUS_RIGHT || direction == View.FOCUS_LEFT));
            int next = getPosition(card) + (toEnd ? 1 : -1);
            if (next < 0 || next >= getItemCount()) return focused;
            // A card that is not laid out yet is brought in by onFocusSearchFailed.
            return findViewByPosition(next);
        }
        if (rowFocus == null || (direction != View.FOCUS_UP && direction != View.FOCUS_DOWN)) {
            return super.onInterceptFocusSearch(focused, direction);
        }
        View row = rowOf(focused);
        if (row == null) return super.onInterceptFocusSearch(focused, direction);
        int next = getPosition(row) + (direction == View.FOCUS_DOWN ? 1 : -1);
        if (next < 0 || next >= getItemCount()) return super.onInterceptFocusSearch(focused, direction);
        View target = findViewByPosition(next);
        // A shelf that is not laid out yet is brought in by onFocusSearchFailed.
        return target == null ? null : cardOr(target, focused);
    }

    /**
     * Focus that comes into the shelves from outside them (down from the shell bar) goes to the card the first shelf in view
     * remembers, not to the card nearest across: the shelves offer that one card and no other.
     */
    @Override
    public boolean onAddFocusables(RecyclerView recyclerView, java.util.ArrayList<View> views, int direction, int focusableMode) {
        if (cards || rowFocus == null || recyclerView.hasFocus() || getChildCount() == 0) return false;
        if (direction != View.FOCUS_DOWN && direction != View.FOCUS_FORWARD) return false;
        View from = recyclerView.getRootView().findFocus();
        View card = from == null ? null : rowFocus.cardIn(recyclerView, getChildAt(0), from);
        if (card == null || !card.isFocusable()) return false;
        views.add(card);
        return true;
    }

    @Override
    public View onFocusSearchFailed(View focused, int focusDirection, RecyclerView.Recycler recycler, RecyclerView.State state) {
        View found = super.onFocusSearchFailed(focused, focusDirection, recycler, state);
        if (cards || rowFocus == null || found == null || attached == null) return found;
        return found.getParent() == attached ? cardOr(found, focused) : found;
    }

    private View cardOr(View row, View from) {
        View card = rowFocus.cardIn(attached, row, from);
        return card != null ? card : row;
    }
}
