package com.limelight.binding.audio;

/**
 * The microphone's manual gain, applied to each 20 ms frame before it is encoded. Up to the knee the
 * gain is linear; above it a soft limiter bends the curve so a loud voice at high gain rounds off
 * below full scale instead of clipping hard or wrapping around. Plain math, no allocation.
 */
public final class MicGain {
    public static final int MIN_PERCENT = 25, MAX_PERCENT = 400, DEFAULT_PERCENT = 100;
    /** Where the limiter starts, as a fraction of full scale. */
    static final float KNEE = 0.75f;
    private static final float FULL_SCALE = 32767f;

    private MicGain() { }

    public static int clampPercent(int percent) {
        return Math.max(MIN_PERCENT, Math.min(MAX_PERCENT, percent));
    }

    /** Scales {@code samples} of {@code pcm} in place by {@code percent} / 100. 100 changes nothing. */
    public static void apply(short[] pcm, int samples, int percent) {
        percent = clampPercent(percent);
        if (percent == 100) return;
        float gain = percent / 100f;
        int count = Math.min(samples, pcm.length);
        for (int i = 0; i < count; i++) {
            pcm[i] = sample(pcm[i], gain);
        }
    }

    /** One sample through the gain and the limiter. */
    static short sample(short value, float gain) {
        float x = value / FULL_SCALE * gain;
        float magnitude = Math.abs(x);
        if (magnitude > KNEE) {
            // tanh keeps the slope at 1 where it meets the linear part and stays below 1 for any input.
            float over = (magnitude - KNEE) / (1 - KNEE);
            magnitude = KNEE + (1 - KNEE) * (float) Math.tanh(over);
            x = Math.copySign(magnitude, x);
        }
        int out = Math.round(x * FULL_SCALE);
        return (short) Math.max(-32767, Math.min(32767, out));
    }
}
