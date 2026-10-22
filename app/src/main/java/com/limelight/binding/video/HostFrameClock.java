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
    };

    private final Bridge bridge;
    private final long probeIntervalMs;
    private final long[] codecPts = new long[PENDING];
    private final long[] hostUs = new long[PENDING];
    private int next;
    private final long[] schedule = new long[4];
    private final long[] stats = new long[3];
    private ScheduledExecutorService prober;
    private volatile boolean scheduling;
    private volatile boolean started;
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
        if (!schedule && !probe) return;
        int minIntervalUs = maxRefreshHz > 0 ? (int) (1_000_000f / maxRefreshHz) : 0;
        scheduling = schedule;
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
    synchronized void onQueued(long codecPtsUs, long frameHostUs) {
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
