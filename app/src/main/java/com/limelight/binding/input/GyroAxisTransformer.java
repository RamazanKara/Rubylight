package com.limelight.binding.input;

/**
 * Maps a controller's gyroscope axes onto the axes Moonlight sends (X pitch, Y yaw, Z roll)
 * and scales pitch and yaw, without changing units.
 *
 * Ported from jvanderv42's Nightjar fork (jvanderv42/nightjar, GPL-3.0, commit d0e14cf25,
 * app/src/main/java/com/limelight/binding/input/GyroAxisTransformer.java), with an
 * allocation-free transform and an identity check for the motion send paths.
 */
public final class GyroAxisTransformer {
    public static final float MIN_SENSITIVITY = 0.1f;
    public static final float MAX_SENSITIVITY = 3f;

    private static final GyroAxisTransformer IDENTITY = new GyroAxisTransformer(0, 1, 2, 1, 1, 1);

    private final int[] sourceAxes;
    private final int[] signs;
    private final float horizontalSensitivity;
    private final float verticalSensitivity;
    private final boolean identity;

    GyroAxisTransformer(int xAxis, int yAxis, int zAxis,
                        int xSign, int ySign, int zSign) {
        this(xAxis, yAxis, zAxis, xSign, ySign, zSign, 1f, 1f);
    }

    GyroAxisTransformer(int xAxis, int yAxis, int zAxis,
                        int xSign, int ySign, int zSign,
                        float horizontalSensitivity, float verticalSensitivity) {
        if (!validSensitivity(horizontalSensitivity) || !validSensitivity(verticalSensitivity)) {
            throw new IllegalArgumentException("Sensitivity must be between 0.10 and 3.00");
        }
        sourceAxes = new int[] {xAxis, yAxis, zAxis};
        signs = new int[] {xSign, ySign, zSign};
        boolean[] seen = new boolean[3];
        for (int axis : sourceAxes) {
            if (axis < 0 || axis > 2 || seen[axis]) {
                throw new IllegalArgumentException("Axes must be a permutation of 0, 1, 2");
            }
            seen[axis] = true;
        }
        for (int sign : signs) {
            if (sign != 1 && sign != -1) {
                throw new IllegalArgumentException("Axis signs must be +1 or -1");
            }
        }
        this.horizontalSensitivity = horizontalSensitivity;
        this.verticalSensitivity = verticalSensitivity;
        identity = xAxis == 0 && yAxis == 1 && zAxis == 2 && xSign == 1 && ySign == 1 && zSign == 1
                && horizontalSensitivity == 1f && verticalSensitivity == 1f;
    }

    static boolean validSensitivity(float value) {
        return Float.isFinite(value) && value >= MIN_SENSITIVITY && value <= MAX_SENSITIVITY;
    }

    public static GyroAxisTransformer identity() {
        return IDENTITY;
    }

    /** True when {@link #transform} returns its input unchanged, so a caller can skip it. */
    public boolean isIdentity() {
        return identity;
    }

    /** Writes the mapped values into {@code out} (length 3), which may be reused across calls. */
    public void transform(float x, float y, float z, float[] out) {
        float sx = sourceAxes[0] == 0 ? x : sourceAxes[0] == 1 ? y : z;
        float sy = sourceAxes[1] == 0 ? x : sourceAxes[1] == 1 ? y : z;
        float sz = sourceAxes[2] == 0 ? x : sourceAxes[2] == 1 ? y : z;
        out[0] = sx * signs[0] * verticalSensitivity;
        out[1] = sy * signs[1] * horizontalSensitivity;
        out[2] = sz * signs[2];
    }

    float[] transform(float x, float y, float z) {
        float[] out = new float[3];
        transform(x, y, z, out);
        return out;
    }
}
