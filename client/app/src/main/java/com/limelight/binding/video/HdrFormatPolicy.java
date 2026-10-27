package com.limelight.binding.video;

import android.media.MediaFormat;
import android.view.Display;

/**
 * The HDR format beyond stock HDR10 that this client asks a Rubylight 3.0 host for
 * (x-rl-video[0].hdrFormat), apart from the Android objects so it can be tested with made-up
 * capabilities. The bits are rubylight_protocol::hdr's. A host that lists none (every other host)
 * gets nothing new, and nothing asked for streams stock PQ.
 * <ul>
 * <li>Dolby Vision profile 8 when the screen reports Dolby Vision and a hardware decoder takes
 * video/dolby-vision profile 8: 8.1 (PQ base layer) before 8.4 (HLG base layer). Both are plain
 * HEVC Main10 to every other decoder, so a decoder that then fails falls back to HEVC with the
 * same stream.</li>
 * <li>HLG when the screen reports HLG and no HDR10, so a panel without PQ still gets HDR.</li>
 * </ul>
 * Neither when the client draws the picture itself (packed full chroma, PyroWave, the Vulkan
 * renderer): those surfaces are BT.2020/PQ only, and a Dolby Vision decoder would map a picture
 * that is not the one shown.
 */
public final class HdrFormatPolicy {
    private HdrFormatPolicy() {}

    public static final int PQ = 1;
    public static final int HLG = 2;
    public static final int DV_81 = 4;
    public static final int DV_84 = 8;
    public static final int ALL = PQ | HLG | DV_81 | DV_84;
    public static final int DOLBY_VISION = DV_81 | DV_84;

    public static final String MIME_DOLBY_VISION = "video/dolby-vision";

    /**
     * @param hostFormats  the host's HdrFormats bits (serverinfo), 0 when it lists none
     * @param displayTypes the types from Display.getHdrCapabilities(), null when there are none
     * @param dolbyVisionDecoder a hardware decoder takes video/dolby-vision profile 8 (DvheSt)
     * @param pqSurface    the client draws the picture itself on a BT.2020/PQ surface (packed full
     *                     chroma, PyroWave, the Vulkan renderer)
     * @return one bit to ask for, or 0 for stock HDR10
     */
    public static int pick(int hostFormats, int[] displayTypes, boolean dolbyVisionDecoder, boolean pqSurface) {
        if (pqSurface) return 0;
        if (dolbyVisionDecoder && has(displayTypes, Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION)) {
            if ((hostFormats & DV_81) != 0) return DV_81;
            if ((hostFormats & DV_84) != 0) return DV_84;
        }
        if ((hostFormats & HLG) != 0 && !HdrPolicy.hasHdr10(displayTypes) &&
                has(displayTypes, Display.HdrCapabilities.HDR_TYPE_HLG)) {
            return HLG;
        }
        return 0;
    }

    /** The screen can show an HLG stream from this host, which then stands in for HDR10. */
    public static boolean hlgInstead(int hostFormats, int[] displayTypes) {
        return (hostFormats & HLG) != 0 && has(displayTypes, Display.HdrCapabilities.HDR_TYPE_HLG);
    }

    /** The format goes through a video/dolby-vision decoder. */
    public static boolean dolbyVision(int format) {
        return (format & DOLBY_VISION) != 0;
    }

    /** The MediaFormat colour transfer of an HDR stream in this format: HLG for HLG and 8.4, else PQ. */
    public static int transfer(int format) {
        return (format & (HLG | DV_84)) != 0 ? MediaFormat.COLOR_TRANSFER_HLG : MediaFormat.COLOR_TRANSFER_ST2084;
    }

    /**
     * The overlay's colour word while the PC reports HDR.
     * @param format       what this stream's ANNOUNCE asked for
     * @param dolbyVisionDecoding the stream goes through the Dolby Vision decoder (false after a fallback)
     */
    public static String overlayWord(int format, boolean dolbyVisionDecoding) {
        if (dolbyVision(format)) {
            return dolbyVisionDecoding ? name(format) : baseLayerName(format) + " (Dolby Vision decoder failed)";
        }
        return format == HLG ? "HLG" : "HDR";
    }

    /** What a decoder without Dolby Vision shows of the format: HLG for HLG and 8.4, else HDR10. */
    public static String baseLayerName(int format) {
        return transfer(format) == MediaFormat.COLOR_TRANSFER_HLG ? "HLG" : "HDR10";
    }

    /** A log name for the format, "HDR10" for stock. */
    public static String name(int format) {
        switch (format) {
            case HLG: return "HLG";
            case DV_81: return "Dolby Vision 8.1";
            case DV_84: return "Dolby Vision 8.4";
            default: return "HDR10";
        }
    }

    static boolean has(int[] types, int type) {
        if (types == null) return false;
        for (int t : types) {
            if (t == type) return true;
        }
        return false;
    }
}
