package com.limelight.binding.input.pad;

import android.view.HapticFeedbackConstants;
import android.view.View;

import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.TouchLayout;
import com.limelight.ui.Haptics;

/**
 * Handles analog stick and WASD stick ({@link PadLayout.Shape#STICK}) touches for {@link PadControlView}: floating or
 * fixed center, double-tap stick click (L3/R3), optional Steam Controller stick-touch flag, WASD direction keys with
 * haptics, and base-vs-knob redraw gating.
 */
final class PadStickHandler {
    private final PadControlView view;
    /** The keys the WASD stick holds; null for every other control. */
    private final WasdKeys wasd;
    private LayoutStick stick;
    private int stickPointer = -1;
    // A stick tap (short, barely pushed) arms its click for the next touch; a flick does not.
    private long stickReleasedAt, stickDownAt;
    private float stickPush;
    private boolean stickClick, stickTouch;

    PadStickHandler(PadControlView view, WasdKeys.Keys keys) {
        this.view = view;
        this.wasd = view.control == PadLayout.Control.WASD ? new WasdKeys(keys) : null;
        configure(true, .08f);
    }

    void configure(boolean floating, float deadzone) {
        stick = new LayoutStick(floating, Math.max(0, Math.min(.9f, deadzone)));
    }

    /** With the Steam Controller controls, a finger on an analog stick also touches that controller's stick. */
    void setStickTouch(boolean stickTouch) {
        this.stickTouch = stickTouch;
    }

    /** The WASD stick's keys; null for analog sticks. */
    WasdKeys wasd() {
        return wasd;
    }

    void down(int pointer, float x, float y, long time) {
        if (stickPointer >= 0) return;
        stickPointer = pointer;
        stick.down(x, y, view.getWidth(), view.getHeight(), PadPainter.travel(view.size()));
        stickClick = PadInput.stickClick(stickReleasedAt, time);
        stickDownAt = time;
        stickPush = 0;
        PadPainter.State state = view.state();
        state.stickActive = true;
        state.centerX = stick.centerX();
        state.centerY = stick.centerY();
        // The WASD stick answers when a key goes down, in updateStick, not on touch.
        if (wasd == null) view.feedback(HapticFeedbackConstants.VIRTUAL_KEY);
        updateStick();
    }

    void move(int pointer, float x, float y) {
        if (pointer != stickPointer) return;
        stick.move(x, y);
        updateStick();
    }

    void up(int pointer, long time) {
        if (pointer != stickPointer) return;
        stickPointer = -1;
        stickReleasedAt = !stickClick && PadInput.stickTap(time - stickDownAt, stickPush) ? time : 0;
        stickClick = false;
        stick.release();
        PadPainter.State state = view.state();
        state.stickActive = false;
        state.centerX = Float.NaN;
        state.centerY = Float.NaN;
        updateStick();
    }

    void releaseState() {
        stickPointer = -1;
        stickClick = false;
        stickReleasedAt = 0;
        stick.release();
    }

    private void updateStick() {
        PadPainter.State state = view.state();
        boolean arcWasDrawn = state.stickX != 0 || state.stickY != 0;
        state.stickX = stick.x;
        state.stickY = stick.y;
        if (wasd != null) {
            WasdKeys.Feedback entered = wasd.update(stick.x, stick.y);
            if (view.hapticsEnabled() && entered != WasdKeys.Feedback.NONE) {
                Haptics.perform(view, entered == WasdKeys.Feedback.DIAGONAL ? Haptics.Kind.DIAGONAL : Haptics.Kind.DIRECTION);
            }
            state.flags = wasd.directions();
            view.applyAlpha();
            view.redraw();
            return;
        }
        stickPush = Math.max(stickPush, (float) Math.hypot(stick.x, stick.y));
        int side = view.control == PadLayout.Control.LEFT_STICK ? TouchLayout.LEFT : TouchLayout.RIGHT;
        view.gamepad().setStick(view.slot(), side, stick.x, stick.y);
        int flags = stickClick ? PadInput.buttonFlag(view.control) : 0;
        if (flags != state.flags && flags != 0) view.feedback(HapticFeedbackConstants.LONG_PRESS);
        boolean flagsChanged = flags != state.flags;
        state.flags = flags;
        // The touch is held, not pressed: it neither shows nor vibrates, and it goes when the finger lifts.
        int touch = stickTouch && stickPointer >= 0 ? PadInput.stickTouchFlag(view.control) : 0;
        view.gamepad().setButtons(view.slot(), flags | touch);
        stickMoved(flagsChanged, arcWasDrawn);
    }

    /**
     * A stick moved. The knob redraws on every move. The base also draws the amber arc from the stick
     * position, so it redraws only when that arc was or is drawn, or when the buttons it shows changed.
     */
    private void stickMoved(boolean flagsChanged, boolean arcWasDrawn) {
        view.applyAlpha();
        PadPainter.State state = view.state();
        if (PadControlView.baseNeedsRedraw(flagsChanged, arcWasDrawn, state.stickX != 0 || state.stickY != 0)) {
            view.invalidate();
        }
        View knobLayer = view.knobLayer();
        if (knobLayer != null) knobLayer.invalidate();
        view.notifyGamepadChanged();
    }
}
