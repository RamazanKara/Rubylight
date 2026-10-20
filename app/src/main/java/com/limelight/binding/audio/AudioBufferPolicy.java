package com.limelight.binding.audio;

final class AudioBufferPolicy {
    private final int burst;
    private final int minimum;
    private final int maximum;
    private int frames;
    private int lastUnderruns;
    private long lastSampleMs;
    private long cleanSinceMs;
    private int limitUnderruns;

    AudioBufferPolicy(int burst, int capacity, int initial, int underruns, long nowMs) {
        this.burst = burst;
        minimum = Math.min(capacity, Math.max(burst * 2, initial));
        maximum = Math.max(minimum, Math.min(capacity / burst, 8) * burst);
        frames = Math.max(minimum, Math.min(maximum, initial));
        lastUnderruns = underruns;
        lastSampleMs = cleanSinceMs = nowMs;
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
