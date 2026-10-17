package com.limelight.binding.input.touch;

/**
 * Recognises a quick three-finger tap: exactly three fingers down, lifted within
 * {@link #TIME_LIMIT_MS} of the third landing, without the first finger travelling far.
 */
public final class ThreeFingerTap {
    public static final String KEYBOARD = FingerTapGesture.KEYBOARD;
    public static final String MIDDLE_CLICK = FingerTapGesture.MIDDLE_CLICK;
    public static final long TIME_LIMIT_MS = FingerTapGesture.TIME_LIMIT_MS;
    private final FingerTapGesture gesture = new FingerTapGesture();

    /**
     * The third finger landed at downTime while the first finger, pointerId, was at (x, y). Travel is
     * measured on that finger, not on whichever one holds pointer index 0 when the first has lifted.
     */
    public void start(long downTime, int pointerId, float x, float y) {
        gesture.start(downTime, pointerId, x, y);
    }

    public boolean isStarted() {
        return gesture.isStarted();
    }

    public int pointerId() {
        return gesture.pointerId();
    }

    public void pointers(int count) {
        gesture.pointers(count);
    }

    /** The tracked finger moved to (x, y). */
    public void move(float x, float y) {
        gesture.move(x, y);
    }

    /** All fingers lifted at upTime; true when this was a tap. Resets either way. */
    public boolean finish(long upTime, float slop) {
        return gesture.finish(upTime, slop) == 3;
    }

    public void reset() {
        gesture.reset();
    }
}
