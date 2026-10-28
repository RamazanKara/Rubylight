package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.TouchLayout;

/**
 * One control of the modern on-screen controller. Each control is its own view so a finger that
 * misses every control reaches the stream underneath, while several fingers on different controls
 * each go to their own. Sticks follow one finger; the face cluster and the D-pad follow every finger
 * on them, so a thumb can slide from A to B or rest between them for both.
 */
@SuppressLint("ViewConstructor") // Created by ModernController for one control.
final class PadControlView extends View {
    private static final int MAX_POINTERS = 4;

    final PadLayout.Control control;
    private final PadPainter painter;
    private final LayoutGamepad gamepad;
    private final Runnable changed;
    private final PadPainter.State state = new PadPainter.State();
    private LayoutStick stick;
    private final int[] pointerIds = new int[MAX_POINTERS], pointerFlags = new int[MAX_POINTERS];
    private int stickPointer = -1;
    // A stick tap (short, barely pushed) arms its click for the next touch; a flick does not.
    private long stickReleasedAt, stickDownAt;
    private float stickPush;
    private boolean stickClick, haptics = true;
    private float restingAlpha = 1;

    PadControlView(Context context, PadLayout.Control control, PadPainter painter, LayoutGamepad gamepad,
                   Runnable changed) {
        super(context);
        this.control = control;
        this.painter = painter;
        this.gamepad = gamepad;
        this.changed = changed;
        setFocusable(false);
        setHapticFeedbackEnabled(true);
        java.util.Arrays.fill(pointerIds, -1);
        configureStick(true, .08f);
    }

    void configureStick(boolean floating, float deadzone) {
        stick = new LayoutStick(floating, Math.max(0, Math.min(.9f, deadzone)));
    }

    void setHaptics(boolean haptics) {
        this.haptics = haptics;
    }

    /** The alpha at rest; a press shows at least 60 % so a faint control still answers visibly. */
    void setRestingAlpha(float alpha) {
        restingAlpha = alpha;
        applyAlpha();
    }

    private void applyAlpha() {
        boolean active = state.flags != 0 || state.pressed || state.stickActive;
        setAlpha(active ? Math.max(restingAlpha, .6f) : restingAlpha);
    }

    private int slot() {
        return control.ordinal();
    }

    private float size() {
        return Math.min(getWidth(), getHeight());
    }

    @Override
    protected void onDraw(Canvas canvas) {
        painter.draw(canvas, control, 0, 0, getWidth(), getHeight(), state);
    }

    @SuppressLint("ClickableViewAccessibility") // A gamepad control, not a click target.
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN: {
                int index = event.getActionIndex();
                down(event.getPointerId(index), event.getX(index), event.getY(index), event.getEventTime());
                break;
            }
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < event.getPointerCount(); i++) {
                    move(event.getPointerId(i), event.getX(i), event.getY(i));
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                up(event.getPointerId(event.getActionIndex()), event.getEventTime());
                break;
            case MotionEvent.ACTION_CANCEL:
                release();
                break;
            default:
                break;
        }
        return true;
    }

    void down(int pointer, float x, float y, long time) {
        switch (control.shape) {
            case STICK:
                if (stickPointer >= 0) return;
                stickPointer = pointer;
                stick.down(x, y, getWidth(), getHeight(), PadPainter.travel(size()));
                stickClick = PadInput.stickClick(stickReleasedAt, time);
                stickDownAt = time;
                stickPush = 0;
                state.stickActive = true;
                state.centerX = stick.centerX();
                state.centerY = stick.centerY();
                feedback(HapticFeedbackConstants.KEYBOARD_TAP);
                updateStick();
                break;
            case FACE:
            case DPAD: {
                int free = slotFor(-1);
                if (free < 0) return;
                pointerIds[free] = pointer;
                pointerFlags[free] = 0;
                move(pointer, x, y);
                break;
            }
            default:
                if (state.pressed) return;
                pointerIds[0] = pointer;
                state.pressed = true;
                if (PadInput.isTrigger(control)) {
                    gamepad.setTrigger(slot(), control == PadLayout.Control.LT ? TouchLayout.LEFT : TouchLayout.RIGHT, 1);
                } else {
                    gamepad.setButtons(slot(), PadInput.buttonFlag(control));
                }
                feedback(HapticFeedbackConstants.KEYBOARD_TAP);
                changed();
                break;
        }
    }

    void move(int pointer, float x, float y) {
        switch (control.shape) {
            case STICK:
                if (pointer != stickPointer) return;
                stick.move(x, y);
                updateStick();
                break;
            case FACE:
            case DPAD: {
                int index = slotFor(pointer);
                if (index < 0) return;
                int flags = control.shape == PadLayout.Shape.FACE ? PadInput.face(x, y, size()) : PadInput.dpad(x, y, size());
                if (flags == pointerFlags[index]) return;
                int before = state.flags;
                pointerFlags[index] = flags;
                updateMulti();
                // A tick for every newly held button or direction; sliding off to nothing is silent.
                if ((state.flags & ~before) != 0) {
                    feedback(control.shape == PadLayout.Shape.FACE ? HapticFeedbackConstants.KEYBOARD_TAP :
                            HapticFeedbackConstants.CLOCK_TICK);
                }
                break;
            }
            default:
                break;
        }
    }

    void up(int pointer, long time) {
        switch (control.shape) {
            case STICK:
                if (pointer != stickPointer) return;
                stickPointer = -1;
                stickReleasedAt = !stickClick && PadInput.stickTap(time - stickDownAt, stickPush) ? time : 0;
                stickClick = false;
                stick.release();
                state.stickActive = false;
                state.centerX = Float.NaN;
                state.centerY = Float.NaN;
                updateStick();
                break;
            case FACE:
            case DPAD: {
                int index = slotFor(pointer);
                if (index < 0) return;
                pointerIds[index] = -1;
                pointerFlags[index] = 0;
                updateMulti();
                break;
            }
            default:
                if (!state.pressed || pointerIds[0] != pointer) return;
                pointerIds[0] = -1;
                releaseButton();
                break;
        }
    }

    /** Lets go of everything this control holds, as a cancelled touch does. */
    void release() {
        java.util.Arrays.fill(pointerIds, -1);
        java.util.Arrays.fill(pointerFlags, 0);
        stickPointer = -1;
        stickClick = false;
        stickReleasedAt = 0;
        stick.release();
        boolean was = state.flags != 0 || state.pressed || state.stickActive;
        state.clear();
        gamepad.setButtons(slot(), 0);
        if (control.shape == PadLayout.Shape.STICK) {
            gamepad.setStick(slot(), control == PadLayout.Control.LEFT_STICK ? TouchLayout.LEFT : TouchLayout.RIGHT, 0, 0);
        }
        if (PadInput.isTrigger(control)) {
            gamepad.setTrigger(slot(), control == PadLayout.Control.LT ? TouchLayout.LEFT : TouchLayout.RIGHT, 0);
        }
        if (was) changed();
        else invalidate();
    }

    private void releaseButton() {
        state.pressed = false;
        if (PadInput.isTrigger(control)) {
            gamepad.setTrigger(slot(), control == PadLayout.Control.LT ? TouchLayout.LEFT : TouchLayout.RIGHT, 0);
        } else {
            gamepad.setButtons(slot(), 0);
        }
        changed();
    }

    private void updateStick() {
        state.stickX = stick.x;
        state.stickY = stick.y;
        stickPush = Math.max(stickPush, (float) Math.hypot(stick.x, stick.y));
        int side = control == PadLayout.Control.LEFT_STICK ? TouchLayout.LEFT : TouchLayout.RIGHT;
        gamepad.setStick(slot(), side, stick.x, stick.y);
        int flags = stickClick ? PadInput.buttonFlag(control) : 0;
        if (flags != state.flags && flags != 0) feedback(HapticFeedbackConstants.LONG_PRESS);
        state.flags = flags;
        gamepad.setButtons(slot(), flags);
        changed();
    }

    private void updateMulti() {
        int flags = 0;
        for (int i = 0; i < MAX_POINTERS; i++) {
            if (pointerIds[i] >= 0) flags |= pointerFlags[i];
        }
        state.flags = flags;
        gamepad.setButtons(slot(), flags);
        changed();
    }

    private int slotFor(int pointer) {
        for (int i = 0; i < MAX_POINTERS; i++) {
            if (pointerIds[i] == pointer) return i;
        }
        return -1;
    }

    private void changed() {
        applyAlpha();
        invalidate();
        changed.run();
    }

    private void feedback(int constant) {
        if (haptics) performHapticFeedback(constant);
    }

    PadPainter.State state() {
        return state;
    }
}
