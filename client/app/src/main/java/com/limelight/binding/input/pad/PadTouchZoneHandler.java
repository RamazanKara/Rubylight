package com.limelight.binding.input.pad;

import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.nvstream.input.MouseButtonPacket;

/**
 * Pure-JVM, zero-hot-path-allocation motion and gesture engine for {@link PadTouchZone}:
 * floating-origin WASD + forward-arc {@code VK_LSHIFT} auto-sprint with hysteresis, floating
 * analog movement stick, mouse/stick camera look with acceleration and Y inversion, tap clicks,
 * and the 2-slot {@link AimForwarder} for jump-free simultaneous Look + Fire drag handoff.
 */
public final class PadTouchZoneHandler {
    public static final int VK_LSHIFT = 0xA0;
    public static final float SPRINT_ENGAGE_ARC_DEG = 50f;
    public static final float SPRINT_RELEASE_ARC_DEG = 62f;
    public static final float SPRINT_THRESHOLD_HYSTERESIS = 0.08f;

    /** Receives analog stick updates for {@code MOVE_STICK} (left) and {@code LOOK_STICK} (right). */
    public interface StickSink {
        void onStick(boolean right, float x, float y);
    }

    /** Receives relative pointer deltas and tap mouse clicks for {@code LOOK_MOUSE} / {@code TapAction}. */
    public interface PointerOut {
        void move(int dx, int dy);
        void tap(int mouseButton);
    }

    /**
     * Two-slot primitive aim arbitrator shared between {@code LOOK_*} zones and {@code dragAim}
     * custom buttons so simultaneous aiming and pointer lift hand off cleanly with zero camera jump.
     */
    public static final class AimForwarder {
        private static final int SLOTS = 2;

        private final int[] tokens = {-1, -1};
        private final float[] lastX = new float[SLOTS];
        private final float[] lastY = new float[SLOTS];
        private final MouseDeltaAccumulator accumX = new MouseDeltaAccumulator();
        private final MouseDeltaAccumulator accumY = new MouseDeltaAccumulator();
        private final PointerAcceleration[] accels = new PointerAcceleration[SLOTS];
        private final PadPointerMotion[] cameraMotions = new PadPointerMotion[SLOTS];
        private final int[] scratchDelta = new int[2];

        private String curve = PointerAcceleration.LINEAR;
        private int sensitivityPercent = PadTouchZone.DEFAULT_SENSITIVITY;
        private boolean invertY;
        private boolean stickMode;
        private PointerOut pointerOut;
        private StickSink stickSink;
        private int primarySlot = -1;
        private int decayingSlot = -1;

        public AimForwarder() {
            reconfigure(PointerAcceleration.LINEAR, PadTouchZone.DEFAULT_SENSITIVITY, false, false);
        }

        public void setSinks(PointerOut pointerOut, StickSink stickSink) {
            this.pointerOut = pointerOut;
            this.stickSink = stickSink;
        }

        void mergeSinks(PointerOut pointerOut, StickSink stickSink) {
            if (pointerOut != null) this.pointerOut = pointerOut;
            if (stickSink != null) this.stickSink = stickSink;
        }

        public void reconfigure(String curve, int sensitivityPercent, boolean invertY, boolean stickMode) {
            String normalized = PadTouchZone.normalizeCurve(curve);
            int clampedSens = Math.max(PadTouchZone.MIN_SENSITIVITY,
                    Math.min(PadTouchZone.MAX_SENSITIVITY, sensitivityPercent));
            boolean changedCurve = !normalized.equals(this.curve) || clampedSens != this.sensitivityPercent;
            this.curve = normalized;
            this.sensitivityPercent = clampedSens;
            this.invertY = invertY;
            if (this.stickMode && !stickMode && this.stickSink != null) {
                this.stickSink.onStick(true, 0f, 0f);
            }
            this.stickMode = stickMode;
            for (int i = 0; i < SLOTS; i++) {
                if (accels[i] == null || changedCurve) {
                    accels[i] = PointerAcceleration.forCurve(normalized);
                    cameraMotions[i] = new PadPointerMotion(PointerAcceleration.forCurve(normalized), clampedSens);
                }
            }
        }

        public boolean begin(int token, float frameX, float frameY) {
            int existing = slotOf(token);
            if (existing >= 0) {
                lastX[existing] = frameX;
                lastY[existing] = frameY;
                return true;
            }
            int free = tokens[0] == -1 ? 0 : (tokens[1] == -1 ? 1 : -1);
            if (free < 0) return false;
            tokens[free] = token;
            lastX[free] = frameX;
            lastY[free] = frameY;
            accels[free].reset();
            cameraMotions[free].reset();
            if (primarySlot < 0) {
                primarySlot = free;
                decayingSlot = -1;
            }
            return true;
        }

        public boolean move(int token, float frameX, float frameY, long eventTimeNs, float density) {
            int slot = slotOf(token);
            if (slot < 0) return false;
            float dx = frameX - lastX[slot];
            float dy = frameY - lastY[slot];
            lastX[slot] = frameX;
            lastY[slot] = frameY;
            if (dx == 0f && dy == 0f) return false;
            if (!Float.isFinite(dx) || !Float.isFinite(dy)) return false;
            float effectiveDy = invertY ? -dy : dy;
            primarySlot = slot;
            decayingSlot = -1;
            if (stickMode) {
                cameraMotions[slot].move(dx, effectiveDy, eventTimeNs, density, scratchDelta);
                if (stickSink != null) {
                    stickSink.onStick(true, cameraMotions[slot].cameraX(), cameraMotions[slot].cameraY());
                }
                return true;
            }
            float gain = accels[slot].gain(dx, effectiveDy, eventTimeNs, density);
            float scale = gain * (sensitivityPercent / 100f);
            int outX = accumX.scale(dx, scale);
            int outY = accumY.scale(effectiveDy, scale);
            if ((outX != 0 || outY != 0) && pointerOut != null) {
                pointerOut.move(outX, outY);
            }
            return true;
        }

        public boolean end(int token) {
            int slot = slotOf(token);
            if (slot < 0) return false;
            tokens[slot] = -1;
            lastX[slot] = 0f;
            lastY[slot] = 0f;
            int other = 1 - slot;
            if (tokens[other] != -1) {
                primarySlot = other;
                decayingSlot = -1;
                return false;
            }
            primarySlot = -1;
            if (stickMode && cameraMotions[slot].cameraMagnitude() > PadPointerMotion.CAMERA_STOP) {
                decayingSlot = slot;
                return true;
            }
            decayingSlot = -1;
            if (stickMode && stickSink != null) {
                cameraMotions[slot].stopCamera();
                stickSink.onStick(true, 0f, 0f);
            }
            return false;
        }

        /** Advances release decay for {@code LOOK_STICK}; returns true while further ticks are needed. */
        public boolean tickDecay(long elapsedNs) {
            if (!stickMode || decayingSlot < 0) return false;
            PadPointerMotion motion = cameraMotions[decayingSlot];
            motion.tick(elapsedNs);
            if (motion.cameraMagnitude() <= PadPointerMotion.CAMERA_STOP) {
                motion.stopCamera();
                if (stickSink != null) {
                    stickSink.onStick(true, 0f, 0f);
                }
                decayingSlot = -1;
                return false;
            }
            if (stickSink != null) {
                stickSink.onStick(true, motion.cameraX(), motion.cameraY());
            }
            return true;
        }

        /** Immediately cancels {@code token} and zeroes {@code LOOK_STICK} if no aim finger remains. */
        public void cancel(int token) {
            int slot = slotOf(token);
            if (slot >= 0) {
                tokens[slot] = -1;
                lastX[slot] = 0f;
                lastY[slot] = 0f;
                if (accels[slot] != null) accels[slot].reset();
                if (cameraMotions[slot] != null) cameraMotions[slot].reset();
                int other = 1 - slot;
                if (tokens[other] != -1) {
                    primarySlot = other;
                    decayingSlot = -1;
                    return;
                }
                primarySlot = -1;
                decayingSlot = -1;
                if (stickMode && stickSink != null) {
                    stickSink.onStick(true, 0f, 0f);
                }
                return;
            }
            if (stickMode && decayingSlot >= 0 && activeCount() == 0) {
                if (cameraMotions[decayingSlot] != null) cameraMotions[decayingSlot].stopCamera();
                decayingSlot = -1;
                if (stickSink != null) {
                    stickSink.onStick(true, 0f, 0f);
                }
            }
        }

        public boolean isDecaying() {
            return stickMode && decayingSlot >= 0;
        }

        public int activeCount() {
            return (tokens[0] != -1 ? 1 : 0) + (tokens[1] != -1 ? 1 : 0);
        }

        public void cancelAll() {
            boolean wasStick = stickMode && (activeCount() > 0 || decayingSlot >= 0);
            tokens[0] = tokens[1] = -1;
            lastX[0] = lastY[0] = lastX[1] = lastY[1] = 0f;
            primarySlot = -1;
            decayingSlot = -1;
            accumX.reset();
            accumY.reset();
            for (int i = 0; i < SLOTS; i++) {
                if (accels[i] != null) accels[i].reset();
                if (cameraMotions[i] != null) cameraMotions[i].reset();
            }
            if (wasStick && stickSink != null) {
                stickSink.onStick(true, 0f, 0f);
            }
        }

        private int slotOf(int token) {
            if (token == -1) return -1;
            if (tokens[0] == token) return 0;
            if (tokens[1] == token) return 1;
            return -1;
        }
    }

    private final PadTouchZone zone;
    private final PadHeldInputs heldInputs;
    private final WasdKeys wasd;
    private final AimForwarder aim;
    private final int aimToken;
    private StickSink stickSink;
    private PointerOut pointerOut;
    private LayoutStick stick;
    private boolean sprinting;
    private int activePointer = -1;
    private float downLocalX, downLocalY, originLocalX, originLocalY, touchLocalX, touchLocalY;
    private long downTimeMs;
    private float travelPx;
    private boolean tapEligible;
    private boolean decayActive;

    public PadTouchZoneHandler(PadTouchZone zone, PadHeldInputs heldInputs, AimForwarder aim, int aimToken) {
        this.zone = zone;
        this.heldInputs = heldInputs;
        this.wasd = heldInputs != null ? new WasdKeys(heldInputs.asWasdKeys()) : null;
        this.aim = aim != null ? aim : new AimForwarder();
        this.aimToken = aimToken;
        configure();
    }

    public void setSinks(PointerOut pointerOut, StickSink stickSink) {
        this.pointerOut = pointerOut;
        this.stickSink = stickSink;
        if (zone.kind.isLook()) {
            aim.mergeSinks(pointerOut, stickSink);
        }
    }

    /** Re-applies zone configuration (deadzone, sensitivity, invertY, acceleration curve, mode). */
    public void configure() {
        float dz = Math.max(0f, Math.min(PadTouchZone.MAX_DEADZONE, zone.deadzone));
        stick = new LayoutStick(true, dz);
        if (zone.kind.isLook()) {
            aim.reconfigure(zone.accelerationCurve, zone.sensitivityPercent, zone.invertY,
                    zone.kind == PadTouchZone.Kind.LOOK_STICK);
        }
        if (!zone.autoSprint || zone.kind != PadTouchZone.Kind.MOVE_WASD) {
            setSprinting(false);
        }
    }

    public void down(int pointerId, float localX, float localY, float frameX, float frameY,
                     float width, float height, float travelRadiusPx, long timeMs) {
        if (activePointer != -1) return;
        activePointer = pointerId;
        decayActive = false;
        downLocalX = localX;
        downLocalY = localY;
        originLocalX = localX;
        originLocalY = localY;
        touchLocalX = localX;
        touchLocalY = localY;
        downTimeMs = timeMs;
        travelPx = 0f;
        tapEligible = zone.kind.isLook() && zone.tapAction != PadTouchZone.TapAction.NONE;
        if (zone.kind.isMove()) {
            stick.down(localX, localY, width, height, Math.max(1f, travelRadiusPx));
            emitMoveState();
        } else {
            aim.mergeSinks(pointerOut, stickSink);
            aim.reconfigure(zone.accelerationCurve, zone.sensitivityPercent, zone.invertY,
                    zone.kind == PadTouchZone.Kind.LOOK_STICK);
            aim.begin(aimToken, frameX, frameY);
        }
    }

    public void move(int pointerId, float localX, float localY, float frameX, float frameY,
                     long eventTimeNs, float density) {
        if (pointerId != activePointer) return;
        float dx = localX - touchLocalX;
        float dy = localY - touchLocalY;
        touchLocalX = localX;
        touchLocalY = localY;
        if (zone.kind.isMove()) {
            stick.move(localX, localY);
            emitMoveState();
            return;
        }
        if (tapEligible) {
            travelPx += (float) Math.hypot(dx, dy);
            if (travelPx > PadInput.TOUCHPAD_TAP_SLOP_DP * Math.max(1f, density)) {
                tapEligible = false;
            }
        }
        aim.move(aimToken, frameX, frameY, eventTimeNs, density);
    }

    public void up(int pointerId, long timeMs) {
        if (pointerId != activePointer) return;
        activePointer = -1;
        if (zone.kind.isMove()) {
            stick.release();
            setSprinting(false);
            if (wasd != null) wasd.release();
            if (zone.kind == PadTouchZone.Kind.MOVE_STICK && stickSink != null) {
                stickSink.onStick(false, 0f, 0f);
            }
            return;
        }
        decayActive = aim.end(aimToken);
        if (tapEligible && zone.tapAction != PadTouchZone.TapAction.NONE && pointerOut != null
                && timeMs >= downTimeMs && timeMs - downTimeMs <= PadInput.TOUCHPAD_TAP_MS) {
            int button = zone.tapAction == PadTouchZone.TapAction.RIGHT_CLICK
                    ? MouseButtonPacket.BUTTON_RIGHT : MouseButtonPacket.BUTTON_LEFT;
            pointerOut.tap(button);
        }
        tapEligible = false;
    }

    public boolean tickDecay(long elapsedNs) {
        if (!decayActive) return false;
        decayActive = aim.tickDecay(elapsedNs);
        return decayActive;
    }

    public void release() {
        activePointer = -1;
        tapEligible = false;
        decayActive = false;
        stick.release();
        setSprinting(false);
        if (wasd != null) wasd.release();
        if (zone.kind == PadTouchZone.Kind.MOVE_STICK && stickSink != null) {
            stickSink.onStick(false, 0f, 0f);
        }
        if (zone.kind.isLook()) {
            aim.cancel(aimToken);
        }
    }

    private void emitMoveState() {
        if (zone.kind == PadTouchZone.Kind.MOVE_WASD) {
            if (wasd != null) wasd.update(stick.x, stick.y);
            updateAutoSprint(stick.x, stick.y);
        } else if (stickSink != null) {
            stickSink.onStick(false, stick.x, stick.y);
        }
    }

    private void updateAutoSprint(float x, float y) {
        if (!zone.autoSprint) {
            setSprinting(false);
            return;
        }
        float mag = (float) Math.hypot(x, y);
        if (mag <= 0f) {
            setSprinting(false);
            return;
        }
        double angle = Math.toDegrees(Math.atan2(y, x));
        double fromUp = Math.abs(angleDifference(angle, 90.0));
        if (!sprinting) {
            if (mag >= zone.sprintThreshold && fromUp <= SPRINT_ENGAGE_ARC_DEG) {
                setSprinting(true);
            }
        } else {
            float keepThreshold = Math.max(0.10f, zone.sprintThreshold - SPRINT_THRESHOLD_HYSTERESIS);
            if (mag < keepThreshold || fromUp > SPRINT_RELEASE_ARC_DEG) {
                setSprinting(false);
            }
        }
    }

    private void setSprinting(boolean on) {
        if (sprinting == on) return;
        sprinting = on;
        if (heldInputs != null) {
            heldInputs.key(VK_LSHIFT, on);
        }
    }

    private static double angleDifference(double angle, double target) {
        double value = (angle - target) % 360.0;
        if (value > 180.0) value -= 360.0;
        if (value < -180.0) value += 360.0;
        return value;
    }

    public boolean isActive() {
        return activePointer != -1;
    }

    public boolean isDecayActive() {
        return decayActive;
    }

    public boolean isSprinting() {
        return sprinting;
    }

    public float originX() {
        return originLocalX;
    }

    public float originY() {
        return originLocalY;
    }

    public float touchX() {
        return touchLocalX;
    }

    public float touchY() {
        return touchLocalY;
    }

    public float stickX() {
        return stick.x;
    }

    public float stickY() {
        return stick.y;
    }

    public float downX() {
        return downLocalX;
    }

    public float downY() {
        return downLocalY;
    }

    /** Computes a comfortable floating-stick travel radius in pixels for a zone of {@code width} x {@code height}. */
    public static float defaultTravelRadius(float width, float height, float density) {
        float d = Math.max(1f, density);
        float minSide = Math.min(Math.max(1f, width), Math.max(1f, height));
        return Math.min(minSide * 0.30f, 72f * d);
    }
}
