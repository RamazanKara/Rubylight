package com.limelight.ui;

import android.content.Context;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.View;
import android.view.ViewParent;

import com.limelight.binding.input.touch.TwoFingerTapGesture;

/** Two-finger pan/pinch never leaks pointer-up events into a canceled host gesture. */
public final class DesktopGestures {
    private static final float PASSIVE_PINCH_THRESHOLD_DP = 28f;

    private final DesktopViewport viewport = new DesktopViewport();
    private final StreamView view;
    private final ScaleGestureDetector detector;
    private final Runnable cancelInput;
    private final Runnable rightClick;
    private final TwoFingerTapGesture twoFingerTap = new TwoFingerTapGesture();
    private final float density;
    private final View.OnLayoutChangeListener parentLayoutListener = (v, l, t, r, b, ol, ot, or, ob) -> syncBounds();
    private View observedParent;
    private boolean enabled, consuming, tapAllowed, hasInitialSpan, passiveReset, passiveMoved;
    private int passiveDirection;
    private float lastX, lastY, initialSpan;
    private Runnable onTransformChanged;

    public DesktopGestures(Context context, StreamView view, Runnable cancelInput, Runnable rightClick) {
        this.view = view; this.cancelInput = cancelInput; this.rightClick = rightClick;
        density = context.getResources().getDisplayMetrics().density;
        detector = new ScaleGestureDetector(context, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
            @Override public boolean onScaleBegin(ScaleGestureDetector d) {
                twoFingerTap.invalidate();
                return true;
            }
            @Override public boolean onScale(ScaleGestureDetector d) {
                if (!consuming) {
                    if (!enabled && (!hasInitialSpan || Math.abs(d.getCurrentSpan() - initialSpan) < PASSIVE_PINCH_THRESHOLD_DP * density)) {
                        return false;
                    }
                    startConsuming(d.getFocusX(), d.getFocusY());
                }
                if (enabled) {
                    viewport.zoom(d.getScaleFactor(), d.getFocusX(), d.getFocusY());
                } else {
                    if (passiveReset) return true;
                    latchPassiveDirection(d.getCurrentSpan());
                    float minScale = passiveDirection > 0 ? 1.0f : DesktopViewport.MIN_SCALE;
                    float maxScale = passiveDirection < 0 ? 1.0f : DesktopViewport.MAX_SCALE;
                    float factor = d.getScaleFactor();
                    viewport.zoom(factor, d.getFocusX(), d.getFocusY(), minScale, maxScale);
                    float scale = viewport.scale();
                    if (passiveDirection > 0) {
                        if (scale > 1.001f) passiveMoved = true;
                        else if (passiveMoved || factor < 1.0f) {
                            viewport.reset();
                            passiveReset = true;
                        }
                    } else if (passiveDirection < 0) {
                        if (scale < 0.999f) passiveMoved = true;
                        else if (passiveMoved || factor > 1.0f) {
                            viewport.reset();
                            passiveReset = true;
                        }
                    }
                }
                apply();
                return true;
            }
        });
        detector.setQuickScaleEnabled(false);
        view.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            attachParentListener();
            syncBounds();
        });
        view.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(View v) {
                attachParentListener();
                syncBounds();
            }
            @Override public void onViewDetachedFromWindow(View v) {
                if (observedParent != null) {
                    observedParent.removeOnLayoutChangeListener(parentLayoutListener);
                    observedParent = null;
                }
            }
        });
        attachParentListener();
        syncBounds();
    }

    private void attachParentListener() {
        ViewParent rawParent = view.getParent();
        View parent = rawParent instanceof View ? (View) rawParent : null;
        if (parent == observedParent) return;
        if (observedParent != null) observedParent.removeOnLayoutChangeListener(parentLayoutListener);
        observedParent = parent;
        if (observedParent != null) observedParent.addOnLayoutChangeListener(parentLayoutListener);
    }

    private void syncBounds() {
        int vw = view.getWidth(), vh = view.getHeight();
        if (vw <= 0 || vh <= 0) return;
        ViewParent rawParent = view.getParent();
        if (rawParent instanceof View) {
            View parent = (View) rawParent;
            int cw = parent.getWidth() - parent.getPaddingLeft() - parent.getPaddingRight();
            int ch = parent.getHeight() - parent.getPaddingTop() - parent.getPaddingBottom();
            if (cw > 0 && ch > 0) {
                viewport.setBounds(vw, vh, cw, ch,
                        view.getLeft() - parent.getPaddingLeft(),
                        view.getTop() - parent.getPaddingTop());
                apply();
                return;
            }
        }
        viewport.resize(vw, vh);
        apply();
    }

    public DesktopViewport viewport() { return viewport; }
    public boolean enabled() { return enabled; }
    public void setOnTransformChanged(Runnable listener) { this.onTransformChanged = listener; }
    public void setEnabled(boolean enabled) {
        cancelInput.run(); cancelTouch(); consuming = false; this.enabled = enabled;
        if (!enabled) reset();
    }
    /** Invalidate a pending tap on focus loss, mode changes, or an overlay taking input. */
    public void cancelTouch() {
        twoFingerTap.reset(); tapAllowed = false; hasInitialSpan = false;
        passiveDirection = 0; passiveReset = false; passiveMoved = false;
    }
    public void reset() { viewport.reset(); apply(); }
    private void apply() {
        view.setVideoTransform(viewport.scale(), viewport.x(), viewport.y());
        if (onTransformChanged != null) onTransformChanged.run();
    }

    private void latchPassiveDirection(float span) {
        if (passiveDirection != 0) return;
        if (viewport.scale() > 1.001f) passiveDirection = 1;
        else if (viewport.scale() < 0.999f) passiveDirection = -1;
        else if (hasInitialSpan && span != initialSpan) passiveDirection = span > initialSpan ? 1 : -1;
    }

    private void startConsuming(float x, float y) {
        cancelInput.run();
        consuming = true;
        lastX = x;
        lastY = y;
    }

    private static float fingerSpan(MotionEvent event) {
        return (float) Math.hypot(event.getX(0) - event.getX(1), event.getY(0) - event.getY(1));
    }

    public boolean onTouch(MotionEvent event, boolean allowRightClick) {
        return onTouch(event, allowRightClick, false);
    }

    public boolean onTouch(MotionEvent event, boolean allowRightClick, boolean allowScrollThrough) {
        if ((!enabled && !allowScrollThrough) || event.getToolType(0) != MotionEvent.TOOL_TYPE_FINGER) return false;
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) {
            consuming = false;
            tapAllowed = allowRightClick;
            hasInitialSpan = false;
            passiveDirection = 0;
            passiveReset = false;
            passiveMoved = false;
        }
        boolean tapped = twoFingerTap.onTouch(event, density);
        if (event.getPointerCount() >= 2 && (action == MotionEvent.ACTION_POINTER_DOWN || !hasInitialSpan)) {
            initialSpan = fingerSpan(event);
            hasInitialSpan = true;
            if (!consuming) {
                passiveDirection = viewport.scale() > 1.001f ? 1 : viewport.scale() < 0.999f ? -1 : 0;
                passiveMoved = passiveDirection != 0;
                passiveReset = false;
            }
        } else if (event.getPointerCount() < 2) {
            hasInitialSpan = false;
        }
        detector.onTouchEvent(event);
        if (event.getPointerCount() >= 2) {
            float x = (event.getX(0) + event.getX(1)) / 2;
            float y = (event.getY(0) + event.getY(1)) / 2;
            boolean passiveWaitAtOneX = allowScrollThrough && !enabled && !viewport.isTransformed();
            if (!consuming) {
                boolean pinchDiverged = detector.isInProgress() && hasInitialSpan
                        && Math.abs(fingerSpan(event) - initialSpan) >= PASSIVE_PINCH_THRESHOLD_DP * density;
                if (!passiveWaitAtOneX || pinchDiverged) {
                    startConsuming(x, y);
                    if (!enabled && pinchDiverged) latchPassiveDirection(fingerSpan(event));
                }
            }
            else if (action == MotionEvent.ACTION_MOVE && !twoFingerTap.isPossible()
                    && (enabled || (!passiveReset && viewport.scale() != 1.0f))) {
                viewport.pan(x - lastX, y - lastY); apply();
                lastX = x; lastY = y;
            }
            else {
                lastX = x; lastY = y;
            }
        }
        boolean handled = consuming;
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
            if (!enabled && viewport.isTransformed() && viewport.scale() >= 0.999f && viewport.scale() <= 1.001f) {
                viewport.reset();
                apply();
            }
            consuming = false;
            hasInitialSpan = false;
            passiveDirection = 0;
            passiveReset = false;
            passiveMoved = false;
            if (handled && tapped && tapAllowed) rightClick.run();
            tapAllowed = false;
        }
        return handled;
    }
}
