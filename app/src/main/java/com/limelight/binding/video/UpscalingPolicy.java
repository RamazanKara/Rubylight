package com.limelight.binding.video;

public final class UpscalingPolicy {
    public enum Mode {
        OFF("off"), BILINEAR("bilinear"), FSR1("fsr1"), SGSR1("sgsr1");

        public final String value;

        Mode(String value) {
            this.value = value;
        }

        public static Mode fromPreference(String value) {
            for (Mode mode : values()) {
                if (mode.value.equals(value)) return mode;
            }
            return OFF;
        }
    }

    public enum Reason {
        NONE, DISABLED, HDR, PYROWAVE, NO_UPSCALE, UNSUPPORTED,
        GPU_ERROR, SLOW, DROPPED, SIZE_CHANGED
    }

    static final int WARMUP_FRAMES = 8;
    static final int SLOW_FRAME_LIMIT = 6;
    private volatile long frameBudgetNs;
    private int warmup = WARMUP_FRAMES;
    private int slowFrames;
    private volatile Reason reason = Reason.NONE;
    private volatile float addedMs = -1;

    public UpscalingPolicy(int fps) {
        setFrameRate(fps);
    }

    void setFrameRate(int fps) {
        if (fps <= 0) throw new IllegalArgumentException("Invalid frame rate");
        long budgetNs = 1000000000L / fps;
        if (frameBudgetNs != budgetNs) {
            frameBudgetNs = budgetNs;
            slowFrames = 0;
        }
    }

    public static Reason unavailableReason(Mode mode, int width, int height, int outputWidth,
                                           int outputHeight, boolean hdrOrTenBit,
                                           boolean pyroWave, boolean glesSupported) {
        if (mode == Mode.OFF) return Reason.DISABLED;
        if (hdrOrTenBit) return Reason.HDR;
        if (pyroWave) return Reason.PYROWAVE;
        if (!glesSupported) return Reason.UNSUPPORTED;
        if (width <= 0 || height <= 0 || outputWidth < width || outputHeight < height ||
                (outputWidth == width && outputHeight == height)) return Reason.NO_UPSCALE;
        return Reason.NONE;
    }

    // The surface already reflects the stream aspect ratio except for stretch/PiP transitions.
    static int[] viewport(int width, int height, int surfaceWidth, int surfaceHeight, boolean stretch) {
        if (width <= 0 || height <= 0 || surfaceWidth <= 0 || surfaceHeight <= 0) {
            throw new IllegalArgumentException("Invalid image size");
        }
        if (stretch) return new int[] {0, 0, surfaceWidth, surfaceHeight};
        double scale = Math.min((double) surfaceWidth / width, (double) surfaceHeight / height);
        int outputWidth = (int) Math.round(width * scale);
        int outputHeight = (int) Math.round(height * scale);
        return new int[] {(surfaceWidth - outputWidth) / 2, (surfaceHeight - outputHeight) / 2,
                outputWidth, outputHeight};
    }

    void recordFrame(long addedNs, int dropped) {
        if (reason != Reason.NONE) return;
        float ms = addedNs / 1000000f;
        addedMs = addedMs < 0 ? ms : addedMs + 0.1f * (ms - addedMs);
        if (warmup > 0) {
            warmup--;
            return;
        }
        // A good frame must be comfortably below budget to erase a slow sample.
        if (addedNs > frameBudgetNs) slowFrames++;
        else if (addedNs < frameBudgetNs * 3 / 4) slowFrames = Math.max(0, slowFrames - 1);
        // Coalesced texture frames can come from decoder bursts or a faster stream than the
        // panel. Only sustained measured cost, not routine replacement, warrants fallback.
        if (slowFrames >= SLOW_FRAME_LIMIT) fail(Reason.SLOW);
    }

    void checkStall(long elapsedNs) {
        // A stalled GPU cannot deliver samples to the normal hysteresis window.
        if (elapsedNs > frameBudgetNs * SLOW_FRAME_LIMIT) fail(Reason.SLOW);
    }

    void fail(Reason failure) {
        if (reason == Reason.NONE) reason = failure;
    }

    public Reason getReason() {
        return reason;
    }

    public float getAddedMs() {
        return addedMs;
    }
}
