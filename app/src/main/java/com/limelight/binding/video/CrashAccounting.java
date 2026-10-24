package com.limelight.binding.video;

/**
 * Whether a decoder failure is a decoder crash. The count behind the "video decoder crashed"
 * dialog disables features and resets settings, so it must not grow when the failure is only the
 * stream ending or (Experimental, Android TV) the TV dropping the signal: that is a display loss,
 * not a decoder fault.
 */
public final class CrashAccounting {
    /** On a TV, a decoder failure this soon after the video display changed is a display loss. */
    public static final long DISPLAY_WINDOW_MS = 5000;

    public enum Verdict {
        /** A real decoder crash: counted as always. */
        COUNT,
        /** The stream had started ending (terminated, quit requested, stop begun). */
        STREAM_ENDING,
        /** Android TV, and the video display changed, went away or turned off just before. */
        DISPLAY_LOST
    }

    private CrashAccounting() {}

    /**
     * @param lastDisplayEventMs when the video display last changed or was removed, on the same
     *                           clock as nowMs; negative if it never did
     * @param displayOff         the video display is off or gone right now
     */
    public static Verdict classify(boolean streamEnding, boolean androidTv, long nowMs,
                                   long lastDisplayEventMs, boolean displayOff) {
        if (streamEnding) return Verdict.STREAM_ENDING;
        if (androidTv) {
            boolean recentChange = lastDisplayEventMs >= 0 && nowMs - lastDisplayEventMs <= DISPLAY_WINDOW_MS;
            if (displayOff || recentChange) return Verdict.DISPLAY_LOST;
        }
        return Verdict.COUNT;
    }
}
