package com.limelight.binding.video;

final class FsrConstants {
    // Float equivalents of AMD FsrEasuCon's four bit-cast uint4 constants. See NOTICE-FSR1.txt.
    static float[] easu(int viewportWidth, int viewportHeight, int textureWidth, int textureHeight,
                        int outputWidth, int outputHeight) {
        if (viewportWidth <= 0 || viewportHeight <= 0 || textureWidth < viewportWidth ||
                textureHeight < viewportHeight || outputWidth <= 0 || outputHeight <= 0) {
            throw new IllegalArgumentException("Invalid EASU dimensions");
        }
        float scaleX = viewportWidth * (1f / outputWidth);
        float scaleY = viewportHeight * (1f / outputHeight);
        float inverseX = 1f / textureWidth;
        float inverseY = 1f / textureHeight;
        return new float[] {
                scaleX, scaleY, 0.5f * scaleX - 0.5f, 0.5f * scaleY - 0.5f,
                inverseX, inverseY, inverseX, -inverseY,
                -inverseX, 2f * inverseY, inverseX, 2f * inverseY,
                0f, 4f * inverseY, 0f, 0f
        };
    }

    static float rcas(int strengthPercent) {
        if (strengthPercent < 0 || strengthPercent > 100) {
            throw new IllegalArgumentException("Invalid RCAS strength");
        }
        // FsrRcasCon.x = exp2(-stops). Expose linear strength, including an exact zero bypass.
        return strengthPercent / 100f;
    }
}
