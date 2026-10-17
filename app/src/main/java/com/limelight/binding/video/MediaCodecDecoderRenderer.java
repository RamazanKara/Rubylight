package com.limelight.binding.video;

import java.io.BufferedWriter;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

import org.jcodec.codecs.h264.H264Utils;
import org.jcodec.codecs.h264.io.model.SeqParameterSet;
import org.jcodec.codecs.h264.io.model.VUIParameters;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.ProblemReport;

import android.annotation.TargetApi;
import android.app.Activity;
import android.content.Context;
import android.hardware.display.DisplayManager;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.media.MediaCodec.BufferInfo;
import android.media.MediaCodec.CodecException;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.util.Range;
import android.view.Choreographer;
import android.view.Display;
import android.view.Surface;
import android.widget.Toast;

public class MediaCodecDecoderRenderer extends VideoDecoderRenderer implements Choreographer.FrameCallback {

    private MediaCodecInfo avcDecoder;
    private MediaCodecInfo hevcDecoder;
    private MediaCodecInfo av1Decoder;

    private final ArrayList<byte[]> vpsBuffers = new ArrayList<>();
    private final ArrayList<byte[]> spsBuffers = new ArrayList<>();
    private final ArrayList<byte[]> ppsBuffers = new ArrayList<>();
    private boolean submittedCsd;
    private byte[] currentHdrMetadata;
    private Boolean currentHdrMode;

    private int nextInputBufferIndex = -1;
    private ByteBuffer nextInputBuffer;

    private Context context;
    private Activity activity;
    private MediaCodec videoDecoder;
    private Thread rendererThread;
    private boolean needsSpsBitstreamFixup, isExynos4;
    private boolean adaptivePlayback, directSubmit, fusedIdrFrame;
    private boolean constrainedHighProfile;
    private boolean refFrameInvalidationAvc, refFrameInvalidationHevc, refFrameInvalidationAv1;
    private byte optimalSlicesPerFrame;
    private boolean refFrameInvalidationActive;
    private int initialWidth, initialHeight;
    // Mid-stream reconfigure (0x5532): the adaptive-playback maximum, which grows when a larger
    // size is asked for; the size the host was last asked for; and the size being shown.
    private int maxDecodeWidth, maxDecodeHeight;
    private volatile boolean reconfigureRequested;
    private volatile int expectedWidth, expectedHeight;
    private volatile int streamWidth, streamHeight;
    private volatile StreamSizeListener streamSizeListener;
    private int videoFormat;
    private Surface renderTarget;
    private int renderWidth, renderHeight;
    private volatile GlesUpscaler upscaler;
    private volatile UpscalingPolicy upscalingPolicy;
    private volatile boolean stopping;
    private CrashListener crashListener;
    private boolean reportedCrash;
    private int consecutiveCrashCount;
    private String glRenderer;
    private boolean foreground = true;
    private PerfOverlayListener perfListener;

    private static final int CR_MAX_TRIES = 10;
    private static final int CR_RECOVERY_TYPE_NONE = 0;
    private static final int CR_RECOVERY_TYPE_FLUSH = 1;
    private static final int CR_RECOVERY_TYPE_RESTART = 2;
    private static final int CR_RECOVERY_TYPE_RESET = 3;
    private AtomicInteger codecRecoveryType = new AtomicInteger(CR_RECOVERY_TYPE_NONE);
    private final Object codecRecoveryMonitor = new Object();

    // Each thread that touches the MediaCodec object or any associated buffers must have a flag
    // here and must call doCodecRecoveryIfRequired() on a regular basis.
    private static final int CR_FLAG_INPUT_THREAD = 0x1;
    private static final int CR_FLAG_RENDER_THREAD = 0x2;
    private static final int CR_FLAG_CHOREOGRAPHER = 0x4;
    private static final int CR_FLAG_ALL = CR_FLAG_INPUT_THREAD | CR_FLAG_RENDER_THREAD | CR_FLAG_CHOREOGRAPHER;
    private int codecRecoveryThreadQuiescedFlags = 0;
    private int codecRecoveryAttempts = 0;

    private MediaFormat inputFormat;
    private MediaFormat outputFormat;
    private MediaFormat configuredFormat;

    private boolean needsBaselineSpsHack;
    private SeqParameterSet savedSps;

    private RendererException initialException;
    private long initialExceptionTimestamp;
    private static final int EXCEPTION_REPORT_DELAY_MS = 3000;

    private VideoStats activeWindowVideoStats;
    private VideoStats lastWindowVideoStats;
    private VideoStats globalVideoStats;
    private volatile VideoStatsFps overlayFps;
    private volatile long lastVideoFrameTimeMs;
    private volatile float networkFrameLossPercent;
    private final AtomicLong receivedVideoBytes = new AtomicLong();
    private long bitrateWindowStartNs;
    private long bitrateWindowStartBytes;
    private volatile float receivedVideoMbps = -1;

    private final PyroWaveDecoderRenderer pyroWaveRenderer = new PyroWaveDecoderRenderer();
    private int pyroWaveFailures;
    private float pyroWaveLossTotal;
    private int pyroWaveLossSamples;
    private float pyroWaveQueueDelayTotal;
    private int pyroWaveQueueSamples;
    // Frames skipped because a newer one was already waiting, and frames decoded, per stats window.
    private int pyroWaveSkippedFrames;
    private int pyroWaveDecodedFrames;
    private volatile float pyroWaveSkippedPercent = -1;
    private volatile float pyroWaveLossPercent = -1;
    private volatile float pyroWaveQueueDelayMs = -1;
    private final QueueDelayTracker queueDelay = new QueueDelayTracker();
    private volatile float queueDelayMs = -1;
    // Precise decode time: drives adaptive bitrate and quality, the stream doctor and the health dot.
    private volatile float decodeTimeMs = -1;
    // Conventional decode follows Artemis; PyroWave follows Moonlight X's CPU submission metric.
    // Both use every frame received in the last two one-second windows as the denominator.
    private volatile float shownDecodeTimeMs = -1;
    private final PyroWaveSubmissionTiming pyroWaveSubmissionTiming = new PyroWaveSubmissionTiming();
    private ThermalMonitor thermalMonitor;
    private volatile int thermalLevel = ThermalMonitor.NORMAL;

    private final FrameLatencyStats frameLatencyStats;
    private HandlerThread latencyThread;
    private Handler latencyHandler;
    private final AtomicBoolean phaseFlushQueued = new AtomicBoolean();
    private final Runnable flushPhaseLockReport = new Runnable() {
        @Override
        public void run() {
            // While a send waits for ENet, allow only one further flush to queue.
            // Native storage keeps the latest report, so old feedback cannot pile up.
            phaseFlushQueued.set(false);
            if (!stopping) MoonBridge.flushPhaseLockReport();
        }
    };
    private BufferedWriter latencyCsv;
    private volatile String latencyOverlay = "";
    private volatile double[][] latencySummary = new double[5][4];
    private volatile String decoderDiagnostics = "";
    // Private file name kept from earlier releases so the last stream's timings survive updates.
    public static final String LATENCY_CSV_NAME = "butterpollo-latency.csv";
    public static final String LATENCY_CSV_EXPORT_NAME = "nightjar-latency.csv";
    private static final long MAX_LATENCY_CSV_BYTES = 8 * 1024 * 1024;
    private final Runnable updateLatencyStats = new Runnable() {
        @Override
        public void run() {
            long nowNs = System.nanoTime();
            long bytes = receivedVideoBytes.get();
            if (bitrateWindowStartNs != 0) {
                receivedVideoMbps = PerformanceOverlay.videoMbps(bytes - bitrateWindowStartBytes, nowNs - bitrateWindowStartNs);
            }
            bitrateWindowStartNs = nowNs;
            bitrateWindowStartBytes = bytes;
            thermalLevel = thermalMonitor.level();
            // Forced max clocks on a hot phone only bring on harder throttling.
            if (thermalLevel == ThermalMonitor.HOT && pyroWaveRenderer.hasMaxClocks()) {
                pyroWaveRenderer.restoreClocks();
                updateDecoderDiagnostics();
            }
            pyroWaveRenderer.pollRenderedFrames(frameLatencyStats);
            frameLatencyStats.expire(System.nanoTime());
            flushLatencyCsv();
            if (prefs.enablePerfOverlay) {
                latencySummary = frameLatencyStats.summarize();
                StringBuilder text = new StringBuilder(formatLatencyOverlay(context, latencySummary, shownDecodeTimeMs,
                        (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0));
                if (latencyCsv == null) {
                    text.append('\n').append(context.getString(R.string.latency_csv_failed));
                } else if (frameLatencyStats.getCsvRowsLost() != 0) {
                    text.append('\n').append(context.getString(R.string.latency_csv_lost, frameLatencyStats.getCsvRowsLost()));
                }
                latencyOverlay = text.toString();
                updatePerformanceOverlay();
            }
            latencyHandler.postDelayed(this, 1000);
        }
    };

    @Override
    public void onHostTiming(HostTimingReport report) {
        frameLatencyStats.onHostTiming(report, System.nanoTime());
    }

    public static String formatLatencyOverlay(Context context, double[][] summary) {
        return formatLatencyOverlay(context, summary, -1);
    }

    /** decodeAverageMs, when known, replaces the decode row's average so it matches the decode time shown elsewhere. */
    public static String formatLatencyOverlay(Context context, double[][] summary, float decodeAverageMs) {
        return formatLatencyOverlay(context, summary, decodeAverageMs, false);
    }

    static String formatLatencyOverlay(Context context, double[][] summary, float decodeAverageMs, boolean pyroWave) {
        int[] labels = { R.string.latency_receive_render, R.string.latency_queue_wait,
                pyroWave ? R.string.latency_pyrowave_completed_decode : R.string.latency_input_output, R.string.latency_output_render,
                R.string.latency_host_processing };
        int[] stages = {3, 0, 1, 2, 4};
        StringBuilder text = new StringBuilder(context.getString(R.string.latency_header, FrameLatencyStats.WINDOW_SIZE));
        for (int line = 0; line < labels.length; line++) {
            int stage = stages[line];
            if (stage == 4) {
                text.append("\n\n").append(context.getString(R.string.overlay_host));
            }
            text.append('\n').append(context.getString(labels[line])).append(": ");
            if (summary[stage][0] == 0) {
                text.append(context.getString(R.string.latency_unavailable));
            } else {
                // Submission time has a different endpoint and denominator. Never combine it
                // with completed-decode percentiles in the same row.
                double average = stage == 1 && !pyroWave && decodeAverageMs >= 0 ? decodeAverageMs : summary[stage][1];
                text.append(context.getString(R.string.latency_values, average,
                        summary[stage][2], summary[stage][3], (int) summary[stage][0]));
            }
        }
        // Only a host that sends timing reports has the queue, encode and send split.
        if (summary.length > 7 && summary[5][0] > 0 && summary[6][0] > 0 && summary[7][0] > 0) {
            text.append('\n').append(context.getString(R.string.latency_host_split,
                    summary[5][1], summary[6][1], summary[7][1]));
        }
        return text.toString();
    }

    public boolean hasRecentVideoFrames(long nowMs) {
        return lastVideoFrameTimeMs != 0 && nowMs - lastVideoFrameTimeMs < 2000;
    }

    public float getNetworkFrameLossPercent() {
        return networkFrameLossPercent;
    }

    public float getPyroWaveLossPercent() {
        return Math.max(networkFrameLossPercent, pyroWaveLossPercent);
    }

    public float getPyroWaveQueueDelayMs() {
        return pyroWaveQueueDelayMs;
    }

    public float getDecodeTimeMs() {
        return decodeTimeMs;
    }

    /** A bounded snapshot for the stream doctor, including unavailable stage values. */
    public com.limelight.utils.StreamHealth.Sample healthSample(long nowMs, int fps, float jitterMs) {
        long nowNs = System.nanoTime();
        double[][] timing = frameLatencyStats.summarizeRecent(nowNs, 2_000_000_000L);
        float[] rates = frameLatencyStats.getFrameRates(nowNs);
        return new com.limelight.utils.StreamHealth.Sample(nowMs, hasRecentVideoFrames(nowMs), fps,
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ? getPyroWaveLossPercent() : getNetworkFrameLossPercent(),
                jitterMs, getDecodeTimeMs(), timing[4][0] > 0 ? (float) timing[4][2] : -1,
                timing[2][0] > 0 ? (float) timing[2][2] : -1, rates[0], rates[1], rates[2], getThermalLevel());
    }

    public LatencyProbe getLatencyProbe() {
        return frameLatencyStats.latencyProbe;
    }

    /** ThermalMonitor level from the last stats window. */
    public int getThermalLevel() {
        return thermalLevel;
    }

    private String thermalStats() {
        int state = thermalLevel == ThermalMonitor.HOT ? R.string.perf_overlay_thermal_hot :
                thermalLevel == ThermalMonitor.WARM ? R.string.perf_overlay_thermal_warm : R.string.perf_overlay_thermal_normal;
        float headroom = thermalMonitor.headroom();
        return context.getString(R.string.perf_overlay_thermal, context.getString(state)) +
                (headroom >= 0 ? String.format(Locale.US, " (%.2f)", headroom) : "");
    }

    // Network queueing delay above its recent minimum, ms; -1 when unknown.
    public float getQueueDelayMs() {
        return queueDelayMs;
    }

    static String formatPyroWaveStats(float recordLoss, float queueMs, int gpuUs) {
        return "PyroWave records missing: " + metric(recordLoss, "%") +
                "\nQueue: " + metric(queueMs, " ms") +
                (gpuUs > 0 ? " | GPU decode (last): " + metric(gpuUs / 1000.0f, " ms") : "");
    }

    private static String metric(float value, String unit) {
        return value >= 0 && Float.isFinite(value) ?
                String.format(java.util.Locale.getDefault(), "%.2f%s", value, unit) : "unavailable";
    }

    private long lastTimestampUs;
    private int lastFrameNumber;
    private long baseTimestampUs;
    private volatile int refreshRate;
    private volatile int streamFrameRateMilliHz;
    private PreferenceConfiguration prefs;

    // Its monitor makes replacing a full queue atomic with the display thread's poll.
    // The queue's own thread safety alone does not cover size/poll/add together.
    private final LinkedBlockingQueue<Integer> outputBufferQueue = new LinkedBlockingQueue<>();
    private int outputBufferQueueLimit = 1;
    // Why the pending restart was requested. Problem reports keep warnings and "Decoder" lines,
    // so they show HDR, size and upscaler restarts as such instead of as a CodecException.
    private volatile String restartReason = "CodecException";
    private long nextFrameTimeNanos;
    private int pacingFrameRateMilliHz;
    private HandlerThread choreographerHandlerThread;
    private Handler choreographerHandler;
    private final Object vsyncMonitor = new Object();
    private final PyroWaveDisplayClock pyroWaveDisplayClock = new PyroWaveDisplayClock();
    private long vsyncTimeNs;
    private long vsyncIntervalNs;
    private long vsyncPresentationDeadlineNs;
    private long appVsyncOffsetNs;
    private DisplayManager displayManager;
    private Display streamDisplay;
    private final DisplayManager.DisplayListener displayListener = new DisplayManager.DisplayListener() {
        @Override public void onDisplayAdded(int displayId) { }
        @Override public void onDisplayRemoved(int displayId) { }
        @Override public void onDisplayChanged(int displayId) {
            if (streamDisplay.getDisplayId() == displayId) updateDisplayTiming();
        }
    };
    // Guarded by vsyncMonitor together with the timing sample for the active mode.
    private boolean outputHasArrSupport;
    private final Display outputDisplay;
    private final boolean outputVsyncAvailable;

    private int numSpsIn;
    private int numPpsIn;
    private int numVpsIn;
    private int numFramesIn;
    private int numFramesOut;

    private MediaCodecInfo findAvcDecoder() {
        MediaCodecInfo decoder = prefs.enableYuv444 ?
                MediaCodecHelper.findProbableSafeDecoder("video/avc", MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444) : null;
        if (decoder == null) {
            decoder = MediaCodecHelper.findProbableSafeDecoder("video/avc", MediaCodecInfo.CodecProfileLevel.AVCProfileHigh);
        }
        if (decoder == null) {
            decoder = MediaCodecHelper.findFirstDecoder("video/avc");
        }
        return decoder;
    }

    private boolean decoderCanMeetPerformancePoint(MediaCodecInfo.VideoCapabilities caps, PreferenceConfiguration prefs) {
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

    private boolean decoderCanMeetPerformancePointWithHevcAndNotAvc(MediaCodecInfo hevcDecoderInfo, MediaCodecInfo avcDecoderInfo, PreferenceConfiguration prefs) {
        MediaCodecInfo.VideoCapabilities avcCaps = avcDecoderInfo.getCapabilitiesForType("video/avc").getVideoCapabilities();
        MediaCodecInfo.VideoCapabilities hevcCaps = hevcDecoderInfo.getCapabilitiesForType("video/hevc").getVideoCapabilities();

        return !decoderCanMeetPerformancePoint(avcCaps, prefs) && decoderCanMeetPerformancePoint(hevcCaps, prefs);
    }

    private boolean decoderCanMeetPerformancePointWithAv1AndNotHevc(MediaCodecInfo av1DecoderInfo, MediaCodecInfo hevcDecoderInfo, PreferenceConfiguration prefs) {
        MediaCodecInfo.VideoCapabilities av1Caps = av1DecoderInfo.getCapabilitiesForType("video/av01").getVideoCapabilities();
        MediaCodecInfo.VideoCapabilities hevcCaps = hevcDecoderInfo.getCapabilitiesForType("video/hevc").getVideoCapabilities();

        return !decoderCanMeetPerformancePoint(hevcCaps, prefs) && decoderCanMeetPerformancePoint(av1Caps, prefs);
    }

    private boolean decoderCanMeetPerformancePointWithAv1AndNotAvc(MediaCodecInfo av1DecoderInfo, MediaCodecInfo avcDecoderInfo, PreferenceConfiguration prefs) {
        MediaCodecInfo.VideoCapabilities avcCaps = avcDecoderInfo.getCapabilitiesForType("video/avc").getVideoCapabilities();
        MediaCodecInfo.VideoCapabilities av1Caps = av1DecoderInfo.getCapabilitiesForType("video/av01").getVideoCapabilities();

        return !decoderCanMeetPerformancePoint(avcCaps, prefs) && decoderCanMeetPerformancePoint(av1Caps, prefs);
    }

    private MediaCodecInfo findHevcDecoder(PreferenceConfiguration prefs, boolean meteredNetwork, boolean requestedHdr) {
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
                else if (requestedHdr) {
                    LimeLog.info("Forcing HEVC enabled for HDR streaming");
                }
                else if ((getDecoderVideoFormats(hevcDecoderInfo, "video/hevc", false, prefs.enableYuv444) &
                        MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0) {
                    LimeLog.info("Using HEVC for requested 4:4:4 streaming");
                }
                // > 4K streaming also requires HEVC, so force it on there too.
                else if (prefs.width > 4096 || prefs.height > 4096) {
                    LimeLog.info("Forcing HEVC enabled for over 4K streaming");
                }
                // Use HEVC if the H.264 decoder is unable to meet the performance point
                else if (avcDecoder != null && decoderCanMeetPerformancePointWithHevcAndNotAvc(hevcDecoderInfo, avcDecoder, prefs)) {
                    LimeLog.info("Using non-whitelisted HEVC decoder to meet performance point");
                }
                else {
                    return null;
                }
            }
        }

        return hevcDecoderInfo;
    }

    // Automatic: leave out codecs this phone measurably decodes slower than another (DecoderBenchmark).
    private void skipMeasuredSlowerCodecs(PreferenceConfiguration prefs, boolean requestedHdr) {
        long[] us = {DecoderBenchmark.result(context, avcDecoder, "video/avc"),
                DecoderBenchmark.result(context, hevcDecoder, "video/hevc"),
                DecoderBenchmark.result(context, av1Decoder, "video/av01")};
        boolean[] skip = DecoderBenchmark.slower(us, requestedHdr);
        // 4:4:4 and over-4K streams need HEVC whatever its speed.
        boolean needsHevc = prefs.enableYuv444 || prefs.width > 4096 || prefs.height > 4096;
        if (skip[1] && hevcDecoder != null && !needsHevc) {
            LimeLog.info("Skipping HEVC: measured " + us[1] + " us/frame on this phone");
            hevcDecoder = null;
        }
        if (skip[2] && av1Decoder != null) {
            LimeLog.info("Skipping AV1: measured " + us[2] + " us/frame on this phone");
            av1Decoder = null;
        }
    }

    private MediaCodecInfo findAv1Decoder(PreferenceConfiguration prefs, boolean requestedHdr) {
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
        if (decoderInfo == null) {
            decoderInfo = MediaCodecHelper.findProbableSafeDecoder("video/av01", -1);
        }
        if (decoderInfo != null) {
            if (!MediaCodecHelper.isDecoderWhitelistedForAv1(decoderInfo)) {
                LimeLog.info("Found AV1 decoder, but it's not whitelisted - "+decoderInfo.getName());

                // Force HEVC enabled if the user asked for it
                if (prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1 || requestedHdr) {
                    LimeLog.info("Forcing AV1 enabled despite non-whitelisted decoder");
                }
                // Use AV1 if the HEVC decoder is unable to meet the performance point
                else if (hevcDecoder != null && decoderCanMeetPerformancePointWithAv1AndNotHevc(decoderInfo, hevcDecoder, prefs)) {
                    LimeLog.info("Using non-whitelisted AV1 decoder to meet performance point");
                }
                // Use AV1 if the H.264 decoder is unable to meet the performance point and we have no HEVC decoder
                else if (hevcDecoder == null && avcDecoder != null && decoderCanMeetPerformancePointWithAv1AndNotAvc(decoderInfo, avcDecoder, prefs)) {
                    LimeLog.info("Using non-whitelisted AV1 decoder to meet performance point");
                }
                else {
                    return null;
                }
            }
        }

        return decoderInfo;
    }

    public interface StreamSizeListener {
        // Called on the renderer thread when frames of a new size come out of the decoder after
        // a mid-stream reconfigure.
        void onStreamSizeChanged(int width, int height);

        // Called on the renderer thread with each output format: the decoded picture and the part
        // of it that is the stream. They differ when the decoder leaves encoder padding uncropped.
        default void onDecodedSize(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {
        }
    }

    public void setStreamSizeListener(StreamSizeListener listener) {
        streamSizeListener = listener;
    }

    // The size of the frames being shown: the negotiated size until a reconfigure changes it.
    public int[] getStreamSize() {
        return new int[] {streamWidth, streamHeight};
    }

    // A decoder that can change size mid-stream, or null: PyroWave (its Vulkan path sizes its
    // planes once), decoders without adaptive playback, and Android 7.1 and older, whose H.264
    // SPS fixups assume the first size.
    private MediaCodecInfo.VideoCapabilities reconfigureCapabilities() {
        if (stopping || videoDecoder == null || configuredFormat == null || !adaptivePlayback ||
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ||
                Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return null;
        }
        try {
            return videoDecoder.getCodecInfo()
                    .getCapabilitiesForType(configuredFormat.getString(MediaFormat.KEY_MIME))
                    .getVideoCapabilities();
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot read decoder sizes: " + e);
            return null;
        }
    }

    // {max width, max height, width alignment, height alignment} this stream's decoder can switch
    // to mid-stream, or null when it cannot change size in place.
    public int[] getReconfigureLimits() {
        synchronized (codecRecoveryMonitor) {
            MediaCodecInfo.VideoCapabilities caps = reconfigureCapabilities();
            if (caps == null) {
                return null;
            }
            int maxWidth = caps.getSupportedWidths().getUpper();
            int maxHeight = caps.getSupportedHeights().getUpper();
            if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
                // initializeDecoder() refuses H.264 above 4096 either way.
                maxWidth = Math.min(maxWidth, 4096);
                maxHeight = Math.min(maxHeight, 4096);
            }
            return new int[] {maxWidth, maxHeight, caps.getWidthAlignment(), caps.getHeightAlignment()};
        }
    }

    // Gets the decoder ready for an IDR of width x height before the host is asked for one.
    // Adaptive playback takes any size up to the configured maximum without a pause. A larger size
    // raises the maximum to cover both sizes, which restarts the decoder once and asks for an IDR;
    // either size can then arrive first. False when this decoder cannot take the size.
    public boolean prepareForReconfigure(int width, int height) {
        synchronized (codecRecoveryMonitor) {
            MediaCodecInfo.VideoCapabilities caps = reconfigureCapabilities();
            if (caps == null) {
                return false;
            }
            int[] max = StreamResizePolicy.coveringMax(maxDecodeWidth, maxDecodeHeight, width, height);
            if (!caps.isSizeSupported(width, height) || !caps.isSizeSupported(max[0], max[1])) {
                LimeLog.warning("Decoder cannot switch to " + width + "x" + height + " within " +
                        max[0] + "x" + max[1]);
                return false;
            }
            boolean restart = false;
            if (max[0] != maxDecodeWidth || max[1] != maxDecodeHeight) {
                LimeLog.info("Decoder restarts for sizes up to " + max[0] + "x" + max[1] +
                        " (was " + maxDecodeWidth + "x" + maxDecodeHeight + ")");
                maxDecodeWidth = max[0];
                maxDecodeHeight = max[1];
                configuredFormat.setInteger(MediaFormat.KEY_MAX_WIDTH, maxDecodeWidth);
                configuredFormat.setInteger(MediaFormat.KEY_MAX_HEIGHT, maxDecodeHeight);
                restart = true;
            }
            if (upscaler != null && (width != initialWidth || height != initialHeight)) {
                // The GLES upscaler is built for the first size; continue on direct output.
                upscalingPolicy.fail(UpscalingPolicy.Reason.SIZE_CHANGED);
                restart = true;
            }
            if (restart) {
                // An expected "recovery", like an HDR change.
                codecRecoveryAttempts = 0;
                requestRestart("size change");
            }
            expectedWidth = width;
            expectedHeight = height;
            reconfigureRequested = true;
            return true;
        }
    }

    public void setFrameRate(int fps) {
        setStreamFrameRateMilliHz(fps * 1000);
    }

    /** Exact stream cadence; rounded FPS remains the MediaCodec/performance-hint setting. */
    @Override
    public void setStreamFrameRateMilliHz(int fpsMilliHz) {
        FramePacingPolicy.frameIntervalNs(fpsMilliHz); // Validate before publishing to the render threads.
        synchronized (codecRecoveryMonitor) {
            if (stopping || fpsMilliHz == streamFrameRateMilliHz) return;
            streamFrameRateMilliHz = fpsMilliHz;
            frameLatencyStats.resetDecodeTiming();
            int fps = (fpsMilliHz + 500) / 1000;
            refreshRate = fps;
            if (configuredFormat != null) configuredFormat.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
            Display display = outputDisplay;
            if (display != null) {
                updateUpscalingFrameRate(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                        display.getMode().getRefreshRate() : display.getRefreshRate());
            }
        }
    }

    public int getStreamFrameRateMilliHz() {
        return streamFrameRateMilliHz;
    }

    public com.limelight.nvstream.DecodeTiming takeDecodeTiming(int fps) {
        return frameLatencyStats.takeDecodeTiming(System.nanoTime(), fps);
    }

    public void resetDecodeTiming() {
        frameLatencyStats.resetDecodeTiming();
    }

    private void updateUpscalingFrameRate(float panelHz) {
        synchronized (codecRecoveryMonitor) {
            GlesUpscaler current = upscaler;
            if (current != null) current.setFrameRate(Math.max(1, Math.min(refreshRate, Math.round(panelHz))));
        }
    }

    // Logs the size each decoder output format carries and, after a reconfigure, reports a new
    // size once its frames come out.
    private void updateShownSize() {
        int[] visible;
        try {
            if (!outputFormat.containsKey(MediaFormat.KEY_WIDTH) || !outputFormat.containsKey(MediaFormat.KEY_HEIGHT)) {
                return;
            }
            visible = StreamResizePolicy.visibleSize(outputFormat.getInteger(MediaFormat.KEY_WIDTH),
                    outputFormat.getInteger(MediaFormat.KEY_HEIGHT),
                    formatInteger(outputFormat, "crop-left"), formatInteger(outputFormat, "crop-right"),
                    formatInteger(outputFormat, "crop-top"), formatInteger(outputFormat, "crop-bottom"));
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot read decoded size: " + e);
            return;
        }
        int[] shown = StreamResizePolicy.shownSize(visible[0], visible[1], expectedWidth, expectedHeight);
        LimeLog.info("Decoded size: " + visible[0] + "x" + visible[1] + ", shown as " + shown[0] + "x" + shown[1]);
        StreamSizeListener sizeListener = streamSizeListener;
        if (sizeListener != null) {
            sizeListener.onDecodedSize(visible[0], visible[1], shown[0], shown[1]);
        }
        if (!reconfigureRequested || (shown[0] == streamWidth && shown[1] == streamHeight)) {
            return;
        }
        LimeLog.info("Stream size changed from " + streamWidth + "x" + streamHeight + " to " +
                shown[0] + "x" + shown[1] + " (asked for " + expectedWidth + "x" + expectedHeight + ")");
        streamWidth = shown[0];
        streamHeight = shown[1];
        StreamSizeListener listener = streamSizeListener;
        if (listener != null) {
            listener.onStreamSizeChanged(shown[0], shown[1]);
        }
    }

    private static int formatInteger(MediaFormat format, String key) {
        return format.containsKey(key) ? format.getInteger(key) : -1;
    }

    public void setRenderTarget(Surface renderTarget, int width, int height) {
        this.renderTarget = renderTarget;
        renderWidth = width;
        renderHeight = height;
    }

    private void setupUpscaling() {
        Display display = outputDisplay;
        float panelHz = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate();
        upscalingPolicy = new UpscalingPolicy(Math.max(1, Math.min(refreshRate, Math.round(panelHz))));
        if (prefs.upscalingMode == UpscalingPolicy.Mode.OFF) {
            upscalingPolicy.fail(UpscalingPolicy.Reason.DISABLED);
            return;
        }
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // The Vulkan renderer runs SGSR itself; the GLES upscaler only serves MediaCodec.
            upscalingPolicy.fail(UpscalingPolicy.Reason.PYROWAVE);
            return;
        }
        int[] viewport = UpscalingPolicy.viewport(initialWidth, initialHeight, renderWidth, renderHeight, prefs.stretchVideo);
        UpscalingPolicy.Reason reason = UpscalingPolicy.unavailableReason(prefs.upscalingMode,
                initialWidth, initialHeight, viewport[2], viewport[3],
                Boolean.TRUE.equals(currentHdrMode) || (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0,
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0, true);
        upscalingPolicy.fail(reason);
        if (reason == UpscalingPolicy.Reason.NONE) {
            upscaler = new GlesUpscaler(context, renderTarget, prefs.upscalingMode, initialWidth, initialHeight,
                    prefs.stretchVideo, prefs.upscalingSharpness, upscalingPolicy);
            if (upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) closeUpscaler();
        } else {
            notifyUpscalingFallback();
        }
    }

    private void closeUpscaler() {
        if (upscaler != null) {
            upscaler.close();
            upscaler = null;
            notifyUpscalingFallback();
        }
    }

    private void notifyUpscalingFallback() {
        UpscalingPolicy.Reason reason = upscalingPolicy.getReason();
        if (reason != UpscalingPolicy.Reason.NONE && reason != UpscalingPolicy.Reason.DISABLED &&
                reason != UpscalingPolicy.Reason.NO_UPSCALE) {
            String message = context.getString(R.string.upscaling_fallback, upscalingReason(reason));
            LimeLog.info(message);
            activity.runOnUiThread(() -> {
                if (!stopping) Toast.makeText(context, message, Toast.LENGTH_LONG).show();
            });
        }
    }

    private String upscalingReason(UpscalingPolicy.Reason reason) {
        switch (reason) {
            case HDR: return context.getString(R.string.upscaling_hdr);
            case PYROWAVE: return context.getString(R.string.upscaling_pyrowave);
            case NO_UPSCALE: return context.getString(R.string.upscaling_native);
            case UNSUPPORTED: return context.getString(R.string.upscaling_unsupported);
            case GPU_ERROR: return context.getString(R.string.upscaling_gpu_error);
            case SLOW: return context.getString(R.string.upscaling_slow);
            case DROPPED: return context.getString(R.string.upscaling_dropped);
            case SIZE_CHANGED: return context.getString(R.string.upscaling_size_changed);
            default: return "";
        }
    }

    private String upscalingStats() {
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            UpscalingPolicy.Reason reason = pyroWaveRenderer.getUpscalingReason();
            String mode = reason == UpscalingPolicy.Reason.NONE ?
                    (prefs.upscalingMode == UpscalingPolicy.Mode.BILINEAR ?
                            context.getString(R.string.upscaling_bilinear) : "SGSR (Vulkan)") :
                    context.getString(R.string.upscaling_direct);
            String text = context.getString(R.string.perf_overlay_upscaling_pyrowave, mode);
            String explanation = upscalingReason(reason);
            return explanation.isEmpty() ? text : text + '\n' + explanation;
        }
        boolean active = upscaler != null;
        String mode = active ? context.getResources().getStringArray(R.array.upscaling_names)[prefs.upscalingMode.ordinal()] :
                context.getString(R.string.upscaling_direct);
        float addedMs = upscalingPolicy == null ? -1 : upscalingPolicy.getAddedMs();
        String text = context.getString(R.string.perf_overlay_upscaling, mode, metric(active ? addedMs : 0, " ms"));
        if (active) text += '\n' + context.getString(R.string.upscaling_timing_estimate);
        else if (addedMs >= 0) text += '\n' + context.getString(R.string.upscaling_last_timing, metric(addedMs, " ms"));
        if (upscalingPolicy != null) {
            String reason = upscalingReason(upscalingPolicy.getReason());
            if (!reason.isEmpty()) text += '\n' + reason;
        }
        return text;
    }

    private void checkUpscaling() {
        GlesUpscaler current = upscaler;
        if (current == null || stopping) return;
        current.checkHealth();
        if (upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) {
            synchronized (codecRecoveryMonitor) {
                if (stopping) return;
                codecRecoveryAttempts = 0;
                requestRestart("upscaler failure");
            }
        }
    }

    private void releaseOutputFrame(int index, long presentationTimeNs) {
        GlesUpscaler current = upscaler;
        if (current != null && upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) {
            videoDecoder.releaseOutputBuffer(index, false);
        } else {
            videoDecoder.releaseOutputBuffer(index, current == null ? presentationTimeNs : current.releaseFrame(presentationTimeNs));
        }
    }

    private void checkUpscalingOutputFormat() {
        if (upscaler == null) return;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && outputFormat.containsKey(MediaFormat.KEY_COLOR_TRANSFER)) {
            int transfer = outputFormat.getInteger(MediaFormat.KEY_COLOR_TRANSFER);
            if (transfer == MediaFormat.COLOR_TRANSFER_ST2084 || transfer == MediaFormat.COLOR_TRANSFER_HLG) {
                upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
            }
        }
        if (outputFormat.containsKey(MediaFormat.KEY_COLOR_FORMAT) &&
                outputFormat.getInteger(MediaFormat.KEY_COLOR_FORMAT) == MediaCodecInfo.CodecCapabilities.COLOR_FormatYUVP010) {
            upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
        }
        int width = outputFormat.getInteger(MediaFormat.KEY_WIDTH);
        int height = outputFormat.getInteger(MediaFormat.KEY_HEIGHT);
        if (outputFormat.containsKey("crop-right") && outputFormat.containsKey("crop-left")) {
            width = outputFormat.getInteger("crop-right") - outputFormat.getInteger("crop-left") + 1;
        }
        if (outputFormat.containsKey("crop-bottom") && outputFormat.containsKey("crop-top")) {
            height = outputFormat.getInteger("crop-bottom") - outputFormat.getInteger("crop-top") + 1;
        }
        if (width != initialWidth || height != initialHeight) upscalingPolicy.fail(UpscalingPolicy.Reason.SIZE_CHANGED);
    }

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
        //dumpDecoders();

        this.context = activity;
        this.thermalMonitor = new ThermalMonitor(activity);
        this.activity = activity;
        this.outputDisplay = outputDisplay;
        // The public Choreographer API receives internal-display callbacks, even
        // when a Presentation and its decoder surface are on another display.
        this.outputVsyncAvailable = outputDisplay.getDisplayId() == Display.DEFAULT_DISPLAY;
        this.prefs = prefs;
        this.frameLatencyStats = new FrameLatencyStats(prefs.vrr);
        this.outputHasArrSupport = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA &&
                outputDisplay.hasArrSupport();
        this.crashListener = crashListener;
        this.consecutiveCrashCount = consecutiveCrashCount;
        this.glRenderer = glRenderer;
        this.perfListener = perfListener;

        this.activeWindowVideoStats = new VideoStats();
        this.lastWindowVideoStats = new VideoStats();
        this.globalVideoStats = new VideoStats();

        avcDecoder = findAvcDecoder();
        if (avcDecoder != null) {
            LimeLog.info("Selected AVC decoder: "+avcDecoder.getName());
        }
        else {
            LimeLog.warning("No AVC decoder found");
        }

        hevcDecoder = findHevcDecoder(prefs, meteredData, requestedHdr);
        if (hevcDecoder != null) {
            LimeLog.info("Selected HEVC decoder: "+hevcDecoder.getName());
        }
        else {
            LimeLog.info("No HEVC decoder found");
        }

        av1Decoder = findAv1Decoder(prefs, requestedHdr);
        if (av1Decoder != null) {
            LimeLog.info("Selected AV1 decoder: "+av1Decoder.getName());
        }
        else {
            LimeLog.info("No AV1 decoder found");
        }

        if (prefs.videoFormat == PreferenceConfiguration.FormatOption.AUTO) {
            skipMeasuredSlowerCodecs(prefs, requestedHdr);
        }

        // Set attributes that are queried in getCapabilities(). This must be done here
        // because getCapabilities() may be called before setup() in current versions of the common
        // library. The limitation of this is that we don't know whether we're using HEVC or AVC.
        int avcOptimalSlicesPerFrame = 0;
        int hevcOptimalSlicesPerFrame = 0;
        if (avcDecoder != null) {
            directSubmit = MediaCodecHelper.decoderCanDirectSubmit(avcDecoder.getName());
            refFrameInvalidationAvc = MediaCodecHelper.decoderSupportsRefFrameInvalidationAvc(avcDecoder.getName(), prefs.height);
            avcOptimalSlicesPerFrame = MediaCodecHelper.getDecoderOptimalSlicesPerFrame(avcDecoder.getName());

            if (directSubmit) {
                LimeLog.info("Decoder "+avcDecoder.getName()+" will use direct submit");
            }
            if (refFrameInvalidationAvc) {
                LimeLog.info("Decoder "+avcDecoder.getName()+" will use reference frame invalidation for AVC");
            }
            LimeLog.info("Decoder "+avcDecoder.getName()+" wants "+avcOptimalSlicesPerFrame+" slices per frame");
        }

        if (hevcDecoder != null) {
            refFrameInvalidationHevc = MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(hevcDecoder);
            hevcOptimalSlicesPerFrame = MediaCodecHelper.getDecoderOptimalSlicesPerFrame(hevcDecoder.getName());

            if (refFrameInvalidationHevc) {
                LimeLog.info("Decoder "+hevcDecoder.getName()+" will use reference frame invalidation for HEVC");
            }

            LimeLog.info("Decoder "+hevcDecoder.getName()+" wants "+hevcOptimalSlicesPerFrame+" slices per frame");
        }

        if (av1Decoder != null) {
            refFrameInvalidationAv1 = MediaCodecHelper.decoderSupportsRefFrameInvalidationAv1(av1Decoder);

            if (refFrameInvalidationAv1) {
                LimeLog.info("Decoder "+av1Decoder.getName()+" will use reference frame invalidation for AV1");
            }
        }

        // Use the larger of the two slices per frame preferences
        optimalSlicesPerFrame = (byte)Math.max(avcOptimalSlicesPerFrame, hevcOptimalSlicesPerFrame);
        LimeLog.info("Requesting "+optimalSlicesPerFrame+" slices per frame");

        if (consecutiveCrashCount % 2 == 1) {
            refFrameInvalidationAvc = refFrameInvalidationHevc = refFrameInvalidationAv1 = false;
            LimeLog.warning("Disabling RFI due to previous crash");
        }
    }

    public boolean isHevcSupported() {
        return hevcDecoder != null;
    }

    public boolean isAvcSupported() {
        return avcDecoder != null;
    }

    public boolean isHevcMain10Hdr10Supported() {
        return (getDecoderVideoFormats(hevcDecoder, "video/hevc", true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    public boolean isAv1Supported() {
        return av1Decoder != null;
    }

    public boolean isAv1Main10Supported() {
        return (getDecoderVideoFormats(av1Decoder, "video/av01", true, false) & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    static int videoFormatsForProfiles(String mime, int[] profiles, boolean hdr, boolean yuv444) {
        int formats = mime.equals("video/avc") ? MoonBridge.VIDEO_FORMAT_H264 :
                mime.equals("video/hevc") ? MoonBridge.VIDEO_FORMAT_H265 : MoonBridge.VIDEO_FORMAT_AV1_MAIN8;
        for (int profile : profiles) {
            if (mime.equals("video/avc")) {
                if (yuv444 && profile == MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444) {
                    formats |= MoonBridge.VIDEO_FORMAT_H264_HIGH8_444;
                }
            } else if (mime.equals("video/hevc")) {
                if (hdr && (profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10 ||
                        profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10Plus)) {
                    formats |= MoonBridge.VIDEO_FORMAT_H265_MAIN10;
                }
                // API 37 adds this 8-bit profile. Older devices never advertise it.
                if (yuv444 && profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444) {
                    formats |= MoonBridge.VIDEO_FORMAT_H265_REXT8_444;
                }
            } else if (hdr && (profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10 ||
                    profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10Plus)) {
                formats |= MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
            }
        }
        return formats;
    }

    private int getDecoderVideoFormats(MediaCodecInfo decoder, String mime, boolean hdr, boolean yuv444) {
        if (decoder == null) {
            return 0;
        }
        int baseFormat = videoFormatsForProfiles(mime, new int[0], false, false);
        if (!hdr && !yuv444) {
            return baseFormat;
        }
        try {
            MediaCodecInfo.CodecCapabilities caps = decoder.getCapabilitiesForType(mime);
            int[] profiles = new int[caps.profileLevels.length];
            for (int i = 0; i < profiles.length; i++) {
                profiles[i] = caps.profileLevels[i].profile;
            }
            return videoFormatsForProfiles(mime, profiles, hdr, yuv444 &&
                    caps.getVideoCapabilities().areSizeAndRateSupported(prefs.width, prefs.height, prefs.fps));
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot query decoder profiles; using SDR 4:2:0: " + e);
            return baseFormat;
        }
    }

    public boolean isPyroWaveSupported() {
        return prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE &&
                !prefs.useTextureView && PyroWaveDecoderRenderer.isAvailable(context);
    }

    public int getSupportedVideoFormats(boolean hdr) {
        int pyroWave = 0;
        if (isPyroWaveSupported()) {
            pyroWave = MoonBridge.VIDEO_FORMAT_PYROWAVE;
            if (prefs.enableYuv444) pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_444;
            if (hdr) {
                pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10;
                if (prefs.enableYuv444) pyroWave |= MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10_444;
            }
        }
        return pyroWave | MoonBridge.VIDEO_FORMAT_H264 |
                getDecoderVideoFormats(avcDecoder, "video/avc", false, prefs.enableYuv444) |
                getDecoderVideoFormats(hevcDecoder, "video/hevc", hdr, prefs.enableYuv444) |
                getDecoderVideoFormats(av1Decoder, "video/av01", hdr, false);
    }

    @Override
    public int prepareVideoFormats(int formats, int width, int height, int fps) {
        float displayRefreshRate = outputDisplay.getRefreshRate();
        int[] choices = { MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10_444, MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10,
                MoonBridge.VIDEO_FORMAT_PYROWAVE_444, MoonBridge.VIDEO_FORMAT_PYROWAVE };
        for (int choice : choices) {
            if ((formats & choice) != 0 && !stopping &&
                    pyroWaveRenderer.setup(renderTarget, choice, width, height, fps, displayRefreshRate,
                            FramePacingPolicy.pyroWaveMode(prefs.framePacing, prefs.vrr), prefs.fullRange, prefs.stretchVideo,
                            prefs.upscalingMode, prefs.upscalingSharpness, context, prefs.gpuMaxClocks) && !stopping) {
                LimeLog.info("PyroWave surface initialized before negotiation: " + Integer.toHexString(choice));
                return (formats & ~MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) | choice;
            }
        }
        pyroWaveRenderer.cleanup();
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            LimeLog.warning("PyroWave initialization failed; retaining MediaCodec fallback");
        }
        return formats & ~MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE;
    }

    public int getPreferredColorSpace() {
        // Default to Rec 709 which is probably better supported on modern devices.
        //
        // We are sticking to Rec 601 on older devices unless the device has an HEVC decoder
        // to avoid possible regressions (and they are < 5% of installed devices). If we have
        // an HEVC decoder, we will use Rec 709 (even for H.264) since we can't choose a
        // colorspace by codec (and it's probably safe to say a SoC with HEVC decoding is
        // plenty modern enough to handle H.264 VUI colorspace info).
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O || hevcDecoder != null || av1Decoder != null) {
            return MoonBridge.COLORSPACE_REC_709;
        }
        else {
            return MoonBridge.COLORSPACE_REC_601;
        }
    }

    public int getPreferredColorRange() {
        if (prefs.fullRange) {
            return MoonBridge.COLOR_RANGE_FULL;
        }
        else {
            return MoonBridge.COLOR_RANGE_LIMITED;
        }
    }

    public void notifyVideoForeground() {
        foreground = true;
    }

    public void notifyVideoBackground() {
        foreground = false;
    }

    public int getActiveVideoFormat() {
        return this.videoFormat;
    }

    // Decoders size their input buffers for ordinary video, and a frame that doesn't fit ends the
    // stream. Above about 125 Mbps at 60 FPS, ask for room for four average frames instead.
    static int maxInputSize(int bitrateKbps, int fps) {
        if (bitrateKbps <= 0 || fps <= 0) {
            return 0;
        }
        long bytes = (long) bitrateKbps * 1000 / 8 / fps * 4;
        if (bytes <= 2 * 1024 * 1024) {
            return 0;
        }
        return (int) Math.min(bytes, 32 * 1024 * 1024);
    }

    private MediaFormat createBaseMediaFormat(String mimeType) {
        MediaFormat videoFormat = MediaFormat.createVideoFormat(mimeType, initialWidth, initialHeight);
        if (this.videoFormat == MoonBridge.VIDEO_FORMAT_H264_HIGH8_444) {
            videoFormat.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileHigh444);
        } else if (this.videoFormat == MoonBridge.VIDEO_FORMAT_H265_REXT8_444) {
            videoFormat.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444);
        }

        // Avoid setting KEY_FRAME_RATE on Lollipop and earlier to reduce compatibility risk
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            videoFormat.setInteger(MediaFormat.KEY_FRAME_RATE, refreshRate);
        }

        int maxInputSize = maxInputSize(prefs.bitrate, prefs.fps);
        if (maxInputSize > 0) {
            videoFormat.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxInputSize);
        }

        // Populate keys for adaptive playback. The maximum starts at the stream size and only
        // grows when a mid-stream reconfigure asks for more.
        if (adaptivePlayback) {
            videoFormat.setInteger(MediaFormat.KEY_MAX_WIDTH, maxDecodeWidth);
            videoFormat.setInteger(MediaFormat.KEY_MAX_HEIGHT, maxDecodeHeight);
        }

        // Android 7.0 adds color options to the MediaFormat
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            videoFormat.setInteger(MediaFormat.KEY_COLOR_RANGE,
                    getPreferredColorRange() == MoonBridge.COLOR_RANGE_FULL ?
                    MediaFormat.COLOR_RANGE_FULL : MediaFormat.COLOR_RANGE_LIMITED);

            // If the stream is HDR-capable, the decoder will detect transitions in color standards
            // rather than us hardcoding them into the MediaFormat.
            if ((getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_10BIT) == 0) {
                // Set color format keys when not in HDR mode, since we know they won't change
                videoFormat.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
                switch (getPreferredColorSpace()) {
                    case MoonBridge.COLORSPACE_REC_601:
                        videoFormat.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT601_NTSC);
                        break;
                    case MoonBridge.COLORSPACE_REC_709:
                        videoFormat.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                        break;
                    case MoonBridge.COLORSPACE_REC_2020:
                        videoFormat.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020);
                        break;
                }
            }
        }

        return videoFormat;
    }

    static ByteBuffer createHdrStaticInfo(byte[] metadata) {
        if (metadata == null || metadata.length < 24) {
            return null;
        }
        // The wire layout is CTA-861.3 without the type byte, followed by an
        // optional full-frame luminance field that Android does not accept.
        ByteBuffer info = ByteBuffer.allocate(25);
        info.put((byte) 0).put(metadata, 0, 24).rewind();
        return info;
    }

    private void configureAndStartDecoder(MediaFormat format) {
        // Set HDR metadata if present
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            boolean hdr = currentHdrMode != null ? currentHdrMode :
                    (getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
            format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, hdr ?
                    MediaFormat.COLOR_TRANSFER_ST2084 : MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
            format.setInteger(MediaFormat.KEY_COLOR_STANDARD, hdr ? MediaFormat.COLOR_STANDARD_BT2020 :
                    getPreferredColorSpace() == MoonBridge.COLORSPACE_REC_601 ?
                            MediaFormat.COLOR_STANDARD_BT601_NTSC : MediaFormat.COLOR_STANDARD_BT709);
            ByteBuffer hdrStaticInfo = hdr ? createHdrStaticInfo(currentHdrMetadata) : null;
            if (hdrStaticInfo != null) {
                format.setByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO, hdrStaticInfo);
            }
            else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                format.removeKey(MediaFormat.KEY_HDR_STATIC_INFO);
            } else {
                format.setByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO, null);
            }
        }

        LimeLog.info("Configuring with format: "+format);

        if (upscaler != null) {
            if (Boolean.TRUE.equals(currentHdrMode)) upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
            if (upscalingPolicy.getReason() != UpscalingPolicy.Reason.NONE) closeUpscaler();
        }
        videoDecoder.configure(format, upscaler == null ? renderTarget : upscaler.getInputSurface(), null, 0);
        videoDecoder.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT);
        videoDecoder.start();

        configuredFormat = format;

        // After reconfiguration, we must resubmit CSD buffers
        submittedCsd = false;
        vpsBuffers.clear();
        spsBuffers.clear();
        ppsBuffers.clear();

        setFrameRenderedListener();

        reassertLowLatencyParameters(format);

        // Vendor parameters are applied at start; echoed or declared values confirm acceptance.
        inputFormat = videoDecoder.getInputFormat();
        LimeLog.info("Input format: "+inputFormat);
        updateDecoderDiagnostics();
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
        List<String> vendorParams = MediaCodecHelper.getIntegerVendorParameters(videoDecoder.getName());
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
            videoDecoder.setParameters(params);
        } catch (Exception e) {
            LimeLog.warning("Decoder rejected runtime low-latency parameters: " + e);
        }
    }

    private void updateDecoderDiagnostics() {
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            decoderDiagnostics = context.getString(R.string.perf_overlay_decoder,
                    "PyroWave (Vulkan, " + pyroWaveRenderer.getPresentMode() + ")") + '\n' +
                    context.getString(R.string.perf_overlay_gpu_driver, pyroWaveRenderer.getDriver()) +
                    (pyroWaveRenderer.hasMaxClocks() ? '\n' + context.getString(R.string.title_gpu_max_clocks) + ": " +
                            context.getString(R.string.stream_enabled) : "");
        } else {
            String mimeType = configuredFormat.getString(MediaFormat.KEY_MIME);
            String[] options = MediaCodecHelper.getDecoderLowLatencyOptions(configuredFormat, inputFormat,
                    MediaCodecHelper.getIntegerVendorParameters(videoDecoder.getName()),
                    MediaCodecHelper.decoderSupportsAndroidRLowLatency(videoDecoder.getCodecInfo(), mimeType));
            int mode;
            switch (options[2]) {
                case "android+vendor": mode = R.string.perf_overlay_low_latency_android_vendor; break;
                case "android": mode = R.string.perf_overlay_low_latency_android; break;
                case "vendor": mode = R.string.perf_overlay_low_latency_vendor; break;
                default: mode = R.string.perf_overlay_low_latency_off; break;
            }
            decoderDiagnostics = context.getString(R.string.perf_overlay_decoder,
                    videoDecoder.getName() + " (" + mimeType + ")") + '\n' +
                    context.getString(R.string.perf_overlay_low_latency, context.getString(mode)) + '\n' +
                    context.getString(R.string.perf_overlay_low_latency_keys, options[0]) + '\n' +
                    context.getString(R.string.perf_overlay_low_latency_unconfirmed, options[1]);
        }
        decoderDiagnostics += '\n' + context.getString(R.string.perf_overlay_phone_hints,
                context.getString(prefs.phonePerformanceHints ? R.string.stream_enabled : R.string.stream_disabled));
        LimeLog.info(decoderDiagnostics);
        ProblemReport.setDecoderDiagnostics(initialWidth + "x" + initialHeight + " @ " + refreshRate + " FPS\n" +
                decoderDiagnostics);
    }

    private boolean tryConfigureDecoder(MediaCodecInfo selectedDecoderInfo, MediaFormat format, boolean throwOnCodecError) {
        boolean configured = false;
        try {
            videoDecoder = MediaCodec.createByCodecName(selectedDecoderInfo.getName());
            configureAndStartDecoder(format);
            LimeLog.info("Using codec " + selectedDecoderInfo.getName() + " for hardware decoding " + format.getString(MediaFormat.KEY_MIME));
            configured = true;
        } catch (IllegalArgumentException e) {
            e.printStackTrace();
            if (throwOnCodecError) {
                throw e;
            }
        } catch (IllegalStateException e) {
            e.printStackTrace();
            if (throwOnCodecError) {
                throw e;
            }
        } catch (IOException e) {
            e.printStackTrace();
            if (throwOnCodecError) {
                throw new RuntimeException(e);
            }
        } finally {
            if (!configured && videoDecoder != null) {
                videoDecoder.release();
                videoDecoder = null;
            }
        }
        return configured;
    }

    public int initializeDecoder(boolean throwOnCodecError) {
        String mimeType;
        MediaCodecInfo selectedDecoderInfo;

        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
            mimeType = "video/avc";
            selectedDecoderInfo = avcDecoder;

            if (avcDecoder == null) {
                LimeLog.severe("No available AVC decoder!");
                return -1;
            }

            if (initialWidth > 4096 || initialHeight > 4096) {
                LimeLog.severe("> 4K streaming only supported on HEVC");
                return -1;
            }

            // These fixups only apply to H264 decoders
            needsSpsBitstreamFixup = videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsSpsBitstreamRestrictions(selectedDecoderInfo.getName());
            needsBaselineSpsHack = videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsBaselineSpsHack(selectedDecoderInfo.getName());
            constrainedHighProfile = videoFormat == MoonBridge.VIDEO_FORMAT_H264 &&
                    MediaCodecHelper.decoderNeedsConstrainedHighProfile(selectedDecoderInfo.getName());
            isExynos4 = MediaCodecHelper.isExynos4Device();
            if (needsSpsBitstreamFixup) {
                LimeLog.info("Decoder "+selectedDecoderInfo.getName()+" needs SPS bitstream restrictions fixup");
            }
            if (needsBaselineSpsHack) {
                LimeLog.info("Decoder "+selectedDecoderInfo.getName()+" needs baseline SPS hack");
            }
            if (constrainedHighProfile) {
                LimeLog.info("Decoder "+selectedDecoderInfo.getName()+" needs constrained high profile");
            }
            if (isExynos4) {
                LimeLog.info("Decoder "+selectedDecoderInfo.getName()+" is on Exynos 4");
            }

            refFrameInvalidationActive = refFrameInvalidationAvc;
        }
        else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            mimeType = "video/hevc";
            selectedDecoderInfo = hevcDecoder;

            if (hevcDecoder == null) {
                LimeLog.severe("No available HEVC decoder!");
                return -2;
            }

            refFrameInvalidationActive = refFrameInvalidationHevc;
        }
        else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            mimeType = "video/av01";
            selectedDecoderInfo = av1Decoder;

            if (av1Decoder == null) {
                LimeLog.severe("No available AV1 decoder!");
                return -2;
            }

            refFrameInvalidationActive = refFrameInvalidationAv1;
        }
        else {
            // Unknown format
            LimeLog.severe("Unknown format");
            return -3;
        }

        adaptivePlayback = MediaCodecHelper.decoderSupportsAdaptivePlayback(selectedDecoderInfo, mimeType);
        fusedIdrFrame = MediaCodecHelper.decoderSupportsFusedIdrFrame(selectedDecoderInfo, mimeType);

        boolean retryWithoutUpscaling = false;
        for (int tryNumber = 0;; tryNumber++) {
            LimeLog.info("Decoder configuration try: "+tryNumber);

            MediaFormat mediaFormat = createBaseMediaFormat(mimeType);

            // This will try low latency options until we find one that works (or we give up).
            boolean newFormat = MediaCodecHelper.setDecoderLowLatencyOptions(mediaFormat, selectedDecoderInfo, tryNumber,
                    prefs.codecLowLatency, prefs.vendorLowLatency, prefs.codecPerformance);

            // Throw the underlying codec exception on the last attempt if the caller requested it
            if (tryConfigureDecoder(selectedDecoderInfo, mediaFormat, !newFormat && upscaler == null && throwOnCodecError)) {
                outputBufferQueueLimit = DecoderQueuePolicy.outputLimit(selectedDecoderInfo.getName(),
                        prefs.codecLowLatency && MediaCodecHelper.decoderSupportsAndroidRLowLatency(selectedDecoderInfo, mimeType),
                        prefs.dropLateFrames, tryNumber != 0 || codecRecoveryAttempts != 0);
                if (retryWithoutUpscaling) {
                    upscalingPolicy.fail(UpscalingPolicy.Reason.GPU_ERROR);
                    notifyUpscalingFallback();
                }
                // Success!
                break;
            }

            if (!newFormat) {
                if (upscaler != null) {
                    LimeLog.warning("Retrying decoder formats without the upscaling surface");
                    closeUpscaler();
                    retryWithoutUpscaling = true;
                    tryNumber = -1;
                    continue;
                }
                // We couldn't even configure a decoder without any low latency options
                return -5;
            }
        }

        return 0;
    }

    private void setFrameRenderedListener() {
        // MediaCodec's callback on the GPU path refers to the intermediate texture, not the display.
        if (upscaler == null && latencyHandler != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            videoDecoder.setOnFrameRenderedListener(new MediaCodec.OnFrameRenderedListener() {
                @Override
                public void onFrameRendered(MediaCodec mediaCodec, long presentationTimeUs, long renderTimeNanos) {
                    // Callbacks may be batched; their timestamp, not delivery time, measures rendering.
                    frameLatencyStats.onFrameRendered(presentationTimeUs, renderTimeNanos);
                }
            }, latencyHandler);
        }
    }

    @Override
    public int setup(int format, int width, int height, int redrawRate) {
        int result;
        synchronized (codecRecoveryMonitor) {
            if (stopping) {
                return -1;
            }
            this.initialWidth = width;
            this.initialHeight = height;
            this.maxDecodeWidth = this.expectedWidth = this.streamWidth = width;
            this.maxDecodeHeight = this.expectedHeight = this.streamHeight = height;
            this.videoFormat = format;
            this.refreshRate = redrawRate;
            this.streamFrameRateMilliHz = FramePacingPolicy.negotiatedFrameRateMilliHz(
                    streamFrameRateMilliHz, redrawRate);

            if ((format & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                setupUpscaling();
                result = pyroWaveRenderer.getFormat() == format ? 0 : -1;
                if (result == 0 && currentHdrMode != null) {
                    pyroWaveRenderer.setHdrMode(currentHdrMode, currentHdrMetadata);
                }
                if (result == 0) updateDecoderDiagnostics();
            } else {
                // RTSP may reject an incompatible PyroWave bitstream after surface preparation.
                pyroWaveRenderer.cleanup();
                setupUpscaling();
                result = initializeDecoder(false);
            }
        }
        // The host may stream a smaller size than was asked for (4K to a host without 4K is
        // negotiated down to 1080p); scaling and touch follow the size the decoder gets.
        StreamSizeListener listener = streamSizeListener;
        if (listener != null) {
            listener.onStreamSizeChanged(width, height);
        }
        if (result == 0) {
            if (prefs.enableYuv444 && (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) == 0) {
                activity.runOnUiThread(() -> Toast.makeText(context, R.string.yuv444_fallback, Toast.LENGTH_LONG).show());
            }
            // Default, not background: the decode thread shares the timing lock with this one.
            latencyThread = new HandlerThread("Video - Latency", Process.THREAD_PRIORITY_DEFAULT);
            latencyThread.start();
            latencyHandler = new Handler(latencyThread.getLooper());
            latencyHandler.post(new Runnable() {
                @Override
                public void run() {
                    try {
                        openLatencyCsv();
                    } catch (IOException e) {
                        LimeLog.warning("Unable to open latency CSV: " + e);
                        closeLatencyCsv();
                    }
                    updateLatencyStats.run();
                }
            });
            if (videoDecoder != null) setFrameRenderedListener();
        }
        return result;
    }

    private void openLatencyCsv() throws IOException {
        latencyCsv = new BufferedWriter(new OutputStreamWriter(new FileOutputStream(
                new File(context.getFilesDir(), LATENCY_CSV_NAME + ".tmp")), StandardCharsets.UTF_8));
        latencyCsv.write(FrameLatencyStats.CSV_HEADER);
    }

    private void flushLatencyCsv() {
        if (latencyCsv != null) {
            try {
                // Keep the most recent segment without growing the capture for the entire session.
                if (frameLatencyStats.hasCompletedFrames() &&
                        new File(context.getFilesDir(), LATENCY_CSV_NAME + ".tmp").length() >= MAX_LATENCY_CSV_BYTES) {
                    closeLatencyCsv();
                    openLatencyCsv();
                }
                frameLatencyStats.writeCsv(latencyCsv);
                latencyCsv.flush();
            } catch (IOException e) {
                LimeLog.warning("Unable to write latency CSV: " + e);
                closeLatencyCsv();
            }
        }
    }

    private boolean closeLatencyCsv() {
        if (latencyCsv != null) {
            try {
                latencyCsv.close();
            } catch (IOException e) {
                LimeLog.warning("Unable to close latency CSV: " + e);
                return false;
            } finally {
                latencyCsv = null;
            }
            return true;
        }
        return false;
    }

    // All threads that interact with the MediaCodec instance must call this function regularly!
    private boolean doCodecRecoveryIfRequired(int quiescenceFlag) {
        // NB: We cannot check 'stopping' here because we could end up bailing in a partially
        // quiesced state that will cause the quiesced threads to never wake up.
        if (codecRecoveryType.get() == CR_RECOVERY_TYPE_NONE) {
            // Common case
            return false;
        }

        // We need some sort of recovery, so quiesce all threads before starting that
        synchronized (codecRecoveryMonitor) {
            if (choreographerHandlerThread == null || prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED) {
                // Only balanced pacing touches codec buffers on the Choreographer thread.
                codecRecoveryThreadQuiescedFlags |= CR_FLAG_CHOREOGRAPHER;
            }

            codecRecoveryThreadQuiescedFlags |= quiescenceFlag;

            // This is the final thread to quiesce, so let's perform the codec recovery now.
            if (codecRecoveryThreadQuiescedFlags == CR_FLAG_ALL) {
                if (upscaler != null) {
                    // A flush can strand frames in the texture queue. Restart onto direct output.
                    upscalingPolicy.fail(UpscalingPolicy.Reason.GPU_ERROR);
                    if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_FLUSH, CR_RECOVERY_TYPE_RESTART)) {
                        restartReason = "flush with the upscaler";
                    }
                }
                // Input and output buffers are invalidated by stop() and reset().
                nextInputBuffer = null;
                nextInputBufferIndex = -1;
                synchronized (outputBufferQueue) {
                    outputBufferQueue.clear();
                }
                nextFrameTimeNanos = 0;
                pacingFrameRateMilliHz = 0;
                outputBufferQueueLimit = 1;
                frameLatencyStats.discardPending("codec_reset");

                // If we just need a flush, do so now with all threads quiesced.
                if (codecRecoveryType.get() == CR_RECOVERY_TYPE_FLUSH) {
                    LimeLog.warning("Flushing decoder");
                    try {
                        videoDecoder.flush();
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalStateException e) {
                        e.printStackTrace();

                        // Something went wrong during the restart, let's use a bigger hammer
                        // and try a reset instead.
                        restartReason = "failed flush";
                        codecRecoveryType.set(CR_RECOVERY_TYPE_RESTART);
                    }
                }

                // We don't count flushes as codec recovery attempts
                if (codecRecoveryType.get() != CR_RECOVERY_TYPE_NONE) {
                    codecRecoveryAttempts++;
                    LimeLog.info("Codec recovery attempt: "+codecRecoveryAttempts);
                }

                // For "recoverable" exceptions, we can just stop, reconfigure, and restart.
                if (codecRecoveryType.get() == CR_RECOVERY_TYPE_RESTART) {
                    LimeLog.warning("Trying to restart decoder after " + restartReason);
                    try {
                        videoDecoder.stop();
                        configureAndStartDecoder(configuredFormat);
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalArgumentException e) {
                        e.printStackTrace();

                        // Our Surface is probably invalid, so just stop
                        stopping = true;
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalStateException e) {
                        e.printStackTrace();

                        // Something went wrong during the restart, let's use a bigger hammer
                        // and try a reset instead.
                        codecRecoveryType.set(CR_RECOVERY_TYPE_RESET);
                    }
                }

                // For "non-recoverable" exceptions on L+, we can call reset() to recover
                // without having to recreate the entire decoder again.
                if (codecRecoveryType.get() == CR_RECOVERY_TYPE_RESET) {
                    LimeLog.warning("Trying to reset decoder after CodecException");
                    try {
                        videoDecoder.reset();
                        configureAndStartDecoder(configuredFormat);
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalArgumentException e) {
                        e.printStackTrace();

                        // Our Surface is probably invalid, so just stop
                        stopping = true;
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalStateException e) {
                        e.printStackTrace();

                        // Something went wrong during the reset, we'll have to resort to
                        // releasing and recreating the decoder now.
                    }
                }

                // If we _still_ haven't managed to recover, go for the nuclear option and just
                // throw away the old decoder and reinitialize a new one from scratch.
                if (codecRecoveryType.get() == CR_RECOVERY_TYPE_RESET) {
                    LimeLog.warning("Trying to recreate decoder after CodecException");
                    videoDecoder.release();

                    try {
                        int err = initializeDecoder(true);
                        if (err != 0) {
                            throw new IllegalStateException("Decoder reset failed: " + err);
                        }
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalArgumentException e) {
                        e.printStackTrace();

                        // Our Surface is probably invalid, so just stop
                        stopping = true;
                        codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalStateException e) {
                        // If we failed to recover after all of these attempts, just crash
                        if (!reportedCrash) {
                            reportedCrash = true;
                            crashListener.notifyCrash(e);
                        }
                        throw new RendererException(this, e);
                    }
                }

                // Wake all quiesced threads and allow them to begin work again
                codecRecoveryThreadQuiescedFlags = 0;
                codecRecoveryMonitor.notifyAll();
            }
            else {
                // If we haven't quiesced all threads yet, wait to be signalled after recovery.
                // The final thread to be quiesced will handle the codec recovery.
                while (codecRecoveryType.get() != CR_RECOVERY_TYPE_NONE) {
                    try {
                        LimeLog.info("Waiting to quiesce decoder threads: "+codecRecoveryThreadQuiescedFlags);
                        codecRecoveryMonitor.wait(1000);
                    } catch (InterruptedException e) {
                        e.printStackTrace();

                        // InterruptedException clears the thread's interrupt status. Since we can't
                        // handle that here, we will re-interrupt the thread to set the interrupt
                        // status back to true.
                        Thread.currentThread().interrupt();

                        break;
                    }
                }
            }
        }

        return true;
    }

    // Returns true if the exception is transient
    private boolean handleDecoderException(IllegalStateException e) {
        // Eat decoder exceptions if we're in the process of stopping
        if (stopping) {
            return false;
        }

        if (e instanceof CodecException) {
            CodecException codecExc = (CodecException) e;

            if (codecExc.isTransient()) {
                // We'll let transient exceptions go
                LimeLog.warning(codecExc.getDiagnosticInfo());
                return true;
            }

            LimeLog.severe(codecExc.getDiagnosticInfo());

            // We can attempt a recovery or reset at this stage to try to start decoding again
            if (codecRecoveryAttempts < CR_MAX_TRIES) {
                // If the exception is non-recoverable or we already require a reset, perform a reset.
                // If we have no prior unrecoverable failure, we will try a restart instead.
                if (codecExc.isRecoverable()) {
                    if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_NONE, CR_RECOVERY_TYPE_RESTART)) {
                        restartReason = "CodecException";
                        LimeLog.info("Decoder requires restart for recoverable CodecException");
                        e.printStackTrace();
                    }
                    else if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_FLUSH, CR_RECOVERY_TYPE_RESTART)) {
                        restartReason = "CodecException";
                        LimeLog.info("Decoder flush promoted to restart for recoverable CodecException");
                        e.printStackTrace();
                    }
                    else if (codecRecoveryType.get() != CR_RECOVERY_TYPE_RESET && codecRecoveryType.get() != CR_RECOVERY_TYPE_RESTART) {
                        throw new IllegalStateException("Unexpected codec recovery type: " + codecRecoveryType.get());
                    }
                }
                else if (!codecExc.isRecoverable()) {
                    if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_NONE, CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder requires reset for non-recoverable CodecException");
                        e.printStackTrace();
                    }
                    else if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_FLUSH, CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder flush promoted to reset for non-recoverable CodecException");
                        e.printStackTrace();
                    }
                    else if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_RESTART, CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder restart promoted to reset for non-recoverable CodecException");
                        e.printStackTrace();
                    }
                    else if (codecRecoveryType.get() != CR_RECOVERY_TYPE_RESET) {
                        throw new IllegalStateException("Unexpected codec recovery type: " + codecRecoveryType.get());
                    }
                }

                // The recovery will take place when all threads reach doCodecRecoveryIfRequired().
                return false;
            }
        }
        else {
            // IllegalStateException was primarily used prior to the introduction of CodecException.
            // Recovery from this requires a full decoder reset.
            //
            // NB: CodecException is an IllegalStateException, so we must check for it first.
            if (codecRecoveryAttempts < CR_MAX_TRIES) {
                if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_NONE, CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder requires reset for IllegalStateException");
                    e.printStackTrace();
                }
                else if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_FLUSH, CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder flush promoted to reset for IllegalStateException");
                    e.printStackTrace();
                }
                else if (codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_RESTART, CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder restart promoted to reset for IllegalStateException");
                    e.printStackTrace();
                }
                else if (codecRecoveryType.get() != CR_RECOVERY_TYPE_RESET) {
                    throw new IllegalStateException("Unexpected codec recovery type: " + codecRecoveryType.get());
                }

                return false;
            }
        }

        // Only throw if we're not in the middle of codec recovery
        if (codecRecoveryType.get() == CR_RECOVERY_TYPE_NONE) {
            //
            // There seems to be a race condition with decoder/surface teardown causing some
            // decoders to to throw IllegalStateExceptions even before 'stopping' is set.
            // To workaround this while allowing real exceptions to propagate, we will eat the
            // first exception. If we are still receiving exceptions 3 seconds later, we will
            // throw the original exception again.
            //
            if (initialException != null) {
                // This isn't the first time we've had an exception processing video
                if (SystemClock.uptimeMillis() - initialExceptionTimestamp >= EXCEPTION_REPORT_DELAY_MS) {
                    // It's been over 3 seconds and we're still getting exceptions. Throw the original now.
                    if (!reportedCrash) {
                        reportedCrash = true;
                        crashListener.notifyCrash(initialException);
                    }
                    throw initialException;
                }
            }
            else {
                // This is the first exception we've hit
                initialException = new RendererException(this, e);
                initialExceptionTimestamp = SystemClock.uptimeMillis();
            }
        }

        // Not transient
        return false;
    }

    static long nextVsyncTimeNs(long nowNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) {
        // Before the first callback there is no display phase to align to.
        if (vsyncNs == 0 || intervalNs <= 0) return nowNs;
        long intervals = Math.max(1, (nowNs + presentationDeadlineNs - vsyncNs) / intervalNs + 1);
        return vsyncNs + intervals * intervalNs;
    }

    // Phase lock: how long a frame ready at readyNs waits for the next latch deadline.
    static long latchSlackNs(long readyNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) {
        if (vsyncNs == 0 || intervalNs <= 0) return -1;
        return nextVsyncTimeNs(readyNs, vsyncNs, intervalNs, presentationDeadlineNs) - presentationDeadlineNs - readyNs;
    }

    /** Changes only this stream's preference object, never the saved pacing setting. */
    public boolean applyOutputPacingFallback() {
        // Vulkan has its own presentation clock, including actual external-display timestamps.
        // Keep its requested mode before negotiation; conventional fallback still uses this policy.
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ||
                (videoFormat == 0 && prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE)) return false;
        if (outputVsyncAvailable || prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED) return false;
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
        return true;
    }

    private void reportPhaseSlack(long readyNs) {
        // Adaptive refresh has no fixed latch; an upscaled frame is not display-ready yet.
        if (stopping || !outputVsyncAvailable || upscaler != null) return;
        long slackNs, intervalNs;
        synchronized (vsyncMonitor) {
            if (outputHasArrSupport) return;
            intervalNs = vsyncIntervalNs;
            slackNs = latchSlackNs(readyNs, vsyncTimeNs, intervalNs, vsyncPresentationDeadlineNs);
        }
        if (slackNs >= 0 && MoonBridge.reportFrameSlack(slackNs, intervalNs)) {
            Handler handler = latencyHandler;
            if (handler != null && phaseFlushQueued.compareAndSet(false, true) &&
                    !handler.post(flushPhaseLockReport)) {
                phaseFlushQueued.set(false);
            }
        }
    }

    private void updateDisplayTiming() {
        // ARR is a property of the active output mode, not the saved VRR preference.
        // A fold or mode change can invalidate a fixed-refresh latch estimate.
        boolean arrSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA &&
                streamDisplay.hasArrSupport();
        float physicalRefreshRate = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                streamDisplay.getMode().getRefreshRate() : streamDisplay.getRefreshRate();
        updateUpscalingFrameRate(physicalRefreshRate);
        synchronized (vsyncMonitor) {
            outputHasArrSupport = arrSupported;
            vsyncIntervalNs = (long) (1000000000.0 / physicalRefreshRate);
            vsyncPresentationDeadlineNs = streamDisplay.getPresentationDeadlineNanos();
            appVsyncOffsetNs = streamDisplay.getAppVsyncOffsetNanos();
            // Wait for a sample from the new mode before reporting its phase.
            vsyncTimeNs = 0;
            pyroWaveDisplayClock.reset(vsyncIntervalNs);
        }
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        // Do nothing if we're stopping
        if (stopping || !outputVsyncAvailable) {
            return;
        }

        synchronized (vsyncMonitor) {
            frameTimeNanos -= appVsyncOffsetNs;
            vsyncTimeNs = frameTimeNanos;
        }
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 &&
                FramePacingPolicy.usesPyroWaveDisplayClock(prefs.framePacing, prefs.vrr, outputVsyncAvailable)) {
            // This only publishes immutable Java values. No Vulkan call or renderer lock can
            // delay the display callback or race native teardown.
            pyroWaveDisplayClock.observe(frameTimeNanos);
        }
        if (prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED ||
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // Other modes render from the decoder thread; this callback only tracks vsync.
            Choreographer.getInstance().postFrameCallback(this);
            return;
        }

        // Only the Choreographer thread advances the cadence. A rate change starts a new
        // timeline here instead of racing its deadline from a connection/UI callback.
        int fpsMilliHz = streamFrameRateMilliHz;
        if (pacingFrameRateMilliHz != fpsMilliHz) {
            pacingFrameRateMilliHz = fpsMilliHz;
            nextFrameTimeNanos = 0;
        }

        // Don't render unless a new frame is due. This prevents microstutter when streaming
        // at a frame rate that doesn't match the display (such as 60 FPS on 120 Hz).
        if (FramePacingPolicy.isFrameDue(nextFrameTimeNanos, frameTimeNanos, vsyncIntervalNs)) {
            // Render up to one frame when in frame pacing mode.
            //
            // The queue holds at most two buffers to avoid starving the decoder. Keeping
            // only the newest frame trades the extra jitter buffer for lower latency.
            Integer nextOutputBuffer;
            synchronized (outputBufferQueue) {
                nextOutputBuffer = outputBufferQueue.poll();
            }
            if (nextOutputBuffer != null) {
                try {
                    frameLatencyStats.onOutputReleased(nextOutputBuffer, System.nanoTime(), true,
                            upscaler == null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.M);
                    releaseOutputFrame(nextOutputBuffer, System.nanoTime());

                    nextFrameTimeNanos = FramePacingPolicy.nextFrameTimeNsMilliHz(
                            nextFrameTimeNanos, frameTimeNanos, fpsMilliHz);
                    activeWindowVideoStats.totalFramesRendered++;
                } catch (IllegalStateException ignored) {
                    try {
                        // Try to avoid leaking the output buffer by releasing it without rendering
                        videoDecoder.releaseOutputBuffer(nextOutputBuffer, false);
                    } catch (IllegalStateException e) {
                        // This will leak nextOutputBuffer, but there's really nothing else we can do
                        e.printStackTrace();
                        handleDecoderException(e);
                    }
                }
            }
        }

        // Attempt codec recovery even if we have nothing to render right now. Recovery can still
        // be required even if the codec died before giving any output.
        doCodecRecoveryIfRequired(CR_FLAG_CHOREOGRAPHER);

        // Request another callback for next frame
        Choreographer.getInstance().postFrameCallback(this);
    }

    private void startChoreographerThread() {
        // Local output tracks vsync for phase-lock reports; only balanced renders here.
        // External output still observes mode changes for upscaling, but cannot use
        // the internal display's callback as an external pacing or phase signal.
        // We use a separate thread to avoid any main thread delays from delaying rendering
        choreographerHandlerThread = new HandlerThread("Video - Choreographer", Process.THREAD_PRIORITY_DISPLAY);
        choreographerHandlerThread.start();

        streamDisplay = outputDisplay;
        displayManager = (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);

        // Start the frame callbacks
        choreographerHandler = new Handler(choreographerHandlerThread.getLooper());
        choreographerHandler.post(new Runnable() {
            @Override
            public void run() {
                displayManager.registerDisplayListener(displayListener, choreographerHandler);
                updateDisplayTiming();
                if (outputVsyncAvailable) {
                    Choreographer.getInstance().postFrameCallback(MediaCodecDecoderRenderer.this);
                }
            }
        });
    }

    private void startRendererThread()
    {
        rendererThread = new Thread() {
            @Override
            public void run() {
                // Frames leave the decoder for the display from here.
                Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY);
                try (DecoderPerformanceHints performanceHints =
                             new DecoderPerformanceHints(context, prefs.phonePerformanceHints, refreshRate)) {
                    BufferInfo info = new BufferInfo();
                    while (!stopping) {
                        long workStartNs = 0;
                        try {
                            // Try to output a frame
                            int outIndex = videoDecoder.dequeueOutputBuffer(info, 50000);
                            if (outIndex >= 0) {
                                // Waiting for decoder output is idle time, not work for ADPF.
                                workStartNs = System.nanoTime();
                                // This separate elapsed clock includes scheduling/suspend delays;
                                // frame timestamps and ADPF durations keep their existing clock.
                                long outputDrainStartNs = SystemClock.elapsedRealtimeNanos();
                                frameLatencyStats.onDecoderOutput(outIndex, info.presentationTimeUs, workStartNs);
                                int lastIndex = outIndex;
                                long lastPtsUs = info.presentationTimeUs;

                                numFramesOut++;

                                // Render the latest frame now if frame pacing isn't in balanced mode
                                if (prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED) {
                                    // Latency mode takes the newest output. Smooth modes only catch up
                                    // after a backlog exceeds the age budget, then resume normal order.
                                    // All frames have already been decoded, preserving codec references.
                                    int drained = 0;
                                    while (!stopping && codecRecoveryType.get() == CR_RECOVERY_TYPE_NONE &&
                                            FramePacingPolicy.hasOutputDrainTimeRemaining(
                                                    SystemClock.elapsedRealtimeNanos() - outputDrainStartNs,
                                                    streamFrameRateMilliHz) &&
                                            FramePacingPolicy.shouldDrainOutput(prefs.framePacing,
                                                    frameLatencyStats.getEnqueueTimeNsForOutput(lastIndex),
                                                    System.nanoTime(), streamFrameRateMilliHz, drained) &&
                                            (outIndex = videoDecoder.dequeueOutputBuffer(info, 0)) >= 0) {
                                        frameLatencyStats.onDecoderOutput(outIndex, info.presentationTimeUs, System.nanoTime());
                                        frameLatencyStats.onOutputReleased(lastIndex, System.nanoTime(), false,
                                                upscaler == null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.M);
                                        videoDecoder.releaseOutputBuffer(lastIndex, false);

                                        numFramesOut++;

                                        lastIndex = outIndex;
                                        lastPtsUs = info.presentationTimeUs;
                                        drained++;
                                    }

                                    frameLatencyStats.onOutputReleased(lastIndex, System.nanoTime(), true,
                                            upscaler == null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.M);
                                    if (prefs.framePacing == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS ||
                                            prefs.framePacing == PreferenceConfiguration.FRAME_PACING_CAP_FPS) {
                                        // Retain the ordinary smooth-mode presentation behavior after
                                        // replacing any stale decoded outputs above.
                                        releaseOutputFrame(lastIndex, 0);
                                    }
                                    else {
                                        // Use a PTS that will cause this frame to be dropped if another comes in within
                                        // the same V-sync period. "Now" lets the compositor take it at its very next
                                        // latch; a computed vsync target that errs late (a conservative deadline, a
                                        // drifting phase estimate) holds frames back a whole refresh.
                                        releaseOutputFrame(lastIndex, System.nanoTime());
                                    }
                                    // Artemis counts the frame it presents, once it has been released.
                                    frameLatencyStats.onArtemisDecodeCounted(lastPtsUs, System.nanoTime());
                                    reportPhaseSlack(System.nanoTime());

                                    activeWindowVideoStats.totalFramesRendered++;
                                }
                                else {
                                    // For balanced frame pacing case, the Choreographer callback will handle rendering.
                                    // We just put all frames into the output buffer queue and let it handle things.

                                    // Discard the oldest buffer if we've exceeded our limit.
                                    //
                                    // NB: We have to do this on the producer side because the consumer may not
                                    // run for a while (if there is a huge mismatch between stream FPS and display
                                    // refresh rate).
                                    int queueLimit = prefs.dropLateFrames ? 1 : outputBufferQueueLimit;
                                    Integer droppedIndex;
                                    synchronized (outputBufferQueue) {
                                        droppedIndex = outputBufferQueue.size() >= queueLimit ?
                                                outputBufferQueue.poll() : null;
                                        outputBufferQueue.add(lastIndex);
                                    }
                                    // Publish the replacement before returning the old output to the
                                    // codec. That call (or timing bookkeeping) can block; the display
                                    // callback must still find the ready replacement in the meantime.
                                    if (droppedIndex != null) {
                                        frameLatencyStats.onOutputReleased(droppedIndex, System.nanoTime(), false,
                                                upscaler == null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.M);
                                        videoDecoder.releaseOutputBuffer(droppedIndex, false);
                                    }
                                    // Artemis counts a queued frame unless queuing it dropped an older one.
                                    if (droppedIndex == null) {
                                        frameLatencyStats.onArtemisDecodeCounted(lastPtsUs, System.nanoTime());
                                    }
                                }
                            }
                            if (outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                                LimeLog.info("Output format changed");
                                outputFormat = videoDecoder.getOutputFormat();
                                LimeLog.info("New output format: " + outputFormat);
                                checkUpscalingOutputFormat();
                                updateShownSize();
                            }
                        } catch (IllegalStateException e) {
                            handleDecoderException(e);
                        } finally {
                            if (workStartNs != 0) {
                                // Report the complete handoff once, after presenting. Reporting each
                                // discarded output adds work while we are trying to leave catch-up.
                                performanceHints.reportWorkDuration(System.nanoTime() - workStartNs, refreshRate);
                            }
                            checkUpscaling();
                            doCodecRecoveryIfRequired(CR_FLAG_RENDER_THREAD);
                        }
                    }
                }
            }
        };
        rendererThread.setName("Video - Renderer (MediaCodec)");
        rendererThread.start();
    }

    private boolean fetchNextInputBuffer() {
        long startTime;
        boolean codecRecovered;

        if (nextInputBuffer != null) {
            // We already have an input buffer
            return true;
        }

        startTime = SystemClock.uptimeMillis();

        try {
            // If we don't have an input buffer index yet, fetch one now
            while (nextInputBufferIndex < 0 && !stopping &&
                    codecRecoveryType.get() == CR_RECOVERY_TYPE_NONE) {
                // Under input pressure, recheck recovery and shutdown without a 10 ms wait.
                nextInputBufferIndex = videoDecoder.dequeueInputBuffer(1000);
                if (SystemClock.uptimeMillis() - startTime >= 5000) break;
            }

            // Get the backing ByteBuffer for the input buffer index
            if (nextInputBufferIndex >= 0) {
                // Using the new getInputBuffer() API on Lollipop allows
                // the framework to do some performance optimizations for us
                nextInputBuffer = videoDecoder.getInputBuffer(nextInputBufferIndex);
                if (nextInputBuffer == null) {
                    // According to the Android docs, getInputBuffer() can return null "if the
                    // index is not a dequeued input buffer". I don't think this ever should
                    // happen but if it does, let's try to get a new input buffer next time.
                    nextInputBufferIndex = -1;
                }
            }
        } catch (IllegalStateException e) {
            handleDecoderException(e);
            return false;
        } finally {
            codecRecovered = doCodecRecoveryIfRequired(CR_FLAG_INPUT_THREAD);
        }

        // If codec recovery is required, always return false to ensure the caller will request
        // an IDR frame to complete the codec recovery.
        if (codecRecovered) {
            return false;
        }

        int deltaMs = (int)(SystemClock.uptimeMillis() - startTime);

        if (deltaMs >= 20) {
            LimeLog.warning("Dequeue input buffer ran long: " + deltaMs + " ms");
        }

        if (nextInputBuffer == null) {
            // We've been hung for 5 seconds and no other exception was reported,
            // so generate a decoder hung exception
            if (deltaMs >= 5000 && initialException == null) {
                DecoderHungException decoderHungException = new DecoderHungException(deltaMs);
                if (!reportedCrash) {
                    reportedCrash = true;
                    crashListener.notifyCrash(decoderHungException);
                }
                throw new RendererException(this, decoderHungException);
            }

            return false;
        }

        return true;
    }

    @Override
    public void start() {
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            if (FramePacingPolicy.usesPyroWaveDisplayClock(prefs.framePacing, prefs.vrr, outputVsyncAvailable)) {
                startChoreographerThread();
            }
            return;
        }
        applyOutputPacingFallback();
        startRendererThread();
        startChoreographerThread();
    }

    // !!! May be called even if setup()/start() fails !!!
    public void prepareForStop() {
        // Let the decoding code know to ignore codec exceptions now
        stopping = true;
        if (prefs.upscalingMode != UpscalingPolicy.Mode.OFF && Looper.myLooper() == Looper.getMainLooper()) {
            // stop() repeats this on the shutdown worker, away from EGL startup/recovery on the UI.
            return;
        }
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // Vulkan teardown and HDR transitions may wait on the GPU; keep the UI off that lock.
            // The vsync tracker still has to end, or stop() would wait for it forever.
            quitChoreographerThread();
            return;
        }

        // Halt the rendering thread
        if (rendererThread != null) {
            rendererThread.interrupt();
        }

        // Stop any active codec recovery operations
        synchronized (codecRecoveryMonitor) {
            codecRecoveryType.set(CR_RECOVERY_TYPE_NONE);
            codecRecoveryMonitor.notifyAll();
        }

        quitChoreographerThread();
    }

    private void quitChoreographerThread() {
        // Post a quit message to the Choreographer looper (if we have one)
        if (choreographerHandler != null) {
            choreographerHandler.post(new Runnable() {
                @Override
                public void run() {
                    displayManager.unregisterDisplayListener(displayListener);
                    // Don't allow any further messages to be queued
                    choreographerHandlerThread.quit();

                    // Deregister the frame callback (if registered)
                    if (outputVsyncAvailable) {
                        Choreographer.getInstance().removeFrameCallback(MediaCodecDecoderRenderer.this);
                    }
                }
            });
        }
    }

    @Override
    public void stop() {
        // May be called already, but we'll call it now to be safe
        prepareForStop();

        // Wait for the Choreographer looper to shut down (if we have one)
        if (choreographerHandlerThread != null) {
            try {
                choreographerHandlerThread.join();
            } catch (InterruptedException e) {
                e.printStackTrace();

                // InterruptedException clears the thread's interrupt status. Since we can't
                // handle that here, we will re-interrupt the thread to set the interrupt
                // status back to true.
                Thread.currentThread().interrupt();
            }
        }

        // Wait for the renderer thread to shut down
        if (rendererThread == null) return;
        try {
            rendererThread.join();
        } catch (InterruptedException e) {
            e.printStackTrace();

            // InterruptedException clears the thread's interrupt status. Since we can't
            // handle that here, we will re-interrupt the thread to set the interrupt
            // status back to true.
            Thread.currentThread().interrupt();
        }
    }

    @Override
    public void cleanup() {
        pyroWaveRenderer.cleanup(frameLatencyStats);
        if (videoDecoder != null) {
            videoDecoder.release();
            videoDecoder = null;
        }
        // The stopped codec has invalidated these outputs; never offer them to a later callback.
        synchronized (outputBufferQueue) {
            outputBufferQueue.clear();
        }
        closeUpscaler();
        if (latencyHandler != null) {
            latencyHandler.removeCallbacks(flushPhaseLockReport);
            phaseFlushQueued.set(false);
            latencyHandler.post(new Runnable() {
                @Override
                public void run() {
                    latencyHandler.removeCallbacks(updateLatencyStats);
                    frameLatencyStats.discardPending("stream_ended");
                    flushLatencyCsv();
                    if (closeLatencyCsv() && !new File(context.getFilesDir(), LATENCY_CSV_NAME + ".tmp")
                            .renameTo(new File(context.getFilesDir(), LATENCY_CSV_NAME))) {
                        LimeLog.warning("Unable to save completed latency CSV");
                    }
                    latencyThread.quitSafely();
                }
            });
            boolean interrupted = false;
            while (latencyThread.isAlive()) {
                try {
                    latencyThread.join();
                } catch (InterruptedException e) {
                    // Reconnect must not overwrite the CSV until the cancelled session closes it.
                    interrupted = true;
                }
            }
            latencyHandler = null;
            latencyThread = null;
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    @Override
    public void setHdrMode(boolean enabled, byte[] hdrMetadata) {
        // A callback during recovery must not have its new metadata or restart request overwritten.
        synchronized (codecRecoveryMonitor) {
            if (stopping) {
                return;
            }
            if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                currentHdrMode = enabled;
                currentHdrMetadata = enabled && hdrMetadata != null && hdrMetadata.length >= 24 ? hdrMetadata.clone() : null;
                pyroWaveRenderer.setHdrMode(enabled, currentHdrMetadata);
                return;
            }
            // HDR metadata is only supported in Android 7.0 and later, so don't bother
            // restarting the codec on anything earlier than that.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                byte[] metadata = enabled && hdrMetadata != null && hdrMetadata.length >= 24 ? hdrMetadata : null;
                boolean previousMode = currentHdrMode != null ? currentHdrMode :
                        (getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
                currentHdrMode = enabled;
                if (enabled && upscaler != null) upscalingPolicy.fail(UpscalingPolicy.Reason.HDR);
                if (previousMode == enabled && Arrays.equals(currentHdrMetadata, metadata)) {
                    return;
                }
                currentHdrMetadata = metadata == null ? null : metadata.clone();

                // If we reach this point, we need to restart the MediaCodec instance to
                // pick up the HDR metadata change. This will happen on the next input
                // or output buffer.

                // HACK: Reset codec recovery attempt counter, since this is an expected "recovery"
                codecRecoveryAttempts = 0;

                // The host's first HDR report restarts a 10-bit stream once: it adds metadata or turns HDR off.
                requestRestart(enabled ? (metadata != null ? "HDR metadata" : "HDR on") : "HDR off");
            }
        }
    }

    // Promotes None/Flush to Restart and leaves Reset alone.
    private void requestRestart(String reason) {
        restartReason = reason;
        LimeLog.info("Decoder restart requested: " + reason);
        if (!codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_NONE, CR_RECOVERY_TYPE_RESTART)) {
            codecRecoveryType.compareAndSet(CR_RECOVERY_TYPE_FLUSH, CR_RECOVERY_TYPE_RESTART);
        }
    }

    private boolean queueNextInputBuffer(long timestampUs, int codecFlags, int frameNumber, long receiveTimeNs,
                                         long enqueueTimeNs, char hostProcessingLatency) {
        boolean codecRecovered;

        try {
            if ((codecFlags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                frameLatencyStats.onDecoderInput(frameNumber, timestampUs, receiveTimeNs, enqueueTimeNs,
                        System.nanoTime(), hostProcessingLatency);
            }
            videoDecoder.queueInputBuffer(nextInputBufferIndex,
                    0, nextInputBuffer.position(),
                    timestampUs, codecFlags);
            if ((codecFlags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                frameLatencyStats.onDecoderQueued(timestampUs, System.nanoTime());
            }

            // We need a new buffer now
            nextInputBufferIndex = -1;
            nextInputBuffer = null;
        } catch (IllegalStateException e) {
            frameLatencyStats.discard(timestampUs, "input_failed");
            if (handleDecoderException(e)) {
                // We encountered a transient error. In this case, just hold onto the buffer
                // (to avoid leaking it), clear it, and keep it for the next frame. We'll return
                // false to trigger an IDR frame to recover.
                nextInputBuffer.clear();
            }
            else {
                // We encountered a non-transient error. In this case, we will simply leak the
                // buffer because we cannot be sure we will ever succeed in queuing it.
                nextInputBufferIndex = -1;
                nextInputBuffer = null;
            }
            return false;
        } finally {
            codecRecovered = doCodecRecoveryIfRequired(CR_FLAG_INPUT_THREAD);
        }

        // If codec recovery is required, always return false to ensure the caller will request
        // an IDR frame to complete the codec recovery.
        if (codecRecovered) {
            return false;
        }

        // Fetch a new input buffer now while we have some time between frames
        // to have it ready immediately when the next frame arrives.
        //
        // We must propagate the return value here in order to properly handle
        // codec recovery happening in fetchNextInputBuffer(). If we don't, we'll
        // never get an IDR frame to complete the recovery process.
        return fetchNextInputBuffer();
    }

    private void doProfileSpecificSpsPatching(SeqParameterSet sps) {
        // Some devices benefit from setting constraint flags 4 & 5 to make this Constrained
        // High Profile which allows the decoder to assume there will be no B-frames and
        // reduce delay and buffering accordingly. Some devices (Marvell, Exynos 4) don't
        // like it so we only set them on devices that are confirmed to benefit from it.
        if (sps.profileIdc == 100 && constrainedHighProfile) {
            LimeLog.info("Setting constraint set flags for constrained high profile");
            sps.constraintSet4Flag = true;
            sps.constraintSet5Flag = true;
        }
        else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            // Force the constraints unset otherwise (some may be set by default)
            sps.constraintSet4Flag = false;
            sps.constraintSet5Flag = false;
        }
    }

    private void updatePerformanceOverlay() {
        VideoStatsFps fps = overlayFps;
        if (perfListener != null && fps != null) {
            float[] frameRates = frameLatencyStats.getFrameRates(System.nanoTime());
            String shownFps = metric(frameRates[2], " FPS");
            long presentDrops = frameLatencyStats.getPresentDrops();
            boolean mediaCodec = (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0;
            long rttInfo = MoonBridge.getEstimatedRttInfo();
            float networkMs = rttInfo == -1 ? -1 : (int) (rttInfo >> 32);
            float varianceMs = rttInfo == -1 ? -1 : (int) rttInfo;
            String codec = (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ? "PyroWave" :
                    (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0 ? "AV1" :
                    (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0 ? "HEVC" : "H.264";
            String color = Boolean.TRUE.equals(currentHdrMode) ? "HDR" :
                    currentHdrMode == null && (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 ? "10-bit" : "SDR";
            String stream = streamWidth + "×" + streamHeight + " · " + codec + " · " + color;
            StringBuilder video = new StringBuilder();
            video.append(stream).append('\n');
            video.append(context.getString(R.string.perf_overlay_resolution_source, context.getString(
                    prefs.resolutionFollowsScreen ?
                            (prefs.fromPcProfile ? R.string.resolution_source_native_profile : R.string.resolution_source_native) :
                            (prefs.fromPcProfile ? R.string.resolution_source_fixed_profile : R.string.resolution_source_fixed))))
                    .append('\n');
            if (prefs.layersText != null) {
                video.append(prefs.layersText).append('\n');
            }
            if (prefs.hostDisplay != null) {
                video.append(context.getString(R.string.perf_overlay_host_display,
                        context.getString(prefs.hostDisplay.label))).append('\n');
            }
            video.append(context.getString(R.string.perf_overlay_incomingfps,
                    frameRates[0])).append('\n');
            video.append(context.getString(R.string.perf_overlay_releasedfps, frameRates[1])).append('\n');
            video.append(context.getString(R.string.perf_overlay_shownfps, shownFps)).append('\n');
            video.append(context.getString(R.string.perf_overlay_presentdrops, presentDrops)).append('\n');
            video.append((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0 ? "YUV 4:4:4" : "YUV 4:2:0");
            if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0) {
                video.append(" 10-bit");
            }
            Locale locale = context.getResources().getConfiguration().locale;
            String network = context.getString(R.string.stats_throughput,
                    PerformanceOverlay.measurement(locale, receivedVideoMbps, "%.1f"),
                    PerformanceOverlay.measurement(locale, fps.lossPercent, "%.2f")) + '\n' +
                    context.getString(R.string.stats_network,
                            PerformanceOverlay.measurement(locale, networkMs, "%.0f"),
                            PerformanceOverlay.measurement(locale, varianceMs, "%.0f"));
            StringBuilder decode = new StringBuilder(decoderDiagnostics);
            // The timing block below already lists decode time; show it here only without that block.
            if (mediaCodec && latencyOverlay.isEmpty()) {
                decode.append('\n').append(shownDecodeTimeMs >= 0 ? context.getString(R.string.perf_overlay_dectime, shownDecodeTimeMs) :
                        context.getString(R.string.latency_input_output) + ": " + context.getString(R.string.latency_unavailable));
            }
            if (!mediaCodec) {
                decode.append('\n').append(context.getString(R.string.perf_overlay_pyrowave_submit,
                        PerformanceOverlay.measurement(locale, shownDecodeTimeMs, "%.2f")));
                int gpuUs = pyroWaveRenderer.getLastGpuDecodeUs();
                float[] waits = pyroWaveRenderer.getWaits();
                decode.append('\n').append(formatPyroWaveStats(pyroWaveLossPercent, pyroWaveQueueDelayMs, gpuUs))
                        .append("\nWaits per frame: free planes ").append(metric(waits[0], " ms"))
                        .append(" | screen image ").append(metric(waits[1], " ms"))
                        .append("\nReplaced by a newer frame: before decode ").append(metric(pyroWaveSkippedPercent, "%"))
                        .append(" | before display ").append(metric(waits[2], "%"));
            }
            int leadUs = MoonBridge.getPhaseLockLeadUs();
            if (leadUs != Integer.MIN_VALUE) {
                decode.append("\nReady before screen refresh: ").append(metric(leadUs / 1000f, " ms"));
            }
            decode.append('\n').append(upscalingStats());
            decode.append('\n').append(thermalStats());
            decode.append('\n').append(latencyOverlay);
            double[][] timings = latencySummary;
            perfListener.onPerfUpdate(video.toString(), network, decode.toString(),
                    PerformanceOverlay.statisticsText(context, stream, frameRates[2],
                            frameRates[0], receivedVideoMbps, fps.lossPercent,
                            networkMs, varianceMs, PerformanceOverlay.average(timings, 4), shownDecodeTimeMs,
                            frameRates[2] < 0 ? -1 : PerformanceOverlay.average(timings, 2), !mediaCodec),
                    PerformanceOverlay.compactText(context, frameRates[2], frameRates[0], networkMs,
                            fps.lossPercent, decodeTimeMs));
        }
    }

    @SuppressWarnings("deprecation")
    @Override
    public int submitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                int frameNumber, int frameType, char frameHostProcessingLatency,
                                long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                long presentationTimeUs) {
        if (stopping) {
            // Don't bother if we're stopping
            return MoonBridge.DR_OK;
        }

        lastVideoFrameTimeMs = SystemClock.uptimeMillis();
        long enqueueTimeNs = receiveTimeNs + (enqueueTimeUs - receiveTimeUs) * 1000;

        if (lastFrameNumber == 0) {
            activeWindowVideoStats.measurementStartTimestamp = SystemClock.uptimeMillis();
        } else if (frameNumber != lastFrameNumber && frameNumber != lastFrameNumber + 1) {
            // We can receive the same "frame" multiple times if it's an IDR frame.
            // In that case, each frame start NALU is submitted independently.
            activeWindowVideoStats.framesLost += frameNumber - lastFrameNumber - 1;
            activeWindowVideoStats.totalFrames += frameNumber - lastFrameNumber - 1;
            activeWindowVideoStats.frameLossEvents++;
        }

        // Reset CSD data for each IDR frame
        if (lastFrameNumber != frameNumber && frameType == MoonBridge.FRAME_TYPE_IDR) {
            vpsBuffers.clear();
            spsBuffers.clear();
            ppsBuffers.clear();
        }

        if (frameNumber != lastFrameNumber) {
            queueDelay.onFrame(receiveTimeUs, presentationTimeUs, frameHostProcessingLatency * 100L);
        }
        lastFrameNumber = frameNumber;

        // Flip stats windows roughly every second
        if (SystemClock.uptimeMillis() >= activeWindowVideoStats.measurementStartTimestamp + 1000) {
            pyroWaveLossPercent = pyroWaveLossSamples == 0 ? -1 : pyroWaveLossTotal / pyroWaveLossSamples;
            pyroWaveLossTotal = 0;
            pyroWaveLossSamples = 0;
            if (pyroWaveQueueSamples != 0) {
                pyroWaveQueueDelayMs = pyroWaveQueueDelayTotal / pyroWaveQueueSamples;
                pyroWaveQueueDelayTotal = 0;
                pyroWaveQueueSamples = 0;
            }
            if (pyroWaveSkippedFrames + pyroWaveDecodedFrames != 0) {
                pyroWaveSkippedPercent = 100f * pyroWaveSkippedFrames / (pyroWaveSkippedFrames + pyroWaveDecodedFrames);
                pyroWaveSkippedFrames = pyroWaveDecodedFrames = 0;
            }
            decodeTimeMs = frameLatencyStats.takeDecodeTimeMs();
            boolean pyroWave = (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
            if (!pyroWave) {
                activeWindowVideoStats.decoderTimeMs += frameLatencyStats.takeArtemisDecodeMs();
            }
            queueDelayMs = queueDelay.takeWindowMs();
            VideoStats lastTwo = new VideoStats();
            lastTwo.add(lastWindowVideoStats);
            lastTwo.add(activeWindowVideoStats);
            // PyroWave can measure real zero whole milliseconds while carrying the sub-ms remainder
            // into the next window. GPU completion is not a prerequisite for that CPU measurement.
            shownDecodeTimeMs = lastTwo.getWindowDecoderLatencyMs(pyroWave || decodeTimeMs >= 0);
            networkFrameLossPercent = activeWindowVideoStats.totalFrames == 0 ? 0 :
                    (float) activeWindowVideoStats.framesLost / activeWindowVideoStats.totalFrames * 100;
            overlayFps = lastTwo.getFps();

            globalVideoStats.add(activeWindowVideoStats);
            lastWindowVideoStats.copy(activeWindowVideoStats);
            activeWindowVideoStats.clear();
            activeWindowVideoStats.measurementStartTimestamp = SystemClock.uptimeMillis();
        }

        receivedVideoBytes.addAndGet(decodeUnitLength);

        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            long inputNs = System.nanoTime();
            pyroWaveQueueDelayTotal += Math.max(0, inputNs - enqueueTimeNs) / 1000000.0f;
            pyroWaveQueueSamples++;
            long ptsUs = Math.max(inputNs / 1000, lastTimestampUs + 1);
            lastTimestampUs = ptsUs;
            frameLatencyStats.onDecoderInput(frameNumber, ptsUs, receiveTimeNs, enqueueTimeNs, inputNs, frameHostProcessingLatency);
            frameLatencyStats.onPyroWaveDecoderQueued(ptsUs, inputNs);
            // Newest frame wins: every PyroWave frame is intra-only, so when a newer one is already
            // waiting, decoding this one would only delay it. A phone that falls behind catches up
            // at once instead of working through a backlog.
            if (MoonBridge.getPendingVideoFrames() > 0) {
                activeWindowVideoStats.totalFrames++;
                activeWindowVideoStats.totalFramesReceived++;
                pyroWaveSkippedFrames++;
                frameLatencyStats.onFrameReceived(receiveTimeNs);
                frameLatencyStats.discard(ptsUs, "replaced_before_decode");
                return MoonBridge.DR_OK;
            }
            PyroWaveDisplayClock.Snapshot displayClock =
                    FramePacingPolicy.usesPyroWaveDisplayClock(prefs.framePacing, prefs.vrr, outputVsyncAvailable) ?
                            pyroWaveDisplayClock.snapshot() : PyroWaveDisplayClock.Snapshot.UNAVAILABLE;
            long submissionResult = pyroWaveRenderer.submitFrame(decodeUnitData, decodeUnitLength, ptsUs,
                    presentationTimeUs, displayClock, frameLatencyStats,
                    context, prefs.phonePerformanceHints, refreshRate);
            activeWindowVideoStats.decoderTimeMs += pyroWaveSubmissionTiming.addDurationNs(
                    pyroWaveRenderer.getLastSubmissionDurationNs());
            float recordLoss = pyroWaveRenderer.getLastRecordLossPercent();
            if (recordLoss >= 0) {
                pyroWaveLossTotal += recordLoss;
                pyroWaveLossSamples++;
            }
            activeWindowVideoStats.totalFrames++;
            activeWindowVideoStats.totalFramesReceived++;
            frameLatencyStats.onFrameReceived(receiveTimeNs);
            if (submissionResult == PyroWaveDecoderRenderer.SUBMIT_ACCEPTED) {
                pyroWaveDecodedFrames++;
                pyroWaveFailures = 0;
            } else if (submissionResult == PyroWaveDecoderRenderer.SUBMIT_BUSY) {
                // Native validated this frame before declining it for capacity.
                pyroWaveFailures = 0;
                pyroWaveSkippedFrames++;
                frameLatencyStats.discard(ptsUs, "replaced_before_decode");
            } else {
                frameLatencyStats.discard(ptsUs, submissionResult < 0 ? "decode_failed" : "invalid_frame");
                if (submissionResult < 0 || ++pyroWaveFailures == 60) {
                    LimeLog.severe("PyroWave decoder stopped producing frames");
                    stopping = true;
                    MoonBridge.bridgeClConnectionTerminated(MoonBridge.ML_ERROR_NO_VIDEO_FRAME);
                }
            }
            // Intra-only: recover on the next complete frame, without IDR request storms.
            return MoonBridge.DR_OK;
        }

        boolean csdSubmittedForThisFrame = false;

        // IDR frames require special handling for CSD buffer submission
        if (frameType == MoonBridge.FRAME_TYPE_IDR) {
            // H264 SPS
            if (decodeUnitType == MoonBridge.BUFFER_TYPE_SPS && videoFormat == MoonBridge.VIDEO_FORMAT_H264) {
                numSpsIn++;

                ByteBuffer spsBuf = ByteBuffer.wrap(decodeUnitData);
                int startSeqLen = decodeUnitData[2] == 0x01 ? 3 : 4;

                // Skip to the start of the NALU data
                spsBuf.position(startSeqLen + 1);

                // The H264Utils.readSPS function safely handles
                // Annex B NALUs (including NALUs with escape sequences)
                SeqParameterSet sps = H264Utils.readSPS(spsBuf);

                // Some decoders rely on H264 level to decide how many buffers are needed
                // Since we only need one frame buffered, we'll set the level as low as we can
                // for known resolution combinations. Reference frame invalidation may need
                // these, so leave them be for those decoders.
                if (!refFrameInvalidationActive && Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                    if (initialWidth <= 720 && initialHeight <= 480 && refreshRate <= 60) {
                        // Max 5 buffered frames at 720x480x60
                        LimeLog.info("Patching level_idc to 31");
                        sps.levelIdc = 31;
                    }
                    else if (initialWidth <= 1280 && initialHeight <= 720 && refreshRate <= 60) {
                        // Max 5 buffered frames at 1280x720x60
                        LimeLog.info("Patching level_idc to 32");
                        sps.levelIdc = 32;
                    }
                    else if (initialWidth <= 1920 && initialHeight <= 1080 && refreshRate <= 60) {
                        // Max 4 buffered frames at 1920x1080x64
                        LimeLog.info("Patching level_idc to 42");
                        sps.levelIdc = 42;
                    }
                    else {
                        // Leave the profile alone (currently 5.0)
                    }
                }

                // TI OMAP4 requires a reference frame count of 1 to decode successfully. Exynos 4
                // also requires this fixup.
                //
                // I'm doing this fixup for all devices because I haven't seen any devices that
                // this causes issues for. At worst, it seems to do nothing and at best it fixes
                // issues with video lag, hangs, and crashes.
                //
                // It does break reference frame invalidation, so we will not do that for decoders
                // where we've enabled reference frame invalidation.
                if (!refFrameInvalidationActive) {
                    LimeLog.info("Patching num_ref_frames in SPS");
                    sps.numRefFrames = 1;
                }

                // GFE 2.5.11 changed the SPS to add additional extensions. Some devices don't like these
                // so we remove them here on old devices unless these devices also support HEVC.
                // See getPreferredColorSpace() for further information.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O &&
                        sps.vuiParams != null &&
                        hevcDecoder == null &&
                        av1Decoder == null) {
                    sps.vuiParams.videoSignalTypePresentFlag = false;
                    sps.vuiParams.colourDescriptionPresentFlag = false;
                    sps.vuiParams.chromaLocInfoPresentFlag = false;
                }

                // Some older devices used to choke on a bitstream restrictions, so we won't provide them
                // unless explicitly whitelisted. For newer devices, leave the bitstream restrictions present.
                if (needsSpsBitstreamFixup || isExynos4 || Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    // The SPS that comes in the current H264 bytestream doesn't set bitstream_restriction_flag
                    // or max_dec_frame_buffering which increases decoding latency on Tegra.

                    // If the encoder didn't include VUI parameters in the SPS, add them now
                    if (sps.vuiParams == null) {
                        LimeLog.info("Adding VUI parameters");
                        sps.vuiParams = new VUIParameters();
                    }

                    // GFE 2.5.11 started sending bitstream restrictions
                    if (sps.vuiParams.bitstreamRestriction == null) {
                        LimeLog.info("Adding bitstream restrictions");
                        sps.vuiParams.bitstreamRestriction = new VUIParameters.BitstreamRestriction();
                        sps.vuiParams.bitstreamRestriction.motionVectorsOverPicBoundariesFlag = true;
                        sps.vuiParams.bitstreamRestriction.maxBytesPerPicDenom = 2;
                        sps.vuiParams.bitstreamRestriction.maxBitsPerMbDenom = 1;
                        sps.vuiParams.bitstreamRestriction.log2MaxMvLengthHorizontal = 16;
                        sps.vuiParams.bitstreamRestriction.log2MaxMvLengthVertical = 16;
                        sps.vuiParams.bitstreamRestriction.numReorderFrames = 0;
                    }
                    else {
                        LimeLog.info("Patching bitstream restrictions");
                    }

                    // Some devices throw errors if maxDecFrameBuffering < numRefFrames
                    sps.vuiParams.bitstreamRestriction.maxDecFrameBuffering = sps.numRefFrames;

                    // These values are the defaults for the fields, but they are more aggressive
                    // than what GFE sends in 2.5.11, but it doesn't seem to cause picture problems.
                    // We'll leave these alone for "modern" devices just in case they care.
                    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                        sps.vuiParams.bitstreamRestriction.maxBytesPerPicDenom = 2;
                        sps.vuiParams.bitstreamRestriction.maxBitsPerMbDenom = 1;
                    }

                    // log2_max_mv_length_horizontal and log2_max_mv_length_vertical are set to more
                    // conservative values by GFE 2.5.11. We'll let those values stand.
                }
                else if (sps.vuiParams != null) {
                    // Devices that didn't/couldn't get bitstream restrictions before GFE 2.5.11
                    // will continue to not receive them now
                    sps.vuiParams.bitstreamRestriction = null;
                }

                // If we need to hack this SPS to say we're baseline, do so now
                if (needsBaselineSpsHack) {
                    LimeLog.info("Hacking SPS to baseline");
                    sps.profileIdc = 66;
                    savedSps = sps;
                }

                // Patch the SPS constraint flags
                doProfileSpecificSpsPatching(sps);

                // The H264Utils.writeSPS function safely handles
                // Annex B NALUs (including NALUs with escape sequences)
                ByteBuffer escapedNalu = H264Utils.writeSPS(sps, decodeUnitLength);

                // Construct the patched SPS
                byte[] naluBuffer = new byte[startSeqLen + 1 + escapedNalu.limit()];
                System.arraycopy(decodeUnitData, 0, naluBuffer, 0, startSeqLen + 1);
                escapedNalu.get(naluBuffer, startSeqLen + 1, escapedNalu.limit());

                // Batch this to submit together with other CSD per AOSP docs
                spsBuffers.add(naluBuffer);
                return MoonBridge.DR_OK;
            }
            else if (decodeUnitType == MoonBridge.BUFFER_TYPE_VPS) {
                numVpsIn++;

                // Batch this to submit together with other CSD per AOSP docs
                byte[] naluBuffer = new byte[decodeUnitLength];
                System.arraycopy(decodeUnitData, 0, naluBuffer, 0, decodeUnitLength);
                vpsBuffers.add(naluBuffer);
                return MoonBridge.DR_OK;
            }
            // Preserve HEVC and H.264 4:4:4 SPS data without the 4:2:0 fixups above.
            else if (decodeUnitType == MoonBridge.BUFFER_TYPE_SPS) {
                numSpsIn++;

                // Batch this to submit together with other CSD per AOSP docs
                byte[] naluBuffer = new byte[decodeUnitLength];
                System.arraycopy(decodeUnitData, 0, naluBuffer, 0, decodeUnitLength);
                spsBuffers.add(naluBuffer);
                return MoonBridge.DR_OK;
            }
            else if (decodeUnitType == MoonBridge.BUFFER_TYPE_PPS) {
                numPpsIn++;

                // Batch this to submit together with other CSD per AOSP docs
                byte[] naluBuffer = new byte[decodeUnitLength];
                System.arraycopy(decodeUnitData, 0, naluBuffer, 0, decodeUnitLength);
                ppsBuffers.add(naluBuffer);
                return MoonBridge.DR_OK;
            }
            else if ((videoFormat & (MoonBridge.VIDEO_FORMAT_MASK_H264 | MoonBridge.VIDEO_FORMAT_MASK_H265)) != 0) {
                // If this is the first CSD blob or we aren't supporting fused IDR frames, we will
                // submit the CSD blob in a separate input buffer for each IDR frame.
                if (!submittedCsd || !fusedIdrFrame) {
                    if (!fetchNextInputBuffer()) {
                        return MoonBridge.DR_NEED_IDR;
                    }

                    // Submit all CSD when we receive the first non-CSD blob in an IDR frame
                    for (byte[] vpsBuffer : vpsBuffers) {
                        nextInputBuffer.put(vpsBuffer);
                    }
                    for (byte[] spsBuffer : spsBuffers) {
                        nextInputBuffer.put(spsBuffer);
                    }
                    for (byte[] ppsBuffer : ppsBuffers) {
                        nextInputBuffer.put(ppsBuffer);
                    }

                    if (!queueNextInputBuffer(0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, 0, 0, 0, (char) 0)) {
                        return MoonBridge.DR_NEED_IDR;
                    }

                    // Remember that we already submitted CSD for this frame, so we don't do it
                    // again in the fused IDR case below.
                    csdSubmittedForThisFrame = true;

                    // Remember that we submitted CSD globally for this MediaCodec instance
                    submittedCsd = true;

                    if (needsBaselineSpsHack) {
                        needsBaselineSpsHack = false;

                        if (!replaySps()) {
                            return MoonBridge.DR_NEED_IDR;
                        }

                        LimeLog.info("SPS replay complete");
                    }
                }
            }
        }

        if (frameHostProcessingLatency != 0) {
            if (activeWindowVideoStats.minHostProcessingLatency != 0) {
                activeWindowVideoStats.minHostProcessingLatency = (char) Math.min(activeWindowVideoStats.minHostProcessingLatency, frameHostProcessingLatency);
            } else {
                activeWindowVideoStats.minHostProcessingLatency = frameHostProcessingLatency;
            }
            activeWindowVideoStats.framesWithHostProcessingLatency += 1;
        }
        activeWindowVideoStats.maxHostProcessingLatency = (char) Math.max(activeWindowVideoStats.maxHostProcessingLatency, frameHostProcessingLatency);
        activeWindowVideoStats.totalHostProcessingLatency += frameHostProcessingLatency;

        activeWindowVideoStats.totalFramesReceived++;
        frameLatencyStats.onFrameReceived(receiveTimeNs);
        activeWindowVideoStats.totalFrames++;

        if (!fetchNextInputBuffer()) {
            return MoonBridge.DR_NEED_IDR;
        }

        int codecFlags = 0;

        if (frameType == MoonBridge.FRAME_TYPE_IDR) {
            codecFlags |= MediaCodec.BUFFER_FLAG_SYNC_FRAME;

            // If we are using fused IDR frames, submit the CSD with each IDR frame
            if (fusedIdrFrame && !csdSubmittedForThisFrame) {
                for (byte[] vpsBuffer : vpsBuffers) {
                    nextInputBuffer.put(vpsBuffer);
                }
                for (byte[] spsBuffer : spsBuffers) {
                    nextInputBuffer.put(spsBuffer);
                }
                for (byte[] ppsBuffer : ppsBuffers) {
                    nextInputBuffer.put(ppsBuffer);
                }
            }
        }

        // The frame timestamps use an undefined epoch, so normalize them to uptime microseconds.
        if (baseTimestampUs == 0) {
            baseTimestampUs = (SystemClock.uptimeMillis() * 1000) - enqueueTimeUs;
        }

        long timestampUs = baseTimestampUs + enqueueTimeUs;
        if (timestampUs <= lastTimestampUs) {
            // We can't submit multiple buffers with the same timestamp
            // so bump it up by one before queuing
            timestampUs = lastTimestampUs + 1;
        }
        lastTimestampUs = timestampUs;

        numFramesIn++;

        if (decodeUnitLength > nextInputBuffer.limit() - nextInputBuffer.position()) {
            IllegalArgumentException exception = new IllegalArgumentException(
                    "Decode unit length "+decodeUnitLength+" too large for input buffer "+nextInputBuffer.limit());
            if (!reportedCrash) {
                reportedCrash = true;
                crashListener.notifyCrash(exception);
            }
            throw new RendererException(this, exception);
        }

        // Copy data from our buffer list into the input buffer
        nextInputBuffer.put(decodeUnitData, 0, decodeUnitLength);

        if (!queueNextInputBuffer(timestampUs, codecFlags, frameNumber, receiveTimeNs, enqueueTimeNs, frameHostProcessingLatency)) {
            return MoonBridge.DR_NEED_IDR;
        }

        return MoonBridge.DR_OK;
    }

    private boolean replaySps() {
        if (!fetchNextInputBuffer()) {
            return false;
        }

        // Write the Annex B header
        nextInputBuffer.put(new byte[]{0x00, 0x00, 0x00, 0x01, 0x67});

        // Switch the H264 profile back to high
        savedSps.profileIdc = 100;

        // Patch the SPS constraint flags
        doProfileSpecificSpsPatching(savedSps);

        // The H264Utils.writeSPS function safely handles
        // Annex B NALUs (including NALUs with escape sequences)
        ByteBuffer escapedNalu = H264Utils.writeSPS(savedSps, 128);
        nextInputBuffer.put(escapedNalu);

        // No need for the SPS anymore
        savedSps = null;

        // Queue the new SPS
        return queueNextInputBuffer(0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, 0, 0, 0, (char) 0);
    }

    @Override
    public int getCapabilities() {
        int capabilities = 0;

        // Request the optimal number of slices per frame for this decoder
        capabilities |= MoonBridge.CAPABILITY_SLICES_PER_FRAME(optimalSlicesPerFrame);

        // Enable reference frame invalidation on supported hardware
        if (refFrameInvalidationAvc) {
            capabilities |= MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC;
        }
        if (refFrameInvalidationHevc) {
            capabilities |= MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC;
        }
        if (refFrameInvalidationAv1) {
            capabilities |= MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
        }

        // Enable direct submit on supported hardware
        if (directSubmit) {
            capabilities |= MoonBridge.CAPABILITY_DIRECT_SUBMIT;
        }

        return capabilities;
    }

    public int getAverageEndToEndLatency() {
        return frameLatencyStats.getAverageEndToEndLatency();
    }

    /** Same post-stream counter as Moonlight X for PyroWave; existing Artemis count otherwise. */
    public int getAverageDecoderLatency() {
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            return globalVideoStats.getAverageDecoderLatencyMs();
        }
        return globalVideoStats.getAverageDecoderLatencyMs(activeWindowVideoStats,
                frameLatencyStats.getArtemisDecodeMs());
    }

    static class DecoderHungException extends RuntimeException {
        private int hangTimeMs;

        DecoderHungException(int hangTimeMs) {
            this.hangTimeMs = hangTimeMs;
        }

        public String toString() {
            String str = "";

            str += "Hang time: "+hangTimeMs+" ms"+ RendererException.DELIMITER;
            str += super.toString();

            return str;
        }
    }

    static class RendererException extends RuntimeException {
        private static final long serialVersionUID = 8985937536997012406L;
        protected static final String DELIMITER = BuildConfig.DEBUG ? "\n" : " | ";

        private String text;

        RendererException(MediaCodecDecoderRenderer renderer, Exception e) {
            super(e);
            this.text = generateText(renderer, e);
        }

        public String toString() {
            return text;
        }

        private String generateText(MediaCodecDecoderRenderer renderer, Exception originalException) {
            String str;

            if (renderer.numVpsIn == 0 && renderer.numSpsIn == 0 && renderer.numPpsIn == 0) {
                str = "PreSPSError";
            }
            else if (renderer.numSpsIn > 0 && renderer.numPpsIn == 0) {
                str = "PrePPSError";
            }
            else if (renderer.numPpsIn > 0 && renderer.numFramesIn == 0) {
                str = "PreIFrameError";
            }
            else if (renderer.numFramesIn > 0 && renderer.outputFormat == null) {
                str = "PreOutputConfigError";
            }
            else if (renderer.outputFormat != null && renderer.numFramesOut == 0) {
                str = "PreOutputError";
            }
            else if (renderer.numFramesOut <= renderer.refreshRate * 30) {
                str = "EarlyOutputError";
            }
            else {
                str = "ErrorWhileStreaming";
            }

            str += "Format: "+String.format("%x", renderer.videoFormat)+DELIMITER;
            str += "AVC Decoder: "+((renderer.avcDecoder != null) ? renderer.avcDecoder.getName():"(none)")+DELIMITER;
            str += "HEVC Decoder: "+((renderer.hevcDecoder != null) ? renderer.hevcDecoder.getName():"(none)")+DELIMITER;
            str += "AV1 Decoder: "+((renderer.av1Decoder != null) ? renderer.av1Decoder.getName():"(none)")+DELIMITER;
            if (renderer.avcDecoder != null) {
                Range<Integer> avcWidthRange = renderer.avcDecoder.getCapabilitiesForType("video/avc").getVideoCapabilities().getSupportedWidths();
                str += "AVC supported width range: "+avcWidthRange+DELIMITER;
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    try {
                        Range<Double> avcFpsRange = renderer.avcDecoder.getCapabilitiesForType("video/avc").getVideoCapabilities().getAchievableFrameRatesFor(renderer.initialWidth, renderer.initialHeight);
                        str += "AVC achievable FPS range: "+avcFpsRange+DELIMITER;
                    } catch (IllegalArgumentException e) {
                        str += "AVC achievable FPS range: UNSUPPORTED!"+DELIMITER;
                    }
                }
            }
            if (renderer.hevcDecoder != null) {
                Range<Integer> hevcWidthRange = renderer.hevcDecoder.getCapabilitiesForType("video/hevc").getVideoCapabilities().getSupportedWidths();
                str += "HEVC supported width range: "+hevcWidthRange+DELIMITER;
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    try {
                        Range<Double> hevcFpsRange = renderer.hevcDecoder.getCapabilitiesForType("video/hevc").getVideoCapabilities().getAchievableFrameRatesFor(renderer.initialWidth, renderer.initialHeight);
                        str += "HEVC achievable FPS range: " + hevcFpsRange + DELIMITER;
                    } catch (IllegalArgumentException e) {
                        str += "HEVC achievable FPS range: UNSUPPORTED!"+DELIMITER;
                    }
                }
            }
            if (renderer.av1Decoder != null) {
                Range<Integer> av1WidthRange = renderer.av1Decoder.getCapabilitiesForType("video/av01").getVideoCapabilities().getSupportedWidths();
                str += "AV1 supported width range: "+av1WidthRange+DELIMITER;
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    try {
                        Range<Double> av1FpsRange = renderer.av1Decoder.getCapabilitiesForType("video/av01").getVideoCapabilities().getAchievableFrameRatesFor(renderer.initialWidth, renderer.initialHeight);
                        str += "AV1 achievable FPS range: " + av1FpsRange + DELIMITER;
                    } catch (IllegalArgumentException e) {
                        str += "AV1 achievable FPS range: UNSUPPORTED!"+DELIMITER;
                    }
                }
            }
            str += "Configured format: "+renderer.configuredFormat+DELIMITER;
            str += "Input format: "+renderer.inputFormat+DELIMITER;
            str += "Output format: "+renderer.outputFormat+DELIMITER;
            str += "Adaptive playback: "+renderer.adaptivePlayback+DELIMITER;
            str += "GL Renderer: "+renderer.glRenderer+DELIMITER;
            //str += "Build fingerprint: "+Build.FINGERPRINT+DELIMITER;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                str += "SOC: "+Build.SOC_MANUFACTURER+" - "+Build.SOC_MODEL+DELIMITER;
                str += "Performance class: "+Build.VERSION.MEDIA_PERFORMANCE_CLASS+DELIMITER;
                /*str += "Vendor params: ";
                List<String> params = renderer.videoDecoder.getSupportedVendorParameters();
                if (params.isEmpty()) {
                    str += "NONE";
                }
                else {
                    for (String param : params) {
                        str += param + " ";
                    }
                }
                str += DELIMITER;*/
            }
            str += "Consecutive crashes: "+renderer.consecutiveCrashCount+DELIMITER;
            str += "RFI active: "+renderer.refFrameInvalidationActive+DELIMITER;
            str += "Using modern SPS patching: "+(Build.VERSION.SDK_INT >= Build.VERSION_CODES.O)+DELIMITER;
            str += "Fused IDR frames: "+renderer.fusedIdrFrame+DELIMITER;
            str += "Video dimensions: "+renderer.initialWidth+"x"+renderer.initialHeight+DELIMITER;
            str += "FPS target: "+renderer.refreshRate+DELIMITER;
            str += "Bitrate: "+renderer.prefs.bitrate+" Kbps"+DELIMITER;
            str += "CSD stats: "+renderer.numVpsIn+", "+renderer.numSpsIn+", "+renderer.numPpsIn+DELIMITER;
            str += "Frames in-out: "+renderer.numFramesIn+", "+renderer.numFramesOut+DELIMITER;
            str += "Total frames received: "+renderer.globalVideoStats.totalFramesReceived+DELIMITER;
            str += "Total frames rendered: "+renderer.globalVideoStats.totalFramesRendered+DELIMITER;
            str += "Frame losses: "+renderer.globalVideoStats.framesLost+" in "+renderer.globalVideoStats.frameLossEvents+" loss events"+DELIMITER;
            str += "Average end-to-end client latency: "+renderer.getAverageEndToEndLatency()+"ms"+DELIMITER;
            str += "Average hardware decoder latency: "+renderer.frameLatencyStats.getAverageDecoderLatency()+"ms"+DELIMITER;
            str += "Frame pacing mode: "+renderer.prefs.framePacing+DELIMITER;

            if (originalException instanceof CodecException) {
                CodecException ce = (CodecException) originalException;

                str += "Diagnostic Info: " + ce.getDiagnosticInfo() + DELIMITER;
                str += "Recoverable: " + ce.isRecoverable() + DELIMITER;
                str += "Transient: " + ce.isTransient() + DELIMITER;

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    str += "Codec Error Code: " + ce.getErrorCode() + DELIMITER;
                }
            }

            str += originalException.toString();

            return str;
        }
    }
}
