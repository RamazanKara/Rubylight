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
        GPU_ERROR, SLOW, DROPPED, SIZE_CHANGED,
        // 10-bit SDR: direct output for precision, not because the picture is HDR.
        TEN_BIT
    }

    /** The reason an HDR/10-bit decline is told as: 10-bit for a 10-bit SDR stream the PC does not send as HDR. */
    public static Reason colorReason(Reason reason, boolean hostHdr, boolean sdr10) {
        return reason == Reason.HDR && sdr10 && !hostHdr ? Reason.TEN_BIT : reason;
    }

    static final int WARMUP_FRAMES = 8;
    /**
     * The window the sustained-load rule looks at, in time. A GPU path is only too slow when it
     * is over budget for most of this long: a display mode change, a thermal dip or a one-off
     * garbage collection costs a handful of frames and must not end a stream, but overload is
     * over budget on nearly every frame and fills a window within seconds.
     */
    static final long WINDOW_NS = 2000000000L;
    /** Fewer samples than this are too few to call a trend, whatever the frame rate. */
    static final int MIN_WINDOW_FRAMES = 30;
    /**
     * One frame that takes this long, or a GPU that stops answering for this long, is a stall and
     * not a load trend. It is far above any spike seen from driver scheduling, a mode switch or
     * the compositor holding a buffer back (tens of milliseconds), and still ends a hung GPU in
     * about a second. It deliberately does not scale with the frame rate: at 120 fps six frames
     * were only 50 ms, which one scheduler hiccup exceeds.
     */
    static final long STALL_LIMIT_NS = 1000000000L;
    private volatile long frameBudgetNs;
    private int warmup = WARMUP_FRAMES;
    // Which of the last window frames were over budget, as a ring with a running count.
    private boolean[] overBudget;
    private int ringIndex, ringFilled, ringSlow;
    private volatile Reason reason = Reason.NONE;
    private volatile float addedMs = -1;

    public UpscalingPolicy(int fps) {
        setFrameRate(fps);
    }

    synchronized void setFrameRate(int fps) {
        if (fps <= 0) throw new IllegalArgumentException("Invalid frame rate");
        long budgetNs = 1000000000L / fps;
        if (frameBudgetNs != budgetNs) {
            frameBudgetNs = budgetNs;
            overBudget = new boolean[(int) Math.max(MIN_WINDOW_FRAMES, WINDOW_NS / budgetNs)];
            ringIndex = ringFilled = ringSlow = 0;
        }
    }

    public static Reason unavailableReason(Mode mode, int width, int height, int outputWidth,
                                           int outputHeight, boolean hdrOrTenBit,
                                           boolean pyroWave, boolean glesSupported) {
        if (mode == Mode.OFF) return Reason.DISABLED;
        if (hdrOrTenBit) return Reason.HDR;
        if (pyroWave) return Reason.PYROWAVE;
        if (!glesSupported) return Reason.UNSUPPORTED;
        if (width <= 0 || height <= 0) return Reason.NO_UPSCALE;
        if (outputWidth < width || outputHeight < height ||
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

    synchronized void recordFrame(long addedNs, int dropped) {
        if (reason != Reason.NONE) return;
        float ms = addedNs / 1000000f;
        addedMs = addedMs < 0 ? ms : addedMs + 0.1f * (ms - addedMs);
        if (warmup > 0) {
            warmup--;
            return;
        }
        boolean slow = addedNs > frameBudgetNs;
        if (ringFilled == overBudget.length && overBudget[ringIndex]) ringSlow--;
        overBudget[ringIndex] = slow;
        if (slow) ringSlow++;
        ringIndex = (ringIndex + 1) % overBudget.length;
        if (ringFilled < overBudget.length) ringFilled++;
        // Coalesced texture frames can come from decoder bursts or a faster stream than the
        // panel. Only a full window that is mostly over budget, not routine replacement or a few
        // spikes anywhere in it, warrants fallback.
        if (ringFilled == overBudget.length && ringSlow * 2 > overBudget.length) fail(Reason.SLOW);
    }

    void checkStall(long elapsedNs) {
        // A stalled GPU cannot deliver samples to the window.
        if (elapsedNs > STALL_LIMIT_NS) fail(Reason.SLOW);
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
