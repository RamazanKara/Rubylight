package com.limelight.binding.video;

import android.media.MediaFormat;
import android.os.Build;
import android.view.Display;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * The decisions around an HDR10 stream, apart from the Android objects so they can be tested with
 * made-up capabilities. HDR is asked for only when the setting is on and the screen reports HDR10
 * (not just Dolby Vision; HLG only when the host streams HLG and the decoder or the Vulkan renderer
 * puts the picture on the screen, see HdrFormatPolicy); from Android 14 the capabilities follow the mode the screen is in,
 * so a mode without HDR10 is refused, with its own reason when another mode has it; a decoder
 * with a Main10 HDR10 profile is needed too. Whatever is asked for and not granted, or granted and then not delivered, is named
 * rather than left to look like SDR. Nothing here has run on an HDR panel; see
 * docs/expected-hdr10.md.
 */
public final class HdrPolicy {
    private HdrPolicy() {}

    public enum Refusal {
        NONE,
        SETTING_OFF,
        /** The screen reports no HDR10, or no HDR capabilities at all. */
        DISPLAY_WITHOUT_HDR10,
        /** Android 14 and newer: the mode the screen is in has no HDR10, but another mode does. */
        MODE_WITHOUT_HDR10,
        DECODER_WITHOUT_HDR10,
        /**
         * The screen shows HDR only as HLG, but the client would draw the picture itself on a
         * BT.2020/PQ surface (the Vulkan renderer without VK_EXT_swapchain_colorspace), so the
         * stream could only be HDR10.
         */
        HLG_NEEDS_DIRECT_OUTPUT
    }

    /**
     * @param displayTypes the types from Display.getHdrCapabilities(), null when the call gave no
     *                     capabilities (some devices return null). From Android 14 these follow the
     *                     active mode.
     * @param anyModeTypes the types of all the screen's modes together, null before Android 14 (a
     *                     mode has no list there). Only used to name the reason; HDR is never
     *                     asked for because some other mode has it.
     */
    public static Refusal displayRefusal(int sdk, boolean enabled, int[] displayTypes, int[] anyModeTypes) {
        return displayRefusal(sdk, enabled, displayTypes, anyModeTypes, 0);
    }

    /**
     * @param hostHdrFormats the host's HdrFormats bits (HdrFormatPolicy); a screen with HLG and no
     *                       HDR10 gets HDR when the host offers HLG, which is then asked for.
     */
    public static Refusal displayRefusal(int sdk, boolean enabled, int[] displayTypes, int[] anyModeTypes,
                                         int hostHdrFormats) {
        return displayRefusal(sdk, enabled, displayTypes, anyModeTypes, hostHdrFormats,
                HdrFormatPolicy.DrawnSurface.NONE);
    }

    /**
     * @param surface who will put the picture on the screen (the Vulkan renderer as
     *                MediaCodecDecoderRenderer.vulkanSurface predicts it): HLG cannot stand in for
     *                HDR10 on a PQ-only surface
     */
    static int displayTypesMask(int[] types) {
        if (types == null) return 0;
        int mask = 0;
        for (int type : types) {
            if (type == Display.HdrCapabilities.HDR_TYPE_HDR10) mask |= NightjarPolicyJni.DISPLAY_HDR_HDR10;
            else if (type == Display.HdrCapabilities.HDR_TYPE_HLG) mask |= NightjarPolicyJni.DISPLAY_HDR_HLG;
            else if (type == Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION) mask |= NightjarPolicyJni.DISPLAY_HDR_DOLBY_VISION;
            else if (type == Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS) mask |= NightjarPolicyJni.DISPLAY_HDR_HDR10_PLUS;
        }
        return mask;
    }

    static int transferCode(Integer transfer) {
        if (transfer == null) return NightjarPolicyJni.TRANSFER_NONE;
        if (transfer == MediaFormat.COLOR_TRANSFER_ST2084) return NightjarPolicyJni.TRANSFER_PQ;
        if (transfer == MediaFormat.COLOR_TRANSFER_HLG) return NightjarPolicyJni.TRANSFER_HLG;
        return NightjarPolicyJni.TRANSFER_OTHER;
    }

    public static Refusal displayRefusal(int sdk, boolean enabled, int[] displayTypes, int[] anyModeTypes,
                                         int hostHdrFormats, HdrFormatPolicy.DrawnSurface surface) {
        int surf = surface == null ? HdrFormatPolicy.DrawnSurface.NONE.ordinal() : surface.ordinal();
        return Refusal.values()[NightjarPolicyJni.hdrDisplayRefusal(
                sdk, enabled, displayTypesMask(displayTypes), displayTypesMask(anyModeTypes), hostHdrFormats, surf)];
    }

    // The hidden debug_hdr_hlg_only preference (debug builds only, no UI): an HDR screen counts as
    // HLG-only, so a screen with HDR10 asks for HLG. For testing HLG on screens that also have HDR10.
    private static volatile boolean debugHlgOnly;

    /** Takes the debug_hdr_hlg_only preference, which counts only in a debug build. */
    public static void setDebugHlgOnly(boolean debugBuild, boolean hlgOnly) {
        debugHlgOnly = debugBuild && hlgOnly;
        if (debugHlgOnly) LimeLog.warning("debug_hdr_hlg_only: HDR screens count as HLG-only");
    }

    /** The screen's types as the policy sees them: HLG alone for an HDR screen under debug_hdr_hlg_only. */
    static int[] effectiveHdrTypes(int[] types) {
        if (!NightjarPolicyJni.hdrShouldForceHlgOnly(debugHlgOnly, displayTypesMask(types))) {
            return types;
        }
        return new int[] {Display.HdrCapabilities.HDR_TYPE_HLG};
    }

    /** The screen's HDR types, null when Android gives none (before Android 7, and a few devices return null). */
    public static int[] displayHdrTypes(Display display) {
        Display.HdrCapabilities capabilities = display.getHdrCapabilities();
        return capabilities == null ? null : effectiveHdrTypes(capabilities.getSupportedHdrTypes());
    }

    /** The types any of the screen's modes offers; null before Android 14, which has no per-mode list. */
    public static int[] anyModeHdrTypes(Display display) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return null;
        Display.Mode[] modes = display.getSupportedModes();
        if (modes == null) return null;
        java.util.TreeSet<Integer> types = new java.util.TreeSet<>();
        for (Display.Mode mode : modes) {
            for (int type : mode.getSupportedHdrTypes()) types.add(type);
        }
        int[] all = new int[types.size()];
        int i = 0;
        for (int type : types) all[i++] = type;
        return effectiveHdrTypes(all);
    }

    public static Refusal displayRefusal(Display display, boolean enabled, int hostHdrFormats,
                                         HdrFormatPolicy.DrawnSurface surface) {
        return displayRefusal(Build.VERSION.SDK_INT, enabled, displayHdrTypes(display), anyModeHdrTypes(display),
                hostHdrFormats, surface);
    }

    /** HDR10 PQ is what the PC encodes; HLG and Dolby Vision on the panel do not stand in for it. */
    /** Whether the screen reports Dolby Vision. */
    public static boolean hasDolbyVision(int[] types) {
        return (displayTypesMask(types) & NightjarPolicyJni.DISPLAY_HDR_DOLBY_VISION) != 0;
    }

    public static boolean hasHdr10(int[] types) {
        return NightjarPolicyJni.hdrHasHdr10(displayTypesMask(types));
    }

    /** The screen shows HDR as HLG and not as HDR10: only an HLG stream is HDR on it. */
    public static boolean hlgOnly(int[] displayTypes) {
        return NightjarPolicyJni.hdrHlgOnly(displayTypesMask(displayTypes));
    }

    /**
     * Whether the screen is reported to the PC as an HDR output (displayHdr in the display caps):
     * with HDR10, or with HLG alone while this stream's transfer is HLG. An HLG-only screen is never
     * reported for a PQ stream, which it cannot show.
     * @param streamTransfer the stream's transfer (HdrFormatPolicy.transfer of the announced format)
     */
    public static boolean reportsHdrOutput(int[] displayTypes, int streamTransfer) {
        return NightjarPolicyJni.hdrReportsOutput(displayTypesMask(displayTypes), transferCode(streamTransfer));
    }

    public static Refusal decoderRefusal(boolean hevcMain10Hdr10, boolean av1Main10, boolean pyroWave) {
        return Refusal.values()[NightjarPolicyJni.hdrDecoderRefusal(hevcMain10Hdr10, av1Main10, pyroWave)];
    }

    /** How the host's mastering metadata looks, from its 24 byte wire form (CTA-861.3 without the type byte). */
    public enum Metadata {
        /** None came with the HDR report. */
        ABSENT,
        /** Present but empty: no primaries, or a maximum luminance of zero. */
        EMPTY,
        USABLE
    }

    public static Metadata metadata(byte[] wire) {
        return Metadata.values()[NightjarPolicyJni.hdrMetadata(wire)];
    }

    /** What the running stream says about HDR, for the overlay's status line. */
    public enum Status {
        /** HDR was not asked for, or it is working and the overlay's colour word says HDR. */
        QUIET,
        /** Asked for, but the PC's answer is not in yet. */
        WAITING,
        /** Asked for, but the PC did not send HDR10 for this stream. */
        NOT_NEGOTIATED,
        /** The PC reported its picture as SDR. */
        HOST_SDR,
        /** HDR10 is on without usable mastering data from the PC. */
        NO_METADATA,
        /** The decoder says its output is not PQ, so the compositor would treat it as SDR. */
        OUTPUT_NOT_PQ,
        /**
         * HDR was asked for because the screen shows HLG, but the stream is PQ: the client draws
         * PQ only (packed full chroma, PyroWave), or the PC did not offer HLG for this stream, or
         * the codec carries none.
         */
        PQ_ON_HLG_SCREEN,
        /** The PC sends HDR, but the Vulkan renderer's surface refused it, so it is tone mapped to SDR. */
        SHOWN_AS_SDR
    }

    /**
     * The status with what the Vulkan renderer reports: HDR the PC sends and the renderer tone maps
     * replaces a quiet status, and the missing metadata that no longer matters.
     * @param toneMapped VulkanRendererBridge.isHdrToneMapped, false without the Vulkan renderer
     */
    public static Status drawnStatus(Status status, boolean toneMapped) {
        if (status == null) return null;
        return Status.values()[NightjarPolicyJni.hdrDrawnStatus(status.ordinal(), toneMapped)];
    }

    /**
     * Whether the stream hands back from the Vulkan renderer to the standard one: only when the PC
     * reported the stream as HDR and the Vulkan renderer can only tone map it, since the standard
     * renderer shows that HDR on this screen. A stream the PC reports as SDR stays where it is.
     * @param hdrReported the PC's HDR report is on (currentHdrMode is TRUE)
     * @param toneMapped the Vulkan renderer tone maps its current swapchain (isHdrToneMapped)
     */
    public static boolean handsBackFromVulkan(boolean hdrReported, boolean toneMapped) {
        return NightjarPolicyJni.hdrHandsBackFromVulkan(hdrReported, toneMapped);
    }

    /**
     * Whether such a stream may move to the standard renderer at this decoder restart: only when
     * the Vulkan renderer really let go of the output window. A stop that gave up after
     * RendererGate.STOP_WAIT_MS is still running, and its native part may not have released the
     * window yet, so MediaCodec must not be configured onto that same window; with no renderer to
     * stop there is nothing holding it.
     * @param hadRenderer the stream was drawing through the Vulkan renderer
     * @param stopFinished VulkanRendererBridge.stop returned true: the native stop completed
     */
    public static boolean handOverAfterVulkanStop(boolean hadRenderer, boolean stopFinished) {
        return NightjarPolicyJni.hdrHandOverAfterVulkanStop(hadRenderer, stopFinished);
    }

    /**
     * @param asked this stream asked for HDR
     * @param tenBit the negotiated format is 10-bit
     * @param hostHdr the host's HDR report (setHdrMode), null before it arrives
     * @param metadata the metadata that came with it
     * @param outputTransfer the decoder's reported colour transfer, null when it reports none
     */
    public static Status status(boolean asked, boolean tenBit, Boolean hostHdr, Metadata metadata,
                                Integer outputTransfer) {
        return status(asked, tenBit, hostHdr, metadata, outputTransfer, MediaFormat.COLOR_TRANSFER_ST2084);
    }

    /**
     * @param streamTransfer the stream's transfer (HdrFormatPolicy.transfer): an HLG stream's output
     *                       is HLG, and HLG carries no mastering metadata to miss
     */
    public static Status status(boolean asked, boolean tenBit, Boolean hostHdr, Metadata metadata,
                                Integer outputTransfer, int streamTransfer) {
        return status(asked, tenBit, hostHdr, metadata, outputTransfer, streamTransfer, false);
    }

    /**
     * @param hlgOnlyScreen the screen shows HDR only as HLG (hlgOnly): a PQ stream is named first
     */
    public static Status status(boolean asked, boolean tenBit, Boolean hostHdr, Metadata metadata,
                                Integer outputTransfer, int streamTransfer, boolean hlgOnlyScreen) {
        int host = hostHdr == null ? -1 : (hostHdr ? 1 : 0);
        int meta = metadata == null ? Metadata.ABSENT.ordinal() : metadata.ordinal();
        return Status.values()[NightjarPolicyJni.hdrStatus(
                asked, tenBit, host, meta, transferCode(outputTransfer), transferCode(streamTransfer), hlgOnlyScreen)];
    }
}
