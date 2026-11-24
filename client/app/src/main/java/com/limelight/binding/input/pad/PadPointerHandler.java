package com.limelight.binding.input.pad;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.nvstream.input.MouseButtonPacket;

/**
 * Handles pointer touchpad ({@link PadLayout.Shape#TOUCHPAD}) gestures for {@link PadControlView}: relative mouse
 * motion or smoothed right-stick camera motion with release decay, single-finger tap left-click, two-finger tap
 * right-click and two-finger vertical/horizontal scrolling.
 */
final class PadPointerHandler {
    private static final float SCROLL_DP_PER_NOTCH = 24, SCROLL_UNITS_PER_NOTCH = 120, HORIZONTAL_SCROLL_RATIO = .5f;
    private static final int CAMERA_TICK_MS = 16;

    private final PadControlView view;
    private final int[] pointerIds = {-1, -1};
    /** Where the pointer touchpad's mouse movement, clicks and scrolls go; it sends nothing without one. */
    private ModernController.PointerSink pointerSink;
    private boolean pointerCamera;
    private MouseDeltaAccumulator scrollV = new MouseDeltaAccumulator(), scrollH = new MouseDeltaAccumulator();
    private final int[] pointerDelta = new int[2];
    private long pointerTickNs;
    private float pointerTravel;
    private final float[] pointerLastX = new float[2], pointerLastY = new float[2];
    private final long[] pointerDownAt = new long[2];
    private boolean pointerScrolled, pointerSecondDown, pointerTap;
    private final Runnable cameraTick = new Runnable() {
        @Override
        public void run() {
            PadPointerMotion motion = view.pointerMotion;
            if (motion == null || !pointerCamera) return;
            long now = System.nanoTime();
            motion.tick(now - pointerTickNs);
            pointerTickNs = now;
            view.gamepad().setStick(view.slot(), TouchLayout.RIGHT, motion.cameraX(), motion.cameraY());
            view.notifyGamepadChanged();
            if (motion.cameraMagnitude() > PadPointerMotion.CAMERA_STOP) view.postDelayed(this, CAMERA_TICK_MS);
        }
    };

    PadPointerHandler(PadControlView view) {
        this.view = view;
    }

    /**
     * Builds the pointer touchpad's motion model: the camera's mapping, or the pointer's with the user's curve.
     * {@code speedPercent} is the Trackpad speed setting, which sets how far a swipe turns the camera.
     */
    void configure(boolean camera, String curve, int speedPercent) {
        pointerCamera = camera;
        view.pointerMotion = new PadPointerMotion(PointerAcceleration.forCurve(curve), speedPercent);
    }

    void setSink(ModernController.PointerSink sink) {
        pointerSink = sink;
    }

    void down(int pointer, float x, float y, long time) {
        PadPainter.State state = view.state();
        if (!state.pressed) {
            // The first finger: a new gesture, from where it landed.
            pointerIds[0] = pointer;
            pointerIds[1] = -1;
            state.pressed = true;
            pointerDownAt[0] = time;
            pointerTravel = 0;
            pointerScrolled = pointerSecondDown = pointerTap = false;
            pointerLastX[0] = x;
            pointerLastY[0] = y;
            if (view.pointerMotion != null) view.pointerMotion.reset();
            view.removeCallbacks(cameraTick);
            view.touchAt(x, y, true);
            view.applyAlpha();
            view.redraw();
        } else if (pointerIds[1] < 0) {
            // A second finger turns the gesture into scroll or right-click; the pointer stops where it was.
            pointerIds[1] = pointer;
            pointerSecondDown = true;
            pointerDownAt[1] = time;
            pointerLastX[1] = x;
            pointerLastY[1] = y;
            view.removeCallbacks(cameraTick);
            if (pointerCamera && view.pointerMotion != null) {
                view.pointerMotion.stopCamera();
                view.gamepad().setStick(view.slot(), TouchLayout.RIGHT, 0, 0);
                view.notifyGamepadChanged();
            }
        }
    }

    void move(int pointer, float x, float y, long eventTimeNs) {
        int pointerSlot = pointer == pointerIds[0] ? 0 : pointerIds[1] >= 0 && pointer == pointerIds[1] ? 1 : -1;
        if (pointerSlot < 0) return;
        // The centroid travels with the fingers: each contributes its share, so two fingers moving
        // together scroll once rather than twice, and a finger left after a lift scrolls at its own pace.
        int fingers = (pointerIds[0] >= 0 ? 1 : 0) + (pointerIds[1] >= 0 ? 1 : 0);
        float dx = (x - pointerLastX[pointerSlot]) / fingers, dy = (y - pointerLastY[pointerSlot]) / fingers;
        pointerLastX[pointerSlot] = x;
        pointerLastY[pointerSlot] = y;
        pointerTravel += (float) Math.hypot(dx, dy);
        if (pointerSecondDown) {
            // Scrolling starts only past the tap slop, so a two-finger tap with a small drift stays a
            // tap and never both scrolls and clicks right; two fingers scroll; neither moves the
            // pointer, even after one lifts.
            if (!pointerScrolled && pointerTravel > PadInput.TOUCHPAD_TAP_SLOP_DP * view.density()) {
                pointerScrolled = true;
                pointerTap = false;
            }
            if (pointerScrolled) scrollBy(dx, dy);
            view.redraw();
        } else if (pointerSlot == 0) {
            view.touchAt(x, y, false);
            PadPointerMotion motion = view.pointerMotion;
            if (motion != null) {
                motion.move(dx, dy, eventTimeNs, view.density(), pointerDelta);
                if (pointerCamera) {
                    pointerTickNs = System.nanoTime();
                    view.gamepad().setStick(view.slot(), TouchLayout.RIGHT, motion.cameraX(), motion.cameraY());
                    view.notifyGamepadChanged();
                    view.removeCallbacks(cameraTick);
                    view.postDelayed(cameraTick, CAMERA_TICK_MS);
                } else if (pointerDelta[0] != 0 || pointerDelta[1] != 0) {
                    if (pointerSink != null) pointerSink.move(pointerDelta[0], pointerDelta[1]);
                }
            }
            view.redraw();
        }
    }

    /** Ends a pointer touchpad touch: a tap clicks, two fingers tap the right button, and a camera push decays. */
    void up(int pointer, long time) {
        boolean first = pointer == pointerIds[0];
        boolean second = pointerIds[1] >= 0 && pointer == pointerIds[1];
        if (!first && !second) return;
        int slot = first ? 0 : 1;
        if (first) pointerIds[0] = -1;
        if (second) pointerIds[1] = -1;
        // Each finger's own touch decides: short, and staying near where it landed. A finger that lingers
        // after its partner lifts is not a tap, so it cannot turn a scroll or a long touch into a right click.
        boolean tap = !pointerScrolled && PadInput.touchpadTap(time - pointerDownAt[slot], pointerTravel, view.density());
        if (pointerIds[0] >= 0 || pointerIds[1] >= 0) {
            // A finger is still down: the right click needs the last lift to be a tap too.
            pointerTap = tap;
            return;
        }
        if (pointerSecondDown) {
            if (pointerTap && tap) click(MouseButtonPacket.BUTTON_RIGHT);
        } else if (tap) {
            click(MouseButtonPacket.BUTTON_LEFT);
        }
        pointerSecondDown = pointerScrolled = pointerTap = false;
        pointerIds[0] = pointerIds[1] = -1;
        PadPainter.State state = view.state();
        state.pressed = false;
        state.touchX = Float.NaN;
        state.touchY = Float.NaN;
        scrollV = new MouseDeltaAccumulator();
        scrollH = new MouseDeltaAccumulator();
        if (pointerCamera && view.pointerMotion != null) {
            // Let go: the camera eases back to the center over the next few frames, not with a jump.
            pointerTickNs = System.nanoTime();
            view.removeCallbacks(cameraTick);
            view.postDelayed(cameraTick, CAMERA_TICK_MS);
        }
        view.applyAlpha();
        view.redraw();
    }

    void release() {
        // A pointer touchpad stops at once when input is taken away; nothing can carry on.
        view.removeCallbacks(cameraTick);
        if (pointerCamera) view.gamepad().setStick(view.slot(), TouchLayout.RIGHT, 0, 0);
        pointerSecondDown = pointerScrolled = pointerTap = false;
        pointerIds[0] = pointerIds[1] = -1;
        if (view.pointerMotion != null) view.pointerMotion.reset();
        scrollV = new MouseDeltaAccumulator();
        scrollH = new MouseDeltaAccumulator();
    }

    private void click(int button) {
        if (pointerSink != null) {
            pointerSink.button(button, true);
            pointerSink.button(button, false);
        }
        view.clickHaptic();
    }

    /** Two fingers on the pointer touchpad scroll: down moves the wheel down, right moves it right, in whole notches. */
    private void scrollBy(float dx, float dy) {
        if (pointerSink == null) return;
        float unitsPerPixel = SCROLL_UNITS_PER_NOTCH / Math.max(1, SCROLL_DP_PER_NOTCH * view.density());
        int vertical = scrollV.scale(dy, unitsPerPixel);
        int horizontal = scrollH.scale(-dx, unitsPerPixel * HORIZONTAL_SCROLL_RATIO);
        if (vertical != 0 || horizontal != 0) pointerSink.scroll(vertical, horizontal);
    }
}
