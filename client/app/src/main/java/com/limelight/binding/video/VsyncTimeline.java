package com.limelight.binding.video;

/**
 * The display timeline from Android 13's per-frame vsync data (Choreographer.FrameData), in
 * place of {@code Display.getPresentationDeadlineNanos()} and the app vsync offset.
 *
 * Those two are fixed values from the display's configuration. SurfaceFlinger's own timeline
 * says, for each vsync, when a frame queued now will be presented and by when it must be queued
 * for that: the preferred frame timeline's expected presentation time and deadline. The grid
 * anchor and the latch deadline (how long before its vsync a buffer must be queued) come from
 * those two numbers here, so a device whose configured values are off still gets frames aimed at
 * the latch it really has. Values that cannot be right are rejected and the fixed ones stay.
 */
final class VsyncTimeline {
    private VsyncTimeline() {
    }

    /** A deadline more than this many refreshes before its vsync is not plausible. */
    static final int MAX_DEADLINE_REFRESHES = 3;

    /**
     * The latest vsync on the grid through expectedPresentNs that is not after frameTimeNs: a vsync
     * that has happened when the callback runs, as the fixed-offset path gives. Returns 0 when the
     * values cannot be a timeline (no interval, or a presentation time before the frame time).
     */
    static long anchorNs(long expectedPresentNs, long frameTimeNs, long intervalNs) {
        if (intervalNs <= 0 || expectedPresentNs <= 0 || frameTimeNs <= 0 || expectedPresentNs < frameTimeNs) {
            return 0;
        }
        long ahead = expectedPresentNs - frameTimeNs;
        long intervals = (ahead + intervalNs - 1) / intervalNs;
        return expectedPresentNs - intervals * intervalNs;
    }

    /**
     * How long before its vsync a buffer must be queued to make it: the expected presentation
     * time less the deadline. Returns -1 when that is negative or more than
     * {@link #MAX_DEADLINE_REFRESHES} refreshes, which no real timeline has.
     */
    static long deadlineLeadNs(long expectedPresentNs, long deadlineNs, long intervalNs) {
        if (intervalNs <= 0 || expectedPresentNs <= 0 || deadlineNs <= 0) return -1;
        long lead = expectedPresentNs - deadlineNs;
        return lead >= 0 && lead <= MAX_DEADLINE_REFRESHES * intervalNs ? lead : -1;
    }

    /** Whether a new lead differs enough from the one last logged to log it again. */
    static boolean worthLogging(long loggedLeadNs, long leadNs) {
        return loggedLeadNs < 0 || Math.abs(leadNs - loggedLeadNs) >= 1_000_000;
    }
}
