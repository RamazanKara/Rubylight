package com.limelight.nvstream.jni;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;

/**
 * The Rust streaming engine (nightjar-core, protocol/android in this repository), loaded from
 * libnightjar_core.so. MoonBridge routes a stream here when the user turned the engine on and
 * the stream needs nothing it lacks; the natives mirror MoonBridge's streaming natives and call
 * back into MoonBridge's bridge* methods, so renderers and listeners serve both engines.
 */
final class RustBridge {
    private RustBridge() {}

    private static Boolean available;

    /** Loads the library once; false (and the C engine) when it is missing or broken. */
    static synchronized boolean isAvailable() {
        if (available == null) {
            boolean loaded;
            try {
                System.loadLibrary("nightjar_core");
                loaded = init(BuildConfig.VERSION_NAME);
            } catch (UnsatisfiedLinkError e) {
                LimeLog.warning("Rust engine unavailable: " + e.getMessage());
                loaded = false;
            }
            available = loaded;
        }
        return available;
    }

    // GFE 3.22 (app version 7.1.431) introduced the encrypted control stream; the Rust engine
    // does not speak the older generations.
    private static final int[] MIN_APP_VERSION = {7, 1, 431};

    /**
     * Why this stream cannot use the Rust engine, or null when it can. The library is checked
     * separately (isAvailable), so this stays testable without it.
     */
    static String fallbackReason(String appVersion, int serverCodecModeSupport, boolean inputOnly) {
        if (!appVersionAtLeast(appVersion, MIN_APP_VERSION)) {
            return "the host is older than GFE 3.22 (app version " + appVersion + ")";
        }
        if (!inputOnly && serverCodecModeSupport == 0) {
            return "the host reports no codec support";
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

    /** Loads the engine; {@code appVersion} names this app in the User-Agent of requests to the host. */
    private static native boolean init(String appVersion);

    static native int startConnection(String address, String appVersion, String rtspSessionUrl,
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
                                      int audioPassthrough, int hdrFormat, int videoFecPercentage);
    static native void stopConnection();
    static native void interruptConnection();
    static native int networkChanged(boolean metered);
    static native void sendMouseMove(short deltaX, short deltaY);
    static native void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight);
    static native void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight);
    static native void sendMouseButton(byte buttonEvent, byte mouseButton);
    static native void sendMultiControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags, byte leftTrigger, byte rightTrigger, short leftStickX, short leftStickY, short rightStickX, short rightStickY);
    static native int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation);
    static native int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation, byte tilt);
    static native int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities);
    static native int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure);
    static native int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z);
    static native int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage);
    static native void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags);
    static native void sendMouseHighResScroll(short scrollAmount);
    static native void sendMouseHighResHScroll(short scrollAmount);
    static native void sendUtf8Text(byte[] text);
    static native boolean sendServerCommand(byte[] payload);
    static native int getPendingAudioDuration();

    static native long audioConcealer();
    static native int getPendingVideoFrames();
    static native long getEstimatedRttInfo();
    static native long getWireBytesReceived();
    static native boolean reportFrameSlack(long slackNs, long periodNs);
    static native void flushPhaseLockReport();
    static native int getPhaseLockLeadUs();
    static native boolean hostSupportsControlMessage(int messageType);
    static native boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits);
    static native int getMicrophoneState();
    static native int getHostPacked444Modes();
    static native int getAnnouncedHdrFormat();
    static native int sendMicrophonePcm(short[] pcm, int samples);
    static native boolean requestReconfigure(int width, int height, int fpsMillihz);
    static native boolean sendHostTimingRequest(boolean enabled);
    static native boolean sendLinkNoticeRequest(boolean enabled);
    static native boolean sendClockSyncProbe();
    static native void presentClockStart(int minIntervalUs);
    static native void presentClockStop();
    static native void presentClockReset();
    static native boolean presentClockSchedule(long hostUs, long readyNs, long[] out);
    static native boolean presentClockStats(long[] out);
    static native boolean presentClockOffset(long[] out);
    static native boolean presentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out);
    static native void presentClockObserve(long hostUs);
    static native int presentClockContentRate();
    static native boolean sendCursorShapeRequest(boolean enabled);
    static native boolean sendControllerAudio(int kind, byte[] payload);
    static native boolean sendTextInputRequest(boolean enabled);
    static native boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz);
    static native boolean sendKeyboardLayout(String bcp47Tag);
    // Clipboard sync (0x5508): MoonBridge.CLIPBOARD_SYNC_* and MoonBridge.CLIPBOARD_COPY_*.
    static native int clipboardSyncStart(String current);
    static native int clipboardSyncLocal(String text);
    // USB forwarding (0x553A): a request id, or one of MoonBridge.USB_*.
    static native int usbAttach(int fd, String path, String name, String address, int httpPort,
                                int httpsPort, String serverCertificatePem, String uniqueId,
                                byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8);
    static native boolean usbDetach(int requestId);
    static native String usbList();
}
