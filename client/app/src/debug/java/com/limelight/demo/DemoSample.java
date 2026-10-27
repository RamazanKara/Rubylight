package com.limelight.demo;

import android.media.MediaCodecInfo;
import android.media.MediaCodecList;

import com.limelight.binding.video.MediaCodecHelper;

/**
 * Picks the bundled demo sample. AV1 and HEVC are 10-bit, H.264 is 8-bit; a software decoder
 * renders the 10-bit ones corrupt on emulators, so those need a hardware decoder.
 */
final class DemoSample {
    private DemoSample() {
    }

    /**
     * @param hardwareAv1 a hardware AV1 decoder exists
     * @param hardwareHevc a hardware HEVC decoder exists
     * @param hasAv1 the AV1 asset is bundled
     * @param hasHevc the HEVC asset is bundled
     * @param forceH264 the launch asked for the 8-bit sample
     * @param fallback a previous sample failed to play
     */
    static String choose(boolean hardwareAv1, boolean hardwareHevc, boolean hasAv1, boolean hasHevc,
                         boolean forceH264, boolean fallback) {
        if (forceH264 || fallback) return "h264";
        if (hardwareAv1 && hasAv1) return "av1";
        if (hardwareHevc && hasHevc) return "hevc";
        return "h264";
    }

    /** The device has a decoder for the mime type that is not software-only. */
    static boolean hasHardwareDecoder(String mime) {
        for (MediaCodecInfo info : new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos()) {
            if (info.isEncoder() || MediaCodecHelper.isSoftwareDecoder(info)) continue;
            for (String type : info.getSupportedTypes()) {
                if (mime.equalsIgnoreCase(type)) return true;
            }
        }
        return false;
    }

    /** The overlay's codec name and dynamic range for a sample. */
    static String label(String sample) {
        switch (sample) {
            case "av1": return "AV1 · HDR";
            case "hevc": return "HEVC · HDR";
            default: return "H.264 · SDR";
        }
    }

    /** The overlay's stream-details line for a sample. */
    static String detail(String sample) {
        switch (sample) {
            case "av1": return "AV1 · 10-bit HDR";
            case "hevc": return "HEVC · 10-bit HDR";
            default: return "H.264 · 8-bit SDR";
        }
    }

    static String decoderName(String sample) {
        switch (sample) {
            case "av1": return "AV1";
            case "hevc": return "HEVC";
            default: return "H.264";
        }
    }
}
