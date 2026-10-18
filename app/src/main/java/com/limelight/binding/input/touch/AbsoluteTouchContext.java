package com.limelight.binding.input.touch;

import android.os.Handler;
import android.os.Looper;
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.MouseButtonPacket;

import java.util.function.DoubleSupplier;

public class AbsoluteTouchContext implements TouchContext {
    private int lastTouchDownX = 0;
    private int lastTouchDownY = 0;
    private long lastTouchDownTime = 0;
    private int lastTouchUpX = 0;
    private int lastTouchUpY = 0;
    private long lastTouchUpTime = 0;
    private int lastTouchLocationX = 0;
    private int lastTouchLocationY = 0;
    private boolean cancelled;
    private boolean confirmedLongPress;
    private boolean confirmedTap;
    private boolean pendingLeftButtonUp;
    private boolean lastTouchWasTap;

    private final Runnable longPressRunnable = new Runnable() {
        @Override
        public void run() {
            if (cancelled || confirmedTap || confirmedLongPress) return;
            // No left press is sent while deciding between a tap, a drag and a hold.
            updatePosition(lastTouchDownX, lastTouchDownY);
            confirmedLongPress = true;
            lastTouchWasTap = false;
            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
        }
    };

    private final NvConnection conn;
    private final int actionIndex;
    private final View targetView;
    private final Handler handler;
    private final float density;
    private final DoubleSupplier coordinateScale;

    private final Runnable leftButtonUpRunnable = new Runnable() {
        @Override
        public void run() {
            pendingLeftButtonUp = false;
            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
        }
    };

    private static final int SCROLL_SPEED_FACTOR = 3;

    private static final int LONG_PRESS_TIME_THRESHOLD = 650;
    private static final int LONG_PRESS_DISTANCE_THRESHOLD_DP = 30;

    private static final int DOUBLE_TAP_TIME_THRESHOLD = 250;
    private static final int DOUBLE_TAP_DISTANCE_THRESHOLD_DP = 60;

    public AbsoluteTouchContext(NvConnection conn, int actionIndex, View view)
    {
        this(conn, actionIndex, view, () -> 1.0);
    }

    /** coordinateScale converts inverse-zoomed positions back to physical screen distances. */
    public AbsoluteTouchContext(NvConnection conn, int actionIndex, View view, DoubleSupplier coordinateScale)
    {
        this.conn = conn;
        this.actionIndex = actionIndex;
        this.targetView = view;
        this.handler = new Handler(Looper.getMainLooper());
        this.density = view.getResources().getDisplayMetrics().density;
        this.coordinateScale = coordinateScale;
    }

    @Override
    public int getActionIndex()
    {
        return actionIndex;
    }

    @Override
    public boolean touchDownEvent(int eventX, int eventY, long eventTime, boolean isNewFinger)
    {
        if (!isNewFinger) {
            // We don't handle finger transitions for absolute mode
            return true;
        }

        lastTouchLocationX = lastTouchDownX = eventX;
        lastTouchLocationY = lastTouchDownY = eventY;
        lastTouchDownTime = eventTime;
        cancelled = confirmedTap = confirmedLongPress = false;

        if (actionIndex == 0) {
            releasePendingLeftButtonUp();
            startLongPressTimer();
        }

        return true;
    }

    private boolean distanceExceeds(int deltaX, int deltaY, double limitDp) {
        return Math.hypot(deltaX, deltaY) * coordinateScale.getAsDouble() > limitDp * density;
    }

    private void updatePosition(int eventX, int eventY) {
        // We may get values slightly outside our view region on ACTION_HOVER_ENTER and ACTION_HOVER_EXIT.
        // Normalize these to the view size. We can't just drop them because we won't always get an event
        // right at the boundary of the view, so dropping them would result in our cursor never really
        // reaching the sides of the screen.
        eventX = Math.min(Math.max(eventX, 0), targetView.getWidth());
        eventY = Math.min(Math.max(eventY, 0), targetView.getHeight());

        conn.sendMousePosition((short)eventX, (short)eventY, (short)targetView.getWidth(), (short)targetView.getHeight());
    }

    @Override
    public void touchUpEvent(int eventX, int eventY, long eventTime)
    {
        if (cancelled) {
            return;
        }

        if (actionIndex == 0) {
            // Include final movement so a release outside the hold bounds starts a drag,
            // even when Android did not deliver a separate move event.
            if (!confirmedLongPress && !confirmedTap && distanceExceeds(
                    eventX - lastTouchDownX, eventY - lastTouchDownY, LONG_PRESS_DISTANCE_THRESHOLD_DP)) {
                touchMoveEvent(eventX, eventY, eventTime);
            }
            cancelLongPressTimer();

            // Raise the mouse buttons that we currently have down
            if (confirmedLongPress) {
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                confirmedLongPress = false;
            }
            else if (confirmedTap) {
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                confirmedTap = false;
            }
            else {
                // A short stationary touch clicks only on release, at its original position.
                tapConfirmed();
                confirmedTap = false;
                lastTouchWasTap = true;

                // Release the left mouse button in 100ms to allow for apps that use polling
                // to detect mouse button presses.
                handler.removeCallbacks(leftButtonUpRunnable);
                pendingLeftButtonUp = true;
                handler.postDelayed(leftButtonUpRunnable, 100);
            }
        }

        lastTouchLocationX = lastTouchUpX = eventX;
        lastTouchLocationY = lastTouchUpY = eventY;
        lastTouchUpTime = eventTime;
    }

    private void startLongPressTimer() {
        cancelLongPressTimer();
        handler.postDelayed(longPressRunnable, LONG_PRESS_TIME_THRESHOLD);
    }

    private void cancelLongPressTimer() {
        handler.removeCallbacks(longPressRunnable);
    }

    private void tapConfirmed() {
        if (confirmedTap || confirmedLongPress) {
            return;
        }

        confirmedTap = true;
        cancelLongPressTimer();
        releasePendingLeftButtonUp();

        // Left button down at original position
        if (!lastTouchWasTap || lastTouchDownTime - lastTouchUpTime > DOUBLE_TAP_TIME_THRESHOLD ||
                distanceExceeds(lastTouchDownX - lastTouchUpX, lastTouchDownY - lastTouchUpY, DOUBLE_TAP_DISTANCE_THRESHOLD_DP)) {
            // Don't reposition for finger down events within the deadzone. This makes double-clicking easier.
            updatePosition(lastTouchDownX, lastTouchDownY);
        }
        lastTouchWasTap = false;
        conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
    }

    private void releasePendingLeftButtonUp() {
        if (pendingLeftButtonUp) {
            handler.removeCallbacks(leftButtonUpRunnable);
            leftButtonUpRunnable.run();
        }
    }

    @Override
    public boolean touchMoveEvent(int eventX, int eventY, long eventTime)
    {
        if (cancelled) {
            return true;
        }

        if (actionIndex == 0) {
            if (confirmedLongPress) {
                updatePosition(eventX, eventY);
            }
            else if (confirmedTap || distanceExceeds(eventX - lastTouchDownX, eventY - lastTouchDownY,
                    LONG_PRESS_DISTANCE_THRESHOLD_DP)) {
                tapConfirmed();
                updatePosition(eventX, eventY);
            }
        }
        else if (actionIndex == 1) {
            conn.sendMouseHighResScroll((short)((eventY - lastTouchLocationY) * SCROLL_SPEED_FACTOR));
        }

        lastTouchLocationX = eventX;
        lastTouchLocationY = eventY;

        return true;
    }

    @Override
    public void cancelTouch() {
        if (cancelled) {
            return;
        }
        cancelled = true;

        cancelLongPressTimer();
        releasePendingLeftButtonUp();

        // Raise the mouse buttons
        if (confirmedLongPress) {
            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
        }
        else if (confirmedTap) {
            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
        }
        confirmedLongPress = confirmedTap = lastTouchWasTap = false;
    }

    @Override
    public boolean isCancelled() {
        return cancelled;
    }

    @Override
    public void setPointerCount(int pointerCount) {
        if (actionIndex == 0 && pointerCount > 1) {
            cancelTouch();
        }
    }
}
