package com.limelight.binding.input;

/**
 * Hold Guide to open the stream menu, for one controller. While the hold time runs Guide stays off
 * the host; let go before it and the press goes out as a short Guide tap, held past it and the stream
 * menu opens and Guide never reaches the host. Another button pressed meanwhile is a chord (Steam's
 * Guide+RB): Guide goes down at once, before that button, and is an ordinary press from then on.
 * Other buttons are never held back. The caller runs the timer and calls {@link #expire()} when it
 * ends, on the main thread, while a USB driver's pad reports on its own thread.
 */
final class GuideHold {
    enum Release {
        /** Not a press this gesture holds back: Guide's release goes to the host as usual. */
        PASS,
        /** Let go before the hold time: send Guide as a short tap now. */
        TAP,
        /** The stream menu opened for this hold, or the input was let go: the release goes nowhere. */
        SWALLOW,
    }

    // Waiting for the hold time; held back until let go (the menu opened, or the input was let go).
    private boolean pending, swallowing;

    /**
     * Guide went down. Returns whether the press is held back (the hold time starts for a new
     * press); false when holding is off, and Guide goes to the host at once.
     */
    synchronized boolean down(long holdMs, boolean repeat) {
        if (repeat && active()) return true;
        // A new press: a release lost while the menu had the input ends the last one.
        reset();
        if (holdMs <= 0) return false;
        pending = true;
        return true;
    }

    /** The hold time ended; returns whether the stream menu opens now. */
    synchronized boolean expire() {
        if (!pending) return false;
        pending = false;
        swallowing = true;
        return true;
    }

    /**
     * Another button went down: returns whether a waiting Guide must go to the host now, before
     * that button. Its release is then an ordinary one ({@link Release#PASS}).
     */
    synchronized boolean chord() {
        if (!pending) return false;
        pending = false;
        return true;
    }

    synchronized Release up() {
        if (pending) {
            pending = false;
            return Release.TAP;
        }
        if (swallowing) {
            swallowing = false;
            return Release.SWALLOW;
        }
        return Release.PASS;
    }

    /** Whether Guide is held back now: waiting for the hold time or until it is let go. */
    synchronized boolean active() {
        return pending || swallowing;
    }

    /**
     * The input was let go before the hold time: no menu and no tap, and Guide, still held, stays
     * off the host until let go. Returns whether a hold was waiting.
     */
    synchronized boolean cancelPending() {
        if (!pending) return false;
        pending = false;
        swallowing = true;
        return true;
    }

    synchronized void reset() {
        pending = false;
        swallowing = false;
    }
}
