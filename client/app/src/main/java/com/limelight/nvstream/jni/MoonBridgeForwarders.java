package com.limelight.nvstream.jni;

import com.limelight.LimeLog;
import com.limelight.nativeapi.HostEndpoint;

/**
 * Pure-Java streaming forwarders and pre-start request state inherited by {@link MoonBridge},
 * delegating to {@link RustBridge} and {@link PortFlags} without holding native declarations or JNI callbacks.
 */
public abstract class MoonBridgeForwarders implements MoonBridgeConstants {
    MoonBridgeForwarders() {}

    static volatile Runnable usbListener;
    static volatile int hdrFormatRequested;
    static volatile boolean roamingRequested = true;
    static volatile boolean roamingMeteredRequested;
    static volatile int audioPassthroughRequested;
    static volatile int videoFecRequested = -2;
    static volatile boolean pyrowaveEarlyRequested;
    static volatile boolean codecDirectRequested;
    static volatile boolean receiveDecodePriorityRequested;
    static volatile boolean receiveCoalescingRequested = true;
    static volatile int decodeQueueBound;
    static volatile boolean frameWarpWanted;
    static volatile boolean frameWarpRequested;
    static volatile boolean frameWarpHostTooOld;

    public static int CAPABILITY_SLICES_PER_FRAME(byte slices) {
        return slices << 24;
    }

    /** Runs on any thread whenever usbList() may have changed; null stops. */
    public static void setUsbListener(Runnable listener) {
        usbListener = listener;
    }

    /**
     * Asks the host to attach the device open on fd (a UsbDeviceConnection's, which the caller
     * may close afterwards) at its usbfs path: a request id, or USB_*.
     */
    public static int usbAttach(int fd, String path, String name, HostEndpoint host) {
        if (host == null) return USB_HOST_FAILED;
        if (!RustBridge.isAvailable()) return USB_NO_STREAM;
        return RustBridge.usbAttach(fd, path, name, host.address, host.httpPort, host.httpsPort,
                host.serverCertificatePem, host.uniqueId, host.clientCertificateDer,
                host.clientPrivateKeyPkcs8);
    }

    /** Detaches an attachment of this stream; false when it is not attached. */
    public static boolean usbDetach(int requestId) {
        return RustBridge.isAvailable() && RustBridge.usbDetach(requestId);
    }

    /** This stream's attachments as JSON (src/usb.rs's list_json), "[]" without any. */
    public static String usbList() {
        // The USB screen asks with no stream too: without the engine there is nothing attached.
        if (!RustBridge.isAvailable()) return "[]";
        String list = RustBridge.usbList();
        if (list == null) {
            LimeLog.severe("USB: the attachment list did not reach Java");
            return "[]";
        }
        return list;
    }

    public static boolean sendControllerAudio(int kind, byte[] payload) {
        return RustBridge.isAvailable() && RustBridge.sendControllerAudio(kind, payload);
    }

    /**
     * Call before startConnection(): "Client roaming" and "Continue on mobile
     * data" for the next stream.
     */
    public static void requestRoaming(boolean enabled, boolean metered) {
        roamingRequested = enabled;
        roamingMeteredRequested = metered;
    }

    /**
     * Call before startConnection(): the passthrough codecs (AudioPassthrough.CODEC_* bits) the
     * audio route plays, or 0 for Opus.
     */
    public static void requestAudioPassthrough(int codecs) {
        audioPassthroughRequested = codecs;
    }

    /**
     * Call before startConnection(): early record push for the next stream
     * (PyroWaveDecoderRenderer.wantsEarlyRecords).
     */
    public static void requestPyroWaveEarly(boolean requested) {
        pyrowaveEarlyRequested = requested;
    }

    /**
     * Call before startConnection(): "Video data straight to the decoder" for the next stream, which
     * debug.nightjar.codec_direct overrides.
     */
    public static void requestCodecDirect(boolean requested) {
        codecDirectRequested = requested;
    }

    /**
     * Call before startConnection(): "Decoder priority for receiving" for the next stream, which
     * debug.nightjar.decode_priority overrides. Only direct submission uses it.
     */
    public static void requestReceiveDecodePriority(boolean requested) {
        receiveDecodePriorityRequested = requested;
    }

    /**
     * Call before startConnection(): "Group incoming video packets" for the next stream (UDP_GRO
     * on the video socket).
     */
    public static void requestReceiveCoalescing(boolean requested) {
        receiveCoalescingRequested = requested;
    }

    /**
     * Call before startConnection(): Frame Warp for the next stream (docs/frame-warp.md). {@code wanted} is the
     * setting, {@code listed} whether the host's serverinfo lists it, {@code hostTooOld} whether it is unlisted because
     * the PC's streaming app is older than this Nightjar's Frame Warp (the overlay then says so).
     */
    public static void requestFrameWarp(boolean wanted, boolean listed, boolean hostTooOld) {
        frameWarpWanted = wanted;
        frameWarpRequested = wanted && listed;
        frameWarpHostTooOld = !listed && hostTooOld;
    }

    /**
     * Whether the running stream draws with Frame Warp: asked for, the host announced it, and no
     * renderer declined it. Known from the end of the RTSP handshake, so while the renderers start.
     */
    public static boolean isFrameWarpStream() {
        return frameWarpRequested && RustBridge.isAvailable() && RustBridge.frameWarpOffered();
    }

    /** The address of the table the native renderers take (jni/frame_warp_api.h), 0 without Frame Warp. */
    public static long frameWarpApi() {
        return isFrameWarpStream() ? RustBridge.frameWarpApi() : 0;
    }

    /** The decoder queued frame {@code frame} with presentation time {@code ptsUs}; call only on a Frame Warp stream. */
    public static void frameWarpNoteFrame(long ptsUs, int frame) {
        if (RustBridge.isAvailable()) RustBridge.frameWarpNoteFrame(ptsUs, frame);
    }

    /**
     * The shift for the picture queued with {@code ptsUs}, drawn now: {@code out} (FRAME_WARP_FLOATS floats) gets
     * the shift of the picture's centre as fractions of the picture (x, y) and in picture pixels (x, y), then the lens
     * terms the rest of the picture is turned through (k.x, k.y, n.x, n.y; 0, 0, 1, 1 for a flat picture, which is
     * sampled at its position less the shift), all with y down, then the shader row (GlesFrameWarpV2: the edge
     * fill's strength, the HUD probe's reach in mask cells, the PC's confidence and flags). Returns the HUD mask
     * generation, FRAME_WARP_NO_MASK, or FRAME_WARP_INACTIVE to draw the picture as it is (also when {@code out} is
     * shorter).
     */
    public static int frameWarpForPts(long ptsUs, float[] out) {
        return RustBridge.isAvailable() ? RustBridge.frameWarpForPts(ptsUs, out) : FRAME_WARP_INACTIVE;
    }

    /**
     * As {@link #frameWarpForPts}, without counting it in the overlay's statistics: whether the picture on screen
     * should be drawn again. What a redraw then draws goes through frameWarpForPts().
     */
    public static int frameWarpPeekForPts(long ptsUs, float[] out) {
        return RustBridge.isAvailable() ? RustBridge.frameWarpPeekForPts(ptsUs, out) : FRAME_WARP_INACTIVE;
    }

    /**
     * Whether a redraw between pictures can have anything new to show, without asking for a shift: the count of
     * mouse and controller inputs sent (the low 32 bits; read it before a draw's frameWarpForPts() and compare)
     * and FRAME_WARP_STICK_DEFLECTED. 0 without a stream.
     */
    public static long frameWarpInputState() {
        return RustBridge.isAvailable() ? RustBridge.frameWarpInputState() : 0L;
    }

    /**
     * The HUD mask when its generation is not {@code known} (-1 for none): {@code size} gets width and height and
     * {@code cells}, when it has room, one byte per cell (0 or 255, rows from the top). Returns the generation or a
     * FRAME_WARP_MASK_* code.
     */
    public static int frameWarpMask(int known, int[] size, byte[] cells) {
        return RustBridge.isAvailable() ? RustBridge.frameWarpMask(known, size, cells)
                : (known < 0 ? FRAME_WARP_MASK_UNCHANGED : FRAME_WARP_MASK_CLEARED);
    }

    /** The renderer cannot draw Frame Warp for this stream: the host stops its work for it. */
    public static void frameWarpDecline() {
        if (frameWarpRequested && RustBridge.isAvailable()) RustBridge.frameWarpDecline();
    }

    /**
     * The overlay's Frame Warp line: a FRAME_WARP_* state, and in {@code out} (FRAME_WARP_STATUS_NUMBERS floats) the
     * gain, the game's lag in ms, the shift and the last second's peak in picture pixels, the PC's confidence (0 to 1,
     * -1 before it sent one) and 1 while the PC has the gyro locked.
     */
    public static int frameWarpStatus(float[] out) {
        if (!frameWarpWanted) return FRAME_WARP_OFF;
        if (!frameWarpRequested) return frameWarpHostTooOld ? FRAME_WARP_UPDATE_HOST : FRAME_WARP_NOT_OFFERED;
        return RustBridge.isAvailable() ? RustBridge.frameWarpStatus(out) : FRAME_WARP_OFF_FOR_STREAM;
    }

    /** Whether the running stream (or the last one) hands PyroWave frames over while they arrive. */
    public static boolean isPyroWaveEarlyStream() {
        return pyrowaveEarlyRequested;
    }

    /**
     * The default network changed while a stream runs; metered is the new network's
     * !NET_CAPABILITY_NOT_METERED. Returns what it means for the stream (NETWORK_*).
     */
    public static int networkChanged(boolean metered) {
        return RustBridge.isAvailable() ? RustBridge.networkChanged(metered) : NETWORK_CARRIES_ON;
    }

    /**
     * Call before startConnection(): the one HdrFormatPolicy bit the next stream asks a Rubylight
     * 3.0 host for, 0 for stock HDR10. The ANNOUNCE carries it only when the DESCRIBE offered it.
     */
    public static void requestHdrFormat(int format) {
        hdrFormatRequested = format;
    }

    /**
     * Call before startConnection(): the video FEC the next stream asks a Foundation Sunshine host
     * for in its ANNOUNCE (FecChoice.summaryPercentage(): -2 host, -1 automatic, 0-100 fixed).
     * Rubylight hosts take the choice at launch; other hosts ignore it.
     */
    public static void requestVideoFec(int percentage) {
        videoFecRequested = Math.max(-2, Math.min(percentage, 100));
    }

    /** The HDR format this stream's ANNOUNCE asked for, 0 for stock (and before the ANNOUNCE). */
    public static int getAnnouncedHdrFormat() {
        // Nothing asked keeps the native libraries out of it (and out of tests without them).
        if (hdrFormatRequested == 0 || !RustBridge.isAvailable()) return 0;
        return RustBridge.getAnnouncedHdrFormat();
    }

    /**
     * Call before startConnection(): "Frames waiting for the decoder", the complete frames that may wait
     * for a decoder without direct submission before they are dropped and a key frame is requested. 0 keeps
     * 15, the bound it had before it was a setting; so does anything above 15.
     */
    public static void setDecodeQueueBound(int frames) {
        decodeQueueBound = frames;
    }

    public static void stopConnection() {
        if (RustBridge.isAvailable()) RustBridge.stopConnection();
    }

    public static void interruptConnection() {
        if (RustBridge.isAvailable()) RustBridge.interruptConnection();
    }

    public static void sendMouseMove(short deltaX, short deltaY) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMouseMove(deltaX, deltaY);
    }

    public static void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMousePosition(x, y, referenceWidth, referenceHeight);
    }

    public static void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMouseMoveAsMousePosition(deltaX, deltaY, referenceWidth, referenceHeight);
    }

    public static void sendMouseButton(byte buttonEvent, byte mouseButton) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMouseButton(buttonEvent, mouseButton);
    }

    public static void sendMultiControllerInput(short controllerNumber,
                                    short activeGamepadMask, int buttonFlags,
                                    byte leftTrigger, byte rightTrigger,
                                    short leftStickX, short leftStickY,
                                    short rightStickX, short rightStickY) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMultiControllerInput(controllerNumber, activeGamepadMask, buttonFlags, leftTrigger, rightTrigger, leftStickX, leftStickY, rightStickX, rightStickY);
    }

    public static int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure,
                                            float contactAreaMajor, float contactAreaMinor, short rotation) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendTouchEvent(eventType, pointerId, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation);
    }

    /** How this stream takes a physical touchpad's fingers ("Touchpad gestures on the PC"). */
    public static int touchpadTransport() {
        if (!RustBridge.isAvailable()) return TOUCHPAD_NONE;
        return RustBridge.touchpadTransport();
    }

    /** One motion event of a physical touchpad; 0, or the input error. */
    public static int touchpadFrame(int device, int[] ids, float[] xs, float[] ys, float[] pressures, int count,
                                    boolean button, short widthMm, short heightMm) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.touchpadFrame(device, ids, xs, ys, pressures, count, button, widthMm, heightMm);
    }

    /** Lifts the touchpad's fingers on the host. */
    public static int touchpadCancel() {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.touchpadCancel();
    }

    public static int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y,
                                          float pressure, float contactAreaMajor, float contactAreaMinor,
                                          short rotation, byte tilt) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendPenEvent(eventType, toolType, penButtons, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation, tilt);
    }

    /** Controller identity (0x55000080): the engine drops it for a host without it. */
    public static int sendControllerIdentityEvent(byte controllerNumber, int vendorId, int productId) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendControllerIdentityEvent(controllerNumber, vendorId, productId);
    }

    public static int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendControllerArrivalEvent(controllerNumber, activeGamepadMask, type, supportedButtonFlags, capabilities);
    }

    public static int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendControllerTouchEvent(controllerNumber, eventType, pointerId, x, y, pressure);
    }

    public static int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendControllerMotionEvent(controllerNumber, motionType, x, y, z);
    }

    public static int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage) {
        if (!RustBridge.isAvailable()) return LI_ERR_UNSUPPORTED;
        return RustBridge.sendControllerBatteryEvent(controllerNumber, batteryState, batteryPercentage);
    }

    public static void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendKeyboardInput(keyMap, keyDirection, modifier, flags);
    }

    public static void sendMouseHighResScroll(short scrollAmount) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMouseHighResScroll(scrollAmount);
    }

    public static void sendMouseHighResHScroll(short scrollAmount) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendMouseHighResHScroll(scrollAmount);
    }

    public static void sendUtf8Text(byte[] text) {
        if (!RustBridge.isAvailable()) return;
        RustBridge.sendUtf8Text(text);
    }

    public static String getStageName(int stage) {
        return PortFlags.stageName(stage);
    }

    /** This device's WAN IPv4 address from a STUN server, or null (also when the Rust library did not load). */
    public static String findExternalAddressIP4(String stunHostName, int stunPort) {
        return RustBridge.isAvailable() ? RustBridge.findExternalAddressIP4(stunHostName, stunPort) : null;
    }

    public static int getPendingAudioDuration() {
        return RustBridge.isAvailable() ? RustBridge.getPendingAudioDuration() : 0;
    }

    /** The engine's session clock in microseconds, which the Vulkan renderer maps receive times with. */
    public static long sessionNowUs() {
        return RustBridge.isAvailable() ? RustBridge.sessionNowUs() : 0L;
    }

    /** The engine's Opus concealer for the AAudio callback, a C function pointer for aaudio_output.c. */
    public static long audioConcealer() {
        return RustBridge.isAvailable() ? RustBridge.audioConcealer() : 0L;
    }

    public static int getPendingVideoFrames() {
        return RustBridge.isAvailable() ? RustBridge.getPendingVideoFrames() : 0;
    }

    public static int getPortFlagsFromStage(int stage) {
        return PortFlags.fromStage(stage);
    }

    /** Whether this session's host's own disconnect ended the connection, not a silence past the timeout. */
    public static boolean hostSaidGoodbye() {
        return RustBridge.isAvailable() && RustBridge.hostSaidGoodbye();
    }

    /** The address the running stream connected to (as text, no DNS), or null without one. */
    public static String getHostAddress() {
        return RustBridge.isAvailable() ? RustBridge.getHostAddress() : null;
    }

    /** nightjar_core::host_loss::watch_goodbye: probes after the host's goodbye. */
    public static int hostLossWatchGoodbye(long handle, String host, int port, NightjarJni.HostLossCallback callback) {
        return RustBridge.isAvailable() ? RustBridge.hostLossWatchGoodbye(handle, host, port, callback) : 2;
    }

    public static boolean hostLossAvailable() {
        return RustBridge.isAvailable();
    }

    public static String getLaunchUrlQueryParameters() {
        return PortFlags.LAUNCH_URL_QUERY_PARAMETERS;
    }

    // Bytes of every video and audio datagram received this stream, FEC parity included: the
    // stream's bandwidth on the wire (Moonlight V+'s LiGetRTPVideoBytesReceived, with audio).
    public static long getWireBytesReceived() {
        return RustBridge.isAvailable() ? RustBridge.getWireBytesReceived() : 0L;
    }

    // Early edge of the last phase-lock report in microseconds before the latch, or
    // Integer.MIN_VALUE before the first one.
    public static int getPhaseLockLeadUs() {
        return RustBridge.isAvailable() ? RustBridge.getPhaseLockLeadUs() : Integer.MIN_VALUE;
    }

    // Sends the display's HDR luminance in nits (0 when unknown) on the encrypted control
    // stream; false when the host did not announce 0x5531 or the stream is not up.
    public static boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits) {
        return RustBridge.isAvailable() && RustBridge.sendDisplayCaps(hdr, maxNits, maxAverageNits, minNits);
    }

    // Encodes one frame of 48 kHz mono PCM as Opus and sends it to the host. Returns the
    // datagram length, or a negative value when the frame was not sent.
    public static int sendMicrophonePcm(short[] pcm, int samples) {
        return RustBridge.isAvailable() ? RustBridge.sendMicrophonePcm(pcm, samples) : -1;
    }

    // Asks the host to report each frame's present-to-sent time (or stop). Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_HOST_TIMING); false when it could not be sent.
    public static boolean sendHostTimingRequest(boolean enabled) {
        return RustBridge.isAvailable() && RustBridge.sendHostTimingRequest(enabled);
    }

    public static boolean sendLinkNoticeRequest(boolean enabled) {
        return RustBridge.isAvailable() && RustBridge.sendLinkNoticeRequest(enabled);
    }

    // Sends one probe to a host that announced clock sync; false otherwise.
    public static boolean sendClockSyncProbe() {
        return RustBridge.isAvailable() && RustBridge.sendClockSyncProbe();
    }

    // adaptive: the display refreshes when a frame comes (adaptive refresh rate), so a frame much
    // later than the typical one is shown when ready instead of every frame waiting for it.
    public static void presentClockStart(int minIntervalUs, boolean adaptive) {
        if (RustBridge.isAvailable()) RustBridge.presentClockStart(minIntervalUs, adaptive);
    }

    public static void presentClockStop() {
        if (RustBridge.isAvailable()) RustBridge.presentClockStop();
    }

    public static void presentClockReset() {
        if (RustBridge.isAvailable()) RustBridge.presentClockReset();
    }

    // out: present time (ns), hold (us), host-to-present latency (us, -1 without clock sync), late.
    public static boolean presentClockSchedule(long hostUs, long readyNs, long[] out) {
        return RustBridge.isAvailable() && RustBridge.presentClockSchedule(hostUs, readyNs, out);
    }

    // out: frames scheduled, frames late, clock sync round trip (us, -1 without).
    public static boolean presentClockStats(long[] out) {
        return RustBridge.isAvailable() && RustBridge.presentClockStats(out);
    }

    // out: the host-to-client clock offset in us now (client = host + offset, the client clock
    // being CLOCK_MONOTONIC like System.nanoTime()) and the round trip in us of the exchange it
    // comes from. False before the first clock sync exchange or without a running clock.
    public static boolean presentClockOffset(long[] out) {
        return RustBridge.isAvailable() && RustBridge.presentClockOffset(out);
    }

    // Notes a frame's host time (us) for the content rate; the rate in millihertz once it held
    // steady for two seconds, else 0.
    public static void presentClockObserve(long hostUs) {
        if (RustBridge.isAvailable()) RustBridge.presentClockObserve(hostUs);
    }

    public static int presentClockContentRate() {
        return RustBridge.isAvailable() ? RustBridge.presentClockContentRate() : 0;
    }

    // Asks the host to send its cursor image and leave it out of the video (true), or to stop
    // (false). Only call when hostSupportsControlMessage(CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST).
    // Returns false when the request could not be sent.
    public static boolean sendCursorShapeRequest(boolean enabled) {
        return RustBridge.isAvailable() && RustBridge.sendCursorShapeRequest(enabled);
    }

    // Asks the host for text field notices (0x553B; textInput on the listener), or to stop. Only
    // call when hostSupportsControlMessage(CONTROL_MESSAGE_TEXT_INPUT).
    public static boolean sendTextInputRequest(boolean enabled) {
        return RustBridge.isAvailable() && RustBridge.sendTextInputRequest(enabled);
    }

    // Asks the host for a bitrate in kbps, a rate in mHz or both (0x553C; rateChange on the
    // listener with the same serial), 0 keeping that value. Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_RATE_CHANGE); false when it
    // could not be sent or a value is out of range.
    public static boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz) {
        return RustBridge.isAvailable() && RustBridge.sendRateChange(serial, bitrateKbps, fpsMillihz);
    }

    // Reports the client's keyboard layout as a BCP 47 tag (0x553E), so the host types keys the
    // client did not normalize in that layout. Only call when
    // hostSupportsControlMessage(CONTROL_MESSAGE_KEYBOARD_LAYOUT); false when it could not be
    // sent or the tag is not a valid one (empty, over 64 characters, not printable ASCII).
    public static boolean sendKeyboardLayout(String bcp47Tag) {
        return RustBridge.isAvailable() && RustBridge.sendKeyboardLayout(bcp47Tag);
    }

    // Starts clipboard sync (0x5508, Moonlight V+) once the connection started. current is this device's
    // clipboard text as it is, which stays here; null when it could not be read. The host's
    // copies then arrive on the listener's clipboardSync.
    public static int clipboardSyncStart(String current) {
        return RustBridge.isAvailable() ? RustBridge.clipboardSyncStart(current) : CLIPBOARD_SYNC_FAILED;
    }

    // Text copied on this device, for clipboard sync (0x5508, Moonlight V+): one of CLIPBOARD_COPY_*.
    public static int clipboardSyncLocal(String text) {
        return RustBridge.isAvailable() ? RustBridge.clipboardSyncLocal(text) : CLIPBOARD_COPY_FAILED;
    }
}
