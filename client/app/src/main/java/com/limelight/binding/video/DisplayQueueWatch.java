package com.limelight.binding.video;

import java.util.Arrays;
import java.util.Locale;

/**
 * Lowest latency and Balanced: a buffer too many in the display's queue (nightjar#7).
 *
 * <p>Both modes hand each frame to the display at once with "now" as its time, so the compositor
 * shows it at its next latch and replaces a waiting frame by a newer one. Some displays still keep
 * one or two frames waiting for minutes: their render times (the codec's render callback) then sit
 * one or two whole refreshes later after the hand-off than the same stream showed before, and only
 * a stutter that lets the queue run dry gives that refresh back. Dropping one frame does the same
 * on purpose: the screen keeps the previous frame one refresh longer, and the queue holds one buffer
 * less from then on.
 *
 * <p>Gated by what the display reports, not by device: only a wait that is at least a refresh above
 * the stream's own lowest level for a few seconds, or a steady wait of several refreshes whose
 * lowest level is not known yet (one probe), drops a frame. Each drop is checked: when the wait
 * does not fall by about a refresh, that level is kept as this display's own, and after
 * {@link #MAX_MISSES} such misses the watch stops for the stream. A device whose render times
 * follow the hand-off (most phones report a time close to it) never reaches either condition.
 *
 * <p>Waits are taken per second as medians, and a level is the median of the last
 * {@link #LEVEL_SECONDS} of them, so a single late frame or a stutter does not count. A frame the
 * display reports before the previous one is a frame it replaced in its queue: it is logged (the
 * line that shows this happening on a device) and left out.
 *
 * <p>Thread use: {@link #onRendered} runs on the render callback's thread, {@link #takeDrop} on the
 * thread that releases frames; every method is synchronized.
 */
final class DisplayQueueWatch {
    static final long SECOND_NS = 1_000_000_000L;
    /** A second with fewer frames than this gives no median. */
    static final int MIN_FRAMES = 10;
    /** A level is the median of this many one-second medians. */
    static final int LEVEL_SECONDS = 3;
    /** Seconds a level stays a refresh over the lowest one before a drop. */
    static final int SUSTAIN_SECONDS = 3;
    /** On top of the refresh, so a wait at the far end of one refresh's phase never counts. */
    static final long MARGIN_NS = 1_000_000;
    /** Seconds a level stays within a quarter refresh before a probe. */
    static final int STEADY_SECONDS = 5;
    /** A probe only where the display's wait is at least this many refreshes. */
    static final double PROBE_REFRESHES = 2.5;
    /** The least time between two drops. */
    static final long MIN_GAP_NS = 10 * SECOND_NS;
    static final int MAX_DROPS = 12;
    static final int MAX_MISSES = 2;
    /** A wait longer than this is no queue (a stall, a stream start) and is left out. */
    static final long MAX_WAIT_NS = 500_000_000L;
    /** The least time between two lines about replaced frames. */
    static final long REPLACED_LOG_NS = 10 * SECOND_NS;

    private boolean enabled;
    private long refreshNs;

    private final long[] waits = new long[512];
    private int count;
    private long secondStartNs;
    private final long[] medians = new long[LEVEL_SECONDS];
    private int medianCount;

    private long floorNs = Long.MAX_VALUE;
    private boolean floorKnown;
    private int overSeconds;
    private int steadySeconds;
    private long steadyLevelNs;

    private boolean dropDue;
    private boolean probe;
    private long dropLevelNs;
    private boolean checking;
    private boolean checkingProbe;
    private long checkFromNs;
    private long lastDropNs;
    private int drops;
    private int misses;
    private boolean off;

    private long lastRenderNs;
    private int replaced;
    private long replacedLoggedNs;
    private String event;

    /**
     * Whether the watch applies: its setting on (Take back display queue delay), Lowest latency or
     * Balanced without host frame times (the scheduled modes keep their own queue), frames straight
     * to the display with render callbacks, a fixed refresh, and a stream at least as fast as the
     * display (a slower one lets the queue run dry every refresh, so no frame stays waiting).
     * When it does not apply the watch reads no frames and drops none.
     */
    static boolean applies(boolean settingOn, boolean latencyOrBalanced, boolean scheduled, boolean renderCallbacks,
                           boolean vrr, int streamFrameRateMilliHz, long refreshNs) {
        if (!settingOn || !latencyOrBalanced || scheduled || !renderCallbacks || vrr || refreshNs <= 0 ||
                streamFrameRateMilliHz <= 0) {
            return false;
        }
        long frameIntervalNs = 1_000_000_000_000L / streamFrameRateMilliHz;
        return frameIntervalNs <= refreshNs + refreshNs / 50;
    }

    /** Called with each frame's state; a change of either starts over. */
    synchronized void configure(boolean enabled, long refreshNs) {
        if (enabled == this.enabled && refreshNs == this.refreshNs) return;
        boolean wasEnabled = this.enabled;
        this.enabled = enabled;
        this.refreshNs = refreshNs;
        reset();
        if (enabled && wasEnabled) {
            event = String.format(Locale.US, "Display queue: refresh now %.2f ms, starting over", refreshNs / 1e6);
        }
    }

    private void reset() {
        count = 0;
        medianCount = 0;
        floorNs = Long.MAX_VALUE;
        floorKnown = false;
        overSeconds = 0;
        steadySeconds = 0;
        steadyLevelNs = 0;
        dropDue = false;
        checking = false;
        lastDropNs = 0;
        drops = 0;
        misses = 0;
        off = false;
        lastRenderNs = 0;
        replaced = 0;
        replacedLoggedNs = 0;
    }

    /**
     * One frame the display showed: {@code waitNs} from its hand-off to its render time
     * {@code renderNs} (the platform's, so callback batching does not matter).
     */
    synchronized void onRendered(long waitNs, long renderNs) {
        if (!enabled) return;
        if (lastRenderNs != 0 && renderNs <= lastRenderNs) {
            replaced++;
            if (replacedLoggedNs == 0 || renderNs - replacedLoggedNs >= REPLACED_LOG_NS) {
                replacedLoggedNs = renderNs;
                event = String.format(Locale.US, "Display queue: a frame reported %.1f ms before the previous one " +
                                "(wait %.1f ms) was replaced in the display's queue (%d this stream)",
                        (lastRenderNs - renderNs) / 1e6, waitNs / 1e6, replaced);
            }
            return;
        }
        lastRenderNs = renderNs;
        if (waitNs < 0 || waitNs > MAX_WAIT_NS) return;
        if (count == 0) secondStartNs = renderNs;
        if (count < waits.length) waits[count++] = waitNs;
        if (renderNs - secondStartNs >= SECOND_NS) {
            if (count >= MIN_FRAMES) onSecond(median(waits, count), renderNs);
            count = 0;
        }
    }

    private void onSecond(long secondMedianNs, long nowNs) {
        medians[medianCount % LEVEL_SECONDS] = secondMedianNs;
        medianCount++;
        if (medianCount < LEVEL_SECONDS || off) return;
        long level = median(Arrays.copyOf(medians, LEVEL_SECONDS), LEVEL_SECONDS);
        if (checking) {
            checking = false;
            if (level <= checkFromNs - refreshNs / 2) {
                event = String.format(Locale.US, "Display queue: wait %.1f -> %.1f ms after one dropped frame: " +
                        "the display held a buffer too many", checkFromNs / 1e6, level / 1e6);
                // The new level may still hold one: a probe may follow.
                floorNs = level;
                floorKnown = false;
            } else {
                // No buffer to give back: this is the display's own level.
                floorNs = level;
                floorKnown = true;
                if (!checkingProbe) misses++;
                if (misses >= MAX_MISSES) off = true;
                event = String.format(Locale.US, "Display queue: wait %.1f -> %.1f ms after one dropped frame: " +
                                "no buffer too many%s", checkFromNs / 1e6, level / 1e6,
                        off ? "; this display's wait does not follow its queue, no more drops this stream" : "");
            }
            overSeconds = 0;
            steadySeconds = 0;
            steadyLevelNs = level;
            return;
        }
        floorNs = Math.min(floorNs, level);
        overSeconds = level >= floorNs + refreshNs + MARGIN_NS ? overSeconds + 1 : 0;
        if (Math.abs(level - steadyLevelNs) <= refreshNs / 4) {
            steadySeconds++;
        } else {
            steadySeconds = 0;
            steadyLevelNs = level;
        }
        String why;
        boolean asProbe;
        if (overSeconds >= SUSTAIN_SECONDS) {
            why = String.format(Locale.US, "%.1f ms over its lowest level, %.1f ms", (level - floorNs) / 1e6, floorNs / 1e6);
            asProbe = false;
        } else if (!floorKnown && steadySeconds >= STEADY_SECONDS && level >= PROBE_REFRESHES * refreshNs) {
            why = String.format(Locale.US, "steady at %.1f refreshes, its own level not known yet", level / (double) refreshNs);
            asProbe = true;
        } else {
            return;
        }
        if (drops >= MAX_DROPS || dropDue || (lastDropNs != 0 && nowNs - lastDropNs < MIN_GAP_NS)) return;
        dropDue = true;
        probe = asProbe;
        dropLevelNs = level;
        event = String.format(Locale.US, "Display queue: wait %.1f ms (%s; refresh %.2f ms): dropping one frame " +
                "so the display gives back a buffer", level / 1e6, why, refreshNs / 1e6);
    }

    /** Whether the frame about to be shown is dropped instead; true once per decision. */
    synchronized boolean takeDrop(long nowNs) {
        if (!enabled || !dropDue) return false;
        dropDue = false;
        drops++;
        lastDropNs = nowNs;
        checking = true;
        checkingProbe = probe;
        checkFromNs = dropLevelNs;
        // The check reads only waits after the drop.
        count = 0;
        medianCount = 0;
        return true;
    }

    /** The line to log about the last decision, once, or null. */
    synchronized String takeEvent() {
        String taken = event;
        event = null;
        return taken;
    }

    private static long median(long[] values, int n) {
        long[] sorted = Arrays.copyOf(values, n);
        Arrays.sort(sorted);
        return sorted[n / 2];
    }
}
