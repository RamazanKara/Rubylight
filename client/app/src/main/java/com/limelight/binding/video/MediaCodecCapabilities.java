package com.limelight.binding.video;

import java.util.LinkedList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import android.app.ActivityManager;
import android.content.Context;
import android.content.pm.ConfigurationInfo;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaFormat;
import android.os.Build;

import com.limelight.LimeLog;

/**
 * Decoder prefix tables, runtime device/GPU capability initialization, and codec capability
 * checks supporting {@link MediaCodecHelper}.
 */
final class MediaCodecCapabilities {
    static final boolean SHOULD_BYPASS_SOFTWARE_BLOCK =
            Build.HARDWARE.equals("ranchu") || Build.HARDWARE.equals("cheets") || Build.BRAND.equals("Android-x86");

    static final List<String> preferredDecoders;

    static final List<String> blacklistedDecoderPrefixes;
    static final List<String> spsFixupBitstreamFixupDecoderPrefixes;
    static final List<String> blacklistedAdaptivePlaybackPrefixes;
    static final List<String> baselineProfileHackPrefixes;
    static final List<String> directSubmitPrefixes;
    static final List<String> constrainedHighProfilePrefixes;
    static final List<String> refFrameInvalidationAvcPrefixes;
    static final List<String> useFourSlicesPrefixes;
    static final List<String> qualcommDecoderPrefixes;
    static final List<String> tegraDecoderPrefixes;
    static final List<String> mediaTekDecoderPrefixes;
    static final List<String> kirinDecoderPrefixes;
    static final List<String> exynosDecoderPrefixes;
    static final List<String> amlogicDecoderPrefixes;
    static final List<String> googleDecoderPrefixes;

    static {
        directSubmitPrefixes = new LinkedList<>();

        // These decoders have low enough input buffer latency that they
        // can be directly invoked from the receive thread
        directSubmitPrefixes.add("omx.qcom");
        directSubmitPrefixes.add("omx.sec");
        directSubmitPrefixes.add("omx.exynos");
        directSubmitPrefixes.add("omx.intel");
        directSubmitPrefixes.add("omx.brcm");
        directSubmitPrefixes.add("omx.TI");
        directSubmitPrefixes.add("omx.arc");
        directSubmitPrefixes.add("omx.nvidia");

        // All Codec2 decoders
        directSubmitPrefixes.add("c2.");
    }

    static {
        refFrameInvalidationAvcPrefixes = new LinkedList<>();
        // Qualcomm and NVIDIA may be added at runtime
    }

    static {
        preferredDecoders = new LinkedList<>();
    }

    static {
        blacklistedDecoderPrefixes = new LinkedList<>();

        // Blacklist software decoders that don't support H264 high profile except on systems
        // that are expected to only have software decoders (like emulators).
        if (!SHOULD_BYPASS_SOFTWARE_BLOCK) {
            blacklistedDecoderPrefixes.add("omx.google");
            blacklistedDecoderPrefixes.add("AVCDecoder");

            // We want to avoid ffmpeg decoders since they're usually software decoders,
            // but we'll defer to the Android 10 isSoftwareOnly() API on newer devices
            // to determine if we should use these or not.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                blacklistedDecoderPrefixes.add("OMX.ffmpeg");
            }
        }

        // Force these decoders disabled because:
        // 1) They are software decoders, so the performance is terrible
        // 2) They crash with our HEVC stream anyway (at least prior to CSD batching)
        blacklistedDecoderPrefixes.add("OMX.qcom.video.decoder.hevcswvdec");
        blacklistedDecoderPrefixes.add("OMX.SEC.hevc.sw.dec");
    }

    static {
        // If a decoder qualifies for reference frame invalidation,
        // these entries will be ignored for those decoders.
        spsFixupBitstreamFixupDecoderPrefixes = new LinkedList<>();
        spsFixupBitstreamFixupDecoderPrefixes.add("omx.nvidia");
        spsFixupBitstreamFixupDecoderPrefixes.add("omx.qcom");
        spsFixupBitstreamFixupDecoderPrefixes.add("omx.brcm");

        baselineProfileHackPrefixes = new LinkedList<>();
        baselineProfileHackPrefixes.add("omx.intel");

        blacklistedAdaptivePlaybackPrefixes = new LinkedList<>();
        // The Intel decoder on Lollipop on Nexus Player would increase latency badly
        // if adaptive playback was enabled so let's avoid it to be safe.
        blacklistedAdaptivePlaybackPrefixes.add("omx.intel");
        // The MediaTek decoder crashes at 1080p when adaptive playback is enabled
        // on some Android TV devices with HEVC only.
        blacklistedAdaptivePlaybackPrefixes.add("omx.mtk");

        constrainedHighProfilePrefixes = new LinkedList<>();
        constrainedHighProfilePrefixes.add("omx.intel");
    }

    static {
        useFourSlicesPrefixes = new LinkedList<>();

        // Software decoders will use 4 slices per frame to allow for slice multithreading
        useFourSlicesPrefixes.add("omx.google");
        useFourSlicesPrefixes.add("AVCDecoder");
        useFourSlicesPrefixes.add("omx.ffmpeg");
        useFourSlicesPrefixes.add("c2.android");

        // Old Qualcomm decoders are detected at runtime
    }

    static {
        qualcommDecoderPrefixes = new LinkedList<>();

        qualcommDecoderPrefixes.add("omx.qcom");
        qualcommDecoderPrefixes.add("c2.qti");
        // Artemis also lists c2.qcom, the Codec2 name some older Qualcomm builds use.
        qualcommDecoderPrefixes.add("c2.qcom");
    }

    static {
        tegraDecoderPrefixes = new LinkedList<>();

        tegraDecoderPrefixes.add("omx.nvidia");
        tegraDecoderPrefixes.add("c2.nvidia");
    }

    static {
        mediaTekDecoderPrefixes = new LinkedList<>();

        mediaTekDecoderPrefixes.add("omx.mtk");
        mediaTekDecoderPrefixes.add("c2.mtk");
    }

    static {
        kirinDecoderPrefixes = new LinkedList<>();

        kirinDecoderPrefixes.add("omx.hisi");
        kirinDecoderPrefixes.add("c2.hisi"); // Unconfirmed
    }

    static {
        exynosDecoderPrefixes = new LinkedList<>();

        exynosDecoderPrefixes.add("omx.exynos");
        exynosDecoderPrefixes.add("c2.exynos");
    }

    static {
        amlogicDecoderPrefixes = new LinkedList<>();

        amlogicDecoderPrefixes.add("omx.amlogic");
        amlogicDecoderPrefixes.add("c2.amlogic"); // Unconfirmed
    }

    static {
        googleDecoderPrefixes = new LinkedList<>();

        // Google Codec2 decoders, the hardware decoders of Tensor phones (Moonlight X names them
        // for Pixels too).
        googleDecoderPrefixes.add("c2.google");
    }

    private MediaCodecCapabilities() {}

    static List<String> defaultRefFrameInvalidationHevcPrefixes() {
        List<String> list = new LinkedList<>();
        list.add("omx.exynos");
        list.add("c2.exynos");

        // Qualcomm and NVIDIA may be added at runtime
        return list;
    }

    static List<String> defaultWhitelistedHevcDecoders() {
        List<String> list = new LinkedList<>();

        // Allow software HEVC decoding in the official AOSP emulator
        if (Build.HARDWARE.equals("ranchu")) {
            list.add("omx.google");
        }

        // Exynos seems to be the only HEVC decoder that works reliably
        list.add("omx.exynos");

        // On Darcy (Shield 2017), HEVC runs fine with no fixups required. For some reason,
        // other X1 implementations require bitstream fixups. However, since numReferenceFrames
        // has been supported in GFE since late 2017, we'll go ahead and enable HEVC for all
        // device models.
        //
        // NVIDIA does partial HEVC acceleration on the Shield Tablet. I don't know
        // whether the performance is good enough to use for streaming, but they're
        // using the same omx.nvidia.h265.decode name as the Shield TV which has a
        // fully accelerated HEVC pipeline. AFAIK, the only K1 devices with this
        // partially accelerated HEVC decoder are the Shield Tablet and Xiaomi MiPad,
        // so I'll check for those here.
        //
        // In case there are some that I missed, I will also exclude pre-Oreo OSes since
        // only Shield ATV got an Oreo update and any newer Tegra devices will not ship
        // with an old OS like Nougat.
        if (!Build.DEVICE.equalsIgnoreCase("shieldtablet") &&
                !Build.DEVICE.equalsIgnoreCase("mocha") &&
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            list.add("omx.nvidia");
        }

        // Plot twist: On newer Sony devices (BRAVIA_ATV2, BRAVIA_ATV3_4K, BRAVIA_UR1_4K) the H.264 decoder crashes
        // on several configurations (> 60 FPS and 1440p) that work with HEVC, so we'll whitelist those devices for HEVC.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && Build.DEVICE.startsWith("BRAVIA_")) {
            list.add("omx.mtk");
        }

        // Amlogic requires 1 reference frame for HEVC to avoid hanging. Since it's been years
        // since GFE added support for maxNumReferenceFrames, we'll just enable all Amlogic SoCs
        // running Android 9 or later.
        //
        // NB: We don't do this on Sabrina (GCWGTV) because H.264 is lower latency when we use
        // vendor.low-latency.enable. We will still use HEVC if decoderCanMeetPerformancePointWithHevcAndNotAvc()
        // determines it's the only way to meet the performance requirements.
        //
        // With the Android 12 update, Sabrina now uses HEVC (with RFI) based upon FEATURE_LowLatency
        // support, which provides equivalent latency to H.264 now.
        //
        // FIXME: Should we do this for all Amlogic S905X SoCs?
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && !Build.DEVICE.equalsIgnoreCase("sabrina")) {
            list.add("omx.amlogic");
        }

        // Realtek SoCs are used inside many Android TV devices and can only do 4K60 with HEVC.
        // We'll enable those HEVC decoders by default and see if anything breaks.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            list.add("omx.realtek");
        }

        // These theoretically have good HEVC decoding capabilities (potentially better than
        // their AVC decoders), but haven't been tested enough
        //list.add("omx.rk");

        // Let's see if HEVC decoders are finally stable with C2
        list.add("c2.");

        // Based on GPU attributes queried at runtime, the omx.qcom/c2.qti prefix will be added
        // during initialization to avoid SoCs with broken HEVC decoders.
        return list;
    }

    private static boolean isPowerVR(String glRenderer) {
        return glRenderer != null && glRenderer.toLowerCase(java.util.Locale.ROOT).contains("powervr");
    }

    private static String getAdrenoVersionString(String glRenderer) {
        glRenderer = glRenderer.toLowerCase().trim();

        if (!glRenderer.contains("adreno")) {
            return null;
        }

        Pattern modelNumberPattern = Pattern.compile("(.*)([0-9]{3})(.*)");

        Matcher matcher = modelNumberPattern.matcher(glRenderer);
        if (!matcher.matches()) {
            return null;
        }

        String modelNumber = matcher.group(2);
        LimeLog.info("Found Adreno GPU: " + modelNumber);
        return modelNumber;
    }

    private static boolean isLowEndSnapdragonRenderer(String glRenderer) {
        String modelNumber = getAdrenoVersionString(glRenderer);
        if (modelNumber == null) {
            // Not an Adreno GPU
            return false;
        }

        // The current logic is to identify low-end SoCs based on a zero in the x0x place.
        return modelNumber.charAt(1) == '0';
    }

    private static int getAdrenoRendererModelNumber(String glRenderer) {
        String modelNumber = getAdrenoVersionString(glRenderer);
        if (modelNumber == null) {
            // Not an Adreno GPU
            return -1;
        }

        return Integer.parseInt(modelNumber);
    }

    // This is a workaround for some broken devices that report
    // only GLES 3.0 even though the GPU is an Adreno 4xx series part.
    // An example of such a device is the Huawei Honor 5x with the
    // Snapdragon 616 SoC (Adreno 405).
    private static boolean isGLES31SnapdragonRenderer(String glRenderer) {
        // Snapdragon 4xx and higher support GLES 3.1
        return getAdrenoRendererModelNumber(glRenderer) >= 400;
    }

    /**
     * Reference frame invalidation for MediaTek (AVC and HEVC) and for HEVC on Snapdragon decoders
     * named c2.qcom, as in the derflacco fork (moonlight-noir, GPL-3.0). Experimental: we have no
     * MediaTek or c2.qcom device to run it on. Gated like the other
     * Codec2-era entries, by the GLES 3 check in initialize().
     */
    static void addMediaTekAndQcomRefFrameInvalidation(List<String> refFrameInvalidationHevcPrefixes) {
        addMediaTekAndQcomRefFrameInvalidation(refFrameInvalidationHevcPrefixes, null);
    }

    static void addMediaTekAndQcomRefFrameInvalidation(List<String> refFrameInvalidationHevcPrefixes, String glRenderer) {
        // PowerVR SoCs (e.g. MT8176 / GE8100 / GE8300 / GE8320) handle MediaTek RFI below
        // so GE8xxx hangs on HEVC and MT8176 avoids high-latency AVC maxNumReferenceFrames.
        if (!isPowerVR(glRenderer)) {
            LimeLog.info("Added omx.mtk/c2.mtk and c2.qcom to reference frame invalidation support list");
            for (String prefix : new String[] {"omx.mtk", "c2.mtk"}) {
                refFrameInvalidationAvcPrefixes.add(prefix);
                refFrameInvalidationHevcPrefixes.add(prefix);
            }
        } else {
            LimeLog.info("Added c2.qcom to reference frame invalidation support list (PowerVR MediaTek RFI handled separately)");
        }
        refFrameInvalidationHevcPrefixes.add("c2.qcom");
    }

    static void initializeDeviceRules(Context context, String glRenderer,
                                      List<String> whitelistedHevcDecoders,
                                      List<String> refFrameInvalidationHevcPrefixes) {
        // Older Sony ATVs (SVP-DTV15) have broken MediaTek codecs (decoder hangs after rendering the first frame).
        // I know the Fire TV 2 and 3 works, so I'll whitelist Amazon devices which seem to actually be tested.
        // We still have to check Build.MANUFACTURER to catch Amazon Fire tablets.
        boolean fireTv = context.getPackageManager().hasSystemFeature("amazon.hardware.fire_tv");
        MediaCodecHelper.setFireTv(fireTv);
        if (fireTv ||
                Build.MANUFACTURER.equalsIgnoreCase("Amazon")) {
            // HEVC and RFI have been confirmed working on Fire TV 2, Fire TV Stick 2, Fire TV 4K Max,
            // Fire HD 8 2020, and Fire HD 8 2022 models.
            //
            // This is probably a good enough sample to conclude that all MediaTek Fire OS devices
            // are likely to be okay.
            whitelistedHevcDecoders.add("omx.mtk");
            refFrameInvalidationHevcPrefixes.add("omx.mtk");
            refFrameInvalidationHevcPrefixes.add("c2.mtk");

            // This requires setting vdec-lowlatency on the Fire TV 3, otherwise the decoder
            // never produces any output frames. See comment above for details on why we only
            // do this for Fire TV devices.
            whitelistedHevcDecoders.add("omx.amlogic");

            // HEVC stays enabled because that's the only way these devices can hit 4K. RFI hangs the
            // Amlogic decoders of Fire TV devices, which decoderSupportsRefFrameInvalidationHevc()
            // rules out first, so this list only reaches Amazon devices that aren't Fire TVs.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                refFrameInvalidationHevcPrefixes.add("omx.amlogic");
                refFrameInvalidationHevcPrefixes.add("c2.amlogic");
            }
        }

        ActivityManager activityManager =
                (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        ConfigurationInfo configInfo = activityManager.getDeviceConfigurationInfo();
        if (configInfo.reqGlEsVersion != ConfigurationInfo.GL_ES_VERSION_UNDEFINED) {
            LimeLog.info("OpenGL ES version: " + configInfo.reqGlEsVersion);

            MediaCodecHelper.setLowEndSnapdragon(isLowEndSnapdragonRenderer(glRenderer));
            MediaCodecHelper.setAdreno620(getAdrenoRendererModelNumber(glRenderer) == 620);

            // Tegra K1 and later can do reference frame invalidation properly
            if (configInfo.reqGlEsVersion >= 0x30000) {
                LimeLog.info("Added omx.nvidia/c2.nvidia to reference frame invalidation support list");
                refFrameInvalidationAvcPrefixes.add("omx.nvidia");

                // Exclude HEVC RFI on Pixel C and Tegra devices prior to Android 11. Misbehaving RFI
                // on these devices can cause hundreds of milliseconds of latency, so it's not worth
                // using it unless we're absolutely sure that it will not cause increased latency.
                if (!Build.DEVICE.equalsIgnoreCase("dragon") && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    refFrameInvalidationHevcPrefixes.add("omx.nvidia");
                }

                refFrameInvalidationAvcPrefixes.add("c2.nvidia"); // Unconfirmed
                refFrameInvalidationHevcPrefixes.add("c2.nvidia"); // Unconfirmed

                LimeLog.info("Added omx.qcom/c2.qti to reference frame invalidation support list");
                refFrameInvalidationAvcPrefixes.add("omx.qcom");
                refFrameInvalidationHevcPrefixes.add("omx.qcom");
                refFrameInvalidationAvcPrefixes.add("c2.qti");
                refFrameInvalidationHevcPrefixes.add("c2.qti");
                addMediaTekAndQcomRefFrameInvalidation(refFrameInvalidationHevcPrefixes, glRenderer);
            }

            // Qualcomm's early HEVC decoders break hard on our HEVC stream. The best check to
            // tell the good from the bad decoders are the generation of Adreno GPU included:
            // 3xx - bad
            // 4xx - good
            //
            // The "good" GPUs support GLES 3.1, but we can't just check that directly
            // (see comment on isGLES31SnapdragonRenderer).
            //
            if (isGLES31SnapdragonRenderer(glRenderer)) {
                LimeLog.info("Added omx.qcom/c2.qti to HEVC decoders based on GLES 3.1+ support");
                whitelistedHevcDecoders.add("omx.qcom");
                whitelistedHevcDecoders.add("c2.qti");
            }
            else {
                blacklistedDecoderPrefixes.add("OMX.qcom.video.decoder.hevc");

                // These older decoders need 4 slices per frame for best performance
                useFourSlicesPrefixes.add("omx.qcom");
            }

            // Older MediaTek SoCs have issues with HEVC rendering but the newer chips with
            // PowerVR GPUs have good HEVC support.
            if (isPowerVR(glRenderer)) {
                LimeLog.info("Added omx.mtk to HEVC decoders based on PowerVR GPU");
                whitelistedHevcDecoders.add("omx.mtk");

                // This SoC (MT8176 in GPD XD+) supports AVC RFI too, but the maxNumReferenceFrames setting
                // required to make it work adds a huge amount of latency. However, RFI on HEVC causes
                // decoder hangs on the newer GE8100, GE8300, and GE8320 GPUs, so we limit it to the
                // Series6XT GPUs where we know it works.
                if (glRenderer.contains("GX6")) {
                    LimeLog.info("Added omx.mtk/c2.mtk to RFI list for HEVC");
                    refFrameInvalidationHevcPrefixes.add("omx.mtk");
                    refFrameInvalidationHevcPrefixes.add("c2.mtk");
                }
            }
        }
    }

    static boolean decoderSupportsAndroidRLowLatency(MediaCodecInfo decoderInfo, String mimeType) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            try {
                if (decoderInfo.getCapabilitiesForType(mimeType).isFeatureSupported(CodecCapabilities.FEATURE_LowLatency)) {
                    LimeLog.info("Low latency decoding mode supported (FEATURE_LowLatency)");
                    return true;
                }
            } catch (Exception e) {
                // Tolerate buggy codecs
                e.printStackTrace();
            }
        }

        return false;
    }

    /**
     * Whether this decoder is one of those of erratum 23. Other models return false without looking
     * at the name. Needs no {@link MediaCodecHelper#initialize}: preferences ask before the first stream.
     */
    static boolean corruptsInLowLatency(String decoderName) {
        if (!MediaCodecHelper.LOW_LATENCY_CORRUPTION_MODEL.equals(Build.MODEL)) return false;
        for (String prefix : mediaTekDecoderPrefixes) {
            if (decoderName.regionMatches(true, 0, prefix, 0, prefix.length())) return true;
        }
        return false;
    }

    /**
     * Whether a stream of this format runs in that mode: the decoder qualifies and Android's
     * low-latency flag reaches it (the HEVC override can leave the flag out).
     */
    static boolean runsCorruptingLowLatency(String mimeType, String decoderName, boolean lowLatency, int hevcLowLatencyMode) {
        return lowLatency && corruptsInLowLatency(decoderName) &&
                !MediaCodecLowLatencyOptions.omitsHevcLowLatencyFlags(mimeType, decoderName, hevcLowLatencyMode);
    }

    /** Whether this format carries Android's low-latency flag; a retry after a rejected configure may not. */
    static boolean asksForAndroidLowLatency(MediaFormat format) {
        return format.containsKey(MediaFormat.KEY_LOW_LATENCY) && format.getInteger(MediaFormat.KEY_LOW_LATENCY) != 0;
    }

    /** Whether this device has a decoder of {@link #corruptsInLowLatency}. Other models list no codecs. */
    static boolean hasLowLatencyCorruptingDecoder() {
        if (!MediaCodecHelper.LOW_LATENCY_CORRUPTION_MODEL.equals(Build.MODEL)) return false;
        for (MediaCodecInfo codec : MediaCodecDiscovery.getMediaCodecList()) {
            if (!codec.isEncoder() && corruptsInLowLatency(codec.getName())) return true;
        }
        return false;
    }

    static boolean decoderSupportsFusedIdrFrame(MediaCodecInfo decoderInfo, String mimeType) {
        // If adaptive playback is supported, we can submit new CSD together with a keyframe
        try {
            if (decoderInfo.getCapabilitiesForType(mimeType).
                    isFeatureSupported(CodecCapabilities.FEATURE_AdaptivePlayback)) {
                LimeLog.info("Decoder supports fused IDR frames (FEATURE_AdaptivePlayback)");
                return true;
            }
        } catch (Exception e) {
            // Tolerate buggy codecs
            e.printStackTrace();
        }

        return false;
    }

    static boolean decoderSupportsAdaptivePlayback(MediaCodecInfo decoderInfo, String mimeType) {
        if (MediaCodecHelper.isDecoderInList(blacklistedAdaptivePlaybackPrefixes, decoderInfo.getName())) {
            LimeLog.info("Decoder blacklisted for adaptive playback");
            return false;
        }

        try {
            if (decoderInfo.getCapabilitiesForType(mimeType).
                    isFeatureSupported(CodecCapabilities.FEATURE_AdaptivePlayback))
            {
                // This will make getCapabilities() return that adaptive playback is supported
                LimeLog.info("Adaptive playback supported (FEATURE_AdaptivePlayback)");
                return true;
            }
        } catch (Exception e) {
            // Tolerate buggy codecs
            e.printStackTrace();
        }

        return false;
    }

    /**
     * A decoder switch for A/B runs: adb shell setprop debug.nightjar.&lt;name&gt; 1 (or 0) before a
     * stream, read once when the stream starts. Unset, or anything but 0 and 1, is
     * {@code defaultValue}: the switch's Debug setting where it has one, so a set property wins over it.
     */
    static boolean debugSwitch(String name, boolean defaultValue) {
        String property = "debug.nightjar." + name;
        String value;
        try {
            // SystemProperties is hidden but readable; the native renderer reads its switches the same way.
            value = (String) Class.forName("android.os.SystemProperties")
                    .getMethod("get", String.class, String.class).invoke(null, property, "");
        } catch (ReflectiveOperationException | RuntimeException e) {
            LimeLog.warning("Cannot read " + property + ", using " + (defaultValue ? 1 : 0) + ": " + e);
            return defaultValue;
        }
        boolean on = "1".equals(value) || (!"0".equals(value) && defaultValue);
        if (on != defaultValue) {
            LimeLog.info(property + " is " + (on ? 1 : 0) + " (default " + (defaultValue ? 1 : 0) + ")");
        }
        return on;
    }

    static boolean decoderNeedsConstrainedHighProfile(String decoderName) {
        return MediaCodecHelper.isDecoderInList(constrainedHighProfilePrefixes, decoderName);
    }

    static boolean decoderCanDirectSubmit(String decoderName) {
        return MediaCodecHelper.isDecoderInList(directSubmitPrefixes, decoderName) && !MediaCodecDiscovery.isExynos4Device();
    }

    static boolean decoderNeedsSpsBitstreamRestrictions(String decoderName) {
        return MediaCodecHelper.isDecoderInList(spsFixupBitstreamFixupDecoderPrefixes, decoderName);
    }

    static boolean decoderNeedsBaselineSpsHack(String decoderName) {
        return MediaCodecHelper.isDecoderInList(baselineProfileHackPrefixes, decoderName);
    }

    static byte getDecoderOptimalSlicesPerFrame(String decoderName) {
        if (MediaCodecHelper.isDecoderInList(useFourSlicesPrefixes, decoderName)) {
            // 4 slices per frame reduces decoding latency on older Qualcomm devices
            return 4;
        }
        else {
            // 1 slice per frame produces the optimal encoding efficiency
            return 1;
        }
    }

    static boolean decoderSupportsRefFrameInvalidationAvc(String decoderName, int videoHeight) {
        // Reference frame invalidation is broken on low-end Snapdragon SoCs at 1080p.
        if (videoHeight > 720 && MediaCodecHelper.isLowEndSnapdragon()) {
            return false;
        }

        // This device seems to crash constantly at 720p, so try disabling
        // RFI to see if we can get that under control.
        if (Build.DEVICE.equals("b3") || Build.DEVICE.equals("b5")) {
            return false;
        }

        return MediaCodecHelper.isDecoderInList(refFrameInvalidationAvcPrefixes, decoderName);
    }

    static boolean decoderSupportsRefFrameInvalidationHevc(MediaCodecInfo decoderInfo,
                                                           boolean lowLatency, boolean vendorLowLatency,
                                                           int hevcLowLatencyMode,
                                                           List<String> refFrameInvalidationHevcPrefixes) {
        if (decoderInfo == null) return false;
        // Omitting HEVC low-latency flags (explicit Off, or Auto on decoders like c2.amlogic.*
        // outside Fire TV, erratum 22) must also disable HEVC RFI so decoders fall back to 1
        // reference frame instead of stalling at ~1 FPS.
        if (MediaCodecLowLatencyOptions.omitsHevcLowLatencyFlags("video/hevc", decoderInfo.getName(), hevcLowLatencyMode)) {
            return false;
        }
        // Amlogic HEVC decoders on Fire TV hang on RFI. On Fire OS 6 (Fire TV 3, Fire TV Cube 1) it
        // turned artifacts after packet loss into full decoder output hangs. On Fire OS 7 the Fire
        // TV Cube 3 (OMX.amlogic.hevc) stops rendering seconds into a stream from a host that marks
        // long-term references, which RFI's maxNumReferenceFrames of 0 lets it do, while the same
        // stream with 0 long-term references plays (Rubylight issue 73). Without RFI the stream
        // asks for 1 reference frame and recovers with keyframes. This comes before the low
        // latency check, which would otherwise enable RFI on these decoders from Android 11.
        // Amlogic on other devices has shown no such hang and keeps the rule below.
        if (MediaCodecHelper.isFireTv() && MediaCodecHelper.isDecoderInList(amlogicDecoderPrefixes, decoderInfo.getName())) return false;
        // HEVC decoders seem to universally support RFI, but it can have huge latency penalties
        // for some decoders due to the number of references frames being > 1. Old Amlogic
        // decoders are known to have this problem.
        //
        // If the decoder supports FEATURE_LowLatency or any vendor low latency option,
        // we will use that as an indication that it can handle HEVC RFI without excessively
        // buffering frames.
        if ((lowLatency && decoderSupportsAndroidRLowLatency(decoderInfo, "video/hevc")) ||
                (vendorLowLatency && (MediaCodecLowLatencyOptions.decoderSupportsKnownVendorLowLatencyOption(decoderInfo.getName()) ||
                        MediaCodecLowLatencyOptions.decoderSupportsMediaTekRfiSwitch(decoderInfo.getName())))) {
            LimeLog.info("Enabling HEVC RFI based on low latency option support");
            return true;
        }

        return MediaCodecHelper.isDecoderInList(refFrameInvalidationHevcPrefixes, decoderInfo.getName());
    }

    static boolean decoderSupportsRefFrameInvalidationAv1(MediaCodecInfo decoderInfo) {
        // AV1 names its references explicitly (eight slots, no POC arithmetic and no
        // reordering), so a frame predicted from an older slot is ordinary AV1 for every
        // conforming decoder. The host answers with a 15-25 KB frame instead of a
        // 100+ KB keyframe. Unlike H.264, there is no SPS patching to give up.
        LimeLog.info("Enabling AV1 RFI for " + decoderInfo.getName());
        return true;
    }

    static boolean decoderIsWhitelistedForHevc(MediaCodecInfo decoderInfo, List<String> whitelistedHevcDecoders) {
        //
        // Software decoders are terrible and we never want to use them.
        // We want to catch decoders like:
        // OMX.qcom.video.decoder.hevcswvdec
        // OMX.SEC.hevc.sw.dec
        //
        if (decoderInfo.getName().contains("sw")) {
            LimeLog.info("Disallowing HEVC on software decoder: " + decoderInfo.getName());
            return false;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && (!decoderInfo.isHardwareAccelerated() || decoderInfo.isSoftwareOnly())) {
            LimeLog.info("Disallowing HEVC on software decoder: " + decoderInfo.getName());
            return false;
        }

        // If this device is media performance class 12 or higher, we will assume any hardware
        // HEVC decoder present is fast and modern enough for streaming.
        //
        // [5.3/H-1-1] MUST NOT drop more than 2 frames in 10 seconds (i.e less than 0.333 percent frame drop) for a 1080p 60 fps video session under load.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            LimeLog.info("Media performance class: " + Build.VERSION.MEDIA_PERFORMANCE_CLASS);
            if (Build.VERSION.MEDIA_PERFORMANCE_CLASS >= Build.VERSION_CODES.S) {
                LimeLog.info("Allowing HEVC based on media performance class");
                return true;
            }
        }

        // If the decoder supports FEATURE_LowLatency, we will assume it is fast and modern enough
        // to be preferable for streaming over H.264 decoders.
        if (decoderSupportsAndroidRLowLatency(decoderInfo, "video/hevc")) {
            LimeLog.info("Allowing HEVC based on FEATURE_LowLatency support");
            return true;
        }

        // Otherwise, we use our list of known working HEVC decoders
        return MediaCodecHelper.isDecoderInList(whitelistedHevcDecoders, decoderInfo.getName());
    }

    static boolean isDecoderWhitelistedForAv1(MediaCodecInfo decoderInfo) {
        // Google didn't have official support for AV1 (or more importantly, a CTS test) until
        // Android 10, so don't use any decoder before then.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return false;
        }

        //
        // Software decoders are terrible and we never want to use them.
        // We want to catch decoders like:
        // OMX.qcom.video.decoder.hevcswvdec
        // OMX.SEC.hevc.sw.dec
        //
        if (decoderInfo.getName().contains("sw")) {
            LimeLog.info("Disallowing AV1 on software decoder: " + decoderInfo.getName());
            return false;
        }
        else if (!decoderInfo.isHardwareAccelerated() || decoderInfo.isSoftwareOnly()) {
            LimeLog.info("Disallowing AV1 on software decoder: " + decoderInfo.getName());
            return false;
        }

        return decoderSupportsAndroidRLowLatency(decoderInfo, "video/av01") ||
                MediaCodecLowLatencyOptions.decoderSupportsKnownVendorLowLatencyOption(decoderInfo.getName());
    }
}
