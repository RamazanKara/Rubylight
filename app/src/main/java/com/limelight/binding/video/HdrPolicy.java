package com.limelight.binding.video;

import android.media.MediaFormat;
import android.os.Build;
import android.view.Display;

/**
 * The decisions around an HDR10 stream, apart from the Android objects so they can be tested with
 * made-up capabilities. HDR is asked for only when the setting is on and the screen reports HDR10
 * (not just HLG or Dolby Vision) for the mode it is in now; a decoder with a Main10 HDR10 profile
 * is needed too. Whatever is asked for and not granted, or granted and then not delivered, is named
 * rather than left to look like SDR. Nothing here has run on an HDR panel; see
 * docs/expected-hdr10.md.
 */
public final class HdrPolicy {
    private HdrPolicy() {}

    public enum Refusal {
        NONE,
        SETTING_OFF,
        /** Before Android 7 there are no HDR keys on MediaFormat. */
        ANDROID_TOO_OLD,
        /** The screen reports no HDR10, or no HDR capabilities at all. */
        DISPLAY_WITHOUT_HDR10,
        /** Android 14 and newer: the screen has HDR10, but not in the mode it is in now. */
        MODE_WITHOUT_HDR10,
        DECODER_WITHOUT_HDR10
    }

    /**
     * @param displayTypes the types from Display.getHdrCapabilities(), null when the call gave no
     *                     capabilities (some devices return null)
     * @param modeTypes the active mode's types, null before Android 14 (the mode has no list there)
     */
    public static Refusal displayRefusal(int sdk, boolean enabled, int[] displayTypes, int[] modeTypes) {
        if (!enabled) return Refusal.SETTING_OFF;
        if (sdk < 24) return Refusal.ANDROID_TOO_OLD;
        if (!hasHdr10(displayTypes)) return Refusal.DISPLAY_WITHOUT_HDR10;
        if (sdk >= 34 && modeTypes != null && !hasHdr10(modeTypes)) return Refusal.MODE_WITHOUT_HDR10;
        return Refusal.NONE;
    }

    /** The screen's HDR types, null when Android gives none (before Android 7, and a few devices return null). */
    public static int[] displayHdrTypes(Display display) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return null;
        Display.HdrCapabilities capabilities = display.getHdrCapabilities();
        return capabilities == null ? null : capabilities.getSupportedHdrTypes();
    }

    /** The types of the mode the screen is in now; null before Android 14, which has no per-mode list. */
    public static int[] modeHdrTypes(Display display) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return null;
        Display.Mode mode = display.getMode();
        return mode == null ? null : mode.getSupportedHdrTypes();
    }

    public static Refusal displayRefusal(Display display, boolean enabled) {
        return displayRefusal(Build.VERSION.SDK_INT, enabled, displayHdrTypes(display), modeHdrTypes(display));
    }

    /** HDR10 PQ is what the PC encodes; HLG and Dolby Vision on the panel do not stand in for it. */
    public static boolean hasHdr10(int[] types) {
        if (types == null) return false;
        for (int type : types) {
            if (type == Display.HdrCapabilities.HDR_TYPE_HDR10) return true;
        }
        return false;
    }

    public static Refusal decoderRefusal(boolean hevcMain10Hdr10, boolean av1Main10, boolean pyroWave) {
        return hevcMain10Hdr10 || av1Main10 || pyroWave ? Refusal.NONE : Refusal.DECODER_WITHOUT_HDR10;
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
        if (wire == null || wire.length < 24) return Metadata.ABSENT;
        boolean primaries = false;
        for (int i = 0; i < 12; i++) primaries |= wire[i] != 0;
        int maxLuminance = (wire[16] & 0xff) | (wire[17] & 0xff) << 8;
        return primaries && maxLuminance > 0 ? Metadata.USABLE : Metadata.EMPTY;
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
        OUTPUT_NOT_PQ
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
        if (!asked) return Status.QUIET;
        if (!tenBit) return Status.NOT_NEGOTIATED;
        if (hostHdr == null) return Status.WAITING;
        if (!hostHdr) return Status.HOST_SDR;
        if (outputTransfer != null && outputTransfer != MediaFormat.COLOR_TRANSFER_ST2084) {
            return Status.OUTPUT_NOT_PQ;
        }
        return metadata == Metadata.USABLE ? Status.QUIET : Status.NO_METADATA;
    }
}
