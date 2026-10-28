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
 * Never Dolby Vision when the client draws the picture itself (DrawnSurface): a Dolby Vision decoder
 * would map a picture that is not the one shown, and no API takes Dolby Vision with drawn buffers.
 * HLG on the Vulkan renderer, whose surface takes it, and on PyroWave from a host that offers it
 * there (PYROWAVE_HLG) where this display's window takes the HLG dataspace (HlgWindow); packed
 * full chroma draws BT.2020/PQ only.
 */
public final class HdrFormatPolicy {
    private HdrFormatPolicy() {}

    /** Who puts the decoder's picture on the screen. */
    public enum DrawnSurface {
        /** The decoder itself: the picture reaches the screen as it was decoded. */
        NONE,
        /**
         * The client draws it on a BT.2020/PQ surface: packed full chroma, and PyroWave from a host
         * without PYROWAVE_HLG or on a window that refused HLG.
         */
        PQ_ONLY,
        /**
         * The Vulkan renderer, where its instance has VK_EXT_swapchain_colorspace
         * (VulkanRendererBridge.drawsHlg), and PyroWave from a host with PYROWAVE_HLG: BT.2020 PQ or
         * HLG, through the HDR colour spaces or the window dataspace.
         */
        PQ_HLG
    }

    /**
     * How this display's stream window took the BT2020_HLG dataspace (WindowDataspace.probeHlg),
     * asked of each device at run time: the public call's list differs between Android builds.
     */
    public enum HlgWindow {
        /** ANativeWindow_setBuffersDataSpace took it. */
        PUBLIC,
        /** ANativeWindow_setBuffersDataSpace refused it and ANativeWindow::perform took it. */
        PERFORM,
        /** Neither took it: this display gets HDR10 on PyroWave. */
        REFUSED;

        public boolean accepted() {
            return this != REFUSED;
        }

        /** The answer of WindowDataspace.Probe.call. */
        public static HlgWindow of(int call) {
            switch (call) {
                case WindowDataspace.CALL_PUBLIC: return PUBLIC;
                case WindowDataspace.CALL_PERFORM: return PERFORM;
                default: return REFUSED;
            }
        }
    }

    /** What the screen would show with the decoder's own output and a drawn surface leaves out. */
    public enum DrawnLimit {
        NONE,
        /** A Dolby Vision screen and decoder: the stream is HDR10 instead. */
        DOLBY_VISION,
        /** An HDR10+ screen: PyroWave carries no dynamic metadata (the Vulkan renderer passes it on). */
        HDR10_PLUS
    }

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

    /**
     * Offer flag, never a format (rubylight_protocol::hdr::PYROWAVE_HLG): the host also takes HLG on
     * PyroWave Main10 streams. Rubylight 3.0 rc hosts refuse HLG there.
     */
    public static final int PYROWAVE_HLG = 0x80;
    public static final int OFFER_FLAGS = PYROWAVE_HLG;

    public static final String MIME_DOLBY_VISION = "video/dolby-vision";

    /** The order Dolby Vision formats are asked for in: AV1 first, as the handshake takes it. */
    private static final int[] DOLBY_VISION_ORDER = {DV_101, DV_81, DV_84};

    /**
     * @param hostFormats  the host's HdrFormats bits and offer flags (serverinfo), 0 when it lists none
     * @param displayTypes the types from Display.getHdrCapabilities(), null when there are none
     * @param dolbyVisionDecoders the Dolby Vision bits a hardware decoder takes:
     *                     {@link #DOLBY_VISION_HEVC} for video/dolby-vision profile 8 (DvheSt),
     *                     {@link #DOLBY_VISION_AV1} for profile 10 (Dvav110)
     * @param videoFormats the video formats both sides can negotiate (MoonBridge.VIDEO_FORMAT_*)
     * @param surface      who draws the picture
     * @return one bit to ask for, or 0 for stock HDR10
     */
    public static int pick(int hostFormats, int[] displayTypes, int dolbyVisionDecoders, int videoFormats,
                           DrawnSurface surface) {
        if (surface == DrawnSurface.PQ_ONLY) return 0;
        if (surface == DrawnSurface.NONE && has(displayTypes, Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION)) {
            for (int format : DOLBY_VISION_ORDER) {
                if ((hostFormats & dolbyVisionDecoders & format) != 0 && (videoFormats & carriers(format)) != 0) {
                    return format;
                }
            }
        }
        int format = 0;
        if ((hostFormats & HLG) != 0 && !HdrPolicy.hasHdr10(displayTypes) &&
                has(displayTypes, Display.HdrCapabilities.HDR_TYPE_HLG)) {
            format = HLG;
        }
        // A drawn surface takes nothing beyond PQ and HLG, whatever formats come later.
        return surface == DrawnSurface.NONE ? format : format & (PQ | HLG);
    }

    /**
     * What the Vulkan renderer leaves out that this screen would show from the decoder's own output,
     * said once per PC: Dolby Vision. HDR10+ is not left out: the renderer hands it to its window
     * (passesHdr10Plus), and says so if the window refuses it.
     */
    public static DrawnLimit vulkanLimit(int hostFormats, int[] displayTypes, int dolbyVisionDecoders,
                                         int videoFormats) {
        if (hostFormats == 0) return DrawnLimit.NONE;
        if (dolbyVision(pick(hostFormats, displayTypes, dolbyVisionDecoders, videoFormats, DrawnSurface.NONE))) {
            return DrawnLimit.DOLBY_VISION;
        }
        return DrawnLimit.NONE;
    }

    /**
     * The Vulkan renderer would ask this host for HLG, so the window is asked whether it takes it
     * first: the renderer draws HLG (surface PQ_HLG) and the screen shows HDR only as HLG.
     */
    public static boolean vulkanNeedsHlgProbe(int hostFormats, int[] displayTypes, DrawnSurface surface) {
        return surface == DrawnSurface.PQ_HLG && pick(hostFormats, displayTypes, 0, 0, surface) == HLG;
    }

    /**
     * What the Vulkan renderer draws on this display's window: surface, PQ only where the window
     * refused HLG. For the HDR10_HLG colour space (Android 15+) the loader sets the same dataspace
     * through ANativeWindow::perform, so the probe answers for both of the renderer's HDR paths.
     */
    public static DrawnSurface vulkanSurface(DrawnSurface surface, HlgWindow window) {
        return surface == DrawnSurface.PQ_HLG && !window.accepted() ? DrawnSurface.PQ_ONLY : surface;
    }

    /**
     * Whether the Vulkan renderer takes the HDR10+ metadata out of each frame and hands it to its
     * output window: only for a screen that reports HDR10+ and a PQ stream (HDR10+ describes PQ
     * pictures only, and HLG streams carry none). Any host's HDR10+ SEI or metadata OBU counts.
     * @param transfer the stream's MediaFormat colour transfer (transfer())
     */
    public static boolean passesHdr10Plus(int[] displayTypes, int transfer) {
        return transfer == MediaFormat.COLOR_TRANSFER_ST2084 &&
                has(displayTypes, Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS);
    }

    /** What PyroWave draws on: PQ and HLG from a host that offers HLG on PyroWave, else PQ only. */
    public static DrawnSurface pyroWaveSurface(int hostFormats) {
        return (hostFormats & PYROWAVE_HLG) != 0 && (hostFormats & HLG) != 0 ? DrawnSurface.PQ_HLG :
                DrawnSurface.PQ_ONLY;
    }

    /** What PyroWave draws on this display's window: pyroWaveSurface, PQ only where it refused HLG. */
    public static DrawnSurface pyroWaveSurface(int hostFormats, HlgWindow window) {
        return window.accepted() ? pyroWaveSurface(hostFormats) : DrawnSurface.PQ_ONLY;
    }

    /**
     * PyroWave would ask this host for HLG, so the window is asked whether it takes it first:
     * the host offers HLG on PyroWave and the screen shows HDR only as HLG.
     */
    public static boolean pyroWaveNeedsHlgProbe(int hostFormats, int[] displayTypes) {
        return pick(hostFormats, displayTypes, 0, 0, pyroWaveSurface(hostFormats)) == HLG;
    }

    /** That screen gets HDR10 on PyroWave because its window refused the HLG dataspace. */
    public static boolean pyroWaveHlgRefusedByDisplay(int hostFormats, int[] displayTypes, HlgWindow window) {
        return window == HlgWindow.REFUSED && pyroWaveNeedsHlgProbe(hostFormats, displayTypes);
    }

    /**
     * An HLG PyroWave stream, asked for because the window took HLG, that is not shown as HLG: the
     * window no longer holds the dataspace after the first frame, or the decoder stopped producing
     * frames. Said loudly; never an SDR or PQ picture without a word.
     * @param window        this stream's probe, null when there was none
     * @param windowKeptHlg the renderer read BT2020_HLG back from the window (or has not shown a frame yet)
     */
    public static boolean pyroWaveHlgLost(int announcedFormat, HlgWindow window, boolean windowKeptHlg,
            boolean framesStopped) {
        return announcedFormat == HLG && window != null && window.accepted() && (!windowKeptHlg || framesStopped);
    }

    /**
     * What PyroWave leaves out that this screen would show from HEVC or AV1, said once per PC:
     * Dolby Vision first (it needs the device's Dolby Vision decoder, so HEVC or AV1), then HDR10+ on a PQ
     * stream (PyroWave carries no dynamic metadata; HLG carries none anyway).
     * @param format what pick chose for PyroWave
     */
    public static DrawnLimit pyroWaveLimit(int hostFormats, int[] displayTypes, int dolbyVisionDecoders,
                                           int format) {
        if (hostFormats == 0) return DrawnLimit.NONE;
        // Either codec could carry it instead of PyroWave, whatever this stream negotiates.
        int carriers = MoonBridge.VIDEO_FORMAT_H265_MAIN10 | MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        if (dolbyVision(pick(hostFormats, displayTypes, dolbyVisionDecoders, carriers, DrawnSurface.NONE))) {
            return DrawnLimit.DOLBY_VISION;
        }
        if (transfer(format) == MediaFormat.COLOR_TRANSFER_ST2084 &&
                has(displayTypes, Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS)) {
            return DrawnLimit.HDR10_PLUS;
        }
        return DrawnLimit.NONE;
    }

    /**
     * A screen that shows HDR only as HLG gets HDR10 on PyroWave because the host offers HLG but not
     * on PyroWave: updating the host would give it HLG.
     */
    public static boolean pyroWaveHlgNeedsHostUpdate(int hostFormats, int[] displayTypes) {
        return (hostFormats & PYROWAVE_HLG) == 0 && !HdrPolicy.hasHdr10(displayTypes) &&
                hlgInstead(hostFormats, displayTypes);
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
        return overlayWord(format, dolbyVisionDecoding, false);
    }

    /** As above; hdr10Plus: the stream's HDR10+ metadata reaches the screen through the Vulkan renderer. */
    public static String overlayWord(int format, boolean dolbyVisionDecoding, boolean hdr10Plus) {
        if (hdr10Plus && !dolbyVision(format) && transfer(format) == MediaFormat.COLOR_TRANSFER_ST2084) return "HDR10+";
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
