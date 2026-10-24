package com.limelight.binding.audio;

final class AudioBufferPolicy {
    // Mirrors AudioTrack.PERFORMANCE_MODE_*; the constants only exist from Android 8.0.
    static final int PERF_NONE = 0, PERF_LOW_LATENCY = 1, PERF_POWER_SAVING = 2;
    // Growth room kept above the floor, in steps.
    private static final int HEADROOM_STEPS = 6;

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

    /** AudioTrack policy: never below floor, grows by step up to HEADROOM_STEPS above it (within capacity). */
    static AudioBufferPolicy forTrack(int step, int capacity, int floor, int initial, int underruns, long nowMs) {
        int minimum = Math.min(capacity, floor);
        return new AudioBufferPolicy(step, minimum, initial, underruns, nowMs,
                Math.min(capacity, minimum + HEADROOM_STEPS * step));
    }

    /**
     * Smallest buffer, in frames, the path an AudioTrack really got can run without underrunning.
     * A fast (low latency) track needs a couple of packets or output bursts. A track on the normal
     * mixer needs the framework's own minimum, which the platform sizes to the mixer period.
     */
    static int trackFloorFrames(int sdk, int perfMode, boolean lowLatencyRequested, int packetFrames,
                                int outputFramesPerBuffer, int minBufferFrames) {
        int floor = packetFrames * 2;
        if (trackIsFast(sdk, perfMode, lowLatencyRequested)) {
            return Math.max(floor, outputFramesPerBuffer > 0 ? outputFramesPerBuffer * 2 : 0);
        }
        return Math.max(floor, minBufferFrames);
    }

    // Before Android 8.0 the granted mode cannot be read; a low latency request is taken as granted.
    static boolean trackIsFast(int sdk, int perfMode, boolean lowLatencyRequested) {
        return sdk >= 26 ? perfMode == PERF_LOW_LATENCY : lowLatencyRequested;
    }

    /** Growth step: a packet on a fast track, at least the output burst on the normal mixer. */
    static int trackStepFrames(boolean fast, int packetFrames, int outputFramesPerBuffer) {
        return fast ? packetFrames : Math.max(packetFrames, outputFramesPerBuffer);
    }

    /** Capacity to request: 8 packets as before, or the framework minimum plus 4 packets, in whole packets. */
    static int trackCapacityFrames(int packetFrames, int minBufferFrames) {
        int wanted = Math.max(packetFrames * 8, minBufferFrames + packetFrames * 4);
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

    boolean needsFallback() {
        return limitUnderruns >= 3;
    }

    static boolean useAAudio(int sdk, int channels, boolean effects, int sampleRate, int nativeRate) {
        // Android 8.0's first AAudio implementation and surround/effects retain AudioTrack.
        return sdk >= 27 && channels == 2 && !effects && sampleRate == nativeRate;
    }
}
