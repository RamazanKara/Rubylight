package com.limelight.binding.input;

public final class MouseDeltaAccumulator {
    private float remainder;

    public int scale(float delta, float factor) {
        float scaled = delta * factor + remainder;
        int whole = Math.round(scaled);
        remainder = scaled - whole;
        return whole;
    }
}
