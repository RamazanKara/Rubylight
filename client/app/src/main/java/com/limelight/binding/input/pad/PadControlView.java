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
import com.limelight.ui.Haptics;

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
    /** A stick's knob, drawn in a larger view of its own; null for every other control. */
    private final KnobLayer knobLayer;
    /** The keys the WASD stick holds; null for every other control. */
    private final WasdKeys wasd;

    PadControlView(Context context, PadLayout.Control control, PadPainter painter, LayoutGamepad gamepad,
                   Runnable changed, WasdKeys.Keys keys) {
        super(context);
        this.control = control;
        this.painter = painter;
        this.gamepad = gamepad;
        this.changed = changed;
        setFocusable(false);
        setHapticFeedbackEnabled(true);
        java.util.Arrays.fill(pointerIds, -1);
        configureStick(true, .08f);
        knobLayer = control.shape == PadLayout.Shape.STICK ? new KnobLayer(context, this) : null;
        wasd = control == PadLayout.Control.WASD ? new WasdKeys(keys) : null;
    }

    /**
     * Draws a stick's knob. At full deflection the knob's centre reaches the base's rim, so half of
     * it leaves the control's box. Rather than growing the control (which would take touches meant
     * for its neighbours or the stream), the knob draws in this view: the box plus the overhang on
     * every side, above the controls, taking no touches at all.
     */
    @SuppressLint("ViewConstructor")
    static final class KnobLayer extends View {
        private final PadControlView owner;

        KnobLayer(Context context, PadControlView owner) {
            super(context);
            this.owner = owner;
            setFocusable(false);
            setClickable(false);
            setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            // The control's box sits centred in this view.
            int width = owner.getWidth(), height = owner.getHeight();
            owner.painter.drawStickKnob(canvas, owner.control, (getWidth() - width) / 2f, (getHeight() - height) / 2f,
                    width, height, owner.state);
        }

        @SuppressLint("ClickableViewAccessibility") // Draws only; touches fall through to what is below.
        @Override
        public boolean onTouchEvent(MotionEvent event) {
            return false;
        }
    }

    /** The knob's own view for a stick, to add above the controls; null for other controls. */
    View knobLayer() {
        return knobLayer;
    }

    /** How far past each edge the knob layer reaches for a stick of this box, in whole pixels. */
    int knobOverhang(float width, float height) {
        return (int) Math.ceil(painter.knobOverhang(width, height));
    }

    @Override
    public void setVisibility(int visibility) {
        super.setVisibility(visibility);
        if (knobLayer != null) knobLayer.setVisibility(visibility);
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
        float alpha = active ? Math.max(restingAlpha, .6f) : restingAlpha;
        setAlpha(alpha);
        if (knobLayer != null) knobLayer.setAlpha(alpha);
    }

    private void redraw() {
        invalidate();
        if (knobLayer != null) knobLayer.invalidate();
    }

    private int slot() {
        return control.ordinal();
    }

    private float size() {
        return Math.min(getWidth(), getHeight());
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (knobLayer != null) {
            painter.drawStickBase(canvas, control, 0, 0, getWidth(), getHeight(), state);
        } else {
            painter.draw(canvas, control, 0, 0, getWidth(), getHeight(), state);
        }
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
                // The WASD stick answers when a key goes down, in updateStick, not on touch.
                if (wasd == null) feedback(HapticFeedbackConstants.VIRTUAL_KEY);
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
                feedback(HapticFeedbackConstants.VIRTUAL_KEY);
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
                    feedback(control.shape == PadLayout.Shape.FACE ? HapticFeedbackConstants.VIRTUAL_KEY :
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
        if (wasd == null) gamepad.setButtons(slot(), 0);
        if (control.shape == PadLayout.Shape.STICK && wasd == null) {
            gamepad.setStick(slot(), control == PadLayout.Control.LEFT_STICK ? TouchLayout.LEFT : TouchLayout.RIGHT, 0, 0);
        }
        if (PadInput.isTrigger(control)) {
            gamepad.setTrigger(slot(), control == PadLayout.Control.LT ? TouchLayout.LEFT : TouchLayout.RIGHT, 0);
        }
        if (wasd != null) {
            wasd.release();
            applyAlpha();
            redraw();
            return;
        }
        if (was) changed();
        else redraw();
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
        boolean arcWasDrawn = state.stickX != 0 || state.stickY != 0;
        state.stickX = stick.x;
        state.stickY = stick.y;
        if (wasd != null) {
            WasdKeys.Feedback entered = wasd.update(stick.x, stick.y);
            if (haptics && entered != WasdKeys.Feedback.NONE) {
                Haptics.perform(this, entered == WasdKeys.Feedback.DIAGONAL ? Haptics.Kind.DIAGONAL : Haptics.Kind.DIRECTION);
            }
            state.flags = wasd.directions();
            applyAlpha();
            redraw();
            return;
        }
        stickPush = Math.max(stickPush, (float) Math.hypot(stick.x, stick.y));
        int side = control == PadLayout.Control.LEFT_STICK ? TouchLayout.LEFT : TouchLayout.RIGHT;
        gamepad.setStick(slot(), side, stick.x, stick.y);
        int flags = stickClick ? PadInput.buttonFlag(control) : 0;
        if (flags != state.flags && flags != 0) feedback(HapticFeedbackConstants.LONG_PRESS);
        boolean flagsChanged = flags != state.flags;
        state.flags = flags;
        gamepad.setButtons(slot(), flags);
        stickMoved(flagsChanged, arcWasDrawn);
    }

    /**
     * A stick moved. The knob redraws on every move. The base also draws the amber arc from the stick
     * position, so it redraws only when that arc was or is drawn, or when the buttons it shows changed.
     */
    private void stickMoved(boolean flagsChanged, boolean arcWasDrawn) {
        applyAlpha();
        if (baseNeedsRedraw(flagsChanged, arcWasDrawn, state.stickX != 0 || state.stickY != 0)) invalidate();
        if (knobLayer != null) knobLayer.invalidate();
        changed.run();
    }

    /** Whether the base view must redraw: its glass and rim follow the state, its arc follows the stick push. */
    static boolean baseNeedsRedraw(boolean flagsChanged, boolean arcWasDrawn, boolean arcDrawn) {
        return flagsChanged || arcWasDrawn || arcDrawn;
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
        redraw();
        changed.run();
    }

    private void feedback(int constant) {
        if (haptics) performHapticFeedback(constant);
    }

    /** The WASD stick's keys; null for every other control. */
    WasdKeys wasd() {
        return wasd;
    }

    PadPainter.State state() {
        return state;
    }
}
