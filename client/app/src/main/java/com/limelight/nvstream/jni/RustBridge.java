package com.limelight.nvstream.jni;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;

/**
 * The streaming engine (nightjar-core, protocol/android in this repository), loaded from
 * libnightjar_core.so. It is the only engine: MoonBridge starts every stream here, or fails the
 * start with the reason ({@link #startRefusal}). The natives mirror MoonBridge's streaming calls
 * and call back into MoonBridge's bridge* methods, so renderers and listeners serve it.
 */
final class RustBridge {
    private RustBridge() {}

    private static volatile boolean initFailed;

    /**
     * Loads the library once via {@link NightjarJni}; false when missing or broken ({@link #loadFailure} says why).
     * Never touches {@link MoonBridge} here; {@link #initStream()} binds {@code MoonBridge.class} lazily on stream start.
     */
    static boolean isAvailable() {
        return NightjarJni.isAvailable();
    }

    /** Why {@link #isAvailable} is false or stream bridge init failed, or null while available. */
    static String loadFailure() {
        String failure = NightjarJni.loadFailure();
        if (failure != null) {
            return failure;
        }
        return initFailed ? "its initialisation failed" : null;
    }

    private static synchronized boolean initStream() {
        if (!NightjarJni.isAvailable()) {
            return false;
        }
        if (!NightjarJni.rustBridgeInit(BuildConfig.VERSION_NAME, MoonBridge.class)) {
            initFailed = true;
            LimeLog.warning("Streaming engine bridge initialisation failed");
            return false;
        }
        initFailed = false;
        return true;
    }

    // GFE 3.22 (app version 7.1.431) introduced the encrypted control stream; the engine
    // does not speak the older generations.
    private static final int[] MIN_APP_VERSION = {7, 1, 431};

    /**
     * Why the engine cannot start this stream, as the sentence the connecting screen shows, or null
     * when it can. The library is checked separately ({@link #startRefusal}), so this stays
     * testable without it.
     */
    static String hostRefusal(String appVersion, int serverCodecModeSupport, boolean inputOnly) {
        if (!appVersionAtLeast(appVersion, MIN_APP_VERSION)) {
            return "This PC's streaming software is older than GFE 3.22 (app version " + appVersion +
                    "), which Nightjar does not support. Update it, or use Sunshine, Apollo or Rubylight.";
        }
        if (!inputOnly && serverCodecModeSupport == 0) {
            return "This PC reports no video codec it can encode, so the stream cannot start. " +
                    "Check the PC's GPU and streaming software.";
        }
        return null;
    }

    /** Why the next stream cannot start (the host's, or the library's), or null when it can. */
    static String startRefusal(String appVersion, int serverCodecModeSupport, boolean inputOnly) {
        String refusal = hostRefusal(appVersion, serverCodecModeSupport, inputOnly);
        if (refusal != null) return refusal;
        if (!isAvailable()) {
            return "Nightjar's streaming engine (libnightjar_core.so) cannot start: " + loadFailure() +
                    ". Reinstall Nightjar, and send a problem report if this stays.";
        }
        return null;
    }

    static boolean appVersionAtLeast(String appVersion, int[] minimum) {
        if (appVersion == null) {
            return false;
        }
        String[] parts = appVersion.split("\\.");
        for (int i = 0; i < minimum.length; i++) {
            int value;
            try {
                value = i < parts.length ? Integer.parseInt(parts[i].trim()) : 0;
            } catch (NumberFormatException e) {
                return false;
            }
            if (value != minimum[i]) {
                return value > minimum[i];
            }
        }
        return true;
    }

    static int startConnection(String address, String appVersion, String rtspSessionUrl,
                               int serverCodecModeSupport, int width, int height, int fps,
                               int bitrate, int packetSize, int streamingRemotely,
                               int audioConfiguration, int supportedVideoFormats,
                               int clientRefreshRateX100, byte[] riAesKey, byte[] riAesIv,
                               int videoCapabilities, int colorSpace, int colorRange,
                               int hostFeatures, boolean inputOnly, boolean enableMic,
                               boolean hapticsPcm, int pyrowaveLinkMbps, int pyrowavePaceMbps,
                               boolean clientWireless, int networkGracePeriodMs, boolean packed444,
                               int packed444Modes, boolean remoteMonitor, boolean unbatchedInput,
                               boolean networkPriority, boolean roaming, boolean roamingMetered,
                               int audioPassthrough, int hdrFormat, int videoFecPercentage,
                               boolean pyrowaveEarly, boolean codecDirect,
                               boolean receiveDecodePriority, boolean frameWarpRequested,
                               boolean receiveCoalescing, int decodeQueueBound) {
        if (!initStream()) return -1;
        return NightjarJni.startConnection(address, appVersion, rtspSessionUrl,
                serverCodecModeSupport, width, height, fps, bitrate, packetSize, streamingRemotely,
                audioConfiguration, supportedVideoFormats, clientRefreshRateX100, riAesKey, riAesIv,
                videoCapabilities, colorSpace, colorRange, hostFeatures, inputOnly, enableMic,
                hapticsPcm, pyrowaveLinkMbps, pyrowavePaceMbps, clientWireless, networkGracePeriodMs,
                packed444, packed444Modes, remoteMonitor, unbatchedInput, networkPriority, roaming,
                roamingMetered, audioPassthrough, hdrFormat, videoFecPercentage, pyrowaveEarly,
                codecDirect, receiveDecodePriority, frameWarpRequested, receiveCoalescing,
                decodeQueueBound);
    }

    static void stopConnection() { NightjarJni.stopConnection(); }
    static void interruptConnection() { NightjarJni.interruptConnection(); }
    static int networkChanged(boolean metered) { return NightjarJni.networkChanged(metered); }
    static boolean hostSaidGoodbye() { return NightjarJni.hostSaidGoodbye(); }
    static String getHostAddress() { return NightjarJni.getHostAddress(); }
    static long hostLossWatchCreate() { return NightjarJni.hostLossWatchCreate(); }
    static void hostLossWatchStop(long handle) { NightjarJni.hostLossWatchStop(handle); }
    static void hostLossWatchDestroy(long handle) { NightjarJni.hostLossWatchDestroy(handle); }
    static int hostLossWatchGoodbye(long handle, String host, int port, NightjarJni.HostLossCallback callback) {
        return NightjarJni.hostLossWatchGoodbye(handle, host, port, callback);
    }
    static void sendMouseMove(short deltaX, short deltaY) { NightjarJni.sendMouseMove(deltaX, deltaY); }
    static void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight) { NightjarJni.sendMousePosition(x, y, referenceWidth, referenceHeight); }
    static void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight) { NightjarJni.sendMouseMoveAsMousePosition(deltaX, deltaY, referenceWidth, referenceHeight); }
    static void sendMouseButton(byte buttonEvent, byte mouseButton) { NightjarJni.sendMouseButton(buttonEvent, mouseButton); }
    static void sendMultiControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags, byte leftTrigger, byte rightTrigger, short leftStickX, short leftStickY, short rightStickX, short rightStickY) {
        NightjarJni.sendMultiControllerInput(controllerNumber, activeGamepadMask, buttonFlags, leftTrigger, rightTrigger, leftStickX, leftStickY, rightStickX, rightStickY);
    }
    static int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation) {
        return NightjarJni.sendTouchEvent(eventType, pointerId, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation);
    }
    static int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation, byte tilt) {
        return NightjarJni.sendPenEvent(eventType, toolType, penButtons, x, y, pressure, contactAreaMajor, contactAreaMinor, rotation, tilt);
    }
    static int sendControllerIdentityEvent(byte controllerNumber, int vendorId, int productId) {
        return NightjarJni.sendControllerIdentityEvent(controllerNumber, vendorId, productId);
    }
    /** How the host takes a physical touchpad's fingers: {@link MoonBridge#TOUCHPAD_NONE}, _INDIVIDUAL or _FRAME. */
    static int touchpadTransport() { return NightjarJni.touchpadTransport(); }
    /** One motion event of a physical touchpad: the first {@code count} fingers on it, positions 0-1 on its surface. */
    static int touchpadFrame(int device, int[] ids, float[] xs, float[] ys, float[] pressures, int count,
                             boolean button, short widthMm, short heightMm) {
        return NightjarJni.touchpadFrame(device, ids, xs, ys, pressures, count, button, widthMm, heightMm);
    }
    static int touchpadCancel() { return NightjarJni.touchpadCancel(); }
    static int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities) {
        return NightjarJni.sendControllerArrivalEvent(controllerNumber, activeGamepadMask, type, supportedButtonFlags, capabilities);
    }
    static int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure) {
        return NightjarJni.sendControllerTouchEvent(controllerNumber, eventType, pointerId, x, y, pressure);
    }
    static int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z) {
        return NightjarJni.sendControllerMotionEvent(controllerNumber, motionType, x, y, z);
    }
    static int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage) {
        return NightjarJni.sendControllerBatteryEvent(controllerNumber, batteryState, batteryPercentage);
    }
    static void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags) {
        NightjarJni.sendKeyboardInput(keyMap, keyDirection, modifier, flags);
    }
    static void sendMouseHighResScroll(short scrollAmount) { NightjarJni.sendMouseHighResScroll(scrollAmount); }
    static void sendMouseHighResHScroll(short scrollAmount) { NightjarJni.sendMouseHighResHScroll(scrollAmount); }
    static void sendUtf8Text(byte[] text) { NightjarJni.sendUtf8Text(text); }
    static boolean sendServerCommand(byte[] payload) { return NightjarJni.sendServerCommand(payload); }
    static int getPendingAudioDuration() { return NightjarJni.getPendingAudioDuration(); }

    // nightjar-core's connectivity test and STUN lookup (protocol/android/src/lib.rs).
    static int testClientConnectivity(String testServerHostName, int referencePort, int testFlags) {
        return NightjarJni.testClientConnectivity(testServerHostName, referencePort, testFlags);
    }
    static String findExternalAddressIP4(String stunHostName, int stunPort) {
        return NightjarJni.findExternalAddressIP4(stunHostName, stunPort);
    }
    static long audioConcealer() { return NightjarJni.audioConcealer(); }
    static int getPendingVideoFrames() { return NightjarJni.getPendingVideoFrames(); }
    static long getDecoderDrops() { return NightjarJni.getDecoderDrops(); }
    static long getEstimatedRttInfo() { return NightjarJni.getEstimatedRttInfo(); }
    static long getWireBytesReceived() { return NightjarJni.getWireBytesReceived(); }
    static long getVideoReceiveBufferInfo() { return NightjarJni.getVideoReceiveBufferInfo(); }
    static boolean reportFrameSlack(long slackNs, long periodNs) { return NightjarJni.reportFrameSlack(slackNs, periodNs); }
    static void flushPhaseLockReport() { NightjarJni.flushPhaseLockReport(); }
    static int getPhaseLockLeadUs() { return NightjarJni.getPhaseLockLeadUs(); }
    /** The session clock's microseconds now: what frame receive and enqueue times are measured on. */
    static long sessionNowUs() { return NightjarJni.sessionNowUs(); }
    static boolean hostSupportsControlMessage(int messageType) { return NightjarJni.hostSupportsControlMessage(messageType); }
    // Frame Warp (protocol/android/src/frame_warp.rs); MoonBridge has what they mean.
    static boolean frameWarpOffered() { return NightjarJni.frameWarpOffered(); }
    static long frameWarpApi() { return NightjarJni.frameWarpApi(); }
    static void frameWarpNoteFrame(long ptsUs, int frame) { NightjarJni.frameWarpNoteFrame(ptsUs, frame); }
    static int frameWarpForPts(long ptsUs, float[] out) { return NightjarJni.frameWarpForPts(ptsUs, out); }
    static int frameWarpPeekForPts(long ptsUs, float[] out) { return NightjarJni.frameWarpPeekForPts(ptsUs, out); }
    static long frameWarpInputState() { return NightjarJni.frameWarpInputState(); }
    static int frameWarpMask(int known, int[] size, byte[] cells) { return NightjarJni.frameWarpMask(known, size, cells); }
    static void frameWarpDecline() { NightjarJni.frameWarpDecline(); }
    static int frameWarpStatus(float[] out) { return NightjarJni.frameWarpStatus(out); }
    static boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits) {
        return NightjarJni.sendDisplayCaps(hdr, maxNits, maxAverageNits, minNits);
    }
    static int getMicrophoneState() { return NightjarJni.getMicrophoneState(); }
    static int getHostPacked444Modes() { return NightjarJni.getHostPacked444Modes(); }
    static int getAnnouncedHdrFormat() { return NightjarJni.getAnnouncedHdrFormat(); }
    static int sendMicrophonePcm(short[] pcm, int samples) { return NightjarJni.sendMicrophonePcm(pcm, samples); }
    static boolean requestReconfigure(int width, int height, int fpsMillihz) {
        return NightjarJni.requestReconfigure(width, height, fpsMillihz);
    }
    static boolean sendHostTimingRequest(boolean enabled) { return NightjarJni.sendHostTimingRequest(enabled); }
    static boolean sendLinkNoticeRequest(boolean enabled) { return NightjarJni.sendLinkNoticeRequest(enabled); }
    static boolean sendClockSyncProbe() { return NightjarJni.sendClockSyncProbe(); }
    static void presentClockStart(int minIntervalUs, boolean adaptive) { NightjarJni.presentClockStart(minIntervalUs, adaptive); }
    static void presentClockStop() { NightjarJni.presentClockStop(); }
    static void presentClockReset() { NightjarJni.presentClockReset(); }
    static boolean presentClockSchedule(long hostUs, long readyNs, long[] out) {
        return NightjarJni.presentClockSchedule(hostUs, readyNs, out);
    }
    static boolean presentClockStats(long[] out) { return NightjarJni.presentClockStats(out); }
    static boolean presentClockOffset(long[] out) { return NightjarJni.presentClockOffset(out); }
    static boolean presentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out) {
        return NightjarJni.presentClockScheduleOnGrid(hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs, out);
    }
    static void presentClockObserve(long hostUs) { NightjarJni.presentClockObserve(hostUs); }
    static int presentClockContentRate() { return NightjarJni.presentClockContentRate(); }
    static boolean sendCursorShapeRequest(boolean enabled) { return NightjarJni.sendCursorShapeRequest(enabled); }
    static boolean sendControllerAudio(int kind, byte[] payload) { return NightjarJni.sendControllerAudio(kind, payload); }
    static boolean sendTextInputRequest(boolean enabled) { return NightjarJni.sendTextInputRequest(enabled); }
    static boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz) {
        return NightjarJni.sendRateChange(serial, bitrateKbps, fpsMillihz);
    }
    static boolean sendStreamDvr(int requestId, int op, int durationSecs) {
        return NightjarJni.sendStreamDvr(requestId, op, durationSecs);
    }
    static boolean sendKeyboardLayout(String bcp47Tag) { return NightjarJni.sendKeyboardLayout(bcp47Tag); }
    // Clipboard sync (0x5508): MoonBridge.CLIPBOARD_SYNC_* and MoonBridge.CLIPBOARD_COPY_*.
    static int clipboardSyncStart(String current) { return NightjarJni.clipboardSyncStart(current); }
    static int clipboardSyncLocal(String text) { return NightjarJni.clipboardSyncLocal(text); }
    // USB forwarding (0x553A): a request id, or one of MoonBridge.USB_*.
    static int usbAttach(int fd, String path, String name, String address, int httpPort,
                         int httpsPort, String serverCertificatePem, String uniqueId,
                         byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8) {
        if (!initStream()) return MoonBridge.USB_NO_STREAM;
        return NightjarJni.usbAttach(fd, path, name, address, httpPort, httpsPort,
                serverCertificatePem, uniqueId, clientCertificateDer, clientPrivateKeyPkcs8);
    }
    static boolean usbDetach(int requestId) { return NightjarJni.usbDetach(requestId); }
    static String usbList() { return NightjarJni.usbList(); }
}
