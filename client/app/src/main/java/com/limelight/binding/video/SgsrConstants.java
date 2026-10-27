package com.limelight.binding.video;

final class SgsrConstants {
    static float[] viewport(int width, int height) {
        if (width <= 0 || height <= 0) throw new IllegalArgumentException("Invalid SGSR dimensions");
        return new float[] {1f / width, 1f / height, width, height};
    }

    static float edgeSharpness(int strengthPercent) {
        if (strengthPercent < 0 || strengthPercent > 100) {
            throw new IllegalArgumentException("Invalid SGSR strength");
        }
        // Qualcomm's [1, 2] range retains reconstruction at zero extra sharpening.
        return 1f + strengthPercent / 100f;
    }
}
