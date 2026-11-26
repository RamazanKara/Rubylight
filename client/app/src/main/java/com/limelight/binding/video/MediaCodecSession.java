package com.limelight.binding.video;

import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Bundle;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Display;
import android.view.Surface;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

/**
 * Manages MediaCodec format creation, low-latency configuration and parameter
 * reassertion, decoder startup/teardown lifecycle, and runtime HDR static
 * metadata updates for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecSession {
    private final MediaCodecDecoderRenderer r;

    // Why the last setup() failed, for the stream dialog; null when it did not. The last codec
    // exception stays here too, because configure() is retried and only the final one matters.
    volatile DecoderSetupFailure setupFailure;
    private Throwable lastConfigureError;
    MediaFormat inputFormat;
    // Tuning keys of the configure attempt in progress, and those an earlier attempt asked for that it no longer has.
    volatile List<String> lowLatencyKeysApplied = new ArrayList<>();
    volatile List<String> lowLatencyKeysDropped = new ArrayList<>();
    volatile String lowLatencyKeysDecoder;
    boolean lowLatencyKeysExperimental;
    // Whether the configured decoder is still to be recorded for the next streams once it ran well without vendor keys.
    final AtomicBoolean vendorKeyHangUnrecorded = new AtomicBoolean();
    volatile boolean frameRenderedListenerSet;
    // Follow the content rate for Game's frame rate vote (host frame times, scheduled or not).
    volatile boolean trackContentRate;

    MediaCodecSession(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    static final class CodecTarget {
        String mimeType;
        MediaCodecInfo decoderInfo;
        int error;
    }

    // Asks for a restart with a larger input buffer, the way Moonlight V+ does
    // (qiin2333/moonlight-vplus, GPL-3.0), instead of ending the stream on the first frame that does
    // not fit. False when growing cannot help or the recovery attempts are used up; the caller then
    // fails the stream.
    boolean requestInputBufferGrowth(long requiredBytes, int capacity) {
        int grown = r.recovery.recoveryAttemptsLeft() ?
                NightjarPolicyJni.videoGrownInputSize(requiredBytes, capacity, r.requiredInputBufferSize) : 0;
        if (grown == 0) {
            return false;
        }
        LimeLog.warning("Growing decoder input buffer: " + capacity + " -> " + grown + " bytes (frame needs " +
                requiredBytes + ")");
        r.requiredInputBufferSize = grown;
        requestRestart("input buffer growth to " + grown + " bytes");
        return true;
    }

    // Keeps a grown input buffer through restarts and the configure fallbacks.
    static void applyRequiredInputSize(MediaFormat format, int requiredInputBufferSize) {
        if (requiredInputBufferSize <= 0) {
            return;
        }
        int requested = format.containsKey(MediaFormat.KEY_MAX_INPUT_SIZE) ?
                format.getInteger(MediaFormat.KEY_MAX_INPUT_SIZE) : 0;
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, Math.max(requested, requiredInputBufferSize));
    }

    MediaFormat createBaseMediaFormat(String mimeType) {
        MediaFormat videoFormat = MediaFormat.createVideoFormat(mimeType, r.initialWidth, r.initialHeight);
        if (r.videoFormat == MoonBridge.VIDEO_FORMAT_H264_HIGH8_444) {
            videoFormat.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444);
        } else if (r.videoFormat == MoonBridge.VIDEO_FORMAT_H265_REXT8_444) {
            videoFormat.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444);
        } else if (HdrFormatPolicy.MIME_DOLBY_VISION.equals(mimeType)) {
            videoFormat.setInteger(MediaFormat.KEY_PROFILE, (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0 ?
                    MediaCodecInfo.CodecProfileLevel.DolbyVisionProfileDvav110 :
                    MediaCodecInfo.CodecProfileLevel.DolbyVisionProfileDvheSt);
        }

        // Avoid setting KEY_FRAME_RATE on Lollipop and earlier to reduce compatibility risk
        videoFormat.setInteger(MediaFormat.KEY_FRAME_RATE, r.refreshRate);

        int maxInputSize = NightjarPolicyJni.videoMaxInputSize(r.prefs.bitrate, r.prefs.fps, r.initialWidth, r.initialHeight);
        if (maxInputSize > 0) {
            videoFormat.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxInputSize);
        }

        // Populate keys for adaptive playback. The maximum starts at the stream size, or the size
        // covering a fold or turn (setCoveringSize), and grows when a mid-stream reconfigure asks for more.
        if (r.adaptivePlayback) {
            videoFormat.setInteger(MediaFormat.KEY_MAX_WIDTH, r.maxDecodeWidth);
            videoFormat.setInteger(MediaFormat.KEY_MAX_HEIGHT, r.maxDecodeHeight);
        }

        // Android 7.0 adds color options to the MediaFormat
        FullRangePolicy.applyBaseColor(videoFormat, r.getPreferredColorRange(), r.getPreferredColorSpace(),
                r.codecSelector.assumesHdr());

        return videoFormat;
    }

    boolean configureAndStartDecoder(MediaFormat format) {
        applyRequiredInputSize(format, r.requiredInputBufferSize);
        // A stream handed back from Vulkan draws on the standard renderer from here
        if (!r.vulkanHelper.handOverFromVulkan()) {
            return false;
        }
        // Set HDR metadata if present
        if (r.packing != null) {
            Packed444.sampleDataFormat(format);
            if (r.upscaler == null || r.upscaler.getInputSurface() == null ||
                    r.upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) {
                throw new IllegalStateException("Packed full-chroma reconstruction is unavailable");
            }
        } else {
            boolean hdr = r.currentHdrMode != null ? r.currentHdrMode : r.codecSelector.assumesHdr();
            MediaCodecHdrControl.applyConfigureColor(format, hdr, r.getPreferredColorSpace(), r.currentHdrMetadata, r.getStreamTransfer());
        }

        LimeLog.info("Configuring with format: " + format);

        if (r.upscaler != null && r.packing == null) {
            if (Boolean.TRUE.equals(r.currentHdrMode)) r.upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
            if (r.upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) r.vulkanHelper.closeUpscaler();
        }
        if (r.wantVulkan && r.vulkanRenderer == null) {
            r.vulkanRenderer = r.vulkanHelper.createVulkanRenderer();
            if (r.vulkanRenderer == null) {
                r.wantVulkan = false;
                String detail = VulkanRendererBridge.getLastError();
                if (r.vulkanHelper.frameWarp) r.vulkanHelper.declineFrameWarp(R.string.frame_warp_reason_renderer);
                // Frame Warp chooses Vulkan unasked; only a user who asked for it hears it is unused.
                if (r.prefs.vulkanRender) {
                    r.vulkanHelper.notifyVulkanUnused(r.context.getString(R.string.vulkan_reason_start) +
                            (detail == null || detail.isEmpty() ? "" : " (" + detail + ")"));
                }
                // The stream was switched for full color range: it stays on direct output after all.
                if (r.fullRangeFallbackAttempted) {
                    r.vulkanHelper.notifyFullRange(R.string.full_range_vulkan_unavailable);
                }
            }
        }
        Surface surface = r.vulkanRenderer != null ? r.vulkanRenderer.getDecoderSurface() :
                r.upscaler == null ? r.renderTarget : r.upscaler.getInputSurface();
        try {
            r.videoDecoder.configure(format, surface, null, 0);
        } catch (IllegalArgumentException e) {
            // Recovery reads IllegalArgumentException as an invalid surface and quietly stops the
            // stream. A decoder refusing the grown input buffer must go through reset and
            // recreation instead, and fail loudly if nothing takes it (as in Moonlight V+).
            if (r.requiredInputBufferSize > 0 && !r.stopping && surface != null && surface.isValid()) {
                throw new IllegalStateException("Decoder rejected input buffer size " + r.requiredInputBufferSize, e);
            }
            throw e;
        }
        r.videoDecoder.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT);
        r.videoDecoder.start();

        r.configuredFormat = format;
        r.reconfigure.publishReconfigureLimits();

        // After reconfiguration, we must resubmit CSD buffers
        r.decodeLoop.clearCsdOnConfigure();
        r.compositorWatch.resetWindow();

        setFrameRenderedListener();

        reassertLowLatencyParameters(format);

        // Vendor parameters are applied at start; echoed or declared values confirm acceptance.
        // These only describe the decoder; a failure here must not count as a rejected configure.
        try {
            inputFormat = r.videoDecoder.getInputFormat();
            LimeLog.info("Input format: " + inputFormat);
            r.telemetry.updateDecoderDiagnostics();
        } catch (RuntimeException e) {
            LimeLog.warning("Decoder diagnostics unavailable: " + e);
            r.telemetry.decoderDiagnostics = "";
        }
        return true;
    }

    // Some decoders only honour low-latency settings sent after start, so send them again at runtime.
    private void reassertLowLatencyParameters(MediaFormat format) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return;
        }
        Bundle params = new Bundle();
        if (format.containsKey(MediaFormat.KEY_LOW_LATENCY)) {
            params.putInt(MediaCodec.PARAMETER_KEY_LOW_LATENCY, format.getInteger(MediaFormat.KEY_LOW_LATENCY));
        }
        List<String> vendorParams = MediaCodecHelper.getIntegerVendorParameters(r.videoDecoder.getName());
        if (vendorParams != null) {
            for (String key : vendorParams) {
                if (format.containsKey(key)) {
                    params.putInt(key, format.getInteger(key));
                }
            }
        }
        if (params.isEmpty()) {
            return;
        }
        try {
            r.videoDecoder.setParameters(params);
        } catch (Exception e) {
            LimeLog.warning("Decoder rejected runtime low-latency parameters: " + e);
        }
    }

    private void noteConfigureError(MediaCodecInfo decoder, MediaFormat format, Exception e) {
        e.printStackTrace();
        lastConfigureError = e;
        LimeLog.warning("Decoder " + decoder.getName() + " rejected " + format + ": " + DecoderSetupFailure.describe(e));
    }

    boolean tryConfigureDecoder(MediaCodecInfo selectedDecoderInfo, MediaFormat format, boolean throwOnCodecError) {
        boolean configured = false;
        try {
            r.videoDecoder = MediaCodec.createByCodecName(selectedDecoderInfo.getName());
            if (!configureAndStartDecoder(format)) {
                return false;
            }
            LimeLog.info("Using codec " + selectedDecoderInfo.getName() + " for hardware decoding " + format.getString(MediaFormat.KEY_MIME));
            r.activeDecoderName = selectedDecoderInfo.getName();
            r.debt.watchDisplay(DecoderQueuePolicy.watchesDisplayQueue(r.activeDecoderName));
            startLowLatencyRepairs(format, r.activeDecoderName);
            configured = true;
            // Only now, with this try complete (a later step failing would clear the limits again):
            // a turn or fold the UI could not ask for during a rebuild is asked for.
            if (r.reconfigure.endRebuild()) r.notifyReconfigureLimits();
        } catch (IllegalArgumentException e) {
            noteConfigureError(selectedDecoderInfo, format, e);
            if (throwOnCodecError) {
                throw e;
            }
        } catch (IllegalStateException e) {
            noteConfigureError(selectedDecoderInfo, format, e);
            if (throwOnCodecError) {
                throw e;
            }
        } catch (IOException e) {
            noteConfigureError(selectedDecoderInfo, format, e);
            if (throwOnCodecError) {
                // An IllegalStateException, as codec errors are: codec recovery reports it as a crash.
                throw new IllegalStateException(e);
            }
        } finally {
            if (!configured && r.videoDecoder != null) {
                r.reconfigure.reconfigureLimits = null;
                r.videoDecoder.release();
                r.videoDecoder = null;
            }
        }
        return configured;
    }

    static boolean lowLatencyOptions(MediaFormat format, MediaCodecInfo decoder, int tryNumber,
                                     PreferenceConfiguration prefs, boolean vendorLowLatency) {
        return MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, tryNumber, prefs.codecLowLatency,
                vendorLowLatency, prefs.codecPerformance, prefs.hevcLowLatency(), prefs.experimentalChipsetTuning);
    }

    private List<String> vendorAddedKeys(List<String> triedKeys, String mimeType, MediaCodecInfo decoder, int tryNumber,
                                         List<String> vendorParams) {
        return MediaCodecHelper.vendorAddedKeys(triedKeys, createBaseMediaFormat(mimeType), decoder, tryNumber,
                r.prefs.codecLowLatency, r.prefs.codecPerformance, r.prefs.hevcLowLatency(),
                r.prefs.experimentalChipsetTuning, vendorParams);
    }

    boolean dropsVendorKeysAfterHang(String decoderName, String mimeType) {
        String key = VendorKeyHangs.key(decoderName, mimeType);
        MediaCodecDecoderRenderer.HungKeys hung = r.hungKeys.get(key);
        if (hung == null) {
            List<String> earlier = VendorKeyHangs.recorded(r.context, key);
            if (earlier == null) return false;
            hung = new MediaCodecDecoderRenderer.HungKeys(earlier, true);
            r.hungKeys.put(key, hung);
        }
        LimeLog.warning("Decoder " + decoderName + " gets no vendor low-latency keys: it took no input with them" +
                (hung.earlierStream ? " in an earlier stream" : "") + " (" + MediaCodecTelemetry.describeKeys(hung.keys) + ")" +
                (hung.earlierStream ? "; an app or system update tries them again" : ""));
        return true;
    }

    MediaCodecDecoderRenderer.HungKeys hungKeysOfConfigured() {
        String decoder = lowLatencyKeysDecoder;
        return decoder == null ? null : r.hungKeys.get(decoder);
    }

    // Judged by the flag this decoder got: a retry after a rejected configure has none, whatever the setting.
    private void startLowLatencyRepairs(MediaFormat format, String decoderName) {
        if (!MediaCodecHelper.asksForAndroidLowLatency(format) || !MediaCodecHelper.corruptsInLowLatency(decoderName)) {
            r.keyframeSafetyNet = null;
            r.inputSpacer = null;
            return;
        }
        if (r.repairCounts == null) r.repairCounts = new KeyframeSafetyNet.Counts();
        r.keyframeSafetyNet = new KeyframeSafetyNet(r.repairCounts);
        r.inputSpacer = new InputSpacer();
        LimeLog.info("Decoder " + decoderName + " in Android low latency: keyframes on a decode latency climb," +
                " on a calm picture and at least every " + KeyframeSafetyNet.LIMIT_MS / 1000 + " s, frames spaced");
    }

    CodecTarget resolveCodecTarget() {
        CodecTarget target = new CodecTarget();
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
            target.mimeType = "video/avc";
            target.decoderInfo = r.avcDecoder;
            if (r.avcDecoder == null) {
                LimeLog.severe("No available AVC decoder!");
                setupFailure = new DecoderSetupFailure(DecoderSetupFailure.Kind.NO_DECODER, null, target.mimeType,
                        r.initialWidth, r.initialHeight, r.refreshRate, null);
                target.error = -1;
                return target;
            }
            if (r.initialWidth > 4096 || r.initialHeight > 4096) {
                LimeLog.severe("> 4K streaming only supported on HEVC");
                target.error = -1;
                return target;
            }
            // These fixups only apply to H264 decoders
            r.decodeLoop.needsSpsBitstreamFixup = r.videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsSpsBitstreamRestrictions(target.decoderInfo.getName());
            r.decodeLoop.needsBaselineSpsHack = r.videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsBaselineSpsHack(target.decoderInfo.getName());
            r.decodeLoop.constrainedHighProfile = r.videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsConstrainedHighProfile(target.decoderInfo.getName());
            r.decodeLoop.isExynos4 = MediaCodecHelper.isExynos4Device();
            if (r.decodeLoop.needsSpsBitstreamFixup) {
                LimeLog.info("Decoder " + target.decoderInfo.getName() + " needs SPS bitstream restrictions fixup");
            }
            if (r.decodeLoop.needsBaselineSpsHack) {
                LimeLog.info("Decoder " + target.decoderInfo.getName() + " needs baseline SPS hack");
            }
            if (r.decodeLoop.constrainedHighProfile) {
                LimeLog.info("Decoder " + target.decoderInfo.getName() + " needs constrained high profile");
            }
            if (r.decodeLoop.isExynos4) {
                LimeLog.info("Decoder " + target.decoderInfo.getName() + " is on Exynos 4");
            }
            r.refFrameInvalidationActive = r.codecSelector.refFrameInvalidationAvc;
        } else if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            target.mimeType = "video/hevc";
            target.decoderInfo = r.decoderFor(r.videoFormat);
            MediaCodecInfo dolbyVisionInfo = r.codecSelector.dolbyVisionDecoderForStream();
            if (dolbyVisionInfo != null) {
                // Profile 8 is HEVC Main10 with RPUs: the HEVC decoder takes the same stream.
                target.mimeType = HdrFormatPolicy.MIME_DOLBY_VISION;
                target.decoderInfo = dolbyVisionInfo;
            }
            if (target.decoderInfo == null) {
                LimeLog.severe("No available HEVC decoder!");
                setupFailure = new DecoderSetupFailure(DecoderSetupFailure.Kind.NO_DECODER, null, target.mimeType,
                        r.initialWidth, r.initialHeight, r.refreshRate, null);
                target.error = -2;
                return target;
            }
            r.refFrameInvalidationActive = !r.codecSelector.dolbyVisionTookHevcRfi &&
                    (r.codecSelector.tenBitSdr(r.videoFormat) ? r.refFrameInvalidationHevc10 : r.refFrameInvalidationHevc);
        } else if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            target.mimeType = "video/av01";
            target.decoderInfo = r.decoderFor(r.videoFormat);
            MediaCodecInfo dolbyVisionInfo = r.codecSelector.dolbyVisionDecoderForStream();
            if (dolbyVisionInfo != null) {
                // Profile 10 is AV1 Main10 with RPU metadata OBUs: the AV1 decoder takes the same stream.
                target.mimeType = HdrFormatPolicy.MIME_DOLBY_VISION;
                target.decoderInfo = dolbyVisionInfo;
            }
            if (target.decoderInfo == null) {
                LimeLog.severe("No available AV1 decoder!");
                setupFailure = new DecoderSetupFailure(DecoderSetupFailure.Kind.NO_DECODER, null, target.mimeType,
                        r.initialWidth, r.initialHeight, r.refreshRate, null);
                target.error = -2;
                return target;
            }
            r.refFrameInvalidationActive = !r.codecSelector.dolbyVisionTookAv1Rfi &&
                    (r.codecSelector.tenBitSdr(r.videoFormat) ? r.refFrameInvalidationAv110 : r.codecSelector.refFrameInvalidationAv1);
        } else {
            LimeLog.severe("Unknown format");
            target.error = -3;
            return target;
        }
        r.dolbyVisionActive = false;
        r.decodeLoop.dolbyVisionPaddingLogged = false;
        r.vividSeiDropDecoder = HdrBitstream.dropsVivid(target.decoderInfo.getName(), r.videoFormat) ?
                target.decoderInfo.getName() : null;
        r.decodeLoop.vividSeiDropLogged = false;
        r.adaptivePlayback = MediaCodecHelper.decoderSupportsAdaptivePlayback(target.decoderInfo, target.mimeType);
        r.decodeLoop.fusedIdrFrame = MediaCodecHelper.decoderSupportsFusedIdrFrame(target.decoderInfo, target.mimeType);
        return target;
    }

    boolean applyCoveringMax(MediaCodecInfo selectedDecoderInfo, String mimeType, int[] coveringSize) {
        MediaCodecInfo.VideoCapabilities caps = null;
        try {
            caps = selectedDecoderInfo.getCapabilitiesForType(mimeType).getVideoCapabilities();
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot read decoder sizes for a covering maximum: " + e);
        }
        int[] max = MediaCodecReconfigure.upFrontMax(r.initialWidth, r.initialHeight, coveringSize, caps,
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0);
        if (max[0] > r.maxDecodeWidth || max[1] > r.maxDecodeHeight) {
            r.maxDecodeWidth = Math.max(r.maxDecodeWidth, max[0]);
            r.maxDecodeHeight = Math.max(r.maxDecodeHeight, max[1]);
            LimeLog.info("Decoder configured for sizes up to " + r.maxDecodeWidth + "x" + r.maxDecodeHeight +
                    " for a fold or turn (stream " + r.initialWidth + "x" + r.initialHeight + ")");
            return true;
        }
        return false;
    }

    boolean prepareVendorLowLatency(MediaCodecInfo selectedDecoderInfo, String mimeType) {
        boolean vendorLowLatency = MediaCodecHelper.usesVendorLowLatency(r.prefs.vendorLowLatency,
                selectedDecoderInfo.getName(), r.wantVulkan);
        if (r.prefs.vendorLowLatency && !vendorLowLatency) {
            LimeLog.info("Decoder " + selectedDecoderInfo.getName() +
                    " gets no vendor low-latency keys: the Vulkan renderer draws its frames");
        }
        String decoderKey = VendorKeyHangs.key(selectedDecoderInfo.getName(), mimeType);
        if (!decoderKey.equals(lowLatencyKeysDecoder)) {
            vendorKeyHangUnrecorded.set(false);
            r.framesOutAtHangReset = -1;
        }
        lowLatencyKeysDecoder = decoderKey;
        if (vendorLowLatency && dropsVendorKeysAfterHang(selectedDecoderInfo.getName(), mimeType)) {
            vendorLowLatency = false;
        }
        return vendorLowLatency;
    }

    List<String> recordTryKeys(MediaFormat mediaFormat, MediaCodecInfo selectedDecoderInfo, String mimeType,
                               int tryNumber, boolean vendorLowLatency, List<String> firstTryKeys) {
        List<String> vendorParams = MediaCodecHelper.getIntegerVendorParameters(selectedDecoderInfo.getName());
        List<String> triedKeys = MediaCodecHelper.requestedTuningKeys(mediaFormat, vendorParams);
        if (firstTryKeys == null) {
            firstTryKeys = triedKeys;
        }
        lowLatencyKeysDropped = MediaCodecHelper.droppedTuningKeys(firstTryKeys, triedKeys);
        lowLatencyKeysApplied = triedKeys;
        r.lowLatencyKeysVendor = vendorLowLatency ?
                vendorAddedKeys(triedKeys, mimeType, selectedDecoderInfo, tryNumber, vendorParams) : new ArrayList<>();
        return firstTryKeys;
    }

    void onConfigureTrySucceeded(MediaCodecInfo selectedDecoderInfo, String mimeType, MediaFormat mediaFormat,
                                 int tryNumber, boolean retryWithoutUpscaling, boolean dolbyVision) {
        boolean androidLowLatency = MediaCodecHelper.asksForAndroidLowLatency(mediaFormat);
        r.outputBufferQueueLimit = DecoderQueuePolicy.outputLimit(selectedDecoderInfo.getName(),
                androidLowLatency && MediaCodecHelper.decoderSupportsAndroidRLowLatency(selectedDecoderInfo, mimeType),
                r.prefs.dropLateFrames, tryNumber != 0 || r.codecRecoveryAttempts != 0);
        if (retryWithoutUpscaling) {
            r.upscalingPolicy.fail(UpscalingPolicy.Reason.GPU_ERROR);
            r.vulkanHelper.notifyUpscalingFallback();
        }
        lowLatencyKeysExperimental = MediaCodecHelper.isExperimentalVendorDecoder(selectedDecoderInfo.getName());
        MediaCodecDecoderRenderer.HungKeys hung = hungKeysOfConfigured();
        LimeLog.info("Decoder " + selectedDecoderInfo.getName() +
                (lowLatencyKeysExperimental ? " (vendor keys not run on this hardware)" : "") +
                " tuning keys applied: " +
                MediaCodecTelemetry.describeKeys(lowLatencyKeysApplied) + "; dropped after rejected configures: " +
                MediaCodecTelemetry.describeKeys(lowLatencyKeysDropped) +
                (hung != null ? "; dropped after a hang: " + MediaCodecTelemetry.describeKeys(hung.keys) : ""));
        r.dolbyVisionActive = dolbyVision;
        r.restartRecreates = MediaCodecHdrControl.restartRecreates(selectedDecoderInfo.getName(), lowLatencyKeysApplied);
        if (r.restartRecreates) {
            LimeLog.info("Decoder " + selectedDecoderInfo.getName() + " restarts as a new codec:" +
                    " stop() and configure() are not safe on it as it is set up");
        }
    }

    int recordSetupRejection(MediaCodecInfo selectedDecoderInfo, String mimeType) {
        setupFailure = new DecoderSetupFailure(DecoderSetupFailure.Kind.REJECTED, selectedDecoderInfo.getName(),
                mimeType, r.initialWidth, r.initialHeight, r.refreshRate, lastConfigureError);
        LimeLog.severe("Decoder setup failed: " + setupFailure);
        return -5;
    }

    void setFrameRenderedListener() {
        // MediaCodec's callback on the GPU path refers to the intermediate texture, not the display.
        if (r.upscaler == null && r.vulkanRenderer == null && r.latencyHandler != null) {
            r.videoDecoder.setOnFrameRenderedListener(new MediaCodec.OnFrameRenderedListener() {
                @Override
                public void onFrameRendered(MediaCodec mediaCodec, long presentationTimeUs, long renderTimeNanos) {
                    // Callbacks may be batched; their timestamp, not delivery time, measures rendering.
                    r.frameLatencyStats.onFrameRendered(presentationTimeUs, renderTimeNanos, System.nanoTime());
                    r.inFlight.onRendered(presentationTimeUs);
                }
            }, r.latencyHandler);
            frameRenderedListenerSet = true;
        }
    }

    void beforeSetupLocked(int format, int width, int height, int redrawRate) {
        setupFailure = null;
        lastConfigureError = null;
        r.initialWidth = width;
        r.initialHeight = height;
        r.requiredInputBufferSize = 0;
        r.maxDecodeWidth = r.expectedWidth = r.reconfigure.streamWidth = width;
        r.maxDecodeHeight = r.expectedHeight = r.reconfigure.streamHeight = height;
        r.videoFormat = format;
        r.refreshRate = redrawRate;
        if (r.streamStartMs < 0) {
            r.streamStartMs = System.currentTimeMillis();
            r.streamStartUptimeMs = SystemClock.uptimeMillis();
        }
        r.streamFrameRateMilliHz = FramePacingPolicy.negotiatedFrameRateMilliHz(
                r.streamFrameRateMilliHz, redrawRate);
        String drop = NightjarPolicyJni.videoHostTenBitDrop(r.packedTenBitOffered, r.codecSelector.packedHdrDrop,
                format, r.codecSelector.hostPacked444Modes);
        if (drop != null && !drop.equals(r.codecSelector.packedHdrDrop)) {
            LimeLog.warning("Packed full chroma without HDR: " + drop);
            r.codecSelector.packedHdrDrop = drop;
        }
    }

    void finishSetup(int format, int width, int height, int result) {
        // The host may stream a smaller size than was asked for (4K to a host without 4K is
        // negotiated down to 1080p); scaling and touch follow the size the decoder gets.
        MediaCodecDecoderRenderer.StreamSizeListener listener = r.reconfigure.streamSizeListener;
        if (listener != null) {
            listener.onStreamSizeChanged(r.packing == null ? width : r.packing.width,
                    r.packing == null ? height : r.packing.height);
        }
        if (result == 0) {
            if (r.packing == null && r.prefs.enableYuv444 && r.codecSelector.native444Picked &&
                    (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) == 0) {
                int fallback = MediaCodecCodecSelector.yuv444FallbackMessage(format, r.codecSelector.sdr10);
                r.activity.runOnUiThread(() -> Toast.makeText(r.context, fallback, Toast.LENGTH_LONG).show());
            }
            r.telemetry.startLatencyThread();
            if (r.videoDecoder != null) setFrameRenderedListener();
        }
    }

    void startPyroWave() {
        // Clock sync for the timing CSV only. With Real VRR the PyroWave presenter shows frames
        // at the PC's frame times from a clock of its own (FramePacingPolicy.pyroWaveMode).
        r.hostFrameClock.start(0, false, hostAnnouncesClockSync());
        if (FramePacingPolicy.usesPyroWaveDisplayClock(r.prefs.framePacing, r.prefs.vrr, r.outputVsyncAvailable)) {
            r.vsyncPacer.startChoreographerThread();
        }
    }

    void prepareMediaCodecStart() {
        MediaCodecInfo runner = r.codecSelector.decoderForNegotiated(r.videoFormat);
        if ((r.submitHandoff == null || r.submitHandoff.isClosed()) &&
                (runner != null ? MediaCodecHelper.decoderCanDirectSubmit(runner.getName()) : r.codecSelector.directSubmit)) {
            r.decodeLoop.enableSubmitHandoff();
        }
        r.vsyncPacer.applyOutputPacingFallback();
    }

    static boolean hostAnnouncesClockSync() {
        return MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CLOCK_SYNC);
    }

    float maxDisplayRefreshRate() {
        Display display = r.outputDisplay;
        if (display == null) return 0;
        float highest = display.getRefreshRate();
        for (Display.Mode mode : display.getSupportedModes()) highest = Math.max(highest, mode.getRefreshRate());
        return highest;
    }

    void prepareForStop() {
        // Let the decoding code know to ignore codec exceptions now
        r.stopping = true;
        SubmitHandoff<MediaCodecDecodeLoop.HandedUnit> handoff = r.submitHandoff;
        if (handoff != null) handoff.close(0);
        if (r.vulkanRenderer == null && r.prefs.upscalingMode != UpscalingPolicy.Mode.OFF &&
                Looper.myLooper() == Looper.getMainLooper()) {
            // stop() repeats this on the shutdown worker, away from EGL startup/recovery on the UI.
            return;
        }
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // Vulkan teardown and HDR transitions may wait on the GPU; keep the UI off that lock.
            // The vsync tracker still has to end, or stop() would wait for it forever.
            r.vsyncPacer.quitChoreographerThread();
            return;
        }

        // Halt the rendering thread
        if (r.rendererThread != null) {
            r.rendererThread.interrupt();
        }

        // Stop any active codec recovery operations
        if (r.decoderFailure != null || Looper.myLooper() == Looper.getMainLooper()) {
            // A reset holds the monitor inside MediaCodec: for seconds (the restart an HDR change
            // asks for, a big AV1 decoder's) or for good (one that hung, which the watchdog ends the
            // stream for). The UI must not wait for it (#69). Waiting threads look at the type again
            // within a second, and stop() repeats this with the wake-up on the shutdown worker.
            r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE);
        } else {
            synchronized (r.codecRecoveryMonitor) {
                r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE);
                r.codecRecoveryMonitor.notifyAll();
            }
        }

        // The output surface may be about to go away, so stop presenting to it now. From the UI this
        // waits for the GPU at most 2.5 s; destroy() on the shutdown worker waits for the rest (#73).
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        if (vulkan != null) {
            // The problem report's row is written after the stop, which the renderer no longer answers
            String diagnostics = vulkan.getDiagnosticsText();
            if (diagnostics != null) r.vulkanHelper.vulkanDiagnostics = diagnostics;
            vulkan.stop();
        }

        r.vsyncPacer.quitChoreographerThread();
    }

    void stop() {
        // May be called already, but we'll call it now to be safe
        r.prepareForStop();

        // Wait for the Choreographer looper to shut down (if we have one)
        if (r.vsyncPacer.choreographerHandlerThread != null) {
            try {
                r.vsyncPacer.choreographerHandlerThread.join();
            } catch (InterruptedException e) {
                e.printStackTrace();
                Thread.currentThread().interrupt();
            }
        }

        // Wait for the renderer thread to shut down
        if (r.rendererThread == null) return;
        try {
            r.rendererThread.join();
        } catch (InterruptedException e) {
            e.printStackTrace();
            Thread.currentThread().interrupt();
        }
    }

    void cleanup() {
        r.hostFrameClock.stop();
        String lastStalls = r.telemetry.inputStalls.flush();
        if (lastStalls != null) LimeLog.warning(lastStalls);
        MediaCodecOverlayReport.addRecentStreamRow(r);
        r.pyroWaveRenderer.cleanup(r.frameLatencyStats);
        r.reconfigure.reconfigureLimits = null;
        SubmitHandoff<MediaCodecDecodeLoop.HandedUnit> handoff = r.submitHandoff;
        boolean workerEnded = handoff == null || r.decodeLoop.awaitSubmitWorker(handoff);
        if (r.videoDecoder != null && workerEnded) {
            r.videoDecoder.release();
            r.videoDecoder = null;
        }
        // The stopped codec has invalidated these outputs; never offer them to a later callback.
        synchronized (r.outputBufferQueue) {
            r.outputBufferQueue.clear();
        }
        // Only once the decoder has let go of the renderer's surface
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        if (vulkan != null && workerEnded) {
            r.vulkanRenderer = null;
            vulkan.destroy();
        }
        VulkanRendererBridge handedBack = r.vulkanHelper.handedBackVulkan;
        if (handedBack != null && workerEnded) {
            r.vulkanHelper.handedBackVulkan = null;
            handedBack.destroy();
        }
        r.vulkanHelper.closeUpscaler();
        r.telemetry.stopLatencyThread();
        r.refreshSlots.close(); r.vsyncPacer.holdStamp.close(); r.residency.close(); r.debt.close();
        r.inFlight.close(); r.framePresenter.displayQueue.close(); r.presentScheduler.close(); r.compositorWatch.close();
        if (r.recoveryBackoff != null) r.recoveryBackoff.close();
    }

    // Promotes None/Flush to Restart and leaves Reset alone.
    void requestRestart(String reason) {
        r.restartReason = reason;
        LimeLog.info("Decoder restart requested: " + reason);
        if (!r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE,
                MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART)) {
            r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH,
                    MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART);
        }
    }
}
