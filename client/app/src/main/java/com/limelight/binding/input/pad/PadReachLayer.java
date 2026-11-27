package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.os.SystemClock;
import android.util.SparseArray;
import android.view.MotionEvent;
import android.view.View;

/**
 * A control's touch area past its drawn box (Nightjar #172): the box grown to the layout's reach, under every control
 * and drawing nothing. Gone at the default reach, where only the box takes touches, as before.
 *
 * <p>A finger that lands in an area works the nearest shown control whose area holds it, so with large areas the
 * control under the finger's side wins, not whichever area lies on top. It works that control as a finger on the box's
 * nearest edge would: a button presses, a trackpad touches at its edge, a stick starts there and follows the finger
 * from it, and a split cluster's member presses and lets the finger slide on to the next member. The pointer touchpad
 * moves by how far the finger goes, wherever it is. Each finger keeps the control it landed on until it lifts.
 */
@SuppressLint("ViewConstructor")
final class PadReachLayer extends View {
    /** Finds the control a finger in an area works. */
    interface Areas {
        /** The shown control nearest the frame's point whose touch area holds it; {@code owner} unless one is nearer. */
        PadControlView nearest(PadControlView owner, float x, float y);
    }

    private final PadControlView owner;
    private Areas areas;
    /** Each finger this area delivered: the control it works. */
    private final SparseArray<PadControlView> targets = new SparseArray<>();
    /** How far a stick's finger was moved onto the box when it landed, per finger; its moves keep the offset. */
    private final SparseArray<float[]> shifts = new SparseArray<>();

    PadReachLayer(Context context, PadControlView owner) {
        super(context);
        this.owner = owner;
        setFocusable(false);
        setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
    }

    void setAreas(Areas areas) {
        this.areas = areas;
    }

    @SuppressLint("ClickableViewAccessibility") // Works its control, which is no click target either.
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                land(event, event.getActionIndex());
                break;
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < event.getPointerCount(); i++) slide(event, i);
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP: {
                int pointer = event.getPointerId(event.getActionIndex());
                PadControlView target = targets.get(pointer);
                forget(pointer);
                if (target != null) target.up(pointer, event.getEventTime());
                break;
            }
            case MotionEvent.ACTION_CANCEL:
                cancel(event.getEventTime());
                break;
            default:
                break;
        }
        return true;
    }

    private void land(MotionEvent event, int index) {
        float x = getLeft() + event.getX(index), y = getTop() + event.getY(index);
        PadControlView target = areas != null ? areas.nearest(owner, x, y) : owner;
        int pointer = event.getPointerId(index);
        targets.put(pointer, target);
        float localX = x - target.getLeft(), localY = y - target.getTop();
        if (target.control.shape == PadLayout.Shape.TOUCHPAD) {
            target.down(pointer, localX, localY, event.getEventTime());
            return;
        }
        float onX = onBox(target.getWidth(), localX), onY = onBox(target.getHeight(), localY);
        if (target.control.shape == PadLayout.Shape.STICK || target.control.shape == PadLayout.Shape.PAD) {
            shifts.put(pointer, new float[] {onX - localX, onY - localY});
        }
        target.down(pointer, onX, onY, event.getEventTime());
    }

    private void slide(MotionEvent event, int index) {
        int pointer = event.getPointerId(index);
        PadControlView target = targets.get(pointer);
        if (target == null) return;
        float dx = getLeft() - target.getLeft(), dy = getTop() - target.getTop();
        if (target.control.shape == PadLayout.Shape.TOUCHPAD) {
            // As on the box: the historical samples carry their own times, so the pointer's speed stays smooth.
            for (int history = 0; history < event.getHistorySize(); history++) {
                target.move(pointer, event.getHistoricalX(index, history) + dx, event.getHistoricalY(index, history) + dy,
                        PadControlView.eventTimeNs(event, history));
            }
            target.move(pointer, event.getX(index) + dx, event.getY(index) + dy, PadControlView.eventTimeNs(event, -1));
            return;
        }
        float x = event.getX(index) + dx, y = event.getY(index) + dy;
        long timeNs = PadControlView.eventTimeNs(event, -1);
        float[] shift = shifts.get(pointer);
        if (shift != null) {
            target.move(pointer, x + shift[0], y + shift[1], timeNs);
            return;
        }
        // A split cluster's finger slides onto another member's box as it does from a box; off every member it works
        // the one it landed on, from that box's nearest edge.
        PadControlView.Group group = target.group();
        if (group != null && group.under(target.getLeft() + x, target.getTop() + y) != 0) {
            target.move(pointer, x, y, timeNs);
            return;
        }
        target.move(pointer, onBox(target.getWidth(), x), onBox(target.getHeight(), y), timeNs);
    }

    /**
     * A cancelled gesture lets go of what this area's fingers held, and only that: a finger on a control's own box
     * stays. Trackpads let go without the click a lifted tap would send.
     */
    private void cancel(long time) {
        drain(time, null);
    }

    /**
     * Clears this area's tracked pointers when its owning control releases, letting go of any pointer that had been
     * routed onto a neighbouring control without recursing back into {@code owner}.
     */
    void releaseOwner(long time) {
        drain(time, owner);
    }

    private void drain(long time, PadControlView skip) {
        if (targets.size() == 0) {
            shifts.clear();
            return;
        }
        int count = targets.size();
        int[] pointers = new int[count];
        PadControlView[] held = new PadControlView[count];
        for (int i = 0; i < count; i++) {
            pointers[i] = targets.keyAt(i);
            held[i] = targets.valueAt(i);
        }
        targets.clear();
        shifts.clear();
        for (int i = count - 1; i >= 0; i--) {
            PadControlView target = held[i];
            if (target == skip) continue;
            PadLayout.Shape shape = target.control.shape;
            if (shape == PadLayout.Shape.STICK || shape == PadLayout.Shape.PAD || shape == PadLayout.Shape.TOUCHPAD) target.release();
            else target.up(pointers[i], time);
        }
    }

    @Override
    public void setVisibility(int visibility) {
        super.setVisibility(visibility);
        if (visibility != VISIBLE) cancel(SystemClock.uptimeMillis());
    }

    @Override
    protected void onVisibilityChanged(View changedView, int visibility) {
        super.onVisibilityChanged(changedView, visibility);
        if (visibility != VISIBLE) cancel(SystemClock.uptimeMillis());
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        cancel(SystemClock.uptimeMillis());
    }

    int trackedPointerCount() {
        return targets.size();
    }

    private void forget(int pointer) {
        targets.remove(pointer);
        shifts.remove(pointer);
    }

    private static float onBox(int size, float at) {
        return Math.max(0, Math.min(size - 1, at));
    }
}
