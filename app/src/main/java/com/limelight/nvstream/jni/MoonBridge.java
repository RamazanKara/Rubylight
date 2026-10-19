package com.limelight.nvstream.jni;

import com.limelight.nvstream.NvConnectionListener;
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
            return videoRenderer.setup(videoFormat, width, height, redrawRate);
        }
        else {
            return -1;
        }
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

    // Host cursor shape message (0x5541, docs/protocol/cursor-shape.md), on the control stream's
    // receive thread; only between an accepted sendCursorShapeRequest(true) and the next false.
    public static void bridgeClCursorShape(byte[] message) {
        if (connectionListener != null) connectionListener.cursorShape(message);
    }

    public static native boolean sendControllerAudio(int kind, byte[] payload);
    public static native long openUsbMicrophone(int fd, int endpoint, int interval);
    public static native int readUsbMicrophone(long handle, byte[] samples);
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

    public static native int startConnection(String address, String appVersion, String gfeVersion,
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
                                              int pyrowaveLinkMbps, int pyrowavePaceMbps);

    /** Call before startConnection(); 0 keeps the native 10 second control timeout. */
    public static native void setNetworkGracePeriodMs(int gracePeriodMs);

    public static native void stopConnection();

    public static native void interruptConnection();

    public static native void sendMouseMove(short deltaX, short deltaY);

    public static native void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight);

    public static native void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight);

    public static native void sendMouseButton(byte buttonEvent, byte mouseButton);

    public static native void sendMultiControllerInput(short controllerNumber,
                                    short activeGamepadMask, int buttonFlags,
                                    byte leftTrigger, byte rightTrigger,
                                    short leftStickX, short leftStickY,
                                    short rightStickX, short rightStickY);

    public static native int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure,
                                            float contactAreaMajor, float contactAreaMinor, short rotation);

    public static native int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y,
                                          float pressure, float contactAreaMajor, float contactAreaMinor,
                                          short rotation, byte tilt);

    public static native int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities);

    public static native int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure);

    public static native int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z);

    public static native int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage);

    public static native void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags);

    public static native void sendMouseHighResScroll(short scrollAmount);

    public static native void sendMouseHighResHScroll(short scrollAmount);

    public static native void sendUtf8Text(byte[] text);
    public static native boolean sendServerCommand(byte[] payload);

    public static native String getStageName(int stage);

    public static native String findExternalAddressIP4(String stunHostName, int stunPort);

    public static native int getPendingAudioDuration();

    public static native int getPendingVideoFrames();

    public static native int testClientConnectivity(String testServerHostName, int referencePort, int testFlags);

    public static native int getPortFlagsFromStage(int stage);

    public static native int getPortFlagsFromTerminationErrorCode(int errorCode);

    public static native String stringifyPortFlags(int portFlags, String separator);

    // The RTT is in the top 32 bits, and the RTT variance is in the bottom 32 bits
    public static native long getEstimatedRttInfo();

    public static native String getLaunchUrlQueryParameters();

    public static native byte guessControllerType(int vendorId, int productId);

    public static native boolean guessControllerHasPaddles(int vendorId, int productId);

    public static native boolean guessControllerHasShareButton(int vendorId, int productId);

    public static native void init();

    // Phase lock: reports how long a frame waited from ready to the next display latch, so a
    // Rubylight host can time frames to land just before it.
    // Collects locally; true means a new report is ready for the background sender.
    public static native boolean reportFrameSlack(long slackNs, long periodNs);
    public static native void flushPhaseLockReport();

    // Early edge of the last phase-lock report in microseconds before the latch, or
    // Integer.MIN_VALUE before the first one.
    public static native int getPhaseLockLeadUs();

    // Whether this connection's host named the control message in its DESCRIBE answer
    // (a=x-rl-control, Rubylight 2.2.0+). False before the RTSP handshake and for other hosts.
    public static native boolean hostSupportsControlMessage(int messageType);

    // Display HDR luminance, client to host (protocol/core/src/control.rs DisplayCaps).
    public static final int CONTROL_MESSAGE_DISPLAY_CAPS = 0x5531;

    // Sends the display's HDR luminance in nits (0 when unknown) on the encrypted control
    // stream; false when the host did not announce 0x5531 or the stream is not up.
    public static native boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits);

    // Microphone state for this session (android_microphone.h).
    public static final int MIC_STATE_OFF = 0;
    public static final int MIC_STATE_NOT_OFFERED = 1;
    public static final int MIC_STATE_UNAVAILABLE = 2;
    public static final int MIC_STATE_READY = 3;

    public static native int getMicrophoneState();

    // Encodes one frame of 48 kHz mono PCM as Opus and sends it to the host. Returns the
    // datagram length, or a negative value when the frame was not sent.
    public static native int sendMicrophonePcm(short[] pcm, int samples);

    // Rubylight 2.2 control message: change resolution or frame rate mid-stream.
    public static final int CONTROL_MESSAGE_RECONFIGURE = 0x5532;

    // Asks the host to continue at width x height and fpsMillihz / 1000 fps; it answers with an
    // IDR at the new size. Only call when hostSupportsControlMessage(CONTROL_MESSAGE_RECONFIGURE).
    // Returns false when the request could not be sent.
    public static native boolean requestReconfigure(int width, int height, int fpsMillihz);

    // Host timing, host to client (protocol/core/src/control.rs HostTimingRequest and HostTiming).
    public static final int CONTROL_MESSAGE_HOST_TIMING = HostTimingReport.MESSAGE_TYPE;

    // Asks the host to report each frame's present-to-sent time (or stop). Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_HOST_TIMING); false when it could not be sent.
    public static native boolean sendHostTimingRequest(boolean enabled);

    // Clock sync (0x5537) and timestamp presentation (protocol/core/src/present_clock.rs); see
    // android_present_clock.c. Times are System.nanoTime().
    public static final int CONTROL_MESSAGE_CLOCK_SYNC = 0x5537;

    // Sends one probe to a host that announced clock sync; false otherwise.
    public static native boolean sendClockSyncProbe();
    public static native void presentClockStart(int minIntervalUs);
    public static native void presentClockStop();
    public static native void presentClockReset();
    // out: present time (ns), hold (us), host-to-present latency (us, -1 without clock sync), late.
    public static native boolean presentClockSchedule(long hostUs, long readyNs, long[] out);
    // out: frames scheduled, frames late, clock sync round trip (us, -1 without).
    public static native boolean presentClockStats(long[] out);

    // Host cursor shapes, client to host (docs/protocol/cursor-shape.md).
    public static final int CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST = 0x5540;

    // Asks the host to send its cursor image and leave it out of the video (true), or to stop
    // (false). Only call when hostSupportsControlMessage(CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST).
    // Returns false when the request could not be sent.
    public static native boolean sendCursorShapeRequest(boolean enabled);
}
