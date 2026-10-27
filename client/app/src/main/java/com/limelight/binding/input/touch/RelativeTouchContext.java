package com.limelight.binding.input.touch;

import android.os.Handler;
import android.os.Looper;
import android.view.View;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;

public class RelativeTouchContext implements TouchContext {
    private int lastTouchX = 0;
    private int lastTouchY = 0;
    private int originalTouchX = 0;
    private int originalTouchY = 0;
    private long originalTouchTime = 0;
    private long lastMoveTime = 0;
    private boolean cancelled;
    private boolean confirmedMove;
    private boolean confirmedDrag;
    private boolean confirmedScroll;
    private boolean pendingButtonUp;
    private double distanceMoved;
    private float xFactor, yFactor;
    private final MouseDeltaAccumulator deltaXAccumulator = new MouseDeltaAccumulator();
    private final MouseDeltaAccumulator deltaYAccumulator = new MouseDeltaAccumulator();
    private int pointerCount;
    private int maxPointerCountInGesture;

    private final NvConnection conn;
    private final int actionIndex;
    private final int referenceWidth;
    private final int referenceHeight;
    private final View targetView;
    private final PreferenceConfiguration prefConfig;
    private final TrackpadPointer pointer;
    private final float density;
    private final Handler handler;

    private final Runnable dragTimerRunnable = new Runnable() {
        @Override
        public void run() {
            // Check if someone already set move
            if (confirmedMove) {
                return;
            }

            // The drag should only be processed for the primary finger
            if (actionIndex != maxPointerCountInGesture - 1) {
                return;
            }

            // We haven't been cancelled before the timer expired so begin dragging
            confirmedDrag = true;
            releasePendingButtonUp();
            conn.sendMouseButtonDown(getMouseButtonIndex());
        }
    };

    private final Runnable buttonUpRunnable = new Runnable() {
        @Override
        public void run() {
            pendingButtonUp = false;
            conn.sendMouseButtonUp(getMouseButtonIndex());
        }
    };

    private static final int DRAG_TIME_THRESHOLD = 650;

    private static final int SCROLL_SPEED_FACTOR = 5;

    public RelativeTouchContext(NvConnection conn, int actionIndex,
                                int referenceWidth, int referenceHeight,
                                View view, PreferenceConfiguration prefConfig, TrackpadPointer pointer)
    {
        this.conn = conn;
        this.actionIndex = actionIndex;
        this.referenceWidth = referenceWidth;
        this.referenceHeight = referenceHeight;
        this.targetView = view;
        this.prefConfig = prefConfig;
        this.pointer = pointer;
        this.density = view.getResources().getDisplayMetrics().density;
        this.handler = new Handler(Looper.getMainLooper());
    }

    @Override
    public int getActionIndex()
    {
        return actionIndex;
    }

    private boolean isWithinTapBounds(int touchX, int touchY)
    {
        int xDelta = Math.abs(touchX - originalTouchX);
        int yDelta = Math.abs(touchY - originalTouchY);
        return xDelta <= TwoFingerTapGesture.MOVEMENT_THRESHOLD_DP * density &&
                yDelta <= TwoFingerTapGesture.MOVEMENT_THRESHOLD_DP * density;
    }

    private boolean isTap(long eventTime)
    {
        if (confirmedDrag || confirmedMove || confirmedScroll) {
            return false;
        }

        // Multi-finger taps belong to the whole gesture, not a pointer index that can
        // change when another finger lifts. Game recognizes those independently.
        if (actionIndex != 0 || maxPointerCountInGesture != 1) {
            return false;
        }

        long timeDelta = eventTime - originalTouchTime;
        return isWithinTapBounds(lastTouchX, lastTouchY) &&
                timeDelta >= 0 && timeDelta <= TwoFingerTapGesture.TIME_LIMIT_MS;
    }

    private byte getMouseButtonIndex()
    {
        if (actionIndex == 1) {
            return MouseButtonPacket.BUTTON_RIGHT;
        }
        else {
            return MouseButtonPacket.BUTTON_LEFT;
        }
    }

    @Override
    public boolean touchDownEvent(int eventX, int eventY, long eventTime, boolean isNewFinger)
    {
        // Get the view dimensions to scale inputs on this touch
        float speed = (pointer.isLocalCursor() ? prefConfig.localCursorSpeed : prefConfig.trackpadSpeed) / 100f;
        if (pointer.isLocalCursor()) {
            // Under desktop zoom the drawn pointer keeps pace with the finger on screen.
            speed /= pointer.zoomScale();
        }
        xFactor = referenceWidth / (float)targetView.getWidth() * speed;
        yFactor = referenceHeight / (float)targetView.getHeight() * speed;

        originalTouchX = lastTouchX = eventX;
        originalTouchY = lastTouchY = eventY;
        lastMoveTime = eventTime;

        if (isNewFinger) {
            maxPointerCountInGesture = pointerCount;
            originalTouchTime = eventTime;
            cancelled = confirmedDrag = confirmedMove = confirmedScroll = false;
            distanceMoved = 0;

            if (actionIndex == 0) {
                pointer.touchDown();
                // Start the timer for engaging a drag
                startDragTimer();
            }
        }

        return true;
    }

    @Override
    public void touchUpEvent(int eventX, int eventY, long eventTime)
    {
        if (cancelled) {
            return;
        }

        // Cancel the drag timer
        cancelDragTimer();

        // A lift can carry the last movement even if there was no ACTION_MOVE.
        checkForConfirmedMove(eventX, eventY);

        byte buttonIndex = getMouseButtonIndex();

        if (confirmedDrag) {
            // Raise the button after a drag
            conn.sendMouseButtonUp(buttonIndex);
            confirmedDrag = false;
        }
        else if (actionIndex == 0 && confirmedMove && !confirmedScroll && maxPointerCountInGesture == 1) {
            // A flick keeps the pointer gliding when momentum is on.
            pointer.release(eventTime);
        }
        else if (isTap(eventTime))
        {
            releasePendingButtonUp();
            // Lower the mouse button
            conn.sendMouseButtonDown(buttonIndex);

            // Release the mouse button in 100ms to allow for apps that use polling
            // to detect mouse button presses.
            pendingButtonUp = true;
            handler.postDelayed(buttonUpRunnable, 100);
        }
    }

    private void startDragTimer() {
        cancelDragTimer();
        handler.postDelayed(dragTimerRunnable, DRAG_TIME_THRESHOLD);
    }

    private void cancelDragTimer() {
        handler.removeCallbacks(dragTimerRunnable);
    }

    private void releasePendingButtonUp() {
        if (pendingButtonUp) {
            handler.removeCallbacks(buttonUpRunnable);
            buttonUpRunnable.run();
        }
    }

    private void checkForConfirmedMove(int eventX, int eventY) {
        // If we've already confirmed something, get out now
        if (confirmedMove || confirmedDrag) {
            return;
        }

        // If it leaves the tap bounds before the drag time expires, it's a move.
        if (!isWithinTapBounds(eventX, eventY)) {
            confirmedMove = true;
            cancelDragTimer();
            return;
        }

        // Check if we've exceeded the maximum distance moved
        distanceMoved += Math.sqrt(Math.pow(eventX - lastTouchX, 2) + Math.pow(eventY - lastTouchY, 2));
        if (distanceMoved >= TwoFingerTapGesture.DISTANCE_THRESHOLD_DP * density) {
            confirmedMove = true;
            cancelDragTimer();
            return;
        }
    }

    private void checkForConfirmedScroll() {
        // Enter scrolling mode if we've already left the tap zone
        // and we have 2 fingers on screen. Leave scroll mode if
        // we no longer have 2 fingers on screen
        confirmedScroll = (actionIndex == 0 && pointerCount == 2 && confirmedMove);
    }

    @Override
    public boolean touchMoveEvent(int eventX, int eventY, long eventTime)
    {
        if (cancelled) {
            return true;
        }

        if (eventX != lastTouchX || eventY != lastTouchY)
        {
            checkForConfirmedMove(eventX, eventY);
            checkForConfirmedScroll();

            // We only send moves and drags for the primary touch point
            if (actionIndex == 0) {
                float gain = 1;
                PointerAcceleration acceleration = pointer.acceleration();
                if (acceleration != null && pointerCount != 2) {
                    gain = acceleration.gain(PointerAcceleration.speed(eventX - lastTouchX, eventY - lastTouchY,
                            eventTime - lastMoveTime, density));
                }
                int deltaX = deltaXAccumulator.scale(eventX - lastTouchX, xFactor * gain);
                int deltaY = deltaYAccumulator.scale(eventY - lastTouchY, yFactor * gain);

                if (pointerCount == 2) {
                    if (confirmedScroll) {
                        conn.sendMouseHighResScroll((short)(deltaY * SCROLL_SPEED_FACTOR));
                    }
                } else {
                    if (deltaX != 0 || deltaY != 0) {
                        pointer.moveBy(deltaX, deltaY);
                    }
                    pointer.track(deltaX, deltaY, eventTime);
                }

            }
            lastMoveTime = eventTime;
            lastTouchX = eventX;
            lastTouchY = eventY;
        }

        return true;
    }

    @Override
    public void cancelTouch() {
        if (cancelled) {
            return;
        }
        cancelled = true;

        // Cancel the drag timer
        cancelDragTimer();

        // If it was a confirmed drag, we'll need to raise the button now
        releasePendingButtonUp();
        if (confirmedDrag) {
            conn.sendMouseButtonUp(getMouseButtonIndex());
            confirmedDrag = false;
        }
    }

    @Override
    public boolean isCancelled() {
        return cancelled;
    }

    @Override
    public void setPointerCount(int pointerCount) {
        this.pointerCount = pointerCount;

        if (pointerCount > maxPointerCountInGesture) {
            maxPointerCountInGesture = pointerCount;
        }
    }
}
