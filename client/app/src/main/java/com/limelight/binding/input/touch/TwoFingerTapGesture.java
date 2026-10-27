package com.limelight.binding.input.touch;

import android.view.MotionEvent;

/** Tracks both fingers by ID until the whole tap ends, including motion batched with a lift. */
public final class TwoFingerTapGesture {
    static final int MOVEMENT_THRESHOLD_DP = 20;
    static final int DISTANCE_THRESHOLD_DP = 25;
    static final int TIME_LIMIT_MS = 250;

    private final int[] ids = new int[2];
    private final float[] startX = new float[2], startY = new float[2];
    private final float[] lastX = new float[2], lastY = new float[2], distance = new float[2];
    private int fingers;
    private boolean active, invalid;
    private long downTime;
    private float density;

    public boolean isPossible() { return active && !invalid; }
    public void invalidate() { invalid = true; }
    public void reset() { active = false; invalid = false; fingers = 0; }

    /** Returns true only on the final lift of an uncanceled, exactly two-finger tap. */
    public boolean onTouch(MotionEvent event, float screenDensity) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) {
            reset();
            active = true;
            density = screenDensity;
            downTime = event.getEventTime();
            addFinger(event, 0);
        }
        if (!active) return false;
        if (action == MotionEvent.ACTION_CANCEL) {
            reset();
            return false;
        }
        long elapsed = event.getEventTime() - downTime;
        if (elapsed < 0 || elapsed > TIME_LIMIT_MS || event.getPointerCount() > 2 ||
                (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) invalid = true;

        if (action == MotionEvent.ACTION_POINTER_DOWN) {
            // Even a replacement finger makes this a different gesture, not a two-finger tap.
            if (fingers == 1 && event.getPointerCount() == 2) addFinger(event, event.getActionIndex());
            else invalid = true;
        }
        for (int finger = 0; finger < fingers; finger++) {
            int index = event.findPointerIndex(ids[finger]);
            if (index < 0) continue; // This finger already lifted.
            for (int history = 0; history < event.getHistorySize(); history++) {
                move(finger, event.getHistoricalX(index, history), event.getHistoricalY(index, history));
            }
            move(finger, event.getX(index), event.getY(index));
        }
        if (action == MotionEvent.ACTION_UP) {
            boolean tapped = !invalid && fingers == 2;
            reset();
            return tapped;
        }
        return false;
    }

    private void addFinger(MotionEvent event, int index) {
        ids[fingers] = event.getPointerId(index);
        startX[fingers] = lastX[fingers] = event.getX(index);
        startY[fingers] = lastY[fingers] = event.getY(index);
        distance[fingers] = 0;
        fingers++;
    }

    private void move(int finger, float x, float y) {
        distance[finger] += (float) Math.hypot(x - lastX[finger], y - lastY[finger]);
        if (Math.abs(x - startX[finger]) > MOVEMENT_THRESHOLD_DP * density ||
                Math.abs(y - startY[finger]) > MOVEMENT_THRESHOLD_DP * density ||
                distance[finger] >= DISTANCE_THRESHOLD_DP * density) invalid = true;
        lastX[finger] = x;
        lastY[finger] = y;
    }
}
