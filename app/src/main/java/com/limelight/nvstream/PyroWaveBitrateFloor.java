package com.limelight.nvstream;

/**
 * PyroWave's bitrates, equal to nightjar-policy's pyrowave module through the shared traces.
 *
 * The floor is the host's severe-detail-loss floor (pyrowave::minimum_kbps in Rubylight's
 * rust/core/src/pyrowave.rs): 2.5 bits per pixel per frame below 1080 lines and 1.5 from 1080 up,
 * the same for SDR and HDR, 4:2:0 and 4:4:4, measured on representative pictures. The host lowers
 * PyroWave no further and warns below it. PyroWave is intra-only, so below this every frame
 * visibly falls apart instead of just softening.
 */
public final class PyroWaveBitrateFloor {
    /** The highest bitrate a stream may ask for (StreamConfiguration.MAX_BITRATE_KBPS). */
    private static final int MAX_KBPS = 2_000_000;

    private PyroWaveBitrateFloor() {
    }

    /** The floor in Kbps. */
    public static int minimumKbps(int width, int height, int fps) {
        return bitrateKbps(width, height, fps, height < 1080 ? 25 : 15);
    }

    /**
     * The bitrate a PyroWave stream starts at when the user has not chosen one: the host's
     * clean-picture target (pyrowave::recommended_kbps), 5 bits per pixel per frame below 1080 lines
     * and 3.2 from 1080 up, which is about 280 Mbps at 720p60 and 400 Mbps at 1080p60, the starting
     * points the client docs give. No more than a stream may ask for.
     */
    public static int defaultKbps(int width, int height, int fps) {
        return Math.min(bitrateKbps(width, height, fps, height < 1080 ? 50 : 32), MAX_KBPS);
    }

    /**
     * The bitrate a PyroWave stream starts at: {@link #defaultKbps} when the bitrate is automatic,
     * else the chosen one, raised to {@link #minimumKbps} (up to what a stream may ask for) when below it.
     */
    public static int startKbps(int configured, boolean automatic, int width, int height, int fps) {
        if (automatic) {
            return defaultKbps(width, height, fps);
        }
        return Math.max(configured, Math.min(minimumKbps(width, height, fps), MAX_KBPS));
    }

    // The host's arithmetic at a whole frame rate: tenths of a bit per pixel per frame, rounded up.
    private static int bitrateKbps(int width, int height, int fps, int bppTenths) {
        long bits = (long) Math.max(width, 0) * Math.max(height, 0) * Math.max(fps, 0) * bppTenths;
        return (int) Math.min((bits + 9_999) / 10_000, Integer.MAX_VALUE);
    }
}
