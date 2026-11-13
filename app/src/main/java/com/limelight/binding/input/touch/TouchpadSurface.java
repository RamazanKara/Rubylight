package com.limelight.binding.input.touch;

/**
 * A physical touchpad's axes for Touchpad gestures on the PC: positions on its surface as 0-1 and
 * its size in millimetres, from the device's motion range.
 */
public final class TouchpadSurface {
    private TouchpadSurface() {
    }

    /** {@code value} on the range {@code min}-{@code max} as 0-1; 0 for an empty range. */
    public static float unit(float value, float min, float max) {
        if (!(max > min) || Float.isNaN(value)) {
            return 0f;
        }
        return Math.max(0f, Math.min(1f, (value - min) / (max - min)));
    }

    /** An axis' length from its range and resolution in units per millimetre; 0 when unknown. */
    public static short lengthMm(float range, float unitsPerMm) {
        if (!(range > 0) || !(unitsPerMm > 0)) {
            return 0;
        }
        return (short) Math.max(1, Math.min(Short.MAX_VALUE, Math.round(range / unitsPerMm)));
    }
}
