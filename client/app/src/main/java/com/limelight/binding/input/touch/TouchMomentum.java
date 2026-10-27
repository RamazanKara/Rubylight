package com.limelight.binding.input.touch;

/**
 * Fling momentum for the trackpad: tracks the pointer velocity while a finger moves, and after a
 * quick release keeps the pointer gliding with exponential friction until it is slow enough to stop.
 * Velocities are in output pixels per millisecond; nothing here allocates.
 */
public final class TouchMomentum {
    public static final String SHORT = "short";
    public static final String MEDIUM = "medium";
    public static final String LONG = "long";

    // A finger resting this long before it lifts does not fling.
    static final long REST_MS = 40;
    static final float MIN_FLING_SPEED = 0.5f;
    static final float STOP_SPEED = 0.03f;
    static final float MAX_SPEED = 20f;
    static final long MAX_GLIDE_MS = 3000;
    // A late frame (or a debugger pause) moves at most this much time's worth.
    static final long MAX_STEP_MS = 50;

    private final float timeConstantMs;
    private float vx, vy;
    private long lastSampleTime = -1;
    private float pendingX, pendingY;
    private boolean active;
    private boolean clockStarted;
    private long lastStepTime;
    private long glideMs;
    private float remainderX, remainderY;
    private int stepX, stepY;

    public TouchMomentum(String glide) {
        timeConstantMs = SHORT.equals(glide) ? 110 : LONG.equals(glide) ? 420 : 220;
    }

    public float timeConstantMs() {
        return timeConstantMs;
    }

    /** Ends any glide and forgets the tracked velocity. */
    public void stop() {
        active = false;
        vx = vy = 0;
        lastSampleTime = -1;
        pendingX = pendingY = 0;
        remainderX = remainderY = 0;
        stepX = stepY = 0;
    }

    public boolean isActive() {
        return active;
    }

    /** Records a pointer move of (dx, dy) output pixels at timeMs. */
    public void track(float dx, float dy, long timeMs) {
        if (active) stop();
        if (lastSampleTime < 0) {
            lastSampleTime = timeMs;
            pendingX = pendingY = 0;
            vx = vy = 0;
            return;
        }
        long elapsed = timeMs - lastSampleTime;
        if (elapsed <= 0) {
            // Samples with the same timestamp add up into the next measurable interval.
            pendingX += dx;
            pendingY += dy;
            return;
        }
        float ix = (dx + pendingX) / elapsed, iy = (dy + pendingY) / elapsed;
        pendingX = pendingY = 0;
        if (elapsed > REST_MS) {
            vx = ix;
            vy = iy;
        } else {
            vx = vx * 0.4f + ix * 0.6f;
            vy = vy * 0.4f + iy * 0.6f;
        }
        lastSampleTime = timeMs;
    }

    /** The finger lifted at timeMs; returns true when a glide starts. */
    public boolean release(long timeMs) {
        boolean recent = lastSampleTime >= 0 && timeMs - lastSampleTime <= REST_MS;
        float speed = (float) Math.sqrt(vx * vx + vy * vy);
        lastSampleTime = -1;
        pendingX = pendingY = 0;
        if (!recent || !(speed >= MIN_FLING_SPEED)) {
            stop();
            return false;
        }
        if (speed > MAX_SPEED) {
            vx *= MAX_SPEED / speed;
            vy *= MAX_SPEED / speed;
        }
        active = true;
        clockStarted = false;
        glideMs = 0;
        remainderX = remainderY = 0;
        return true;
    }

    /**
     * Advances the glide to frameTimeMs and leaves the whole-pixel movement in {@link #stepX()} and
     * {@link #stepY()}. Returns false once the glide has ended; the last step may still move.
     */
    public boolean step(long frameTimeMs) {
        stepX = stepY = 0;
        if (!active) return false;
        if (!clockStarted) {
            clockStarted = true;
            lastStepTime = frameTimeMs;
            return true;
        }
        long elapsed = Math.max(0, Math.min(MAX_STEP_MS, frameTimeMs - lastStepTime));
        lastStepTime = frameTimeMs;
        glideMs += elapsed;
        float decay = (float) Math.exp(-elapsed / timeConstantMs);
        // Distance covered while the speed decays over this interval.
        float travel = timeConstantMs * (1 - decay);
        float x = vx * travel + remainderX, y = vy * travel + remainderY;
        stepX = Math.round(x);
        stepY = Math.round(y);
        remainderX = x - stepX;
        remainderY = y - stepY;
        vx *= decay;
        vy *= decay;
        if (vx * vx + vy * vy < STOP_SPEED * STOP_SPEED || glideMs >= MAX_GLIDE_MS) {
            active = false;
            vx = vy = 0;
            remainderX = remainderY = 0;
        }
        return active;
    }

    public int stepX() {
        return stepX;
    }

    public int stepY() {
        return stepY;
    }
}
