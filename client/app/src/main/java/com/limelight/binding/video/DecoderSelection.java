package com.limelight.binding.video;

import android.media.MediaCodecInfo;
import android.os.Build;
import android.util.Range;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.List;

/**
 * Which MediaCodec decoder runs each codec, and which stream formats a decoder can take. Stateless:
 * MediaCodecDecoderRenderer keeps the decoders chosen here.
 */
final class DecoderSelection {
    private DecoderSelection() {}

    static MediaCodecInfo findAvcDecoder(boolean yuv444) {
        MediaCodecInfo decoder = yuv444 ?
                MediaCodecHelper.findProbableSafeDecoder("video/avc", MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444) : null;
        if (decoder == null) {
            decoder = MediaCodecHelper.findProbableSafeDecoder("video/avc", MediaCodecInfo.CodecProfileLevel.AVCProfileHigh);
        }
        if (decoder == null) {
            decoder = MediaCodecHelper.findFirstDecoder("video/avc");
        }
        return decoder;
    }

    private static boolean decoderCanMeetPerformancePoint(MediaCodecInfo.VideoCapabilities caps, PreferenceConfiguration prefs) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            MediaCodecInfo.VideoCapabilities.PerformancePoint targetPerfPoint = new MediaCodecInfo.VideoCapabilities.PerformancePoint(prefs.width, prefs.height, prefs.fps);
            List<MediaCodecInfo.VideoCapabilities.PerformancePoint> perfPoints = caps.getSupportedPerformancePoints();
            if (perfPoints != null) {
                for (MediaCodecInfo.VideoCapabilities.PerformancePoint perfPoint : perfPoints) {
                    // If we find a performance point that covers our target, we're good to go
                    if (perfPoint.covers(targetPerfPoint)) {
                        return true;
                    }
                }

                // We had performance point data but none met the specified streaming settings
                return false;
            }

            // Fall-through to try the Android M API if there's no performance point data
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            try {
                // We'll ask the decoder what it can do for us at this resolution and see if our
                // requested frame rate falls below or inside the range of achievable frame rates.
                Range<Double> fpsRange = caps.getAchievableFrameRatesFor(prefs.width, prefs.height);
                if (fpsRange != null) {
                    return prefs.fps <= fpsRange.getUpper();
                }

                // Fall-through to try the Android L API if there's no performance point data
            } catch (IllegalArgumentException e) {
                // Video size not supported at any frame rate
                return false;
            }
        }

        // As a last resort, we will use areSizeAndRateSupported() which is explicitly NOT a
        // performance metric, but it can work at least for the purpose of determining if
        // the codec is going to die when given a stream with the specified settings.
        return caps.areSizeAndRateSupported(prefs.width, prefs.height, prefs.fps);
    }

    /** Whether only the candidate decoder, not the one it would replace, meets the stream's performance point. */
    private static boolean onlyCandidateMeetsPerformancePoint(MediaCodecInfo candidate, String candidateMime,
                                                              MediaCodecInfo replaced, String replacedMime,
                                                              PreferenceConfiguration prefs) {
        MediaCodecInfo.VideoCapabilities replacedCaps = replaced.getCapabilitiesForType(replacedMime).getVideoCapabilities();
        MediaCodecInfo.VideoCapabilities candidateCaps = candidate.getCapabilitiesForType(candidateMime).getVideoCapabilities();

        return !decoderCanMeetPerformancePoint(replacedCaps, prefs) && decoderCanMeetPerformancePoint(candidateCaps, prefs);
    }

    static MediaCodecInfo findHevcDecoder(PreferenceConfiguration prefs, boolean requestedHdr, boolean wantsSdr10,
                                          MediaCodecInfo avcDecoder) {
        // Don't return anything if H.264 is forced
        if (prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_H264) {
            return null;
        }

        // We don't try the first HEVC decoder. We'd rather fall back to hardware accelerated AVC instead
        //
        // We need HEVC Main profile, so we could pass that constant to findProbableSafeDecoder, however
        // some decoders (at least Qualcomm's Snapdragon 805) don't properly report support
        // for even required levels of HEVC.
        MediaCodecInfo hevcDecoderInfo = requestedHdr ?
                MediaCodecHelper.findProbableSafeDecoder("video/hevc", MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10) : null;
        if (hevcDecoderInfo == null && requestedHdr) {
            hevcDecoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/hevc", MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10Plus);
        }
        if (hevcDecoderInfo == null && wantsSdr10) {
            hevcDecoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/hevc", MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10);
        }
        if (hevcDecoderInfo == null && prefs.enableYuv444) {
            hevcDecoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/hevc", MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444);
        }
        if (hevcDecoderInfo == null) {
            hevcDecoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/hevc", -1);
        }
        if (hevcDecoderInfo != null) {
            if (!MediaCodecHelper.decoderIsWhitelistedForHevc(hevcDecoderInfo)) {
                LimeLog.info("Found HEVC decoder, but it's not whitelisted - "+hevcDecoderInfo.getName());

                // Force HEVC enabled if the user asked for it
                if (prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_HEVC) {
                    LimeLog.info("Forcing HEVC enabled despite non-whitelisted decoder");
                }
                // HDR implies HEVC forced on, since HEVCMain10HDR10 is required for HDR.
                else if (requestedHdr || (wantsSdr10 && reportsMain10(hevcDecoderInfo, "video/hevc", prefs))) {
                    LimeLog.info("Forcing HEVC enabled for HDR or 10-bit SDR streaming");
                }
                else if ((decoderVideoFormats(hevcDecoderInfo, "video/hevc", false, false, prefs.enableYuv444, prefs) &
                        MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0) {
                    LimeLog.info("Using HEVC for requested 4:4:4 streaming");
                }
                // > 4K streaming also requires HEVC, so force it on there too.
                else if (prefs.width > 4096 || prefs.height > 4096) {
                    LimeLog.info("Forcing HEVC enabled for over 4K streaming");
                }
                // Use HEVC if the H.264 decoder is unable to meet the performance point
                else if (avcDecoder != null && onlyCandidateMeetsPerformancePoint(hevcDecoderInfo, "video/hevc",
                        avcDecoder, "video/avc", prefs)) {
                    LimeLog.info("Using non-whitelisted HEVC decoder to meet performance point");
                }
                else {
                    return null;
                }
            }
        }

        return hevcDecoderInfo;
    }

    static MediaCodecInfo findAv1Decoder(PreferenceConfiguration prefs, boolean requestedHdr, boolean wantsSdr10,
                                         MediaCodecInfo avcDecoder, MediaCodecInfo hevcDecoder) {
        if (prefs.videoFormat != PreferenceConfiguration.FormatOption.FORCE_AV1 &&
                prefs.videoFormat != PreferenceConfiguration.FormatOption.AUTO &&
                !(requestedHdr && prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE)) {
            return null;
        }

        MediaCodecInfo decoderInfo = requestedHdr ?
                MediaCodecHelper.findProbableSafeDecoder("video/av01", MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10) : null;
        if (decoderInfo == null && requestedHdr) {
            decoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/av01", MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10Plus);
        }
        if (decoderInfo == null && wantsSdr10) {
            decoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/av01", MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10);
        }
        if (decoderInfo == null) {
            decoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/av01", -1);
        }
        if (decoderInfo != null) {
            if (!MediaCodecHelper.isDecoderWhitelistedForAv1(decoderInfo)) {
                LimeLog.info("Found AV1 decoder, but it's not whitelisted - "+decoderInfo.getName());

                // Force AV1 enabled if the user asked for it
                if (prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1 || requestedHdr ||
                        (wantsSdr10 && reportsMain10(decoderInfo, "video/av01", prefs))) {
                    LimeLog.info("Forcing AV1 enabled despite non-whitelisted decoder");
                }
                // Use AV1 if the HEVC decoder is unable to meet the performance point
                else if (hevcDecoder != null && onlyCandidateMeetsPerformancePoint(decoderInfo, "video/av01",
                        hevcDecoder, "video/hevc", prefs)) {
                    LimeLog.info("Using non-whitelisted AV1 decoder to meet performance point");
                }
                // Use AV1 if the H.264 decoder is unable to meet the performance point and we have no HEVC decoder
                else if (hevcDecoder == null && avcDecoder != null && onlyCandidateMeetsPerformancePoint(decoderInfo,
                        "video/av01", avcDecoder, "video/avc", prefs)) {
                    LimeLog.info("Using non-whitelisted AV1 decoder to meet performance point");
                }
                else {
                    return null;
                }
            }
        }

        return decoderInfo;
    }

    /** The decoder lists a Main10 profile (HDR10 profiles included), the least 10-bit SDR needs. */
    static boolean reportsMain10(MediaCodecInfo decoder, String mime, PreferenceConfiguration prefs) {
        return (decoderVideoFormats(decoder, mime, false, true, false, prefs) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    static int videoFormatsForProfiles(String mime, int[] profiles, boolean hdr, boolean yuv444) {
        return videoFormatsForProfiles(mime, profiles, hdr, false, yuv444);
    }

    /**
     * @param hdr the HDR10 profiles give the 10-bit formats
     * @param sdr10 plain Main10 gives them too (10-bit SDR needs no HDR10 profile)
     */
    static int videoFormatsForProfiles(String mime, int[] profiles, boolean hdr, boolean sdr10, boolean yuv444) {
        int formats = mime.equals("video/avc") ? MoonBridge.VIDEO_FORMAT_H264 :
                mime.equals("video/hevc") ? MoonBridge.VIDEO_FORMAT_H265 : MoonBridge.VIDEO_FORMAT_AV1_MAIN8;
        for (int profile : profiles) {
            if (mime.equals("video/avc")) {
                if (yuv444 && profile == MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444) {
                    formats |= MoonBridge.VIDEO_FORMAT_H264_HIGH8_444;
                }
            } else if (mime.equals("video/hevc")) {
                if ((hdr || sdr10) && (profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10 ||
                        profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10Plus) ||
                        sdr10 && profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10) {
                    formats |= MoonBridge.VIDEO_FORMAT_H265_MAIN10;
                }
                // API 37 adds this 8-bit profile. Older devices never advertise it.
                if (yuv444 && profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444) {
                    formats |= MoonBridge.VIDEO_FORMAT_H265_REXT8_444;
                }
            } else if ((hdr || sdr10) && (profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10 ||
                    profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10Plus) ||
                    sdr10 && profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10) {
                formats |= MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
            }
        }
        return formats;
    }

    /** The stream formats decoder offers for mime; 4:4:4 only when it takes the stream's size and rate. */
    static int decoderVideoFormats(MediaCodecInfo decoder, String mime, boolean hdr, boolean sdr10, boolean yuv444,
                                   PreferenceConfiguration prefs) {
        if (decoder == null) {
            return 0;
        }
        int baseFormat = videoFormatsForProfiles(mime, new int[0], false, false);
        // A software decoder (kept on emulators by SHOULD_BYPASS_SOFTWARE_BLOCK) renders 10-bit
        // output corrupt, so it never offers a 10-bit format.
        if ((hdr || sdr10) && MediaCodecHelper.isSoftwareDecoder(decoder)) {
            LimeLog.info("Not offering 10-bit or HDR for software decoder " + decoder.getName());
            hdr = false;
            sdr10 = false;
        }
        if (!hdr && !sdr10 && !yuv444) {
            return baseFormat;
        }
        try {
            MediaCodecInfo.CodecCapabilities caps = decoder.getCapabilitiesForType(mime);
            int[] profiles = new int[caps.profileLevels.length];
            for (int i = 0; i < profiles.length; i++) {
                profiles[i] = caps.profileLevels[i].profile;
            }
            return videoFormatsForProfiles(mime, profiles, hdr, sdr10, yuv444 &&
                    caps.getVideoCapabilities().areSizeAndRateSupported(prefs.width, prefs.height, prefs.fps));
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot query decoder profiles; using SDR 4:2:0: " + e);
            return baseFormat;
        }
    }
}
