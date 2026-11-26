package com.limelight.nvstream.jni;

/**
 * Pure-JDK JNI boundary for {@code nightjar_core} ({@code protocol/android}).
 *
 * <p>Robolectric excludes this single class from {@code SandboxClassLoader} acquisition via
 * {@code NightjarAndroidConfigurer} so {@code System.loadLibrary("nightjar_core")} runs once on
 * the JVM's application classloader and every {@code native} symbol resolves for all sandboxes.
 * Keep this class strictly pure-JDK ({@code java.*} only, zero Android/JSON/app types) and never
 * touch {@code MoonBridge} or {@code RustBridge} during library loading.
 */
public final class NightjarJni {
    private NightjarJni() {}

    static final class Loader {
        private boolean checked;
        private boolean available;
        private String failure;

        synchronized boolean isAvailable(Runnable loadLibrary) {
            if (!checked) {
                try {
                    loadLibrary.run();
                    available = true;
                } catch (LinkageError e) {
                    failure = "the library did not load (" + e.getMessage() + ")";
                    available = false;
                }
                checked = true;
            }
            return available;
        }

        synchronized String failure() {
            return checked && !available ? failure : null;
        }
    }

    private static final Loader LOADER = new Loader();
    private static volatile Throwable unavailableForTest;

    /** Simulates an unavailable {@code nightjar_core} library in JVM tests; always clear in {@code finally}. */
    public static void forceUnavailableForTest(Throwable error) {
        unavailableForTest = error;
    }

    /** Loads {@code nightjar_core} once without touching {@code MoonBridge}; false when missing or forced off. */
    public static boolean isAvailable() {
        if (unavailableForTest != null) {
            return false;
        }
        return LOADER.isAvailable(() -> System.loadLibrary("nightjar_core"));
    }

    /** Why {@link #isAvailable()} is false, or null while it is true. */
    public static String loadFailure() {
        Throwable forced = unavailableForTest;
        if (forced != null) {
            return "the library did not load (" + forced.getMessage() + ")";
        }
        return LOADER.failure();
    }

    /** Ensures {@code nightjar_core} is loaded, or throws the simulated/real {@link LinkageError}. */
    public static void ensureLoaded() {
        Throwable forced = unavailableForTest;
        if (forced != null) {
            if (forced instanceof Error) {
                throw (Error) forced;
            }
            if (forced instanceof RuntimeException) {
                throw (RuntimeException) forced;
            }
            UnsatisfiedLinkError error = new UnsatisfiedLinkError(forced.getMessage());
            error.initCause(forced);
            throw error;
        }
        if (!isAvailable()) {
            throw new UnsatisfiedLinkError(loadFailure());
        }
    }

    /** Loads {@code nightjar_core} without consulting {@link #forceUnavailableForTest(Throwable)}. */
    public static void ensureLibraryLoaded() {
        if (!LOADER.isAvailable(() -> System.loadLibrary("nightjar_core"))) {
            throw new UnsatisfiedLinkError(LOADER.failure());
        }
    }

    // --- Stream engine (lib.rs) ---

    public static native boolean rustBridgeInit(String appVersion, Class<?> moonBridgeClass);

    public static native int startConnection(
            String address, String appVersion, String rtspSessionUrl,
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
            boolean receiveCoalescing, int decodeQueueBound);

    public static native void stopConnection();

    public static native void interruptConnection();

    public static native int networkChanged(boolean metered);

    public interface HostLossCallback {
        String gatewayToward();
        boolean isStopped();
        void log(String line);
    }

    public interface RouteWaitCallback {
        long nowMs();
        String probe(String host, int port);
        boolean isCancelled();
        boolean sleep(long ms);
        void logInfo(String line);
        void logWarn(String line);
    }

    public static native boolean hostSaidGoodbye();

    public static native String getHostAddress();

    public static native long hostLossWatchCreate();

    public static native void hostLossWatchStop(long handle);

    public static native void hostLossWatchDestroy(long handle);

    public static native int hostLossWatchGoodbye(long handle, String host, int port, HostLossCallback callback);

    public static native String routeProblem(String host, int port);

    public static native int routeWait(String host, int port, long deadlineMs, RouteWaitCallback callback);

    public static native void sendMouseMove(short deltaX, short deltaY);

    public static native void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight);

    public static native void sendMouseMoveAsMousePosition(
            short deltaX, short deltaY, short referenceWidth, short referenceHeight);

    public static native void sendMouseButton(byte buttonEvent, byte mouseButton);

    public static native void sendMultiControllerInput(
            short controllerNumber, short activeGamepadMask, int buttonFlags,
            byte leftTrigger, byte rightTrigger,
            short leftStickX, short leftStickY, short rightStickX, short rightStickY);

    public static native int sendTouchEvent(
            byte eventType, int pointerId, float x, float y, float pressure,
            float contactAreaMajor, float contactAreaMinor, short rotation);

    public static native int sendPenEvent(
            byte eventType, byte toolType, byte penButtons, float x, float y,
            float pressure, float contactAreaMajor, float contactAreaMinor,
            short rotation, byte tilt);

    public static native int sendControllerIdentityEvent(byte controllerNumber, int vendorId, int productId);

    public static native int touchpadTransport();

    public static native int touchpadFrame(
            int device, int[] ids, float[] xs, float[] ys, float[] pressures, int count,
            boolean button, short widthMm, short heightMm);

    public static native int touchpadCancel();

    public static native int sendControllerArrivalEvent(
            byte controllerNumber, short activeGamepadMask, byte type,
            int supportedButtonFlags, short capabilities);

    public static native int sendControllerTouchEvent(
            byte controllerNumber, byte eventType, int pointerId,
            float x, float y, float pressure);

    public static native int sendControllerMotionEvent(
            byte controllerNumber, byte motionType, float x, float y, float z);

    public static native int sendControllerBatteryEvent(
            byte controllerNumber, byte batteryState, byte batteryPercentage);

    public static native void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags);

    public static native void sendMouseHighResScroll(short scrollAmount);

    public static native void sendMouseHighResHScroll(short scrollAmount);

    public static native void sendUtf8Text(byte[] text);

    public static native boolean sendServerCommand(byte[] payload);

    public static native int getPendingAudioDuration();

    public static native int testClientConnectivity(String testServerHostName, int referencePort, int testFlags);

    public static native String findExternalAddressIP4(String stunHostName, int stunPort);

    public static native long audioConcealer();

    public static native int getPendingVideoFrames();

    public static native long getDecoderDrops();

    public static native long getEstimatedRttInfo();

    public static native long getWireBytesReceived();

    public static native long getVideoReceiveBufferInfo();

    public static native boolean reportFrameSlack(long slackNs, long periodNs);

    public static native void flushPhaseLockReport();

    public static native int getPhaseLockLeadUs();

    public static native long sessionNowUs();

    public static native boolean hostSupportsControlMessage(int messageType);

    public static native boolean frameWarpOffered();

    public static native long frameWarpApi();

    public static native void frameWarpNoteFrame(long ptsUs, int frame);

    public static native int frameWarpForPts(long ptsUs, float[] out);

    public static native int frameWarpPeekForPts(long ptsUs, float[] out);

    public static native long frameWarpInputState();

    public static native int frameWarpMask(int known, int[] size, byte[] cells);

    public static native void frameWarpDecline();

    public static native int frameWarpStatus(float[] out);

    public static native boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits);

    public static native int getMicrophoneState();

    public static native int getHostPacked444Modes();

    public static native int getAnnouncedHdrFormat();

    public static native int sendMicrophonePcm(short[] pcm, int samples);

    public static native boolean requestReconfigure(int width, int height, int fpsMillihz);

    public static native boolean sendHostTimingRequest(boolean enabled);

    public static native boolean sendLinkNoticeRequest(boolean enabled);

    public static native boolean sendClockSyncProbe();

    public static native void presentClockStart(int minIntervalUs, boolean adaptive);

    public static native void presentClockStop();

    public static native void presentClockReset();

    public static native boolean presentClockSchedule(long hostUs, long readyNs, long[] out);

    public static native boolean presentClockStats(long[] out);

    public static native boolean presentClockOffset(long[] out);

    public static native boolean presentClockScheduleOnGrid(
            long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out);

    public static native void presentClockObserve(long hostUs);

    public static native int presentClockContentRate();

    public static native boolean sendCursorShapeRequest(boolean enabled);

    public static native boolean sendControllerAudio(int kind, byte[] payload);

    public static native boolean sendTextInputRequest(boolean enabled);

    public static native boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz);

    public static native boolean sendStreamDvr(int requestId, int op, int durationSecs);

    public static native boolean sendKeyboardLayout(String bcp47Tag);

    public static native int clipboardSyncStart(String current);

    public static native int clipboardSyncLocal(String text);

    public static native int usbAttach(
            int fd, String path, String name, String address, int httpPort,
            int httpsPort, String serverCertificatePem, String uniqueId,
            byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8);

    public static native boolean usbDetach(int requestId);

    public static native String usbList();

    // --- Native API (native_api.rs) ---

    public static native void nativeApiSetSecondScreen(boolean secondScreen);

    public static native String nativeApiRequest(
            String op, String address, int httpPort, int httpsPort,
            String serverCertificatePem, String uniqueId,
            byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8,
            String argument);

    // --- File transfer (files.rs) ---

    public static native long fileTransferCreate();

    public static native void fileTransferDestroy(long handle);

    public static native long fileTransferQueueUpload(long handle, String localName, long size);

    public static native long fileTransferQueueDownload(long handle, String name, String localName, long size);

    public static native boolean fileTransferCancel(long handle, long id);

    public static native void fileTransferCancelAll(long handle);

    public static native boolean fileTransferRetry(long handle, long id);

    public static native void fileTransferClearFinished(long handle);

    public static native String fileTransferSnapshot(long handle);

    public static native String fileTransferStartNext(long handle);

    public static native String fileTransferList(
            String address, int httpPort, int httpsPort, String serverCertificatePem,
            String uniqueId, byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8);

    public static native String fileTransferRun(
            long handle, long id, String address, int httpPort, int httpsPort,
            String serverCertificatePem, String uniqueId, byte[] clientCertificateDer,
            byte[] clientPrivateKeyPkcs8, int fd, Object progress);

    // --- Echo canceller (echo.rs) ---

    public static native long echoCreate();

    public static native void echoReference(long handle, short[] pcm, int length, int channels, int outputMs);

    public static native boolean echoProcess(long handle, short[] pcm, int samples);

    public static native void echoRestart(long handle);

    public static native int[] echoStats(long handle);

    public static native void echoDestroy(long handle);

    // --- Policy / core JSON models ---

    public static String streamMenuCall(String request) {
        ensureLoaded();
        return nativeStreamMenuCall(request);
    }

    public static String simpleProfileCall(String request) {
        ensureLoaded();
        return nativeSimpleProfileCall(request);
    }

    public static String collectionGroupsCall(String request) {
        ensureLoaded();
        return nativeCollectionGroupsCall(request);
    }

    public static String libraryOrderCall(String request) {
        ensureLoaded();
        return nativeLibraryOrderCall(request);
    }

    public static String sharpColorCall(String request) {
        ensureLoaded();
        return nativeSharpColorCall(request);
    }

    public static String internetPlayCall(String request) {
        ensureLoaded();
        return nativeInternetPlayCall(request);
    }

    public static String openPlayCall(String request) {
        ensureLoaded();
        return nativeOpenPlayCall(request);
    }

    public static String gameInfoCall(String request) {
        ensureLoaded();
        return nativeGameInfoCall(request);
    }

    private static native String nativeStreamMenuCall(String request);

    private static native String nativeSimpleProfileCall(String request);

    private static native String nativeCollectionGroupsCall(String request);

    private static native String nativeLibraryOrderCall(String request);

    private static native String nativeSharpColorCall(String request);

    private static native String nativeInternetPlayCall(String request);

    private static native String nativeOpenPlayCall(String request);

    private static native String nativeGameInfoCall(String request);

    public static String capabilitiesCall(String request) {
        ensureLibraryLoaded();
        return nativeCapabilitiesCall(request);
    }

    public static String uriCall(String request) {
        ensureLibraryLoaded();
        return nativeUriCall(request);
    }

    private static native String nativeCapabilitiesCall(String request);

    private static native String nativeUriCall(String request);
}
