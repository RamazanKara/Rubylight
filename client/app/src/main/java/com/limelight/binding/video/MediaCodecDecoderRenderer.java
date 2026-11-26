package com.limelight.binding.video;

import java.io.BufferedWriter;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

import android.app.Activity;
import android.content.Context;
import android.hardware.display.DisplayManager;
import android.media.MediaCodec;
import android.media.MediaCodec.BufferInfo;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Debug;
import android.os.Handler;
import android.os.Process;
import android.os.SystemClock;
import android.view.Choreographer;
import android.view.Display;
import android.view.Surface;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

public class MediaCodecDecoderRenderer extends VideoDecoderRenderer implements Choreographer.FrameCallback {

    public static final int PYROWAVE_NOT_OFFERED = -1, PYROWAVE_FULL_CHROMA = -2, PYROWAVE_NO_HDR_ON_PC = -3;
    public static final String LATENCY_CSV_NAME = "butterpollo-latency.csv", LATENCY_CSV_EXPORT_NAME = "nightjar-latency.csv";
    static final String COVERING_SWITCH = "covering_max";
    static final int PACKED_10BIT_MODES = 0b1010, HDR_STATIC_INFO_OUTPUTS = 60;
    static final long SCHEDULED_LATCH_ROOM_NS = 1_000_000L;
    static final String ISSUE_HOST_NO_TEN_BIT = "host doesn't offer 10-bit full chroma", ISSUE_NO_TEN_BIT_IN_COMMON = "this device and PC share no 10-bit full chroma mode";
    static final String ISSUE_NO_HDR10_DISPLAY = "this display does not report HDR10", ISSUE_NO_TEN_BIT_OUTPUT = "the GPU offers no 10-bit BT.2020 PQ output surface", ISSUE_TEN_BIT_SAMPLING = "the decoder's raw 10-bit output could not be sampled";
    static final int CR_MAX_TRIES = 10, CR_RECOVERY_TYPE_NONE = 0, CR_RECOVERY_TYPE_FLUSH = 1, CR_RECOVERY_TYPE_RESTART = 2, CR_RECOVERY_TYPE_RESET = 3;
    static final int CR_FLAG_INPUT_THREAD = 0x1, CR_FLAG_RENDER_THREAD = 0x2, CR_FLAG_CHOREOGRAPHER = 0x4, CR_FLAG_ALL = CR_FLAG_INPUT_THREAD | CR_FLAG_RENDER_THREAD | CR_FLAG_CHOREOGRAPHER;

    MediaCodecInfo avcDecoder, hevcDecoder, av1Decoder, hevcDecoder10, av1Decoder10, dolbyVisionDecoder, dolbyVisionAv1Decoder;
    volatile int announcedHdrFormat;
    // The announced Dolby Vision 8.1 stream was asked for full range.
    volatile boolean dolbyVisionFullRange;
    volatile boolean dolbyVisionFailed, dolbyVisionActive, reconfigureRequested, packedTenBitOffered, fullRangeFallbackAttempted, rebuildForVulkan, stopping;
    String vividSeiDropDecoder, packedTenBitIssue, glRenderer;
    volatile byte[] currentHdrMetadata, uncheckedHdrStaticInfo;
    volatile Boolean currentHdrMode;
    Context context;
    Activity activity;
    MediaCodec videoDecoder;
    Thread rendererThread;
    boolean adaptivePlayback, refFrameInvalidationHevc, refFrameInvalidationHevc10, refFrameInvalidationAv110, refFrameInvalidationActive, coveringRefused, wantVulkan, vulkanHdr10Plus, vulkanHandedBack, restartRecreates, refreshSlotsEnabled, outputHasArrSupport;
    int initialWidth, initialHeight, videoFormat, renderWidth, renderHeight, consecutiveCrashCount, codecRecoveryThreadQuiescedFlags, codecRecoveryAttempts, pyroWaveUnavailableReason, requiredInputBufferSize, heldIndex, lastFrameNumber, hdrStaticInfoOutputs, numSpsIn, numPpsIn, numVpsIn;
    Packed444 packing;
    volatile int maxDecodeWidth, maxDecodeHeight, expectedWidth, expectedHeight, framesOutAtHangReset = -1, hottestThermalLevel = ThermalMonitor.NORMAL, thermalLevel = ThermalMonitor.NORMAL, refreshRate, streamFrameRateMilliHz, streamBitrateKbps, heldAnchorRefreshes = 1, numFramesIn, numFramesOut, decoderDepthBase;
    int[] coveringSize;
    Surface renderTarget;
    volatile GlesUpscaler upscaler;
    volatile UpscalingPolicy upscalingPolicy;
    volatile VulkanRendererBridge vulkanRenderer;
    byte[] vulkanHdr10PlusNoted;
    CrashListener crashListener;
    PerfOverlayListener perfListener;
    AtomicInteger codecRecoveryType = new AtomicInteger(CR_RECOVERY_TYPE_NONE);
    final Object codecRecoveryMonitor = new Object(), vsyncMonitor = new Object();
    long hangResetTimeoutMs = MediaCodecRecovery.HANG_RESET_TIMEOUT_MS, streamStartMs = -1, streamStartUptimeMs, latencyCsvBytes, vsyncTimeNs, vsyncIntervalNs, vsyncPresentationDeadlineNs, appVsyncOffsetNs, balancedOnOutput, balancedOnVsync, vulkanOutputs, phaseReports;
    volatile boolean hangResetPending, restartWithoutVendorKeys, recoveryAfterFailure, heatHoldsBitrate, heatEasesLoad, hostFrameTimes, hostPhaseLock, submitHints;
    volatile long inputWaitSinceMs = -1, lastVideoFrameTimeMs;
    // A test pins the output path's clocks here (MediaCodecBalancedQueueTest); 0 reads the system clocks.
    long testClockNs;
    long nowNs() { long t = testClockNs; return t != 0 ? t : System.nanoTime(); }
    long elapsedNs() { long t = testClockNs; return t != 0 ? t : SystemClock.elapsedRealtimeNanos(); }
    volatile SubmitHandoff<MediaCodecDecodeLoop.HandedUnit> submitHandoff;
    final Map<String, HungKeys> hungKeys = new ConcurrentHashMap<>();
    DecoderRecoveryBackoff recoveryBackoff = new DecoderRecoveryBackoff();
    volatile String decoderFailure, activeDecoderName, restartReason = "CodecException";
    volatile MediaFormat outputFormat;
    MediaFormat configuredFormat;
    VideoStats activeWindowVideoStats, globalVideoStats;
    volatile KeyframeSafetyNet keyframeSafetyNet;
    volatile InputSpacer inputSpacer;
    volatile KeyframeSafetyNet.Counts repairCounts;
    final PyroWaveDecoderRenderer pyroWaveRenderer = new PyroWaveDecoderRenderer();
    final com.limelight.utils.LateFrames lateFrames = new com.limelight.utils.LateFrames();
    final FrameLatencyStats frameLatencyStats;
    final HostFrameClock hostFrameClock = new HostFrameClock();
    final PresentScheduler presentScheduler = new PresentScheduler(hostFrameClock);
    final DecoderQueuePolicy.Residency residency = new DecoderQueuePolicy.Residency();
    final DecoderQueuePolicy.InFlight inFlight = new DecoderQueuePolicy.InFlight();
    final DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
    final CompositorWatch compositorWatch = new CompositorWatch();
    Handler latencyHandler;
    BufferedWriter latencyCsv;
    volatile List<String> lowLatencyKeysVendor = new ArrayList<>();
    PreferenceConfiguration prefs;
    final IntRing outputBufferQueue = new IntRing();
    int outputBufferQueueLimit = 1;
    final RefreshSlots refreshSlots = new RefreshSlots();
    final PyroWaveDisplayClock pyroWaveDisplayClock = new PyroWaveDisplayClock();
    Display streamDisplay;
    final Display outputDisplay;
    final boolean outputVsyncAvailable;
    final DecoderPerformanceHints.SubmitWork submitWork = new DecoderPerformanceHints.SubmitWork();
    ByteBuffer directInput;

    final MediaCodecVulkanHelper vulkanHelper = new MediaCodecVulkanHelper(this);
    final MediaCodecTelemetry telemetry = new MediaCodecTelemetry(this);
    final MediaCodecCodecSelector codecSelector = new MediaCodecCodecSelector(this);
    final MediaCodecReconfigure reconfigure = new MediaCodecReconfigure(this);
    final MediaCodecSession session = new MediaCodecSession(this);
    final MediaCodecRecovery recovery = new MediaCodecRecovery(this);
    final MediaCodecVsyncPacer vsyncPacer = new MediaCodecVsyncPacer(this);
    final MediaCodecFramePresenter framePresenter = new MediaCodecFramePresenter(this);
    final MediaCodecDecodeLoop decodeLoop = new MediaCodecDecodeLoop(this);

    final Runnable updateLatencyStats = new Runnable() {
        @Override public void run() { telemetry.runLatencyTick(this); }
    };
    final DisplayManager.DisplayListener displayListener = vsyncPacer.createDisplayListener();
    final CodecRecoverySteps.Codec recoveryCodec = recovery.createRecoveryCodec();
    final ScheduledRelease scheduledRelease = new ScheduledRelease(presentScheduler, inFlight, vsyncPacer.createReleasePort());

    public MediaCodecDecoderRenderer(Activity activity, PreferenceConfiguration prefs,
                                     CrashListener crashListener, int consecutiveCrashCount,
                                     boolean meteredData, boolean requestedHdr,
                                     String glRenderer, PerfOverlayListener perfListener) {
        this(activity, prefs, crashListener, consecutiveCrashCount, meteredData, requestedHdr,
                glRenderer, perfListener, activity.getWindowManager().getDefaultDisplay());
    }

    public MediaCodecDecoderRenderer(Activity activity, PreferenceConfiguration prefs,
                                     CrashListener crashListener, int consecutiveCrashCount,
                                     boolean meteredData, boolean requestedHdr,
                                     String glRenderer, PerfOverlayListener perfListener, Display outputDisplay) {
        this.outputDisplay = outputDisplay;
        this.outputVsyncAvailable = outputDisplay.getDisplayId() == Display.DEFAULT_DISPLAY;
        this.frameLatencyStats = new FrameLatencyStats(prefs.vrr);
        this.frameLatencyStats.setCompositorWatch(this.compositorWatch);
        codecSelector.init(activity, prefs, crashListener, consecutiveCrashCount, requestedHdr,
                glRenderer, perfListener, outputDisplay);
    }

    public interface StreamSizeListener {
        void onStreamSizeChanged(int width, int height);
        default void onDecodedSize(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {}
    }

    public interface PackedFailureListener {
        void onPackedFailure(String reason);
    }

    interface SizeCheck {
        boolean takes(int width, int height);
    }

    enum HdrChange { NONE, PARAMETERS, RESTART }

    static final class HungKeys {
        final List<String> keys;
        final boolean earlierStream;

        HungKeys(List<String> keys, boolean earlierStream) {
            this.keys = keys;
            this.earlierStream = earlierStream;
        }
    }

    public String getVulkanFailure() { return vulkanHelper.getVulkanFailure(); }
    public String getDecoderFailure() { return decoderFailure; }
    public boolean decoderFailedWithVulkan() { return telemetry.decoderFailedWithVulkan; }
    private void failDecoder(String reason, Exception cause) { telemetry.failDecoder(reason, cause); }
    @Override public void onHostTiming(HostTimingReport report) { telemetry.onHostTiming(report); }
    public static String formatLatencyOverlay(Context context, double[][] summary) { return formatLatencyOverlay(context, summary, -1); }
    public static String formatLatencyOverlay(Context context, double[][] summary, float decodeAverageMs) { return formatLatencyOverlay(context, summary, decodeAverageMs, false); }
    static String formatLatencyOverlay(Context context, double[][] summary, float decodeAverageMs, boolean pyroWave) { return MediaCodecTelemetry.formatLatencyOverlay(context, summary, decodeAverageMs, pyroWave); }
    public boolean hasReceivedVideo() { return lastVideoFrameTimeMs != 0; }
    public boolean hasRecentVideoFrames(long nowMs) { return lastVideoFrameTimeMs != 0 && nowMs - lastVideoFrameTimeMs < 2000; }
    public float getNetworkFrameLossPercent() { return telemetry.networkFrameLossPercent; }
    public float getPyroWaveLossPercent() { return telemetry.getPyroWaveLossPercent(); }
    public float getPyroWaveQueueDelayMs() { return telemetry.getPyroWaveQueueDelayMs(); }
    public float getDecodeTimeMs() { return telemetry.getDecodeTimeMs(); }
    public com.limelight.utils.StreamHealth.Sample healthSample(long nowMs, int fps, float jitterMs) { return telemetry.healthSample(nowMs, fps, jitterMs); }
    public void skipHealthSample() { telemetry.skipHealthSample(); }
    public LatencyProbe getLatencyProbe() { return frameLatencyStats.latencyProbe; }
    public int getThermalLevel() { return thermalLevel; }
    public void setHeatEffects(boolean holdsBitrate, boolean easesLoad) { heatHoldsBitrate = holdsBitrate; heatEasesLoad = easesLoad; }
    public void setStreamBitrateKbps(int kbps) { streamBitrateKbps = kbps; }
    private void noteVideoReceiveBuffer(long info) { telemetry.noteVideoReceiveBuffer(info); }
    private String thermalStats() { return telemetry.thermalStats(); }
    static int thermalState(int level, boolean holdsBitrate, boolean easesLoad) { return MediaCodecTelemetry.thermalState(level, holdsBitrate, easesLoad); }
    public float getQueueDelayMs() { return telemetry.getQueueDelayMs(); }
    static String formatPyroWaveStats(float recordLoss, float queueMs, int gpuUs) { return MediaCodecTelemetry.formatPyroWaveStats(recordLoss, queueMs, gpuUs); }
    static long recordFrameGap(VideoStats window, int gap, long newDecoderDrops) { return MediaCodecTelemetry.recordFrameGap(window, gap, newDecoderDrops); }
    static void recordHostProcessing(VideoStats window, char frameHostProcessingLatency) { MediaCodecTelemetry.recordHostProcessing(window, frameHostProcessingLatency); }
    public int getAverageEndToEndLatency() { return frameLatencyStats.getAverageEndToEndLatency(); }
    public int getAverageDecoderLatency() { return telemetry.getAverageDecoderLatency(); }

    MediaCodecInfo decoderFor(int format) { return codecSelector.decoderFor(format); }
    public void setStreamSizeListener(StreamSizeListener listener) { reconfigure.streamSizeListener = listener; }
    public void setPackedFailureListener(PackedFailureListener listener) { reconfigure.packedFailureListener = listener; }
    public int[] getStreamSize() { return reconfigure.getStreamSize(); }
    public int[] getReconfigureLimits() { return reconfigure.getReconfigureLimits(); }
    public boolean reconfigureLimitsPending() { return reconfigure.reconfigureLimitsPending(); }
    public boolean runUnlessRebuilding(Runnable action) { return reconfigure.runUnlessRebuilding(action); }
    void startRebuild() { reconfigure.startRebuild(); }
    boolean endRebuild() { return reconfigure.endRebuild(); }
    public void setReconfigureLimitsListener(Runnable listener) { reconfigure.setReconfigureLimitsListener(listener); }
    void notifyReconfigureLimits() { reconfigure.notifyReconfigureLimits(); }
    private void publishReconfigureLimits() { reconfigure.publishReconfigureLimits(); }

    public void setCoveringSize(int[] covering) {
        if (covering != null && !MediaCodecHelper.debugSwitch(COVERING_SWITCH, prefs.coveringDecoderSize)) {
            covering = null;
        }
        coveringSize = covering;
        coveringRefused = false;
    }

    static int[] upFrontMax(int width, int height, int[] covering, MediaCodecInfo.VideoCapabilities caps, boolean h264) { return MediaCodecReconfigure.upFrontMax(width, height, covering, caps, h264); }
    static int[] upFrontMax(int width, int height, int[] covering, int widthAlignment, int heightAlignment, SizeCheck decoder, boolean h264) { return MediaCodecReconfigure.upFrontMax(width, height, covering, widthAlignment, heightAlignment, decoder, h264); }
    public boolean prepareForReconfigure(int width, int height) { return reconfigure.prepareForReconfigure(width, height); }
    public void cancelReconfigure() { reconfigure.cancelReconfigure(); }
    private boolean applyStreamChanges() { return reconfigure.applyStreamChanges(); }
    public void setFrameRate(int fps) { setStreamFrameRateMilliHz(fps * 1000); }
    @Override public void setStreamFrameRateMilliHz(int fpsMilliHz) { reconfigure.setStreamFrameRateMilliHz(fpsMilliHz); }
    public int getStreamFrameRateMilliHz() { return streamFrameRateMilliHz; }
    public int getContentRateMilliHz() { return hostFrameClock.contentRateMilliHz(); }
    public float[] getFrameRates() { vulkanHelper.drainVulkanPresentReports(vulkanRenderer); return frameLatencyStats.getFrameRates(System.nanoTime()); }
    public com.limelight.nvstream.DecodeTiming takeDecodeTiming(int fps) { return frameLatencyStats.takeDecodeTiming(System.nanoTime(), fps); }
    public void resetDecodeTiming() { frameLatencyStats.resetDecodeTiming(); }
    public void resetCompositorWatchWindow() { compositorWatch.resetWindow(); }
    public int pollCompositorKeepaliveAction(boolean overlayVisible, boolean manualKeepaliveOn) {
        vulkanHelper.drainVulkanPresentReports(vulkanRenderer);
        return compositorWatch.poll(System.nanoTime(), overlayVisible, manualKeepaliveOn);
    }
    public String takeCompositorWatchEvent() { return compositorWatch.takeEvent(); }
    public boolean isCompositorKeepaliveLockedOn() { return compositorWatch.isLockedOn(); }
    public void setRenderTarget(Surface renderTarget, int width, int height) { this.renderTarget = renderTarget; renderWidth = width; renderHeight = height; }
    private void setupUpscaling() { vulkanHelper.setupUpscaling(); }
    static String pyroWaveModeName(UpscalingPolicy.Mode mode, String bilinearName) { return MediaCodecVulkanHelper.pyroWaveModeName(mode, bilinearName); }

    public boolean isHevcSupported() { return hevcDecoder != null; }
    public boolean isAvcSupported() { return avcDecoder != null; }
    public boolean isHevcMain10Hdr10Supported() { return codecSelector.isHevcMain10Hdr10Supported(); }
    public boolean isHevcMain10Supported() { return codecSelector.isHevcMain10Supported(); }
    public boolean isAv1Supported() { return av1Decoder != null; }
    public boolean isAv1Main10SdrSupported() { return codecSelector.isAv1Main10SdrSupported(); }
    public boolean isAv1Main10Supported() { return codecSelector.isAv1Main10Supported(); }
    public boolean isPyroWaveSupported() { return codecSelector.isPyroWaveSupported(); }
    public int getPyroWaveDropReason() { return codecSelector.pyroWaveDropReason; }
    @Override public void setHostCodecModeSupport(int serverCodecModeSupport) { codecSelector.hostCodecModeSupport = serverCodecModeSupport; }
    static int yuv444FallbackMessage(int format, boolean sdr10) { return MediaCodecCodecSelector.yuv444FallbackMessage(format, sdr10); }
    static boolean pyroWaveFullChromaDrop(boolean packed444, boolean yuv444) { return MediaCodecCodecSelector.pyroWaveFullChromaDrop(packed444, yuv444); }
    static int pyroWaveDropReason(boolean chosen, boolean fullChroma, int deviceReason, int hostCodecs, int formats, int choice, int hdrFailure) { return MediaCodecCodecSelector.pyroWaveDropReason(chosen, fullChroma, deviceReason, hostCodecs, formats, choice, hdrFailure); }
    public int getSupportedVideoFormats(boolean hdr) { return getSupportedVideoFormats(hdr, false); }
    public int getSupportedVideoFormats(boolean hdr, boolean sdr10) { return codecSelector.getSupportedVideoFormats(hdr, sdr10); }
    public static int firstPyroWaveFormat(int formats, java.util.function.IntPredicate setup) { return MediaCodecCodecSelector.firstPyroWaveFormat(formats, setup); }
    @Override public int pyroWaveHdrFailureReason() { return codecSelector.pyroWaveHdrFailure; }

    @Override
    public int prepareVideoFormats(int formats, int width, int height, int fps) {
        int commonFormats = formats;
        codecSelector.pyroWaveDropReason = 0;
        formats = gatePackedHdr(formats, width, height, fps);
        float displayRefreshRate = outputDisplay.getRefreshRate();
        boolean adaptive;
        synchronized (vsyncMonitor) {
            adaptive = adaptiveInUse();
        }
        int pyroWavePacing = FramePacingPolicy.pyroWaveMode(prefs.framePacing, prefs.vrr, prefs.hostFrameTimes, adaptive);
        float topRefreshRate = pyroWavePacing == FramePacingPolicy.PYROWAVE_HOST_TIMES ? maxDisplayRefreshRate() : 0;
        codecSelector.pyroWaveHdrFailure = 0;
        int choice = firstPyroWaveFormat(formats, candidate -> {
            if (stopping) return false;
            boolean ready = pyroWaveRenderer.setup(renderTarget, candidate, width, height, fps, displayRefreshRate,
                    topRefreshRate, pyroWavePacing, prefs.fullRange, prefs.stretchVideo,
                    prefs.upscalingMode, prefs.upscalingSharpness, context, prefs.gpuMaxClocks,
                    prefs.pyroWavePreacquire) && !stopping;
            if (!ready) LimeLog.warning("PyroWave surface not created for format " + Integer.toHexString(candidate));
            return ready;
        });
        return codecSelector.finishPrepareVideoFormats(commonFormats, formats, choice);
    }

    private int gatePackedHdr(int formats, int width, int height, int fps) { return codecSelector.gatePackedHdr(formats, width, height, fps); }
    static String hostTenBitIssue(int deviceModes, int hostModes) { return NightjarPolicyJni.videoHostTenBitIssue(deviceModes, hostModes); }
    static String hostTenBitDrop(boolean tenBitOffered, String dropAtLaunch, int negotiatedFormat, int hostModes) { return NightjarPolicyJni.videoHostTenBitDrop(tenBitOffered, dropAtLaunch, negotiatedFormat, hostModes); }
    @Override public void setHostPacked444Modes(int modes) { codecSelector.hostPacked444Modes = modes; }
    @Override public void setPacked444(boolean packed) { prefs.experimentalPacked444 = packed; }
    @Override public void setNative444(boolean native444) { codecSelector.native444Picked = native444; }
    @Override public String packedHdrDropReason() { return codecSelector.packedHdrDrop; }
    @Override public int preparePacked444Modes(int formats, int width, int height, int fps) { return codecSelector.preparePacked444Modes(formats, width, height, fps); }
    static String tenBitOutputIssue(boolean displayHdr10, boolean gpuTenBitOutput) { return NightjarPolicyJni.videoTenBitOutputIssue(displayHdr10, gpuTenBitOutput); }
    int probePacked444Modes(int formats, int width, int height, int fps) { return codecSelector.probePacked444Modes(formats, width, height, fps); }
    public void setSdr10Requested(boolean sdr10) { codecSelector.sdr10 = sdr10; }
    public static boolean vulkanPresents(PreferenceConfiguration prefs, boolean frameWarpListed) { return MediaCodecCodecSelector.vulkanPresents(prefs, frameWarpListed); }
    static boolean frameWarpAsked(PreferenceConfiguration prefs, boolean frameWarpListed) { return NightjarPolicyJni.videoRendererFrameWarpAsked(prefs.frameWarp, frameWarpListed); }
    public static HdrFormatPolicy.DrawnSurface vulkanSurface(PreferenceConfiguration prefs, boolean frameWarpListed) { return MediaCodecCodecSelector.vulkanSurface(prefs, frameWarpListed); }
    @Override public int pickHdrFormat(int hostHdrFormats, int videoFormats, boolean packed444, boolean frameWarpListed) { return codecSelector.pickHdrFormat(hostHdrFormats, videoFormats, packed444, frameWarpListed); }
    @Override public int videoFormatsForHdrFormat(int hdrFormat, int videoFormats) { return codecSelector.videoFormatsForHdrFormat(hdrFormat, videoFormats); }
    // pickHdrFormat chose 8.1 with full range on only where the PC and the decoder take it.
    @Override public int hdrFormatRequest(int hdrFormat) {
        return prefs.fullRange && hdrFormat == HdrFormatPolicy.DV_81 ?
                hdrFormat | HdrFormatPolicy.DV_81_FULL_RANGE : hdrFormat;
    }
    @Override public void setAnnouncedHdrFormat(int format) { codecSelector.setAnnouncedHdrFormat(format); }
    public int getAnnouncedHdrFormat() { return announcedHdrFormat; }
    public int getStreamTransfer() { return HdrFormatPolicy.transfer(announcedHdrFormat); }
    private MediaCodecInfo dolbyVisionDecoderForStream() { return codecSelector.dolbyVisionDecoderForStream(); }
    public Boolean getHostHdrReport() { return currentHdrMode; }
    public HdrPolicy.Metadata getHdrMetadataState() { return HdrPolicy.metadata(currentHdrMetadata); }
    public boolean isVulkanHdrToneMapped() { return vulkanHelper.isVulkanHdrToneMapped(); }
    public boolean isVulkanHdrOutput() { return vulkanHelper.isVulkanHdrOutput(); }
    public boolean takeVulkanHdr10PlusRefused() { return vulkanHelper.takeVulkanHdr10PlusRefused(); }
    public boolean takeVulkanBehind() { return vulkanHelper.takeVulkanBehind(); }
    public HdrFormatPolicy.DrawnLimit takeVulkanHdrLimit() { return vulkanHelper.takeVulkanHdrLimit(); }
    public boolean vulkanForFrameWarp() { return vulkanHelper.vulkanForFrameWarp; }
    public HdrFormatPolicy.DrawnLimit takePyroWaveHdrLimit() { return codecSelector.takePyroWaveHdrLimit(); }
    public boolean isPyroWaveHlgNeedingHostUpdate() { return codecSelector.isPyroWaveHlgNeedingHostUpdate(); }
    public boolean isPyroWaveHlgRefusedByDisplay() { return codecSelector.isPyroWaveHlgRefusedByDisplay(); }
    public boolean isPyroWaveHlgLost() { return codecSelector.isPyroWaveHlgLost(); }
    public Integer getDecodedTransfer() { return codecSelector.getDecodedTransfer(); }
    @Override public int setChromaPacking(int version, int width, int height, int colorSpace, int colorRange) { return codecSelector.setChromaPacking(version, width, height, colorSpace, colorRange); }
    public int getPreferredColorSpace() { return codecSelector.getPreferredColorSpace(); }
    // A Dolby Vision ANNOUNCE asks for limited range (nightjar-core's sdp), its base layer included,
    // unless it asked for 8.1 full range.
    public int getPreferredColorRange() {
        return prefs.fullRange && (dolbyVisionFullRange || (announcedHdrFormat & HdrFormatPolicy.DOLBY_VISION) == 0) ?
                MoonBridge.COLOR_RANGE_FULL : MoonBridge.COLOR_RANGE_LIMITED;
    }
    public int getActiveVideoFormat() { return this.videoFormat; }

    static int maxInputSize(int bitrateKbps, int fps, int width, int height) { return NightjarPolicyJni.videoMaxInputSize(bitrateKbps, fps, width, height); }
    // The input buffer size to ask for when a frame of requiredBytes does not fit: twice the buffer,
    // or the frame if larger. 0 when growing cannot help: the frame fits, or the decoder already
    // ignored a request this large. From Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0).
    static int grownInputSize(long requiredBytes, int capacity, int previousRequest) { return NightjarPolicyJni.videoGrownInputSize(requiredBytes, capacity, previousRequest); }
    // Asks for a restart with a larger input buffer, the way Moonlight V+ does, instead of ending
    // the stream on the first frame that does not fit.
    boolean requestInputBufferGrowth(long requiredBytes, int capacity) { return session.requestInputBufferGrowth(requiredBytes, capacity); }
    static void applyRequiredInputSize(MediaFormat format, int requiredInputBufferSize) { MediaCodecSession.applyRequiredInputSize(format, requiredInputBufferSize); }
    private MediaFormat createBaseMediaFormat(String mimeType) { return session.createBaseMediaFormat(mimeType); }
    static void applyHdrStaticInfo(MediaFormat format, ByteBuffer hdrStaticInfo) { MediaCodecHdrControl.applyHdrStaticInfo(format, hdrStaticInfo); }
    static ByteBuffer createHdrStaticInfo(byte[] metadata) { return MediaCodecHdrControl.createHdrStaticInfo(metadata); }
    static void applyConfigureColor(MediaFormat format, boolean hdr, int colorSpace, byte[] hdrMetadata) { applyConfigureColor(format, hdr, colorSpace, hdrMetadata, MediaFormat.COLOR_TRANSFER_ST2084); }
    static void applyConfigureColor(MediaFormat format, boolean hdr, int colorSpace, byte[] hdrMetadata, int hdrTransfer) { MediaCodecHdrControl.applyConfigureColor(format, hdr, colorSpace, hdrMetadata, hdrTransfer); }
    private void configureAndStartDecoder(MediaFormat format) { session.configureAndStartDecoder(format); }
    private boolean tryConfigureDecoder(MediaCodecInfo selectedDecoderInfo, MediaFormat format, boolean throwOnCodecError) { return session.tryConfigureDecoder(selectedDecoderInfo, format, throwOnCodecError); }
    static boolean lowLatencyOptions(MediaFormat format, MediaCodecInfo decoder, int tryNumber, PreferenceConfiguration prefs, boolean vendorLowLatency) { return MediaCodecSession.lowLatencyOptions(format, decoder, tryNumber, prefs, vendorLowLatency); }
    public DecoderSetupFailure getSetupFailure() { return session.setupFailure; }

    public int initializeDecoder(boolean throwOnCodecError) {
        MediaCodecSession.CodecTarget target = session.resolveCodecTarget();
        if (target.error != 0) return target.error;
        String mimeType = target.mimeType;
        MediaCodecInfo selectedDecoderInfo = target.decoderInfo;
        boolean dolbyVision = HdrFormatPolicy.MIME_DOLBY_VISION.equals(mimeType);
        int[] streamMax = {maxDecodeWidth, maxDecodeHeight};
        boolean coveringApplied = false;
        if (adaptivePlayback && coveringSize != null && !coveringRefused && !wantVulkan && vulkanRenderer == null &&
                packing == null && (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0 &&
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            coveringApplied = session.applyCoveringMax(selectedDecoderInfo, mimeType, coveringSize);
        }
        boolean retryWithoutUpscaling = false;
        List<String> firstTryKeys = null;
        boolean vendorLowLatency = session.prepareVendorLowLatency(selectedDecoderInfo, mimeType);
        for (int tryNumber = 0;; tryNumber++) {
            LimeLog.info("Decoder configuration try: " + tryNumber);
            MediaFormat mediaFormat = createBaseMediaFormat(mimeType);
            boolean newFormat = lowLatencyOptions(mediaFormat, selectedDecoderInfo, tryNumber, prefs, vendorLowLatency);
            firstTryKeys = session.recordTryKeys(mediaFormat, selectedDecoderInfo, mimeType, tryNumber, vendorLowLatency, firstTryKeys);
            if (tryConfigureDecoder(selectedDecoderInfo, mediaFormat,
                    !newFormat && upscaler == null && throwOnCodecError && !dolbyVision && !coveringApplied)) {
                session.onConfigureTrySucceeded(selectedDecoderInfo, mimeType, mediaFormat, tryNumber, retryWithoutUpscaling, dolbyVision);
                break;
            }
            if (!newFormat) {
                if (coveringApplied) {
                    LimeLog.warning("Decoder " + selectedDecoderInfo.getName() + " refused every try with the " +
                            maxDecodeWidth + "x" + maxDecodeHeight + " covering maximum; retrying at " +
                            streamMax[0] + "x" + streamMax[1]);
                    maxDecodeWidth = streamMax[0];
                    maxDecodeHeight = streamMax[1];
                    coveringApplied = false;
                    coveringRefused = true;
                    tryNumber = -1;
                    continue;
                }
                if (dolbyVision) {
                    dolbyVisionFailed = true;
                    LimeLog.warning("Dolby Vision decoder " + selectedDecoderInfo.getName() +
                            " refused the stream; decoding it as " + codecSelector.dolbyVisionBaseCodec() + " with its " +
                            HdrFormatPolicy.baseLayerName(announcedHdrFormat) + " base layer");
                    return initializeDecoder(throwOnCodecError);
                }
                if (upscaler != null && packing == null) {
                    LimeLog.warning("Retrying decoder formats without the upscaling surface");
                    vulkanHelper.closeUpscaler();
                    retryWithoutUpscaling = true;
                    tryNumber = -1;
                    continue;
                }
                return session.recordSetupRejection(selectedDecoderInfo, mimeType);
            }
        }
        return 0;
    }

    private void setFrameRenderedListener() { session.setFrameRenderedListener(); }

    @Override
    public int setup(int format, int width, int height, int redrawRate) {
        int result;
        synchronized (codecRecoveryMonitor) {
            if (stopping) return -1;
            session.beforeSetupLocked(format, width, height, redrawRate);
            this.submitHints = prefs.phonePerformanceHints && (format & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0 &&
                    MediaCodecHelper.debugSwitch(DecoderPerformanceHints.SUBMIT_SWITCH, prefs.submitHint);
            LimeLog.info("Speed up frame hand-off: " + (submitHints ? "on" : "off"));
            if ((format & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                setupUpscaling();
                result = pyroWaveRenderer.getFormat() == format ? 0 : -1;
                if (result == 0 && vulkanHelper.frameWarp && !pyroWaveRenderer.setFrameWarp(MoonBridge.frameWarpApi())) {
                    vulkanHelper.declineFrameWarp(R.string.frame_warp_reason_renderer);
                }
                if (result == 0) pyroWaveRenderer.setTransfer(getStreamTransfer() == MediaFormat.COLOR_TRANSFER_HLG);
                if (result == 0 && currentHdrMode != null) pyroWaveRenderer.setHdrMode(currentHdrMode, currentHdrMetadata);
                if (result == 0) telemetry.updateDecoderDiagnostics();
            } else {
                pyroWaveRenderer.cleanup();
                setupUpscaling();
                result = initializeDecoder(false);
            }
        }
        session.finishSetup(format, width, height, result);
        return result;
    }

    private void openLatencyCsv() throws IOException { telemetry.openLatencyCsv(); }
    private void flushLatencyCsv() { telemetry.flushLatencyCsv(); }
    private boolean closeLatencyCsv() { return telemetry.closeLatencyCsv(); }
    private boolean doCodecRecoveryIfRequired(int quiescenceFlag) { return recovery.doCodecRecoveryIfRequired(quiescenceFlag); }
    private boolean handleDecoderException(IllegalStateException e) { return recovery.handleDecoderException(e); }
    private void recordVendorKeyHang() { recovery.recordVendorKeyHang(); }

    static long nextVsyncTimeNs(long nowNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) { return MediaCodecVsyncPacer.nextVsyncTimeNs(nowNs, vsyncNs, intervalNs, presentationDeadlineNs); }
    static long latchSlackNs(long readyNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) { return MediaCodecVsyncPacer.latchSlackNs(readyNs, vsyncNs, intervalNs, presentationDeadlineNs); }
    public boolean applyOutputPacingFallback() { return vsyncPacer.applyOutputPacingFallback(); }
    private void reportPhaseSlack(long readyNs) { vsyncPacer.reportPhaseSlack(readyNs); }
    private void updateDisplayTiming() { vsyncPacer.updateDisplayTiming(); }
    @Override public void doFrame(long frameTimeNanos) { vsyncPacer.doFrame(frameTimeNanos); }
    void onFrameTimeline(long frameTimeNanos, long expectedPresentNs, long deadlineNs) { vsyncPacer.onFrameTimeline(frameTimeNanos, expectedPresentNs, deadlineNs); }
    private boolean adaptiveInUse() { return vsyncPacer.adaptiveInUse(); }
    private int heldRefreshes() { return vsyncPacer.heldRefreshes(); }
    private long heldDisplayNs(long releaseNs) { return vsyncPacer.heldDisplayNs(releaseNs); }
    private long smoothDisplayNs(long releaseNs) { return vsyncPacer.smoothDisplayNs(releaseNs); }
    private long planRefreshSlot(long releaseNs) { return vsyncPacer.planRefreshSlot(releaseNs); }

    private void onOutputFormatChanged() { framePresenter.onOutputFormatChanged(); }
    private void noteDecoderOutput(int index, long ptsUs, long outputNs) { framePresenter.noteDecoderOutput(index, ptsUs, outputNs); }
    private int releaseNewestOutput(int outIndex, long ptsUs, BufferInfo info, long outputDrainStartNs) { return framePresenter.releaseNewestOutput(outIndex, ptsUs, info, outputDrainStartNs); }
    private boolean fetchNextInputBuffer() { return decodeLoop.fetchNextInputBuffer(); }
    private void holdForInputSpacing() { decodeLoop.holdForInputSpacing(); }
    private void noteVulkanHdr10Plus(VulkanRendererBridge vulkan, long timestampUs, ByteBuffer direct, byte[] data, int length) { vulkanHelper.noteVulkanHdr10Plus(vulkan, timestampUs, direct, data, length); }

    private void startRendererThread() {
        rendererThread = new Thread() {
            @Override
            public void run() {
                Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY);
                try (DecoderPerformanceHints performanceHints =
                             new DecoderPerformanceHints(context, prefs.phonePerformanceHints, refreshRate)) {
                    boolean hintsActive = performanceHints.isActive();
                    boolean cpuTime = hintsActive && prefs.phonePerformanceHints &&
                            MediaCodecHelper.debugSwitch(DecoderPerformanceHints.CPU_TIME_SWITCH, true);
                    BufferInfo info = new BufferInfo();
                    while (!stopping) {
                        long workStartNs = 0, workCpuStartNs = 0;
                        try {
                            int outIndex = videoDecoder.dequeueOutputBuffer(info, 50000);
                            if (outIndex >= 0) {
                                workStartNs = nowNs();
                                if (cpuTime) workCpuStartNs = Debug.threadCpuTimeNanos();
                                long outputDrainStartNs = elapsedNs();
                                outIndex = framePresenter.handleDecodedOutput(outIndex, workStartNs, info, outputDrainStartNs);
                            }
                            if (outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) onOutputFormatChanged();
                        } catch (IllegalStateException e) {
                            handleDecoderException(e);
                        } finally {
                            if (workStartNs != 0 && hintsActive) {
                                long workNs = cpuTime ? Debug.threadCpuTimeNanos() - workCpuStartNs :
                                        System.nanoTime() - workStartNs;
                                if (submitHints) {
                                    performanceHints.setSubmitThreads(submitWork.threadId(), submitWork.secondThreadId());
                                    workNs += submitWork.take();
                                }
                                performanceHints.reportWorkDuration(workNs, refreshRate);
                            }
                            vulkanHelper.checkUpscaling();
                            doCodecRecoveryIfRequired(CR_FLAG_RENDER_THREAD);
                        }
                    }
                }
            }
        };
        rendererThread.setName("Video - Renderer (MediaCodec)");
        rendererThread.start();
    }

    @Override
    public void start() {
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            session.startPyroWave();
            return;
        }
        session.prepareMediaCodecStart();
        hostFrameTimes = prefs.hostFrameTimes;
        boolean trackContentRate = prefs.contentRateFrameRate && hostAnnouncesClockSync();
        session.trackContentRate = trackContentRate;
        hostPhaseLock = MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_PHASE_LOCK);
        refreshSlotsEnabled = MediaCodecHelper.debugSwitch("refresh_slots", true);
        boolean adaptive;
        synchronized (vsyncMonitor) {
            adaptive = adaptiveInUse();
        }
        hostFrameClock.start(hostFrameTimes ? maxDisplayRefreshRate() : 0, hostFrameTimes,
                hostAnnouncesClockSync(), trackContentRate || hostFrameTimes, hostFrameTimes && adaptive);
        startRendererThread();
        if (vulkanRenderer == null) vsyncPacer.startChoreographerThread();
    }

    private static boolean hostAnnouncesClockSync() { return MediaCodecSession.hostAnnouncesClockSync(); }
    private float maxDisplayRefreshRate() { return session.maxDisplayRefreshRate(); }
    public void prepareForStop() { session.prepareForStop(); }
    @Override public void stop() { session.stop(); }
    @Override public void cleanup() { session.cleanup(); }
    @Override public void setHdrMode(boolean enabled, byte[] hdrMetadata) { MediaCodecHdrControl.setHdrMode(this, enabled, hdrMetadata); }
    static HdrChange hdrChange(boolean previousMode, boolean enabled, byte[] previousMetadata, byte[] metadata, int sdkInt) { return HdrChange.values()[NightjarPolicyJni.videoHdrChange(previousMode, enabled, previousMetadata, metadata, sdkInt)]; }
    static boolean restartRecreates(String decoderName, List<String> tuningKeys) { return MediaCodecHdrControl.restartRecreates(decoderName, tuningKeys); }
    static boolean decoderCarriesHdrMetadata(boolean vulkanPresents) { return NightjarPolicyJni.videoDecoderCarriesHdrMetadata(vulkanPresents); }
    private void checkRunningHdrStaticInfo() { MediaCodecHdrControl.checkRunningHdrStaticInfo(this); }
    static boolean carriesHdrStaticInfo(ByteBuffer carried, byte[] expected) { return MediaCodecHdrControl.carriesHdrStaticInfo(carried, expected); }
    private void requestRestart(String reason) { session.requestRestart(reason); }

    void enableSubmitHandoff() { decodeLoop.enableSubmitHandoff(); }

    int submitHanded(MediaCodecDecodeLoop.HandedUnit u) {
        return submitInPlace(u.data, u.length, u.type, u.frameNumber, u.frameType, u.hostProcessingLatency,
                u.receiveTimeUs, u.enqueueTimeUs, u.receiveTimeNs, u.presentationTimeUs);
    }

    public int submitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                int frameNumber, int frameType, char frameHostProcessingLatency,
                                long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                long presentationTimeUs) {
        SubmitHandoff<MediaCodecDecodeLoop.HandedUnit> handoff = submitHandoff;
        if (handoff == null || stopping) {
            return submitInPlace(decodeUnitData, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        }
        int frames = decodeUnitType == MoonBridge.BUFFER_TYPE_PICDATA ? 1 : 0;
        boolean keyUnit = frameType == MoonBridge.FRAME_TYPE_IDR;
        int gate = handoff.tryGateInPlace(frames, keyUnit, frameNumber);
        if (gate <= 0) return gate;
        if (gate == SubmitHandoff.GATE_IN_PLACE && decodeLoop.inputReadyForReceiveThread(frameType, decodeUnitType)) {
            return submitInPlace(decodeUnitData, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        }
        return decodeLoop.enqueueForHandoff(handoff, decodeUnitData, decodeUnitLength, decodeUnitType,
                frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs,
                receiveTimeNs, presentationTimeUs, frames, keyUnit);
    }

    private int submitInPlace(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                              int frameNumber, int frameType, char frameHostProcessingLatency,
                              long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                              long presentationTimeUs) {
        if (!submitHints) {
            return queueDecodeUnit(decodeUnitData, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        }
        long cpuStartNs = Debug.threadCpuTimeNanos();
        try {
            return queueDecodeUnit(decodeUnitData, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        } finally {
            submitWork.add(Process.myTid(), Debug.threadCpuTimeNanos() - cpuStartNs);
        }
    }

    @SuppressWarnings("deprecation")
    private int queueDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                int frameNumber, int frameType, char frameHostProcessingLatency,
                                long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                long presentationTimeUs) {
        if (stopping) return MoonBridge.DR_OK;
        long enqueueTimeNs = receiveTimeNs + (enqueueTimeUs - receiveTimeUs) * 1000;
        int pre = decodeLoop.beforeDecodeUnit(decodeUnitLength, frameNumber, frameType,
                frameHostProcessingLatency, receiveTimeUs, receiveTimeNs, presentationTimeUs);
        if (pre != 0) return pre == 1 ? MoonBridge.DR_OK : MoonBridge.DR_NEED_IDR;
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            return submitPyroWaveFrame(decodeUnitData, decodeUnitLength, frameNumber, frameHostProcessingLatency,
                    enqueueTimeNs, receiveTimeNs, presentationTimeUs);
        }
        if (dolbyVisionActive && decodeUnitType == MoonBridge.BUFFER_TYPE_PICDATA && HdrBitstream.endsAtRpu(videoFormat) &&
                !(decodeUnitData == null ? HdrBitstream.endsInRpu(directInput, decodeUnitLength) :
                        HdrBitstream.endsInRpu(decodeUnitData, decodeUnitLength))) {
            return recovery.leaveDolbyVision();
        }
        int csd = decodeLoop.handleIdrCsd(decodeUnitData, decodeUnitLength, decodeUnitType, frameType);
        if (csd == 1) return MoonBridge.DR_OK;
        if (csd == 2) return MoonBridge.DR_NEED_IDR;

        recordHostProcessing(activeWindowVideoStats, frameHostProcessingLatency);
        long timestampUs = decodeLoop.prepareInputFrame(decodeUnitLength, frameType, receiveTimeNs, enqueueTimeUs);
        if (timestampUs == Long.MIN_VALUE) return MoonBridge.DR_NEED_IDR;

        ByteBuffer nextInputBuffer = decodeLoop.nextInputBuffer;
        boolean dolbyVision = dolbyVisionActive && HdrBitstream.endsAtRpu(videoFormat);
        String vividDecoder = vividSeiDropDecoder;
        boolean hdr10PlusParse = vulkanHdr10Plus && Boolean.TRUE.equals(currentHdrMode);
        ByteBuffer direct = decodeUnitData == null ? directInput : null;
        int queuedLength = direct != null ? HdrBitstream.queuedLength(dolbyVision, direct, decodeUnitLength) :
                HdrBitstream.queuedLength(dolbyVision, decodeUnitData, decodeUnitLength);
        if (dolbyVision && !decodeLoop.dolbyVisionPaddingLogged) {
            decodeLoop.dolbyVisionPaddingLogged = true;
            LimeLog.info("Dolby Vision: the first frame ends " + (direct != null ?
                    HdrBitstream.tail(direct, queuedLength, 8) : HdrBitstream.tail(decodeUnitData, queuedLength, 8)) +
                    ", dropped " + (decodeUnitLength - queuedLength) + " zero bytes after it");
        }
        int[] vividRange = vividDecoder != null && (direct != null ?
                HdrBitstream.findVividSei(direct, queuedLength, decodeLoop.vividSeiRange) :
                HdrBitstream.findVividSei(decodeUnitData, queuedLength, decodeLoop.vividSeiRange)) ? decodeLoop.vividSeiRange : null;
        decodeLoop.logVividDropIfNeeded(vividDecoder, vividRange);
        int putLength = HdrBitstream.putLength(queuedLength, vividRange);
        if (!decodeLoop.ensureInputCapacity(putLength)) return MoonBridge.DR_NEED_IDR;
        if (direct != null) {
            HdrBitstream.put(nextInputBuffer, direct, queuedLength, vividRange);
        } else {
            HdrBitstream.put(nextInputBuffer, decodeUnitData, queuedLength, vividRange);
        }
        if (vulkanHelper.frameWarp) MoonBridge.frameWarpNoteFrame(timestampUs, frameNumber);
        VulkanRendererBridge vulkan = vulkanRenderer;
        if (vulkan != null) {
            vulkan.noteFrameReceived(timestampUs, receiveTimeUs, enqueueTimeUs, receiveTimeNs, frameHostProcessingLatency);
            if (hdr10PlusParse) {
                noteVulkanHdr10Plus(vulkan, timestampUs, direct != null ? direct : null, decodeUnitData, decodeUnitLength);
            }
        }
        holdForInputSpacing();
        return decodeLoop.finishQueueDecodeUnit(timestampUs, decodeUnitLength, frameNumber, frameType,
                frameHostProcessingLatency, receiveTimeNs, enqueueTimeNs, presentationTimeUs);
    }

    @Override
    public int submitDirectDecodeUnit(ByteBuffer decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                      int frameNumber, int frameType, char frameHostProcessingLatency,
                                      long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                      long presentationTimeUs) {
        if (decodeUnitType != MoonBridge.BUFFER_TYPE_PICDATA) {
            return super.submitDirectDecodeUnit(decodeUnitData, decodeUnitLength, decodeUnitType, frameNumber,
                    frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        }
        directInput = decodeUnitData;
        try {
            return submitDecodeUnit(null, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        } finally {
            directInput = null;
        }
    }

    public int submitRawDecodeUnit(long dataPtr, int decodeUnitLength, int decodeUnitType,
                                   int frameNumber, int frameType, char frameHostProcessingLatency,
                                   long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                   long presentationTimeUs) {
        decodeLoop.rawInputPtr = dataPtr;
        try {
            return submitDecodeUnit(null, decodeUnitLength, decodeUnitType, frameNumber, frameType,
                    frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs);
        } finally {
            decodeLoop.rawInputPtr = 0;
        }
    }

    @Override
    public void submitChunks(byte[] data, int length, boolean first) {
        if (!stopping && (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) pyroWaveRenderer.submitChunks(data, length, first);
    }

    @Override
    public void submitChunksDirect(ByteBuffer data, int length, boolean first) {
        if (!stopping && (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) pyroWaveRenderer.submitChunksDirect(data, length, first);
    }

    public void submitChunksRaw(long dataPtr, int length, boolean first) {
        if (!stopping && (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) pyroWaveRenderer.submitChunksRaw(dataPtr, length, first);
    }

    private int submitPyroWaveFrame(byte[] decodeUnitData, int decodeUnitLength, int frameNumber,
                                    char frameHostProcessingLatency, long enqueueTimeNs, long receiveTimeNs,
                                    long presentationTimeUs) {
        recordHostProcessing(activeWindowVideoStats, frameHostProcessingLatency);
        long ptsUs = decodeLoop.preparePyroWaveFrame(frameNumber, frameHostProcessingLatency,
                enqueueTimeNs, receiveTimeNs, presentationTimeUs);
        if (ptsUs < 0) return MoonBridge.DR_OK;
        PyroWaveDisplayClock.Snapshot displayClock =
                FramePacingPolicy.usesPyroWaveDisplayClock(prefs.framePacing, prefs.vrr, outputVsyncAvailable) ?
                        pyroWaveDisplayClock.snapshot() : PyroWaveDisplayClock.Snapshot.UNAVAILABLE;
        long submissionResult = decodeLoop.rawInputPtr != 0 ?
                pyroWaveRenderer.submitRawFrame(decodeLoop.rawInputPtr, decodeUnitLength, ptsUs, frameNumber,
                        presentationTimeUs, displayClock, frameLatencyStats, context, prefs.phonePerformanceHints, refreshRate) :
                pyroWaveRenderer.submitFrame(decodeUnitData, directInput, decodeUnitLength, ptsUs,
                frameNumber, presentationTimeUs, displayClock, frameLatencyStats,
                context, prefs.phonePerformanceHints, refreshRate);
        decodeLoop.finishPyroWaveFrame(ptsUs, submissionResult);
        return MoonBridge.DR_OK;
    }

    @Override public int getCapabilities() { return codecSelector.capabilitiesFor(0); }
    @Override public int getCapabilities(int negotiatedFormat) { return codecSelector.capabilitiesFor(negotiatedFormat); }
    @Override public int getCapabilities(int negotiatedFormat, int hdrFormat) { return codecSelector.getCapabilities(negotiatedFormat, hdrFormat); }
    MediaCodecInfo decoderForNegotiated(int formats) { return codecSelector.decoderForNegotiated(formats); }

    static class DecoderHungException extends RuntimeException {
        private final int hangTimeMs;

        DecoderHungException(int hangTimeMs) {
            this.hangTimeMs = hangTimeMs;
        }

        public String toString() {
            return "Hang time: " + hangTimeMs + " ms" + RendererException.DELIMITER + super.toString();
        }
    }

    static class RendererException extends RuntimeException {
        private static final long serialVersionUID = 8985937536997012406L;
        protected static final String DELIMITER = BuildConfig.DEBUG ? "\n" : " | ";
        private final String text;

        RendererException(MediaCodecDecoderRenderer renderer, Exception e) {
            super(e);
            this.text = MediaCodecOverlayReport.formatRendererException(renderer, e, DELIMITER);
        }

        // Logs and crash reporters read getMessage() and the cause, not toString() (from V+ fefc9db).
        @Override public String getMessage() { return text; }
        public String toString() { return text; }
    }
}
