package com.limelight.binding.input.pad;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.touch.PointerAcceleration;

/**
 * A finger on the on-screen controller's touchpad, as mouse movement or as a right stick. The gain and the
 * speed behind it come from {@link PointerAcceleration}: the last three moves smooth the speed by their event
 * times, so one jittery report neither spikes nor stalls the pointer; slow movements stay precise and a fast
 * flick crosses more of the screen, and a gesture starts at the slow end. Mouse movement keeps the leftover
 * fraction of a pixel for the next report ({@link MouseDeltaAccumulator}), and the camera's deflection is
 * deadzone-free and returns to the center within about 50 ms after the finger stops. The Trackpad speed setting sets
 * how far a swipe turns the camera, as it sets how far it moves the pointer.
 *
 * <p>Plain Java, no Android types, so the mapping is tested without a device.
 */
public final class PadPointerMotion {
    /** The finger speed, in dp per second, that fully deflects the camera stick at a Trackpad speed of 100 %. */
    static final float CAMERA_FULL_SPEED = 300f;
    /** The Trackpad speed setting's range in percent ({@code seekbar_trackpad_speed}); 100 % is the default. */
    static final int MIN_SPEED_PERCENT = 25, MAX_SPEED_PERCENT = 400, DEFAULT_SPEED_PERCENT = 100;
    /** The camera's deflection falls by e every this many milliseconds without movement. */
    static final float CAMERA_DECAY_TAU_MS = 15f;
    /** Below this share of full deflection the camera is centered and the decay stops. */
    static final float CAMERA_STOP = .02f;

    // A fresh pair starts each touch, so no fraction of the last gesture's pixel is carried into the next.
    private final MouseDeltaAccumulator deltaX = new MouseDeltaAccumulator(), deltaY = new MouseDeltaAccumulator();
    private final PointerAcceleration acceleration;
    /** The finger speed that fully deflects the camera stick: {@link #CAMERA_FULL_SPEED} at the Trackpad speed. */
    private final float cameraFullSpeed;
    private float directionX, directionY;
    private float cameraX, cameraY;

    public PadPointerMotion() {
        this(PointerAcceleration.forCurve(PointerAcceleration.MEDIUM));
    }

    public PadPointerMotion(PointerAcceleration acceleration) {
        this(acceleration, DEFAULT_SPEED_PERCENT);
    }

    /**
     * @param speedPercent the Trackpad speed setting, clamped to its range: at 200 % a swipe turns the camera as far
     *                     as one twice as fast does at 100 %. The pointer's own speed is applied where its movement
     *                     is sent ({@link PointerMove}), so it changes only the camera here.
     */
    public PadPointerMotion(PointerAcceleration acceleration, int speedPercent) {
        this.acceleration = acceleration;
        int percent = Math.max(MIN_SPEED_PERCENT, Math.min(MAX_SPEED_PERCENT, speedPercent));
        cameraFullSpeed = CAMERA_FULL_SPEED * DEFAULT_SPEED_PERCENT / percent;
    }

    /**
     * Adds a finger move of ({@code dx}, {@code dy}) pixels at {@code eventTimeNs} and writes the pointer's pixel
     * movement into {@code out[0]} and {@code out[1]}. The fraction of a pixel that does not fit is carried.
     *
     * @param density the display's density, so speed is measured in dp as the acceleration curve expects
     */
    public void move(float dx, float dy, long eventTimeNs, float density, int[] out) {
        float gain = acceleration.gain(dx, dy, eventTimeNs, density);
        // The direction is the finger's own; only the speed is smoothed, which is what the curve needs.
        float length = (float) Math.hypot(dx, dy);
        if (length > 0 && Float.isFinite(length)) {
            directionX = dx / length;
            directionY = dy / length;
        }
        out[0] = deltaX.scale(dx, gain);
        out[1] = deltaY.scale(dy, gain);
        updateCamera();
    }

    /** The smoothed finger speed in dp per second after the last {@link #move}. */
    public float speed() {
        return acceleration.speed();
    }

    /** The smoothed movement direction, a unit vector; (0, 0) when nothing moved. */
    public float directionX() {
        return directionX;
    }

    public float directionY() {
        return directionY;
    }

    /** The camera stick's deflection from the last movement, x right and y up positive. */
    public float cameraX() {
        return cameraX;
    }

    public float cameraY() {
        return cameraY;
    }

    public float cameraMagnitude() {
        return (float) Math.hypot(cameraX, cameraY);
    }

    /**
     * Ages the camera's deflection by {@code elapsedNs} without a movement: it decays to the center within about
     * 50 ms after the finger stops, and stops entirely once it is imperceptible.
     */
    public void tick(long elapsedNs) {
        if (elapsedNs <= 0 || (cameraX == 0 && cameraY == 0)) return;
        float factor = (float) Math.exp(-(elapsedNs / 1_000_000f) / CAMERA_DECAY_TAU_MS);
        cameraX *= factor;
        cameraY *= factor;
        if (cameraMagnitude() < CAMERA_STOP) {
            cameraX = 0;
            cameraY = 0;
        }
    }

    /** Centers the camera at once, for a cancelled touch or a second finger landing. */
    public void stopCamera() {
        cameraX = 0;
        cameraY = 0;
    }

    /** Forgets every sample and the camera, as a new touch does. */
    public void reset() {
        acceleration.reset();
        directionX = 0;
        directionY = 0;
        cameraX = 0;
        cameraY = 0;
        deltaX.reset();
        deltaY.reset();
    }

    private void updateCamera() {
        // No deadzone: any movement at all deflects the camera, and a faster one approaches full deflection.
        float speed = speed();
        float magnitude = speed > 0 ? 1 - (float) Math.exp(-speed / cameraFullSpeed) : 0;
        cameraX = directionX * magnitude;
        // Screen down is a negative stick y, so a finger moving down looks down.
        cameraY = -directionY * magnitude;
    }
}
