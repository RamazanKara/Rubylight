package com.limelight.binding.video;

import com.limelight.nvstream.jni.MoonBridge;

import java.util.Locale;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * Shows each decoded frame at the host's frame time plus a small delay that covers recent jitter
 * (protocol/core/src/present_clock.rs), so the screen keeps the game's frame times instead of the
 * network's. With adaptive refresh, the display refreshes at those times.
 *
 * MediaCodec carries its own input timestamps, so the host timestamp of each queued frame is kept
 * here until the decoder hands the frame back.
 *
 * The clock also runs, without scheduling anything, when only the clock sync is wanted: with
 * host frame times off, a host that announced clock sync is still probed so the timing CSV can
 * carry the clock offset. Then [presentNs] returns the ready time unchanged.
 *
 * On a panel with a fixed refresh rate, [presentOnGrid] moves the same schedule onto the
 * display's vsync grid: it names the vsync a frame is meant for and when to release it so the
 * compositor takes it for that vsync. [contentRateMilliHz] is the rate the game presents at,
 * from the host timestamps of the frames passed to [onQueued].
 */
final class HostFrameClock {
    /** Clock sync probes four times a second; the host answers each at once. */
    static final long PROBE_INTERVAL_MS = 250;
    /** Frames the decoder may hold between input and output. */
    static final int PENDING = 64;

    /** The native side: MoonBridge in the app, a fake in tests (JNI is not available there). */
    interface Bridge {
        void start(int minIntervalUs);
        void stop();
        void reset();
        boolean schedule(long hostUs, long readyNs, long[] out);
        boolean stats(long[] out);
        /** The clock offset and round trip (microseconds) into out[0..1]; false before an exchange. */
        boolean offset(long[] out);
        /** Sends a clock sync probe when the host announced clock sync. */
        void probe();
        /**
         * As [schedule], onto a vsync grid: the vsync times are anchorNs plus whole multiples of
         * intervalNs (System.nanoTime()), the frame is released leadNs before its vsync, and a
         * vsync up to slackNs before the schedule's present time may be taken. Fills out[0..6]:
         * release time (ns), the vsync (ns), its index from the anchor, flags (bit 0 late, bit 1
         * shares the previous frame's vsync), hold (us), host-to-vsync latency (us, -1 without
         * clock sync), and how late the frame was (us).
         */
        boolean scheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
                long leadNs, long slackNs, long[] out);
        /** Notes a frame's host time (us) for the content rate; call for every frame in order. */
        void observe(long hostUs);
        /** The content rate in millihertz once it held steady for two seconds, else 0. */
        int contentRate();
    }

    /** Where one frame goes on the panel's vsync grid; see [presentOnGrid]. */
    static final class GridPlan {
        /** System.nanoTime() to release the frame at; never before it is ready. */
        long releaseNs;
        /** The vsync the frame is meant for, and its index from the anchor passed in. */
        long vsyncNs;
        long vsyncIndex;
        /** The frame was ready after the host time plus the delay: behind the game's cadence. */
        boolean late;
        long lateByUs;
        /** Same vsync as the frame scheduled before it: content faster than the panel. */
        boolean sharesVsync;
        long holdUs;
        /** Host present to this vsync in us, -1 without clock sync. */
        long latencyUs;
    }

    private static final Bridge NATIVE = new Bridge() {
        @Override public void start(int minIntervalUs) { MoonBridge.presentClockStart(minIntervalUs); }
        @Override public void stop() { MoonBridge.presentClockStop(); }
        @Override public void reset() { MoonBridge.presentClockReset(); }
        @Override public boolean schedule(long hostUs, long readyNs, long[] out) {
            return MoonBridge.presentClockSchedule(hostUs, readyNs, out);
        }
        @Override public boolean stats(long[] out) { return MoonBridge.presentClockStats(out); }
        @Override public boolean offset(long[] out) { return MoonBridge.presentClockOffset(out); }
        @Override public void probe() { MoonBridge.sendClockSyncProbe(); }
        @Override public boolean scheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
                long leadNs, long slackNs, long[] out) {
            return MoonBridge.presentClockScheduleOnGrid(hostUs, readyNs, anchorNs, intervalNs, leadNs,
                    slackNs, out);
        }
        @Override public void observe(long hostUs) { MoonBridge.presentClockObserve(hostUs); }
        @Override public int contentRate() { return MoonBridge.presentClockContentRate(); }
    };

    private final Bridge bridge;
    private final long probeIntervalMs;
    private final long[] codecPts = new long[PENDING];
    private final long[] hostUs = new long[PENDING];
    private int next;
    private final long[] schedule = new long[4];
    private final long[] grid = new long[7];
    private final long[] stats = new long[3];
    private ScheduledExecutorService prober;
    private volatile boolean scheduling;
    private volatile boolean started;
    private volatile boolean trackingRate;
    private volatile long lastHoldUs = -1;
    private volatile long lastLatencyUs = -1;

    HostFrameClock() {
        this(NATIVE, PROBE_INTERVAL_MS);
    }

    HostFrameClock(Bridge bridge, long probeIntervalMs) {
        this.bridge = bridge;
        this.probeIntervalMs = probeIntervalMs;
        clear();
    }

    /**
     * Starts a clock for a stream on a display whose highest refresh rate is maxRefreshHz. It
     * schedules frames when schedule is true and sends clock sync probes when probe is true (the
     * host announced clock sync); with neither it does not start at all.
     */
    void start(float maxRefreshHz, boolean schedule, boolean probe) {
        start(maxRefreshHz, schedule, probe, false);
    }

    /** As above; trackRate also follows the content rate (see [contentRateMilliHz]) on its own. */
    void start(float maxRefreshHz, boolean schedule, boolean probe, boolean trackRate) {
        if (!schedule && !probe && !trackRate) return;
        int minIntervalUs = maxRefreshHz > 0 ? (int) (1_000_000f / maxRefreshHz) : 0;
        scheduling = schedule;
        trackingRate = trackRate;
        bridge.start(minIntervalUs);
        started = true;
        clear();
        if (!probe) return;
        ScheduledExecutorService executor = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread thread = new Thread(r, "Video - clock sync");
            thread.setDaemon(true);
            return thread;
        });
        executor.scheduleWithFixedDelay(bridge::probe, probeIntervalMs, probeIntervalMs,
                TimeUnit.MILLISECONDS);
        prober = executor;
    }

    void stop() {
        if (prober != null) {
            prober.shutdownNow();
            try {
                // A probe in flight must not outlive the native clock it feeds.
                prober.awaitTermination(200, TimeUnit.MILLISECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            prober = null;
        }
        scheduling = false;
        trackingRate = false;
        if (started) {
            started = false;
            bridge.stop();
        }
    }

    /**
     * The offset from the host's video clock to System.nanoTime()'s clock (client = host + offset)
     * and the round trip of its clock sync exchange, both in microseconds, into out[0..1]; false
     * while the clock is not running or no exchange has happened.
     */
    boolean clockOffset(long[] out) {
        return started && bridge.offset(out);
    }

    /** After a decoder flush or restart: queued timestamps are gone, and the delay starts over. */
    void reset() {
        clear();
        bridge.reset();
    }

    private synchronized void clear() {
        java.util.Arrays.fill(codecPts, Long.MIN_VALUE);
        next = 0;
    }

    /** Remembers the host timestamp of the frame queued with the codec timestamp codecPtsUs. */
    void onQueued(long codecPtsUs, long frameHostUs) {
        remember(codecPtsUs, frameHostUs);
        if (trackingRate && frameHostUs >= 0) bridge.observe(frameHostUs);
    }

    private synchronized void remember(long codecPtsUs, long frameHostUs) {
        codecPts[next] = codecPtsUs;
        hostUs[next] = frameHostUs;
        next = (next + 1) % PENDING;
    }

    /** The host timestamp of a decoded frame, or -1 when it is not known. */
    synchronized long hostUsFor(long codecPtsUs) {
        for (int i = 1; i <= PENDING; i++) {
            int at = (next - i + PENDING) % PENDING;
            if (codecPts[at] == codecPtsUs) return hostUs[at];
        }
        return -1;
    }

    /**
     * The System.nanoTime() at which to release the decoded frame with codec timestamp codecPtsUs,
     * ready at readyNs; readyNs itself when its host time is unknown or the clock only probes.
     */
    long presentNs(long codecPtsUs, long readyNs) {
        if (!scheduling) return readyNs;
        long host = hostUsFor(codecPtsUs);
        if (host < 0 || !bridge.schedule(host, readyNs, schedule)) return readyNs;
        lastHoldUs = schedule[1];
        lastLatencyUs = schedule[2];
        return schedule[0];
    }

    /**
     * Plans the decoded frame with codec timestamp codecPtsUs, ready at readyNs, onto a display
     * whose vsyncs are at anchorNs plus whole multiples of intervalNs: fills out and returns true.
     * The frame is released at out.releaseNs for out.vsyncNs; the plan never holds a frame past
     * the first vsync it can make by more than one vsync, and only to keep the game's cadence.
     * Returns false, leaving out alone, when frames are not scheduled, the interval is not
     * positive or the frame's host time is unknown: release it as without host frame times.
     * leadNs is how long before its vsync a frame is released (the compositor's latch deadline
     * and a margin); slackNs lets a vsync that much before the schedule's present time be taken
     * (zero rounds up to the next vsync, half an interval rounds to the nearest).
     */
    boolean presentOnGrid(long codecPtsUs, long readyNs, long anchorNs, long intervalNs, long leadNs,
            long slackNs, GridPlan out) {
        if (!scheduling || intervalNs <= 0) return false;
        long host = hostUsFor(codecPtsUs);
        if (host < 0 || !bridge.scheduleOnGrid(host, readyNs, anchorNs, intervalNs, leadNs, slackNs, grid)) {
            return false;
        }
        out.releaseNs = grid[0];
        out.vsyncNs = grid[1];
        out.vsyncIndex = grid[2];
        out.late = (grid[3] & 1) != 0;
        out.sharesVsync = (grid[3] & 2) != 0;
        out.holdUs = grid[4];
        out.latencyUs = grid[5];
        out.lateByUs = grid[6];
        lastHoldUs = out.holdUs;
        lastLatencyUs = out.latencyUs;
        return true;
    }

    /**
     * The rate the game presents at, in millihertz (60000 for 60 fps), once it held steady for
     * two seconds and moved only after a new rate did; 0 before that, when the clock was not
     * started with trackRate, or after it stopped. Fed by [onQueued].
     */
    int contentRateMilliHz() {
        return trackingRate ? bridge.contentRate() : 0;
    }

    /** One overlay line, or null before the first scheduled frame. */
    String overlayLine() {
        if (!scheduling || !bridge.stats(stats) || stats[0] == 0) return null;
        StringBuilder line = new StringBuilder(String.format(Locale.US,
                "Host frame times: held %.1f ms | late %.1f%%", lastHoldUs / 1000.0,
                100.0 * stats[1] / stats[0]));
        if (lastLatencyUs >= 0) {
            line.append(String.format(Locale.US, " | host to present %.1f ms", lastLatencyUs / 1000.0));
        }
        return line.toString();
    }
}
