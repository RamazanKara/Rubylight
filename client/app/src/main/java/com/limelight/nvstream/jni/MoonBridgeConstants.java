package com.limelight.nvstream.jni;

import com.limelight.nvstream.av.video.HostTimingReport;

/**
 * Protocol, video/audio format, input, Frame Warp, USB, clipboard, and control-message constants
 * and the {@link AudioConfiguration} value type shared through {@link MoonBridge}.
 */
public interface MoonBridgeConstants {
    /* See documentation in Limelight.h for information about these functions and constants */

    AudioConfiguration AUDIO_CONFIGURATION_STEREO = new AudioConfiguration(2, 0x3);
    AudioConfiguration AUDIO_CONFIGURATION_51_SURROUND = new AudioConfiguration(6, 0x3F);
    AudioConfiguration AUDIO_CONFIGURATION_71_SURROUND = new AudioConfiguration(8, 0x63F);
    // Moonlight V+'s 7.1.4 (AUDIO_CONFIGURATION_714_SURROUND), which Foundation Sunshine and Rubylight hosts stream.
    AudioConfiguration AUDIO_CONFIGURATION_714_SURROUND = new AudioConfiguration(12, 0xF63F);

    int VIDEO_FORMAT_H264 = 0x0001;
    int VIDEO_FORMAT_H264_HIGH8_444 = 0x0004;
    int VIDEO_FORMAT_H265 = 0x0100;
    int VIDEO_FORMAT_H265_MAIN10 = 0x0200;
    int VIDEO_FORMAT_H265_REXT8_444 = 0x0400;
    int VIDEO_FORMAT_H265_REXT10_444 = 0x0800;
    int VIDEO_FORMAT_AV1_MAIN8 = 0x1000;
    int VIDEO_FORMAT_AV1_MAIN10 = 0x2000;
    int VIDEO_FORMAT_AV1_HIGH8_444 = 0x4000;
    int VIDEO_FORMAT_AV1_HIGH10_444 = 0x8000;
    int VIDEO_FORMAT_PYROWAVE = 0x010000;
    int VIDEO_FORMAT_PYROWAVE_444 = 0x020000;
    int VIDEO_FORMAT_PYROWAVE_MAIN10 = 0x040000;
    int VIDEO_FORMAT_PYROWAVE_MAIN10_444 = 0x080000;

    int VIDEO_FORMAT_MASK_H264 = 0x000F;
    int VIDEO_FORMAT_MASK_H265 = 0x0F00;
    int VIDEO_FORMAT_MASK_AV1 = 0xF000;
    int VIDEO_FORMAT_MASK_PYROWAVE = 0x0F0000;
    int VIDEO_FORMAT_MASK_10BIT = 0x0CAA00;
    int VIDEO_FORMAT_MASK_YUV444 = 0x0ACC04;

    int BUFFER_TYPE_PICDATA = 0;
    int BUFFER_TYPE_SPS = 1;
    int BUFFER_TYPE_PPS = 2;
    int BUFFER_TYPE_VPS = 3;

    int FRAME_TYPE_PFRAME = 0;
    int FRAME_TYPE_IDR = 1;

    int COLORSPACE_REC_601 = 0;
    int COLORSPACE_REC_709 = 1;
    int COLORSPACE_REC_2020 = 2;

    int COLOR_RANGE_LIMITED = 0;
    int COLOR_RANGE_FULL = 1;

    int CAPABILITY_DIRECT_SUBMIT = 1;
    int CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC = 2;
    int CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC = 4;
    int CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1 = 0x40;

    int DR_OK = 0;
    int DR_NEED_IDR = -1;

    int CONN_STATUS_OKAY = 0;
    int CONN_STATUS_POOR = 1;

    int ML_ERROR_GRACEFUL_TERMINATION = 0;
    int ML_ERROR_NO_VIDEO_TRAFFIC = -100;
    int ML_ERROR_NO_VIDEO_FRAME = -101;
    int ML_ERROR_UNEXPECTED_EARLY_TERMINATION = -102;
    int ML_ERROR_PROTECTED_CONTENT = -103;
    int ML_ERROR_FRAME_CONVERSION = -104;
    // This device's own code, never sent by a PC: the optional Vulkan renderer failed mid-stream
    int CLIENT_ERROR_VULKAN_RENDERER = -105;
    // The engine's start code (nightjar-core error::PACKED_444_REFUSED), never sent by a PC:
    // experimental full chroma cannot be streamed with this PC (no packed mode both sides offer,
    // or the PC refused it), so the stream may reconnect without it
    int CLIENT_ERROR_PACKED_444_REFUSED = -106;
    // This device's own code, never sent by a PC: the video decoder failed and recovering it did not help
    int CLIENT_ERROR_DECODER = -107;
    // This device's own code, never sent by a PC: the PC said goodbye and closed its server: it shuts down or its host stopped (HostLossWatch)
    int CLIENT_ERROR_HOST_GONE = -108;
    // This device's own code, never sent by a PC: the streaming engine cannot start this stream (a PC older than
    // GFE 3.22, a PC that reports no codec, or libnightjar_core.so did not load); StreamEngine says which
    int CLIENT_ERROR_ENGINE_UNAVAILABLE = -109;

    int ML_PORT_INDEX_TCP_47984 = 0;
    int ML_PORT_INDEX_TCP_47989 = 1;
    int ML_PORT_INDEX_TCP_48010 = 2;
    int ML_PORT_INDEX_UDP_47998 = 8;
    int ML_PORT_INDEX_UDP_47999 = 9;
    int ML_PORT_INDEX_UDP_48000 = 10;
    int ML_PORT_INDEX_UDP_48010 = 11;

    int ML_PORT_FLAG_ALL = 0xFFFFFFFF;
    int ML_PORT_FLAG_TCP_47984 = 0x0001;
    int ML_PORT_FLAG_TCP_47989 = 0x0002;
    int ML_PORT_FLAG_TCP_48010 = 0x0004;
    int ML_PORT_FLAG_UDP_47998 = 0x0100;
    int ML_PORT_FLAG_UDP_47999 = 0x0200;
    int ML_PORT_FLAG_UDP_48000 = 0x0400;
    int ML_PORT_FLAG_UDP_48010 = 0x0800;

    int ML_TEST_RESULT_INCONCLUSIVE = 0xFFFFFFFF;

    byte SS_KBE_FLAG_NON_NORMALIZED = 0x01;

    int LI_ERR_UNSUPPORTED = -5501;

    byte LI_TOUCH_EVENT_HOVER       = 0x00;
    byte LI_TOUCH_EVENT_DOWN        = 0x01;
    byte LI_TOUCH_EVENT_UP          = 0x02;
    byte LI_TOUCH_EVENT_MOVE        = 0x03;
    byte LI_TOUCH_EVENT_CANCEL      = 0x04;
    byte LI_TOUCH_EVENT_BUTTON_ONLY = 0x05;
    byte LI_TOUCH_EVENT_HOVER_LEAVE = 0x06;
    byte LI_TOUCH_EVENT_CANCEL_ALL  = 0x07;

    byte LI_TOOL_TYPE_UNKNOWN = 0x00;
    byte LI_TOOL_TYPE_PEN = 0x01;
    byte LI_TOOL_TYPE_ERASER = 0x02;

    byte LI_PEN_BUTTON_PRIMARY = 0x01;
    byte LI_PEN_BUTTON_SECONDARY = 0x02;
    byte LI_PEN_BUTTON_TERTIARY = 0x04;

    byte LI_TILT_UNKNOWN = (byte)0xFF;
    short LI_ROT_UNKNOWN = (short)0xFFFF;

    byte LI_CTYPE_UNKNOWN  = 0x00;
    byte LI_CTYPE_XBOX     = 0x01;
    byte LI_CTYPE_PS       = 0x02;
    byte LI_CTYPE_NINTENDO = 0x03;
    byte LI_CTYPE_STEAM    = 0x04;

    byte LI_MOTION_TYPE_ACCEL = 0x01;
    byte LI_MOTION_TYPE_GYRO  = 0x02;

    byte LI_BATTERY_STATE_UNKNOWN      = 0x00;
    byte LI_BATTERY_STATE_NOT_PRESENT  = 0x01;
    byte LI_BATTERY_STATE_DISCHARGING  = 0x02;
    byte LI_BATTERY_STATE_CHARGING     = 0x03;
    byte LI_BATTERY_STATE_NOT_CHARGING = 0x04; // Connected to power but not charging
    byte LI_BATTERY_STATE_FULL         = 0x05;

    byte LI_BATTERY_PERCENTAGE_UNKNOWN = (byte)0xFF;

    // USB forwarding (0x553A, docs/control-messages.md): what usbAttach returns instead of a request id.
    int USB_NO_STREAM = -1;
    int USB_BUSY = -2;
    int USB_UNSUPPORTED = -3;
    int USB_DEVICE_FAILED = -4;
    int USB_INVALID = -5;
    int USB_HOST_FAILED = -6;

    /** frameWarpStatus(): no Frame Warp line (the setting is off). */
    int FRAME_WARP_OFF = 0;
    /** frameWarpStatus(): the setting is on, the host does not offer it. */
    int FRAME_WARP_NOT_OFFERED = 1;
    /** frameWarpStatus(): offered; the host has not learned this game's camera yet. */
    int FRAME_WARP_LEARNING = 2;
    /** frameWarpStatus(): locked to the mouse; the gain is picture pixels per mouse count. */
    int FRAME_WARP_MOUSE = 3;
    /** frameWarpStatus(): locked to the right stick; the gain is picture pixels per second of full stick. */
    int FRAME_WARP_STICK = 4;
    /** frameWarpStatus(): asked for, but this stream draws without it (a renderer declined). */
    int FRAME_WARP_OFF_FOR_STREAM = 5;
    /**
     * frameWarpStatus(): the setting is on and the PC's streaming app is too old for this Nightjar's Frame Warp
     * (HostCapabilities.Reason.UPDATE_HOST). Java's own, from requestFrameWarp(); the engine never returns it.
     */
    int FRAME_WARP_UPDATE_HOST = 6;
    /**
     * The numbers frameWarpStatus() writes: gain, lag ms, shift px, peak px, the PC's confidence (-1 before it sent
     * one) and 1 while the PC has the gyro locked.
     */
    int FRAME_WARP_STATUS_NUMBERS = 6;
    /** frameWarpForPts() and the native table: draw the picture as it is. */
    int FRAME_WARP_INACTIVE = -2;
    /** frameWarpForPts(): shift the whole picture, there is no HUD mask. */
    int FRAME_WARP_NO_MASK = -1;
    /** frameWarpMask(): the mask the caller holds is current. */
    int FRAME_WARP_MASK_UNCHANGED = -1;
    /** frameWarpMask(): there is no mask any more. */
    int FRAME_WARP_MASK_CLEARED = -2;
    /** frameWarpMask(): the cells array is too small; size holds what it needs. */
    int FRAME_WARP_MASK_TOO_SMALL = -3;
    /**
     * The floats frameWarpForPts() and frameWarpPeekForPts() write, as the native table's warp() and peek(): the shift,
     * the lens terms, then the shader row (GlesFrameWarpV2).
     */
    int FRAME_WARP_FLOATS = 12;
    /** frameWarpInputState(): set while the tracked right stick is deflected. */
    long FRAME_WARP_STICK_DEFLECTED = 1L << 32;

    /** networkChanged: the stream's connection goes on (same address, or no route yet). */
    int NETWORK_CARRIES_ON = 0;
    /** networkChanged: a roam started, the stream moves onto the new network. */
    int NETWORK_ROAMED = 1;
    /** networkChanged: the route toward the PC left the stream's address; it cannot carry on. */
    int NETWORK_CANNOT_CONTINUE = 2;

    /** The stage a start the engine cannot make is reported as failed in. */
    String ENGINE_STAGE = "streaming engine check";

    /** The host takes no touchpad fingers. */
    int TOUCHPAD_NONE = 0;
    int TOUCHPAD_INDIVIDUAL = 1;
    int TOUCHPAD_FRAME = 2;

    // Phase lock reports (reportFrameSlack), client to host (protocol/core/src/phase_lock.rs).
    int CONTROL_MESSAGE_PHASE_LOCK = 0x5530;
    // Display HDR luminance, client to host (protocol/core/src/control.rs DisplayCaps).
    int CONTROL_MESSAGE_DISPLAY_CAPS = 0x5531;
    // Rubylight 2.2 control message: change resolution or frame rate mid-stream.
    int CONTROL_MESSAGE_RECONFIGURE = 0x5532;
    // Host timing, host to client (protocol/core/src/control.rs HostTimingRequest and HostTiming).
    int CONTROL_MESSAGE_HOST_TIMING = HostTimingReport.MESSAGE_TYPE;
    // Link notice (0x5538, docs/control-messages.md in the Rubylight host): the host says when it
    // lowers the video bitrate because of the network.
    int CONTROL_MESSAGE_LINK_NOTICE = 0x5538;
    // Clock sync (0x5537) and timestamp presentation (protocol/core/src/present_clock.rs). Times are System.nanoTime().
    int CONTROL_MESSAGE_CLOCK_SYNC = 0x5537;
    // Host cursor shapes, client to host (docs/protocol/cursor-shape.md).
    int CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST = 0x5540;

    // The 3.0 extensions (protocol/core/src/ids.rs). Defined so hostSupportsControlMessage can ask
    // for them. MoonBridgeControlMessagesTest holds these and the cursor shape request to test
    // resources protocol/control-messages.txt, which nightjar-core checks too.
    int CONTROL_MESSAGE_FEC_REPORT = 0x5539;
    int CONTROL_MESSAGE_USB = 0x553A;
    int CONTROL_MESSAGE_TEXT_INPUT = 0x553B;
    int CONTROL_MESSAGE_RATE_CHANGE = 0x553C;
    int CONTROL_MESSAGE_PIPELINE_RATES = 0x553D;
    int CONTROL_MESSAGE_KEYBOARD_LAYOUT = 0x553E;
    // Moonlight V+'s clipboard sync, which Rubylight hosts offer to launches that ask for it.
    int CONTROL_MESSAGE_CLIPBOARD_SYNC = 0x5508;
    // Moonlight V+'s FEC summary, which the engine sends to Foundation Sunshine hosts that
    // offer FEC control in DESCRIBE (no a=x-rl-control entry).
    int CONTROL_MESSAGE_FEC_SUMMARY = 0x5510;

    // Microphone state for this session.
    int MIC_STATE_OFF = 0;
    int MIC_STATE_NOT_OFFERED = 1;
    int MIC_STATE_UNAVAILABLE = 2;
    int MIC_STATE_READY = 3;

    // What clipboardSyncStart did: on, the host did not offer it, or the hello could not be sent.
    int CLIPBOARD_SYNC_ON = 0;
    int CLIPBOARD_SYNC_NOT_OFFERED = 1;
    int CLIPBOARD_SYNC_FAILED = 2;
    // What clipboardSyncLocal did with a copy: sent, skipped (empty, or the host's own copy coming
    // back), too large for a frame, or not sent (the sync is off or the send failed).
    int CLIPBOARD_COPY_SENT = 0;
    int CLIPBOARD_COPY_SKIPPED = 1;
    int CLIPBOARD_COPY_TOO_LARGE = 2;
    int CLIPBOARD_COPY_FAILED = 3;
    // clipboardSyncRefused reasons: the host copied an image, or sent what the sync cannot use.
    int CLIPBOARD_REFUSED_IMAGE = 1;
    int CLIPBOARD_REFUSED_OTHER = 2;

    class AudioConfiguration {
        public final int channelCount;
        public final int channelMask;

        public AudioConfiguration(int channelCount, int channelMask) {
            this.channelCount = channelCount;
            this.channelMask = channelMask;
        }

        // Creates an AudioConfiguration from the integer value returned by moonlight-common-c
        // See CHANNEL_COUNT_FROM_AUDIO_CONFIGURATION() and CHANNEL_MASK_FROM_AUDIO_CONFIGURATION()
        // in Limelight.h
        AudioConfiguration(int audioConfiguration) {
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
}
