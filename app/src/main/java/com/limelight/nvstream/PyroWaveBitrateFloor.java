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
     * The default's ceiling when the client does not know its link: 900 Mbps, so a first stream
     * fits ordinary gigabit Ethernet and Wi-Fi.
     */
    public static final int UNKNOWN_LINK_DEFAULT_CAP_KBPS = 900_000;

    /**
     * The bitrate a PyroWave stream starts at when the user has not chosen one: the host's
     * clean-picture target (pyrowave::recommended_kbps), 5 bits per pixel per frame below 1080 lines
     * and 3.2 from 1080 up, which is about 280 Mbps at 720p60 and 400 Mbps at 1080p60, the starting
     * points the client docs give. It stays within 90% of the client's link ({@code linkMbps}, its
     * wired port speed toward the host, the pyrowaveLinkMbps hint) or, with the link unknown (0),
     * {@link #UNKNOWN_LINK_DEFAULT_CAP_KBPS}, but never below {@link #minimumKbps}; and no more than
     * a stream may ask for.
     */
    public static int defaultKbps(int width, int height, int fps, int linkMbps) {
        int clean = bitrateKbps(width, height, fps, height < 1080 ? 50 : 32);
        int cap = linkMbps > 0 ? (int) Math.min(linkMbps * 900L, Integer.MAX_VALUE) : UNKNOWN_LINK_DEFAULT_CAP_KBPS;
        return Math.min(Math.max(Math.min(clean, cap), minimumKbps(width, height, fps)), MAX_KBPS);
    }

    /**
     * The bitrate a PyroWave stream starts at: {@link #defaultKbps} when the bitrate is automatic,
     * else the chosen one as it is, whatever the link, also below {@link #minimumKbps} (the bitrate
     * setting warns about that).
     */
    public static int startKbps(int configured, boolean automatic, int width, int height, int fps, int linkMbps) {
        return automatic ? defaultKbps(width, height, fps, linkMbps) : configured;
    }

    // The host's arithmetic at a whole frame rate: tenths of a bit per pixel per frame, rounded up.
    private static int bitrateKbps(int width, int height, int fps, int bppTenths) {
        long bits = (long) Math.max(width, 0) * Math.max(height, 0) * Math.max(fps, 0) * bppTenths;
        return (int) Math.min((bits + 9_999) / 10_000, Integer.MAX_VALUE);
    }
}
