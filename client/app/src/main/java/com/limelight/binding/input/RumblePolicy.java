package com.limelight.binding.input;

/** Which host vibration requests reach a motor, and how strongly this device vibrates in a controller's place. */
public final class RumblePolicy {
    private RumblePolicy() {}

    /**
     * "Disable rumble" silences every motor the host can drive: controller and trigger rumble,
     * waveform haptics and this device's vibration standing in for a controller.
     */
    public static boolean allows(boolean rumbleDisabled) {
        return !rumbleDisabled;
    }

    /** Scales an unsigned 16-bit motor level by a percentage, capped at full strength. */
    public static short scaleMotor(short motor, int strengthPercent) {
        int scaled = ((motor & 0xffff) * Math.max(0, strengthPercent)) / 100;
        return (short) Math.min(scaled, Short.MAX_VALUE * 2);
    }
}
