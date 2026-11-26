package com.limelight.nvstream.jni;

import com.limelight.LimeLog;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PackedFallback;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;

import java.nio.ByteBuffer;
import java.util.function.BooleanSupplier;

public class MoonBridge extends MoonBridgeForwarders {
    /* See documentation in Limelight.h for information about these functions and constants */

    public static final short LI_CCAP_ANALOG_TRIGGERS = 0x01;
    public static final short LI_CCAP_RUMBLE          = 0x02;
    public static final short LI_CCAP_TRIGGER_RUMBLE  = 0x04;
    public static final short LI_CCAP_TOUCHPAD        = 0x08;
    public static final short LI_CCAP_ACCEL           = 0x10;
    public static final short LI_CCAP_GYRO            = 0x20;
    public static final short LI_CCAP_BATTERY_STATE   = 0x40;
    public static final short LI_CCAP_RGB_LED         = 0x80;
    public static final short LI_CCAP_DUAL_TOUCHPAD   = 0x100;
    public static final short LI_CCAP_HAPTICS_PCM     = (short)0x8000;

    private static volatile AudioRenderer audioRenderer;
    private static volatile VideoDecoderRenderer videoRenderer;
    private static volatile NvConnectionListener connectionListener;
    private static volatile BooleanSupplier connectionCancelled;
    // Whether the listener was told that this start failed or that its connection ended.
    private static volatile boolean startFailureReported;
    private static volatile boolean packed444Requested;
    private static volatile int networkGracePeriodMs;

    static {
        // The slim native library: AAudio output, USB audio for the DualSense and the controller tables.
        System.loadLibrary("moonlight-core");
        // Not the Rust engine's library: its init() initialises this class, so loading it from here
        // would take RustBridge's lock under this class's init lock while another thread in
        // RustBridge.load() waits the other way round. The forwarders load it on first use.
    }

    public static int bridgeDrSetup(int videoFormat, int width, int height, int redrawRate) {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) {
            // This stream's DESCRIBE has the PC's packed modes now: they name why 10-bit was not picked.
            int packedModes = getHostPacked444Modes();
            if (packedModes > 0) renderer.setHostPacked444Modes(packedModes);
            // And the ANNOUNCE is out: the renderer decodes the HDR format it actually asked for.
            renderer.setAnnouncedHdrFormat(getAnnouncedHdrFormat());
            return renderer.setup(videoFormat, width, height, redrawRate);
        }
        else {
            return -1;
        }
    }

    public static int bridgeDrSetupPacking(int version, int width, int height, int colorSpace, int colorRange) {
        VideoDecoderRenderer renderer = videoRenderer;
        return renderer == null ? -1 : renderer.setChromaPacking(version, width, height, colorSpace, colorRange);
    }

    public static void bridgeClDisplayMessage(String message) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.displayMessage(message);
    }

    public static void bridgeDrStart() {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) renderer.start();
    }

    public static void bridgeDrStop() {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) renderer.stop();
    }

    public static void bridgeDrCleanup() {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) renderer.cleanup();
    }

    public static void bridgeDrSubmitChunks(byte[] data, int length, boolean first) {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) renderer.submitChunks(data, length, first);
    }

    // The engine's chunk batch in place; see VideoDecoderRenderer.submitChunksDirect.
    public static void bridgeDrSubmitChunksDirect(ByteBuffer data, int length, boolean first) {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer != null) renderer.submitChunksDirect(data, length, first);
    }

    // Raw native pointer fast path for PyroWave chunks (avoids NewDirectByteBuffer upcalls).
    public static void drSubmitChunksRaw(long dataPtr, int length, boolean first) {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer instanceof MediaCodecDecoderRenderer) {
            ((MediaCodecDecoderRenderer) renderer).submitChunksRaw(dataPtr, length, first);
        }
    }

    public static int bridgeDrSubmitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                               int frameNumber, int frameType, char frameHostProcessingLatency,
                                               long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                               long presentationTimeUs) {
        VideoDecoderRenderer renderer = videoRenderer;
        return renderer == null ? DR_OK : renderer.submitDecodeUnit(decodeUnitData, decodeUnitLength,
                decodeUnitType, frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs,
                receiveTimeNs, presentationTimeUs);
    }

    // The engine's picture data in place; see VideoDecoderRenderer.submitDirectDecodeUnit.
    public static int bridgeDrSubmitDirectDecodeUnit(ByteBuffer decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                                     int frameNumber, int frameType, char frameHostProcessingLatency,
                                                     long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                                     long presentationTimeUs) {
        VideoDecoderRenderer renderer = videoRenderer;
        return renderer == null ? DR_OK : renderer.submitDirectDecodeUnit(decodeUnitData, decodeUnitLength,
                decodeUnitType, frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs,
                receiveTimeNs, presentationTimeUs);
    }

    // Raw native pointer fast path for PyroWave pictures (avoids NewDirectByteBuffer upcalls).
    public static int drSubmitRawDecodeUnit(long dataPtr, int decodeUnitLength, int decodeUnitType,
                                            int frameNumber, int frameType, char frameHostProcessingLatency,
                                            long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                            long presentationTimeUs) {
        VideoDecoderRenderer renderer = videoRenderer;
        if (renderer instanceof MediaCodecDecoderRenderer) {
            return ((MediaCodecDecoderRenderer) renderer).submitRawDecodeUnit(dataPtr, decodeUnitLength,
                    decodeUnitType, frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs,
                    receiveTimeNs, presentationTimeUs);
        }
        return DR_OK;
    }

    public static int bridgeArInit(int audioConfiguration, int sampleRate, int samplesPerFrame) {
        AudioRenderer renderer = audioRenderer;
        return renderer == null ? -1 : renderer.setup(new AudioConfiguration(audioConfiguration), sampleRate, samplesPerFrame);
    }

    public static void bridgeArStart() {
        AudioRenderer renderer = audioRenderer;
        if (renderer != null) renderer.start();
    }

    public static void bridgeArStop() {
        AudioRenderer renderer = audioRenderer;
        if (renderer != null) renderer.stop();
    }

    public static void bridgeArCleanup() {
        AudioRenderer renderer = audioRenderer;
        if (renderer != null) renderer.cleanup();
    }

    public static void bridgeArPlaySample(short[] pcmData, int length) {
        AudioRenderer renderer = audioRenderer;
        if (renderer != null) renderer.playDecodedAudio(pcmData, length);
    }

    // Direct native-memory stereo AAudio fast path (zero short[] copy, zero getPendingAudioDuration JNI).
    public static int arPlaySampleDirect(long pcmPtr, int length, int pendingMs) {
        AudioRenderer renderer = audioRenderer;
        if (renderer instanceof AndroidAudioRenderer) {
            return ((AndroidAudioRenderer) renderer).playDecodedAudioDirect(pcmPtr, length, pendingMs);
        }
        return -1;
    }

    // Fallback short[] path with pending audio duration forwarded from Rust (zero getPendingAudioDuration JNI).
    public static void arPlaySamplePending(short[] pcmData, int length, int pendingMs) {
        AudioRenderer renderer = audioRenderer;
        if (renderer instanceof AndroidAudioRenderer) {
            ((AndroidAudioRenderer) renderer).playDecodedAudio(pcmData, length, pendingMs);
        } else if (renderer != null) {
            renderer.playDecodedAudio(pcmData, length);
        }
    }

    public static int bridgeArInitEncoded(int audioConfiguration, int codec) {
        AudioRenderer renderer = audioRenderer;
        return renderer == null ? -1 : renderer.setupEncoded(new AudioConfiguration(audioConfiguration), codec);
    }

    public static void bridgeArPlayEncoded(byte[] frame, int length) {
        AudioRenderer renderer = audioRenderer;
        if (renderer != null) renderer.playEncodedAudio(frame, length);
    }

    public static void bridgeClStageStarting(int stage) {
        // LiStartConnection resets its interrupt flag before this first callback.
        BooleanSupplier cancelled = connectionCancelled;
        if (cancelled != null && cancelled.getAsBoolean()) {
            interruptConnection();
            return;
        }
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.stageStarting(getStageName(stage));
        }
    }

    public static void bridgeClStageComplete(int stage) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.stageComplete(getStageName(stage));
        }
    }

    /**
     * Whether the listener heard, since {@link #setupBridge}, that the start failed or the
     * connection ended. A start that returns a failure without either has told nobody.
     */
    public static boolean startFailureReported() {
        return startFailureReported;
    }

    public static void markStartFailureReported(NvConnectionListener listener) {
        if (listener != null && listener == connectionListener) {
            startFailureReported = true;
        }
    }

    public static void bridgeClStageFailed(int stage, int errorCode) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            startFailureReported = true;
            listener.stageFailed(getStageName(stage), getPortFlagsFromStage(stage), errorCode);
        }
    }

    public static void bridgeClConnectionStarted() {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.connectionStarted();
        }
    }

    public static void bridgeClConnectionTerminated(int errorCode) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            startFailureReported = true;
            listener.connectionTerminated(errorCode);
        }
    }

    public static void bridgeClRumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.rumble(controllerNumber, lowFreqMotor, highFreqMotor);
        }
    }

    public static void bridgeClConnectionStatusUpdate(int connectionStatus) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.connectionStatusUpdate(connectionStatus);
        }
    }

    public static void bridgeClSetHdrMode(boolean enabled, byte[] hdrMetadata) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.setHdrMode(enabled, hdrMetadata);
        }
    }

    public static void bridgeClRumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.rumbleTriggers(controllerNumber, leftTrigger, rightTrigger);
        }
    }

    public static void bridgeClSetMotionEventState(short controllerNumber, byte eventType, short sampleRateHz) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.setMotionEventState(controllerNumber, eventType, sampleRateHz);
        }
    }

    public static void bridgeClSetControllerLED(short controllerNumber, byte r, byte g, byte b) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.setControllerLED(controllerNumber, r, g, b);
        }
    }

    public static void bridgeClSetAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                                   byte typeRight, byte[] left, byte[] right) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.setAdaptiveTriggers(controllerNumber, eventFlags, typeLeft, typeRight, left, right);
        }
    }

    public static void bridgeClControllerHaptics(short controllerNumber, int sequence, byte[] pcm) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.controllerHaptics(controllerNumber, sequence, pcm);
        }
    }

    // Steam Controller (2026) haptic report (0x5504) and settings write (0x5505). A host sends them only for a controller that advertised LI_CCAP_STEAM_HAPTIC or
    // LI_CCAP_STEAM_SETTINGS, which Nightjar never does yet.
    public static void bridgeClSteamHaptic(short controllerNumber, byte[] report) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.steamHaptic(controllerNumber, report);
    }

    public static void bridgeClSteamSettings(short controllerNumber, byte command, byte[] triples) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.steamSettings(controllerNumber, command, triples);
    }

    // Host cursor shape message (0x5541, docs/protocol/cursor-shape.md), on the control stream's
    // receive thread; only between an accepted sendCursorShapeRequest(true) and the next false.
    public static void bridgeClCursorShape(byte[] message) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.cursorShape(message);
    }

    public static void bridgeClUsbChanged() {
        Runnable listener = usbListener;
        if (listener != null) listener.run();
    }

    public static native long openUsbMicrophone(int fd, int endpoint, int interval);
    public static native int readUsbMicrophone(long handle, byte[] samples);

    public static void bridgeClLinkNotice(int state, int ceilingKbps, int currentKbps) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.linkNotice(state, ceilingKbps, currentKbps);
    }

    public static void bridgeClRateChange(int serial, int bitrateKbps, int fpsMillihz, int bitrateStatus,
                                          int bitrateReason, int fpsStatus, int fpsReason) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.rateChange(new RateChangeResult(serial, bitrateKbps, fpsMillihz, bitrateStatus,
                    bitrateReason, fpsStatus, fpsReason));
        }
    }

    public static void bridgeClDvrState(int requestId, int state, int reason, int bufferedSecs,
                                        int capacitySecs, int recordingSecs, long quotaUsedBytes,
                                        long quotaLimitBytes) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.dvrState(requestId, state, reason, bufferedSecs, capacitySecs, recordingSecs,
                    quotaUsedBytes, quotaLimitBytes);
        }
    }

    public static void bridgeClDvrClip(int requestId, int outcome, int reason, int durationSecs,
                                       long fileBytes) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.dvrClip(requestId, outcome, reason, durationSecs, fileBytes);
    }

    public static void bridgeClTextInput(int kind, int inputType, int flags, int element) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.textInput(kind, inputType, flags, element);
    }

    public static void bridgeClClipboardSync(String text) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.clipboardSync(text);
    }

    public static void bridgeClClipboardSyncRefused(int reason) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.clipboardSyncRefused(reason);
    }

    public static void bridgeClHostTiming(byte[] payload) {
        VideoDecoderRenderer renderer = videoRenderer;
        HostTimingReport report = HostTimingReport.parse(payload);
        if (renderer != null && report != null) renderer.onHostTiming(report);
    }

    public static void bridgeClControllerAudio(int kind, byte[] payload) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) listener.controllerAudio(kind, payload);
    }

    public static native long openUsbHaptics(int fd, int endpoint, int interval);
    public static native boolean writeUsbHaptics(long handle, byte[] samples);
    public static native void closeUsbHaptics(long handle);
    public static native void restoreUsbAudio(int fd, int controlInterface);

    public static void setupBridge(VideoDecoderRenderer videoRenderer, AudioRenderer audioRenderer,
                                   NvConnectionListener connectionListener, BooleanSupplier connectionCancelled) {
        MoonBridge.videoRenderer = videoRenderer;
        MoonBridge.audioRenderer = audioRenderer;
        MoonBridge.connectionListener = connectionListener;
        MoonBridge.connectionCancelled = connectionCancelled;
        startFailureReported = false;
    }

    public static void cleanupBridge() {
        MoonBridge.videoRenderer = null;
        MoonBridge.audioRenderer = null;
        MoonBridge.connectionListener = null;
        MoonBridge.connectionCancelled = null;
    }

    public static void requestPacked444(boolean requested) {
        packed444Requested = requested;
    }

    /**
     * A start that cannot go on, found before the engine ran a stage. The failure is reported as an
     * engine's would be: nothing else ends the connecting screen, which shows {@code message}. The
     * start's result stays -1, so what the PC offers is not forgotten for a refusal that is not its own.
     */
    private static int refuseStart(String stage, int errorCode, String message) {
        NvConnectionListener listener = connectionListener;
        if (listener != null) {
            listener.displayMessage(message);
            startFailureReported = true;
            listener.stageFailed(stage, 0, errorCode);
        }
        return -1;
    }

    /**
     * Full chroma cannot start on this device, found before the engine ran a stage: reported as the
     * engine's refusal would be, and the session reconnects without full chroma as it does when a PC
     * refuses it.
     */
    private static int refusePacked444(String message) {
        return refuseStart(PackedFallback.CHECK_STAGE, CLIENT_ERROR_PACKED_444_REFUSED, message);
    }

    public static int startConnection(String address, String appVersion,
                                      String rtspSessionUrl, int serverCodecModeSupport,
                                      int width, int height, int fps,
                                      int bitrate, int packetSize, int streamingRemotely,
                                      int audioConfiguration, int supportedVideoFormats,
                                      int clientRefreshRateX100,
                                      byte[] riAesKey, byte[] riAesIv,
                                      int videoCapabilities,
                                      int colorSpace, int colorRange,
                                      boolean unbatchedInput, boolean networkPriority, int hostFeatures,
                                      boolean inputOnly, boolean remoteMonitor, boolean enableMic,
                                      boolean hapticsPcm,
                                      int pyrowaveLinkMbps, int pyrowavePaceMbps, boolean clientWireless) {
        // There is one engine: a stream it cannot start fails here, on the connecting screen, saying why.
        String refusal = RustBridge.startRefusal(appVersion, serverCodecModeSupport, inputOnly);
        StreamEngine.record(refusal);
        if (refusal != null) {
            LimeLog.warning("Streaming engine: " + StreamEngine.summary());
            return refuseStart(ENGINE_STAGE, CLIENT_ERROR_ENGINE_UNAVAILABLE, refusal);
        }
        LimeLog.info("Streaming engine: " + StreamEngine.summary());
        int packedModes = 0;
        if (packed444Requested) {
            VideoDecoderRenderer renderer = videoRenderer;
            packedModes = renderer == null ? 0 :
                    renderer.preparePacked444Modes(supportedVideoFormats, width, height, fps);
            if (packedModes == 0) {
                return refusePacked444("Full chroma requires a hardware decoder for the coded size/rate and verified raw 8/10-bit GL_EXT_YUV_target sampling.");
            }
        }
        return RustBridge.startConnection(address, appVersion, rtspSessionUrl, serverCodecModeSupport,
                width, height, fps, bitrate, packetSize, streamingRemotely, audioConfiguration,
                supportedVideoFormats, clientRefreshRateX100, riAesKey, riAesIv, videoCapabilities,
                colorSpace, colorRange, hostFeatures, inputOnly, enableMic, hapticsPcm,
                pyrowaveLinkMbps, pyrowavePaceMbps, clientWireless, networkGracePeriodMs, packed444Requested, packedModes,
                remoteMonitor, unbatchedInput, networkPriority, roamingRequested, roamingMeteredRequested,
                audioPassthroughRequested, hdrFormatRequested, videoFecRequested, pyrowaveEarlyRequested,
                codecDirectRequested, receiveDecodePriorityRequested, frameWarpRequested,
                receiveCoalescingRequested, decodeQueueBound);
    }

    /** Call before startConnection(); 0 keeps the native 10 second control timeout. */
    public static void setNetworkGracePeriodMs(int gracePeriodMs) {
        networkGracePeriodMs = gracePeriodMs;
    }

    public static boolean sendServerCommand(byte[] payload) {
        return RustBridge.isAvailable() && RustBridge.sendServerCommand(payload);
    }

    // Frames the engine dropped because its decode queue overflowed (the overflowing frame and those
    // dropped until the key frame it asked for), since the stream started: local, not network loss.
    public static long getDecoderDrops() {
        return RustBridge.isAvailable() ? RustBridge.getDecoderDrops() : 0L;
    }

    /**
     * The flags of the stream's ports this device cannot reach on the test server, 0 for none, or
     * ML_TEST_RESULT_INCONCLUSIVE when the test could not run (also when the Rust library did not load).
     */
    public static int testClientConnectivity(String testServerHostName, int referencePort, int testFlags) {
        return RustBridge.isAvailable() ? RustBridge.testClientConnectivity(testServerHostName, referencePort, testFlags)
                : ML_TEST_RESULT_INCONCLUSIVE;
    }

    public static int getPortFlagsFromTerminationErrorCode(int errorCode) {
        return PortFlags.fromTerminationErrorCode(errorCode);
    }

    public static String stringifyPortFlags(int portFlags, String separator) {
        return PortFlags.stringify(portFlags, separator);
    }

    // The RTT is in the top 32 bits, and the RTT variance is in the bottom 32 bits
    public static long getEstimatedRttInfo() {
        return RustBridge.isAvailable() ? RustBridge.getEstimatedRttInfo() : -1L;
    }

    public static native byte guessControllerType(int vendorId, int productId);

    public static native boolean guessControllerHasPaddles(int vendorId, int productId);

    public static native boolean guessControllerHasShareButton(int vendorId, int productId);

    // Phase lock: reports how long a frame waited from ready to the next display latch, so a
    // Rubylight host can time frames to land just before it.
    // Collects locally; true means a new report is ready for the background sender.
    public static boolean reportFrameSlack(long slackNs, long periodNs) {
        return RustBridge.isAvailable() && RustBridge.reportFrameSlack(slackNs, periodNs);
    }

    public static void flushPhaseLockReport() {
        if (RustBridge.isAvailable()) RustBridge.flushPhaseLockReport();
    }

    // The video socket's receive buffer in bytes: asked in the top 32 bits, what the system granted in
    // the bottom. -1 when unknown.
    public static long getVideoReceiveBufferInfo() {
        return RustBridge.isAvailable() ? RustBridge.getVideoReceiveBufferInfo() : -1L;
    }

    // Whether this connection's host named the control message in its DESCRIBE answer
    // (a=x-rl-control, Rubylight 2.2.0+). False before the RTSP handshake and for other hosts.
    public static boolean hostSupportsControlMessage(int messageType) {
        return RustBridge.isAvailable() && RustBridge.hostSupportsControlMessage(messageType);
    }

    /**
     * The packed full-chroma modes the PC's DESCRIBE offered this stream (0 without), from the end
     * of the RTSP handshake on; -1 before it.
     */
    public static int getHostPacked444Modes() {
        return RustBridge.isAvailable() ? RustBridge.getHostPacked444Modes() : -1;
    }

    public static int getMicrophoneState() {
        return RustBridge.isAvailable() ? RustBridge.getMicrophoneState() : MIC_STATE_OFF;
    }

    // Asks the host to continue at width x height and fpsMillihz / 1000 fps; it answers with an
    // IDR at the new size. Only call when hostSupportsControlMessage(CONTROL_MESSAGE_RECONFIGURE).
    // Returns false when the request could not be sent.
    public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
        return RustBridge.isAvailable() && RustBridge.requestReconfigure(width, height, fpsMillihz);
    }

    // PresentClock.schedule onto a display's vsync grid (present_clock.rs schedule_on_grid). out:
    // release time (ns), vsync (ns), vsync index from the anchor, flags (1 late, 2 shares the
    // previous frame's vsync), hold (us), host-to-vsync latency (us, -1 without clock sync) and
    // how late the frame was (us).
    public static boolean presentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out) {
        return RustBridge.isAvailable() && RustBridge.presentClockScheduleOnGrid(hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs, out);
    }
}
