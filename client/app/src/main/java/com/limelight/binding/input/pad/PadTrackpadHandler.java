package com.limelight.binding.input.pad;

import com.limelight.binding.input.SteamControls;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Handles Steam Controller trackpad ({@link PadLayout.Shape#PAD}) touches for {@link PadControlView}: forwards
 * single-finger touch positions to the trackpad sink and turns quick stationary taps into held pad clicks released
 * after {@link PadControlView#PAD_CLICK_HOLD_MS}.
 */
final class PadTrackpadHandler {
    private final PadControlView view;
    /** Where a trackpad's touches go; the trackpads send nothing without one. */
    private ModernController.TouchSink touchSink;
    private int pointerId = -1;
    /** A trackpad tap in the making: when the finger went down, and how far it has travelled since. */
    private long padDownAt;
    /** A tapped trackpad's click is held and its touch not yet ended; where the finger was. */
    private boolean padClickPending;
    private float padClickX, padClickY;
    private final Runnable padClickRelease = this::finishPadClick;
    private float padTravel, padLastX, padLastY;

    PadTrackpadHandler(PadControlView view) {
        this.view = view;
    }

    void setTouchSink(ModernController.TouchSink touchSink) {
        this.touchSink = touchSink;
    }

    void down(int pointer, float x, float y, long time) {
        PadPainter.State state = view.state();
        // One finger per pad, as on the controller.
        if (state.pressed) return;
        finishPadClick();
        pointerId = pointer;
        state.pressed = true;
        padDownAt = time;
        padTravel = 0;
        padLastX = x;
        padLastY = y;
        padTouch(MoonBridge.LI_TOUCH_EVENT_DOWN, x, y);
        view.applyAlpha();
        view.redraw();
    }

    void move(int pointer, float x, float y) {
        if (!view.state().pressed || pointer != pointerId) return;
        padTravel += (float) Math.hypot(x - padLastX, y - padLastY);
        padLastX = x;
        padLastY = y;
        padTouch(MoonBridge.LI_TOUCH_EVENT_MOVE, x, y);
        view.redraw();
    }

    void up(int pointer, long time) {
        PadPainter.State state = view.state();
        if (!state.pressed || pointer != pointerId) return;
        pointerId = -1;
        state.pressed = false;
        // A tap clicks the pad, as pressing the real one does: short, and the finger stayed
        // near where it landed, by the pointer touchpad's own thresholds. A drag or a
        // lingering finger does not. The click goes out with the finger still on the pad
        // and the touch ends after it, so a click that reads the touch position sees one.
        if (PadInput.touchpadTap(time - padDownAt, padTravel, view.density()) && PadInput.padClickFlag(view.control) != 0) {
            padClickX = state.touchX;
            padClickY = state.touchY;
            padClickPending = true;
            view.gamepad().setButtons(view.slot(), PadInput.padClickFlag(view.control));
            view.clickHaptic();
            view.changed();
            view.postDelayed(padClickRelease, PadControlView.PAD_CLICK_HOLD_MS);
        } else {
            padTouch(MoonBridge.LI_TOUCH_EVENT_UP, state.touchX, state.touchY);
        }
        state.touchX = Float.NaN;
        state.touchY = Float.NaN;
        view.applyAlpha();
        view.redraw();
    }

    void release() {
        // A held click ends with its touch, in order, before the rest lets go.
        finishPadClick();
        // A trackpad's finger leaves where it last was, as the classic controller's does.
        PadPainter.State state = view.state();
        if (state.pressed) {
            padTouch(MoonBridge.LI_TOUCH_EVENT_CANCEL, state.touchX, state.touchY);
        }
        pointerId = -1;
        padDownAt = 0;
    }

    /**
     * Ends a tapped pad's click: the release, then the touch's end, so the click went out with the
     * finger on the pad. The pad's slot holds nothing else, so a resend carries the release as for
     * any button let go. A no-op when no click is held.
     */
    void finishPadClick() {
        if (!padClickPending) return;
        padClickPending = false;
        view.removeCallbacks(padClickRelease);
        view.gamepad().setButtons(view.slot(), 0);
        view.changed();
        padTouch(MoonBridge.LI_TOUCH_EVENT_UP, padClickX, padClickY);
        PadPainter.State state = view.state();
        state.touchX = Float.NaN;
        state.touchY = Float.NaN;
    }

    /** Sends a finger at ({@code x}, {@code y}) on this trackpad as a touch on its Steam Controller pad. */
    private void padTouch(byte type, float x, float y) {
        PadPainter.State state = view.state();
        if (type == MoonBridge.LI_TOUCH_EVENT_DOWN || type == MoonBridge.LI_TOUCH_EVENT_MOVE) {
            view.touchAt(x, y, type == MoonBridge.LI_TOUCH_EVENT_DOWN);
        } else {
            state.touchX = x;
            state.touchY = y;
        }
        if (touchSink == null) return;
        int pad = PadInput.steamPad(view.control);
        touchSink.touch(type, SteamControls.pointerId(pad), SteamControls.touchX(pad, fraction(x, view.getWidth())),
                SteamControls.touchY(fraction(y, view.getHeight())), SteamControls.PAD_TOUCH_PRESSURE);
    }

    private static float fraction(float position, int size) {
        return size > 0 ? position / size : 0f;
    }
}
