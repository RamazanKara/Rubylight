package com.limelight.binding.video;

import android.media.MediaFormat;
import android.view.Display;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * The HDR format beyond stock HDR10 that this client asks a Rubylight 3.0 host for
 * (x-rl-video[0].hdrFormat), apart from the Android objects so it can be tested with made-up
 * capabilities. The bits are rubylight_protocol::hdr's. A host that lists none (every other host)
 * gets nothing new, and nothing asked for streams stock PQ.
 * <ul>
 * <li>Dolby Vision when the screen reports Dolby Vision, a hardware decoder takes the profile and
 * the stream can be in its codec: profile 10.1 (AV1 Main10, PQ base layer) first, as the handshake
 * takes AV1 before HEVC, then profile 8.1 (HEVC Main10, PQ base layer) before 8.4 (HLG base
 * layer). Each is plain AV1 or HEVC Main10 to every other decoder, so a decoder that then fails
 * falls back to AV1 or HEVC with the same stream.</li>
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
    public static final int DV_101 = 16;
    public static final int ALL = PQ | HLG | DV_81 | DV_84 | DV_101;
    public static final int DOLBY_VISION = DV_81 | DV_84 | DV_101;
    /** Profile 8 (8.1 and 8.4): HEVC Main10 with RPU NAL units, decoded as video/dolby-vision DvheSt. */
    public static final int DOLBY_VISION_HEVC = DV_81 | DV_84;
    /** Profile 10 (10.1): AV1 Main10 with RPU metadata OBUs, decoded as video/dolby-vision Dvav110. */
    public static final int DOLBY_VISION_AV1 = DV_101;

    public static final String MIME_DOLBY_VISION = "video/dolby-vision";

    /** The order Dolby Vision formats are asked for in: AV1 first, as the handshake takes it. */
    private static final int[] DOLBY_VISION_ORDER = {DV_101, DV_81, DV_84};

    /**
     * @param hostFormats  the host's HdrFormats bits (serverinfo), 0 when it lists none
     * @param displayTypes the types from Display.getHdrCapabilities(), null when there are none
     * @param dolbyVisionDecoders the Dolby Vision bits a hardware decoder takes:
     *                     {@link #DOLBY_VISION_HEVC} for video/dolby-vision profile 8 (DvheSt),
     *                     {@link #DOLBY_VISION_AV1} for profile 10 (Dvav110)
     * @param videoFormats the video formats both sides can negotiate (MoonBridge.VIDEO_FORMAT_*)
     * @param pqSurface    the client draws the picture itself on a BT.2020/PQ surface (packed full
     *                     chroma, PyroWave, the Vulkan renderer)
     * @return one bit to ask for, or 0 for stock HDR10
     */
    public static int pick(int hostFormats, int[] displayTypes, int dolbyVisionDecoders, int videoFormats,
                           boolean pqSurface) {
        if (pqSurface) return 0;
        if (has(displayTypes, Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION)) {
            for (int format : DOLBY_VISION_ORDER) {
                if ((hostFormats & dolbyVisionDecoders & format) != 0 && (videoFormats & carriers(format)) != 0) {
                    return format;
                }
            }
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

    /**
     * The video formats that can carry {@code format}, 0 when every format can. Dolby Vision
     * profile 8 (8.1 and 8.4) is HEVC Main10 with RPUs, profile 10 (10.1) AV1 Main10.
     */
    public static int carriers(int format) {
        if ((format & DOLBY_VISION_AV1) != 0) return MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        return (format & DOLBY_VISION_HEVC) != 0 ? MoonBridge.VIDEO_FORMAT_H265_MAIN10 : 0;
    }

    /**
     * The video formats to negotiate once {@code format} is picked: with the automatic codec
     * choice, a codec that the host can carry the format on. The handshake takes AV1 before HEVC
     * and the ANNOUNCE drops a format its codec can't carry (profile 8 on AV1 became AV1 HDR10),
     * so each of AV1 and HEVC is left out when none of its formats can carry it, as long as one
     * that can is left. H.264 stays: the handshake takes it only without HEVC and AV1. A codec the
     * user chose stays.
     */
    public static int videoFormats(int format, int videoFormats, boolean automaticCodec) {
        int carriers = carriers(format);
        if (!automaticCodec || carriers == 0 || (videoFormats & carriers) == 0) {
            return videoFormats;
        }
        for (int codec : new int[] {MoonBridge.VIDEO_FORMAT_MASK_AV1, MoonBridge.VIDEO_FORMAT_MASK_H265}) {
            if ((videoFormats & codec & carriers) == 0) {
                videoFormats &= ~codec;
            }
        }
        return videoFormats;
    }

    /** "AV1", "HEVC" or "AV1 and HEVC" for the codecs that have formats in {@code formats}, for logs. */
    public static String codecNames(int formats) {
        boolean av1 = (formats & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0;
        boolean hevc = (formats & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0;
        return av1 && hevc ? "AV1 and HEVC" : av1 ? "AV1" : hevc ? "HEVC" : "nothing";
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
            case DV_101: return "Dolby Vision 10.1";
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
