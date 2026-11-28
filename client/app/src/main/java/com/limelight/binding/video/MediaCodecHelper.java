package com.limelight.binding.video;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import android.content.Context;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.preferences.PreferenceConfiguration;

public class MediaCodecHelper {
    public static final boolean SHOULD_BYPASS_SOFTWARE_BLOCK = MediaCodecCapabilities.SHOULD_BYPASS_SOFTWARE_BLOCK;

    // The one Fire TV of erratum 23: its MediaTek decoders can corrupt the picture in Android's
    // low-latency mode. Only this exact model, the only one reported with it. A TCL C6K, no Fire TV
    // but with the same MediaTek AV1 decoder name, showed none in about 50 AV1 HDR starts.
    public static final String LOW_LATENCY_CORRUPTION_MODEL = "AFTNE4E43F";

    private static final List<String> whitelistedHevcDecoders =
            MediaCodecCapabilities.defaultWhitelistedHevcDecoders();
    private static final List<String> refFrameInvalidationHevcPrefixes =
            MediaCodecCapabilities.defaultRefFrameInvalidationHevcPrefixes();

    private static boolean isLowEndSnapdragon = false;
    private static boolean isAdreno620 = false;
    // Amazon Fire TV (the amazon.hardware.fire_tv feature), set by initialize().
    private static boolean isFireTv = false;
    // Read by the settings screen's decoder probe thread and the renderer threads.
    private static volatile boolean initialized = false;

    private static final Map<String, List<String>> vendorParameterCache = new ConcurrentHashMap<>();
    private static final Set<String> unprobeableDecoders = ConcurrentHashMap.newKeySet();

    static boolean isInitialized() {
        return initialized;
    }

    static boolean isFireTv() {
        return isFireTv;
    }

    static void setFireTv(boolean fireTv) {
        isFireTv = fireTv;
    }

    static boolean isLowEndSnapdragon() {
        return isLowEndSnapdragon;
    }

    static void setLowEndSnapdragon(boolean lowEndSnapdragon) {
        isLowEndSnapdragon = lowEndSnapdragon;
    }

    static boolean isAdreno620() {
        return isAdreno620;
    }

    static void setAdreno620(boolean adreno620) {
        isAdreno620 = adreno620;
    }

    static boolean isDecoderInList(List<String> decoderList, String decoderName) {
        if (!initialized) {
            throw new IllegalStateException("MediaCodecHelper must be initialized before use");
        }

        for (String badPrefix : decoderList) {
            if (decoderName.length() >= badPrefix.length()) {
                String prefix = decoderName.substring(0, badPrefix.length());
                if (prefix.equalsIgnoreCase(badPrefix)) {
                    return true;
                }
            }
        }

        return false;
    }

    static boolean usesVendorLowLatency(boolean vendorLowLatency, String decoderName, boolean vulkanOutput) {
        return MediaCodecLowLatencyOptions.usesVendorLowLatency(vendorLowLatency, decoderName, vulkanOutput);
    }

    static boolean isExperimentalVendorDecoder(String decoderName) {
        return MediaCodecLowLatencyOptions.isExperimentalVendorDecoder(decoderName);
    }

    static void addMediaTekAndQcomRefFrameInvalidation() {
        MediaCodecCapabilities.addMediaTekAndQcomRefFrameInvalidation(refFrameInvalidationHevcPrefixes);
    }

    public static synchronized void initialize(Context context, String glRenderer) {
        if (initialized) {
            return;
        }

        MediaCodecCapabilities.initializeDeviceRules(context, glRenderer,
                whitelistedHevcDecoders, refFrameInvalidationHevcPrefixes);

        initialized = true;
    }

    static boolean decoderSupportsAndroidRLowLatency(MediaCodecInfo decoderInfo, String mimeType) {
        return MediaCodecCapabilities.decoderSupportsAndroidRLowLatency(decoderInfo, mimeType);
    }

    /**
     * Integer vendor parameters the decoder declares, or null below Android 12 or when the codec
     * can't be probed. Cached per decoder because probing creates a codec instance.
     */
    static List<String> getIntegerVendorParameters(String decoderName) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            return null;
        }
        if (unprobeableDecoders.contains(decoderName)) {
            return null;
        }
        List<String> cached = vendorParameterCache.get(decoderName);
        if (cached != null) {
            return cached;
        }
        MediaCodec testCodec = null;
        List<String> params = new ArrayList<>();
        try {
            // Unfortunately we have to create an actual codec instance to get supported options.
            testCodec = MediaCodec.createByCodecName(decoderName);
            for (String name : testCodec.getSupportedVendorParameters()) {
                MediaCodec.ParameterDescriptor descriptor = testCodec.getParameterDescriptor(name);
                if (descriptor != null && descriptor.getType() == MediaFormat.TYPE_INTEGER) {
                    params.add(name);
                }
            }
            LimeLog.info(decoderName + " integer vendor parameters: " + params);
        } catch (Exception e) {
            // Tolerate buggy codecs
            e.printStackTrace();
            unprobeableDecoders.add(decoderName);
            return null;
        } finally {
            if (testCodec != null) {
                testCodec.release();
            }
        }
        params = Collections.unmodifiableList(params);
        vendorParameterCache.put(decoderName, params);
        return params;
    }

    static boolean usesVdecLowLatency(String decoderName) {
        return MediaCodecLowLatencyOptions.usesVdecLowLatency(decoderName);
    }

    public static boolean setDecoderLowLatencyOptions(MediaFormat videoFormat, MediaCodecInfo decoderInfo, int tryNumber,
                                                      boolean lowLatency, boolean vendorLowLatency, boolean performanceHints,
                                                      boolean disableHevcLowLatency) {
        return setDecoderLowLatencyOptions(videoFormat, decoderInfo, tryNumber, lowLatency, vendorLowLatency,
                performanceHints, disableHevcLowLatency, false);
    }

    /**
     * Experimental chipset tuning (opt-in, off by default): Codec2 MediaTek decoders also get
     * "vdec-lowlatency", which MediaTek's ACodec only reads for OMX today, and Exynos decoders get
     * the attainable operating rate next to real-time priority, like Tensor. Both leave after the
     * first try so a decoder that rejects them falls back to the default ladder.
     */
    public static boolean setDecoderLowLatencyOptions(MediaFormat videoFormat, MediaCodecInfo decoderInfo, int tryNumber,
                                                      boolean lowLatency, boolean vendorLowLatency, boolean performanceHints,
                                                      boolean disableHevcLowLatency, boolean chipsetTuning) {
        return setDecoderLowLatencyOptions(videoFormat, decoderInfo, tryNumber, lowLatency, vendorLowLatency,
                performanceHints, disableHevcLowLatency ? PreferenceConfiguration.HEVC_LOW_LATENCY_OFF :
                        PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO, chipsetTuning);
    }

    static boolean omitsHevcLowLatencyFlags(String mimeType, String decoderName, int hevcLowLatencyMode) {
        return MediaCodecLowLatencyOptions.omitsHevcLowLatencyFlags(mimeType, decoderName, hevcLowLatencyMode);
    }

    static boolean autoOmitsHevcLowLatencyFlags(String decoderName) {
        return MediaCodecLowLatencyOptions.autoOmitsHevcLowLatencyFlags(decoderName);
    }

    /**
     * Whether this decoder is one of those of erratum 23. Other models return false without looking
     * at the name. Needs no {@link #initialize}: preferences ask before the first stream.
     */
    public static boolean corruptsInLowLatency(String decoderName) {
        return MediaCodecCapabilities.corruptsInLowLatency(decoderName);
    }

    /**
     * Whether a stream of this format runs in that mode: the decoder qualifies and Android's
     * low-latency flag reaches it (the HEVC override can leave the flag out).
     */
    public static boolean runsCorruptingLowLatency(String mimeType, String decoderName, boolean lowLatency, int hevcLowLatencyMode) {
        return MediaCodecCapabilities.runsCorruptingLowLatency(mimeType, decoderName, lowLatency, hevcLowLatencyMode);
    }

    /** Whether this format carries Android's low-latency flag; a retry after a rejected configure may not. */
    public static boolean asksForAndroidLowLatency(MediaFormat format) {
        return MediaCodecCapabilities.asksForAndroidLowLatency(format);
    }

    /** Whether this device has a decoder of {@link #corruptsInLowLatency}. Other models list no codecs. */
    public static boolean hasLowLatencyCorruptingDecoder() {
        return MediaCodecCapabilities.hasLowLatencyCorruptingDecoder();
    }

    public static boolean setDecoderLowLatencyOptions(MediaFormat videoFormat, MediaCodecInfo decoderInfo, int tryNumber,
                                                      boolean lowLatency, boolean vendorLowLatency, boolean performanceHints,
                                                      int hevcLowLatencyMode, boolean chipsetTuning) {
        return setDecoderLowLatencyOptions(videoFormat, decoderInfo, tryNumber, lowLatency, vendorLowLatency,
                performanceHints, hevcLowLatencyMode, chipsetTuning, true);
    }

    /** As above; without logOverrides the Amlogic HEVC warnings stay out, for a format that is only compared. */
    static boolean setDecoderLowLatencyOptions(MediaFormat videoFormat, MediaCodecInfo decoderInfo, int tryNumber,
                                               boolean lowLatency, boolean vendorLowLatency, boolean performanceHints,
                                               int hevcLowLatencyMode, boolean chipsetTuning, boolean logOverrides) {
        return MediaCodecLowLatencyOptions.setDecoderLowLatencyOptions(videoFormat, decoderInfo, tryNumber,
                lowLatency, vendorLowLatency, performanceHints, hevcLowLatencyMode, chipsetTuning, logOverrides);
    }

    static int googleOperatingRate(MediaFormat format, MediaCodecInfo decoderInfo) {
        return MediaCodecLowLatencyOptions.googleOperatingRate(format, decoderInfo);
    }

    static int googleOperatingRate(int fps, Double capsUpper) {
        return MediaCodecLowLatencyOptions.googleOperatingRate(fps, capsUpper);
    }

    static String[] getDecoderLowLatencyOptions(MediaFormat requested, MediaFormat accepted,
                                                List<String> vendorParams, boolean featureLowLatency) {
        return MediaCodecLowLatencyOptions.getDecoderLowLatencyOptions(requested, accepted, vendorParams, featureLowLatency);
    }

    static List<String> requestedTuningKeys(MediaFormat format, List<String> vendorParams) {
        return MediaCodecLowLatencyOptions.requestedTuningKeys(format, vendorParams);
    }

    static List<String> vendorAddedKeys(List<String> configured, MediaFormat keyless, MediaCodecInfo decoderInfo,
                                        int tryNumber, boolean lowLatency, boolean performanceHints,
                                        int hevcLowLatencyMode, boolean chipsetTuning, List<String> vendorParams) {
        return MediaCodecLowLatencyOptions.vendorAddedKeys(configured, keyless, decoderInfo, tryNumber,
                lowLatency, performanceHints, hevcLowLatencyMode, chipsetTuning, vendorParams);
    }

    static List<String> droppedTuningKeys(List<String> firstTry, List<String> configured) {
        return MediaCodecLowLatencyOptions.droppedTuningKeys(firstTry, configured);
    }

    public static boolean decoderSupportsFusedIdrFrame(MediaCodecInfo decoderInfo, String mimeType) {
        return MediaCodecCapabilities.decoderSupportsFusedIdrFrame(decoderInfo, mimeType);
    }

    public static boolean decoderSupportsAdaptivePlayback(MediaCodecInfo decoderInfo, String mimeType) {
        return MediaCodecCapabilities.decoderSupportsAdaptivePlayback(decoderInfo, mimeType);
    }

    static boolean debugSwitch(String name, boolean defaultValue) {
        return MediaCodecCapabilities.debugSwitch(name, defaultValue);
    }

    public static boolean decoderNeedsConstrainedHighProfile(String decoderName) {
        return MediaCodecCapabilities.decoderNeedsConstrainedHighProfile(decoderName);
    }

    public static boolean decoderCanDirectSubmit(String decoderName) {
        return MediaCodecCapabilities.decoderCanDirectSubmit(decoderName);
    }

    public static boolean decoderNeedsSpsBitstreamRestrictions(String decoderName) {
        return MediaCodecCapabilities.decoderNeedsSpsBitstreamRestrictions(decoderName);
    }

    public static boolean decoderNeedsBaselineSpsHack(String decoderName) {
        return MediaCodecCapabilities.decoderNeedsBaselineSpsHack(decoderName);
    }

    public static byte getDecoderOptimalSlicesPerFrame(String decoderName) {
        return MediaCodecCapabilities.getDecoderOptimalSlicesPerFrame(decoderName);
    }

    public static boolean decoderSupportsRefFrameInvalidationAvc(String decoderName, int videoHeight) {
        return MediaCodecCapabilities.decoderSupportsRefFrameInvalidationAvc(decoderName, videoHeight);
    }

    public static boolean decoderSupportsRefFrameInvalidationHevc(MediaCodecInfo decoderInfo,
                                                                 boolean lowLatency, boolean vendorLowLatency,
                                                                 boolean disableHevcLowLatency) {
        return decoderSupportsRefFrameInvalidationHevc(decoderInfo, lowLatency, vendorLowLatency,
                disableHevcLowLatency ? PreferenceConfiguration.HEVC_LOW_LATENCY_OFF :
                        PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO);
    }

    public static boolean decoderSupportsRefFrameInvalidationHevc(MediaCodecInfo decoderInfo,
                                                                 boolean lowLatency, boolean vendorLowLatency,
                                                                 int hevcLowLatencyMode) {
        return MediaCodecCapabilities.decoderSupportsRefFrameInvalidationHevc(decoderInfo,
                lowLatency, vendorLowLatency, hevcLowLatencyMode, refFrameInvalidationHevcPrefixes);
    }

    public static boolean decoderSupportsRefFrameInvalidationAv1(MediaCodecInfo decoderInfo) {
        return MediaCodecCapabilities.decoderSupportsRefFrameInvalidationAv1(decoderInfo);
    }

    public static boolean decoderIsWhitelistedForHevc(MediaCodecInfo decoderInfo) {
        return MediaCodecCapabilities.decoderIsWhitelistedForHevc(decoderInfo, whitelistedHevcDecoders);
    }

    public static boolean isDecoderWhitelistedForAv1(MediaCodecInfo decoderInfo) {
        return MediaCodecCapabilities.isDecoderWhitelistedForAv1(decoderInfo);
    }

    // Tests swap the platform's codec list between cases.
    @VisibleForTesting
    public static void forgetCodecList() {
        MediaCodecDiscovery.forgetCodecList();
    }

    @SuppressWarnings("RedundantThrows")
    public static String dumpDecoders() throws Exception {
        return MediaCodecDiscovery.dumpDecoders();
    }

    public static boolean isSoftwareDecoder(MediaCodecInfo codecInfo) {
        return MediaCodecDiscovery.isSoftwareDecoder(codecInfo);
    }

    public static MediaCodecInfo findFirstDecoder(String mimeType) {
        return MediaCodecDiscovery.findFirstDecoder(mimeType);
    }

    public static MediaCodecInfo findDolbyVisionDecoder(int profile) {
        return MediaCodecDiscovery.findDolbyVisionDecoder(profile);
    }

    public static String dolbyVisionProfileName(int profile) {
        return MediaCodecDiscovery.dolbyVisionProfileName(profile);
    }

    public static MediaCodecInfo findProbableSafeDecoder(String mimeType, int requiredProfile) {
        return MediaCodecDiscovery.findProbableSafeDecoder(mimeType, requiredProfile);
    }

    public static String readCpuinfo() throws Exception {
        return MediaCodecDiscovery.readCpuinfo();
    }

    public static boolean isExynos4Device() {
        return MediaCodecDiscovery.isExynos4Device();
    }
}
