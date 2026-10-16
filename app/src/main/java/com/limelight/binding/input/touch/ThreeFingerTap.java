package com.limelight.binding.input.touch;

/**
 * Recognises a quick three-finger tap: exactly three fingers down, lifted within
 * {@link #TIME_LIMIT_MS} of the third landing, without the first finger travelling far.
 */
public final class ThreeFingerTap {
    public static final String KEYBOARD = "keyboard";
    public static final String MIDDLE_CLICK = "middle_click";

    public static final long TIME_LIMIT_MS = 300;

    private boolean started;
    private long downTime;
    private int pointerId;
    private float startX, startY;
    private float travel;
    private int maxPointers;

    /**
     * The third finger landed at downTime while the first finger, pointerId, was at (x, y). Travel is
     * measured on that finger, not on whichever one holds pointer index 0 when the first has lifted.
     */
    public void start(long downTime, int pointerId, float x, float y) {
        started = true;
        this.downTime = downTime;
        this.pointerId = pointerId;
        startX = x;
        startY = y;
        travel = 0;
        maxPointers = 3;
    }

    public boolean isStarted() {
        return started;
    }

    public int pointerId() {
        return pointerId;
    }

    public void pointers(int count) {
        if (started && count > maxPointers) maxPointers = count;
    }

    /** The tracked finger moved to (x, y). */
    public void move(float x, float y) {
        if (!started) return;
        float distance = Math.max(Math.abs(x - startX), Math.abs(y - startY));
        if (distance > travel) travel = distance;
    }

    /** All fingers lifted at upTime; true when this was a tap. Resets either way. */
    public boolean finish(long upTime, float slop) {
        boolean tap = started && maxPointers == 3 && upTime - downTime < TIME_LIMIT_MS && travel <= slop;
        reset();
        return tap;
    }

    public void reset() {
        started = false;
        travel = 0;
        maxPointers = 0;
    }
}
