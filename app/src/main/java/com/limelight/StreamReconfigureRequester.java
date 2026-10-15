package com.limelight;

// Decides when a running stream asks the host for a new size or frame rate. Window, display and
// fold changes arrive in bursts, so a request waits until nothing has changed for SETTLE_MS, and
// is only sent when what the phone offers has moved since the stream last matched it. A rotation
// that leaves the native size unchanged, or a size that was never the stream's to begin with,
// sends nothing.
final class StreamReconfigureRequester {
    static final long SETTLE_MS = 300;

    private int width, height, fpsMillihz;
    private int[] baseline;
    private long settleAt = Long.MIN_VALUE;

    // The stream as it started, and what the phone offered at that moment.
    void start(int width, int height, int fpsMillihz, int offeredWidth, int offeredHeight, int offeredFpsMillihz) {
        this.width = width;
        this.height = height;
        this.fpsMillihz = fpsMillihz;
        baseline = new int[] {offeredWidth, offeredHeight, offeredFpsMillihz};
        settleAt = Long.MIN_VALUE;
    }

    // Something that may change the native size happened; returns the delay before deciding.
    long onChange(long now) {
        return onChange(now, SETTLE_MS);
    }

    // As above, waiting at least settleMs; a longer wait already running is kept.
    long onChange(long now, long settleMs) {
        settleAt = Math.max(settleAt, now + settleMs);
        return settleAt - now;
    }

    boolean settled(long now) {
        return now >= settleAt;
    }

    long remainingMs(long now) {
        return Math.max(0, settleAt - now);
    }

    // What to request for what the phone offers now, or null to leave the stream as it is.
    int[] decide(int offeredWidth, int offeredHeight, int offeredFpsMillihz) {
        if (baseline == null || matches(baseline, offeredWidth, offeredHeight, offeredFpsMillihz)) {
            return null;
        }
        if (offeredWidth == width && offeredHeight == height && offeredFpsMillihz == fpsMillihz) {
            baseline = new int[] {offeredWidth, offeredHeight, offeredFpsMillihz};
            return null;
        }
        return new int[] {offeredWidth, offeredHeight, offeredFpsMillihz};
    }

    // The host was asked for this; a request that could not be sent is not recorded, so the next
    // change tries again.
    void sent(int[] request) {
        width = request[0];
        height = request[1];
        fpsMillihz = request[2];
        baseline = request.clone();
    }

    int width() {
        return width;
    }

    int height() {
        return height;
    }

    int fpsMillihz() {
        return fpsMillihz;
    }

    private static boolean matches(int[] a, int width, int height, int fpsMillihz) {
        return a[0] == width && a[1] == height && a[2] == fpsMillihz;
    }
}
