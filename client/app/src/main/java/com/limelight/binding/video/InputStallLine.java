package com.limelight.binding.video;

/**
 * Coalesces slow input-buffer waits into one report line per {@link #INTERVAL_MS}, with their count
 * and worst time: at 20-40 ms each they came hundreds of times a stream and pushed out everything
 * else in the bounded report log. The first wait of a stream gets a line at once.
 */
final class InputStallLine {
    static final long INTERVAL_MS = 10_000;

    private boolean logged;
    private long lastLineMs;
    private int pending;
    private int pendingMaxMs;
    private int total;
    private int totalMaxMs;

    /** Counts a wait of {@code waitedMs} and returns the line to log now, or null. */
    synchronized String onStall(long nowMs, int waitedMs) {
        pending++;
        total++;
        pendingMaxMs = Math.max(pendingMaxMs, waitedMs);
        totalMaxMs = Math.max(totalMaxMs, waitedMs);
        if (logged && nowMs - lastLineMs < INTERVAL_MS) return null;
        logged = true;
        lastLineMs = nowMs;
        return take();
    }

    /** The line for waits not reported yet, or null: a stream's end must not lose its last ones. */
    synchronized String flush() {
        return pending == 0 ? null : take();
    }

    private String take() {
        String line = "Decoder input waits over 20 ms: " + pending + ", worst " + pendingMaxMs + " ms";
        pending = 0;
        pendingMaxMs = 0;
        return line;
    }

    synchronized int total() {
        return total;
    }

    synchronized int totalMaxMs() {
        return totalMaxMs;
    }
}
