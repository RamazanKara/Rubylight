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
 */
final class HostFrameClock {
    /** Clock sync probes a second; the host answers each at once. */
    private static final long PROBE_INTERVAL_MS = 250;
    /** Frames the decoder may hold between input and output. */
    static final int PENDING = 64;

    private final long[] codecPts = new long[PENDING];
    private final long[] hostUs = new long[PENDING];
    private int next;
    private final long[] schedule = new long[4];
    private final long[] stats = new long[3];
    private ScheduledExecutorService prober;
    private volatile long lastHoldUs = -1;
    private volatile long lastLatencyUs = -1;

    HostFrameClock() {
        clear();
    }

    /** Starts a clock for a stream on a display whose highest refresh rate is maxRefreshHz. */
    void start(float maxRefreshHz) {
        int minIntervalUs = maxRefreshHz > 0 ? (int) (1_000_000f / maxRefreshHz) : 0;
        MoonBridge.presentClockStart(minIntervalUs);
        clear();
        ScheduledExecutorService executor = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread thread = new Thread(r, "Video - clock sync");
            thread.setDaemon(true);
            return thread;
        });
        // Hosts that did not announce clock sync get no probes (checked natively).
        executor.scheduleWithFixedDelay(MoonBridge::sendClockSyncProbe, PROBE_INTERVAL_MS, PROBE_INTERVAL_MS,
                TimeUnit.MILLISECONDS);
        prober = executor;
    }

    void stop() {
        if (prober != null) {
            prober.shutdownNow();
            prober = null;
        }
        MoonBridge.presentClockStop();
    }

    /** After a decoder flush or restart: queued timestamps are gone, and the delay starts over. */
    void reset() {
        clear();
        MoonBridge.presentClockReset();
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
     * ready at readyNs; readyNs itself when its host time is unknown.
     */
    long presentNs(long codecPtsUs, long readyNs) {
        long host = hostUsFor(codecPtsUs);
        if (host < 0 || !MoonBridge.presentClockSchedule(host, readyNs, schedule)) return readyNs;
        lastHoldUs = schedule[1];
        lastLatencyUs = schedule[2];
        return schedule[0];
    }

    /** One overlay line, or null before the first scheduled frame. */
    String overlayLine() {
        if (!MoonBridge.presentClockStats(stats) || stats[0] == 0) return null;
        StringBuilder line = new StringBuilder(String.format(Locale.US,
                "Host frame times: held %.1f ms | late %.1f%%", lastHoldUs / 1000.0,
                100.0 * stats[1] / stats[0]));
        if (lastLatencyUs >= 0) {
            line.append(String.format(Locale.US, " | host to present %.1f ms", lastLatencyUs / 1000.0));
        }
        return line.toString();
    }
}
