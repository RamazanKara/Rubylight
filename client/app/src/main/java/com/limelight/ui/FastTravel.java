package com.limelight.ui;

/**
 * Tells a held D-pad key from a single press. While a key is held, or presses follow each other closer than
 * {@link #GAP_MS}, a screen skips its focus tweens and puts the new card straight into its focused state, so a
 * long run across the library is not a chain of animations that never finishes. It ends {@link #HOLD_MS} after
 * the last move. One instance belongs to one screen; the time is passed in so the logic is plain Java.
 */
public final class FastTravel {
    /** Moves closer together than this are one run. */
    public static final long GAP_MS = 120;
    /** Fast travel ends this long after the last move. */
    public static final long HOLD_MS = 150;

    private long lastMove = Long.MIN_VALUE;
    private boolean fast;

    /**
     * A key moved the selection or focus.
     *
     * @param repeatCount the key event's repeat count, above zero while the key is held
     * @return whether this move is part of a fast run
     */
    public boolean onMove(int repeatCount, long nowMs) {
        boolean following = lastMove != Long.MIN_VALUE && nowMs - lastMove < GAP_MS;
        fast = repeatCount > 0 || following;
        lastMove = nowMs;
        return fast;
    }

    /** Whether a run is on: the last move was fast and happened less than {@link #HOLD_MS} ago. */
    public boolean isActive(long nowMs) {
        return fast && lastMove != Long.MIN_VALUE && nowMs - lastMove < HOLD_MS;
    }

    public void reset() {
        fast = false;
        lastMove = Long.MIN_VALUE;
    }

    /**
     * Puts a card that is no longer changing into the look of its current state at once: ends its focus animator,
     * the animators of the views inside it (a poster's title) and the fades of their drawables. The card's drawable
     * state must already be the new one; a focus listener runs before it is, so it refreshes it first.
     */
    public static void settle(android.view.View card) {
        if (card == null) return;
        android.animation.StateListAnimator animator = card.getStateListAnimator();
        if (animator != null) animator.jumpToCurrentState();
        card.jumpDrawablesToCurrentState();
        if (card instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) card;
            for (int i = 0; i < group.getChildCount(); i++) settle(group.getChildAt(i));
        }
    }
}
