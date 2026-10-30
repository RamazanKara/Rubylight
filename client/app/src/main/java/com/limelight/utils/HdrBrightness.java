package com.limelight.utils;

/**
 * One complete display luminance report in nits, as Sunshine-family hosts read it from the launch
 * query (minBrightness, maxBrightness, maxAverageBrightness). Unknown capabilities are null: the host
 * then picks its own fallback. Ported from Moonlight V+ (HdrBrightness.kt, 3e954d8).
 */
public final class HdrBrightness {
    private static final float ASSUMED_SDR_WHITE_NITS = 300f;

    public final float maxNits;
    public final float minNits;
    public final float maxFullFrameNits;
    public final boolean estimatedFromRatio;

    private HdrBrightness(float maxNits, float minNits, float maxFullFrameNits, boolean estimatedFromRatio) {
        this.maxNits = maxNits;
        this.minNits = minNits;
        this.maxFullFrameNits = maxFullFrameNits;
        this.estimatedFromRatio = estimatedFromRatio;
    }

    /** Float.toString keeps the precision and uses a locale-independent decimal point. */
    public String toQueryParameters() {
        return "&minBrightness=" + minNits + "&maxBrightness=" + maxNits + "&maxAverageBrightness=" + maxFullFrameNits;
    }

    /** A ratio is usable only when finite and above 1 (1 is SDR-equal: no headroom). NaN when not. */
    public static float usableHdrSdrRatio(float ratio) {
        return finite(ratio) && ratio > 1f ? ratio : Float.NaN;
    }

    // Float.isFinite needs API 24.
    private static boolean finite(float value) {
        return !Float.isNaN(value) && !Float.isInfinite(value);
    }

    /** The larger usable of the current and highest HDR/SDR ratio, NaN when neither is usable. */
    public static float bestHdrSdrRatio(float current, float highest) {
        float a = usableHdrSdrRatio(current), b = usableHdrSdrRatio(highest);
        if (Float.isNaN(a)) return b;
        if (Float.isNaN(b)) return a;
        return Math.max(a, b);
    }

    private static boolean in(float value, float low, float high) {
        return finite(value) && value >= low && value <= high;
    }

    /** A complete, consistent report, or null when any field is unknown or contradicts another. */
    public static HdrBrightness create(float maxNits, float minNits, float maxFullFrameNits) {
        if (!in(maxNits, 1f, 10000f) || !in(minNits, 0f, 100f) ||
                !in(maxFullFrameNits, 1f, maxNits) || minNits > maxFullFrameNits) {
            return null;
        }
        return new HdrBrightness(maxNits, minNits, maxFullFrameNits, false);
    }

    /**
     * What the display reports. A complete report is sent as it is. Only a missing or invalid peak is
     * estimated, from the HDR/SDR ratio times an assumed 300-nit SDR white; the estimate keeps a
     * known black level and full-frame level, and fills unknown ones with 0.001 nits and 20% of the
     * peak. Anything else (a valid peak with a missing or contradictory other field) is left to the host.
     */
    public static HdrBrightness fromDisplay(float maxNits, float minNits, float maxFullFrameNits, float hdrSdrRatio) {
        if (in(maxNits, 1f, 10000f)) {
            return create(maxNits, minNits, maxFullFrameNits);
        }
        float ratio = usableHdrSdrRatio(hdrSdrRatio);
        if (Float.isNaN(ratio)) return null;
        float estimatedPeak = ASSUMED_SDR_WHITE_NITS * ratio;
        if (!in(estimatedPeak, 1f, 10000f)) return null;

        boolean fullFrameKnown = in(maxFullFrameNits, 1f, 10000f);
        float peak = Math.max(estimatedPeak, fullFrameKnown ? maxFullFrameNits : 0f);
        float minimum = in(minNits, 0f, 100f) ? minNits : 0.001f;
        float fullFrame = fullFrameKnown ? maxFullFrameNits : Math.max(minimum, peak * 0.2f);
        // Known fields are kept; contradictory system values still fall back to the host.
        if (create(peak, minimum, fullFrame) == null) return null;
        return new HdrBrightness(peak, minimum, fullFrame, true);
    }

    @Override
    public String toString() {
        return "peak " + maxNits + ", frame average " + maxFullFrameNits + ", black " + minNits + " nits" +
                (estimatedFromRatio ? " (peak estimated from the HDR/SDR ratio)" : "");
    }
}
