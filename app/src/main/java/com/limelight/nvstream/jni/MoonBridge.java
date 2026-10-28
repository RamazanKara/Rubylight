package com.limelight.nvstream.jni;

import com.limelight.LimeLog;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;

import java.util.function.BooleanSupplier;

public class MoonBridge {
    /* See documentation in Limelight.h for information about these functions and constants */

    public static final AudioConfiguration AUDIO_CONFIGURATION_STEREO = new AudioConfiguration(2, 0x3);
    public static final AudioConfiguration AUDIO_CONFIGURATION_51_SURROUND = new AudioConfiguration(6, 0x3F);
    public static final AudioConfiguration AUDIO_CONFIGURATION_71_SURROUND = new AudioConfiguration(8, 0x63F);

    public static final int VIDEO_FORMAT_H264 = 0x0001;
    public static final int VIDEO_FORMAT_H264_HIGH8_444 = 0x0004;
    public static final int VIDEO_FORMAT_H265 = 0x0100;
    public static final int VIDEO_FORMAT_H265_MAIN10 = 0x0200;
    public static final int VIDEO_FORMAT_H265_REXT8_444 = 0x0400;
    public static final int VIDEO_FORMAT_H265_REXT10_444 = 0x0800;
    public static final int VIDEO_FORMAT_AV1_MAIN8 = 0x1000;
    public static final int VIDEO_FORMAT_AV1_MAIN10 = 0x2000;
    public static final int VIDEO_FORMAT_AV1_HIGH8_444 = 0x4000;
    public static final int VIDEO_FORMAT_AV1_HIGH10_444 = 0x8000;
    public static final int VIDEO_FORMAT_PYROWAVE = 0x010000;
    public static final int VIDEO_FORMAT_PYROWAVE_444 = 0x020000;
    public static final int VIDEO_FORMAT_PYROWAVE_MAIN10 = 0x040000;
    public static final int VIDEO_FORMAT_PYROWAVE_MAIN10_444 = 0x080000;

    public static final int VIDEO_FORMAT_MASK_H264 = 0x000F;
    public static final int VIDEO_FORMAT_MASK_H265 = 0x0F00;
    public static final int VIDEO_FORMAT_MASK_AV1 = 0xF000;
    public static final int VIDEO_FORMAT_MASK_PYROWAVE = 0x0F0000;
    public static final int VIDEO_FORMAT_MASK_10BIT = 0x0CAA00;
    public static final int VIDEO_FORMAT_MASK_YUV444 = 0x0ACC04;

    public static final int BUFFER_TYPE_PICDATA = 0;
    public static final int BUFFER_TYPE_SPS = 1;
    public static final int BUFFER_TYPE_PPS = 2;
    public static final int BUFFER_TYPE_VPS = 3;

    public static final int FRAME_TYPE_PFRAME = 0;
    public static final int FRAME_TYPE_IDR = 1;

    public static final int COLORSPACE_REC_601 = 0;
    public static final int COLORSPACE_REC_709 = 1;
    public static final int COLORSPACE_REC_2020 = 2;

    public static final int COLOR_RANGE_LIMITED = 0;
    public static final int COLOR_RANGE_FULL = 1;

    public static final int CAPABILITY_DIRECT_SUBMIT = 1;
    public static final int CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC = 2;
    public static final int CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC = 4;
    public static final int CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1 = 0x40;

    public static final int DR_OK = 0;
    public static final int DR_NEED_IDR = -1;

    public static final int CONN_STATUS_OKAY = 0;
    public static final int CONN_STATUS_POOR = 1;

    public static final int ML_ERROR_GRACEFUL_TERMINATION = 0;
    public static final int ML_ERROR_NO_VIDEO_TRAFFIC = -100;
    public static final int ML_ERROR_NO_VIDEO_FRAME = -101;
    public static final int ML_ERROR_UNEXPECTED_EARLY_TERMINATION = -102;
    public static final int ML_ERROR_PROTECTED_CONTENT = -103;
    public static final int ML_ERROR_FRAME_CONVERSION = -104;
    // This device's own code, never sent by a PC: the optional Vulkan renderer failed mid-stream
    public static final int CLIENT_ERROR_VULKAN_RENDERER = -105;
    // The New engine's start code (nightjar-core error::PACKED_444_REFUSED), never sent by a PC:
    // experimental full chroma cannot be streamed with this PC (no packed mode both sides offer,
    // or the PC refused it), so the stream may reconnect without it
    public static final int CLIENT_ERROR_PACKED_444_REFUSED = -106;

    public static final int ML_PORT_INDEX_TCP_47984 = 0;
    public static final int ML_PORT_INDEX_TCP_47989 = 1;
    public static final int ML_PORT_INDEX_TCP_48010 = 2;
    public static final int ML_PORT_INDEX_UDP_47998 = 8;
    public static final int ML_PORT_INDEX_UDP_47999 = 9;
    public static final int ML_PORT_INDEX_UDP_48000 = 10;
    public static final int ML_PORT_INDEX_UDP_48010 = 11;

    public static final int ML_PORT_FLAG_ALL = 0xFFFFFFFF;
    public static final int ML_PORT_FLAG_TCP_47984 = 0x0001;
    public static final int ML_PORT_FLAG_TCP_47989 = 0x0002;
    public static final int ML_PORT_FLAG_TCP_48010 = 0x0004;
    public static final int ML_PORT_FLAG_UDP_47998 = 0x0100;
    public static final int ML_PORT_FLAG_UDP_47999 = 0x0200;
    public static final int ML_PORT_FLAG_UDP_48000 = 0x0400;
    public static final int ML_PORT_FLAG_UDP_48010 = 0x0800;

    public static final int ML_TEST_RESULT_INCONCLUSIVE = 0xFFFFFFFF;

    public static final byte SS_KBE_FLAG_NON_NORMALIZED = 0x01;

    public static final int LI_ERR_UNSUPPORTED = -5501;

    public static final byte LI_TOUCH_EVENT_HOVER       = 0x00;
    public static final byte LI_TOUCH_EVENT_DOWN        = 0x01;
    public static final byte LI_TOUCH_EVENT_UP          = 0x02;
    public static final byte LI_TOUCH_EVENT_MOVE        = 0x03;
    public static final byte LI_TOUCH_EVENT_CANCEL      = 0x04;
    public static final byte LI_TOUCH_EVENT_BUTTON_ONLY = 0x05;
    public static final byte LI_TOUCH_EVENT_HOVER_LEAVE = 0x06;
    public static final byte LI_TOUCH_EVENT_CANCEL_ALL  = 0x07;

    public static final byte LI_TOOL_TYPE_UNKNOWN = 0x00;
    public static final byte LI_TOOL_TYPE_PEN = 0x01;
    public static final byte LI_TOOL_TYPE_ERASER = 0x02;

    public static final byte LI_PEN_BUTTON_PRIMARY = 0x01;
    public static final byte LI_PEN_BUTTON_SECONDARY = 0x02;
    public static final byte LI_PEN_BUTTON_TERTIARY = 0x04;

    public static final byte LI_TILT_UNKNOWN = (byte)0xFF;
    public static final short LI_ROT_UNKNOWN = (short)0xFFFF;

    public static final byte LI_CTYPE_UNKNOWN  = 0x00;
    public static final byte LI_CTYPE_XBOX     = 0x01;
    public static final byte LI_CTYPE_PS       = 0x02;
    public static final byte LI_CTYPE_NINTENDO = 0x03;
    public static final byte LI_CTYPE_STEAM    = 0x04;

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

    public static final byte LI_MOTION_TYPE_ACCEL = 0x01;
    public static final byte LI_MOTION_TYPE_GYRO  = 0x02;

    public static final byte LI_BATTERY_STATE_UNKNOWN      = 0x00;
    public static final byte LI_BATTERY_STATE_NOT_PRESENT  = 0x01;
    public static final byte LI_BATTERY_STATE_DISCHARGING  = 0x02;
    public static final byte LI_BATTERY_STATE_CHARGING     = 0x03;
    public static final byte LI_BATTERY_STATE_NOT_CHARGING = 0x04; // Connected to power but not charging
    public static final byte LI_BATTERY_STATE_FULL         = 0x05;

    public static final byte LI_BATTERY_PERCENTAGE_UNKNOWN = (byte)0xFF;

    private static AudioRenderer audioRenderer;
    private static volatile VideoDecoderRenderer videoRenderer;
    private static NvConnectionListener connectionListener;
    private static BooleanSupplier connectionCancelled;

    static {
        System.loadLibrary("moonlight-core");
        init();
    }

    public static int CAPABILITY_SLICES_PER_FRAME(byte slices) {
        return slices << 24;
    }

    public static class AudioConfiguration {
        public final int channelCount;
        public final int channelMask;

        public AudioConfiguration(int channelCount, int channelMask) {
            this.channelCount = channelCount;
            this.channelMask = channelMask;
        }

        // Creates an AudioConfiguration from the integer value returned by moonlight-common-c
        // See CHANNEL_COUNT_FROM_AUDIO_CONFIGURATION() and CHANNEL_MASK_FROM_AUDIO_CONFIGURATION()
        // in Limelight.h
        private AudioConfiguration(int audioConfiguration) {
            // Check the magic byte before decoding to make sure we got something that's actually
            // a MAKE_AUDIO_CONFIGURATION()-based value and not something else like an older version
            // hardcoded AUDIO_CONFIGURATION value from an earlier version of moonlight-common-c.
            if ((audioConfiguration & 0xFF) != 0xCA) {
                throw new IllegalArgumentException("Audio configuration has invalid magic byte!");
            }

            this.channelCount = (audioConfiguration >> 8) & 0xFF;
            this.channelMask = (audioConfiguration >> 16) & 0xFFFF;
        }

        // See SURROUNDAUDIOINFO_FROM_AUDIO_CONFIGURATION() in Limelight.h
        public int getSurroundAudioInfo() {
            return channelMask << 16 | channelCount;
        }

        @Override
        public boolean equals(Object obj) {
            if (obj instanceof AudioConfiguration) {
                AudioConfiguration that = (AudioConfiguration)obj;
                return this.toInt() == that.toInt();
            }

            return false;
        }

        @Override
        public int hashCode() {
            return toInt();
        }

        // Returns the integer value expected by moonlight-common-c
        // See MAKE_AUDIO_CONFIGURATION() in Limelight.h
        public int toInt() {
            return ((channelMask) << 16) | (channelCount << 8) | 0xCA;
        }
    }

    public static int bridgeDrSetup(int videoFormat, int width, int height, int redrawRate) {
        if (videoRenderer != null) {
            // This stream's DESCRIBE has the PC's packed modes now: they name why 10-bit was not picked.
            int packedModes = getHostPacked444Modes();
            if (packedModes > 0) videoRenderer.setHostPacked444Modes(packedModes);
            // And the ANNOUNCE is out: the renderer decodes the HDR format it actually asked for.
            videoRenderer.setAnnouncedHdrFormat(getAnnouncedHdrFormat());
            return videoRenderer.setup(videoFormat, width, height, redrawRate);
        }
        else {
            return -1;
        }
    }

    public static int bridgeDrSetupPacking(int version, int width, int height, int colorSpace, int colorRange) {
        return videoRenderer == null ? -1 : videoRenderer.setChromaPacking(version, width, height, colorSpace, colorRange);
    }

    public static void bridgeClDisplayMessage(String message) {
        if (connectionListener != null) connectionListener.displayMessage(message);
    }

    public static void bridgeDrStart() {
        if (videoRenderer != null) {
            videoRenderer.start();
        }
    }

    public static void bridgeDrStop() {
        if (videoRenderer != null) {
            videoRenderer.stop();
        }
    }

    public static void bridgeDrCleanup() {
        if (videoRenderer != null) {
            videoRenderer.cleanup();
        }
    }

    public static int bridgeDrSubmitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                               int frameNumber, int frameType, char frameHostProcessingLatency,
                                               long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                               long presentationTimeUs) {
        if (videoRenderer != null) {
            return videoRenderer.submitDecodeUnit(decodeUnitData, decodeUnitLength,
                    decodeUnitType, frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs, receiveTimeNs,
                    presentationTimeUs);
        }
        else {
            return DR_OK;
        }
    }

    public static int bridgeArInit(int audioConfiguration, int sampleRate, int samplesPerFrame) {
        if (audioRenderer != null) {
            return audioRenderer.setup(new AudioConfiguration(audioConfiguration), sampleRate, samplesPerFrame);
        }
        else {
            return -1;
        }
    }

    public static void bridgeArStart() {
        if (audioRenderer != null) {
            audioRenderer.start();
        }
    }

    public static void bridgeArStop() {
        if (audioRenderer != null) {
            audioRenderer.stop();
        }
    }

    public static void bridgeArCleanup() {
        if (audioRenderer != null) {
            audioRenderer.cleanup();
        }
    }

    public static void bridgeArPlaySample(short[] pcmData, int length) {
        if (audioRenderer != null) {
            audioRenderer.playDecodedAudio(pcmData, length);
        }
    }

    public static int bridgeArInitEncoded(int audioConfiguration, int codec) {
        if (audioRenderer != null) {
            return audioRenderer.setupEncoded(new AudioConfiguration(audioConfiguration), codec);
        }
        else {
            return -1;
        }
    }

    public static void bridgeArPlayEncoded(byte[] frame, int length) {
        if (audioRenderer != null) {
            audioRenderer.playEncodedAudio(frame, length);
        }
    }

    public static void bridgeClStageStarting(int stage) {
        // LiStartConnection resets its interrupt flag before this first callback.
        if (connectionCancelled != null && connectionCancelled.getAsBoolean()) {
            interruptConnection();
            return;
        }
        if (connectionListener != null) {
            connectionListener.stageStarting(getStageName(stage));
        }
    }

    public static void bridgeClStageComplete(int stage) {
        if (connectionListener != null) {
            connectionListener.stageComplete(getStageName(stage));
        }
    }

    public static void bridgeClStageFailed(int stage, int errorCode) {
        if (connectionListener != null) {
            connectionListener.stageFailed(getStageName(stage), getPortFlagsFromStage(stage), errorCode);
        }
    }

    public static void bridgeClConnectionStarted() {
        if (connectionListener != null) {
            connectionListener.connectionStarted();
        }
    }

    public static void bridgeClConnectionTerminated(int errorCode) {
        if (connectionListener != null) {
            connectionListener.connectionTerminated(errorCode);
        }
    }

    public static void bridgeClRumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        if (connectionListener != null) {
            connectionListener.rumble(controllerNumber, lowFreqMotor, highFreqMotor);
        }
    }

    public static void bridgeClConnectionStatusUpdate(int connectionStatus) {
        if (connectionListener != null) {
            connectionListener.connectionStatusUpdate(connectionStatus);
        }
    }

    public static void bridgeClSetHdrMode(boolean enabled, byte[] hdrMetadata) {
        if (connectionListener != null) {
            connectionListener.setHdrMode(enabled, hdrMetadata);
        }
    }

    public static void bridgeClRumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        if (connectionListener != null) {
            connectionListener.rumbleTriggers(controllerNumber, leftTrigger, rightTrigger);
        }
    }

    public static void bridgeClSetMotionEventState(short controllerNumber, byte eventType, short sampleRateHz) {
        if (connectionListener != null) {
            connectionListener.setMotionEventState(controllerNumber, eventType, sampleRateHz);
        }
    }

    public static void bridgeClSetControllerLED(short controllerNumber, byte r, byte g, byte b) {
        if (connectionListener != null) {
            connectionListener.setControllerLED(controllerNumber, r, g, b);
        }
    }

    public static void bridgeClSetAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                                   byte typeRight, byte[] left, byte[] right) {
        if (connectionListener != null) {
            connectionListener.setAdaptiveTriggers(controllerNumber, eventFlags, typeLeft, typeRight, left, right);
        }
    }

    public static void bridgeClControllerHaptics(short controllerNumber, int sequence, byte[] pcm) {
        if (connectionListener != null) {
            connectionListener.controllerHaptics(controllerNumber, sequence, pcm);
        }
    }

    // Steam Controller (2026) haptic report (0x5504) and settings write (0x5505), Rust engine
    // only. A host sends them only for a controller that advertised LI_CCAP_STEAM_HAPTIC or
    // LI_CCAP_STEAM_SETTINGS, which Nightjar never does yet.
    public static void bridgeClSteamHaptic(short controllerNumber, byte[] report) {
        if (connectionListener != null) connectionListener.steamHaptic(controllerNumber, report);
    }

    public static void bridgeClSteamSettings(short controllerNumber, byte command, byte[] triples) {
        if (connectionListener != null) connectionListener.steamSettings(controllerNumber, command, triples);
    }

    // Host cursor shape message (0x5541, docs/protocol/cursor-shape.md), on the control stream's
    // receive thread; only between an accepted sendCursorShapeRequest(true) and the next false.
    public static void bridgeClCursorShape(byte[] message) {
        if (connectionListener != null) connectionListener.cursorShape(message);
    }

    // USB forwarding (0x553A, docs/control-messages.md), Rust engine only: what
    // usbAttach returns instead of a request id.
    public static final int USB_NO_STREAM = -1;
    public static final int USB_BUSY = -2;
    public static final int USB_UNSUPPORTED = -3;
    public static final int USB_DEVICE_FAILED = -4;
    public static final int USB_INVALID = -5;
    public static final int USB_HOST_FAILED = -6;

    private static volatile Runnable usbListener;

    /** Runs on any thread whenever usbList() may have changed; null stops. */
    public static void setUsbListener(Runnable listener) {
        usbListener = listener;
    }

    public static void bridgeClUsbChanged() {
        Runnable listener = usbListener;
        if (listener != null) listener.run();
    }

    /**
     * Asks the host to attach the device open on fd (a UsbDeviceConnection's, which the caller
     * may close afterwards) at its usbfs path: a request id, or USB_*.
     */
    public static int usbAttach(int fd, String path, String name, HostEndpoint host) {
        if (!rustStream) return USB_NO_STREAM;
        if (host == null) return USB_HOST_FAILED;
        return RustBridge.usbAttach(fd, path, name, host.address, host.httpPort, host.httpsPort,
                host.serverCertificatePem, host.uniqueId, host.clientCertificateDer,
                host.clientPrivateKeyPkcs8);
    }

    /** Detaches an attachment of this stream; false when it is not attached. */
    public static boolean usbDetach(int requestId) {
        return rustStream && RustBridge.usbDetach(requestId);
    }

    /** This stream's attachments as JSON (src/usb.rs's list_json), "[]" without any. */
    public static String usbList() {
        if (!rustStream) return "[]";
        String list = RustBridge.usbList();
        if (list == null) {
            LimeLog.severe("USB: the attachment list did not reach Java");
            return "[]";
        }
        return list;
    }

    public static boolean sendControllerAudio(int kind, byte[] payload) {
        return rustStream ? RustBridge.sendControllerAudio(kind, payload) : cSendControllerAudio(kind, payload);
    }
    private static native boolean cSendControllerAudio(int kind, byte[] payload);
    public static native long openUsbMicrophone(int fd, int endpoint, int interval);
    public static native int readUsbMicrophone(long handle, byte[] samples);
    public static void bridgeClLinkNotice(int state, int ceilingKbps, int currentKbps) {
        if (connectionListener != null) connectionListener.linkNotice(state, ceilingKbps, currentKbps);
    }
    public static void bridgeClRateChange(int serial, int bitrateKbps, int fpsMillihz, int bitrateStatus,
                                          int bitrateReason, int fpsStatus, int fpsReason) {
        if (connectionListener != null) {
            connectionListener.rateChange(new RateChangeResult(serial, bitrateKbps, fpsMillihz, bitrateStatus,
                    bitrateReason, fpsStatus, fpsReason));
        }
    }
    public static void bridgeClTextInput(int kind, int inputType, int flags) {
        if (connectionListener != null) connectionListener.textInput(kind, inputType, flags);
    }
    public static void bridgeClHostTiming(byte[] payload) {
        VideoDecoderRenderer renderer = videoRenderer;
        HostTimingReport report = HostTimingReport.parse(payload);
        if (renderer != null && report != null) renderer.onHostTiming(report);
    }
    public static void bridgeClControllerAudio(int kind, byte[] payload) {
        if (connectionListener != null) connectionListener.controllerAudio(kind, payload);
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
    }

    public static void cleanupBridge() {
        MoonBridge.videoRenderer = null;
        MoonBridge.audioRenderer = null;
        MoonBridge.connectionListener = null;
        MoonBridge.connectionCancelled = null;
    }

    // The streaming engine: moonlight-common-c (C, the default) or nightjar-core (Rust, behind
    // the "New streaming engine" setting). Each stream picks one in startConnection(); every
    // streaming call below goes to that engine until the next start.
    private static volatile boolean rustEngineRequested;
    private static volatile boolean packed444Requested;
    private static volatile int hdrFormatRequested;
    private static volatile boolean rustStream;
    private static volatile int networkGracePeriodMs;
    private static volatile boolean roamingRequested = true;
    private static volatile boolean roamingMeteredRequested;
    private static volatile int audioPassthroughRequested;

    /** Call before startConnection(): whether the next stream should use the Rust engine. */
    public static void requestRustEngine(boolean requested) {
        rustEngineRequested = requested;
    }

    /**
     * Call before startConnection(): "Client roaming (Experimental)" and "Continue on mobile
     * data" for the next stream. Only the Rust engine roams.
     */
    public static void requestRoaming(boolean enabled, boolean metered) {
        roamingRequested = enabled;
        roamingMeteredRequested = metered;
    }

    /**
     * Call before startConnection(): the passthrough codecs (AudioPassthrough.CODEC_* bits) the
     * audio route plays, or 0 for Opus. Only the Rust engine asks the host for them.
     */
    public static void requestAudioPassthrough(int codecs) {
        audioPassthroughRequested = codecs;
    }

    /** Whether the running stream (or the last one) uses the Rust engine. */
    public static boolean isRustStream() {
        return rustStream;
    }

    /**
     * The default network changed while a stream runs; metered is the new network's
     * !NET_CAPABILITY_NOT_METERED. Returns whether a roam started; false on the C engine.
     */
    public static boolean networkChanged(boolean metered) {
        return rustStream && RustBridge.networkChanged(metered);
    }

    public static void requestPacked444(boolean requested) {
        packed444Requested = requested;
    }

    /**
     * Call before startConnection(): the one HdrFormatPolicy bit the next stream asks a Rubylight
     * 3.0 host for, 0 for stock HDR10. The ANNOUNCE carries it only when the DESCRIBE offered it.
     */
    public static void requestHdrFormat(int format) {
        hdrFormatRequested = format;
    }

    /** The HDR format this stream's ANNOUNCE asked for, 0 for stock (and before the ANNOUNCE). */
    public static int getAnnouncedHdrFormat() {
        // Nothing asked keeps the native libraries out of it (and out of tests without them).
        if (hdrFormatRequested == 0) return 0;
        return rustStream ? RustBridge.getAnnouncedHdrFormat() : cGetAnnouncedHdrFormat();
    }
    private static native int cGetAnnouncedHdrFormat();

    public static int startConnection(String address, String appVersion, String gfeVersion,
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
        boolean rust = false;
        int packedModes = 0;
        if (packed444Requested && !rustEngineRequested) {
            StreamEngine.record(false, false, null);
            bridgeClDisplayMessage("Experimental full chroma requires the New streaming engine setting.");
            return -1;
        }
        String reason = null;
        if (rustEngineRequested) {
            reason = RustBridge.fallbackReason(appVersion, serverCodecModeSupport, inputOnly);
            if (reason == null && !RustBridge.isAvailable()) {
                reason = "libnightjar_core.so did not load";
            }
            rust = reason == null;
        }
        StreamEngine.record(rustEngineRequested, rust, reason);
        if (rustEngineRequested && !rust && packed444Requested) {
            LimeLog.warning("Streaming engine: " + StreamEngine.summary());
            bridgeClDisplayMessage("Experimental full chroma is unavailable: " + reason);
            return -1;
        }
        // Warnings stay in the redacted problem report whatever its level filter; a fallback
        // must be visible there, not only in logcat.
        if (rustEngineRequested && !rust) {
            LimeLog.warning("Streaming engine: " + StreamEngine.summary());
        } else {
            LimeLog.info("Streaming engine: " + StreamEngine.summary());
        }
        if (audioPassthroughRequested != 0 && !rust) {
            LimeLog.warning("Audio passthrough: needs the New streaming engine; this stream plays Opus");
            bridgeClDisplayMessage("Audio passthrough needs the New streaming engine. This stream plays Opus.");
        }
        rustStream = rust;
        if (rust) {
            if (packed444Requested) {
                packedModes = videoRenderer == null ? 0 :
                        videoRenderer.preparePacked444Modes(supportedVideoFormats, width, height, fps);
                if (packedModes == 0) {
                    bridgeClDisplayMessage("Experimental full chroma requires a hardware decoder for the coded size/rate and verified raw 8/10-bit GL_EXT_YUV_target sampling.");
                    return -1;
                }
            }
            return RustBridge.startConnection(address, appVersion, rtspSessionUrl, serverCodecModeSupport,
                    width, height, fps, bitrate, packetSize, streamingRemotely, audioConfiguration,
                    supportedVideoFormats, clientRefreshRateX100, riAesKey, riAesIv, videoCapabilities,
                    colorSpace, colorRange, hostFeatures, inputOnly, enableMic, hapticsPcm,
                    pyrowaveLinkMbps, pyrowavePaceMbps, clientWireless, networkGracePeriodMs, packed444Requested, packedModes,
                    remoteMonitor, unbatchedInput, networkPriority, roamingRequested, roamingMeteredRequested,
                    audioPassthroughRequested, hdrFormatRequested);
        }
        return cStartConnection(address, appVersion, gfeVersion, rtspSessionUrl, serverCodecModeSupport,
                width, height, fps, bitrate, packetSize, streamingRemotely, audioConfiguration,
                supportedVideoFormats, clientRefreshRateX100, riAesKey, riAesIv, videoCapabilities,
                colorSpace, colorRange, unbatchedInput, networkPriority, hostFeatures, inputOnly,
                remoteMonitor, enableMic, hapticsPcm, pyrowaveLinkMbps, pyrowavePaceMbps,
                clientWireless, hdrFormatRequested);
    }
    private static native int cStartConnection(String address, String appVersion, String gfeVersion,
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
                                               int pyrowaveLinkMbps, int pyrowavePaceMbps,
                                               boolean clientWireless, int hdrFormat);

    /** Call before startConnection(); 0 keeps the native 10 second control timeout. */
    public static void setNetworkGracePeriodMs(int gracePeriodMs) {
        networkGracePeriodMs = gracePeriodMs;
        cSetNetworkGracePeriodMs(gracePeriodMs);
    }
    private static native void cSetNetworkGracePeriodMs(int gracePeriodMs);

    public static void stopConnection() {
        if (rustStream) RustBridge.stopConnection(); else cStopConnection();
    }
    private static native void cStopConnection();

    public static void interruptConnection() {
        if (rustStream) RustBridge.interruptConnection(); else cInterruptConnection();
    }
    private static native void cInterruptConnection();

    public static void sendMouseMove(short deltaX, short deltaY) {
        if (rustStream) RustBridge.sendMouseMove(deltaX, deltaY); else cSendMouseMove(deltaX, deltaY);
    }
    private static native void cSendMouseMove(short deltaX, short deltaY);

    public static void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight) {
        if (rustStream) RustBridge.sendMousePosition(x, y, referenceWidth, referenceHeight); else cSendMousePosition(x, y, referenceWidth, referenceHeight);
    }
    private static native void cSendMousePosition(short x, short y, short referenceWidth, short referenceHeight);

    public static void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight) {
        if (rustStream) RustBridge.sendMouseMoveAsMousePosition(deltaX, deltaY, referenceWidth, referenceHeight); else cSendMouseMoveAsMousePosition(deltaX, deltaY, referenceWidth, referenceHeight);
    }
    private static native void cSendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight);

    public static void sendMouseButton(byte buttonEvent, byte mouseButton) {
        if (rustStream) RustBridge.sendMouseButton(buttonEvent, mouseButton); else cSendMouseButton(buttonEvent, mouseButton);
    }
    private static native void cSendMouseButton(byte buttonEvent, byte mouseButton);

    public static void sendMultiControllerInput(short controllerNumber,
                                    short activeGamepadMask, int buttonFlags,
                                    byte leftTrigger, byte rightTrigger,
                                    short leftStickX, short leftStickY,
                                    short rightStickX, short rightStickY) {
        if (rustStream) RustBridge.sendMultiControllerInput(controllerNumber, activeGamepadMask, buttonFlags, leftTrigger, rightTrigger, leftStickX, leftStickY, rightStickX, rightStickY); else cSendMultiControllerInput(controllerNumber, activeGamepadMask, buttonFlags, leftTrigger, rightTrigger, leftStickX, leftStickY, rightStickX, rightStickY);
    }
    private static native void cSendMultiControllerInput(short controllerNumber,
                                    short activeGamepadMask, int buttonFlags,
                                    byte leftTrigger, byte rightTrigger,
                                    short leftStickX, short leftStickY,
                                    short rightStickX, short rightStickY);

    public static int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure,
                                            float contactAreaMajor, float contactAreaMinor, short rotation) {
        return rustStream ? RustBridge.sendTouchEvent(eventType, pointerId, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation) : cSendTouchEvent(eventType, pointerId, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation);
    }
    private static native int cSendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure,
                                            float contactAreaMajor, float contactAreaMinor, short rotation);

    public static int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y,
                                          float pressure, float contactAreaMajor, float contactAreaMinor,
                                          short rotation, byte tilt) {
        return rustStream ? RustBridge.sendPenEvent(eventType, toolType, penButtons, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation, tilt) : cSendPenEvent(eventType, toolType, penButtons, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation, tilt);
    }
    private static native int cSendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y,
                                          float pressure, float contactAreaMajor, float contactAreaMinor,
                                          short rotation, byte tilt);

    public static int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities) {
        return rustStream ? RustBridge.sendControllerArrivalEvent(controllerNumber, activeGamepadMask, type, supportedButtonFlags, capabilities) : cSendControllerArrivalEvent(controllerNumber, activeGamepadMask, type, supportedButtonFlags, capabilities);
    }
    private static native int cSendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities);

    public static int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure) {
        return rustStream ? RustBridge.sendControllerTouchEvent(controllerNumber, eventType, pointerId, x, y, pressure) : cSendControllerTouchEvent(controllerNumber, eventType, pointerId, x, y, pressure);
    }
    private static native int cSendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure);

    public static int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z) {
        return rustStream ? RustBridge.sendControllerMotionEvent(controllerNumber, motionType, x, y, z) : cSendControllerMotionEvent(controllerNumber, motionType, x, y, z);
    }
    private static native int cSendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z);

    public static int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage) {
        return rustStream ? RustBridge.sendControllerBatteryEvent(controllerNumber, batteryState, batteryPercentage) : cSendControllerBatteryEvent(controllerNumber, batteryState, batteryPercentage);
    }
    private static native int cSendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage);

    public static void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags) {
        if (rustStream) RustBridge.sendKeyboardInput(keyMap, keyDirection, modifier, flags); else cSendKeyboardInput(keyMap, keyDirection, modifier, flags);
    }
    private static native void cSendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags);

    public static void sendMouseHighResScroll(short scrollAmount) {
        if (rustStream) RustBridge.sendMouseHighResScroll(scrollAmount); else cSendMouseHighResScroll(scrollAmount);
    }
    private static native void cSendMouseHighResScroll(short scrollAmount);

    public static void sendMouseHighResHScroll(short scrollAmount) {
        if (rustStream) RustBridge.sendMouseHighResHScroll(scrollAmount); else cSendMouseHighResHScroll(scrollAmount);
    }
    private static native void cSendMouseHighResHScroll(short scrollAmount);

    public static void sendUtf8Text(byte[] text) {
        if (rustStream) RustBridge.sendUtf8Text(text); else cSendUtf8Text(text);
    }
    private static native void cSendUtf8Text(byte[] text);
    public static boolean sendServerCommand(byte[] payload) {
        return rustStream ? RustBridge.sendServerCommand(payload) : cSendServerCommand(payload);
    }
    private static native boolean cSendServerCommand(byte[] payload);

    public static native String getStageName(int stage);

    public static native String findExternalAddressIP4(String stunHostName, int stunPort);

    public static int getPendingAudioDuration() {
        return rustStream ? RustBridge.getPendingAudioDuration() : cGetPendingAudioDuration();
    }
    private static native int cGetPendingAudioDuration();

    public static int getPendingVideoFrames() {
        return rustStream ? RustBridge.getPendingVideoFrames() : cGetPendingVideoFrames();
    }
    private static native int cGetPendingVideoFrames();

    public static native int testClientConnectivity(String testServerHostName, int referencePort, int testFlags);

    public static native int getPortFlagsFromStage(int stage);

    public static native int getPortFlagsFromTerminationErrorCode(int errorCode);

    public static native String stringifyPortFlags(int portFlags, String separator);

    // The RTT is in the top 32 bits, and the RTT variance is in the bottom 32 bits
    public static long getEstimatedRttInfo() {
        return rustStream ? RustBridge.getEstimatedRttInfo() : cGetEstimatedRttInfo();
    }
    private static native long cGetEstimatedRttInfo();

    public static native String getLaunchUrlQueryParameters();

    public static native byte guessControllerType(int vendorId, int productId);

    public static native boolean guessControllerHasPaddles(int vendorId, int productId);

    public static native boolean guessControllerHasShareButton(int vendorId, int productId);

    public static native void init();

    // Phase lock: reports how long a frame waited from ready to the next display latch, so a
    // Rubylight host can time frames to land just before it.
    // Collects locally; true means a new report is ready for the background sender.
    public static boolean reportFrameSlack(long slackNs, long periodNs) {
        return rustStream ? RustBridge.reportFrameSlack(slackNs, periodNs) : cReportFrameSlack(slackNs, periodNs);
    }
    private static native boolean cReportFrameSlack(long slackNs, long periodNs);
    public static void flushPhaseLockReport() {
        if (rustStream) RustBridge.flushPhaseLockReport(); else cFlushPhaseLockReport();
    }
    private static native void cFlushPhaseLockReport();

    // Early edge of the last phase-lock report in microseconds before the latch, or
    // Integer.MIN_VALUE before the first one.
    public static int getPhaseLockLeadUs() {
        return rustStream ? RustBridge.getPhaseLockLeadUs() : cGetPhaseLockLeadUs();
    }
    private static native int cGetPhaseLockLeadUs();

    // Whether this connection's host named the control message in its DESCRIBE answer
    // (a=x-rl-control, Rubylight 2.2.0+). False before the RTSP handshake and for other hosts.
    public static boolean hostSupportsControlMessage(int messageType) {
        return rustStream ? RustBridge.hostSupportsControlMessage(messageType) : cHostSupportsControlMessage(messageType);
    }
    private static native boolean cHostSupportsControlMessage(int messageType);

    // Display HDR luminance, client to host (protocol/core/src/control.rs DisplayCaps).
    public static final int CONTROL_MESSAGE_DISPLAY_CAPS = 0x5531;

    // Sends the display's HDR luminance in nits (0 when unknown) on the encrypted control
    // stream; false when the host did not announce 0x5531 or the stream is not up.
    public static boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits) {
        return rustStream ? RustBridge.sendDisplayCaps(hdr, maxNits, maxAverageNits, minNits) : cSendDisplayCaps(hdr, maxNits, maxAverageNits, minNits);
    }
    private static native boolean cSendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits);

    // Microphone state for this session (android_microphone.h).
    public static final int MIC_STATE_OFF = 0;
    public static final int MIC_STATE_NOT_OFFERED = 1;
    public static final int MIC_STATE_UNAVAILABLE = 2;
    public static final int MIC_STATE_READY = 3;

    /**
     * The packed full-chroma modes the PC's DESCRIBE offered this stream (0 without), from the end
     * of the RTSP handshake on; -1 before it and with the C engine, which does not read them.
     */
    public static int getHostPacked444Modes() {
        return rustStream ? RustBridge.getHostPacked444Modes() : -1;
    }

    public static int getMicrophoneState() {
        return rustStream ? RustBridge.getMicrophoneState() : cGetMicrophoneState();
    }
    private static native int cGetMicrophoneState();

    // Encodes one frame of 48 kHz mono PCM as Opus and sends it to the host. Returns the
    // datagram length, or a negative value when the frame was not sent.
    public static int sendMicrophonePcm(short[] pcm, int samples) {
        return rustStream ? RustBridge.sendMicrophonePcm(pcm, samples) : cSendMicrophonePcm(pcm, samples);
    }
    private static native int cSendMicrophonePcm(short[] pcm, int samples);

    // Rubylight 2.2 control message: change resolution or frame rate mid-stream.
    public static final int CONTROL_MESSAGE_RECONFIGURE = 0x5532;

    // Asks the host to continue at width x height and fpsMillihz / 1000 fps; it answers with an
    // IDR at the new size. Only call when hostSupportsControlMessage(CONTROL_MESSAGE_RECONFIGURE).
    // Returns false when the request could not be sent.
    public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
        return rustStream ? RustBridge.requestReconfigure(width, height, fpsMillihz) : cRequestReconfigure(width, height, fpsMillihz);
    }
    private static native boolean cRequestReconfigure(int width, int height, int fpsMillihz);

    // Host timing, host to client (protocol/core/src/control.rs HostTimingRequest and HostTiming).
    public static final int CONTROL_MESSAGE_HOST_TIMING = HostTimingReport.MESSAGE_TYPE;

    // Asks the host to report each frame's present-to-sent time (or stop). Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_HOST_TIMING); false when it could not be sent.
    public static boolean sendHostTimingRequest(boolean enabled) {
        return rustStream ? RustBridge.sendHostTimingRequest(enabled) : cSendHostTimingRequest(enabled);
    }
    private static native boolean cSendHostTimingRequest(boolean enabled);

    // Link notice (0x5538, docs/control-messages.md in the Rubylight host): the host says when it
    // lowers the video bitrate because of the network. Only the Rust engine asks for it; the C
    // engine does not support it and this returns false.
    public static final int CONTROL_MESSAGE_LINK_NOTICE = 0x5538;

    public static boolean sendLinkNoticeRequest(boolean enabled) {
        return rustStream && RustBridge.sendLinkNoticeRequest(enabled);
    }

    // Clock sync (0x5537) and timestamp presentation (protocol/core/src/present_clock.rs); see
    // android_present_clock.c. Times are System.nanoTime().
    public static final int CONTROL_MESSAGE_CLOCK_SYNC = 0x5537;

    // Sends one probe to a host that announced clock sync; false otherwise.
    public static boolean sendClockSyncProbe() {
        return rustStream ? RustBridge.sendClockSyncProbe() : cSendClockSyncProbe();
    }
    private static native boolean cSendClockSyncProbe();
    public static void presentClockStart(int minIntervalUs) {
        if (rustStream) RustBridge.presentClockStart(minIntervalUs); else cPresentClockStart(minIntervalUs);
    }
    private static native void cPresentClockStart(int minIntervalUs);
    public static void presentClockStop() {
        if (rustStream) RustBridge.presentClockStop(); else cPresentClockStop();
    }
    private static native void cPresentClockStop();
    public static void presentClockReset() {
        if (rustStream) RustBridge.presentClockReset(); else cPresentClockReset();
    }
    private static native void cPresentClockReset();
    // out: present time (ns), hold (us), host-to-present latency (us, -1 without clock sync), late.
    public static boolean presentClockSchedule(long hostUs, long readyNs, long[] out) {
        return rustStream ? RustBridge.presentClockSchedule(hostUs, readyNs, out) : cPresentClockSchedule(hostUs, readyNs, out);
    }
    private static native boolean cPresentClockSchedule(long hostUs, long readyNs, long[] out);
    // out: frames scheduled, frames late, clock sync round trip (us, -1 without).
    public static boolean presentClockStats(long[] out) {
        return rustStream ? RustBridge.presentClockStats(out) : cPresentClockStats(out);
    }
    private static native boolean cPresentClockStats(long[] out);
    // out: the host-to-client clock offset in us now (client = host + offset, the client clock
    // being CLOCK_MONOTONIC like System.nanoTime()) and the round trip in us of the exchange it
    // comes from. False before the first clock sync exchange or without a running clock.
    public static boolean presentClockOffset(long[] out) {
        return rustStream ? RustBridge.presentClockOffset(out) : cPresentClockOffset(out);
    }
    private static native boolean cPresentClockOffset(long[] out);
    // PresentClock.schedule onto a display's vsync grid (present_clock.rs schedule_on_grid). out:
    // release time (ns), vsync (ns), vsync index from the anchor, flags (1 late, 2 shares the
    // previous frame's vsync), hold (us), host-to-vsync latency (us, -1 without clock sync) and
    // how late the frame was (us).
    public static boolean presentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out) {
        return rustStream
                ? RustBridge.presentClockScheduleOnGrid(hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs, out)
                : cPresentClockScheduleOnGrid(hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs, out);
    }
    private static native boolean cPresentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs,
            long intervalNs, long leadNs, long slackNs, long[] out);
    // Notes a frame's host time (us) for the content rate; the rate in millihertz once it held
    // steady for two seconds, else 0.
    public static void presentClockObserve(long hostUs) {
        if (rustStream) RustBridge.presentClockObserve(hostUs); else cPresentClockObserve(hostUs);
    }
    private static native void cPresentClockObserve(long hostUs);
    public static int presentClockContentRate() {
        return rustStream ? RustBridge.presentClockContentRate() : cPresentClockContentRate();
    }
    private static native int cPresentClockContentRate();

    // Host cursor shapes, client to host (docs/protocol/cursor-shape.md).
    public static final int CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST = 0x5540;

    // The 3.0 extensions (protocol/core/src/ids.rs). Defined so hostSupportsControlMessage can ask
    // for them. MoonBridgeControlMessagesTest holds these and the cursor shape request to test
    // resources protocol/control-messages.txt, which nightjar-core checks too.
    public static final int CONTROL_MESSAGE_FEC_REPORT = 0x5539;
    public static final int CONTROL_MESSAGE_USB = 0x553A;
    public static final int CONTROL_MESSAGE_TEXT_INPUT = 0x553B;
    public static final int CONTROL_MESSAGE_RATE_CHANGE = 0x553C;
    public static final int CONTROL_MESSAGE_PIPELINE_RATES = 0x553D;
    public static final int CONTROL_MESSAGE_KEYBOARD_LAYOUT = 0x553E;

    // Asks the host to send its cursor image and leave it out of the video (true), or to stop
    // (false). Only call when hostSupportsControlMessage(CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST).
    // Returns false when the request could not be sent.
    public static boolean sendCursorShapeRequest(boolean enabled) {
        return rustStream ? RustBridge.sendCursorShapeRequest(enabled) : cSendCursorShapeRequest(enabled);
    }
    private static native boolean cSendCursorShapeRequest(boolean enabled);

    // Asks the host for text field notices (0x553B; textInput on the listener), or to stop. Only
    // call when hostSupportsControlMessage(CONTROL_MESSAGE_TEXT_INPUT). The C engine does not
    // support it and this returns false.
    public static boolean sendTextInputRequest(boolean enabled) {
        return rustStream && RustBridge.sendTextInputRequest(enabled);
    }

    // Asks the host for a bitrate in kbps, a rate in mHz or both (0x553C; rateChange on the
    // listener with the same serial), 0 keeping that value. Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_RATE_CHANGE) on a Rust stream; false when it
    // could not be sent or a value is out of range. The C engine does not support it.
    public static boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz) {
        return rustStream && RustBridge.sendRateChange(serial, bitrateKbps, fpsMillihz);
    }

    // Reports the client's keyboard layout as a BCP 47 tag (0x553E), so the host types keys the
    // client did not normalize in that layout. Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_KEYBOARD_LAYOUT); false when it could not be
    // sent or the tag is not a valid one (empty, over 64 characters, not printable ASCII).
    public static boolean sendKeyboardLayout(String bcp47Tag) {
        return rustStream && RustBridge.sendKeyboardLayout(bcp47Tag);
    }
}
