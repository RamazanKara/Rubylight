package com.limelight.binding.video;

/**
 * Deadline present scheduling for a display with a fixed refresh rate, on top of
 * [HostFrameClock]: where a decoded frame goes on the panel's vsync grid, and whether it is so
 * late that a newer decoded frame should replace it.
 *
 * A frame is meant for the first vsync it can make, which is what releasing it at once would
 * give, unless the game's cadence puts it later. The most it is moved past that vsync is one
 * (the clock's rule); this class only turns the display's timing into the clock's arguments:
 * <ul>
 *   <li>Android shows a buffer queued at least the presentation deadline before a vsync at that
 *   vsync, so a frame ready at t is ready for the grid at t + deadline.</li>
 *   <li>The buffer's timestamp is the time it is due; the compositor takes it at the first vsync
 *   at or after it. A quarter of a vsync before the vsync keeps it after the vsync before, so a
 *   frame planned a vsync later is not shown a vsync early.</li>
 *   <li>The target is rounded to the nearest vsync (a slack of half a vsync), not up: the extra
 *   half vsync a frame waited on average was the largest part of what the schedule added. A frame
 *   still never goes before the first vsync it can make.</li>
 * </ul>
 *
 * Late frames. The clock holds each frame to the host time plus a delay that covers recent
 * jitter. A frame ready more than half a content interval after that is behind the game's
 * cadence; the caller drops it when a newer output is already decoded, never two in a row. The
 * lateness window is kept here, not read from the clock, because the clock must see a frame only
 * once and a dropped frame must not take a vsync from the frame that replaces it.
 */
final class PresentScheduler {
    /** A frame is due this fraction of a vsync before its vsync. */
    static final int LEAD_DIVISOR = 4;
    /**
     * Lateness window and the percentile of it that marks a frame as late. The clock's own delay,
     * on a vsync grid, covers a lower percentile (GRID_PERCENTILE in present_clock.rs); a frame is
     * called late only past the higher one so that fewer frames are dropped.
     */
    static final int WINDOW = 128;
    static final int PERCENTILE = 97;
    static final long MARGIN_US = 500;
    /** No frame counts as late before this many frames and this much host time (the clock's warm-up). */
    static final int MIN_SAMPLES = 32;
    static final long WARMUP_US = 250_000;
    /** A host time this far from the previous one is a restarted video clock. */
    private static final long DISCONTINUITY_US = 10_000_000;

    /** One planned frame. */
    static final class Plan {
        /** System.nanoTime() the buffer is due at (the timestamp for releaseOutputBuffer). */
        long releaseNs;
        /** The vsync the frame is meant for. */
        long vsyncNs;
        /** Time from the frame being ready to its release time. */
        long holdNs;
        /** The frame shares its vsync with the one before it. */
        boolean sharesVsync;
    }

    private final HostFrameClock clock;
    private final HostFrameClock.GridPlan grid = new HostFrameClock.GridPlan();
    private final long[] lateness = new long[WINDOW];
    private final long[] sorted = new long[WINDOW];
    private int count;
    private int next;
    private long firstHostUs = Long.MIN_VALUE;
    private long lastHostUs = Long.MIN_VALUE;
    private volatile long lateDropped;
    private volatile long governorDropped;
    private volatile long capDropped;
    private volatile long capHeld;
    private volatile long planned;
    private volatile long shared;

    PresentScheduler(HostFrameClock clock) {
        this.clock = clock;
    }

    /** Forget the lateness window, for example after a decoder flush; counters stay. */
    synchronized void reset() {
        count = 0;
        next = 0;
        firstHostUs = Long.MIN_VALUE;
        lastHostUs = Long.MIN_VALUE;
    }

    /**
     * Notes a decoded frame, ready at readyNs, in the lateness window (call it once per frame, with
     * the same time basis as the later [plan] or clock call) and returns whether it is behind the
     * game's cadence by more than half of contentIntervalNs. False while the window is filling,
     * and for a frame whose host time is unknown.
     */
    synchronized boolean isLate(long codecPtsUs, long readyNs, long contentIntervalNs) {
        long host = clock.hostUsFor(codecPtsUs);
        if (host < 0) return false;
        long behindUs = readyNs / 1000 - host;
        if (lastHostUs != Long.MIN_VALUE && Math.abs(host - lastHostUs) > DISCONTINUITY_US) reset();
        lastHostUs = host;
        if (firstHostUs == Long.MIN_VALUE) firstHostUs = host;
        boolean late = false;
        if (count >= MIN_SAMPLES && host - firstHostUs >= WARMUP_US) {
            long delayUs = percentile() + MARGIN_US;
            late = behindUs - delayUs > contentIntervalNs / 2000;
        }
        lateness[next] = behindUs;
        next = (next + 1) % WINDOW;
        count = Math.min(count + 1, WINDOW);
        return late;
    }

    private long percentile() {
        System.arraycopy(lateness, 0, sorted, 0, count);
        java.util.Arrays.sort(sorted, 0, count);
        return sorted[(count - 1) * PERCENTILE / 100];
    }

    /**
     * Plans the frame onto the grid of a panel whose last known vsync was at vsyncNs, with vsyncs
     * intervalNs apart and a presentation deadline of deadlineNs. Fills out and returns true; false
     * (leaving out alone) when the frame cannot be planned and should be released as without a grid.
     */
    boolean plan(long codecPtsUs, long readyNs, long vsyncNs, long intervalNs, long deadlineNs, Plan out) {
        if (vsyncNs <= 0 || intervalNs <= 0) return false;
        long readyForGridNs = readyNs + Math.max(0, deadlineNs);
        if (!clock.presentOnGrid(codecPtsUs, readyForGridNs, vsyncNs, intervalNs, intervalNs / LEAD_DIVISOR,
                intervalNs / 2, grid)) {
            return false;
        }
        out.releaseNs = grid.releaseNs;
        out.vsyncNs = grid.vsyncNs;
        out.holdNs = grid.releaseNs - readyNs;
        out.sharesVsync = grid.sharesVsync;
        planned++;
        if (grid.sharesVsync) shared++;
        return true;
    }

    /**
     * A frame dropped for a newer one still teaches the clock its lateness, without taking a
     * vsync. readyNs is on the same basis as the frames that are planned.
     */
    void discard(long codecPtsUs, long readyNs) {
        clock.presentNs(codecPtsUs, readyNs);
    }

    /** Whether a late frame is replaced by the newer output already decoded. Never twice in a row. */
    static boolean dropLate(boolean late, boolean newerAvailable, boolean previousDropped) {
        return late && newerAvailable && !previousDropped;
    }

    void onLateDropped() { lateDropped++; }
    void onGovernorDropped() { governorDropped++; }
    void onCapDropped() { capDropped++; }
    void onCapHeld() { capHeld++; }

    long lateDropped() { return lateDropped; }
    long governorDropped() { return governorDropped; }
    long capDropped() { return capDropped; }
    long capHeld() { return capHeld; }

    /** One overlay line, or null before the first planned frame. */
    String overlayLine() {
        if (planned == 0) return null;
        return String.format(java.util.Locale.US,
                "Scheduler: shared vsync %.1f%% | late dropped %d | queue dropped %d | compositor held %d",
                100.0 * shared / planned, lateDropped, governorDropped + capDropped, capHeld);
    }
}
