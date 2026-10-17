package com.limelight.binding.input.touch;

/** Recognises quick taps with exactly three or four fingers, independently of their lift order. */
public final class FingerTapGesture {
    public static final String KEYBOARD = "keyboard";
    public static final String MIDDLE_CLICK = "middle_click";
    public static final String DESKTOP_CONTROLS = "desktop_controls";
    public static final String NONE = "none";
    public static final long TIME_LIMIT_MS = 300;

    private boolean started, invalid;
    private long downTime;
    private int pointerId, maxPointers;
    private float startX, startY, travel;

    /** Start when the third finger lands. A fourth finger must share this tap's time and travel limits. */
    public void start(long time, int firstPointerId, float x, float y) {
        started = true;
        invalid = false;
        downTime = time;
        pointerId = firstPointerId;
        startX = x;
        startY = y;
        travel = 0;
        maxPointers = 3;
    }

    public boolean isStarted() { return started; }
    public int pointerId() { return pointerId; }

    public void pointers(int count) {
        if (started) maxPointers = Math.max(maxPointers, count);
    }

    /** Track the original finger by ID, including historical and final positions. */
    public void move(float x, float y) {
        if (started) travel = Math.max(travel, Math.max(Math.abs(x - startX), Math.abs(y - startY)));
    }

    /** Keep consuming a canceled sequence until all fingers lift, without triggering an action. */
    public void invalidate() { invalid = true; }

    /** Return 3, 4, or 0 for an invalid tap. Resets in every case. */
    public int finish(long time, float slop) {
        long duration = time - downTime;
        int fingers = started && !invalid && maxPointers >= 3 && maxPointers <= 4 &&
                duration >= 0 && duration < TIME_LIMIT_MS && travel <= slop ? maxPointers : 0;
        reset();
        return fingers;
    }

    public void reset() {
        started = false;
        invalid = false;
        travel = 0;
        maxPointers = 0;
    }
}
