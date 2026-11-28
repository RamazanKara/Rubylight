package com.limelight.ui;

import android.content.Context;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewParent;

import com.limelight.binding.input.touch.TwoFingerTapGesture;

/** Two-finger pan/pinch tracks active pointer IDs directly and never leaks pointer-up events into a canceled host gesture. */
public final class DesktopGestures {
    private static final float PASSIVE_PINCH_THRESHOLD_DP = 28f;
    private static final float PASSIVE_FOCUS_RATIO = 0.75f;
    private static final float PASSIVE_RELEASE_RESET_LOW = 0.97f;
    private static final float PASSIVE_RELEASE_RESET_HIGH = 1.03f;
    private static final float MIN_SPAN_PX = 1e-3f;

    private final DesktopViewport viewport = new DesktopViewport();
    private final StreamView view;
    private final Runnable cancelInput;
    private final Runnable rightClick;
    private final TwoFingerTapGesture twoFingerTap = new TwoFingerTapGesture();
    private final float density;
    private final View.OnLayoutChangeListener parentLayoutListener = (v, l, t, r, b, ol, ot, or, ob) -> syncBounds();
    private View observedParent;
    private boolean enabled, consuming, tapAllowed, hasBaseline;
    private int pointerIdA = MotionEvent.INVALID_POINTER_ID;
    private int pointerIdB = MotionEvent.INVALID_POINTER_ID;
    private float anchorSpan, anchorFocusX, anchorFocusY;
    private float prevSpan, prevFocusX, prevFocusY;
    private Runnable onTransformChanged;

    public DesktopGestures(Context context, StreamView view, Runnable cancelInput, Runnable rightClick) {
        this.view = view;
        this.cancelInput = cancelInput;
        this.rightClick = rightClick;
        density = context.getResources().getDisplayMetrics().density;
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

    public void syncBounds() {
        int vw = view.getWidth(), vh = view.getHeight();
        if (vw <= 0 || vh <= 0) return;
        ViewParent rawParent = view.getParent();
        if (rawParent instanceof View) {
            View parent = (View) rawParent;
            int cw = parent.getWidth() - parent.getPaddingLeft() - parent.getPaddingRight();
            int ch = parent.getHeight() - parent.getPaddingTop() - parent.getPaddingBottom();
            if (cw > 0 && ch > 0) {
                viewport.setBounds(vw, vh, cw, ch,
                        view.getLeft() + view.pictureOffsetX() - parent.getPaddingLeft(),
                        view.getTop() + Math.round(view.pictureOffsetY() + view.keyboardPan()) - parent.getPaddingTop());
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
        cancelInput.run();
        cancelTouch();
        consuming = false;
        this.enabled = enabled;
        if (!enabled) reset();
    }

    /** Invalidate a pending tap on focus loss, mode changes, or an overlay taking input. */
    public void cancelTouch() {
        twoFingerTap.reset();
        tapAllowed = false;
        clearPair();
    }

    public void reset() {
        viewport.reset();
        apply();
    }

    private void apply() {
        view.setVideoTransform(viewport.scale(), viewport.x(), viewport.y());
        if (onTransformChanged != null) onTransformChanged.run();
    }

    private void clearPair() {
        pointerIdA = MotionEvent.INVALID_POINTER_ID;
        pointerIdB = MotionEvent.INVALID_POINTER_ID;
        hasBaseline = false;
    }

    private void capturePair(MotionEvent event, int ignoreIndex) {
        int idxA = -1, idxB = -1;
        int count = event.getPointerCount();
        for (int i = 0; i < count; i++) {
            if (i == ignoreIndex) continue;
            if (idxA < 0) idxA = i;
            else {
                idxB = i;
                break;
            }
        }
        if (idxA < 0 || idxB < 0) {
            clearPair();
            return;
        }
        pointerIdA = event.getPointerId(idxA);
        pointerIdB = event.getPointerId(idxB);
        float ax = event.getX(idxA), ay = event.getY(idxA);
        float bx = event.getX(idxB), by = event.getY(idxB);
        float span = (float) Math.hypot(ax - bx, ay - by);
        float focusX = (ax + bx) * 0.5f;
        float focusY = (ay + by) * 0.5f;
        anchorSpan = span;
        anchorFocusX = focusX;
        anchorFocusY = focusY;
        prevSpan = span;
        prevFocusX = focusX;
        prevFocusY = focusY;
        hasBaseline = true;
    }

    private void softResetIfNearUnity() {
        float s = viewport.scale();
        if (!enabled && viewport.isTransformed()
                && s >= PASSIVE_RELEASE_RESET_LOW && s <= PASSIVE_RELEASE_RESET_HIGH) {
            viewport.reset();
            apply();
        }
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
            clearPair();
        }
        boolean tapped = twoFingerTap.onTouch(event, density);

        boolean wasConsuming = consuming;
        if (action == MotionEvent.ACTION_POINTER_DOWN) {
            if (!hasBaseline && event.getPointerCount() >= 2) {
                capturePair(event, -1);
                boolean passiveWaitAtOneX = allowScrollThrough && !enabled && !viewport.isTransformed();
                if (passiveWaitAtOneX) {
                    consuming = false;
                } else if (!consuming) {
                    cancelInput.run();
                    consuming = true;
                }
            }
        } else if (action == MotionEvent.ACTION_POINTER_UP) {
            int liftedIndex = event.getActionIndex();
            int liftedId = event.getPointerId(liftedIndex);
            if (event.getPointerCount() - 1 >= 2) {
                if (!hasBaseline || liftedId == pointerIdA || liftedId == pointerIdB) {
                    capturePair(event, liftedIndex);
                }
            } else {
                clearPair();
                softResetIfNearUnity();
                if (!enabled && !viewport.isTransformed()) {
                    consuming = false;
                }
            }
        } else if (action == MotionEvent.ACTION_MOVE && event.getPointerCount() >= 2) {
            int idxA = hasBaseline ? event.findPointerIndex(pointerIdA) : -1;
            int idxB = hasBaseline ? event.findPointerIndex(pointerIdB) : -1;
            if (idxA < 0 || idxB < 0) {
                capturePair(event, -1);
                idxA = hasBaseline ? event.findPointerIndex(pointerIdA) : -1;
                idxB = hasBaseline ? event.findPointerIndex(pointerIdB) : -1;
            }
            if (idxA >= 0 && idxB >= 0) {
                float ax = event.getX(idxA), ay = event.getY(idxA);
                float bx = event.getX(idxB), by = event.getY(idxB);
                float span = (float) Math.hypot(ax - bx, ay - by);
                float focusX = (ax + bx) * 0.5f;
                float focusY = (ay + by) * 0.5f;
                if (!consuming) {
                    float dSpan = Math.abs(span - anchorSpan);
                    float focusTravel = (float) Math.hypot(focusX - anchorFocusX, focusY - anchorFocusY);
                    boolean pinchDiverged = dSpan >= PASSIVE_PINCH_THRESHOLD_DP * density
                            && dSpan > focusTravel * PASSIVE_FOCUS_RATIO;
                    boolean passiveWaitAtOneX = allowScrollThrough && !enabled && !viewport.isTransformed();
                    if (!passiveWaitAtOneX || pinchDiverged) {
                        cancelInput.run();
                        consuming = true;
                        twoFingerTap.invalidate();
                        // Re-baseline when passive pinch threshold latches so the first move causes zero start jump.
                        if (pinchDiverged && passiveWaitAtOneX) {
                            prevSpan = span;
                            prevFocusX = focusX;
                            prevFocusY = focusY;
                            return true;
                        }
                    }
                }
                if (consuming && !twoFingerTap.isPossible()) {
                    float factor = (prevSpan > MIN_SPAN_PX && span > MIN_SPAN_PX) ? span / prevSpan : 1.0f;
                    viewport.zoomAndPan(factor, prevFocusX, prevFocusY, focusX - prevFocusX, focusY - prevFocusY);
                    apply();
                }
                prevSpan = span;
                prevFocusX = focusX;
                prevFocusY = focusY;
            }
        }

        boolean handled = consuming || (action == MotionEvent.ACTION_POINTER_UP && wasConsuming);
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
            softResetIfNearUnity();
            consuming = false;
            clearPair();
            if (handled && tapped && tapAllowed) rightClick.run();
            tapAllowed = false;
        }
        return handled;
    }
}
