package com.limelight.binding.input.touch;

/**
 * Trackpad pointer acceleration: slow finger movement is scaled down for precision and fast
 * movement is scaled up, with a smooth ramp in between. Speeds are in dp per millisecond.
 */
public final class PointerAcceleration {
    public static final String GENTLE = "gentle";
    public static final String MEDIUM = "medium";
    public static final String STRONG = "strong";

    static final float SLOW_SPEED = 0.05f;
    static final float FAST_SPEED = 1.5f;
    // Event times are whole milliseconds; shorter gaps would turn rounding into huge speeds.
    private static final float MIN_SAMPLE_MS = 4f;

    private final float slowGain;
    private final float fastGain;

    private PointerAcceleration(float slowGain, float fastGain) {
        this.slowGain = slowGain;
        this.fastGain = fastGain;
    }

    public static PointerAcceleration forCurve(String curve) {
        if (GENTLE.equals(curve)) return new PointerAcceleration(0.8f, 1.6f);
        if (STRONG.equals(curve)) return new PointerAcceleration(0.5f, 3.0f);
        return new PointerAcceleration(0.6f, 2.2f);
    }

    public float gain(float speed) {
        if (!(speed > SLOW_SPEED)) return slowGain;
        if (speed >= FAST_SPEED) return fastGain;
        float t = (speed - SLOW_SPEED) / (FAST_SPEED - SLOW_SPEED);
        return slowGain + (fastGain - slowGain) * t * t * (3 - 2 * t);
    }

    /** Finger speed in dp/ms for a move of (dx, dy) pixels over elapsedMs. */
    public static float speed(float dx, float dy, long elapsedMs, float density) {
        if (!(density > 0)) density = 1;
        float distance = (float) Math.sqrt(dx * dx + dy * dy) / density;
        return distance / Math.max(MIN_SAMPLE_MS, elapsedMs);
    }
}
