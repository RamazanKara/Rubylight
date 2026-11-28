package com.limelight.binding.video;

import java.nio.ByteBuffer;
import java.util.Arrays;

import android.media.MediaFormat;
import android.os.Build;
import android.os.SystemClock;
import android.view.Display;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Manages the native Vulkan renderer, GLES upscaler/packed-chroma presenter,
 * Frame Warp negotiation, and Vulkan HDR/presentation diagnostics for
 * {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecVulkanHelper {
    // The renderer's Lowest latency jitter buffer, which only matters to its host-timed pacing
    private static final int VULKAN_JITTER_BUFFER = 3;

    private final MediaCodecDecoderRenderer r;

    // What the Vulkan renderer leaves out of this screen's HDR, set by pickHdrFormat and said once
    volatile HdrFormatPolicy.DrawnLimit vulkanHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
    // pickHdrFormat counted on the Vulkan renderer for Frame Warp, not for "Render through Vulkan".
    volatile boolean vulkanForFrameWarp;
    private volatile boolean vulkanHdr10PlusRefusalTaken;
    // Render through Vulkan presents too few of the received frames on this display (decoder input thread).
    private final VulkanPaceCheck vulkanPace = new VulkanPaceCheck();
    private volatile boolean vulkanBehind;
    // Set once the renderer reports it gave up; the stream is then ended with a dialog that says so
    private volatile String vulkanFailure;
    // The screen tells the renderer when frames are shown, so frames wait for that report to count
    // as rendered, like MediaCodec's frame rendered callbacks on the direct path
    private boolean vulkanPresentReports;
    private final long[] vulkanReports = new long[3 * 256];
    private long lastScanoutLogNs;
    private long lastVulkanDiagnosticsNs;
    // The Vulkan renderer's last diagnostics line, for this stream's row in the problem report
    volatile String vulkanDiagnostics;
    private volatile boolean vulkanHandoverPending;
    // The renderer such a stream was handed back from: stopped, freed with the decoder
    volatile VulkanRendererBridge handedBackVulkan;
    // The Vulkan notice (why it is unused, or that it replaces the upscaler) is shown once per stream
    private boolean vulkanNoticeShown;
    // Frame Warp (docs/frame-warp.md): this stream's renderer, Vulkan or the GLES presenter, draws the shift.
    // False with the setting off, and then none of the Frame Warp code below runs.
    volatile boolean frameWarp;
    private boolean frameWarpNoticeShown;
    // The decoder's presentation time per output buffer index, for the GLES presenter's Frame Warp.
    private long[] frameWarpOutputPtsUs = new long[16];
    private final Hdr10PlusSei.Scratch hdr10PlusScratch = new Hdr10PlusSei.Scratch();
    boolean packingFailed;

    MediaCodecVulkanHelper(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    // Once a second, from the latency thread, which runs whether or not frames arrive (a frozen
    // picture is exactly when the renderer has given up).
    void pollVulkan() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        if (vulkan == null) return;
        String fatal = vulkan.getFatalReason();
        if (fatal != null) failVulkan(fatal);
        logVulkanDiagnostics(vulkan);
        handBackToneMappedHdr(vulkan);
        drainVulkanPresentReports(vulkan);
    }

    // Drains Vulkan present timings into FrameLatencyStats (display wait to queue time; scanout
    // to screen stage, Presented FPS and present jitter; scanout 0 = replaced).
    void drainVulkanPresentReports(VulkanRendererBridge vulkan) {
        if (vulkan == null || !vulkanPresentReports) return;
        synchronized (vulkanReports) {
            long nowNs = System.nanoTime();
            for (int n; (n = vulkan.takePresentReports(vulkanReports)) > 0; ) {
                for (int i = 0; i < n; i++) {
                    long ptsUs = vulkanReports[3 * i], queuedNs = vulkanReports[3 * i + 1], scanoutNs = vulkanReports[3 * i + 2];
                    if (scanoutNs == 0) r.frameLatencyStats.onFrameReplaced(ptsUs);
                    else r.frameLatencyStats.onFrameShown(ptsUs, queuedNs, scanoutNs, nowNs);
                }
                if (n < vulkanReports.length / 3) break;
            }
        }
    }

    // Every 5 s: decoded to on screen, the compositor and panel included, which the overlays leave out.
    void logScanoutLatency(long nowNs) {
        if (nowNs - lastScanoutLogNs < 5_000_000_000L) return;
        lastScanoutLogNs = nowNs;
        String line = r.frameLatencyStats.takeScanoutLatency();
        if (line != null) LimeLog.info(line);
    }

    // Every 10 s, so a log started after the stream still has the output and swapchain the Vulkan
    // renderer chose and how often it waited; the last line goes into the problem report too.
    private void logVulkanDiagnostics(VulkanRendererBridge vulkan) {
        long nowNs = System.nanoTime();
        if (lastVulkanDiagnosticsNs != 0 && nowNs - lastVulkanDiagnosticsNs < 10_000_000_000L) return;
        String text = vulkan.getDiagnosticsText();
        if (text == null) return;
        lastVulkanDiagnosticsNs = nowNs;
        vulkanDiagnostics = text;
        LimeLog.info(text);
    }

    // An HDR stream the Vulkan renderer can only tone map looks washed out, while the standard
    // renderer shows the same HDR on this screen (its own surface refused the dataspace or format).
    // Hand the stream back once, on the next decoder restart, and log why. A stream the PC reports
    // as SDR is never handed back.
    private void handBackToneMappedHdr(VulkanRendererBridge vulkan) {
        if (r.vulkanHandedBack || vulkanFailure != null ||
                !HdrPolicy.handsBackFromVulkan(Boolean.TRUE.equals(r.currentHdrMode), vulkan.isHdrToneMapped())) {
            return;
        }
        synchronized (r.codecRecoveryMonitor) {
            if (r.stopping) return;
            r.vulkanHandedBack = true;
            vulkanHandoverPending = true;
            // A planned restart, not a recovery from a failure.
            r.codecRecoveryAttempts = 0;
            // The renderer is let go at the handover: this line keeps the reason in the problem report.
            String diagnostics = vulkan.getDiagnosticsText();
            if (diagnostics != null) vulkanDiagnostics = diagnostics;
            LimeLog.warning("Vulkan HDR can only be shown as SDR" +
                    (diagnostics == null ? "" : " (" + diagnostics + ")") +
                    "; handing this stream to the standard renderer");
            r.session.requestRestart("HDR shown as SDR");
        }
    }

    // The configure of the restart a tone-mapped HDR stream asked for: the codec has let go of the
    // Vulkan renderer's surface (stop, reset or recreate), so the renderer stops presenting here and,
    // once its stop finished, the decoder draws on the standard (direct/GL) path from this configure
    // on. The stopped renderer waits for cleanup rather than being freed on a recovery thread.
    // Returns false when the stream is stopping or the handover timed out and must NOT configure.
    boolean handOverFromVulkan() {
        if (!vulkanHandoverPending) return true;
        if (r.stopping) {
            vulkanHandoverPending = false;
            return false;
        }
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        boolean stopFinished = vulkan == null || vulkan.stop();
        // A stop that gave up after RendererGate.STOP_WAIT_MS is still running, and its native part
        // may not have let go of the output window: MediaCodec must not configure onto that same
        // window then. The gate stays closed, so the Vulkan path cannot be kept either (it would
        // freeze on its last frame): end the stream and say why.
        if (!HdrPolicy.handOverAfterVulkanStop(vulkan != null, stopFinished)) {
            vulkanHandoverPending = false;
            r.stopping = true;
            failVulkan("the renderer did not stop within " + RendererGate.STOP_WAIT_MS +
                    " ms while handing an HDR stream to the standard renderer");
            return false;
        }
        vulkanHandoverPending = false;
        r.wantVulkan = false;
        vulkanForFrameWarp = false;
        if (frameWarp) declineFrameWarp(R.string.frame_warp_reason_renderer);
        r.vulkanRenderer = null;
        if (vulkan != null) handedBackVulkan = vulkan;
        // Frame Warp chooses Vulkan unasked; only a user who asked for it hears it is unused.
        if (r.prefs.vulkanRender) {
            // The standard renderer now shows the HDR: only say Vulkan is not used.
            notifyVulkan(r.context.getString(R.string.vulkan_render_fallback, "").trim());
        }
        // Balanced pacing on the direct path releases frames on the Choreographer, which the Vulkan
        // renderer paced itself and so never started.
        if (r.vsyncPacer.choreographerHandlerThread == null) r.vsyncPacer.startChoreographerThread();
        return true;
    }

    String getVulkanFailure() {
        return vulkanFailure;
    }

    // The renderer can no longer show frames (device lost, or submits kept failing). A frozen
    // picture would look like a network problem, so end the stream and say what happened.
    void failVulkan(String reason) {
        if (vulkanFailure != null) return;
        vulkanFailure = reason;
        LimeLog.severe("Vulkan renderer failed, ending the stream: " + reason);
        MoonBridge.bridgeClConnectionTerminated(MoonBridge.CLIENT_ERROR_VULKAN_RENDERER);
    }

    void setupUpscaling() {
        Display display = r.outputDisplay;
        float panelHz = display.getMode().getRefreshRate();
        r.upscalingPolicy = new UpscalingPolicy(Math.max(1, Math.min(r.refreshRate, Math.round(panelHz))));
        r.wantVulkan = false;
        frameWarp = false;
        // Only with the setting on: MoonBridge's statics load the native engine.
        boolean warpOffered = false;
        if (r.prefs.frameWarp) {
            warpOffered = MoonBridge.isFrameWarpStream();
        }
        boolean pyroWave = (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
        VideoRendererChoice choice = VideoRendererChoice.choose(r.prefs.vulkanRender, pyroWave, Build.VERSION.SDK_INT,
                VulkanRendererBridge::isSupported, r.packing != null, r.prefs.upscalingMode, warpOffered,
                Boolean.TRUE.equals(r.currentHdrMode) || (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0);
        if (choice.frameWarpReason == VideoRendererChoice.FrameWarpReason.HDR) {
            declineFrameWarp(R.string.frame_warp_reason_hdr);
        } else if (choice.frameWarpReason == VideoRendererChoice.FrameWarpReason.FULL_CHROMA) {
            declineFrameWarp(R.string.frame_warp_reason_full_chroma);
        }
        if (choice.renderer == VideoRendererChoice.Renderer.VULKAN) {
            r.wantVulkan = true;
            frameWarp = choice.frameWarp;
            r.upscalingPolicy.fail(UpscalingPolicy.Reason.DISABLED);
            if (choice.upscalingOverridden) {
                notifyVulkan(r.context.getString(R.string.vulkan_overrides_upscaling));
            }
            return;
        }
        if (choice.reason != VideoRendererChoice.Reason.NONE) {
            notifyVulkanUnused(vulkanUnusedReason(choice.reason));
        }
        if (choice.frameWarp && (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // PyroWave's renderer shifts the picture itself, upscaled or not; it takes Frame Warp once the
            // decoder is set up (PyroWaveDecoderRenderer.setFrameWarp)
            frameWarp = true;
        } else if (choice.frameWarp) {
            // Drawn without upscaling: the GLES presenter shifts the picture instead.
            frameWarp = true;
            if (choice.upscalingOverridden) {
                notifyFrameWarp(r.context.getString(R.string.frame_warp_replaces_upscaling));
            }
            r.upscaler = new GlesUpscaler(r.context, r.renderTarget, UpscalingPolicy.Mode.BILINEAR, r.initialWidth,
                    r.initialHeight, r.prefs.stretchVideo, 0, r.upscalingPolicy, null, false, null, true);
            if (r.upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) closeUpscaler();
            return;
        }
        if (r.packing != null) {
            r.upscaler = new GlesUpscaler(r.context, r.renderTarget, UpscalingPolicy.Mode.BILINEAR,
                    r.initialWidth, r.initialHeight, r.prefs.stretchVideo, 0, r.upscalingPolicy, r.packing,
                    (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, r.currentHdrMetadata);
            if (r.currentHdrMode != null) r.upscaler.setHdrMode(r.currentHdrMode, r.currentHdrMetadata);
            return;
        }
        if (r.prefs.upscalingMode == UpscalingPolicy.Mode.OFF) {
            r.upscalingPolicy.fail(UpscalingPolicy.Reason.DISABLED);
            return;
        }
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // The Vulkan renderer runs SGSR itself; the GLES upscaler only serves MediaCodec.
            r.upscalingPolicy.fail(UpscalingPolicy.Reason.PYROWAVE);
            return;
        }
        int[] viewport = UpscalingPolicy.viewport(r.initialWidth, r.initialHeight, r.renderWidth, r.renderHeight, r.prefs.stretchVideo);
        boolean hostHdr = Boolean.TRUE.equals(r.currentHdrMode);
        boolean hdrOrTenBit = hostHdr || (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
        UpscalingPolicy.Reason reason = UpscalingPolicy.colorReason(UpscalingPolicy.unavailableReason(
                r.prefs.upscalingMode, r.initialWidth, r.initialHeight, viewport[2], viewport[3], hdrOrTenBit, false, true),
                hostHdr, r.codecSelector.sdr10);
        r.upscalingPolicy.fail(reason);
        if (reason == UpscalingPolicy.Reason.NONE) {
            r.upscaler = new GlesUpscaler(r.context, r.renderTarget, r.prefs.upscalingMode, r.initialWidth, r.initialHeight,
                    r.prefs.stretchVideo, r.prefs.upscalingSharpness, r.upscalingPolicy);
            if (r.upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) closeUpscaler();
        } else {
            notifyUpscalingFallback();
        }
    }

    private String vulkanUnusedReason(VideoRendererChoice.Reason reason) {
        switch (reason) {
            case PYROWAVE: return r.context.getString(R.string.vulkan_reason_pyrowave);
            case API_LEVEL: return r.context.getString(R.string.vulkan_reason_api);
            case PACKED_444: return r.context.getString(R.string.vulkan_reason_packed);
            case UNSUPPORTED: {
                String detail = VulkanRendererBridge.getUnsupportedReason();
                return r.context.getString(R.string.vulkan_reason_unsupported) +
                        (detail == null || detail.isEmpty() ? "" : " (" + detail + ")");
            }
            default: return "";
        }
    }

    // Vulkan was asked for but is not the renderer: say why, in the log and once on screen.
    void notifyVulkanUnused(String reason) {
        notifyVulkan(r.context.getString(R.string.vulkan_render_fallback, reason));
    }

    private void notifyVulkan(String message) {
        LimeLog.warning(message);
        if (vulkanNoticeShown) return;
        vulkanNoticeShown = true;
        showNotice(message);
    }

    // What became of a stream whose decoder dropped full color range (nightjar#153). An earlier
    // Vulkan notice does not hold it back: that one said nothing about the black levels.
    void notifyFullRange(int message) {
        String text = r.context.getString(message);
        LimeLog.warning(text);
        showNotice(text);
    }

    private void showNotice(String message) {
        r.activity.runOnUiThread(() -> {
            if (!r.stopping) Toast.makeText(r.context, message, Toast.LENGTH_LONG).show();
        });
    }

    private void notifyFrameWarp(String message) {
        LimeLog.warning("Frame Warp: " + message);
        if (frameWarpNoticeShown) return;
        frameWarpNoticeShown = true;
        showNotice(message);
    }

    /** The stream offers Frame Warp but this renderer cannot draw it: the host stops its work, and the user is told. */
    void declineFrameWarp(int reason) {
        frameWarp = false;
        MoonBridge.frameWarpDecline();
        notifyFrameWarp(r.context.getString(R.string.frame_warp_unavailable, r.context.getString(reason)));
    }

    VulkanRendererBridge createVulkanRenderer() {
        VulkanRendererBridge renderer = VulkanRendererBridge.create(r.renderTarget,
                r.initialWidth, r.initialHeight, r.refreshRate, r.prefs.framePacing,
                VULKAN_JITTER_BUFFER, 0, r.getPreferredColorSpace(),
                r.getPreferredColorRange() == MoonBridge.COLOR_RANGE_FULL,
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0,
                r.getStreamTransfer() == MediaFormat.COLOR_TRANSFER_HLG,
                r.outputDisplay.getMode().getRefreshRate(),
                vulkanCodecName(), r.videoDecoder.getName(), null, false,
                frameWarp ? MoonBridge.frameWarpApi() : 0);
        if (renderer != null && frameWarp && !renderer.drawsFrameWarp()) {
            declineFrameWarp(R.string.frame_warp_reason_renderer);
        }
        if (renderer != null) {
            r.vulkanHdr10Plus = HdrFormatPolicy.passesHdr10Plus(HdrPolicy.displayHdrTypes(r.outputDisplay),
                    r.getStreamTransfer());
            r.vulkanHdr10PlusNoted = null;
            vulkanPresentReports = renderer.reportsPresentTimes();
            LimeLog.info("Using Vulkan renderer; the screen " + (vulkanPresentReports ?
                    "reports when frames are shown" : "does not report when frames are shown"));
            renderer.setVsyncOffset(r.outputDisplay.getAppVsyncOffsetNanos());
            renderer.setAdaptiveRefresh(r.vsyncPacer.holdsNothingForVrr());
            // A 10-bit stream starts in HDR10 before the PC's HDR report arrives, as the decoder is
            // configured (assumesHdr), so the report doesn't recreate the swapchain a second in.
            if (r.currentHdrMode != null ? r.currentHdrMode : r.codecSelector.assumesHdr()) {
                renderer.setHdrMode(true, r.currentHdrMetadata);
            }
        }
        return renderer;
    }

    private String vulkanCodecName() {
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) return "hevc";
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) return "av1";
        return "h264";
    }

    void closeUpscaler() {
        closeUpscaler(false);
    }

    void closeUpscaler(boolean handoverToVulkan) {
        if (r.upscaler != null) {
            boolean warp = r.upscaler.drawsFrameWarp();
            r.upscaler.close();
            r.upscaler = null;
            if (handoverToVulkan || r.stopping) {
                return;
            }
            if (warp) {
                declineFrameWarp(r.upscalingPolicy.getReason() == UpscalingPolicy.Reason.HDR ?
                        R.string.frame_warp_reason_hdr : R.string.frame_warp_reason_renderer);
            } else {
                notifyUpscalingFallback();
            }
        }
    }

    void notifyUpscalingFallback() {
        UpscalingPolicy.Reason reason = r.upscalingPolicy.getReason();
        if (reason != UpscalingPolicy.Reason.NONE && reason != UpscalingPolicy.Reason.DISABLED &&
                reason != UpscalingPolicy.Reason.NO_UPSCALE) {
            String message = r.context.getString(R.string.upscaling_fallback, upscalingReason(reason));
            LimeLog.info(message);
            r.activity.runOnUiThread(() -> {
                if (!r.stopping) Toast.makeText(r.context, message, Toast.LENGTH_LONG).show();
            });
        }
    }

    private String upscalingReason(UpscalingPolicy.Reason reason) {
        switch (reason) {
            case HDR: return r.context.getString(R.string.upscaling_hdr);
            case TEN_BIT: return r.context.getString(R.string.upscaling_tenbit);
            case PYROWAVE: return r.context.getString(R.string.upscaling_pyrowave);
            case NO_UPSCALE: return r.context.getString(R.string.upscaling_native);
            case UNSUPPORTED: return r.context.getString(R.string.upscaling_unsupported);
            case GPU_ERROR: return r.context.getString(R.string.upscaling_gpu_error);
            case SLOW: return r.context.getString(R.string.upscaling_slow);
            case DROPPED: return r.context.getString(R.string.upscaling_dropped);
            case SIZE_CHANGED: return r.context.getString(R.string.upscaling_size_changed);
            default: return "";
        }
    }

    /**
     * The overlay's packed full-chroma line: the bit depth and HDR state, and why HDR is off when it was dropped.
     * The video line above it already reads "YUV 4:4:4 (packed)".
     */
    private String packedStats() {
        boolean tenBit = (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
        String drop = r.codecSelector.packedHdrDrop;
        if (drop != null) return "8-bit SDR, HDR off: " + drop;
        if (!tenBit) return "8-bit SDR";
        Boolean hdr = r.currentHdrMode;
        return "10-bit " + (hdr == null ? "(waiting for the PC's HDR report)" :
                hdr ? HdrFormatPolicy.baseLayerName(r.announcedHdrFormat) : "SDR (the PC reports SDR)");
    }

    /** The overlay's name of what PyroWave's Vulkan renderer enlarges with. */
    static String pyroWaveModeName(UpscalingPolicy.Mode mode, String bilinearName) {
        return mode == UpscalingPolicy.Mode.BILINEAR ? bilinearName : "SGSR (Vulkan)";
    }

    String upscalingStats() {
        if (r.packing != null) return packedStats();
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            UpscalingPolicy.Reason reason = r.pyroWaveRenderer.getUpscalingReason();
            String mode = reason == UpscalingPolicy.Reason.NONE ?
                    pyroWaveModeName(r.prefs.upscalingMode, r.context.getString(R.string.upscaling_bilinear)) :
                    r.context.getString(R.string.upscaling_direct);
            String text = r.context.getString(R.string.perf_overlay_upscaling_pyrowave, mode);
            String explanation = upscalingReason(reason);
            return explanation.isEmpty() ? text : text + '\n' + explanation;
        }
        GlesUpscaler presenter = r.upscaler;
        boolean active = presenter != null;
        String mode = active ? (presenter.drawsFrameWarp() ? r.context.getString(R.string.title_frame_warp) :
                r.context.getResources().getStringArray(R.array.upscaling_names)[r.prefs.upscalingMode.ordinal()]) :
                r.context.getString(R.string.upscaling_direct);
        float addedMs = r.upscalingPolicy == null ? -1 : r.upscalingPolicy.getAddedMs();
        String text = r.context.getString(R.string.perf_overlay_upscaling, mode, MediaCodecTelemetry.metric(active ? addedMs : 0, " ms"));
        if (active) text += '\n' + r.context.getString(R.string.upscaling_timing_estimate);
        else if (addedMs >= 0) text += '\n' + r.context.getString(R.string.upscaling_last_timing, MediaCodecTelemetry.metric(addedMs, " ms"));
        if (r.upscalingPolicy != null) {
            String reason = upscalingReason(r.upscalingPolicy.getReason());
            if (!reason.isEmpty()) text += '\n' + reason;
        }
        return text;
    }

    /** Why packed full chroma stopped, in the words of the packed path (it has no direct output to fall back to). */
    private String packedFailureReason() {
        UpscalingPolicy.Reason reason = r.upscalingPolicy.getReason();
        GlesUpscaler current = r.upscaler;
        switch (reason) {
            case SLOW: return "the GPU reconstruction could not keep up with the frame rate";
            case GPU_ERROR: {
                String detail = current == null ? null : current.failureDetail();
                return "the GPU path or decoder surface failed" + (detail == null ? "" : ": " + detail);
            }
            case HDR: return "this display no longer reports HDR10 output";
            case SIZE_CHANGED: return "the output surface changed size";
            default: return upscalingReason(reason);
        }
    }

    void checkUpscaling() {
        GlesUpscaler current = r.upscaler;
        if (current == null || r.stopping) return;
        if (r.packing != null && (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 &&
                !r.codecSelector.packedHdrAvailable()) r.upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
        current.checkHealth();
        if (r.upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) {
            if (r.packing != null) {
                if (!packingFailed) {
                    packingFailed = true;
                    String reason = packedFailureReason();
                    MediaCodecDecoderRenderer.PackedFailureListener listener = r.reconfigure.packedFailureListener;
                    if (listener != null) {
                        // The listener reconnects without packed chroma, or ends the stream itself.
                        listener.onPackedFailure(reason);
                    } else {
                        MoonBridge.bridgeClDisplayMessage("Packed full-chroma presentation stopped: " + reason);
                        MoonBridge.bridgeClConnectionTerminated(-1);
                    }
                }
                return;
            }
            synchronized (r.codecRecoveryMonitor) {
                if (r.stopping) return;
                r.codecRecoveryAttempts = 0;
                r.session.requestRestart("upscaler failure");
            }
        }
    }

    void checkUpscalingOutputFormat() {
        GlesUpscaler current = r.upscaler;
        if (current == null) return;
        // AV1 from AMD encoders arrives with its alignment padding uncropped; the upscaler samples
        // the stream's part of it, and only a picture of another size ends upscaling.
        float[] crop = UpscalingPolicy.checkOutputFormat(r.outputFormat, r.packing != null, r.initialWidth, r.initialHeight,
                current.insetsCroppedEdges(), Boolean.TRUE.equals(r.currentHdrMode), r.codecSelector.sdr10, r.upscalingPolicy);
        if (crop == null) return;
        current.setPictureCrop(crop);
        if (crop[0] != 1f || crop[1] != 1f) {
            LimeLog.info("Upscaling samples the " + r.initialWidth + "x" + r.initialHeight +
                    " stream of a padded decoded picture (scale " + crop[0] + " x " + crop[1] + ")");
        }
    }

    /** The decoder time of output index, as noteDecoderOutput() kept it for Frame Warp; -1 unknown. */
    long frameWarpPtsUs(int index) {
        long[] pts = frameWarpOutputPtsUs;
        return index >= 0 && index < pts.length ? pts[index] : -1;
    }

    void recordFrameWarpPts(int index, long ptsUs) {
        if (frameWarp && index >= 0) {
            if (index >= frameWarpOutputPtsUs.length) {
                frameWarpOutputPtsUs = Arrays.copyOf(frameWarpOutputPtsUs, index + 16);
            }
            frameWarpOutputPtsUs[index] = ptsUs;
        }
    }

    void handOutputToVulkan(int lastIndex, long lastPtsUs) {
        // The Vulkan renderer paces and drops frames itself, so hand each
        // one over as soon as it is decoded. It counts those it presents.
        r.frameLatencyStats.onOutputReleased(lastIndex, System.nanoTime(), true,
                vulkanPresentReports);
        r.videoDecoder.releaseOutputBuffer(lastIndex, true);
        r.frameLatencyStats.onArtemisDecodeCounted(lastPtsUs);
    }

    // The frame's HDR10+ payload, when it differs from the last one: a frame without one keeps it.
    void noteVulkanHdr10Plus(VulkanRendererBridge vulkan, long timestampUs, ByteBuffer direct,
                             byte[] data, int length) {
        byte[] t35 = (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0
                ? (direct != null ? Hdr10PlusSei.av1IfChanged(direct, length, r.vulkanHdr10PlusNoted, hdr10PlusScratch)
                        : Hdr10PlusSei.av1IfChanged(data, length, r.vulkanHdr10PlusNoted, hdr10PlusScratch))
                : (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0
                ? (direct != null ? Hdr10PlusSei.hevcIfChanged(direct, length, r.vulkanHdr10PlusNoted, hdr10PlusScratch)
                        : Hdr10PlusSei.hevcIfChanged(data, length, r.vulkanHdr10PlusNoted, hdr10PlusScratch))
                : null;
        if (t35 == null || t35 == r.vulkanHdr10PlusNoted) return;
        if (r.vulkanHdr10PlusNoted == null) LimeLog.info("HDR10+ metadata in the stream (" + t35.length + " bytes)");
        r.vulkanHdr10PlusNoted = t35;
        vulkan.noteHdr10Plus(timestampUs, t35);
    }

    /** The Vulkan renderer tone maps the PC's HDR to SDR because its surface refused HDR. */
    boolean isVulkanHdrToneMapped() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        return vulkan != null && vulkan.isHdrToneMapped();
    }

    /** The Vulkan renderer presents HDR10 or HLG, not tone mapped: its current swapchain is an HDR one. */
    boolean isVulkanHdrOutput() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        return vulkan != null && Boolean.TRUE.equals(r.currentHdrMode) &&
                HdrRecomposition.rendererShowsHdr(vulkan.getRendererText());
    }

    /** The Vulkan renderer hands this stream's HDR10+ metadata to the screen. */
    boolean vulkanPassesHdr10Plus() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        return vulkan != null && vulkan.getHdr10PlusState() == VulkanRendererBridge.HDR10_PLUS_PASSED;
    }

    boolean takeVulkanHdr10PlusRefused() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        if (vulkan == null || vulkanHdr10PlusRefusalTaken ||
                vulkan.getHdr10PlusState() != VulkanRendererBridge.HDR10_PLUS_REFUSED) {
            return false;
        }
        vulkanHdr10PlusRefusalTaken = true;
        return true;
    }

    boolean takeVulkanBehind() {
        if (!vulkanBehind) return false;
        vulkanBehind = false;
        return true;
    }

    HdrFormatPolicy.DrawnLimit takeVulkanHdrLimit() {
        if (r.vulkanRenderer == null) return HdrFormatPolicy.DrawnLimit.NONE;
        HdrFormatPolicy.DrawnLimit limit = vulkanHdrLimit;
        vulkanHdrLimit = HdrFormatPolicy.DrawnLimit.NONE;
        return limit;
    }

    void onFlipStatsWindow() {
        VulkanRendererBridge vulkan = r.vulkanRenderer;
        if (vulkan != null) {
            int presented = vulkan.takePresentedFrames();
            r.activeWindowVideoStats.totalFramesRendered += presented;
            long windowMs = SystemClock.uptimeMillis() - r.activeWindowVideoStats.measurementStartTimestamp;
            if (vulkanPace.window(r.activeWindowVideoStats.totalFramesReceived, presented, windowMs,
                    r.outputDisplay.getRefreshRate())) {
                LimeLog.warning("Render through Vulkan presents " + presented + " of " +
                        r.activeWindowVideoStats.totalFramesReceived + " frames received in " + windowMs +
                        " ms at " + Math.round(r.outputDisplay.getRefreshRate()) + " Hz, under " +
                        Math.round(VulkanPaceCheck.MIN_SHARE * 100) + "% for " +
                        VulkanPaceCheck.BEHIND_MS / 1000 + " s");
                vulkanBehind = true;
            }
            // The display mode can change mid-stream
            vulkan.setVsyncOffset(r.outputDisplay.getAppVsyncOffsetNanos());
            vulkan.setAdaptiveRefresh(r.vsyncPacer.holdsNothingForVrr());
        }
    }
}
