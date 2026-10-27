package com.limelight.nvstream;

/**
 * The lowest bitrate PyroWave still holds together at, from its author's objective bitrate
 * regression (Themaister/pyrowave, eval-results/pyrowave_regression_results.h, MIT). PyroWave is
 * intra-only, so below this every frame visibly falls apart instead of just softening.
 *
 * The floor is the regression's lowest level, 30 dB PSNR-HVS-M-H at a viewing distance of twice the
 * screen height. The regression covers 720p to 4K pixel counts; sizes outside use the nearest end.
 */
public final class PyroWaveBitrateFloor {
    private static final int MIN_PIXELS = 1280 * 720;
    private static final int MAX_PIXELS = 3840 * 2160;

    // Bytes per frame as a polynomial in sqrt(megapixels) - 2: the 30 dB, height factor 2.00 rows.
    private static final double[] YUV420 = {309.25055550415766, 8.351816219985253, -9.905584693249878,
            172.18790660175173, -64.17408945040484, -201.4015992260083, 46.22089684565188, 105.90424439986334};
    private static final double[] YUV444 = {330.65932695689867, -12.487508056245487, 13.060737308248315,
            139.64851902220505, -97.97414051841574, -112.81552415088743, 57.365194646059074, 57.19110311397152};

    // The author allows 1.2x for HDR10.
    private static final double HDR_FACTOR = 1.2;

    private PyroWaveBitrateFloor() {
    }

    /** The floor in Kbps, rounded up to a whole Mbps. */
    public static int minimumKbps(int width, int height, int fps, boolean yuv444, boolean hdr) {
        long pixels = Math.min(Math.max((long) width * height, MIN_PIXELS), MAX_PIXELS);
        double x = Math.sqrt(pixels * 1e-6) - 2.0;
        double[] coefficients = yuv444 ? YUV444 : YUV420;
        double estimate = 0, power = 1;
        for (double coefficient : coefficients) {
            estimate += power * coefficient;
            power *= x;
        }
        double mbps = estimate * 8e-3 * Math.max(fps, 1) * (hdr ? HDR_FACTOR : 1);
        return (int) Math.ceil(mbps) * 1000;
    }
}
