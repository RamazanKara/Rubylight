package com.limelight.binding.video;

import java.util.concurrent.atomic.AtomicBoolean;

import android.app.Activity;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;
import android.view.Display;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.HdrFormatChoice;
import com.limelight.preferences.PreferenceConfiguration;

/**
 * Selects hardware AVC/HEVC/AV1/Dolby Vision decoders, computes negotiated
 * format capabilities and RFI/slice preferences, gates packed 4:4:4 modes, and
 * negotiates HDR formats for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecCodecSelector {
    // ServerCodecModeSupport's PyroWave bits: SDR (4:2:0, 4:4:4), then 10-bit
    private static final int HOST_PYROWAVE_SDR = 0x800000 | 0x1000000;
    private static final int HOST_PYROWAVE_10BIT = 0x2000000 | 0x4000000;

    private final MediaCodecDecoderRenderer r;

    // The stream was asked for as 10-bit SDR: no HDR assumed before the PC reports, SDR colour keys.
    volatile boolean sdr10;
    boolean directSubmit;
    boolean refFrameInvalidationAvc, refFrameInvalidationAv1;
    private byte optimalSlicesPerFrame;
    private byte optimalSlicesPerFrame10;
    // Erratum 23 on the Dolby Vision decoders, and which codec's RFI the launch's capabilities gave up for one:
    // the host was told so, whichever decoder a stream of that codec ends up on.
    private boolean dolbyVisionCorrupts;
    private boolean dolbyVisionAv1Corrupts;
    volatile boolean dolbyVisionTookHevcRfi;
    volatile boolean dolbyVisionTookAv1Rfi;

    volatile String packedHdrDrop;
    // The PC's packed modes (preparePacked444Modes' numbering), -1 when unknown.
    volatile int hostPacked444Modes = -1;
    private int[] packedProbe;
    // False once Sharper colored text started without native 4:4:4: it said why, once per PC, so the
    // stream does not say it again.
    volatile boolean native444Picked = true;

    // Why the last prepareVideoFormats left out PyroWave chosen in the settings, or 0
    volatile int pyroWaveDropReason;
    volatile int hostCodecModeSupport;
    volatile int pyroWaveHdrFailure;
    // What PyroWave leaves out of this screen's HDR, and whether an HLG-only screen gets HDR10 there.
    private volatile HdrFormatPolicy.DrawnLimit pyroWaveHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
    private volatile boolean pyroWaveHlgNeedsHostUpdate;
    private HdrFormatPolicy.HlgWindow displayHlgWindow;
    private volatile HdrFormatPolicy.HlgWindow pyroWaveHlgWindow;
    private volatile boolean pyroWaveHlgRefused;
    volatile boolean pyroWaveHlgFramesStopped;
    private final AtomicBoolean pyroWaveHlgLostSaid = new AtomicBoolean();

    MediaCodecCodecSelector(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    void init(Activity activity, PreferenceConfiguration prefs,
              CrashListener crashListener, int consecutiveCrashCount,
              boolean requestedHdr, String glRenderer,
              PerfOverlayListener perfListener, Display outputDisplay) {
        r.context = activity;
        r.telemetry.thermalMonitor = new ThermalMonitor(activity);
        r.activity = activity;
        r.prefs = prefs;
        r.frameLatencyStats.setDisplayQueueWatch(r.framePresenter.displayQueue);
        r.outputHasArrSupport = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA &&
                outputDisplay.hasArrSupport();
        r.crashListener = crashListener;
        r.consecutiveCrashCount = consecutiveCrashCount;
        r.glRenderer = glRenderer;
        r.perfListener = perfListener;

        r.activeWindowVideoStats = new VideoStats();
        r.telemetry.lastWindowVideoStats = new VideoStats();
        r.globalVideoStats = new VideoStats();

        r.avcDecoder = DecoderSelection.findAvcDecoder(prefs.enableYuv444);
        if (r.avcDecoder != null) {
            LimeLog.info("Selected AVC decoder: " + r.avcDecoder.getName());
        } else {
            LimeLog.warning("No AVC decoder found");
        }

        selectHevcAndAv1Decoders(prefs, requestedHdr, false);
        if (requestedHdr) {
            r.dolbyVisionDecoder = MediaCodecHelper.findDolbyVisionDecoder(
                    MediaCodecInfo.CodecProfileLevel.DolbyVisionProfileDvheSt);
            LimeLog.info(r.dolbyVisionDecoder != null ? "Dolby Vision profile 8 decoder: " + r.dolbyVisionDecoder.getName() :
                    "No Dolby Vision profile 8 decoder");
            r.dolbyVisionAv1Decoder = MediaCodecHelper.findDolbyVisionDecoder(
                    MediaCodecInfo.CodecProfileLevel.DolbyVisionProfileDvav110);
            LimeLog.info(r.dolbyVisionAv1Decoder != null ? "Dolby Vision profile 10 decoder: " + r.dolbyVisionAv1Decoder.getName() :
                    "No Dolby Vision profile 10 decoder");
        }
        if (Sdr10Policy.decoderChoiceMayChange(prefs.enableSdr10Bit, requestedHdr,
                prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE, prefs.experimentalPacked444)) {
            // 10-bit SDR may pick other decoders, but only for its own 10-bit formats and only ones that
            // report Main10. Everything 8-bit keeps exactly the decoders chosen with the setting off.
            MediaCodecInfo baseHevc = r.hevcDecoder;
            MediaCodecInfo baseAv1 = r.av1Decoder;
            selectHevcAndAv1Decoders(prefs, requestedHdr, true);
            r.hevcDecoder10 = DecoderSelection.reportsMain10(r.hevcDecoder, "video/hevc", prefs) ? r.hevcDecoder : null;
            r.av1Decoder10 = DecoderSelection.reportsMain10(r.av1Decoder, "video/av01", prefs) ? r.av1Decoder : null;
            r.hevcDecoder = baseHevc;
            r.av1Decoder = baseAv1;
        }

        // Set attributes that are queried in getCapabilities(). This must be done here
        // because getCapabilities() may be called before setup() in current versions of the common
        // library. The limitation of this is that we don't know whether we're using HEVC or AVC.
        int avcOptimalSlicesPerFrame = 0;
        int hevcOptimalSlicesPerFrame = 0;
        if (r.avcDecoder != null) {
            directSubmit = MediaCodecHelper.decoderCanDirectSubmit(r.avcDecoder.getName());
            refFrameInvalidationAvc = MediaCodecHelper.decoderSupportsRefFrameInvalidationAvc(r.avcDecoder.getName(), prefs.height);
            avcOptimalSlicesPerFrame = MediaCodecHelper.getDecoderOptimalSlicesPerFrame(r.avcDecoder.getName());

            if (directSubmit) {
                LimeLog.info("Decoder " + r.avcDecoder.getName() + " will use direct submit");
            }
            if (refFrameInvalidationAvc) {
                LimeLog.info("Decoder " + r.avcDecoder.getName() + " will use reference frame invalidation for AVC");
            }
            LimeLog.info("Decoder " + r.avcDecoder.getName() + " wants " + avcOptimalSlicesPerFrame + " slices per frame");
        }

        if (r.hevcDecoder != null) {
            r.refFrameInvalidationHevc = MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(r.hevcDecoder,
                    prefs.codecLowLatency, prefs.vendorLowLatency, prefs.disableHevcLowLatency);
            hevcOptimalSlicesPerFrame = MediaCodecHelper.getDecoderOptimalSlicesPerFrame(r.hevcDecoder.getName());

            if (r.refFrameInvalidationHevc) {
                LimeLog.info("Decoder " + r.hevcDecoder.getName() + " will use reference frame invalidation for HEVC");
            }

            LimeLog.info("Decoder " + r.hevcDecoder.getName() + " wants " + hevcOptimalSlicesPerFrame + " slices per frame");
        }

        if (r.av1Decoder != null) {
            refFrameInvalidationAv1 = MediaCodecHelper.decoderSupportsRefFrameInvalidationAv1(r.av1Decoder);

            if (refFrameInvalidationAv1) {
                LimeLog.info("Decoder " + r.av1Decoder.getName() + " will use reference frame invalidation for AV1");
            }
        }

        // Use the larger of the two slices per frame preferences
        optimalSlicesPerFrame = (byte) Math.max(avcOptimalSlicesPerFrame, hevcOptimalSlicesPerFrame);
        LimeLog.info("Requesting " + optimalSlicesPerFrame + " slices per frame");

        // A 10-bit SDR stream may run on other decoders, so the same values are worked out for
        // them and getCapabilities(format) reports those of the decoder that will run the format.
        r.refFrameInvalidationHevc10 = r.refFrameInvalidationHevc;
        optimalSlicesPerFrame10 = optimalSlicesPerFrame;
        if (r.hevcDecoder10 != null && r.hevcDecoder10 != r.hevcDecoder) {
            r.refFrameInvalidationHevc10 = MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(r.hevcDecoder10,
                    prefs.codecLowLatency, prefs.vendorLowLatency, prefs.disableHevcLowLatency);
            optimalSlicesPerFrame10 = (byte) Math.max(avcOptimalSlicesPerFrame,
                    MediaCodecHelper.getDecoderOptimalSlicesPerFrame(r.hevcDecoder10.getName()));
            LimeLog.info("Decoder " + r.hevcDecoder10.getName() + " for 10-bit SDR: RFI " + r.refFrameInvalidationHevc10 +
                    ", " + optimalSlicesPerFrame10 + " slices per frame");
        }
        r.refFrameInvalidationAv110 = r.av1Decoder10 != null && r.av1Decoder10 != r.av1Decoder ?
                MediaCodecHelper.decoderSupportsRefFrameInvalidationAv1(r.av1Decoder10) : refFrameInvalidationAv1;

        if (consecutiveCrashCount % 2 == 1) {
            refFrameInvalidationAvc = r.refFrameInvalidationHevc = refFrameInvalidationAv1 = false;
            r.refFrameInvalidationHevc10 = r.refFrameInvalidationAv110 = false;
            LimeLog.warning("Disabling RFI due to previous crash");
        }

        // Erratum 23 with Android's low-latency mode turned back on: one reference and no long-term
        // ones, so a lost frame recovers with a keyframe. With the mode off these decoders keep RFI.
        if (runsCorruptingLowLatency(r.avcDecoder, "video/avc")) refFrameInvalidationAvc = false;
        if (runsCorruptingLowLatency(r.hevcDecoder, "video/hevc")) r.refFrameInvalidationHevc = false;
        if (runsCorruptingLowLatency(tenBitHevcDecoder(), "video/hevc")) r.refFrameInvalidationHevc10 = false;
        if (runsCorruptingLowLatency(r.av1Decoder, "video/av01")) refFrameInvalidationAv1 = false;
        if (runsCorruptingLowLatency(tenBitAv1Decoder(), "video/av01")) r.refFrameInvalidationAv110 = false;
        // A Dolby Vision stream runs on its own decoder and mime, but declares its base codec's RFI.
        // Only that stream gives it up, in getCapabilities: the base codec's own decoder may have no flag.
        dolbyVisionCorrupts = runsCorruptingLowLatency(r.dolbyVisionDecoder, HdrFormatPolicy.MIME_DOLBY_VISION);
        dolbyVisionAv1Corrupts = runsCorruptingLowLatency(r.dolbyVisionAv1Decoder, HdrFormatPolicy.MIME_DOLBY_VISION);
    }

    private MediaCodecInfo tenBitHevcDecoder() {
        return r.hevcDecoder10 != null ? r.hevcDecoder10 : r.hevcDecoder;
    }

    private MediaCodecInfo tenBitAv1Decoder() {
        return r.av1Decoder10 != null ? r.av1Decoder10 : r.av1Decoder;
    }

    MediaCodecInfo decoderFor(int format) {
        boolean tenBitSdr = tenBitSdr(format);
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            return tenBitSdr ? tenBitHevcDecoder() : r.hevcDecoder;
        }
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            return tenBitSdr ? tenBitAv1Decoder() : r.av1Decoder;
        }
        return r.avcDecoder;
    }

    boolean tenBitSdr(int format) {
        return sdr10 && (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    private void selectHevcAndAv1Decoders(PreferenceConfiguration prefs, boolean requestedHdr, boolean wantsSdr10) {
        r.hevcDecoder = DecoderSelection.findHevcDecoder(prefs, requestedHdr, wantsSdr10, r.avcDecoder);
        if (r.hevcDecoder != null) {
            LimeLog.info("Selected HEVC decoder: " + r.hevcDecoder.getName());
        } else {
            LimeLog.info("No HEVC decoder found");
        }

        r.av1Decoder = DecoderSelection.findAv1Decoder(prefs, requestedHdr, wantsSdr10, r.avcDecoder, r.hevcDecoder);
        if (r.av1Decoder != null) {
            LimeLog.info("Selected AV1 decoder: " + r.av1Decoder.getName());
        } else {
            LimeLog.info("No AV1 decoder found");
        }

        if (prefs.videoFormat == PreferenceConfiguration.FormatOption.AUTO) {
            skipMeasuredSlowerCodecs(prefs, requestedHdr || wantsSdr10);
        }
    }

    // Automatic: leave out codecs this phone measurably decodes slower than another (DecoderBenchmark).
    private void skipMeasuredSlowerCodecs(PreferenceConfiguration prefs, boolean requestedHdr) {
        long[] us = {DecoderBenchmark.result(r.context, r.avcDecoder, "video/avc"),
                DecoderBenchmark.result(r.context, r.hevcDecoder, "video/hevc"),
                DecoderBenchmark.result(r.context, r.av1Decoder, "video/av01")};
        boolean[] skip = DecoderBenchmark.slower(us, requestedHdr);
        // 4:4:4 and over-4K streams need HEVC whatever its speed.
        boolean needsHevc = prefs.enableYuv444 || prefs.width > 4096 || prefs.height > 4096;
        if (skip[1] && r.hevcDecoder != null && !needsHevc) {
            LimeLog.info("Skipping HEVC: measured " + us[1] + " us/frame on this phone");
            r.hevcDecoder = null;
        }
        if (skip[2] && r.av1Decoder != null) {
            LimeLog.info("Skipping AV1: measured " + us[2] + " us/frame on this phone");
            r.av1Decoder = null;
        }
    }

    private boolean runsCorruptingLowLatency(MediaCodecInfo decoder, String mimeType) {
        if (decoder == null || !MediaCodecHelper.runsCorruptingLowLatency(mimeType, decoder.getName(),
                r.prefs.codecLowLatency, r.prefs.hevcLowLatency())) {
            return false;
        }
        LimeLog.warning("Decoder " + decoder.getName() + " in Android low latency: no RFI for " + mimeType);
        return true;
    }

    boolean isHevcMain10Hdr10Supported() {
        return (getDecoderVideoFormats(r.hevcDecoder, "video/hevc", true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    boolean isHevcMain10Supported() {
        return (getDecoderVideoFormats(tenBitHevcDecoder(), "video/hevc", false, true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    boolean isAv1Main10SdrSupported() {
        return (getDecoderVideoFormats(tenBitAv1Decoder(), "video/av01", false, true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    boolean isAv1Main10Supported() {
        return (getDecoderVideoFormats(r.av1Decoder, "video/av01", true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    private int getDecoderVideoFormats(MediaCodecInfo decoder, String mime, boolean hdr, boolean yuv444) {
        return getDecoderVideoFormats(decoder, mime, hdr, false, yuv444);
    }

    private int getDecoderVideoFormats(MediaCodecInfo decoder, String mime, boolean hdr, boolean sdr10, boolean yuv444) {
        return DecoderSelection.decoderVideoFormats(decoder, mime, hdr, sdr10, yuv444, r.prefs);
    }

    boolean isPyroWaveSupported() {
        if (r.prefs.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE || r.prefs.useTextureView) {
            return false;
        }
        int reason = PyroWaveDecoderRenderer.unavailableReason(r.context);
        if (reason != 0 && r.pyroWaveUnavailableReason == 0) {
            LimeLog.warning("PyroWave was chosen, but this device cannot run it: " + r.context.getString(reason));
        }
        r.pyroWaveUnavailableReason = reason;
        return reason == 0;
    }

    static int yuv444FallbackMessage(int format, boolean sdr10) {
        return NightjarPolicyJni.videoYuv444FallbackIsHdr(format, sdr10) ?
                R.string.yuv444_fallback_hdr : R.string.yuv444_fallback;
    }

    static boolean pyroWaveFullChromaDrop(boolean packed444, boolean yuv444) {
        return NightjarPolicyJni.videoPyroWaveFullChromaDrop(packed444, yuv444);
    }

    static int pyroWaveDropReason(boolean chosen, boolean fullChroma, int deviceReason, int hostCodecs,
                                  int formats, int choice, int hdrFailure) {
        int code = NightjarPolicyJni.videoPyroWaveDropReason(
                chosen, fullChroma, deviceReason, hostCodecs, formats, choice, hdrFailure);
        switch (code) {
            case NightjarPolicyJni.PYROWAVE_DROP_DEVICE_REASON: return deviceReason;
            case NightjarPolicyJni.PYROWAVE_DROP_MISSING_SWAPCHAIN: return R.string.pyrowave_missing_swapchain;
            case NightjarPolicyJni.PYROWAVE_DROP_HDR_FAILURE: return hdrFailure;
            default: return code;
        }
    }

    int getSupportedVideoFormats(boolean hdr, boolean sdr10) {
        int tenBit = !sdr10 ? 0 :
                (getDecoderVideoFormats(tenBitHevcDecoder(), "video/hevc", false, true, false) |
                        getDecoderVideoFormats(tenBitAv1Decoder(), "video/av01", false, true, false)) &
                        MoonBridge.VIDEO_FORMAT_MASK_10BIT;
        // Sharper colored text offers native 4:4:4 first (enableYuv444); packed alone, from an older
        // layer, keeps the 4:2:0 HEVC and AV1 formats packing needs.
        if (r.prefs.experimentalPacked444 && !r.prefs.enableYuv444) {
            return getDecoderVideoFormats(r.hevcDecoder, "video/hevc", hdr, false) |
                    getDecoderVideoFormats(r.av1Decoder, "video/av01", hdr, false) | tenBit;
        }
        int pyroWave = 0;
        if (isPyroWaveSupported()) {
            pyroWave = MoonBridge.VIDEO_FORMAT_PYROWAVE;
            if (r.prefs.enableYuv444) pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_444;
            if (hdr) {
                pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10;
                if (r.prefs.enableYuv444) pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10_444;
            }
        }
        return pyroWave | MoonBridge.VIDEO_FORMAT_H264 |
                getDecoderVideoFormats(r.avcDecoder, "video/avc", false, r.prefs.enableYuv444) |
                getDecoderVideoFormats(r.hevcDecoder, "video/hevc", hdr, r.prefs.enableYuv444) |
                getDecoderVideoFormats(r.av1Decoder, "video/av01", hdr, false) | tenBit;
    }

    static int firstPyroWaveFormat(int formats, java.util.function.IntPredicate setup) {
        for (int choice : NightjarPolicyJni.videoPyroWaveCandidateFormats(formats)) {
            if (setup.test(choice)) return choice;
        }
        return 0;
    }

    int finishPrepareVideoFormats(int commonFormats, int formats, int choice) {
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 &&
                (choice & MoonBridge.VIDEO_FORMAT_MASK_10BIT) == 0 && !r.stopping) {
            // The host offers PyroWave HDR and so does this phone, but its output surface failed:
            // the connection falls back to AV1 or HEVC HDR and says why.
            pyroWaveHdrFailure = PyroWaveDecoderRenderer.hdrFailureSummary(r.context);
            LimeLog.warning("PyroWave HDR unavailable on this device: " + r.context.getString(pyroWaveHdrFailure));
        }
        if (!r.stopping) {
            pyroWaveDropReason = pyroWaveDropReason(
                    r.prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE,
                    pyroWaveFullChromaDrop(r.prefs.experimentalPacked444, r.prefs.enableYuv444),
                    r.pyroWaveUnavailableReason, hostCodecModeSupport, commonFormats,
                    choice, pyroWaveHdrFailure);
        }
        if (choice != 0) {
            LimeLog.info("PyroWave surface initialized before negotiation: " + Integer.toHexString(choice));
            return (formats & ~MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) | choice;
        }
        r.pyroWaveRenderer.cleanup();
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            LimeLog.warning("PyroWave initialization failed; retaining MediaCodec fallback");
        }
        return formats & ~MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE;
    }

    int gatePackedHdr(int formats, int width, int height, int fps) {
        packedHdrDrop = null;
        r.packedTenBitOffered = false;
        if (!r.prefs.experimentalPacked444 ||
                (formats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) == 0) return formats;
        int modes = r.preparePacked444Modes(formats, width, height, fps);
        if (modes == 0) return formats;
        if ((modes & MediaCodecDecoderRenderer.PACKED_10BIT_MODES) != 0) {
            // A PC known to have no 10-bit mode this device verified would make the handshake fall
            // back to 8-bit packed after a launch that asked for HDR: ask for SDR right away.
            String host = NightjarPolicyJni.videoHostTenBitIssue(modes, hostPacked444Modes);
            if (host == null) {
                r.packedTenBitOffered = true;
                return formats;
            }
            packedHdrDrop = host;
        } else {
            packedHdrDrop = r.packedTenBitIssue != null ? r.packedTenBitIssue :
                    "no 10-bit full-chroma decoder path could be verified on this device";
        }
        LimeLog.warning("Packed full chroma without HDR: " + packedHdrDrop);
        return formats & ~MoonBridge.VIDEO_FORMAT_MASK_10BIT;
    }

    int preparePacked444Modes(int formats, int width, int height, int fps) {
        // Off for this session (or never on): nothing to verify, and no decode spent on it.
        if (!r.prefs.experimentalPacked444) return 0;
        // The launch asks first (prepareVideoFormats) and the connection again; verifying costs a decode.
        int[] probe = packedProbe;
        if (probe != null && probe[0] == width && probe[1] == height && probe[2] == fps &&
                (formats & ~probe[3]) == 0) return probe[4];
        int modes = r.probePacked444Modes(formats, width, height, fps);
        packedProbe = new int[] {width, height, fps, formats, modes};
        return modes;
    }

    int probePacked444Modes(int formats, int width, int height, int fps) {
        r.packedTenBitIssue = null;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || r.prefs.useTextureView) return 0;
        Packed444 geometry = new Packed444(1, width, height, 0, 0);
        int modes = 0;
        MediaCodecInfo[] decoders = {r.hevcDecoder, r.av1Decoder};
        String[] codecs = {"hevc", "av1"};
        String[] mimes = {"video/hevc", "video/av01"};
        int[] formatBits = {MoonBridge.VIDEO_FORMAT_H265, MoonBridge.VIDEO_FORMAT_H265_MAIN10,
                MoonBridge.VIDEO_FORMAT_AV1_MAIN8, MoonBridge.VIDEO_FORMAT_AV1_MAIN10};
        long budgetDeadlineMs = android.os.SystemClock.uptimeMillis() + PackedFallback.CHECK_TIMEOUT_MS;
        for (int codec = 0; codec < 2; codec++) {
            MediaCodecInfo decoder = decoders[codec];
            if (decoder == null || !decoder.isHardwareAccelerated() || decoder.isSoftwareOnly()) continue;
            try {
                MediaCodecInfo.VideoCapabilities caps = decoder.getCapabilitiesForType(mimes[codec]).getVideoCapabilities();
                if (!caps.areSizeAndRateSupported(geometry.paddedWidth, geometry.paddedHeight * 2, fps)) continue;
                for (int depth = 0; depth < 2; depth++) {
                    int bit = codec * 2 + depth;
                    if ((formats & formatBits[bit]) == 0) continue;
                    if (depth == 1) {
                        String issue = NightjarPolicyJni.videoTenBitOutputIssue(packedHdrAvailable(),
                                GlesUpscaler.tenBitOutputSupported());
                        if (issue != null) {
                            r.packedTenBitIssue = issue;
                            continue;
                        }
                    }
                    // Under a single overall watchdog budget: a decoder or GPU driver that hangs
                    // must not consume another timeout waiting for the other bit depth.
                    final String decoderName = decoder.getName();
                    final String codecName = codecs[codec];
                    final boolean tenBit = depth == 1;
                    long remainingMs = Math.max(0L, budgetDeadlineMs - android.os.SystemClock.uptimeMillis());
                    PackedFallback.Outcome outcome = PackedFallback.withinOutcome(
                            () -> Packed444.verifySampling(r.context, decoderName, codecName, tenBit),
                            remainingMs, decoderName + (tenBit ? " 10-bit" : " 8-bit"));
                    if (outcome == PackedFallback.Outcome.PASS) {
                        modes |= 1 << bit;
                    } else {
                        if (depth == 1) r.packedTenBitIssue = MediaCodecDecoderRenderer.ISSUE_TEN_BIT_SAMPLING;
                        if (outcome == PackedFallback.Outcome.TIMEOUT) {
                            break;
                        }
                    }
                }
            } catch (IllegalArgumentException e) {
                LimeLog.warning("Packed chroma decoder geometry is unsupported: " + e);
            }
        }
        return modes;
    }

    boolean packedHdrAvailable() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false;
        return HdrPolicy.hasHdr10(HdrPolicy.displayHdrTypes(r.outputDisplay));
    }

    /** Whether the PC is taken to send HDR until it reports otherwise: not for 10-bit SDR. */
    boolean assumesHdr() {
        return Sdr10Policy.assumeHdr((r.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, sdr10);
    }

    static boolean vulkanPresents(PreferenceConfiguration prefs, boolean frameWarpListed) {
        return VideoRendererChoice.choose(prefs.vulkanRender, false, Build.VERSION.SDK_INT,
                VulkanRendererBridge::isSupported, false, prefs.upscalingMode,
                NightjarPolicyJni.videoRendererFrameWarpAsked(prefs.frameWarp, frameWarpListed),
                true).renderer == VideoRendererChoice.Renderer.VULKAN;
    }

    static HdrFormatPolicy.DrawnSurface vulkanSurface(PreferenceConfiguration prefs, boolean frameWarpListed) {
        if (!vulkanPresents(prefs, frameWarpListed)) return HdrFormatPolicy.DrawnSurface.NONE;
        return VulkanRendererBridge.drawsHlg() ? HdrFormatPolicy.DrawnSurface.PQ_HLG :
                HdrFormatPolicy.DrawnSurface.PQ_ONLY;
    }

    int pickHdrFormat(int hostHdrFormats, int videoFormats, boolean packed444, boolean frameWarpListed) {
        r.vulkanHelper.vulkanHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
        r.vulkanHelper.vulkanForFrameWarp = false;
        pyroWaveHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
        pyroWaveHlgNeedsHostUpdate = false;
        pyroWaveHlgWindow = null;
        pyroWaveHlgRefused = false;
        pyroWaveHlgFramesStopped = false;
        pyroWaveHlgLostSaid.set(false);
        // "HDR format" narrows what is asked for; the DESCRIBE and ANNOUNCE rules stay the host's.
        hostHdrFormats = HdrFormatPolicy.masked(hostHdrFormats, r.prefs.hdrFormat);
        if (hostHdrFormats == 0) return 0;
        int[] displayTypes = HdrPolicy.displayHdrTypes(r.outputDisplay);
        // Packed full chroma draws a BT.2020/PQ surface, the Vulkan renderer PQ or HLG where this
        // display's window takes HLG, PyroWave PQ or HLG where the host offers HLG on it and the
        // window takes it (HdrFormatPolicy).
        String drawer = null;
        HdrFormatPolicy.DrawnSurface surface = HdrFormatPolicy.DrawnSurface.NONE;
        boolean vulkan = false;
        boolean pyroWave = false;
        boolean vulkanHlgRefused = false;
        if (packed444) {
            drawer = "packed full chroma";
            surface = HdrFormatPolicy.DrawnSurface.PQ_ONLY;
        } else if ((videoFormats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            drawer = "PyroWave";
            surface = HdrFormatPolicy.pyroWaveSurface(hostHdrFormats);
            if (HdrFormatPolicy.pyroWaveNeedsHlgProbe(hostHdrFormats, displayTypes)) {
                pyroWaveHlgWindow = displayHlgWindow();
                surface = HdrFormatPolicy.pyroWaveSurface(hostHdrFormats, pyroWaveHlgWindow);
                pyroWaveHlgRefused = HdrFormatPolicy.pyroWaveHlgRefusedByDisplay(hostHdrFormats, displayTypes,
                        pyroWaveHlgWindow);
            }
            pyroWave = true;
        } else if (vulkanPresents(r.prefs, frameWarpListed)) {
            // Without "Render through Vulkan", only Frame Warp brings the Vulkan renderer.
            r.vulkanHelper.vulkanForFrameWarp = !r.prefs.vulkanRender;
            drawer = r.vulkanHelper.vulkanForFrameWarp ? "the Vulkan renderer for Frame Warp" : "the Vulkan renderer";
            surface = vulkanSurface(r.prefs, frameWarpListed);
            if (HdrFormatPolicy.vulkanNeedsHlgProbe(hostHdrFormats, displayTypes, surface)) {
                HdrFormatPolicy.HlgWindow window = displayHlgWindow();
                surface = HdrFormatPolicy.vulkanSurface(surface, window);
                vulkanHlgRefused = !window.accepted();
            }
            vulkan = true;
        }
        // With "Full color range" on, Dolby Vision 8.1 stays full range from a PC that offers it
        // (hdrFormatRequest), then 8.1 alone where the stream will be HEVC Main10: a forced AV1 still
        // offers HEVC as a fallback, but the PC sends AV1, which cannot carry 8.1. Elsewhere a Dolby
        // Vision stream asks for limited range (sdp, getPreferredColorRange) and the host codes it so
        // with its RPUs (nightjar#193).
        boolean fullRangeDolbyVision = r.prefs.fullRange && r.dolbyVisionDecoder != null &&
                (videoFormats & MoonBridge.VIDEO_FORMAT_H265_MAIN10) != 0 &&
                (r.prefs.videoFormat == PreferenceConfiguration.FormatOption.AUTO ||
                        (videoFormats & MoonBridge.VIDEO_FORMAT_MASK_AV1) == 0) &&
                (hostHdrFormats & HdrFormatPolicy.DV_81) != 0 &&
                (hostHdrFormats & HdrFormatPolicy.DV_81_FULL_RANGE) != 0;
        int dolbyVisionDecoders = fullRangeDolbyVision ? HdrFormatPolicy.DV_81 :
                (r.dolbyVisionDecoder != null ? HdrFormatPolicy.DOLBY_VISION_HEVC : 0) |
                        (r.dolbyVisionAv1Decoder != null ? HdrFormatPolicy.DOLBY_VISION_AV1 : 0);
        int format = HdrFormatPolicy.pick(hostHdrFormats, displayTypes, dolbyVisionDecoders, videoFormats, surface);
        // Game says these once per PC (takeVulkanHdrLimit, takePyroWaveHdrLimit).
        if (vulkan) {
            r.vulkanHelper.vulkanHdrLimit = HdrFormatPolicy.vulkanLimit(hostHdrFormats, displayTypes, dolbyVisionDecoders,
                    videoFormats);
        } else if (pyroWave) {
            pyroWaveHdrLimit = HdrFormatPolicy.pyroWaveLimit(hostHdrFormats, displayTypes, dolbyVisionDecoders,
                    format);
            // "HDR10 only" turned HDR10+ off, so leaving it out is no limit to report.
            if (r.prefs.hdrFormat == HdrFormatChoice.HDR10 && pyroWaveHdrLimit == HdrFormatPolicy.DrawnLimit.HDR10_PLUS) {
                pyroWaveHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
            }
            pyroWaveHlgNeedsHostUpdate = HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(hostHdrFormats, displayTypes);
        }
        HdrFormatPolicy.DrawnLimit limit = vulkan ? r.vulkanHelper.vulkanHdrLimit : pyroWaveHdrLimit;
        LimeLog.info("HDR formats: the PC offers 0x" + Integer.toHexString(hostHdrFormats) + ", asking for " +
                HdrFormatPolicy.name(format) + (drawer != null ? " (" + drawer + " draws " +
                (surface == HdrFormatPolicy.DrawnSurface.PQ_HLG ? "PQ and HLG" : "PQ only") + ")" :
                dolbyVisionDecoders == 0 ? " (no Dolby Vision decoder)" : "") +
                (limit != HdrFormatPolicy.DrawnLimit.NONE ? ", without " + limit : ""));
        if (surface == HdrFormatPolicy.DrawnSurface.PQ_ONLY && !HdrPolicy.hasHdr10(displayTypes) &&
                HdrFormatPolicy.hlgInstead(hostHdrFormats, displayTypes)) {
            // Game names it on the status line (HdrPolicy.Status.PQ_ON_HLG_SCREEN).
            LimeLog.warning("The screen reports HLG and no HDR10, but " + drawer +
                    " draws PQ only" + (pyroWaveHlgNeedsHostUpdate ? " from this PC (HLG on PyroWave needs a host update)" :
                    pyroWaveHlgRefused || vulkanHlgRefused ? " here (this display's window refused HLG)" : "") +
                    ": this stream is HDR10");
        }
        return format;
    }

    // outputDisplay's answer, asked of renderTarget the first time a stream would ask for HLG.
    private synchronized HdrFormatPolicy.HlgWindow displayHlgWindow() {
        if (displayHlgWindow != null) return displayHlgWindow;
        WindowDataspace.Probe probe = WindowDataspace.probeHlg(r.renderTarget);
        displayHlgWindow = HdrFormatPolicy.HlgWindow.of(probe.call);
        String text = "HLG window probe on display " + (r.outputDisplay != null ? r.outputDisplay.getDisplayId() : -1) +
                ": " + probe.describe();
        if (probe.accepted()) LimeLog.info(text);
        else LimeLog.warning(text);
        return displayHlgWindow;
    }

    int videoFormatsForHdrFormat(int hdrFormat, int videoFormats) {
        int formats = HdrFormatPolicy.videoFormats(hdrFormat, videoFormats,
                r.prefs.videoFormat == PreferenceConfiguration.FormatOption.AUTO);
        if (formats != videoFormats) {
            LimeLog.info(HdrFormatPolicy.name(hdrFormat) + " needs " + HdrFormatPolicy.codecNames(HdrFormatPolicy.carriers(hdrFormat)) +
                    ": the automatic codec choice leaves " + HdrFormatPolicy.codecNames(videoFormats & ~formats) + " out");
        }
        return formats;
    }

    void setAnnouncedHdrFormat(int format) {
        r.announcedHdrFormat = format & ~HdrFormatPolicy.DV_81_FULL_RANGE;
        r.dolbyVisionFullRange = (format & HdrFormatPolicy.DV_81_FULL_RANGE) != 0;
        r.dolbyVisionFailed = false;
        if (format != 0) LimeLog.info("HDR format announced: " + HdrFormatPolicy.name(r.announcedHdrFormat) +
                (r.dolbyVisionFullRange ? " full range" : ""));
    }

    MediaCodecInfo dolbyVisionDecoderForStream() {
        // Neither an SDR picture nor a 4:4:4 one carries RPUs.
        if (r.dolbyVisionFailed || r.packing != null ||
                Boolean.FALSE.equals(r.currentHdrMode) ||
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0 ||
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) == 0) {
            return null;
        }
        if ((r.announcedHdrFormat & HdrFormatPolicy.DOLBY_VISION_HEVC) != 0 &&
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            return r.dolbyVisionDecoder;
        }
        if ((r.announcedHdrFormat & HdrFormatPolicy.DOLBY_VISION_AV1) != 0 &&
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            return r.dolbyVisionAv1Decoder;
        }
        return null;
    }

    String dolbyVisionBaseCodec() {
        return (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0 ? "AV1" : "HEVC";
    }

    HdrFormatPolicy.DrawnLimit takePyroWaveHdrLimit() {
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0) return HdrFormatPolicy.DrawnLimit.NONE;
        HdrFormatPolicy.DrawnLimit limit = pyroWaveHdrLimit;
        pyroWaveHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
        return limit;
    }

    boolean isPyroWaveHlgNeedingHostUpdate() {
        return pyroWaveHlgNeedsHostUpdate && (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
    }

    boolean isPyroWaveHlgRefusedByDisplay() {
        return pyroWaveHlgRefused && (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
    }

    boolean isPyroWaveHlgLost() {
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0) return false;
        boolean lost = HdrFormatPolicy.pyroWaveHlgLost(r.announcedHdrFormat, pyroWaveHlgWindow,
                !PyroWaveDecoderRenderer.hlgWindowLost(), pyroWaveHlgFramesStopped);
        if (lost && !pyroWaveHlgLostSaid.getAndSet(true)) {
            LimeLog.severe("PyroWave HLG: this display's window took BT2020_HLG through " + pyroWaveHlgWindow +
                    " before the stream, but the stream is not shown as HLG (" +
                    (pyroWaveHlgFramesStopped ? "the decoder stopped producing frames" :
                            "the window no longer holds it") + ")");
        }
        return lost;
    }

    Integer getDecodedTransfer() {
        MediaFormat format = r.outputFormat;
        if (format == null || r.packing != null || r.upscaler != null || r.dolbyVisionActive ||
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ||
                !format.containsKey(MediaFormat.KEY_COLOR_TRANSFER)) {
            return null;
        }
        return format.getInteger(MediaFormat.KEY_COLOR_TRANSFER);
    }

    int setChromaPacking(int version, int width, int height, int colorSpace, int colorRange) {
        if (version == 0) {
            r.packing = null;
            return 0;
        }
        if ((version != 1 && version != 2) || !r.prefs.experimentalPacked444 ||
                width < 64 || height < 64 || width > 16384 || height > 8192) return -1;
        r.packing = new Packed444(version, width, height, colorSpace, colorRange);
        r.vulkanHelper.packingFailed = false;
        return 0;
    }

    int getPreferredColorSpace() {
        // Default to Rec 709 on Android 8+ or devices with HEVC/AV1 decoders; Rec 601 on older ones.
        return (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O || r.hevcDecoder != null || r.av1Decoder != null) ?
                MoonBridge.COLORSPACE_REC_709 : MoonBridge.COLORSPACE_REC_601;
    }

    int getCapabilities(int negotiatedFormat, int hdrFormat) {
        long packed = NightjarPolicyJni.videoApplyDolbyVisionRfi(
                capabilitiesFor(negotiatedFormat), dolbyVisionCorrupts, dolbyVisionAv1Corrupts, hdrFormat);
        dolbyVisionTookHevcRfi = (packed & (1L << 32)) != 0;
        dolbyVisionTookAv1Rfi = (packed & (1L << 33)) != 0;
        return (int) packed;
    }

    MediaCodecInfo decoderForNegotiated(int formats) {
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            return decoderFor(formats & MoonBridge.VIDEO_FORMAT_MASK_AV1);
        }
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            return decoderFor(formats & MoonBridge.VIDEO_FORMAT_MASK_H265);
        }
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
            return r.avcDecoder;
        }
        return null;
    }

    int capabilitiesFor(int format) {
        // Slices and direct submit follow the decoder that will run the negotiated format: a phone
        // may pair a Codec2 AVC decoder with an OMX HEVC one. Before negotiation the AVC decoder's
        // direct submit and the larger slice count stand in.
        MediaCodecInfo runner = decoderForNegotiated(format);
        boolean hasRunner = runner != null;
        byte runnerSlices = hasRunner ? MediaCodecHelper.getDecoderOptimalSlicesPerFrame(runner.getName()) : 0;
        boolean runnerDirectSubmit = hasRunner && MediaCodecHelper.decoderCanDirectSubmit(runner.getName());
        return NightjarPolicyJni.videoCapabilitiesFor(
                format, sdr10, hasRunner, runnerSlices, runnerDirectSubmit,
                optimalSlicesPerFrame, optimalSlicesPerFrame10, directSubmit,
                refFrameInvalidationAvc, r.refFrameInvalidationHevc, refFrameInvalidationAv1,
                r.refFrameInvalidationHevc10, r.refFrameInvalidationAv110);
    }
}
