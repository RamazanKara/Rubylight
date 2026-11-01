package com.limelight.binding.input;

/**
 * Hold Guide to open the stream menu, for one controller. While the hold time runs Guide stays off
 * the host; let go before it and the press goes out as a short Guide tap, held past it and the stream
 * menu opens and Guide never reaches the host. Other buttons are never held back. The caller runs the
 * timer and calls {@link #expire()} when it ends, on the main thread, while a USB driver's pad
 * reports on its own thread.
 */
final class GuideHold {
    enum Release {
        /** Not a press this gesture held back: Guide's release goes to the host as usual. */
        PASS,
        /** Let go before the hold time: send Guide as a short tap now. */
        TAP,
        /** The stream menu opened for this hold: the release goes nowhere. */
        SWALLOW,
    }

    private boolean pending, opened;

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
        opened = true;
        return true;
    }

    synchronized Release up() {
        if (pending) {
            pending = false;
            return Release.TAP;
        }
        if (opened) {
            opened = false;
            return Release.SWALLOW;
        }
        return Release.PASS;
    }

    /** Whether Guide is held back now: waiting for the hold time or holding the menu open. */
    synchronized boolean active() {
        return pending || opened;
    }

    /** The input was let go before the hold time: the menu stays shut; returns whether a hold was waiting. */
    synchronized boolean cancelPending() {
        boolean was = pending;
        pending = false;
        return was;
    }

    synchronized void reset() {
        pending = false;
        opened = false;
    }
}
