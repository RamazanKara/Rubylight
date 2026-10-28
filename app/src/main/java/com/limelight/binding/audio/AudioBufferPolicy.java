package com.limelight.binding.audio;

final class AudioBufferPolicy {
    // Mirrors AudioTrack.PERFORMANCE_MODE_*; the constants only exist from Android 8.0.
    static final int PERF_NONE = 0, PERF_LOW_LATENCY = 1, PERF_POWER_SAVING = 2;
    // Growth room kept above the floor, in steps.
    private static final int HEADROOM_STEPS = 6;
    // Audio may wait this long beyond the output's reserve before a decoded packet is dropped.
    static final int LATE_BOUND_MS = 40;
    // The most a reserve grows for network jitter. The TCL C6K's Wi-Fi stalls for up to 130 ms every
    // 1.2 s; its AAudio callback takes 18 ms at a time on top of that.
    static final int MAX_RESERVE_MS = 200;

    private final int burst;
    private final int minimum;
    private final int maximum;
    private int frames;
    private int lastUnderruns;
    private long lastSampleMs;
    private long cleanSinceMs;
    private int limitUnderruns;

    AudioBufferPolicy(int burst, int capacity, int initial, int underruns, long nowMs) {
        this(burst, Math.min(capacity, Math.max(burst * 2, initial)), initial, underruns, nowMs,
                Math.min(capacity / burst, 8) * burst);
    }

    private AudioBufferPolicy(int burst, int minimum, int initial, int underruns, long nowMs, int limit) {
        this.burst = burst;
        this.minimum = minimum;
        maximum = Math.max(minimum, limit);
        frames = Math.max(minimum, Math.min(maximum, initial));
        lastUnderruns = underruns;
        lastSampleMs = cleanSinceMs = nowMs;
    }

    /**
     * AudioTrack policy: never below floor, grows by step up to HEADROOM_STEPS above it, or up to
     * maxReserveFrames where the granted capacity holds that much.
     */
    static AudioBufferPolicy forTrack(int step, int capacity, int floor, int initial, int underruns, long nowMs,
                                      int maxReserveFrames) {
        int minimum = Math.min(capacity, floor);
        return new AudioBufferPolicy(step, minimum, initial, underruns, nowMs,
                Math.min(capacity, Math.max(minimum + HEADROOM_STEPS * step, maxReserveFrames)));
    }

    /**
     * AAudio jitter reserve in the ring ahead of the stream: two bursts, growing a burst at a time on
     * starvation up to eight bursts or MAX_RESERVE_MS, whichever is more, within the ring.
     */
    static AudioBufferPolicy forReserve(int burst, int capacity, int initial, int underruns, long nowMs,
                                        int sampleRate) {
        int limit = Math.max(8 * burst, maxReserveFrames(sampleRate) / burst * burst);
        return new AudioBufferPolicy(burst, Math.min(capacity, Math.max(burst * 2, initial)), initial, underruns,
                nowMs, Math.min(capacity / burst * burst, limit));
    }

    static int maxReserveFrames(int sampleRate) {
        return Math.max(0, sampleRate) / 1000 * MAX_RESERVE_MS;
    }

    /**
     * Whether a decoded packet arrived too late to play. The bound is on all audio waiting to be heard:
     * the network queue plus what the output holds, LATE_BOUND_MS above the output's reserve. A reserve
     * that a network stall drained refills from the burst after it; only the excess above it is dropped.
     * With the output full this is the old 40 ms bound on the network queue alone.
     */
    static boolean dropLate(int pendingMs, int queuedFrames, int reserveFrames, int sampleRate) {
        if (sampleRate <= 0) return pendingMs >= LATE_BOUND_MS;
        long queuedMs = Math.max(0, queuedFrames) * 1000L / sampleRate;
        long reserveMs = Math.max(0, reserveFrames) * 1000L / sampleRate;
        return pendingMs + queuedMs >= reserveMs + LATE_BOUND_MS;
    }

    /**
     * Smallest buffer, in frames, the path an AudioTrack really got can run without underrunning.
     * A fast (low latency) track keeps the two-packet floor it always had, but holds at least one
     * output burst and a packet: a TV mixes 864 frames at a time, which a 480-frame buffer never has.
     * A track on the normal mixer needs the framework's own minimum, which the platform sizes to the
     * mixer period.
     */
    static int trackFloorFrames(int sdk, int perfMode, boolean lowLatencyRequested, int packetFrames,
                                int minBufferFrames, int outputFramesPerBuffer) {
        int floor = packetFrames * 2;
        return trackIsFast(sdk, perfMode, lowLatencyRequested) ?
                Math.max(floor, Math.max(0, outputFramesPerBuffer) + packetFrames) : Math.max(floor, minBufferFrames);
    }

    // Before Android 8.0 the granted mode cannot be read; a low latency request is taken as granted.
    static boolean trackIsFast(int sdk, int perfMode, boolean lowLatencyRequested) {
        return sdk >= 26 ? perfMode == PERF_LOW_LATENCY : lowLatencyRequested;
    }

    /** Growth step: a packet on a fast track, at least the output burst on the normal mixer. */
    static int trackStepFrames(boolean fast, int packetFrames, int outputFramesPerBuffer) {
        return fast ? packetFrames : Math.max(packetFrames, outputFramesPerBuffer);
    }

    /**
     * Capacity to request before the granted path is known: 8 packets as before, or enough for the
     * normal-mixer floor plus its growth steps, in whole packets.
     */
    static int trackCapacityFrames(int packetFrames, int minBufferFrames, int outputFramesPerBuffer) {
        int floor = Math.max(packetFrames * 2, minBufferFrames);
        int step = trackStepFrames(false, packetFrames, outputFramesPerBuffer);
        int wanted = Math.max(packetFrames * 8, floor + HEADROOM_STEPS * step);
        return (wanted + packetFrames - 1) / packetFrames * packetFrames;
    }

    /** AudioManager's frames-per-buffer property, 0 when missing or not a positive number. */
    static int parseFrames(String value) {
        if (value == null) return 0;
        try {
            return Math.max(0, Integer.parseInt(value.trim()));
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    static String perfModeName(int perfMode) {
        switch (perfMode) {
            case PERF_LOW_LATENCY: return "lowLatency";
            case PERF_NONE: return "none";
            case PERF_POWER_SAVING: return "powerSaving";
            default: return "unknown";
        }
    }

    int sample(int underruns, long nowMs) {
        if (nowMs - lastSampleMs < 1000) return frames;
        lastSampleMs = nowMs;
        if (underruns < 0 || lastUnderruns < 0 || underruns < lastUnderruns) {
            limitUnderruns = 0;
            cleanSinceMs = nowMs;
        } else if (underruns > lastUnderruns) {
            if (frames >= maximum) limitUnderruns++;
            frames = Math.min(maximum, frames + burst);
            cleanSinceMs = nowMs;
        } else if (nowMs - cleanSinceMs >= 30000) {
            limitUnderruns = 0;
            frames = Math.max(minimum, frames - burst);
            cleanSinceMs = nowMs;
        }
        lastUnderruns = underruns;
        return frames;
    }

    void applied(int actualFrames) {
        if (actualFrames > 0) frames = actualFrames;
    }

    /** Starved in three samples at the limit: for a device buffer the output is broken. */
    boolean needsFallback() {
        return limitUnderruns >= 3;
    }

    int frames() {
        return frames;
    }

    int limit() {
        return maximum;
    }

    static boolean useAAudio(int sdk, int channels, boolean effects, int sampleRate, int nativeRate) {
        // Android 8.0's first AAudio implementation and surround/effects retain AudioTrack.
        return sdk >= 27 && channels == 2 && !effects && sampleRate == nativeRate;
    }
}
