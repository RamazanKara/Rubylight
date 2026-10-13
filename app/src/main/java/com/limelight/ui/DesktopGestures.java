package com.limelight.ui;

import android.content.Context;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;

/** Two-finger pan/pinch never leaks pointer-up events into a canceled host gesture. */
public final class DesktopGestures {
    private final DesktopViewport viewport = new DesktopViewport();
    private final StreamView view;
    private final ScaleGestureDetector detector;
    private final Runnable cancelInput;
    private boolean enabled, consuming;
    private float lastX, lastY;

    public DesktopGestures(Context context, StreamView view, Runnable cancelInput) {
        this.view = view; this.cancelInput = cancelInput;
        detector = new ScaleGestureDetector(context, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
            @Override public boolean onScale(ScaleGestureDetector d) {
                viewport.zoom(d.getScaleFactor(), lastX, lastY); apply(); return true;
            }
        });
        detector.setQuickScaleEnabled(false);
        view.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            viewport.resize(r - l, b - t); apply();
        });
    }
    public DesktopViewport viewport() { return viewport; }
    public boolean enabled() { return enabled; }
    public void setEnabled(boolean enabled) {
        cancelInput.run(); consuming = false; this.enabled = enabled;
        if (!enabled) reset();
    }
    public void reset() { viewport.reset(); apply(); }
    private void apply() { view.setVideoTransform(viewport.scale(), viewport.x(), viewport.y()); }
    public boolean onTouch(MotionEvent event) {
        if (!enabled || event.getToolType(0) != MotionEvent.TOOL_TYPE_FINGER) return false;
        detector.onTouchEvent(event);
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN) consuming = false;
        if (event.getPointerCount() >= 2) {
            float x = (event.getX(0) + event.getX(1)) / 2;
            float y = (event.getY(0) + event.getY(1)) / 2;
            if (!consuming) { cancelInput.run(); consuming = true; }
            else if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
                viewport.pan(x - lastX, y - lastY); apply();
            }
            lastX = x; lastY = y;
        }
        boolean handled = consuming;
        if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL) consuming = false;
        return handled;
    }
}
