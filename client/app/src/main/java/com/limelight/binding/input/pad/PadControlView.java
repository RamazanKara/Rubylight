package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.os.Build;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;
import android.view.animation.AnimationUtils;

import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.TouchLayout;
import com.limelight.ui.Haptics;

import java.util.Arrays;

/**
 * One control of the modern on-screen controller. Each control is its own view so a finger that
 * misses every control reaches the stream underneath, while several fingers on different controls
 * each go to their own. Sticks follow one finger; the face cluster and the D-pad follow every finger
 * on them, so a thumb can slide from A to B or rest between them for both. A Steam Controller trackpad follows one
 * finger, sends it as a touch on that pad and clicks the pad from a tap; a grip holds from one tap to the next. The pointer touchpad moves the
 * mouse or the camera, with tap, two-finger right-click and two-finger scroll. A split cluster's members are views of
 * their own, and a finger that lands on one slides across all four ({@link Group}). A press shows at once; let go, it
 * eases back over {@link PadPainter#RELEASE_MS}, and its halo past the control's edge draws in the {@link HaloLayer}.
 */
@SuppressLint("ViewConstructor") // Created by ModernController for one control.
final class PadControlView extends View {
    private static final int MAX_POINTERS = 4;
    /**
     * How long a tapped trackpad's click is held, finger still on the pad, before it releases and the
     * touch ends: a little over the classic controller's 25 ms first resend, so one report carries
     * the press alone. The OSC has no other tap or hold constant to reuse.
     */
    static final long PAD_CLICK_HOLD_MS = 40;
    /** The alpha a press raises a faint, but visible, control to. */
    static final float PRESSED_ALPHA = .6f;

    final PadLayout.Control control;
    private final PadPainter painter;
    private final LayoutGamepad gamepad;
    private final Runnable changed;
    private final PadPainter.State state = new PadPainter.State();
    private final int[] pointerIds = new int[MAX_POINTERS], pointerFlags = new int[MAX_POINTERS];
    private final float density;
    private boolean haptics = true;
    /** A stick's knob, drawn in a larger view of its own; null for every other control. */
    private final KnobLayer knobLayer;
    private final PadPressAnimator animator;
    private final PadStickHandler stickHandler;
    private final PadTrackpadHandler trackpadHandler;
    private final PadPointerHandler pointerHandler;
    /** Kept on this view so {@link PadPointerHandler} and tests share the same motion model field. */
    PadPointerMotion pointerMotion;
    /** The split cluster this control is a member of; null for every other control. */
    private Group group;

    PadControlView(Context context, PadLayout.Control control, PadPainter painter, LayoutGamepad gamepad,
                   Runnable changed, WasdKeys.Keys keys) {
        super(context);
        this.control = control;
        this.painter = painter;
        this.gamepad = gamepad;
        this.changed = changed;
        density = context.getResources().getDisplayMetrics().density;
        setFocusable(false);
        setHapticFeedbackEnabled(true);
        Arrays.fill(pointerIds, -1);
        knobLayer = control.shape == PadLayout.Shape.STICK ? new KnobLayer(context, this) : null;
        animator = new PadPressAnimator(this, painter, state);
        stickHandler = control.shape == PadLayout.Shape.STICK ? new PadStickHandler(this, keys) : null;
        trackpadHandler = control.shape == PadLayout.Shape.PAD ? new PadTrackpadHandler(this) : null;
        pointerHandler = control.shape == PadLayout.Shape.TOUCHPAD ? new PadPointerHandler(this) : null;
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

    /**
     * Draws the press cue that reaches past the controls' edges ({@link PadPainter#drawHalo}), so a thumb covering a
     * small button still sees it answer: one view under every control, the size of the frame, taking no touches. It
     * draws only around a held or easing part, never for a control at opacity 0, and a control asks it to redraw when
     * its press changes; its own frames run only while a press ring does.
     */
    @SuppressLint("ViewConstructor")
    static final class HaloLayer extends View {
        private final PadPainter painter;
        private final java.util.Collection<PadControlView> controls;

        /** {@code controls} is read on every draw, so the layer follows the controller's controls as they change. */
        HaloLayer(Context context, PadPainter painter, java.util.Collection<PadControlView> controls) {
            super(context);
            this.painter = painter;
            this.controls = controls;
            setFocusable(false);
            setClickable(false);
            setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            long now = AnimationUtils.currentAnimationTimeMillis();
            boolean running = false;
            for (PadControlView view : controls) {
                if (view.getVisibility() != VISIBLE || view.animator.restingAlpha() <= 0) continue;
                // The controls are this layer's siblings, laid out in the same frame.
                running |= painter.drawHalo(canvas, view.control, view.getLeft() - getLeft(), view.getTop() - getTop(),
                        view.getWidth(), view.getHeight(), view.state, view.getAlpha(), now);
            }
            if (running) postInvalidateOnAnimation();
        }

        @SuppressLint("ClickableViewAccessibility") // Draws only; touches fall through to what is below.
        @Override
        public boolean onTouchEvent(MotionEvent event) {
            return false;
        }
    }

    /** Where this control's press halo draws ({@link HaloLayer}). */
    void setHaloLayer(View haloLayer) {
        animator.setHaloLayer(haloLayer);
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
        // A hidden member takes no input, also from a finger that slid onto it from another member.
        if (group != null && visibility != VISIBLE) group.update(null);
        animator.invalidateHalo();
    }

    /**
     * The four members of a split cluster (A, B, X and Y, or the D-pad's directions) as one group. There is no touch
     * router: Android keeps sending a finger's moves to the view it landed on, which owns it. On every move the owner
     * asks the group which members' boxes hold the finger, in the frame's coordinates, and holds all of them, so
     * sliding from A to B lets go of A and presses B, and a finger where two boxes overlap holds both (a D-pad
     * diagonal). Each member shows itself pressed while any finger holds it and sends its own button in its own slot
     * of the gamepad report, as separate controls do.
     */
    static final class Group {
        private final PadControlView[] members;
        /** The members some finger holds, as bits in member order. */
        private int held;

        /** {@code members} in {@link PadLayout.Control#memberIndex} order; one missing is null. */
        Group(PadControlView... members) {
            this.members = members;
            for (PadControlView member : members) {
                if (member != null) member.group = this;
            }
        }

        /** The shown members whose boxes hold the point ({@code x}, {@code y}) of the frame, as bits. */
        int under(float x, float y) {
            int bits = 0;
            for (int i = 0; i < members.length; i++) {
                PadControlView member = members[i];
                if (member == null || member.getVisibility() != VISIBLE) continue;
                if (x >= member.getLeft() && x < member.getRight() && y >= member.getTop() && y < member.getBottom()) {
                    bits |= 1 << i;
                }
            }
            return bits;
        }

        /**
         * Gathers what every finger on the group holds and shows and sends it; {@code owner}'s newly held member
         * ticks, as a newly held button does on the clusters.
         */
        void update(PadControlView owner) {
            int before = held;
            held = 0;
            for (PadControlView member : members) {
                if (member == null) continue;
                for (int i = 0; i < MAX_POINTERS; i++) {
                    if (member.pointerIds[i] >= 0) held |= member.pointerFlags[i];
                }
            }
            for (int i = 0; i < members.length; i++) {
                PadControlView member = members[i];
                if (member == null) continue;
                boolean on = (held & 1 << i) != 0 && member.getVisibility() == VISIBLE;
                if (!on) held &= ~(1 << i);
                member.hold(on);
            }
            if (owner != null && (held & ~before) != 0) {
                owner.feedback(owner.control.shape == PadLayout.Shape.FACE_BUTTON ? HapticFeedbackConstants.VIRTUAL_KEY :
                        HapticFeedbackConstants.CLOCK_TICK);
            }
        }
    }

    /** The group of members this view is one of, for tests; null for every other control. */
    Group group() {
        return group;
    }

    /** A member held or let go by the group's fingers: its look and its button follow. */
    private void hold(boolean on) {
        int flags = on ? PadInput.buttonFlag(control) : 0;
        if (state.pressed == on && state.flags == flags) return;
        state.pressed = on;
        state.flags = flags;
        gamepad.setButtons(slot(), flags);
        changed();
    }

    /** A member's finger at ({@code x}, {@code y}) of this view: it holds every member under it and lets go of the rest. */
    private void memberMove(int index, float x, float y) {
        int under = group.under(getLeft() + x, getTop() + y);
        if (under == pointerFlags[index]) return;
        pointerFlags[index] = under;
        group.update(this);
    }

    void configureStick(boolean floating, float deadzone) {
        if (stickHandler != null) stickHandler.configure(floating, deadzone);
    }

    /** With the Steam Controller controls, a finger on an analog stick also touches that controller's stick. */
    void setStickTouch(boolean stickTouch) {
        if (stickHandler != null) stickHandler.setStickTouch(stickTouch);
    }

    void setHaptics(boolean haptics) {
        this.haptics = haptics;
    }

    void setTouchSink(ModernController.TouchSink touchSink) {
        if (trackpadHandler != null) trackpadHandler.setTouchSink(touchSink);
    }

    /**
     * Builds the pointer touchpad's motion model: the camera's mapping, or the pointer's with the user's curve.
     * {@code speedPercent} is the Trackpad speed setting, which sets how far a swipe turns the camera.
     */
    void configurePointer(boolean camera, String curve, int speedPercent) {
        if (pointerHandler != null) pointerHandler.configure(camera, curve, speedPercent);
        // The pad itself says which it does, so a glance tells the Camera mode from the Mouse one.
        state.label = control == PadLayout.Control.POINTER_PAD ? painter.pointerLabel(camera) : "";
        redraw();
    }

    void setPointerSink(ModernController.PointerSink sink) {
        if (pointerHandler != null) pointerHandler.setSink(sink);
    }

    /**
     * The alpha at rest; a press shows at least 60 % so a faint control still answers visibly. At 0, the opacity
     * setting's 0 %, a control stays invisible even while pressed: the player chose controls nobody sees, so a press
     * draws nothing, not the control, a stick's knob, a touchpad's finger nor the pressed amber.
     */
    void setRestingAlpha(float alpha) {
        animator.setRestingAlpha(alpha);
    }

    /**
     * The alpha for a control at rest at {@code resting}, pressed in by {@code level}: 1 while held, easing back to 0
     * once let go, so a faint control fades back with its press.
     */
    static float alpha(float resting, float level) {
        if (resting <= 0) return 0;
        return resting + (Math.max(resting, PRESSED_ALPHA) - resting) * Math.max(0, Math.min(1, level));
    }

    void applyAlpha() {
        animator.applyAlpha();
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        animator.detach();
        if (trackpadHandler != null) trackpadHandler.finishPadClick();
    }

    void redraw() {
        invalidate();
        if (knobLayer != null) knobLayer.invalidate();
    }

    LayoutGamepad gamepad() {
        return gamepad;
    }

    int slot() {
        return control.ordinal();
    }

    float size() {
        return Math.min(getWidth(), getHeight());
    }

    float density() {
        return density;
    }

    boolean hapticsEnabled() {
        return haptics;
    }

    void clickHaptic() {
        if (haptics) Haptics.perform(this, Haptics.Kind.CLICK);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        state.now = AnimationUtils.currentAnimationTimeMillis();
        if (knobLayer != null) {
            painter.drawStickBase(canvas, control, 0, 0, getWidth(), getHeight(), state);
        } else {
            painter.draw(canvas, control, 0, 0, getWidth(), getHeight(), state);
        }
        // A touchpad's trail fades over the next frames, also once the finger lifts.
        if (animator.restingAlpha() > 0 && state.trailShows(state.now)) postInvalidateOnAnimation();
    }

    /** A touchpad finger at ({@code x}, {@code y}); {@code first} starts a new trail behind it. */
    void touchAt(float x, float y, boolean first) {
        state.touchX = x;
        state.touchY = y;
        if (first) state.trailStart = state.trailCount = 0;
        if (animator.restingAlpha() > 0) state.trail(x, y, AnimationUtils.currentAnimationTimeMillis());
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
                // Historical samples carry their own times, so the pointer touchpad's speed is not one spike per
                // frame. Every other control reports once per event, as it always has: extra stick, button and touch
                // reports for intermediate positions would change existing input.
                if (control.shape == PadLayout.Shape.TOUCHPAD) {
                    for (int history = 0; history < event.getHistorySize(); history++) {
                        long time = eventTimeNs(event, history);
                        for (int i = 0; i < event.getPointerCount(); i++) {
                            move(event.getPointerId(i), event.getHistoricalX(i, history), event.getHistoricalY(i, history), time);
                        }
                    }
                }
                // Each sample is stamped with its own event time, not the time it happened to be dispatched: the gap
                // between them is the finger's speed, and a late frame must not read as a slower one.
                long timeNs = eventTimeNs(event, -1);
                for (int i = 0; i < event.getPointerCount(); i++) {
                    move(event.getPointerId(i), event.getX(i), event.getY(i), timeNs);
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
        if (group != null) {
            int free = slotFor(-1);
            if (free < 0) return;
            pointerIds[free] = pointer;
            pointerFlags[free] = 0;
            memberMove(free, x, y);
            return;
        }
        switch (control.shape) {
            case STICK:
                stickHandler.down(pointer, x, y, time);
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
            case PAD:
                trackpadHandler.down(pointer, x, y, time);
                break;
            case TOUCHPAD:
                pointerHandler.down(pointer, x, y, time);
                break;
            default:
                if (PadInput.latches(control)) {
                    // A grip: each tap toggles it; the finger lifting changes nothing.
                    state.pressed = !state.pressed;
                    gamepad.setButtons(slot(), state.pressed ? PadInput.buttonFlag(control) : 0);
                    feedback(HapticFeedbackConstants.VIRTUAL_KEY);
                    changed();
                    break;
                }
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
        move(pointer, x, y, System.nanoTime());
    }

    /**
     * An event sample's own time in nanoseconds: {@code history} names a historical sample, -1 the event itself.
     * Android 14 reports nanoseconds; before it, whole milliseconds are all the event carries, so they are widened.
     */
    private static long eventTimeNs(MotionEvent event, int history) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            return history < 0 ? event.getEventTimeNanos() : event.getHistoricalEventTimeNanos(history);
        }
        return (history < 0 ? event.getEventTime() : event.getHistoricalEventTime(history)) * 1_000_000L;
    }

    void move(int pointer, float x, float y, long eventTimeNs) {
        if (group != null) {
            int index = slotFor(pointer);
            if (index >= 0) memberMove(index, x, y);
            return;
        }
        switch (control.shape) {
            case STICK:
                stickHandler.move(pointer, x, y);
                break;
            case FACE:
            case DPAD: {
                int index = slotFor(pointer);
                if (index < 0) return;
                // The whole box answers, a resized one's buttons spread over it as they are drawn.
                int flags = control.shape == PadLayout.Shape.FACE ?
                        PadInput.face(x, y, getWidth(), getHeight(), painter.facesSwapped()) :
                        PadInput.dpad(x, y, getWidth(), getHeight());
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
            case PAD:
                trackpadHandler.move(pointer, x, y);
                break;
            case TOUCHPAD:
                pointerHandler.move(pointer, x, y, eventTimeNs);
                break;
            default:
                break;
        }
    }

    void up(int pointer, long time) {
        if (group != null) {
            int index = slotFor(pointer);
            if (index < 0) return;
            pointerIds[index] = -1;
            pointerFlags[index] = 0;
            group.update(this);
            return;
        }
        switch (control.shape) {
            case STICK:
                stickHandler.up(pointer, time);
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
            case PAD:
                trackpadHandler.up(pointer, time);
                break;
            case TOUCHPAD:
                pointerHandler.up(pointer, time);
                break;
            default:
                if (!state.pressed || pointerIds[0] != pointer) return;
                pointerIds[0] = -1;
                releaseButton();
                break;
        }
    }

    /** Lets go of everything this control holds, as a cancelled touch does; a grip lets go too. */
    void release() {
        if (group != null) {
            // Its own fingers let go; a member another member's finger still holds stays held until that one lifts.
            Arrays.fill(pointerIds, -1);
            Arrays.fill(pointerFlags, 0);
            group.update(null);
            animator.stopEasing();
            redraw();
            return;
        }
        if (trackpadHandler != null) trackpadHandler.release();
        if (pointerHandler != null) pointerHandler.release();
        Arrays.fill(pointerIds, -1);
        Arrays.fill(pointerFlags, 0);
        if (stickHandler != null) stickHandler.releaseState();
        boolean was = state.flags != 0 || state.pressed || state.stickActive;
        state.clear();
        animator.stopEasing();
        WasdKeys wasd = wasd();
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

    void changed() {
        applyAlpha();
        redraw();
        changed.run();
    }

    void notifyGamepadChanged() {
        changed.run();
    }

    void feedback(int constant) {
        if (haptics) performHapticFeedback(constant);
    }

    /** The WASD stick's keys; null for every other control. */
    WasdKeys wasd() {
        return stickHandler != null ? stickHandler.wasd() : null;
    }

    PadPainter.State state() {
        return state;
    }
}
